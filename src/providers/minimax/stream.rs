//! 上游 anthropic-messages SSE → 中间事件。
//!
//! 帧结构为**本次实测**（`provider-info/minimax.md` §3.6，早期抓包未存流，
//! 本节由真实请求实测补全）：
//!
//! ```text
//! event: message_start       data: {"type":"message_start","message":{"id":…,"usage":{…}}}
//! event: ping                data: {"type":"ping"}
//! event: content_block_start data: {"type":"content_block_start","index":0,"content_block":{"type":"text"|"thinking"|"tool_use",…}}
//! event: content_block_delta data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta"|"thinking_delta"|"signature_delta"|"input_json_delta",…}}
//! event: content_block_stop  data: {"type":"content_block_stop","index":0}
//! event: message_delta       data: {"type":"message_delta","delta":{"stop_reason":…},"usage":{…}}
//! event: message_stop        data: {"type":"message_stop"}
//! ```
//!
//! 三个与直觉不同的实测点：
//! * `message_start` 的 `usage` 全是 **0**，真实用量只在 `message_delta`；
//! * `input_tokens` **不含**缓存命中部分，`cache_read_input_tokens` 是**另计**的
//!   （实测：`input_tokens=78` + `cache_read=128` = `count_tokens` 的 206）；
//! * `thinking` 块以 `signature_delta` 收尾，下游无对应概念，丢弃。

use serde::Deserialize;

use crate::core::events::{ErrorKind, Event, FinishReason, ToolCallDelta, UpstreamError, Usage};
use crate::core::sse::SseEvent;

/// 单个内容块的状态。Anthropic 的块是**有状态**的：`content_block_start`
/// 只给类型与初始值，增量在后续 `delta` 帧里按 `index` 归位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Text,
    Thinking,
    ToolUse,
    /// 未知类型（如未来新增的块）：忽略其增量。
    Other,
}

/// 解析状态。
#[derive(Debug)]
pub struct FrameParser {
    /// `index` → 块类型。
    blocks: std::collections::HashMap<u64, BlockKind>,
    /// `index` → 下游 tool_calls 序号。
    ///
    /// Anthropic 的 `index` 是**全局**块序号（thinking=0、text=1、tool_use=2…），
    /// 而下游 `tool_calls[].index` 必须在**工具内部**从 0 连续编号，故需重映射。
    tool_index: std::collections::HashMap<u64, u32>,
    next_tool: u32,
    /// 累计用量。`input_tokens` 与 `cache_read_input_tokens` 要相加。
    input_tokens: u64,
    output_tokens: u64,
    cache_read: Option<u64>,
    cache_creation: Option<u64>,
    reasoning_tokens: Option<u64>,
    /// 是否见过 `message_stop`。
    saw_stop: bool,
    /// 是否已有工具调用（`stop_reason` 需要改写成 `tool_calls`）。
    saw_tool: bool,
    /// 上游 `stop_reason`，来自 `message_delta.delta.stop_reason`。
    stop_reason: Option<String>,
}

impl FrameParser {
    pub fn new() -> Self {
        Self {
            blocks: std::collections::HashMap::new(),
            tool_index: std::collections::HashMap::new(),
            next_tool: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read: None,
            cache_creation: None,
            reasoning_tokens: None,
            saw_stop: false,
            saw_tool: false,
            stop_reason: None,
        }
    }

