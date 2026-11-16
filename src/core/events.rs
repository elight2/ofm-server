//! 供应商无关的中间事件流。
//!
//! 供应商把上游响应解析成 [`Event`] 序列；下游服务器把 [`Event`] 编码成
//! OpenAI 格式（流式 chunk 或非流式聚合）。两个方向互不知道对方的实现。

use futures_util::Stream;
use std::pin::Pin;

/// 供应商输出的单个事件。
#[derive(Debug, Clone)]
pub enum Event {
    /// 正文增量。
    Text(String),
    /// 思考增量（OpenAI 扩展字段 `reasoning_content`）。
    Reasoning(String),
    /// 工具调用增量。同一 `index` 的多个事件需按序拼接。
    ToolCall(ToolCallDelta),
    /// 用量统计（通常最后一个到达的非终止事件）。
    Usage(Usage),
    /// 正常结束，携带上游 `finish_reason`（如 `stop`）。
    Finish(FinishReason),
    /// 上游业务错误（多出现在 HTTP 200 + SSE 帧内）。
    Error(UpstreamError),
}

#[derive(Debug, Clone, Default)]
pub struct ToolCallDelta {
    pub index: u32,
    /// 首次出现时非空。
    pub id: Option<String>,
    pub name: Option<String>,
    /// 参数片段（可为空串）。
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    Stop,
    Length,
    ContentFilter,
    ToolCalls,
    Other,
}

impl FinishReason {
    pub fn as_str(self) -> &'static str {
        match self {
            FinishReason::Stop => "stop",
            FinishReason::Length => "length",
            FinishReason::ContentFilter => "content_filter",
            FinishReason::ToolCalls => "tool_calls",
            FinishReason::Other => "stop",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: Option<u64>,
    pub reasoning_tokens: Option<u64>,
}

/// 供应商流内错误。`code` 为上游原始码，仅用于展示。
#[derive(Debug, Clone)]
pub struct UpstreamError {
    /// 上游数字/字符串错误码。
    pub code: Option<String>,
    /// 已经裁剪过的可读消息。
    pub message: String,
    /// 语义分类，决定下游 HTTP 状态码。
    pub kind: ErrorKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 认证失败（凭证无效/过期）→ 401
    Authentication,
    /// 请求参数错误 → 400
    InvalidRequest,
    /// 限流 → 429
    RateLimit,
    /// 其它上游故障 → 502
    Api,
}

pub type EventStream = Pin<Box<dyn Stream<Item = Event> + Send>>;
