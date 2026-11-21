//! MiniMax（MiniMax Code）供应商。
//!
//! 已实现：登录（`auth`，OAuth 设备码 + PKCE）、模型目录（`models`）、
//! 对话（`chat`，Chat Completions ↔ anthropic-messages）、
//! 令牌续期（被动自动 + `ofm-server refresh`）、每日签到（`claim`）、余额（`balance`）。
//!
//! 协议实况见 `provider-info/minimax.md`。关键约束：
//! * 认证域 `account.minimax.cn` 与 API 域 `agent.minimax.cn` 分离，域名写死预设，
//!   **不读**模型目录下发的旧 `baseURL`（§0 出入 1）；
//! * 登录只能半自动：设备码只是第一步，账号确认页需要微信扫码 + 绑定手机号，
//!   我方只打印 `user_code` 与确认链接并轮询（§0 出入 4）；
//! * 上游是 anthropic-messages 而非 Chat Completions（§3）；
//! * 签到在 `/minimax-cloud/api/v1/signin/{status,claim}`（§5）。

pub mod api;
pub mod auth;
pub mod balance;
pub mod chat;
pub mod claim;
pub mod credential;
pub mod models;
pub mod stream;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use credential::Credential;

/// 供应商 id，同时是模型名前缀与数据目录名。
pub const ID: &str = "minimax";

pub struct MinimaxProvider {
    root: PathBuf,
    /// 对话客户端（UA `MiniMaxAgent`）。
    client: reqwest::Client,
    /// 模型目录客户端（桌面浏览器 UA）。
    models_client: reqwest::Client,
    /// 续期串行，避免并发刷新导致 refresh_token 轮换冲突。
    refresh_lock: Arc<Mutex<()>>,
}

impl MinimaxProvider {
    pub fn new(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            root,
            client: api::client()?,
            models_client: api::models_client()?,
            refresh_lock: Arc::new(Mutex::new(())),
        })
    }

    fn credential(&self) -> Result<Credential, ProviderError> {
        crate::core::credential::require(&self.root, ID)
    }

    /// 取可用 token；进入刷新窗口则先续期（§2）。
    async fn access_token(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        if !cred.needs_refresh() {
            return Ok(cred.access_token);
        }

        let _guard = self.refresh_lock.lock().await;
        // 加锁后重读：可能已被其它任务刷新。
        let cred = self.credential()?;
        if !cred.needs_refresh() {
            return Ok(cred.access_token);
        }

        tracing::info!("[{ID}] token 即将过期，正在续期");
        let new = self.refresh_locked(&cred).await?;
        tracing::info!("[{ID}] 续期成功");
        Ok(new.access_token)
    }

    /// 用 `refresh_token` 换新令牌并落盘（§2）。调用方须持有 `refresh_lock`。
    ///
    /// 上游 `refresh_token` 是**单次使用 + 轮换**（实测 2026-11-17：成功一次后
    /// 旧 RT 立刻 `invalid_grant / this refresh token can no longer be used`）。
    /// 因此必须串行、且落盘失败即视为失败——丢掉新 RT 就只能重新登录。
    async fn refresh_locked(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        if cred.refresh_token.trim().is_empty() {
            return Err(ProviderError::Unauthorized(format!(
                "凭证缺少 refresh_token，请重新运行 `ofm-server login {ID}`"
            )));
        }
        // 上游未回新 refresh_token 时沿用旧值（§2）。
        let mut grant = auth::refresh(&self.client, &cred.refresh_token)
            .await
            .map_err(refresh_error)?;
        let refresh_token = grant
            .refresh_token
            .take()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| cred.refresh_token.clone());
        let expires_at = grant.expires_at();
        let new = Credential {
            access_token: grant.access_token,
            refresh_token,
            expires_at,
        };
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        Ok(new)
    }

    /// 向对话端点发一次请求，返回响应（不做状态码判定）。
    async fn post_chat(
        &self,
        token: &str,
        payload: &[u8],
    ) -> Result<reqwest::Response, ProviderError> {
        let url = format!("{}{}", api::AGENT_HOST, chat::PATH);
        Ok(self
            .client
            .post(&url)
            // 双认证头：`x-api-key` 与 `authorization` 同值（§3.1 抓包 26/26）。
            .header("x-api-key", token)
            .header("authorization", format!("Bearer {token}"))
            .header("anthropic-version", chat::ANTHROPIC_VERSION)
            .body(payload.to_vec())
            .send()
            .await?)
    }
}