    pub fn parse(&mut self, ev: &SseEvent) -> Vec<Event> {
        // 事件名与 data.type 实测一致，但以 data 里的 type 为准（更可靠）。
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&ev.data) else {
            // 非 JSON 帧（如心跳注释）直接忽略，不报错。
            return Vec::new();
        };
        let kind = v.get("type").and_then(|t| t.as_str()).unwrap_or(&ev.event);
        match kind {
            "message_start" => {
                self.parse_message_start(&v);
                Vec::new()
            }
            "ping" => Vec::new(),
            "content_block_start" => self.parse_block_start(&v),
            "content_block_delta" => self.parse_block_delta(&v),
            "content_block_stop" => Vec::new(),
            "message_delta" => self.parse_message_delta(&v),
            "message_stop" => {
                self.saw_stop = true;
                // 不在这里发 Finish：`drive` 会在流结束时统一收尾。
                // 但下游非流式聚合需要 finish_reason，由 `finish()` 提供。
                Vec::new()
            }
            "error" => vec![parse_error(&v)],
            _ => Vec::new(),
        }
    }

    fn parse_message_start(&mut self, v: &serde_json::Value) {
        let Some(u) = v.get("message").and_then(|m| m.get("usage")) else {
            return;
        };
        // 实测 message_start 的 usage 全 0，但仍按字段累计，兼容上游改动。
        self.absorb_usage(u);
    }

    fn parse_block_start(&mut self, v: &serde_json::Value) -> Vec<Event> {
        let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
        let block = v.get("content_block");
        let kind = match block.and_then(|b| b.get("type")).and_then(|t| t.as_str()) {
            Some("text") => BlockKind::Text,
            Some("thinking") => BlockKind::Thinking,
            Some("tool_use") => BlockKind::ToolUse,
            _ => BlockKind::Other,
        };
        self.blocks.insert(index, kind);

        if kind != BlockKind::ToolUse {
            return Vec::new();
        }
        // tool_use 起始帧带 id/name，`input` 恒为空对象 `{}`（实测）。
        self.saw_tool = true;
        let down = match self.tool_index.get(&index) {
            Some(i) => *i,
            None => {
                let i = self.next_tool;
                self.tool_index.insert(index, i);
                self.next_tool += 1;
                i
            }
        };
        vec![Event::ToolCall(ToolCallDelta {
            index: down,
            id: block
                .and_then(|b| b.get("id"))
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            name: block
                .and_then(|b| b.get("name"))
                .and_then(|x| x.as_str())
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            arguments: None,
        })]
    }

    fn parse_block_delta(&mut self, v: &serde_json::Value) -> Vec<Event> {
        let index = v.get("index").and_then(|i| i.as_u64()).unwrap_or(0);
        let Some(delta) = v.get("delta") else {
            return Vec::new();
        };
        let kind = match delta.get("type").and_then(|t| t.as_str()) {
            Some("text_delta") => BlockKind::Text,
            Some("thinking_delta") => BlockKind::Thinking,
            Some("input_json_delta") => BlockKind::ToolUse,
            // signature_delta：思考块的签名收尾，下游无对应概念（§3.3）。
            Some("signature_delta") => return Vec::new(),
            _ => *self.blocks.get(&index).unwrap_or(&BlockKind::Other),
        };
        match kind {
            BlockKind::Text => delta
                .get("text")
                .and_then(|t| t.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| vec![Event::Text(s.to_string())])
                .unwrap_or_default(),
            BlockKind::Thinking => delta
                .get("thinking")
                .and_then(|t| t.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| vec![Event::Reasoning(s.to_string())])
                .unwrap_or_default(),
            BlockKind::ToolUse => {
                // 实测首个 input_json_delta 是**空串**，随后才是真片段。
                let Some(frag) = delta
                    .get("partial_json")
                    .and_then(|t| t.as_str())
                    .filter(|s| !s.is_empty())
                else {
                    return Vec::new();
                };
                let down = *self.tool_index.get(&index).unwrap_or(&0);
                vec![Event::ToolCall(ToolCallDelta {
                    index: down,
                    id: None,
                    name: None,
                    arguments: Some(frag.to_string()),
                })]
            }
            BlockKind::Other => Vec::new(),
        }
    }

    fn parse_message_delta(&mut self, v: &serde_json::Value) -> Vec<Event> {
        if let Some(u) = v.get("usage") {
            self.absorb_usage(u);
        }
        if let Some(r) = v
            .get("delta")
            .and_then(|d| d.get("stop_reason"))
            .and_then(|r| r.as_str())
        {
            self.stop_reason = Some(r.to_string());
        }
        // 不在这里发 Finish：`message_delta` 与 `message_stop` 之间可能还有帧。
        Vec::new()
    }

    /// 累计 usage。**`input_tokens` 与 `cache_read_input_tokens` 是并列的**，
    /// 下游 `prompt_tokens` 必须是二者之和（实测见模块注释）。
    fn absorb_usage(&mut self, u: &serde_json::Value) {
        let n = |k: &str| u.get(k).and_then(|x| x.as_u64());
        if let Some(v) = n("input_tokens") {
            self.input_tokens = self.input_tokens.max(v);
        }
        if let Some(v) = n("output_tokens") {
            self.output_tokens = self.output_tokens.max(v);
        }
        if let Some(v) = n("cache_read_input_tokens") {
            self.cache_read = Some(v);
        }
        if let Some(v) = n("cache_creation_input_tokens") {
            self.cache_creation = Some(v);
        }
        if let Some(v) = u
            .get("output_tokens_details")
            .and_then(|d| d.get("thinking_tokens"))
            .and_then(|x| x.as_u64())
        {
            self.reasoning_tokens = Some(v);
        }
    }

    /// 收尾事件序列：`Usage` + `Finish`。
    ///
    /// `drive` 自己也会在流断开时补 `Finish`，但它不知道 `stop_reason`；
    /// 故纯函数 `finish()` 只在流正常跑完时由调用方使用（见本文件末尾说明）。
    pub fn finish_events(&self) -> Vec<Event> {
        let prompt = self.input_tokens + self.cache_read.unwrap_or(0);
        vec![
            Event::Usage(Usage {
                prompt_tokens: prompt,
                completion_tokens: self.output_tokens,
                total_tokens: prompt + self.output_tokens,
                cached_tokens: self.cache_read,
                reasoning_tokens: self.reasoning_tokens,
            }),
            Event::Finish(map_finish(self.stop_reason.as_deref(), self.saw_tool)),
        ]
    }

    pub fn saw_stop(&self) -> bool {
        self.saw_stop
    }

    /// 缓存创建量，仅用于日志（下游无对应位，§3.8）。
    #[allow(dead_code)]
    pub fn cache_creation(&self) -> Option<u64> {
        self.cache_creation
    }
}

