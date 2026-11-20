//! WorkBuddy 凭证（`providers/workbuddy/credential.toml`）。
//!
//! 字段见 `provider-info/workbuddy.md` §1.3。`access_token` / `refresh_token`
//! 都是**不透明串**（上游不给 JWT，也不保证能解出 `exp`），故到期时间一律
//! 用响应里的 `expires_in` / `refresh_expires_in` 换算成 `expires_at`
//! （官方客户端 `calculateExpiresAt` 同款做法）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::credential;

pub const PROVIDER: &str = "workbuddy";

/// 提前刷新窗口（毫秒）。
///
/// 官方在访问令牌即将过期时静默续期；这里取 5 分钟，给续期留足网络余量。
pub const REFRESH_WINDOW_MS: i64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// 访问令牌（`Authorization: Bearer`）。
    pub access_token: String,
    /// 刷新令牌（`X-Refresh-Token`）。
    #[serde(default)]
    pub refresh_token: String,
    /// 访问令牌到期时间（毫秒时间戳）。
    #[serde(default)]
    pub expires_at: i64,
    /// 刷新令牌到期时间（毫秒时间戳）。过期后只能重新登录。
    #[serde(default)]
    pub refresh_expires_at: i64,
    /// 账号 uid（`X-User-Id`）。
    #[serde(default)]
    pub uid: String,
    /// 企业 id（`X-Enterprise-Id` / `X-Tenant-Id`），个人账号为空。
    #[serde(default)]
    pub enterprise_id: String,
    /// 租户域名（`X-Domain`），可选。
    #[serde(default)]
    pub domain: String,
    /// 账号昵称，仅用于展示。
    #[serde(default)]
    pub nickname: String,
}

impl Credential {
    pub fn load(root: &Path) -> Result<Option<Self>, credential::CredentialError> {
        credential::load(root, PROVIDER)
    }

    pub fn store(&self, root: &Path) -> Result<(), credential::CredentialError> {
        credential::store(root, PROVIDER, self)
    }

    /// 是否已进入刷新窗口。
    ///
    /// `expires_at` 缺失（上游没给 `expires_in`）时视为需要刷新，交给 401 兜底，
    /// 避免拿一个可能已过期的令牌硬发请求。
    pub fn needs_refresh(&self) -> bool {
        now_millis() + REFRESH_WINDOW_MS >= self.expires_at
    }

    /// 刷新令牌是否已过期（过期后只能重新登录）。
    pub fn refresh_expired(&self) -> bool {
        self.refresh_expires_at > 0 && now_millis() >= self.refresh_expires_at
    }

    /// 是否为企业账号（决定要不要带 `X-Enterprise-Id`）。
    pub fn is_enterprise(&self) -> bool {
        !self.enterprise_id.trim().is_empty()
    }
}

/// 当前 Unix 毫秒（同 crate 便利转出）。
pub use crate::core::clock::now_millis;

#[cfg(test)]
mod tests {
    use super::*;

    fn cred(expires_at: i64) -> Credential {
        Credential {
            access_token: "t".into(),
            refresh_token: "r".into(),
            expires_at,
            refresh_expires_at: 0,
            uid: "u".into(),
            enterprise_id: String::new(),
            domain: String::new(),
            nickname: String::new(),
        }
    }

    #[test]
    fn 远期凭证不需刷新() {
        assert!(!cred(now_millis() + 3_600_000).needs_refresh());
    }

    #[test]
    fn 五分钟内需要刷新() {
        assert!(cred(now_millis() + 60_000).needs_refresh());
        assert!(cred(now_millis() - 1).needs_refresh());
    }

    #[test]
    fn 未知到期视为需刷新() {
        assert!(cred(0).needs_refresh());
    }

    #[test]
    fn 刷新令牌过期判断() {
        let mut c = cred(now_millis() + 3_600_000);
        assert!(!c.refresh_expired());
        c.refresh_expires_at = now_millis() - 1;
        assert!(c.refresh_expired());
        // 0 = 未知，不算过期
        c.refresh_expires_at = 0;
        assert!(!c.refresh_expired());
    }

    #[test]
    fn 企业账号判定() {
        let mut c = cred(now_millis() + 3_600_000);
        assert!(!c.is_enterprise());
        c.enterprise_id = "ent-1".into();
        assert!(c.is_enterprise());
        c.enterprise_id = "  ".into();
        assert!(!c.is_enterprise());
    }

    #[test]
    fn 凭证可往返() {
        let dir = std::env::temp_dir().join("ofm-workbuddy-cred-test");
        let _ = std::fs::remove_dir_all(&dir);
        let c = Credential {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: 1234,
            refresh_expires_at: 5678,
            uid: "u".into(),
            enterprise_id: "e".into(),
            domain: "d".into(),
            nickname: "n".into(),
        };
        c.store(&dir).unwrap();
        let got = Credential::load(&dir).unwrap().expect("应能读回");
        assert_eq!(got.access_token, "at");
        assert_eq!(got.refresh_token, "rt");
        assert_eq!(got.expires_at, 1234);
        assert_eq!(got.refresh_expires_at, 5678);
        assert_eq!(got.uid, "u");
        assert_eq!(got.enterprise_id, "e");
        assert_eq!(got.domain, "d");
        assert_eq!(got.nickname, "n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