#[async_trait]
impl Provider for MinimaxProvider {
    fn id(&self) -> &'static str {
        ID
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let token = self.access_token().await?;
        models::fetch(&self.models_client, &token)
            .await
            .map_err(ProviderError::from)
    }

    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .map(|c| c.expires_at)
    }

    /// 主动刷新令牌（`ofm-server refresh minimax`）。
    ///
    /// 与 `access_token` 的被动续期不同，这里**不检查是否临近过期**：
    /// 用户显式要求刷新就刷新。注意上游 RT 单次使用（见 `refresh_locked`）。
    async fn refresh(&self) -> Result<String, ProviderError> {
        let _guard = self.refresh_lock.lock().await;
        let cred = self.credential()?;
        let new = self.refresh_locked(&cred).await?;
        Ok(format!(
            "续期成功，新凭证到期时间 {}",
            crate::core::fmt_ms(new.expires_at)
        ))
    }

    /// 对话：Chat Completions → 上游 anthropic-messages（§3）。
    ///
    /// 上游恒回 SSE（§3.2），非流式由下游聚合。
    /// 收到 401 时**刷新一次并重试**（§2 `auth-core.ts:330 handleUnauthorized`）：
    /// 令牌实际失效时 `expires_at` 未必已过期（实测 2026-11-17：RT 被别处轮换后
    /// 旧 AT 立即 401，而磁盘上的 `expires_at` 仍指向未来），故不能只靠时间窗。
    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError> {
        let token = self.access_token().await?;
        let body = chat::ChatBody::build(&req);

        // 自己序列化并 .body()：避免 .json() 追加第二个 content-type 头。
        let payload = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Other(format!("序列化请求体失败: {e}")))?;

        let mut resp = self.post_chat(&token, &payload).await?;
        // 401 时刷新一次并重试（§2）。加锁后重读凭证：若已被其它任务刷新过
        // （`access_token != token`）就直接用新凭证重试，避免把 RT 白白再换一次。
        if resp.status().as_u16() == 401 {
            let _guard = self.refresh_lock.lock().await;
            let cred = self.credential()?;
            let fresh = if cred.access_token == token {
                tracing::warn!("[{ID}] 对话收到 401，刷新令牌后重试一次");
                self.refresh_locked(&cred).await?
            } else {
                tracing::debug!("[{ID}] 对话收到 401，但凭证已被其它任务刷新，直接用新凭证重试");
                cred
            };
            resp = self.post_chat(&fresh.access_token, &payload).await?;
        }

        if !resp.status().is_success() {
            let (status, text) = crate::core::http::error_body(resp, 300).await;
            return Err(ProviderError::from(api::HttpError::Status {
                status,
                body: text,
            }));
        }

        let mut parser = stream::Parser::new();
        Ok(crate::core::stream::drive(resp.bytes_stream(), move |ev| {
            parser.parse(ev)
        }))
    }

    /// 每日签到（§5）。
    ///
    /// 端点 `/minimax-cloud/api/v1/signin/{status,claim}`，来自 TUI 源码
    /// `packages/tui/src/checkin/http-gateway.ts`。
    ///
    /// 先查面板再决定是否领取：今日已签则直接跳过，不打写请求；
    /// 未领到时返回 `skipped=true` 而非错误，避免 `claim.log` 每天记假警报
    /// （自动签到循环每轮都会调 `claim`）。具体判定见 `claim.rs`。
    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        let token = self.access_token().await?;
        let mut cred = self.credential()?;
        cred.access_token = token;
        claim::claim(&self.client, &cred).await
    }

    /// 余额：取 `op_credit_summary.total_remaining_amount`（§5）。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        let token = self.access_token().await?;
        let mut cred = self.credential()?;
        cred.access_token = token;
        balance::fetch(&self.client, &cred).await
    }
}

/// 续期错误归一：RT 确实失效时提示重新登录，其余保留上游原因。
///
/// 上游用 `400 {"error":"invalid_grant","error_description":"this refresh token
/// can no longer be used, start a new authorization"}` 表达「RT 已用掉/已失效」，
/// 此时**没有**任何自动恢复路径（§1 凭证存储末），只能重走登录。
fn refresh_error(e: api::HttpError) -> ProviderError {
    match &e {
        api::HttpError::OAuth { error, .. }
            if error == "invalid_grant" || error == "invalid_token" =>
        {
            ProviderError::Unauthorized(format!(
                "refresh_token 已失效（{e}），请重新运行 `ofm-server login {ID}`"
            ))
        }
        _ => ProviderError::from(e),
    }
}
