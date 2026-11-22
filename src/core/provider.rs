//! 供应商抽象。
//!
//! 新增供应商只需实现 [`Provider`]，并在 [`crate::providers::build_registry`] 中注册。

use async_trait::async_trait;

use super::claim::ClaimOutcome;
use super::events::EventStream;

/// 供应商无关的对话请求（已剥离 `供应商id/` 前缀）。
#[derive(Debug, Clone)]
pub struct ChatRequest {
    /// 上游模型名。
    pub model: String,
    /// 标准化消息。图片以 URL（支持 `data:`）内联。
    pub messages: Vec<Message>,
    pub tools: Vec<ToolSpec>,
    /// 下游的工具选择约束（CC 的 `tool_choice`）。
    pub tool_choice: ToolChoice,
}

/// CC `tool_choice` 的四种取值。
///
/// 供应商支持程度不同：raccoon 逐项生效；trae 的上游接受该字段但**静默忽略**
/// （见 `provider-info/trae.md` §0），故 trae 侧不发送。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ToolChoice {
    /// 未指定或显式 `"auto"`：有工具时由模型自行决定。不发送该字段。
    #[default]
    Auto,
    /// `"none"`：禁止调用工具。
    None,
    /// `"required"`：必须调用至少一个工具。
    Required,
    /// 强制调用指定函数。
    Function(String),
}

impl ToolChoice {
    /// 从 CC 的 `tool_choice` 值解析。
    ///
    /// 未指定、`"auto"`、以及无法识别的形态一律视为 [`ToolChoice::Auto`]
    /// （`auto` 是有工具时的默认行为，不发送字段即可）。
    pub fn from_json(v: Option<&serde_json::Value>) -> Self {
        match v {
            Some(serde_json::Value::String(s)) if s == "none" => Self::None,
            Some(serde_json::Value::String(s)) if s == "required" => Self::Required,
            Some(serde_json::Value::Object(o)) => o
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())
                .map(|n| Self::Function(n.to_string()))
                .unwrap_or(Self::Auto),
            _ => Self::Auto,
        }
    }

    /// 转成上游请求体里的 `tool_choice` 值；[`Auto`](Self::Auto) 返回 `None`（不发送）。
    pub fn to_json(&self) -> Option<serde_json::Value> {
        match self {
            Self::Auto => None,
            Self::None => Some(serde_json::Value::String("none".into())),
            Self::Required => Some(serde_json::Value::String("required".into())),
            Self::Function(name) => Some(serde_json::json!({
                "type": "function",
                "function": { "name": name }
            })),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    /// 上游 `messages[].role` 的取值。
    ///
    /// 只覆盖「标准 OpenAI 取值」——MiniMax 把 `tool` 折成 `user`（工具结果按
    /// 文本消息送出），故它不用这个函数。
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
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

/// 签到结果。
///
/// 结果码与明细是**结构化**的（[`ClaimOutcome`]）：写日志时用英文，展示时用中文，
/// 两套文案只在一处维护。
#[derive(Debug, Clone, Default)]
pub struct ClaimReport {
    /// 结果码 + 明细。
    pub outcome: ClaimOutcome,
    /// 本次签到带来的余额变化（若可观测）。
    pub credits_delta: Option<f64>,
    /// 签到后的当前余额（若可观测）。
    pub balance: Option<f64>,
}

impl ClaimReport {
    /// 本次没领到（今日已领 / 未开启 / 无可领福利）。
    pub fn skipped(&self) -> bool {
        self.outcome.code.is_skipped()
    }

    /// 展示用的一句话（中文）。
    pub fn message_zh(&self) -> String {
        self.outcome.message_zh()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("供应商未登录: 缺少 {0}")]
    NotLoggedIn(String),
    #[error("凭证无效或已过期: {0}")]
    Unauthorized(String),
    #[error("上游限流: {0}")]
    RateLimit(String),
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

    /// 拉取模型目录并保留错误。供应商只需实现这一个。
    ///
    /// 五家原本各自写一对完全相同的 `models`/`models_checked`，故把「失败只记
    /// 警告、返回空表」的策略上收到 trait：新增供应商不会再漏掉那条日志。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError>;

    /// 模型目录。失败返回空表而非硬错误（`/v1/models` 要能容错聚合）。
    async fn models(&self) -> Vec<ModelInfo> {
        match self.fetch_models().await {
            Ok(models) => models,
            Err(e) => {
                tracing::warn!("[{}] 获取模型目录失败: {e}", self.id());
                Vec::new()
            }
        }
    }

    /// 拉取模型目录并保留错误（供 `status` 真实探测接入状态）。
    async fn models_checked(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.fetch_models().await
    }

    /// 凭证到期时间（毫秒时间戳）。不适用或未知时返回 `None`。
    fn credential_expire_at(&self) -> Option<i64> {
        None
    }

    /// 主动刷新登录令牌（`ofm-server refresh <id>`，以及 `serve` 里按
    /// `refresh_interval` 自动调用的定时任务）。
    ///
    /// 返回可读的结果描述（如新到期时间），成功时应已落盘。
    /// 供应商无续期机制时返回 [`ProviderError::Unsupported`]，`serve` 会据此
    /// 停止该供应商的自动刷新循环（不是错误，不重复告警）。
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
