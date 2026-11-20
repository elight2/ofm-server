//! CodeArts SSE 帧 → 内部事件。
//!
//! 上游是标准 OpenAI CC chunk，但错误可能以 `error` 或顶层
//! `error_code`/`error_msg` 出现（HTTP 200 也可能）。本解析器按内核
//! `aU.doStream` 的判据：H 分支任一错误键即错误；这里统一兼容两者。
//! 通用骨架见 [`crate::core::chunk`]。

use serde_json::Value;

use crate::core::chunk;
use crate::core::events::{ErrorKind, Event, UpstreamError};
use crate::core::sse::SseEvent;

pub struct FrameParser;

impl FrameParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&mut self, ev: &SseEvent) -> Vec<Event> {
        // `id` 顶层优先（与 loomy 同），骨架里由第二个参数指定。
        chunk::parse(ev, false, error_of)
    }
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

/// `error` 对象优先；否则看顶层 `error_code`/`error_msg`。
fn error_of(v: &Value) -> Option<UpstreamError> {
    if let Some(err) = chunk::error_object(v, |e| classify(&e.to_string())) {
        return Some(err);
    }
    if v.get("error_code").is_none() && v.get("error_msg").is_none() {
        return None;
    }
    let code = v.get("error_code").map(chunk::code_text);
    Some(UpstreamError {
        kind: classify(code.as_deref().unwrap_or("")),
        code,
        message: v
            .get("error_msg")
            .and_then(|m| m.as_str())
            .unwrap_or("上游返回错误")
            .to_string(),
    })
}

fn classify(code: &str) -> ErrorKind {
    let c = code.to_ascii_lowercase();
    if c.contains("4291") || c.contains("429") || c.contains("quota") || c.contains("limit") {
        ErrorKind::RateLimit
    } else if c.contains("002002009") || c.contains("4004") || c.contains("400") {
        ErrorKind::InvalidRequest
    } else if c.contains("apig.0602") || c.contains("401") || c.contains("403") {
        ErrorKind::Authentication
    } else {
        ErrorKind::Api
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::FinishReason;

    fn parse(s: &str) -> Vec<Event> {
        FrameParser::new().parse(&SseEvent {
            event: String::new(),
            data: s.to_string(),
        })
    }

    #[test]
    fn parses_content_and_reasoning() {
        let ev = parse(r#"{"choices":[{"delta":{"reasoning_content":"想","content":"答"}}]}"#);
        assert!(matches!(&ev[0], Event::Reasoning(s) if s == "想"));
        assert!(matches!(&ev[1], Event::Text(s) if s == "答"));
    }

    #[test]
    fn parses_error_code_frame() {
        let ev = parse(r#"{"error_code":"InferHub.ModelArts.81111.429","error_msg":"limit"}"#);
        match &ev[0] {
            Event::Error(e) => {
                assert_eq!(e.code.as_deref(), Some("InferHub.ModelArts.81111.429"));
                assert_eq!(e.kind, ErrorKind::RateLimit);
            }
            _ => panic!("应为 Error"),
        }
    }

    #[test]
    fn parses_tool_call_and_finish() {
        let ev = parse(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"f","arguments":"{"}}]},"finish_reason":"tool_calls"}]}"#,
        );
        assert!(matches!(&ev[0], Event::ToolCall(t) if t.id.as_deref()==Some("call_1")));
        assert!(matches!(&ev[1], Event::Finish(FinishReason::ToolCalls)));
    }

    #[test]
    fn parses_usage_with_cache() {
        let ev = parse(
            r#"{"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":3,"total_tokens":13,"prompt_cache_hit_tokens":4,"completion_tokens_details":{"reasoning_tokens":1}}}"#,
        );
        match &ev[0] {
            Event::Usage(u) => {
                assert_eq!(u.cached_tokens, Some(4));
                assert_eq!(u.reasoning_tokens, Some(1));
            }
            _ => panic!("应为 Usage"),
        }
    }
}
