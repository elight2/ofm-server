//! Raccoon（商汤小浣熊 / Raccoon Work）供应商。
//!
//! 已实现：登录（`auth`）、`models`、`balance`、`chat`、`claim`。
//! `refresh` 不支持：官方客户端对 IAM token 直接拒绝续期，详见 [`RaccoonProvider::refresh`]。
//!
//! 协议实况见 `provider-info/raccoon.md`。

pub mod api;
pub mod auth;
pub mod balance;
pub mod chat;
pub mod claim;
pub mod credential;
pub mod models;
pub mod refresh;
pub mod stream;

use std::path::PathBuf;
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
            .map_err(ProviderError::from)?;
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

    /// 拉取模型目录（保留错误）。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        models::fetch(&self.client, &self.headers(&token, &cred))
            .await
            .map_err(ProviderError::from)
    }
}

#[async_trait]
impl Provider for RaccoonProvider {
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

    /// 带错误的目录拉取：`status` 用它真实探测接入是否可用。
    async fn models_checked(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.fetch_models().await
    }

    /// 读当前余额（主数字取 `available_points`）。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let b = balance::fetch(&self.client, &self.headers(&token, &cred))
            .await
            .map_err(ProviderError::from)?;
        Ok(Some(b.main_points()))
    }

    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .and_then(|c| credential::jwt_exp(&c.access_token))
            .map(|secs| secs * 1000)
    }

    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let body = chat::ChatBody::build(&req);
        let url = format!("{}{}{}", api::HOST, api::LLM_PREFIX, chat::PATH);

        // 自己序列化并 .body()：避免 .json() 追加第二个 content-type 头。
        let payload = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Other(format!("序列化请求体失败: {e}")))?;
        let mut http = self.client.post(&url).body(payload);
        for (k, v) in self.headers(&token, &cred) {
            http = http.header(k, v);
        }

        let resp = http.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            // 413 = 请求体超 10MB（图片）。属于请求过大，不是凭证问题。
            if status.as_u16() == 413 {
                return Err(ProviderError::Protocol(format!(
                    "请求体超过 10MB 上限: {}",
                    api::truncate(&text, 200)
                )));
            }
            return Err(ProviderError::from(api::HttpError::Status {
                status: status.as_u16(),
                body: api::truncate(&text, 300),
            }));
        }

        // 上游恒回 SSE；分帧后逐帧转成中间事件。
        let mut parser = stream::FrameParser::new();
        Ok(crate::core::stream::drive(resp.bytes_stream(), move |ev| {
            parser.parse(ev)
        }))
    }

    /// 领取移动端登录积分（等价官方客户端的「登录送积分」）。
    ///
    /// 实测每日 300 分**只能靠真实登录触发**：拉余额、续期 token、发对话都不行。
    /// 服务端幂等，同一天重复调用返回 `granted: false`，映射为 `skipped`。
    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        let cred = self.credential()?;
        claim::claim(&self.client, &cred).await
    }

    /// raccoon 不支持主动刷新令牌。
    ///
    /// 官方客户端拿到 IAM token 时**直接拒绝续期**
    /// （`accountRequests.js`: `isIamToken(access) → throw unauthorized`），
    /// 即使遗留的 refresh_token 仍在磁盘。实测该端点虽回 `code:0`，
    /// 但未临近过期时只会原样返还同一 token，且偏离官方行为，
    /// 故不对外暴露该能力（内部 `access_token` 的自动续期仍保留）。
    async fn refresh(&self) -> Result<String, ProviderError> {
        Err(ProviderError::Unsupported(ID))
    }
}

