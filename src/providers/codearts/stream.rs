//! CodeArts SSE 帧 → 内部事件。
//!
//! 上游是标准 OpenAI CC chunk，但错误可能以 `error` 或顶层
//! `error_code`/`error_msg` 出现（HTTP 200 也可能）。本解析器按内核
//! `aU.doStream` 的判据：H 分支任一错误键即错误；这里统一兼容两者。

use serde_json::Value;

use crate::core::events::{ErrorKind, Event, ToolCallDelta, UpstreamError, Usage};
use crate::core::sse::SseEvent;

pub struct FrameParser;

impl FrameParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&mut self, ev: &SseEvent) -> Vec<Event> {
        let data = ev.data.trim();
        if data.is_empty() || data == "[DONE]" {
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

        // `error` 对象优先；否则看顶层 error_code/error_msg。
        if let Some(err) = v.get("error") {
            return vec![Event::Error(UpstreamError {
                code: err
                    .get("code")
                    .map(|c| c.to_string().trim_matches('"').to_string()),
                message: err
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("上游返回错误")
                    .to_string(),
                kind: classify(&err.to_string()),
            })];
        }
        if v.get("error_code").is_some() || v.get("error_msg").is_some() {
            let code = v
                .get("error_code")
                .map(|c| c.to_string().trim_matches('"').to_string());
            let message = v
                .get("error_msg")
                .and_then(|m| m.as_str())
                .unwrap_or("上游返回错误")
                .to_string();
            return vec![Event::Error(UpstreamError {
                kind: classify(code.as_deref().unwrap_or("")),
                code,
                message,
            })];
        }

        let mut out = Vec::new();
        if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
            for ch in choices {
                if let Some(delta) = ch.get("delta") {
                    if let Some(r) = delta
                        .get("reasoning_content")
                        .or_else(|| delta.get("reasoning"))
                        .and_then(|x| x.as_str())
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
                            out.push(Event::ToolCall(parse_tool_call(tc)));
                        }
                    }
                }
                if let Some(fr) = ch.get("finish_reason").and_then(|x| x.as_str()) {
                    out.push(Event::Finish(crate::core::stream::map_finish(fr)));
                }
            }
        }
        if let Some(u) = v.get("usage") {
            out.push(Event::Usage(parse_usage(u)));
        }
        out
    }
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

fn parse_tool_call(tc: &Value) -> ToolCallDelta {
    let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as u32;
    let f = tc.get("function");
    let id = tc
        .get("id")
        .or_else(|| f.and_then(|x| x.get("id")))
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

fn parse_usage(u: &Value) -> Usage {
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
