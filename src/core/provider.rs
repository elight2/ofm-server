//! 供应商抽象。
//!
//! 新增供应商只需实现 [`Provider`]，并在 [`crate::providers::builtin`] 中注册。

use async_trait::async_trait;

use super::events::EventStream;

/// 供应商无关的对话请求（已剥离 `供应商id/` 前缀）。
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// 上游模型名。
    pub model: String,
    /// 标准化消息。图片以 URL（支持 `data:`）内联。
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// 下游是否要求工具调用（`tool_choice=a required`）。
    pub require_tool: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
    pub tool_calls: Vec<ToolCall>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
}

#[derive(Debug, Clone)]
pub enum Part {
    Text(String),
    Image { url: String, detail: Option<String> },
}

#[derive(Debug, Clone)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: Option<String>,
    /// JSON Schema（对象形态，由供应商决定是否字符串化）。
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub model: String,
    pub display_name: Option<String>,
    /// 供应商标的计费倍率（`display_contact_config.consumption_rate.data.rate`）。
    /// 上游未给出时为 `None`。
    pub rate: Option<f64>,
}

#[derive(Debug, Clone, Default)]
pub struct ClaimReport {
    pub skipped: bool,
    pub message: String,
    /// 本次签到带来的余额变化（若可观测）。
    pub credits_delta: Option<f64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("供应商未登录: 缺少 {0}")]
    NotLoggedIn(String),
    #[error("凭证无效或已过期: {0}")]
    Unauthorized(String),
    #[error("上游请求失败: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("上游响应异常: {0}")]
    Protocol(String),
    #[error("功能未实现: {0}")]
    Unimplemented(&'static str),
    #[error("{0}")]
    Other(String),
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// 供应商 id，同时是模型名前缀与数据目录名。
    fn id(&self) -> &'static str;

    /// 模型目录。失败应返回空表而非硬错误（`/v1/models` 要能容错聚合）。
    async fn models(&self) -> Vec<ModelInfo>;

    /// 发起对话，返回事件流。要求：错误以 [`crate::core::events::Event::Error`]
    /// 形式出现在流内，便于下游统一处理"已发内容前后"两种情形。
    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError>;

    /// 签到。返回 `skipped=true` 表示今日无需领取。
    async fn claim(&self) -> Result<ClaimReport, ProviderError>;

    /// 当前余额。上游不提供余额时返回 `None`。
    ///
    /// 默认实现返回 `None`，供应商按需覆盖。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        Ok(None)
    }
}

/// 模型对外展示名后缀：` (x<倍率>)`，见 README「转发出的模型名称」。
///
/// 倍率按最短表示输出（`0.2` → `0.2`），无倍率时返回空串。
pub fn rate_suffix(rate: Option<f64>) -> String {
    match rate {
        Some(r) => format!(" (x{r})"),
        None => String::new(),
    }
}
