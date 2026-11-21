//! MiniMax 凭证（`providers/minimax/credential.toml`）。
//!
//! 字段见 `provider-info/minimax.md` §1。`access_token` / `refresh_token` 都是
//! **不透明串**（`mmoat_` / `mmort_` 前缀，各 60 字符，非 JWT，无法解出到期时间），
//! 故到期判断一律用响应里的 `expires_in` 换算的 `expires_at`（毫秒）。
//! 客户端租约策略是剩余有效期 ≥ 30s（§1）。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::credential;

pub const PROVIDER: &str = "minimax";

/// 提前刷新窗口（毫秒），对应官方的「剩余有效期 ≥ 30s」。
pub const REFRESH_WINDOW_MS: i64 = 30_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// 访问令牌到期时间（毫秒时间戳）。
    #[serde(default)]
    pub expires_at: i64,
}

impl Credential {
    /// 读取凭证。
    pub fn load(root: &Path) -> Result<Option<Self>, credential::CredentialError> {
        credential::load(root, PROVIDER)
    }

    pub fn store(&self, root: &Path) -> Result<(), credential::CredentialError> {
        credential::store(root, PROVIDER, self)
    }

    /// 是否已进入刷新窗口（含未知到期时间的情况）。
    ///
    /// `expires_at` 缺失（旧凭证/上游没给 `expires_in`）时视为需要刷新，
    /// 交给 401 或刷新逻辑兜底，避免拿一个可能已过期的令牌硬发请求。
    pub fn needs_refresh(&self) -> bool {
        now_millis() + REFRESH_WINDOW_MS >= self.expires_at
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
        }
    }

    #[test]
    fn 远期凭证不需刷新() {
        assert!(!cred(now_millis() + 3_600_000).needs_refresh());
    }

    #[test]
    fn 三十秒内需要刷新() {
        assert!(cred(now_millis() + 10_000).needs_refresh());
        assert!(cred(now_millis() - 1).needs_refresh());
    }

    #[test]
    fn 未知到期视为需刷新() {
        assert!(cred(0).needs_refresh());
    }

    #[test]
    fn 凭证可往返() {
        let dir = std::env::temp_dir().join("ofm-minimax-cred-test");
        let _ = std::fs::remove_dir_all(&dir);
        let c = Credential {
            access_token: "at".into(),
            refresh_token: "rt".into(),
            expires_at: 1234,
        };
        c.store(&dir).unwrap();
        let got = Credential::load(&dir).unwrap().expect("应能读回");
        assert_eq!(got.access_token, "at");
        assert_eq!(got.refresh_token, "rt");
        assert_eq!(got.expires_at, 1234);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
