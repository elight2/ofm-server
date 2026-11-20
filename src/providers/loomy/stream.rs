//! 上游 SSE 帧 → 内部事件。
//!
//! 帧是**标准 OpenAI chunk**（实测见 `provider-info/loomy.md` §3.7）：
//! ```text
//! data: {"choices":[{"delta":{"reasoning_content":"..."},"index":0}],...}
//! data: {"choices":[{"delta":{"tool_calls":[{...,"index":0}]}}]}
//! data: {"choices":[{"finish_reason":"stop"}],"usage":{...}}
//! data: [DONE]
//! ```
//! 纯 `data:` 帧，无 `event:` 行。
//!
//! 但有个**非标准点**：认证失效不是 HTTP 401，而是 HTTP 200 + 顶层
//! `{"code":"100002","desc":"登录已失效，请重新登录"}` 的非 SSE 响应体。
//! 该响应体会被 SSE 解码器当成 `data:` 帧送进来（`chat` 里的预检也会兜一层）。

use serde_json::Value;

use crate::core::events::{ErrorKind, Event, ToolCallDelta, UpstreamError, Usage};

/// 解析单帧，可能产出 0..n 个事件。
pub struct FrameParser;

impl FrameParser {
    pub fn new() -> Self {
        Self
    }

    pub fn parse(&mut self, ev: &crate::core::sse::SseEvent) -> Vec<Event> {
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

        // 业务错误信封（认证失效即此形态，HTTP 仍是 200）。
        if let Some((code, desc)) = super::api::biz_error(&v) {
            return vec![Event::Error(UpstreamError {
                message: desc,
                kind: classify_code(&code),
                code: Some(code),
            })];
        }

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
    // 上游后续分片给 `"id":""`、`"name":""`，空串等同缺失，否则会覆盖已累积的值。
    let id = tc
        .get("id")
        .or_else(|| f.and_then(|x| x.get("id")))
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

/// `usage` 里还带 `points_consumed`（本次扣分），不属 OpenAI 标准，忽略。
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

/// 业务码分类。码表来自实测（`provider-info/loomy.md` §8）。
fn classify_code(code: &str) -> ErrorKind {
    match code {
        // 登录态失效 / 无权限。
        "100002" | "020002" | "200003" => ErrorKind::Authentication,
        // 请求参数错误。
        "100001" | "400" => ErrorKind::InvalidRequest,
        // 积分不足 / 限流。
        "100003" | "429" => ErrorKind::RateLimit,
        _ => ErrorKind::Api,
    }
}

/// `error` 框分类。
fn classify_error(err: &Value) -> ErrorKind {
    let code = err
        .get("code")
        .map(|c| c.to_string().trim_matches('"').to_string())
        .unwrap_or_default();
    let by_code = classify_code(&code);
    if by_code != ErrorKind::Api {
        return by_code;
    }
    match err.get("type").and_then(|t| t.as_str()) {
        Some("invalid_request_error") => ErrorKind::InvalidRequest,
        Some("authentication_error") => ErrorKind::Authentication,
        Some("rate_limit_error") | Some("insufficient_quota") => ErrorKind::RateLimit,
        _ => ErrorKind::Api,
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
    fn 首帧只有role不产事件() {
        assert!(
            parse(r#"{"id":"a","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant"}}]}"#)
                .is_empty()
        );
    }

    #[test]
    fn 解析思考与正文() {
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"reasoning_content":"好的"}}]}"#);
        assert!(matches!(&ev[0], Event::Reasoning(s) if s == "好的"));
        let ev = parse(r#"{"choices":[{"index":0,"delta":{"content":"你好"}}]}"#);
        assert!(matches!(&ev[0], Event::Text(s) if s == "你好"));
    }

    #[test]
    fn 空串不产事件() {
        assert!(
            parse(r#"{"choices":[{"index":0,"delta":{"content":"","reasoning_content":""}}]}"#)
                .is_empty()
        );
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
                assert_eq!(t.arguments, None);
            }
            _ => panic!("应为 ToolCall"),
        }

        // 后续分片 id/name 为空串 → 视为缺失。
        let ev = parse(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"id":"","function":{"arguments":"{\"city\"","name":""},"type":"function","index":0}]}}]}"#,
        );
        match &ev[0] {
            Event::ToolCall(t) => {
                assert_eq!(t.id, None);
                assert_eq!(t.name, None);
                assert_eq!(t.arguments.as_deref(), Some(r#"{"city""#));
            }
            _ => panic!("应为 ToolCall"),
        }
    }

    #[test]
    fn finish_reason映射() {
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

    #[test]
    fn 解析usage() {
        let ev = parse(
            r#"{"choices":[],"usage":{"prompt_tokens":28,"completion_tokens":32,"total_tokens":60,"completion_tokens_details":{"reasoning_tokens":29},"points_consumed":1}}"#,
        );
        match &ev[0] {
            Event::Usage(u) => {
                assert_eq!(u.prompt_tokens, 28);
                assert_eq!(u.completion_tokens, 32);
                assert_eq!(u.total_tokens, 60);
                assert_eq!(u.reasoning_tokens, Some(29));
                assert_eq!(u.cached_tokens, None);
            }
            _ => panic!("应为 Usage"),
        }
    }

    #[test]
    fn done不产事件() {
        assert!(parse("[DONE]").is_empty());
    }

    /// 认证失效：HTTP 200 的顶层业务信封。
    #[test]
    fn 业务信封认证失效() {
        let ev = parse(r#"{"code":"100002","desc":"登录已失效，请重新登录","trace_id":"t"}"#);
        match &ev[0] {
            Event::Error(e) => {
                assert_eq!(e.kind, ErrorKind::Authentication);
                assert_eq!(e.code.as_deref(), Some("100002"));
                assert_eq!(e.message, "登录已失效，请重新登录");
            }
            _ => panic!("应为 Error"),
        }
    }

    #[test]
    fn error框分类() {
        let ev = parse(r#"{"error":{"code":400,"message":"请求参数错误"}}"#);
        match &ev[0] {
            Event::Error(e) => assert_eq!(e.kind, ErrorKind::InvalidRequest),
            _ => panic!(),
        }
        let ev = parse(r#"{"error":{"type":"authentication_error","message":"bad"}}"#);
        match &ev[0] {
            Event::Error(e) => assert_eq!(e.kind, ErrorKind::Authentication),
            _ => panic!(),
        }
    }

    #[test]
    fn 非json帧产生错误事件() {
        assert!(matches!(&parse("not json")[0], Event::Error(_)));
    }
}
