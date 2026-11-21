//! Raccoon（商汤小浣熊 / Raccoon Work）供应商。
//!
//! 已实现：登录（`auth`）、`models`、`balance`、`chat`、`claim`。
//! `refresh` 不对外暴露：服务端虽接受 `/refresh`，但对 IAM token 只原样返还、
//! 不延长期限（客户端 1.2.2 核对 + 实测，见 `provider-info/raccoon.md` §2）；
//! 内部用前自动续期保留。详见 [`RaccoonProvider::refresh`]。
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
    pub fn new(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            root,
            client: api::client()?,
            refresh_lock: Arc::new(Mutex::new(())),
        })
    }

    fn credential(&self) -> Result<Credential, ProviderError> {
        crate::core::credential::require(&self.root, ID)
    }

    /// 取可用 token；JWT 已进入刷新窗口则先续期。
    async fn access_token(&self) -> Result<String, ProviderError> {
        let client = &self.client;
        let root = &self.root;
        let cred = crate::core::credential::renew_if_expired(
            root,
            ID,
            &self.refresh_lock,
            |c: &Credential| credential::needs_refresh(&c.access_token),
            |c: Credential| async move {
                let new = refresh::refresh(client, &c)
                    .await
                    .map_err(ProviderError::from)?;
                new.store(root)
                    .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
                Ok(new)
            },
        )
        .await?;
        Ok(cred.access_token)
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
}

#[async_trait]
impl Provider for RaccoonProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 拉取模型目录（保留错误）。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        models::fetch(&self.client, &self.headers(&token, &cred))
            .await
            .map_err(ProviderError::from)
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
        if !resp.status().is_success() {
            let (status, text) = crate::core::http::error_body(resp, 300).await;
            // 413 = 请求体超 10MB（图片）。属于请求过大，不是凭证问题。
            if status == 413 {
                return Err(ProviderError::Protocol(format!(
                    "请求体超过 10MB 上限: {text}"
                )));
            }
            return Err(ProviderError::from(api::HttpError::Status {
                status,
                body: text,
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

    /// raccoon 不对外暴露主动刷新令牌。
    ///
    /// 官方主进程账号模块拿到 IAM token 时**直接拒绝续期**
    /// （`accountRequests.js`: `isIamToken(access) → throw unauthorized`），
    /// 但它自己的 `scheduleAuth.js` 和渲染层 HTTP 客户端都会照常调 `/refresh`。
    /// 实测（2026-11）：该端点回 `code:0`，但两个 token 均**原样返还**，`exp` 不变，
    /// 即调用它并不能延长期限。故不对外暴露该能力（内部 `access_token` 的自动续期仍保留，
    /// 将来服务端若改为真轮换可自动受益）。
    async fn refresh(&self) -> Result<String, ProviderError> {
        Err(ProviderError::Unsupported(ID))
    }
}
