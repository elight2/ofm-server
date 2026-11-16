//! CodeArts（华为云 CodeArts Snap / Agent）供应商。
//!
//! 已实现：登录（`auth`，PKCE + DPoP + 本地回调）、续期（`refresh`）、
//! 模型列表（`benefit`，SDK-HMAC-SHA256 签名见 `sign`）。`chat` / `claim`
//! 尚未实现——需先取得真实凭证实测上游明文/加密链路
//! （见 `provider-info/codearts.md` §8 未验证清单）。
//!
//! 协议实况见 `provider-info/codearts.md`。

pub mod api;
pub mod auth;
pub mod catalog;
pub mod credential;
pub mod dpop;
pub mod pkce;
pub mod refresh;
pub mod sign;

use std::path::PathBuf;

use async_trait::async_trait;

use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use api::HttpError;

pub const ID: &str = "codearts";

pub struct CodeartsProvider {
    root: PathBuf,
    client: reqwest::Client,
    /// AKSK 续期必须串行，避免并发刷新导致 refresh_token 轮换冲突。
    refresh_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
}

impl CodeartsProvider {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: api::token_client().expect("构建 HTTP 客户端失败"),
            refresh_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn credential(&self) -> Result<credential::Credential, ProviderError> {
        match credential::Credential::load(&self.root) {
            Ok(Some(c)) => Ok(c),
            Ok(None) => Err(ProviderError::NotLoggedIn(format!(
                "{}（请先运行 `ofm-server login {ID}`）",
                crate::config::Config::credential_path(&self.root, ID).display()
            ))),
            Err(e) => Err(ProviderError::Other(e.to_string())),
        }
    }

    /// 取可用凭证：AKSK 已过期则先续期（上游 STS 有效期约 1h，模型/对话请求都依赖它）。
    async fn valid_credential(&self) -> Result<credential::Credential, ProviderError> {
        let cred = self.credential()?;
        if !cred.aks_expired() {
            return Ok(cred);
        }

        let _guard = self.refresh_lock.lock().await;
        // 加锁后重读：可能已被其它任务刷新。
        let cred = self.credential()?;
        if !cred.aks_expired() {
            return Ok(cred);
        }

        tracing::info!("[{ID}] AKSK 已过期，正在续期");
        let new = refresh::refresh(&self.client, &cred)
            .await
            .map_err(|e| match e {
                HttpError::Status { status: 401, .. } => {
                    ProviderError::Unauthorized("refresh_token 已失效，请重新登录".to_string())
                }
                other => ProviderError::Other(format!("续期失败: {other}")),
            })?;
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        tracing::info!("[{ID}] AKSK 续期成功");
        Ok(new)
    }

    /// 拉取模型目录（保留错误，独立方法便于单测与错误传播）。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let cred = self.valid_credential().await?;
        catalog::fetch_models(&self.client, &cred).await
    }
}

#[async_trait]
impl Provider for CodeartsProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 登录后凭证包含 AKSK 三元组（`access_key_id`/`secret_access_key`/
    /// `security_token`），有效期以上游 `expires_at` 为准。
    fn credential_expire_at(&self) -> Option<i64> {
        credential::Credential::load(&self.root)
            .ok()
            .flatten()
            .and_then(|c| c.expire_at_millis)
    }

    /// 拉取模型目录。错误保留给 `models_checked`，这里只记日志。
    async fn models(&self) -> Vec<ModelInfo> {
        match self.fetch_models().await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("[{ID}] 获取模型目录失败: {e}");
                Vec::new()
            }
        }
    }

    /// 带错误的目录拉取：`status` 用它真实探测接入是否可用。
    async fn models_checked(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.fetch_models().await
    }

    async fn chat(&self, _req: ChatRequest) -> Result<EventStream, ProviderError> {
        Err(ProviderError::Unsupported("codearts chat"))
    }

    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        Err(ProviderError::Unsupported("codearts claim"))
    }

    /// 续期：复用持久化的 PKCE verifier + DPoP 密钥换新 AKSK。
    async fn refresh(&self) -> Result<String, ProviderError> {
        let _guard = self.refresh_lock.lock().await;
        let cred = self.credential()?;
        let new = refresh::refresh(&self.client, &cred)
            .await
            .map_err(|e| match e {
                HttpError::Status { status: 401, .. } => {
                    ProviderError::Unauthorized("refresh_token 已失效，请重新登录".to_string())
                }
                other => ProviderError::Other(format!("续期失败: {other}")),
            })?;
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        Ok(match new.expire_at_millis {
            Some(ms) => format!("续期成功，新凭证到期时间 {}", crate::core::fmt_ms(ms)),
            None => "续期成功（上游未返回有效期）".to_string(),
        })
    }
}
