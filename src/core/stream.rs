//! 把上游 SSE 字节流转成中间事件流。
//!
//! 各供应商的差异只在「怎么把一帧翻译成事件」，其余（分帧、待发队列、
//! 终止态收尾、断流补 `Finish`）完全一致，故在此统一实现，避免每家复制一份。

use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::Bytes;
use futures_util::Stream;

use super::events::{ErrorKind, Event, EventStream, FinishReason, UpstreamError};
use super::sse::{SseDecoder, SseEvent};

/// 驱动一个 SSE 字节流，产出 [`EventStream`]。
///
/// `parse` 把单帧翻译成 0..n 个事件；解析器自身状态（如累积中的 tool_calls）
/// 由调用方通过闭包捕获。
///
/// 三条收尾规则：
/// * 已产出 [`Event::Finish`] / [`Event::Error`] 后结束该事件流的读取；
/// * 字节流读完时先冲掉解码器里的残余帧；
/// * 上游没发终止事件就断流 → 补 [`Event::Finish(Stop)`]，避免下游挂住。
pub fn drive<S, F>(bytes: S, parse: F) -> EventStream
where
    S: Stream<Item = reqwest::Result<Bytes>> + Send + 'static,
    F: FnMut(&SseEvent) -> Vec<Event> + Send + 'static,
{
    Box::pin(Driver {
        bytes: Box::pin(bytes),
        parse,
        decoder: SseDecoder::new(),
        pending: VecDeque::new(),
        finished: false,
        saw_terminal: false,
    })
}

struct Driver<S, F> {
    bytes: Pin<Box<S>>,
    parse: F,
    decoder: SseDecoder,
    /// 解析出的一批事件，逐条吐出。
    pending: VecDeque<Event>,
    /// 已产出终止事件，不再读流。
    finished: bool,
    /// 是否见过终止事件（决定断流时要不要补 `Finish`）。
    saw_terminal: bool,
}

// `F` 内含 `Pin<Box<S>>` 之外的可自由移动字段，故手动声明 Unpin 以免 get_mut 受限。
impl<S, F> Unpin for Driver<S, F> {}

impl<S, F> Stream for Driver<S, F>
where
    S: Stream<Item = reqwest::Result<Bytes>>,
    F: FnMut(&SseEvent) -> Vec<Event>,
{
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Event>> {
        // 本类型各字段均无需 pin 投影（S 已装箱为 Pin<Box<S>>）。
        let this = self.get_mut();

        loop {
            if let Some(ev) = this.pending.pop_front() {
                if matches!(ev, Event::Finish(_) | Event::Error(_)) {
                    this.saw_terminal = true;
                }
                return Poll::Ready(Some(ev));
            }
            if this.finished {
                return Poll::Ready(None);
            }

            match this.bytes.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    for frame in this.decoder.push(&chunk) {
                        let evs = (this.parse)(&frame);
                        this.pending.extend(evs);
                    }
                }
                Poll::Ready(Some(Err(e))) => {
                    this.finished = true;
                    return Poll::Ready(Some(Event::Error(UpstreamError {
                        code: None,
                        message: format!("读取上游流失败: {e}"),
                        kind: ErrorKind::Api,
                    })));
                }
                Poll::Ready(None) => {
                    // 流结束：先冲掉残余帧。
                    if let Some(frame) = this.decoder.finish() {
                        let evs = (this.parse)(&frame);
                        this.pending.extend(evs);
                    }
                    if this.pending.is_empty() {
                        this.finished = true;
                        // 上游未发终止事件就断流：按 stop 收尾。
                        if !this.saw_terminal {
                            return Poll::Ready(Some(Event::Finish(FinishReason::Stop)));
                        }
                        return Poll::Ready(None);
                    }
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::Event;

    /// 造一段 SSE 字节流。
    fn feed(chunks: Vec<&'static str>) -> impl Stream<Item = reqwest::Result<Bytes>> + Send {
        futures_util::stream::iter(
            chunks
                .into_iter()
                .map(|s| Ok(Bytes::from_static(s.as_bytes()))),
        )
    }

    /// 把帧的 data 原样变成 Text 事件，便于断言顺序。
    fn as_text(ev: &SseEvent) -> Vec<Event> {
        vec![Event::Text(ev.data.clone())]
    }

    async fn collect(s: EventStream) -> Vec<Event> {
        futures_util::StreamExt::collect::<Vec<_>>(s).await
    }

    #[tokio::test]
    async fn 正常帧按序产出并补stop() {
        let s = drive(feed(vec!["data: a\n\n", "data: b\n\n"]), as_text);
        let evs = collect(s).await;
        // 上游未发终止事件 → 末尾补一个 Finish(Stop)。
        assert_eq!(evs.len(), 3);
        assert!(matches!(&evs[0], Event::Text(t) if t == "a"));
        assert!(matches!(&evs[1], Event::Text(t) if t == "b"));
        assert!(matches!(evs[2], Event::Finish(FinishReason::Stop)));
    }

    #[tokio::test]
    async fn 见过finish后不再补stop() {
        let s = drive(feed(vec!["data: a\n\n"]), |ev| {
            vec![
                Event::Text(ev.data.clone()),
                Event::Finish(FinishReason::Stop),
            ]
        });
        let evs = collect(s).await;
        // 恰好一个 Finish，不能追加第二个。
        let n = evs
            .iter()
            .filter(|e| matches!(e, Event::Finish(_)))
            .count();
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn 跨块分帧不丢事件() {
        // 一帧被切成两段喂入。
        let s = drive(feed(vec!["data: he", "llo\n\n"]), as_text);
        let evs = collect(s).await;
        assert!(matches!(&evs[0], Event::Text(t) if t == "hello"), "实际: {evs:?}");
    }

    #[tokio::test]
    async fn 结束前冲掉残余帧() {
        // 末尾帧没有空行结束符，靠 decoder.finish() 冲出。
        let s = drive(feed(vec!["data: tail"]), as_text);
        let evs = collect(s).await;
        assert!(matches!(&evs[0], Event::Text(t) if t == "tail"), "实际: {evs:?}");
    }
}
