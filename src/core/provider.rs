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
    /// 下游是否要求工具调用（`tool_choice="required"`）。
    ///
    /// ⚠️ 两家上游都没有对应的 `tool_choice` 字段可传，当前**未生效**：
    /// 该值只被解析、不参与请求构造。保留以便将来上游支持时接上。
    #[allow(dead_code)]
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
    /// 图片。`detail` 是 OpenAI 的 `low`/`high`/`auto`，两家上游都不支持，解析后丢弃。
    Image {
        url: String,
        #[allow(dead_code)]
        detail: Option<String>,
    },
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
    /// 上游给的人类可读名。对外模型 id 只用 `model`（见 README「转发出的模型名称」），
    /// 此字段留给日志与排查。
    #[allow(dead_code)]
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
    /// 签到后的当前余额（若可观测）。
    pub balance: Option<f64>,
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
    #[error("此供应商不支持: {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Other(String),
}

#[async_trait]
pub trait Provider: Send + Sync {
    /// 供应商 id，同时是模型名前缀与数据目录名。
    fn id(&self) -> &'static str;

    /// 模型目录。失败应返回空表而非硬错误（`/v1/models` 要能容错聚合）。
    async fn models(&self) -> Vec<ModelInfo>;

    /// 拉取模型目录并保留错误（供 `status` 真实探测接入状态）。
    ///
    /// 默认实现回退到 [`Provider::models`]（永远 `Ok`）。需要区分
    /// 「拉取失败」与「真的没有模型」的供应商应覆盖此方法。
    async fn models_checked(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Ok(self.models().await)
    }

    /// 凭证到期时间（毫秒时间戳）。不适用或未知时返回 `None`。
    fn credential_expire_at(&self) -> Option<i64> {
        None
    }

    /// 主动刷新登录令牌（`ofm-server refresh <id>`）。
    ///
    /// 返回可读的结果描述（如新到期时间），成功时应已落盘。
    /// 供应商无续期机制时返回 [`ProviderError::Unsupported`]。
    ///
    /// 默认实现返回不支持；注意这与“已实现但暂未被自动调用”不同。
    async fn refresh(&self) -> Result<String, ProviderError> {
        Err(ProviderError::Unsupported(self.id()))
    }

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
