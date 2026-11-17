//! 上游 SSE → 中间事件（`provider-info/trae.md` §3.3）。
//!
//! 上游帧集合：`metadata` / `timing_cost` / `output` / `extra_info` /
//! `token_usage` / `done` / `error` / `progress_notice`。
//! 只关心 `output`、`token_usage`、`done`、`error`，其余丢弃。

use serde::Deserialize;

use crate::core::events::{ErrorKind, Event, FinishReason, ToolCallDelta, UpstreamError, Usage};

/// `output` 帧：`response` 与 `reasoning_content` 同帧互斥，均为增量。
#[derive(Debug, Default, Deserialize)]
pub struct OutputFrame {
    #[serde(default)]
    pub response: Option<String>,
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<UpToolCallDelta>>,
}

#[derive(Debug, Default, Deserialize)]
pub struct UpToolCallDelta {
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function_call: Option<FunctionCallDelta>,
}

#[derive(Debug, Default, Deserialize)]
pub struct FunctionCallDelta {
    #[serde(default)]
    pub name: Option<String>,
    /// 首帧为空串，后续帧补片段。
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct TokenUsageFrame {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub reasoning_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Default, Deserialize)]
pub struct DoneFrame {
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ErrorFrame {
    #[serde(default)]
    pub code: Option<serde_json::Value>,
    #[serde(default)]
    pub message: Option<String>,
}

impl ErrorFrame {
    fn to_event(&self) -> Event {
        let code = self.code.as_ref().map(|c| match c {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        });
        let raw = self.message.clone().unwrap_or_default();
        Event::Error(UpstreamError {
            kind: classify(code.as_deref(), &raw),
            message: trim_message(&raw),
            code,
        })
    }
}

/// 错误码 → 语义分类（§5.1）。
///
/// 仅列出已实测的码，其余按 `Api`（→502）处理。
fn classify(code: Option<&str>, message: &str) -> ErrorKind {
    match code {
        // 限流
        Some("4011") => ErrorKind::RateLimit,
        // 参数/模型错误
        Some("4001") => ErrorKind::InvalidRequest,
        // 模型服务不可用 / 图片外链不可达
        Some("3003") => ErrorKind::InvalidRequest,
        // 认证失效
        Some("1001") => ErrorKind::Authentication,
        _ => {
            if message.contains("model service is unavailable") {
                ErrorKind::InvalidRequest
            } else {
                ErrorKind::Api
            }
        }
    }
}

/// `message` 只取首行，避免把整段堆栈塞给下游。
fn trim_message(raw: &str) -> String {
    let first = raw.lines().next().unwrap_or("").trim();
    if first.is_empty() {
        "上游返回错误但未给出消息".to_string()
    } else {
        first.to_string()
    }
}

/// 本轮的解析状态（`done` 需要知道是否出现过工具调用）。
#[derive(Debug, Default)]
pub struct FrameParser {
    saw_tool_call: bool,
}

impl FrameParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析一帧。`event` 为 SSE 事件名，`data` 为原始载荷。
    ///
    /// 返回空表示该帧无输出（含非法/未知帧，须容忍）。
    pub fn parse(&mut self, event: &str, data: &str) -> Vec<Event> {
        match event {
            "output" => self.parse_output(data),
            "token_usage" => match serde_json::from_str::<TokenUsageFrame>(data) {
                Ok(u) => vec![Event::Usage(Usage {
                    prompt_tokens: u.prompt_tokens,
                    completion_tokens: u.completion_tokens,
                    total_tokens: u.total_tokens,
                    cached_tokens: u.cache_read_input_tokens,
                    reasoning_tokens: u.reasoning_tokens,
                })],
                // 用量帧解析失败不影响内容，直接丢弃。
                Err(_) => Vec::new(),
            },
            "done" => {
                let reason = serde_json::from_str::<DoneFrame>(data)
                    .ok()
                    .and_then(|d| d.finish_reason);
                vec![Event::Finish(self.finish(reason.as_deref()))]
            }
            "error" => match serde_json::from_str::<ErrorFrame>(data) {
                Ok(e) => vec![e.to_event()],
                Err(_) => vec![Event::Error(UpstreamError {
                    code: None,
                    message: format!("上游错误帧无法解析: {}", truncate(data, 200)),
                    kind: ErrorKind::Api,
                })],
            },
            // metadata / timing_cost / extra_info / progress_notice / 未知帧
            _ => Vec::new(),
        }
    }

    fn parse_output(&mut self, data: &str) -> Vec<Event> {
        let frame: OutputFrame = match serde_json::from_str(data) {
            Ok(f) => f,
            Err(_) => return Vec::new(),
        };

        let mut out = Vec::new();
        if let Some(r) = frame.reasoning_content.filter(|s| !s.is_empty()) {
            out.push(Event::Reasoning(r));
        }
        if let Some(t) = frame.response.filter(|s| !s.is_empty()) {
            out.push(Event::Text(t));
        }
        if let Some(calls) = frame.tool_calls {
            for call in calls {
                let f = call.function_call.unwrap_or_default();
                let has_name = f.name.as_deref().is_some_and(|s| !s.is_empty());
                let has_args = f.arguments.as_deref().is_some_and(|s| !s.is_empty());
                if has_name || has_args {
                    self.saw_tool_call = true;
                }
                out.push(Event::ToolCall(ToolCallDelta {
                    index: call.index,
                    id: call.id.filter(|s| !s.is_empty()),
                    name: f.name.filter(|s| !s.is_empty()),
                    arguments: f.arguments.filter(|s| !s.is_empty()),
                }));
            }
        }
        out
    }

