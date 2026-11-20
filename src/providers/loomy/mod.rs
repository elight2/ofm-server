//! Loomy 供应商。
//!
//! 已实现：登录（`auth`）、模型列表（`catalog`）、对话（`chat`）。
//! 协议实况见 `provider-info/loomy.md`。

pub mod account;
pub mod api;
pub mod auth;
pub mod catalog;
pub mod chat;
pub mod sign;

use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::LoomyReasoningEffort;
use crate::core::claim::{ClaimDetail, ClaimOutcome, SkipReason};
use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use api::VERSION;
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

    /// 取可用 session。
    ///
    /// 讯飞账号登录态 14 天有效且无法续期，过期只能重新登录，故这里先本地判过期，
    /// 免得把注定失败的请求打到上游。
    async fn access_token(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        if cred.is_expired() {
            return Err(ProviderError::Unauthorized(format!(
                "登录态已过期，请重新运行 `ofm-server login {ID}`"
            )));
        }
        Ok(cred.session.clone())
    }

    /// 请求模型目录。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let token = self.access_token().await?;
        let url = format!("{}/models", api::MODEL_BASE_URL);

        let mut http = self.client.get(&url);
        for (k, v) in self.auth_headers(&token) {
            http = http.header(k, v);
        }

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

    /// 鉴权头：`Authorization: Bearer` + `token`，两者都发登录态。
    fn auth_headers(&self, token: &str) -> Vec<(&'static str, String)> {
        vec![
            ("Authorization", format!("Bearer {token}")),
            ("token", token.to_string()),
            ("Accept", "application/json".to_string()),
        ]
    }

    /// 对话请求头。
    ///
    /// 除鉴权外还要带 `traceparent` 与客户端版本头：缺失 `traceparent` 时
    /// 上游 `chat/completions` 会挂住直到超时（`provider-info/loomy.md` §3.4）。
    fn chat_headers(&self, token: &str) -> Vec<(&'static str, String)> {
        let mut headers = self.auth_headers(token);
        headers.push(("Content-Type", "application/json".to_string()));
        headers.push(("traceparent", sign::traceparent()));
        headers.push(("ChatId", sign::trace_id()));
        headers.push(("MsgId", format_msg_id()));
        headers.push(("loomy-version", VERSION.to_string()));
        headers
    }
}

#[async_trait]
impl Provider for LoomyProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 登录态到期时间。
    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .and_then(|c| c.expire_at)
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
        let token = self.access_token().await?;
        let body = chat::build_body(&req, self.effort);
        let url = format!("{}/chat/completions", api::MODEL_BASE_URL);

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

    /// 主动刷新登录态。
    ///
    /// 讯飞账号不提供续期令牌：`session` 有效期 14 天，过期只能重新登录
    /// （`ofm-server login loomy`）。
    async fn refresh(&self) -> Result<String, ProviderError> {
        Err(ProviderError::Unsupported(ID))
    }

    /// 当前余额。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        let token = self.access_token().await?;
        let url = format!("{}/points/records", api::MODEL_BASE_URL);

        let mut http = self.client.get(&url);
        for (k, v) in self.auth_headers(&token) {
            http = http.header(k, v);
        }
        let resp = http.send().await?;

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
        let token = self.access_token().await?;
        let url = format!("{}/checkin", api::MODEL_BASE_URL);

        let mut http = self.client.post(&url);
        for (k, v) in self.auth_headers(&token) {
            http = http.header(k, v);
        }
        let resp = http.send().await?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &body));
        }

        let data: serde_json::Value = resp.json().await?;
        let claimed = data
            .get("claimed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
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

/// 上游要求的消息 id（客户端用 `msg-<uuid>` 形式）。
fn format_msg_id() -> String {
    format!("msg-{}", sign::trace_id())
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
