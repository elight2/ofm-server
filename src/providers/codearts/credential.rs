//! CodeArts 凭证（`providers/codearts/credential.toml`）。
//!
//! 字段对齐官方扩展的登录上下文持久化（`loginContext`）：
//! AKSK 三元组 + `refresh_token` + **PKCE verifier 与 DPoP 密钥对**——
//! 官方续期复用同一 `pkcePair.codeVerifier` 与 DPoP 私钥，缺一不可。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::credential;
use crate::providers::codearts::{auth, dpop};

pub const PROVIDER: &str = "codearts";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// IAM 临时 AK。
    pub access_key_id: String,
    /// IAM 临时 SK。
    pub secret_access_key: String,
    /// IAM 临时 security token（请求头 `X-Security-Token`）。
    pub security_token: String,
    /// 续期令牌。
    pub refresh_token: String,
    /// AKSK 过期时间（毫秒时间戳，上游 `credentials.expires_at`）。
    #[serde(default)]
    pub expire_at_millis: Option<i64>,
    /// PKCE `code_verifier`（续期 body 需要原样携带）。
    pub code_verifier: String,
    /// DPoP 密钥对（续期请求需同密钥签 proof）。
    pub dpop_key_pair: dpop::KeyPair,
    /// 本次登录时间（毫秒，排查用）。
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

    /// 由 token 响应 + 登录上下文构造凭证。
    pub fn from_token(
        token: auth::TokenResponse,
        code_verifier: String,
        dpop_key_pair: dpop::KeyPair,
    ) -> Self {
        let expire_at_millis = token.expires_at_millis();
        Self {
            access_key_id: token.credentials.access_key_id,
            secret_access_key: token.credentials.secret_access_key,
            security_token: token.credentials.security_token,
            refresh_token: token.refresh_token,
            expire_at_millis,
            code_verifier,
            dpop_key_pair,
            last_auth_at: Some(now_millis()),
        }
    }
}

pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
