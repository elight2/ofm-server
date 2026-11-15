//! Trae 供应商（占位实现）。
//!
//! 当前 `models` / `chat` / `claim` 使用硬编码输出，仅用于联调 CLI 与服务器。
//! 协议实现见 `provider-info/trae.md`，后续替换本文件的 TODO 部分。

pub mod auth;
pub mod credential;

use std::path::PathBuf;

use async_trait::async_trait;

use crate::core::events::{self, Event, EventStream, FinishReason, Usage};
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

pub const ID: &str = "trae";

pub struct TraeProvider {
    #[allow(dead_code)]
    root: PathBuf,
}

impl TraeProvider {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

#[async_trait]
impl Provider for TraeProvider {
    fn id(&self) -> &'static str {
        ID
    }

    async fn models(&self) -> Vec<ModelInfo> {
        // TODO(协议): POST /api/ide/v1/batch_get_detail_param，
        // 取 function == "solo_agent" 且 usage == "chat_completion" 的 config_name。
        ["glm-5.2", "deepseek-v4.1-flash", "qwen3.8-flash"]
            .into_iter()
            .map(|m| ModelInfo {
                model: m.to_string(),
                display_name: None,
            })
            .collect()
    }

    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError> {
        // TODO(协议): POST /api/agent/v3/llm_utils_chat，解析 SSE。
        let last_user = req
            .messages
            .iter()
            .rev()
            .find(|m| m.role == crate::core::provider::Role::User)
            .and_then(|m| {
                m.parts.iter().find_map(|p| match p {
                    crate::core::provider::Part::Text(t) => Some(t.clone()),
                    _ => None,
                })
            })
            .unwrap_or_default();

        let text = format!("[trae 占位]{}(model={})", last_user, req.model);
        let mut events = Vec::new();
        events.push(Event::Reasoning("[trae 占位] 思考中… ".to_string()));
        for ch in text.chars() {
            events.push(Event::Text(ch.to_string()));
        }
        events.push(Event::Usage(Usage {
            prompt_tokens: 1,
            completion_tokens: text.chars().count() as u64,
            total_tokens: text.chars().count() as u64 + 1,
            cached_tokens: Some(0),
            reasoning_tokens: Some(0),
        }));
        events.push(Event::Finish(FinishReason::Stop));
        Ok(events::from_events(events))
    }

    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        // TODO(协议): status → claim → 复核 status/usage。
        Ok(ClaimReport {
            skipped: true,
            message: "[trae 占位] 今日已签到，跳过".to_string(),
            credits_delta: None,
        })
    }
}
