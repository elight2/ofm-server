//! 标准 OpenAI chat-completions chunk 的公共解析骨架。
//!
//! raccoon / codearts / loomy 的上游都是同一个形状的 SSE 帧：
//! ```text
//! data: {"choices":[{"delta":{"reasoning_content":"…","content":"…"}}]}
//! data: {"choices":[{"delta":{"tool_calls":[{"index":0,…}]}}]}
//! data: {"choices":[{"finish_reason":"stop"}],"usage":{…}}
//! data: [DONE]
//! ```
//! 三家唯一的实质差异是**错误帧怎么认、怎么分类**：
//!
//! * raccoon：帧内 `error` 对象，按错误码/类型分类；
//! * codearts：帧内 `error`，或顶层 `error_code`/`error_msg`；
//! * loomy：先是 HTTP 200 + `{code,desc}` 业务信封，然后才是 `error`。
//!
//! 所以这里只固定「解析 JSON → 认错误（交给调用方的钩子）→ 抽 delta/usage」，
//! 错误识别与分类留给各家，避免把语义揉成一个大的 `if`。
//!
//! trae 与 minimax 不走这里：前者是自己的一套帧格式，后者是 anthropic-messages
//! 有状态事件流，协议本身不同。

use serde_json::Value;

use crate::core::events::{ErrorKind, Event, ToolCallDelta, UpstreamError, Usage};
use crate::core::sse::SseEvent;

/// 解析单帧，可能产出 0..n 个事件。
///
/// `error_of` 是各家的错误钩子：给定整帧，返回 `Some(上游错误)` 表示这帧是错误帧。
/// 返回 `None` 则按正常 chunk 抽取内容。
pub fn parse(
    ev: &SseEvent,
    tc_id_in_function: bool,
    error_of: impl Fn(&Value) -> Option<UpstreamError>,
) -> Vec<Event> {
    let data = ev.data.trim();
    if data.is_empty() || data == "[DONE]" {
        // 终止由 `core::stream::drive` 统一收尾。
        return Vec::new();
    }

    let v: Value = match serde_json::from_str(data) {
        Ok(v) => v,
        Err(e) => {
            return vec![Event::Error(UpstreamError {
                code: None,
                message: format!("上游返回非 JSON 帧: {e}"),
                kind: ErrorKind::Api,
            })];
        }
    };

    if let Some(err) = error_of(&v) {
        return vec![Event::Error(err)];
    }

    parse_chunk(&v, tc_id_in_function)
}

/// 从一个已确认正常的 chunk 里抽事件。
///
/// 公开出来供需要先做自有判断（如 minimax）的供应商复用 delta/usage 抽取。
pub fn parse_chunk(v: &Value, tc_id_in_function: bool) -> Vec<Event> {
    let mut out = Vec::new();

    if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
        for ch in choices {
            if let Some(delta) = ch.get("delta") {
                if let Some(r) = reasoning_of(delta)
                    && !r.is_empty()
                {
                    out.push(Event::Reasoning(r.to_string()));
                }
                if let Some(c) = delta.get("content").and_then(|x| x.as_str())
                    && !c.is_empty()
                {
                    out.push(Event::Text(c.to_string()));
                }
                if let Some(tcs) = delta.get("tool_calls").and_then(|x| x.as_array()) {
                    for tc in tcs {
                        out.push(Event::ToolCall(parse_tool_call(tc, tc_id_in_function)));
                    }
                }
            }
            // `finish_reason` 为**空串**等同于缺失（上游用 `""` 表示「本帧未结束」，
            // 只有 `null`/字段不存在才是真结束）。误判成终止会导致后续帧全被丢弃。
            if let Some(fr) = ch.get("finish_reason").and_then(|x| x.as_str())
                && !fr.is_empty()
            {
                out.push(Event::Finish(crate::core::stream::map_finish(fr)));
            }
        }
    }

    // `usage` 为 `null`（上游每帧都带）时不当成一次 usage 事件。
    if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
        out.push(Event::Usage(parse_usage(u)));
    }

    out
}

/// 思考增量字段：标准名 `reasoning_content`，codearts 偶尔用 `reasoning`。
fn reasoning_of(delta: &Value) -> Option<&str> {
    delta
        .get("reasoning_content")
        .or_else(|| delta.get("reasoning"))
        .and_then(|x| x.as_str())
}

/// 帧内 `error` 对象 → 上游错误。三家的兜底形态一致，分类交给 `classify`。
pub fn error_object(v: &Value, classify: impl Fn(&Value) -> ErrorKind) -> Option<UpstreamError> {
    let err = v.get("error")?;
    Some(UpstreamError {
        code: err.get("code").map(code_text),
        message: err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("上游返回错误")
            .to_string(),
        kind: classify(err),
    })
}