    /// 上游 `finish_reason` → 统一枚举。
    ///
    /// 本轮出现过工具调用时一律改写为 `tool_calls`（§0 出入 4，已批准）。
    fn finish(&self, upstream: Option<&str>) -> FinishReason {
        if self.saw_tool_call {
            return FinishReason::ToolCalls;
        }
        match upstream {
            Some("stop") | None => FinishReason::Stop,
            Some("length") => FinishReason::Length,
            Some("content_filter") => FinishReason::ContentFilter,
            Some("tool_calls") => FinishReason::ToolCalls,
            Some(_) => FinishReason::Other,
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(evs: &[Event]) -> String {
        evs.iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn parses_output_increments() {
        let mut p = FrameParser::new();
        let a = p.parse(
            "output",
            r#"{"response":"本机","reasoning_content":null,"tool_calls":null}"#,
        );
        assert_eq!(texts(&a), "本机");
        let b = p.parse(
            "output",
            r#"{"response":null,"reasoning_content":"思考","tool_calls":null}"#,
        );
        assert!(matches!(b.as_slice(), [Event::Reasoning(s)] if s == "思考"));
    }

    #[test]
    fn accumulates_tool_calls_by_index_and_rewrites_finish() {
        let mut p = FrameParser::new();
        let first = p.parse(
            "output",
            r#"{"response":null,"reasoning_content":null,"tool_calls":[
                {"index":0,"id":"call_00","type":"function",
                 "function_call":{"name":"get_time","arguments":"","partial_arguments":null}}]}"#,
        );
        match first.as_slice() {
            [Event::ToolCall(d)] => {
                assert_eq!(d.index, 0);
                assert_eq!(d.id.as_deref(), Some("call_00"));
                assert_eq!(d.name.as_deref(), Some("get_time"));
                assert_eq!(d.arguments, None);
            }
            other => panic!("意外的解析结果: {other:?}"),
        }

        let second = p.parse(
            "output",
            r#"{"response":null,"reasoning_content":null,"tool_calls":[
                {"index":0,"id":"","type":"","function_call":{"name":"","arguments":"{}"}}]}"#,
        );
        match second.as_slice() {
            [Event::ToolCall(d)] => {
                assert_eq!(d.id, None);
                assert_eq!(d.name, None);
                assert_eq!(d.arguments.as_deref(), Some("{}"));
            }
            other => panic!("意外的解析结果: {other:?}"),
        }

        // finish_reason 为 stop，但本轮有工具调用 → 改写为 tool_calls
        let done = p.parse("done", r#"{"finish_reason":"stop"}"#);
        assert!(matches!(
            done.as_slice(),
            [Event::Finish(FinishReason::ToolCalls)]
        ));
    }

    #[test]
    fn plain_stop_stays_stop() {
        let mut p = FrameParser::new();
        p.parse("output", r#"{"response":"hi"}"#);
        let done = p.parse("done", r#"{"finish_reason":"stop"}"#);
        assert!(matches!(
            done.as_slice(),
            [Event::Finish(FinishReason::Stop)]
        ));
    }

    #[test]
    fn parses_usage_and_maps_cached_and_reasoning() {
        let mut p = FrameParser::new();
        let evs = p.parse(
            "token_usage",
            r#"{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30,
                "reasoning_tokens":7,"cache_read_input_tokens":3,
                "cache_creation_input_tokens":0}"#,
        );
        match evs.as_slice() {
            [Event::Usage(u)] => {
                assert_eq!(
                    (u.prompt_tokens, u.completion_tokens, u.total_tokens),
                    (10, 20, 30)
                );
                assert_eq!(u.cached_tokens, Some(3));
                assert_eq!(u.reasoning_tokens, Some(7));
            }
            other => panic!("意外的解析结果: {other:?}"),
        }
    }

    #[test]
    fn classifies_known_error_codes() {
        let mut p = FrameParser::new();
        let cases = [
            ("4011", ErrorKind::RateLimit),
            ("4001", ErrorKind::InvalidRequest),
            ("3003", ErrorKind::InvalidRequest),
            ("1001", ErrorKind::Authentication),
            ("9999", ErrorKind::Api),
        ];
        for (code, want) in cases {
            let evs = p.parse(
                "error",
                &format!(r#"{{"code":{code},"message":"boom","extra":null}}"#),
            );
            match evs.as_slice() {
                [Event::Error(e)] => {
                    assert_eq!(e.kind, want, "code={code}");
                    assert_eq!(e.message, "boom");
                }
                other => panic!("意外的解析结果: {other:?}"),
            }
        }
    }

    #[test]
    fn tolerates_non_json_and_unknown_frames() {
        let mut p = FrameParser::new();
        assert!(
            p.parse("progress_notice", "Processing_1794708540")
                .is_empty()
        );
        assert!(
            p.parse("metadata", r#"{"model":"","session_id":"x"}"#)
                .is_empty()
        );
        assert!(
            p.parse("timing_cost", r#"{"name":"llm_raw_chat_v2"}"#)
                .is_empty()
        );
        assert!(p.parse("output", "not json").is_empty());
    }

    #[test]
    fn error_message_takes_first_line() {
        let mut p = FrameParser::new();
        let evs = p.parse(
            "error",
            r#"{"code":4001,"message":"bad request\nstack..."}"#,
        );
        match evs.as_slice() {
            [Event::Error(e)] => assert_eq!(e.message, "bad request"),
            other => panic!("意外的解析结果: {other:?}"),
        }
    }
}
