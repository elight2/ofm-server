//! CodeArts（华为云 CodeArts Snap / Agent）供应商。
//!
//! 已实现：登录（`auth`，PKCE + DPoP + 本地回调）、续期（`refresh`）、
//! 模型列表（`catalog`，PromptCenter builtin）、签到/余额（`ops`，
//! SDK-HMAC-SHA256 签名见 `sign`）、对话（`chat`，明文 CC SSE）。
//!
//! 协议实况见 `provider-info/codearts.md`。

pub mod api;
pub mod auth;
pub mod catalog;
pub mod chat;
pub mod credential;
pub mod dpop;
pub mod ops;
pub mod pkce;
pub mod refresh;
pub mod sign;
pub mod stream;
pub mod variants;

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::config::CodeartsReasoningEffort;
use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use api::HttpError;

pub const ID: &str = "codearts";

pub struct CodeartsProvider {
    root: PathBuf,
    client: reqwest::Client,
    chat_client: reqwest::Client,
    /// AKSK 续期必须串行，避免并发刷新导致 refresh_token 轮换冲突。
    refresh_lock: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// 思考强度。
    effort: CodeartsReasoningEffort,
    /// 上游思考档位缓存（逐模型，TTL [`variants::REFRESH_INTERVAL`]）。
    variants: std::sync::Arc<variants::VariantsCache>,
}

impl CodeartsProvider {
    pub fn new(root: PathBuf, effort: CodeartsReasoningEffort) -> anyhow::Result<Self> {
        let chat_client = api::chat_client()?;
        Ok(Self {
            root,
            client: api::token_client()?,
            variants: variants::VariantsCache::new(chat_client.clone()),
            chat_client,
            refresh_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            effort,
        })
    }

    fn credential(&self) -> Result<credential::Credential, ProviderError> {
        crate::core::credential::require(&self.root, ID)
    }

    /// 取可用凭证：AKSK 已过期则先续期（上游 STS 有效期约 1h，模型/对话请求都依赖它）。
    async fn valid_credential(&self) -> Result<credential::Credential, ProviderError> {
        let client = &self.client;
        let root = &self.root;
        crate::core::credential::renew_if_expired(
            root,
            ID,
            &self.refresh_lock,
            |c: &credential::Credential| c.aks_expired(),
            |c: credential::Credential| async move { self.renew(client, root, c).await },
        )
        .await
    }

    /// 续期并落盘。
    async fn renew(
        &self,
        client: &reqwest::Client,
        root: &Path,
        cred: credential::Credential,
    ) -> Result<credential::Credential, ProviderError> {
        if cred.refresh_token.is_empty() {
            return Err(ProviderError::Unauthorized(
                "凭证没有 refresh_token（ticket 换票登录），请重新运行 `ofm-server login codearts`"
                    .to_string(),
            ));
        }

        let new = refresh::refresh(client, &cred)
            .await
            .map_err(refresh_error)?;
        new.store(root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        Ok(new)
    }
}

#[async_trait]
impl Provider for CodeartsProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 拉取模型目录（保留错误，独立方法便于单测与错误传播）。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let cred = self.valid_credential().await?;
        catalog::fetch_models(&self.client, &cred).await
    }

    /// 登录后凭证包含 AKSK 三元组（`access_key_id`/`secret_access_key`/
    /// `security_token`），有效期以上游 `expires_at` 为准。
    fn credential_expire_at(&self) -> Option<i64> {
        credential::Credential::load(&self.root)
            .ok()
            .flatten()
            .and_then(|c| c.expire_at_millis)
    }

    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError> {
        let cred = self.valid_credential().await?;
        // 思考档位需按模型查上游；取不到（或不支持）则不发送该字段。
        let effort = self
            .variants
            .get(&cred, std::slice::from_ref(&req.model))
            .await
            .ok()
            .and_then(|v| v.pick(&req.model, self.effort));
        chat::stream(&self.chat_client, &cred, &req, effort.as_deref()).await
    }

    /// 签到：遍历 `benefit`/`education`/`examination` 三个渠道（IDE `SLc` 流程）。
    /// 签到：走「限时福利 / 每日签到」的 `/v1/ops/*`（真正的积分签到）。
    ///
    /// 注意与历史实现的区别：旧实现走 benefit-service
    /// （`/api/v1/benefit/claim`，福利模型额度渠道），那条路径对本账号恒返回
    /// `4000 invalid channel, group not found`，**不是**界面上的「每日签到」。
    /// 见 `provider-info/codearts.md` §7.2。
    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        let cred = self.valid_credential().await?;
        ops::claim(&self.client, &cred).await
    }

    /// 余额：账户积分剩余量（`/snap-manager/v1/statistics/plugin`）。
    ///
    /// UI 显示「总已用积分: 已用/总量」，这里返回 `总量 − 已用`。
    /// 旧实现走 benefit-service（福利模型额度渠道），对本账号恒 `4000`，已弃用。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        let cred = self.valid_credential().await?;
        ops::fetch_balance(&self.client, &cred).await
    }

    /// 续期：复用持久化的 PKCE verifier + DPoP 密钥换新 AKSK。
    ///
    /// 与被动续期不同，这里**不检查是否过期**：用户显式要求刷新就刷新
    /// （`refresh_token` 会轮换，刷新后旧令牌即失效，需立即落盘）。
    async fn refresh(&self) -> Result<String, ProviderError> {
        let _guard = self.refresh_lock.lock().await;
        let cred = self.credential()?;
        let new = self.renew(&self.client, &self.root, cred).await?;
        Ok(match new.expire_at_millis {
            Some(ms) => format!("续期成功，新凭证到期时间 {}", crate::core::fmt_ms(ms)),
            None => "续期成功（上游未返回有效期）".to_string(),
        })
    }
}

/// 续期错误归一：令牌确实失效时提示重新登录，其余保留上游原因。
fn refresh_error(e: HttpError) -> ProviderError {
    match &e {
        HttpError::Status { status, body }
            if *status == 401
                || body.contains("invalid_grant")
                || body.contains("invalid_token")
                || body.contains("expired") =>
        {
            ProviderError::Unauthorized(format!(
                "refresh_token 已失效（{e}），请重新运行 `ofm-server login {ID}`"
            ))
        }
        _ => ProviderError::Other(format!("续期失败: {e}")),
    }
}
