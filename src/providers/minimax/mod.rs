//! MiniMax（MiniMax Code）供应商。
//!
//! 已实现：登录（`auth`，OAuth 设备码 + PKCE）、模型目录（`models`）。
//! 待实现（按 `MAINTAIN.md` 顺序）：`chat`、思考强度映射、`refresh`、`claim`。
//!
//! 协议实况见 `provider-info/minimax.md`。关键约束：
//! * 认证域 `account.minimax.cn` 与 API 域 `agent.minimax.cn` 分离，域名写死预设，
//!   **不读**模型目录下发的旧 `baseURL`（§0 出入 1）；
//! * 登录只能半自动：设备码只是第一步，账号确认页需要微信扫码 + 绑定手机号，
//!   我方只打印 `user_code` 与确认链接并轮询（§0 出入 4）；
//! * 上游是 anthropic-messages 而非 Chat Completions（§3）；
//! * 签到端点未知（§7），客户端本身无签到功能，`claim` 暂不可实现。

pub mod api;
pub mod auth;
pub mod credential;
pub mod models;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::core::events::EventStream;
use crate::core::provider::{
    ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError,
};

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
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: api::client().expect("构建 HTTP 客户端失败"),
            models_client: api::models_client().expect("构建 HTTP 客户端失败"),
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
        let grant = auth::refresh(&self.client, &cred.refresh_token)
            .await
            .map_err(ProviderError::from)?;
        // 上游未回新 refresh_token 时沿用旧值（§2）。
        let expires_at = grant.expires_at();
        let new = Credential {
            access_token: grant.access_token,
            refresh_token: grant
                .refresh_token
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(cred.refresh_token),
            expires_at,
        };
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        tracing::info!("[{ID}] 续期成功");
        Ok(new.access_token)
    }

    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let token = self.access_token().await?;
        models::fetch(&self.models_client, &token)
            .await
            .map_err(ProviderError::from)
    }
}

#[async_trait]
impl Provider for MinimaxProvider {
    fn id(&self) -> &'static str {
        ID
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

    async fn models_checked(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.fetch_models().await
    }

    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .map(|c| c.expires_at)
    }

    /// 对话：尚未实现（`MAINTAIN.md` 第 3 步）。
    ///
    /// 上游是 anthropic-messages，需要完整的 Chat Completions ↔ Messages
    /// 协议转换 + SSE 回转，下一轮实现。
    async fn chat(&self, _req: ChatRequest) -> Result<EventStream, ProviderError> {
        Err(ProviderError::Unsupported("minimax 对话转发（待实现）"))
    }

    /// 签到：端点未知（§7）。
    ///
    /// 官方客户端**没有签到功能**，全网包搜 `checkin`/`sign_in`/`daily_reward`
    /// 也无业务命中，协议文档明确「禁止推测路径」。
    ///
    /// 这里返回 `skipped=true` 而非 `Unsupported`：自动签到循环每轮都会调
    /// `claim`，报错会把「账号有问题」写进 `claim.log`，而事实是我们
    /// **无从知道**该账号有没有签到——不应误报为失败。
    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        Ok(ClaimReport {
            skipped: true,
            message: "无签到功能（官方客户端无签到接口，端点未知）".to_string(),
            credits_delta: None,
            balance: None,
        })
    }

    /// 余额：§6 仅有源码、无抓包，且依赖会员/工作区两条前置请求，暂不实现。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        Ok(None)
    }
}
