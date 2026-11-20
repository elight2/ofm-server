//! 上游 SSE 帧 → 内部事件。
//!
//! 帧是**标准 OpenAI chunk**（协议见 `provider-info/workbuddy.md` §3.4）：
//! ```text
//! data: {"choices":[{"delta":{"reasoning_content":"..."},"index":0}]}
//! data: {"choices":[{"delta":{"content":"..."}}]}
//! data: {"choices":[{"delta":{"tool_calls":[{"index":0,...}]}}]}
//! data: {"choices":[{"finish_reason":"stop"}],"usage":{...}}
//! data: [DONE]
//! ```
//! 纯 `data:` 帧，无 `event:` 行。通用骨架见 [`crate::core::chunk`]。
//!
//! **非标准点**（§8.3）：流内可能出现**没有 `choices` 的错误帧**
//! `{"code":11140,"msg":"..."}`（HTTP 仍是 200）。这类帧必须与正常 chunk
//! 区分开，否则会被当成空 chunk 静默丢掉，用户只看到「没反应」。

use serde_json::Value;

use crate::core::chunk;
use crate::core::events::{ErrorKind, Event, UpstreamError};
use crate::core::sse::SseEvent;

/// 解析单帧，可能产出 0..n 个事件。
pub struct FrameParser;

impl FrameParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&mut self, ev: &SseEvent) -> Vec<Event> {
        chunk::parse(ev, false, error_of)
    }
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

/// 错误帧识别。
///
/// 判据（§8.3）：帧**没有 `choices`** 且带 `code`（或 `msg`+`displayMsg`）时
/// 视为错误帧。有 `choices` 的帧即使正文出现「审核」等字样也不是错误帧。
fn error_of(v: &Value) -> Option<UpstreamError> {
    let has_choices = v.get("choices").map(|c| c.is_array()).unwrap_or(false);
    if has_choices {
        return chunk::error_object(v, classify_error);
    }

    // OpenAI 风格的 `error` 对象。
    if v.get("error").is_some() {
        return chunk::error_object(v, classify_error);
    }

    // 腾讯系业务信封：`{"code":11140,"msg":"...","displayMsg":{...}}`。
    let code = v.get("code").and_then(|c| c.as_i64());
    let msg = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
    let display = v.get("displayMsg").and_then(super::api::display_msg_text);
    if code.is_none() && msg.is_empty() && display.is_none() {
        return None;
    }

    // `displayMsg` 是**面向用户**的文案，优先于 `msg` 的原始串。
    let text = if let Some(d) = display {
        d
    } else if !msg.is_empty() {
        msg.to_string()
    } else {
        "上游返回未知错误".to_string()
    };

    Some(UpstreamError {
        code: code.map(|c| c.to_string()),
        message: text,
        kind: classify_code(code),
    })
}

/// 业务码 → 错误分类（§8.2）。
fn classify_code(code: Option<i64>) -> ErrorKind {
    match code {
        // 渠道校验失败：疑似非官方客户端发起。
        Some(11128) => ErrorKind::InvalidRequest,
        // 账号被风控限制。
        Some(11140) => ErrorKind::InvalidRequest,
        // 模型已下线。
        Some(11141) => ErrorKind::InvalidRequest,
        Some(401) | Some(403) => ErrorKind::Authentication,
        Some(429) => ErrorKind::RateLimit,
        Some(400) => ErrorKind::InvalidRequest,
        _ => ErrorKind::Api,
    }
}

