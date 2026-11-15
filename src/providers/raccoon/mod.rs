//! Raccoon（商汤小浣熊 / Raccoon Work）供应商。
//!
//! 已实现：登录（`auth`）、续期（`refresh`）。
//! `models` / `chat` / `claim` 仍为占位，后续按 `provider-info/raccoon.md` 补齐。
//!
//! 协议实况见 `provider-info/raccoon.md`。

pub mod api;
pub mod auth;
pub mod credential;
pub mod refresh;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use credential::Credential;

pub const ID: &str = "raccoon";

pub struct RaccoonProvider {
    root: PathBuf,
    client: reqwest::Client,
    /// 续期必须串行，避免并发刷新导致 refresh_token 轮换冲突。
    refresh_lock: Arc<Mutex<()>>,
}

impl RaccoonProvider {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: api::client().expect("构建 HTTP 客户端失败"),
            refresh_lock: Arc::new(Mutex::new(())),
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

    /// 取可用 token；JWT 已进入刷新窗口则先续期。
    async fn access_token(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        if !credential::needs_refresh(&cred.access_token) {
            return Ok(cred.access_token);
        }

        let _guard = self.refresh_lock.lock().await;
        // 加锁后重读：可能已被其它任务刷新。
        let cred = self.credential()?;
        if !credential::needs_refresh(&cred.access_token) {
            return Ok(cred.access_token);
        }

        tracing::info!("[{ID}] token 即将过期，正在续期");
        let new = refresh::refresh(&self.client, &cred)
            .await
            .map_err(|e| ProviderError::from(e))?;
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        tracing::info!("[{ID}] 续期成功");
        Ok(new.access_token)
    }

    /// 业务请求头。身份靠 Bearer + 这几个头；非 personal 才带 `X-Org-Code`。
    fn headers(&self, token: &str, cred: &Credential) -> Vec<(&'static str, String)> {
        let mut h = vec![
            ("authorization", format!("Bearer {token}")),
            ("x-raccoon-language", "zh".to_string()),
            ("content-type", "application/json".to_string()),
            ("accept", "*/*".to_string()),
        ];
        if let Some(org) = cred.org_code() {
            h.push(("x-org-code", org.to_string()));
        }
        h
    }

    /// 带头的 POST，返回原始字节（流式用）。
    async fn post_stream(
        &self,
        url: &str,
        body: &serde_json::Value,
    ) -> Result<reqwest::Response, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let payload = serde_json::to_vec(body)
            .map_err(|e| ProviderError::Other(format!("序列化请求体失败: {e}")))?;
        let mut http = self.client.post(url).body(payload);
        for (k, v) in self.headers(&token, &cred) {
            http = http.header(k, v);
        }
        let resp = http.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &text));
        }
        Ok(resp)
    }
}

/// HTTP 层错误映射：401/403 视为凭证失效，其余为普通上游错误。
fn map_http_error(status: u16, body: &str) -> ProviderError {
    let snippet = api::truncate(body, 300);
    match status {
        401 | 403 => ProviderError::Unauthorized(snippet),
        _ => ProviderError::Other(format!("上游 HTTP {status}: {snippet}")),
    }
}

#[async_trait]
impl Provider for RaccoonProvider {
    fn id(&self) -> &'static str {
        ID
    }

    async fn models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .and_then(|c| credential::jwt_exp(&c.access_token))
            .map(|secs| secs * 1000)
    }

    async fn chat(&self, _req: ChatRequest) -> Result<EventStream, ProviderError> {
        Err(ProviderError::Unimplemented("raccoon 对话"))
    }

    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        Err(ProviderError::Unimplemented("raccoon 签到"))
    }
}

/// 供 `main` 判断是否已有登录凭证。
pub fn has_credential(root: &Path) -> bool {
    matches!(Credential::load(root), Ok(Some(_)))
}
