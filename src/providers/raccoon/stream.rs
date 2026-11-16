//! 上游 SSE 帧 → 内部事件。
//!
//! 上游帧是**标准 OpenAI chunk**（实测见 `provider-info/raccoon.md` §3）：
//! ```text
//! data: {"choices":[{"delta":{"reasoning_content":"..."},"index":0}],...}
//! data: {"choices":[{"delta":{"tool_calls":[{...,"index":0}]}}]}
//! data: {"choices":[{"finish_reason":"stop"}],"usage":{...}}
//! data: [DONE]
//! ```
//! 无 `event:` 行，纯 `data:` 帧。

use serde_json::Value;

use crate::core::events::{ErrorKind, Event, FinishReason, ToolCallDelta, UpstreamError, Usage};

/// 解析单帧 `data`，可能产出 0..n 个事件。
pub struct FrameParser;

impl FrameParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&mut self, ev: &crate::core::sse::SseEvent) -> Vec<Event> {
        let data = ev.data.as_str();
        let data = data.trim();
        if data.is_empty() {
            return Vec::new();
        }
        if data == "[DONE]" {
            // 终止由下游统一处理；这里不额外发事件。
            return Vec::new();
        }
        let v: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                return vec![Event::Error(UpstreamError {
                    code: None,
                    message: format!("上游返回非 JSON 帧: {e}"),
                    kind: ErrorKind::Api,
                })]
            }
        };

        // 业务错误可能以帧内 error 字段出现。
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
                kind: classify_error(err),
            })];
        }

        let mut out = Vec::new();

        if let Some(choices) = v.get("choices").and_then(|c| c.as_array()) {
            for ch in choices {
                if let Some(delta) = ch.get("delta") {
                    if let Some(r) = delta.get("reasoning_content").and_then(|x| x.as_str())
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
                    out.push(Event::Finish(map_finish(fr)));
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
    // 上游在后续分片里会给出 `"id":""`、`"name":""`，空串等同缺失。
    let id = f
        .and_then(|x| x.get("id"))
        .or_else(|| tc.get("id"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let name = f
        .and_then(|x| x.get("name"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let arguments = f
        .and_then(|x| x.get("arguments"))
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
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
            .and_then(|x| x.as_u64()),
        reasoning_tokens: u
            .get("completion_tokens_details")
            .and_then(|d| d.get("reasoning_tokens"))
            .and_then(|x| x.as_u64()),
    }
}

fn map_finish(s: &str) -> FinishReason {
    match s {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "content_filter" => FinishReason::ContentFilter,
        "tool_calls" | "function_call" => FinishReason::ToolCalls,
        _ => FinishReason::Other,
    }
}

/// 错误分类。上游错误码见 `provider-info/raccoon.md` §6。
fn classify_error(err: &Value) -> ErrorKind {
    // 码可能是数字或字符串。
    let code = err
        .get("code")
        .map(|c| c.to_string().trim_matches('"').to_string())
        .unwrap_or_default();
    match code.as_str() {
        // 点数不足
        "1000007" => ErrorKind::RateLimit,
        // 授权/身份失效
        "200003" | "200009" | "200022" | "200201" => ErrorKind::Authentication,
        "400" | "invalid_request_error" => ErrorKind::InvalidRequest,
        _ => {
            // 无码时按 type 粗略判断。
            match err.get("type").and_then(|t| t.as_str()) {
                Some("invalid_request_error") => ErrorKind::InvalidRequest,
                Some("authentication_error") => ErrorKind::Authentication,
                Some("rate_limit_error") => ErrorKind::RateLimit,
                _ => ErrorKind::Api,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试辅助：只关心 `data`，事件名对 raccoon 恒为空。
    fn parse(s: &str) -> Vec<Event> {
        let ev = crate::core::sse::SseEvent {
            event: String::new(),
            data: s.to_string(),
        };
        FrameParser::new().parse(&ev)
    }

    /// 真实首帧：delta 只有 role。
    #[test]
    fn 首帧只有role不产事件() {
        let ev = parse(
            r#"{"id":"a","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#,
        );
        assert!(ev.is_empty());
    }

    /// 思考增量。
    #[test]
    fn 解析reasoning_content() {
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"reasoning_content":"好的"}}]}"#);
        assert!(matches!(&ev[0], Event::Reasoning(s) if s == "好的"));
    }

    /// 正文增量。
    #[test]
    fn 解析content() {
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"content":"你好"}}]}"#);
        assert!(matches!(&ev[0], Event::Text(s) if s == "你好"));
    }

    /// 空串 delta 不产事件（上游会发 `"content":""`）。
    #[test]
    fn 空串不产事件() {
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"content":"","reasoning_content":""}}]}"#);
        assert!(ev.is_empty());
    }

    /// 真实工具调用分片：首帧带 id/name，后续 arguments 分片。
    #[test]
    fn 解析工具调用分片() {
        let ev = parse(
            r#"{"choices":[{"index":0,"delta":{"content":"","tool_calls":[{"id":"call_1","function":{"arguments":"","name":"get_weather"},"type":"function","index":0}]}}]}"#,
        );
        match &ev[0] {
            Event::ToolCall(t) => {
                assert_eq!(t.index, 0);
                assert_eq!(t.id.as_deref(), Some("call_1"));
                assert_eq!(t.name.as_deref(), Some("get_weather"));
                assert_eq!(t.arguments, None);
            }
            _ => panic!("应为 ToolCall"),
        }

        let ev = parse(
            r#"{"choices":[{"index":0,"delta":{"content":"","tool_calls":[{"id":"","function":{"arguments":"{\"city\"","name":""},"type":"function","index":0}]}}]}"#,
        );
        match &ev[0] {
            Event::ToolCall(t) => {
                // 空串 id/name 视为缺失，不覆盖已累积的值
                assert_eq!(t.id, None);
                assert_eq!(t.name, None);
                assert_eq!(t.arguments.as_deref(), Some(r#"{"city""#));
            }
            _ => panic!("应为 ToolCall"),
        }
    }

    /// finish_reason 映射。
    #[test]
    fn 解析finish_reason() {
        let cases = [
            ("stop", FinishReason::Stop),
            ("length", FinishReason::Length),
            ("tool_calls", FinishReason::ToolCalls),
            ("content_filter", FinishReason::ContentFilter),
            ("weird", FinishReason::Other),
        ];
        for (s, want) in cases {
            let ev = parse(&format!(
                r#"{{"choices":[{{"index":0,"delta":{{}},"finish_reason":"{s}"}}]}}"#
            ));
            match &ev[0] {
                Event::Finish(f) => assert_eq!(*f, want, "finish_reason={s}"),
                _ => panic!("应为 Finish"),
            }
        }
    }

    /// usage 帧含 reasoning/cached tokens。
    #[test]
    fn 解析usage() {
        let ev = parse(
            r#"{"choices":[],"usage":{"completion_tokens":171,"prompt_tokens":61,"total_tokens":232,"completion_tokens_details":{"reasoning_tokens":167},"prompt_tokens_details":{"cached_tokens":0}}}"#,
        );
        match &ev[0] {
            Event::Usage(u) => {
                assert_eq!(u.prompt_tokens, 61);
                assert_eq!(u.completion_tokens, 171);
                assert_eq!(u.total_tokens, 232);
                assert_eq!(u.reasoning_tokens, Some(167));
                assert_eq!(u.cached_tokens, Some(0));
            }
            _ => panic!("应为 Usage"),
        }
    }

    /// [DONE] 不产事件。
    #[test]
    fn done不产事件() {
        assert!(parse("[DONE]").is_empty());
    }

    /// 帧内 error 框：点数不足归为限流。
    #[test]
    fn 错误框分类() {
        let ev = parse(r#"{"error":{"code":1000007,"message":"insufficient points"}}"#);
        match &ev[0] {
            Event::Error(e) => {
                assert_eq!(e.code.as_deref(), Some("1000007"));
                assert_eq!(e.kind, ErrorKind::RateLimit);
            }
            _ => panic!("应为 Error"),
        }
    }

    /// 认证类错误码。
    #[test]
    fn 认证错误分类() {
        let ev = parse(r#"{"error":{"code":200003,"message":"authorization verify failed"}}"#);
        match &ev[0] {
            Event::Error(e) => assert_eq!(e.kind, ErrorKind::Authentication),
            _ => panic!(),
        }
    }

    /// 无码时按 type 判断。
    #[test]
    fn 无码按type分类() {
        let ev = parse(r#"{"error":{"type":"invalid_request_error","message":"bad"}}"#);
        match &ev[0] {
            Event::Error(e) => assert_eq!(e.kind, ErrorKind::InvalidRequest),
            _ => panic!(),
        }
    }

    /// 非 JSON 帧不 panic，转为错误事件。
    #[test]
    fn 非json帧产生错误事件() {
        let ev = parse("not json");
        assert!(matches!(&ev[0], Event::Error(_)));
    }

    /// 未知字段不影响解析。
    #[test]
    fn 忽略未知字段() {
        let ev = parse(r#"{"request_id":"x","choices":[{"index":0,"delta":{"content":"a"}}],"extra":1}"#);
        assert!(matches!(&ev[0], Event::Text(s) if s == "a"));
    }
}
