//! WorkBuddy 供应商（腾讯 WorkBuddy 中国版）。
//!
//! **当前阶段只实现登录与令牌续期**（`auth` / `refresh`）：对话、模型目录、
//! 余额与签到待后续阶段按 `provider-info/workbuddy.md` 补齐。
//!
//! 协议实况见 `provider-info/workbuddy.md`。

pub mod api;
pub mod auth;
pub mod credential;
pub mod refresh;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use credential::Credential;

pub const ID: &str = "workbuddy";

pub struct WorkBuddyProvider {
    root: PathBuf,
    client: reqwest::Client,
    /// 续期必须串行，避免并发刷新导致 refresh_token 轮换冲突。
    #[allow(dead_code)]
    refresh_lock: Arc<Mutex<()>>,
}

impl WorkBuddyProvider {
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

    /// 取可用 access token；已进入刷新窗口则先续期。
    ///
    /// 对话转发阶段使用；当前阶段仅供测试与后续接线。
    #[allow(dead_code)]
    async fn access_token(&self) -> Result<String, ProviderError> {
        let client = &self.client;
        let root = &self.root;
        let cred = crate::core::credential::renew_if_expired(
            root,
            ID,
            &self.refresh_lock,
            |c: &Credential| c.needs_refresh(),
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
}

#[async_trait]
impl Provider for WorkBuddyProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 模型目录尚未实现（等 `/v3/config` 映射落地）。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        Err(ProviderError::Unsupported(ID))
    }

    /// 访问令牌到期时间。
    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .map(|c| c.expires_at)
            .filter(|v| *v > 0)
    }

    /// 刷新登录令牌。
    async fn refresh(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        let new = refresh::refresh(&self.client, &cred)
            .await
            .map_err(ProviderError::from)?;
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        Ok(format!(
            "续期成功，访问令牌有效期至 {}",
            crate::core::fmt_secs(new.expires_at / 1000)
        ))
    }

    /// 对话尚未实现。
    async fn chat(&self, _req: ChatRequest) -> Result<EventStream, ProviderError> {
        Err(ProviderError::Unsupported(ID))
    }

    /// 签到尚未实现。
    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        Err(ProviderError::Unsupported(ID))
    }

    /// 余额尚未实现。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 供应商id固定() {
        assert_eq!(ID, "workbuddy");
    }

    /// 未登录时 `access_token` 应给出「请先登录」的提示。
    #[tokio::test]
    async fn 未登录时报未接入() {
        let dir = std::env::temp_dir().join("ofm-workbuddy-provider-test");
        let _ = std::fs::remove_dir_all(&dir);
        let p = WorkBuddyProvider::new(dir.clone()).unwrap();
        let err = p.access_token().await.unwrap_err();
        assert!(
            matches!(&err, ProviderError::NotLoggedIn(m) if m.contains("login workbuddy")),
            "{err:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
