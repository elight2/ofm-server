//! 最小 SSE（`text/event-stream`）分帧器。
//!
//! 上游是标准 SSE：`id:N\nevent:<name>\ndata:<json>\n\n`，按空行切分。
//! 只需要事件名与 `data` 拼接，故不引入第三方库。

use bytes::Bytes;

#[derive(Debug, Default)]
struct Frame {
    event: Option<String>,
    data: String,
    has_data: bool,
}

impl Frame {
    fn take(&mut self) -> Option<SseEvent> {
        if !self.has_data {
            self.event = None;
            return None;
        }
        let event = self.event.take().unwrap_or_default();
        let data = std::mem::take(&mut self.data);
        self.has_data = false;
        Some(SseEvent { event, data })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
}

#[derive(Debug, Default)]
pub struct SseDecoder {
    buf: Vec<u8>,
    /// `buf` 里已消费到的位置。整轮解析完才一次性 `drain`，
    /// 避免每行都搬移整个缓冲（超长行 / 逐字节拆包时是平方级开销）。
    pos: usize,
    /// 本轮已完成的帧（`push_into` 结束时按序并入 `out`）。
    ready: Vec<SseEvent>,
    frame: Frame,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一段字节，把本次解析出的完整事件**追加**到 `out`。
    ///
    /// 由调用方提供（并在多帧间复用）缓冲区，省掉每次分配一个 `Vec`。
    pub fn push_into(&mut self, chunk: &Bytes, out: &mut Vec<SseEvent>) {
        self.buf.extend_from_slice(chunk);

        // 逐行处理（行尾可能是 \n 或 \r\n）。
        while let Some(off) = self.buf[self.pos..].iter().position(|b| *b == b'\n') {
            let end = self.pos + off;
            let start = self.pos;
            self.pos = end + 1;
            // 单行极少超长，拷贝一行比按字节就地解析更简单（只一次分配）。
            let line = self.buf[start..end].to_vec();
            self.feed_line(&line);
        }
        out.append(&mut self.ready);
        self.compact();
    }
    /// 喂入一段字节，返回本次解析出的完整事件（测试与一次性解析用）。
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn push(&mut self, chunk: &Bytes) -> Vec<SseEvent> {
        let mut out = Vec::new();
        self.push_into(chunk, &mut out);
        out
    }

    /// 流结束时冲掉残余帧。
    ///
    /// 上游偶有不以空行结尾，甚至最后一行没有 `\n`：须先把缓冲里剩下的
    /// 半行按行处理一遍，否则该字段会被静默丢弃。
    pub fn finish(&mut self) -> Option<SseEvent> {
        debug_assert!(self.ready.is_empty(), "finish 前应先取走已完成的帧");
        if self.pos < self.buf.len() {
            let line = self.buf[self.pos..].to_vec();
            self.pos = self.buf.len();
            self.feed_line(&line);
        }
        self.ready.clear();
        self.frame.take()
    }

    /// 丢掉已消费的前缀（只在整轮解析结束后做一次）。
    fn compact(&mut self) {
        if self.pos == 0 {
            return;
        }
        if self.pos == self.buf.len() {
            self.buf.clear();
        } else {
            self.buf.drain(..self.pos);
        }
        self.pos = 0;
    }

    /// 处理一行字段（已去行尾）。空行表示帧结束，由调用方处理。
    fn feed_line(&mut self, raw: &[u8]) {
        let line = std::str::from_utf8(raw)
            .unwrap_or("")
            .trim_end_matches('\r');
        if line.is_empty() {
            if let Some(ev) = self.frame.take() {
                self.ready.push(ev);
            }
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.frame.event = Some(value.to_string()),
            "data" => {
                if self.frame.has_data {
                    self.frame.data.push('\n');
                }
                self.frame.data.push_str(value);
                self.frame.has_data = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_two_events() {
        let mut d = SseDecoder::new();
        let chunk = Bytes::from_static(
            b"id:1\nevent:metadata\ndata:{\"a\":1}\n\nid:2\nevent:done\ndata:{}\n\n",
        );
        let evs = d.push(&chunk);
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event, "metadata");
        assert_eq!(evs[0].data, "{\"a\":1}");
        assert_eq!(evs[1].event, "done");
    }

    #[test]
    fn tolerates_split_and_non_json() {
        let mut d = SseDecoder::new();
        assert!(
            d.push(&Bytes::from_static(b"event:progress_notice\ndata:Proc"))
                .is_empty()
        );
        let evs = d.push(&Bytes::from_static(b"essing_1\n\n"));
        assert_eq!(evs[0].data, "Processing_1");
    }
}

#[cfg(test)]
mod trace_tests {
    use super::*;

    /// 用真实抓包（`tmp/trae-capture/t7.sse`）验证分帧：分块喂入也必须解出相同帧数。
    #[test]
    fn decodes_captured_trace() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tmp/trae-capture/t7.sse");
        let Ok(raw) = std::fs::read(path) else {
            eprintln!("跳过：缺少抓包 {path}");
            return;
        };

        // 整块喂入。
        let mut whole = SseDecoder::new();
        let mut events = whole.push(&Bytes::from(raw.clone()));
        events.extend(whole.finish());

        // 按 7 字节切块喂入（模拟 TCP 拆包）。
        let mut split = SseDecoder::new();
        let mut chunked = Vec::new();
        for chunk in raw.chunks(7) {
            chunked.extend(split.push(&Bytes::copy_from_slice(chunk)));
        }
        chunked.extend(split.finish());

        assert_eq!(events.len(), chunked.len());
        assert_eq!(events, chunked);
        assert!(events.iter().any(|e| e.event == "done"));
        assert!(events.iter().filter(|e| e.event == "output").count() > 5);
        assert!(!events.iter().any(|e| e.event.is_empty()));
    }
}