impl Default for FrameParser {
    fn default() -> Self {
        Self::new()
    }
}

/// `stop_reason` → 统一枚举（§3.8）。
///
/// 出现过工具调用时一律 `ToolCalls`：上游 `stop_reason` 在工具轮是 `tool_use`，
/// 但个别情况会给 `end_turn`，以下游语义为准更稳。
fn map_finish(stop_reason: Option<&str>, saw_tool: bool) -> FinishReason {
    if saw_tool {
        return FinishReason::ToolCalls;
    }
    match stop_reason {
        Some("max_tokens") => FinishReason::Length,
        Some("tool_use") => FinishReason::ToolCalls,
        None | Some("end_turn") | Some("stop_sequence") => FinishReason::Stop,
        Some(_) => FinishReason::Other,
    }
}

/// 上游错误体：`{"type":"error","error":{"type":"invalid_request_error","message":"…"}}`（实测）。
fn parse_error(v: &serde_json::Value) -> Event {
    let err = v.get("error").unwrap_or(v);
    let ty = err.get("type").and_then(|t| t.as_str());
    let message = err
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("上游返回错误")
        .to_string();
    Event::Error(UpstreamError {
        code: ty.map(str::to_string),
        kind: classify(ty, &message),
        message: message.lines().next().unwrap_or("").to_string(),
    })
}

fn classify(ty: Option<&str>, message: &str) -> ErrorKind {
    match ty {
        Some("authentication_error") | Some("permission_error") => ErrorKind::Authentication,
        Some("invalid_request_error") | Some("not_found_error") => ErrorKind::InvalidRequest,
        Some("rate_limit_error") | Some("overloaded_error") => ErrorKind::RateLimit,
        _ => {
            let m = message.to_ascii_lowercase();
            if m.contains("unauthorized") || m.contains("invalid token") {
                ErrorKind::Authentication
            } else if m.contains("rate limit") {
                ErrorKind::RateLimit
            } else {
                ErrorKind::Api
            }
        }
    }
}