/// `error` 对象分类。
fn classify_error(err: &Value) -> ErrorKind {
    let code = err.get("code").map(chunk::code_text).unwrap_or_default();
    match code.as_str() {
        "401" | "403" => ErrorKind::Authentication,
        "429" => ErrorKind::RateLimit,
        "400" => ErrorKind::InvalidRequest,
        _ => match err.get("type").and_then(|t| t.as_str()) {
            Some("invalid_request_error") => ErrorKind::InvalidRequest,
            Some("authentication_error") => ErrorKind::Authentication,
            Some("rate_limit_error") => ErrorKind::RateLimit,
            _ => ErrorKind::Api,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::FinishReason;

    fn parse(s: &str) -> Vec<Event> {
        let ev = crate::core::sse::SseEvent {
            event: String::new(),
            data: s.to_string(),
        };
        FrameParser::new().parse(&ev)
    }

    #[test]
    fn 解析思考与正文() {
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"reasoning_content":"好的"}}]}"#);
        assert!(matches!(&ev[0], Event::Reasoning(s) if s == "好的"));
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"content":"你好"}}]}"#);
        assert!(matches!(&ev[0], Event::Text(s) if s == "你好"));
    }

    /// `reasoning` 字段（官方 harness 两者都读）。
    #[test]
    fn 解析reasoning字段() {
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"reasoning":"想一下"}}]}"#);
        assert!(matches!(&ev[0], Event::Reasoning(s) if s == "想一下"));
    }

    #[test]
    fn 解析工具调用分片() {
        let ev = parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"id":"call_1","function":{"arguments":"","name":"get_weather"},"type":"function","index":0}]}}]}"#,
        );
        match &ev[0] {
            Event::ToolCall(t) => {
                assert_eq!(t.index, 0);
                assert_eq!(t.id.as_deref(), Some("call_1"));
                assert_eq!(t.name.as_deref(), Some("get_weather"));
            }
            _ => panic!("应为 ToolCall"),
        }
    }

    #[test]
    fn finish_reason映射() {
        let ev = parse(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#);
        match &ev[0] {
            Event::Finish(f) => assert_eq!(*f, FinishReason::ToolCalls),
            _ => panic!("应为 Finish"),
        }
    }

    #[test]
    fn 解析usage() {
        let ev = parse(
            r#"{"choices":[],"usage":{"prompt_tokens":28,"completion_tokens":32,"total_tokens":60,"prompt_cache_hit_tokens":4,"completion_tokens_details":{"reasoning_tokens":29}}}"#,
        );
        match &ev[0] {
            Event::Usage(u) => {
                assert_eq!(u.prompt_tokens, 28);
                assert_eq!(u.completion_tokens, 32);
                assert_eq!(u.total_tokens, 60);
                assert_eq!(u.reasoning_tokens, Some(29));
                assert_eq!(u.cached_tokens, Some(4));
            }
            _ => panic!("应为 Usage"),
        }
    }

    /// 上游用**空串** `""` 表示「本帧未结束」，不能当成终止帧，
    /// 否则第一帧之后的全部内容都会被丢弃。
    #[test]
    fn 空串finish_reason不当终止() {
        let ev = parse(
            r#"{"choices":[{"index":0,"delta":{"reasoning_content":"The"},"finish_reason":""}],"usage":null}"#,
        );
        assert_eq!(ev.len(), 1, "只应有 reasoning 事件: {ev:?}");
        assert!(matches!(&ev[0], Event::Reasoning(s) if s == "The"));
    }

    /// `usage: null`（上游每帧都带）不产生 usage 事件。
    #[test]
    fn 空usage不产事件() {
        let ev = parse(
            r#"{"choices":[{"index":0,"delta":{"content":"x"},"finish_reason":""}],"usage":null}"#,
        );
        assert_eq!(ev.len(), 1, "只应有 text 事件: {ev:?}");
        assert!(matches!(&ev[0], Event::Text(s) if s == "x"));
    }

    /// 真正的终止帧（`finish_reason: "stop"`）仍要产出 Finish。
    #[test]
    fn 真终止帧仍产出finish() {
        let ev = parse(
            r#"{"choices":[{"index":0,"delta":{"content":""},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":2,"total_tokens":3}}"#,
        );
        assert!(ev.iter().any(|e| matches!(e, Event::Finish(_))));
        assert!(ev.iter().any(|e| matches!(e, Event::Usage(_))));
    }

    #[test]
    fn done不产事件() {
        assert!(parse("[DONE]").is_empty());
    }

    /// 流内错误帧（无 choices + code）。
    #[test]
    fn 流内业务错误帧() {
        let ev = parse(r#"{"code":11140,"msg":"账号异常活动限制"}"#);
        match &ev[0] {
            Event::Error(e) => {
                assert_eq!(e.kind, ErrorKind::InvalidRequest);
                assert_eq!(e.code.as_deref(), Some("11140"));
                assert_eq!(e.message, "账号异常活动限制");
            }
            _ => panic!("应为 Error"),
        }
    }

    /// `displayMsg` 优先于 `msg`。
    #[test]
    fn display_msg_被识别() {
        let ev =
            parse(r#"{"code":11141,"msg":"raw","displayMsg":{"zh":"模型已下线","en":"gone"}}"#);
        match &ev[0] {
            Event::Error(e) => {
                assert_eq!(e.kind, ErrorKind::InvalidRequest);
                assert_eq!(e.message, "模型已下线");
            }
            _ => panic!("应为 Error"),
        }
    }

    /// 有 choices 的帧即便带 code 也不是错误帧。
    #[test]
    fn 有choices不误判为错误() {
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"content":"审核"}}]}"#);
        assert!(matches!(&ev[0], Event::Text(_)));
    }

    #[test]
    fn error框分类() {
        let ev = parse(r#"{"error":{"code":400,"message":"参数错误"}}"#);
        match &ev[0] {
            Event::Error(e) => assert_eq!(e.kind, ErrorKind::InvalidRequest),
            _ => panic!(),
        }
    }

    #[test]
    fn 非json帧产生错误事件() {
        assert!(matches!(&parse("not json")[0], Event::Error(_)));
    }
}