/// 错误码的统一文本形式（上游可能给数字或字符串）。
pub fn code_text(c: &Value) -> String {
    c.to_string().trim_matches('"').to_string()
}

/// 工具调用增量。
///
/// 各家的 `id` 位置不同：raccoon 优先 `function.id`，codearts/loomy 优先顶层 `id`；
/// `function` 优先与否由 `id_in_function_first` 指定。分片里会给 `"id":""`、
/// `"name":""`，空串等同缺失，否则会覆盖已累积的值。
pub fn parse_tool_call(tc: &Value, id_in_function_first: bool) -> ToolCallDelta {
    let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
    let f = tc.get("function");
    let fid = || f.and_then(|x| x.get("id"));
    let id = if id_in_function_first {
        fid().or_else(|| tc.get("id"))
    } else {
        tc.get("id").or_else(fid)
    }
    .and_then(|x| x.as_str())
    .filter(|s| !s.is_empty())
    .map(str::to_string);
    let name = f
        .and_then(|x| x.get("name"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let arguments = f
        .and_then(|x| x.get("arguments"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    ToolCallDelta {
        index,
        id,
        name,
        arguments,
    }
}

/// 用量统计。`prompt_cache_hit_tokens` 是 codearts 的缓存命中别名。
pub fn parse_usage(u: &Value) -> Usage {
    let n = |k: &str| u.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    Usage {
        prompt_tokens: n("prompt_tokens"),
        completion_tokens: n("completion_tokens"),
        total_tokens: n("total_tokens"),
        cached_tokens: u
            .get("prompt_tokens_details")
            .and_then(|d| d.get("cached_tokens"))
            .and_then(|x| x.as_u64())
            .or_else(|| u.get("prompt_cache_hit_tokens").and_then(|x| x.as_u64())),
        reasoning_tokens: u
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(|x| x.as_u64()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::FinishReason;

    fn none(_: &Value) -> Option<UpstreamError> {
        None
    }

    fn parse_plain(s: &str) -> Vec<Event> {
        parse(
            &SseEvent {
                event: String::new(),
                data: s.to_string(),
            },
            false,
            none,
        )
    }

    #[test]
    fn 空帧与_done_不产事件() {
        assert!(parse_plain("").is_empty());
        assert!(parse_plain("[DONE]").is_empty());
    }

    #[test]
    fn 抽出思考正文与工具调用() {
        let ev = parse_plain(
            r#"{"choices":[{"delta":{"reasoning_content":"想","content":"答","tool_calls":[{"index":0,"id":"c1","function":{"name":"f","arguments":"{"}}]},"finish_reason":"tool_calls"}]}"#,
        );
        assert!(matches!(&ev[0], Event::Reasoning(s) if s == "想"));
        assert!(matches!(&ev[1], Event::Text(s) if s == "答"));
        assert!(matches!(&ev[2], Event::ToolCall(t) if t.id.as_deref() == Some("c1")));
        assert!(matches!(&ev[3], Event::Finish(FinishReason::ToolCalls)));
    }

    #[test]
    fn reasoning_别名也认() {
        let ev = parse_plain(r#"{"choices":[{"delta":{"reasoning":"想"}}]}"#);
        assert!(matches!(&ev[0], Event::Reasoning(s) if s == "想"));
    }

    #[test]
    fn 非_json_帧报_api_错误() {
        let ev = parse_plain("not json");
        assert!(matches!(&ev[0], Event::Error(e) if e.kind == ErrorKind::Api));
    }

    #[test]
    fn 用量含缓存与思考_token() {
        let ev = parse_plain(
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"prompt_tokens_details":{"cached_tokens":2},"prompt_cache_hit_tokens":4,"completion_tokens_details":{"reasoning_tokens":1}}}"#,
        );
        match &ev[0] {
            // `prompt_tokens_details` 优先于别名。
            Event::Usage(u) => {
                assert_eq!(u.cached_tokens, Some(2));
                assert_eq!(u.reasoning_tokens, Some(1));
            }
            _ => panic!("应为 Usage"),
        }
    }

    #[test]
    fn 错误钩子可拦截整帧() {
        let ev = parse(
            &SseEvent {
                event: String::new(),
                data: r#"{"error":{"code":429,"message":"限流"}}"#.to_string(),
            },
            false,
            |v| error_object(v, |_| ErrorKind::RateLimit),
        );
        assert!(matches!(&ev[0], Event::Error(e) if e.kind == ErrorKind::RateLimit));
    }

    #[test]
    fn 工具调用_id_位置可切换() {
        let tc: Value =
            serde_json::from_str(r#"{"index":1,"id":"top","function":{"id":"inner","name":"f"}}"#)
                .unwrap();
        assert_eq!(
            parse_tool_call(&tc, true).id.as_deref(),
            Some("inner"),
            "function 优先"
        );
        assert_eq!(parse_tool_call(&tc, false).id.as_deref(), Some("top"));
    }
}