/// 供流式驱动使用的「带收尾」包装。
///
/// `core::stream::drive` 在断流时补 `Finish(Stop)`，但它看不到 `stop_reason`
/// 与累计 usage。这里把解析器状态留在闭包外，流结束时由 [`finish_events`] 补发。
pub struct Parser {
    inner: FrameParser,
    done: bool,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            inner: FrameParser::new(),
            done: false,
        }
    }

    /// 解析一帧；末帧（`message_stop`）同时吐出 usage 与 finish。
    pub fn parse(&mut self, ev: &SseEvent) -> Vec<Event> {
        let mut out = self.inner.parse(ev);
        if self.inner.saw_stop() && !self.done {
            self.done = true;
            out.extend(self.inner.finish_events());
        }
        out
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

/// 未使用但保留的公开类型，供测试断言错误体形状。
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ErrorBody {
    #[serde(rename = "type")]
    kind: String,
    error: InnerError,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct InnerError {
    #[serde(rename = "type")]
    kind: String,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(event: &str, data: &str) -> SseEvent {
        SseEvent {
            event: event.into(),
            data: data.into(),
        }
    }

    /// 真实首帧：usage 全 0（实测），不产事件。
    #[test]
    fn message_start_不产事件() {
        let mut p = FrameParser::new();
        let evs = p.parse(&frame(
            "message_start",
            r#"{"type":"message_start","message":{"id":"x","type":"message","role":"assistant","content":[],"model":"MiniMax-M3.1-Flash","usage":{"input_tokens":0,"output_tokens":0},"service_tier":"standard"}}"#,
        ));
        assert!(evs.is_empty());
    }

    /// ping 帧被忽略（实测有该帧，文档原本未提）。
    #[test]
    fn ping_被忽略() {
        let mut p = FrameParser::new();
        assert!(p.parse(&frame("ping", r#"{"type":"ping"}"#)).is_empty());
    }

    /// 正文增量。
    #[test]
    fn text_delta() {
        let mut p = FrameParser::new();
        p.parse(&frame(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ));
        let evs = p.parse(&frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#,
        ));
        assert!(matches!(&evs[0], Event::Text(s) if s == "你好"));
    }

    /// 思考增量 → Reasoning；signature_delta 被丢弃。
    #[test]
    fn thinking_delta_与签名() {
        let mut p = FrameParser::new();
        p.parse(&frame(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
        ));
        let evs = p.parse(&frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"想想"}}"#,
        ));
        assert!(matches!(&evs[0], Event::Reasoning(s) if s == "想想"));

        let sig = p.parse(&frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"37ba76ee"}}"#,
        ));
        assert!(sig.is_empty(), "签名不该转发给下游");
    }

    /// tool_use 起始带 id/name，`input` 恒空对象（实测）。
    #[test]
    fn tool_use_起始() {
        let mut p = FrameParser::new();
        let evs = p.parse(&frame(
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_01a1","name":"get_weather","input":{}}}"#,
        ));
        match &evs[0] {
            Event::ToolCall(d) => {
                assert_eq!(d.index, 0, "下游工具序号从 0 开始，与上游块 index 无关");
                assert_eq!(d.id.as_deref(), Some("call_01a1"));
                assert_eq!(d.name.as_deref(), Some("get_weather"));
                assert_eq!(d.arguments, None);
            }
            other => panic!("应为 ToolCall: {other:?}"),
        }
    }

    /// 首个 input_json_delta 是空串（实测），不产事件；后续片段累积。
    #[test]
    fn input_json_delta_空串不产事件() {
        let mut p = FrameParser::new();
        p.parse(&frame(
            "content_block_start",
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"c","name":"t","input":{}}}"#,
        ));
        let empty = p.parse(&frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}"#,
        ));
        assert!(empty.is_empty());

        let frag = p.parse(&frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"city\":\"北京\""}}"#,
        ));
        match &frag[0] {
            Event::ToolCall(d) => {
                assert_eq!(d.index, 0);
                assert_eq!(d.arguments.as_deref(), Some(r#"{"city":"北京""#));
            }
            other => panic!("应为 ToolCall: {other:?}"),
        }
    }

    /// 多个工具的上游块 index 是全局的（thinking=0/text=1/tool=2…），
    /// 下游必须重映射成 0,1,… 连续序号。
    #[test]
    fn 多工具序号重映射() {
        let mut p = FrameParser::new();
        for (block_index, name) in [(2u64, "a"), (3, "b")] {
            p.parse(&frame(
                "content_block_start",
                &format!(
                    r#"{{"type":"content_block_start","index":{block_index},"content_block":{{"type":"tool_use","id":"call_{block_index}","name":"{name}","input":{{}}}}}}"#
                ),
            ));
        }
        let frag = p.parse(&frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":3,"delta":{"type":"input_json_delta","partial_json":"{}"}}"#,
        ));
        match &frag[0] {
            Event::ToolCall(d) => assert_eq!(d.index, 1, "第二个工具应为序号 1"),
            other => panic!("应为 ToolCall: {other:?}"),
        }
    }

    /// `input_tokens` + `cache_read_input_tokens` = `prompt_tokens`（关键实测）。
    #[test]
    fn usage_缓存与输入相加() {
        let mut p = FrameParser::new();
        p.parse(&frame(
            "message_delta",
            r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":78,"output_tokens":8,"cache_read_input_tokens":128,"service_tier":"standard"}}"#,
        ));
        let evs = p.finish_events();
        match &evs[0] {
            Event::Usage(u) => {
                assert_eq!(u.prompt_tokens, 206, "78 + 128 = 206，与 count_tokens 一致");
                assert_eq!(u.completion_tokens, 8);
                assert_eq!(u.total_tokens, 214);
                assert_eq!(u.cached_tokens, Some(128));
            }
            other => panic!("应为 Usage: {other:?}"),
        }
    }

    /// 无缓存时 prompt_tokens 就是 input_tokens。
    #[test]
    fn 无缓存时直接用input_tokens() {
        let mut p = FrameParser::new();
        p.parse(&frame(
            "message_delta",
            r#"{"type":"message_delta","delta":{},"usage":{"input_tokens":50,"output_tokens":10}}"#,
        ));
        match &p.finish_events()[0] {
            Event::Usage(u) => {
                assert_eq!(u.prompt_tokens, 50);
                assert_eq!(u.cached_tokens, None);
            }
            other => panic!("应为 Usage: {other:?}"),
        }
    }

    /// 思考 token 数从 `output_tokens_details.thinking_tokens` 取。
    #[test]
    fn 解析思考token数() {
        let mut p = FrameParser::new();
        p.parse(&frame(
            "message_delta",
            r#"{"type":"message_delta","delta":{},"usage":{"input_tokens":79,"output_tokens":223,"cache_read_input_tokens":128,"output_tokens_details":{"thinking_tokens":123}}}"#,
        ));
        match &p.finish_events()[0] {
            Event::Usage(u) => assert_eq!(u.reasoning_tokens, Some(123)),
            other => panic!("应为 Usage: {other:?}"),
        }
    }

    /// stop_reason 映射。
    #[test]
    fn stop_reason_映射() {
        assert_eq!(map_finish(Some("end_turn"), false), FinishReason::Stop);
        assert_eq!(map_finish(Some("max_tokens"), false), FinishReason::Length);
        assert_eq!(map_finish(Some("stop_sequence"), false), FinishReason::Stop);
        assert_eq!(map_finish(None, false), FinishReason::Stop);
        // 有工具一律 tool_calls
        assert_eq!(map_finish(Some("end_turn"), true), FinishReason::ToolCalls);
        assert_eq!(map_finish(Some("tool_use"), true), FinishReason::ToolCalls);
    }

    /// 错误体（实测形状）。
    #[test]
    fn 错误体解析() {
        let mut p = FrameParser::new();
        let evs = p.parse(&frame(
            "error",
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"invalid params, unknown model 'No-Such-Model' (2013)"}}"#,
        ));
        match &evs[0] {
            Event::Error(e) => {
                assert_eq!(e.kind, ErrorKind::InvalidRequest);
                assert!(e.message.contains("unknown model"));
            }
            other => panic!("应为 Error: {other:?}"),
        }
    }

    /// 认证类错误。
    #[test]
    fn 认证错误分类() {
        assert_eq!(
            classify(Some("authentication_error"), "bad"),
            ErrorKind::Authentication
        );
        assert_eq!(classify(Some("rate_limit_error"), ""), ErrorKind::RateLimit);
        assert_eq!(classify(None, "Unauthorized"), ErrorKind::Authentication);
        assert_eq!(classify(None, "boom"), ErrorKind::Api);
    }

    /// 非 JSON 帧不 panic（心跳/注释行）。
    #[test]
    fn 非json帧被忽略() {
        let mut p = FrameParser::new();
        assert!(p.parse(&frame("", "not json")).is_empty());
        assert!(p.parse(&frame("", "")).is_empty());
    }

    /// 未知块类型：`delta.type` 已知时仍以 `delta.type` 为准（它是权威来源）；
    /// 两者都未知时忽略。
    #[test]
    fn 未知块类型被忽略() {
        let mut p = FrameParser::new();
        p.parse(&frame(
            "content_block_start",
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"server_tool_use"}}"#,
        ));
        // delta 类型未知 → 回退查块类型 → Other → 忽略
        assert!(p
            .parse(&frame(
                "content_block_delta",
                r#"{"type":"content_block_delta","index":0,"delta":{"type":"some_future_delta","x":"y"}}"#,
            ))
            .is_empty());
        // delta 类型为 text_delta 时仍应转发（delta.type 权威）
        let evs = p.parse(&frame(
            "content_block_delta",
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}}"#,
        ));
        assert!(matches!(&evs[0], Event::Text(t) if t == "x"));
    }

    /// 完整一轮的帧序列（真实抓取顺序）→ 事件序列正确。
    #[test]
    fn 完整一轮() {
        let mut p = Parser::new();
        let frames = [
            ("message_start", r#"{"type":"message_start","message":{"id":"x","usage":{"input_tokens":0,"output_tokens":0}}}"#),
            ("ping", r#"{"type":"ping"}"#),
            ("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#),
            ("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#),
            ("content_block_delta", r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"！"}}"#),
            ("content_block_stop", r#"{"type":"content_block_stop","index":0}"#),
            ("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":78,"output_tokens":8,"cache_read_input_tokens":128}}"#),
            ("message_stop", r#"{"type":"message_stop"}"#),
        ];
        let mut all = Vec::new();
        for (e, d) in frames {
            all.extend(p.parse(&frame(e, d)));
        }
        let text: String = all
            .iter()
            .filter_map(|e| match e {
                Event::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "你好！");
        // 恰好一个 Usage + 一个 Finish，且在末尾
        let usage = all.iter().filter(|e| matches!(e, Event::Usage(_))).count();
        let finish = all.iter().filter(|e| matches!(e, Event::Finish(_))).count();
        assert_eq!((usage, finish), (1, 1));
        assert!(matches!(all.last(), Some(Event::Finish(FinishReason::Stop))));
    }

    /// `message_stop` 之后不再重复收尾。
    #[test]
    fn 收尾只发一次() {
        let mut p = Parser::new();
        p.parse(&frame("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":1,"output_tokens":1}}"#));
        let a = p.parse(&frame("message_stop", r#"{"type":"message_stop"}"#));
        assert_eq!(a.len(), 2);
        // 再来一帧（异常重复）不能二次收尾
        let b = p.parse(&frame("message_stop", r#"{"type":"message_stop"}"#));
        assert!(b.is_empty());
    }
}
