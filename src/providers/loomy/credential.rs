//! Loomy 凭证管理。
//!
//! 凭证文件格式（TOML）：
//! ```toml
//! session = "<讯飞账号 session>"
//! userid = "<讯飞账号 userid>"
//! expire_at = 1700000000000
//! ```
//!
//! `session` 由讯飞账号网关（`account.xfinfr.com`）下发，有效期 14 天，
//! **无续期令牌**，过期只能重新登录。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::credential;

pub const PROVIDER: &str = "loomy";

/// 凭证。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// 会话令牌：账号网关与模型 API 共用。
    pub session: String,
    /// 讯飞账号 id。
    pub userid: String,
    /// 登录态到期时间（毫秒时间戳）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expire_at: Option<i64>,
}

impl Credential {
    /// 由登录结果构造，`expire_at` 按 14 天推算。
    pub fn from_login(session: String, userid: String) -> Self {
        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        Self {
            session,
            userid,
            expire_at: Some(millis + super::account::SESSION_EXPIRE_SECS * 1000),
        }
    }

    /// 是否已过期（无到期时间时视为未过期）。
    pub fn is_expired(&self) -> bool {
        let Some(expire_at) = self.expire_at else {
            return false;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        now >= expire_at
    }

    pub fn load(root: &Path) -> Result<Option<Self>, credential::CredentialError> {
        credential::load(root, PROVIDER)
    }

    pub fn store(&self, root: &Path) -> Result<(), credential::CredentialError> {
        credential::store(root, PROVIDER, self)
    }
}
