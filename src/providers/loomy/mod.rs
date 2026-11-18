//! Loomy 供应商。
//!
//! 已实现：登录（`auth`）、模型列表（`catalog`）、对话（`chat`）。
//! 协议实况见 `provider-info/loomy.md`。

pub mod api;
pub mod auth;
pub mod catalog;
pub mod chat;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::config::LoomyReasoningEffort;
use crate::core::claim::{ClaimDetail, ClaimOutcome, SkipReason};
use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use api::{HttpError, VERSION};
use credential::Credential;

pub mod credential;

pub const ID: &str = "loomy";

pub struct LoomyProvider {
    root: PathBuf,
    client: reqwest::Client,
    /// 思考强度。
    #[allow(dead_code)]
    effort: LoomyReasoningEffort,
}

impl LoomyProvider {
    pub fn new(root: PathBuf, effort: LoomyReasoningEffort) -> Self {
        Self {
            root,
            client: api::client().expect("构建 HTTP 客户端失败"),
            effort,
        }
    }

    /// 凭证到期时间。
    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .and_then(|c| c.token_expire_at)
    }

    fn credential(&self) -> Result<Credential, ProviderError> {
        match Credential::load(&self.root) {
            Ok(Some(c)) => Ok(c),
            Ok(None) => Err(ProviderError::NotLoggedIn(format!(
                "{}（请先运行 `ofm-server login {ID}`）",
                crate::config::Config::credential_path(&self.root, ID).display()
            ))),
            Err(e) => Err(ProviderError::Other(e.to_string())),
        }
    }

    /// 取可用 token。
    async fn access_token(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        Ok(cred.token.clone())
    }

    /// 请求模型目录。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let url = format!("{}/models", cred.base_url.trim_end_matches('/'));

        let mut http = self.client.get(&url);
        http = http.header("Authorization", format!("Bearer {}", token));
        http = http.header("Accept", "application/json");

        let resp = http.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &body));
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| ProviderError::Other(format!("解析模型目录失败: {e}")))?;

        let models = catalog::parse_models(&data);
        tracing::debug!("[{ID}] 模型目录共 {} 个可对话模型", models.len());
        Ok(models)
    }

    /// 对话请求头。
    fn chat_headers(&self, token: &str) -> Vec<(&'static str, String)> {
        vec![
            ("Authorization", format!("Bearer {}", token)),
            ("Content-Type", "application/json".to_string()),
            ("Accept", "application/json".to_string()),
        ]
    }
}

#[async_trait]
impl Provider for LoomyProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 拉取上游模型目录。
    async fn models(&self) -> Vec<ModelInfo> {
        match self.fetch_models().await {
            Ok(models) => models,
            Err(e) => {
                tracing::warn!("[{ID}] 获取模型目录失败: {e}");
                Vec::new()
            }
        }
    }

    /// 带错误的目录拉取。
    async fn models_checked(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.fetch_models().await
    }

    /// 对话。
    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let body = chat::ChatBody::build(&req, self.effort);
        let url = format!("{}/chat/completions", cred.base_url.trim_end_matches('/'));

        let headers = self.chat_headers(&token);
        let payload = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Other(format!("序列化请求体失败: {e}")))?;
        let mut http = self.client.post(&url).body(payload);
        for (k, v) in headers {
            http = http.header(k, v);
        }

        let resp = http.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &body));
        }

        // 上游恒回 JSON；转成事件流。
        let mut parser = chat::ResponseParser::new();
        Ok(crate::core::stream::drive(resp.bytes_stream(), move |ev| {
            parser.parse(&ev.event, &ev.data)
        }))
    }

    /// 主动刷新令牌。
    ///
    /// Loomy 的 API Key 通常长期有效，无需刷新。
    /// 如 API Key 已过期，用户需重新登录。
    async fn refresh(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        let new = auth::refresh(&self.client, &cred)
            .await
            .map_err(|e| match e {
                HttpError::Status { status: 401, .. } => {
                    ProviderError::Unauthorized("API Key 已失效，请重新登录".to_string())
                }
                other => ProviderError::Other(format!("续期失败: {other}")),
            })?;
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        Ok("续期成功（API Key 仍有效）".to_string())
    }

    /// 当前余额。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let url = format!("{}/v1/balance", cred.base_url.trim_end_matches('/'));

        let resp = self.client
            .get(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        if !resp.status().is_success() {
            return Ok(None);
        }

        let data: serde_json::Value = resp.json().await?;
        if let Some(total) = data.get("total_balance").and_then(|v| v.as_f64()) {
            return Ok(Some(total));
        }
        Ok(None)
    }

    /// 签到。
    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let url = format!("{}/v1/checkin", cred.base_url.trim_end_matches('/'));

        let resp = self.client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &body));
        }

        let data: serde_json::Value = resp.json().await?;
        let claimed = data.get("claimed").and_then(|v| v.as_bool()).unwrap_or(false);
        if !claimed {
            return Ok(ClaimReport {
                outcome: ClaimOutcome::skipped(SkipReason::AlreadyClaimed, vec![]),
                credits_delta: None,
                balance: None,
            });
        }

        let delta = data.get("delta").and_then(|v| v.as_f64());
        let balance = data.get("balance").and_then(|v| v.as_f64());
        let credits = data.get("credits").and_then(|v| v.as_f64()).unwrap_or(0.0);

        let mut details = vec![ClaimDetail::Credits(credits)];
        if let Some(d) = delta {
            details.push(ClaimDetail::Delta(d));
        }

        Ok(ClaimReport {
            outcome: ClaimOutcome::success(details),
            credits_delta: delta,
            balance,
        })
    }
}

/// HTTP 层错误映射。
fn map_http_error(status: u16, body: &str) -> ProviderError {
    let snippet = truncate(body, 300);
    match status {
        401 | 403 => ProviderError::Unauthorized(snippet),
        _ => ProviderError::Other(format!("上游 HTTP {status}: {snippet}")),
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
