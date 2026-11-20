//! Raccoon 凭证（`providers/raccoon/credential.toml`）。
//!
//! 字段见 `provider-info/raccoon.md` §1。`access_token` 是 JWT，过期与否
//! 直接解 `exp` 判断（官方同样如此：`TOKEN_REFRESH_WINDOW_SECONDS=300`）。
//! `office_identity` 非 `personal` 时所有业务请求都要带 `X-Org-Code`。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::credential;

pub const PROVIDER: &str = "raccoon";

/// 提前刷新窗口（秒），与官方 `TOKEN_REFRESH_WINDOW_SECONDS` 一致。
pub const REFRESH_WINDOW_SECONDS: i64 = 300;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    /// `personal` 或组织码。非 `personal` 时请求带 `X-Org-Code`。
    #[serde(default)]
    pub office_identity: Option<String>,
    #[serde(default)]
    pub office_org_name: Option<String>,
    #[serde(default)]
    pub office_org_role: Option<String>,
    /// 最近一次登录/续期的时间（毫秒）。每日积分延期依赖「当天登录」，
    /// 记录它便于观察续期是否真的发生。
    #[serde(default)]
    pub last_auth_at: Option<i64>,
}

impl Credential {
    pub fn load(root: &Path) -> Result<Option<Self>, credential::CredentialError> {
        credential::load(root, PROVIDER)
    }

    pub fn store(&self, root: &Path) -> Result<(), credential::CredentialError> {
        credential::store(root, PROVIDER, self)
    }

    /// 是否为企业身份（决定了要不要带 `X-Org-Code`）。
    pub fn is_org(&self) -> bool {
        match self.office_identity.as_deref() {
            Some(id) => !id.is_empty() && id != "personal",
            None => false,
        }
    }

    /// 组织码（`office_identity` 取值为组织码时才返回）。
    pub fn org_code(&self) -> Option<&str> {
        if self.is_org() {
            self.office_identity.as_deref()
        } else {
            None
        }
    }
}

/// 解析 JWT 的 `exp`（Unix 秒），失败返回 `None`（视为不可判断）。
pub fn jwt_exp(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let decoded =
        base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, payload)
            .ok()
            .or_else(|| {
                base64::Engine::decode(&base64::engine::general_purpose::URL_SAFE, payload).ok()
            })?;
    let v: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    v.get("exp")?.as_i64()
}

/// 当前 Unix 秒 / 毫秒（同 crate 便利转出）。
pub use crate::core::clock::{now_millis, now_secs};

/// token 是否已进入刷新窗口。无 `exp` 时返回 `false`（交给上游 401 兜底）。
pub fn needs_refresh(token: &str) -> bool {
    match jwt_exp(token) {
        Some(exp) => now_secs() + REFRESH_WINDOW_SECONDS >= exp,
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手工构造 payload 为 `{"exp":N}` 的 token（无签名，只测解析）。
    fn token_with_exp(exp: i64) -> String {
        use base64::Engine;
        let payload =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(format!(r#"{{"exp":{exp}}}"#));
        format!("h.{payload}.s")
    }

    #[test]
    fn parses_exp() {
        assert_eq!(jwt_exp(&token_with_exp(1_700_000_000)), Some(1_700_000_000));
    }

    #[test]
    fn garbage_token_has_no_exp() {
        assert_eq!(jwt_exp("not-a-jwt"), None);
        assert_eq!(jwt_exp("a.!!!.c"), None);
    }

    #[test]
    fn refresh_window() {
        assert!(needs_refresh(&token_with_exp(now_secs() - 1)));
        assert!(needs_refresh(&token_with_exp(now_secs() + 100)));
        assert!(!needs_refresh(&token_with_exp(now_secs() + 3600)));
        // 无 exp 不主动刷新
        assert!(!needs_refresh("opaque-token"));
    }

    #[test]
    fn org_identity() {
        let mut c = Credential {
            access_token: "t".into(),
            refresh_token: String::new(),
            office_identity: Some("personal".into()),
            office_org_name: None,
            office_org_role: None,
            last_auth_at: None,
        };
        assert!(!c.is_org());
        assert_eq!(c.org_code(), None);
        c.office_identity = Some("ORG123".into());
        assert!(c.is_org());
        assert_eq!(c.org_code(), Some("ORG123"));
        c.office_identity = None;
        assert!(!c.is_org());
    }
}
