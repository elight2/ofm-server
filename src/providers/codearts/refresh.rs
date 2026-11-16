//! CodeArts 续期：`refresh_token` 换新 AKSK。
//!
//! 官方扩展（`refreshToken`）：body 为 `{client_id, code_verifier, grant_type:
//! "refresh_token", refresh_token}`——**带 `code_verifier`**（与 kernel 不同），
//! DPoP 密钥复用登录时的密钥对。

use super::api::{self, HttpError};
use super::auth::{self, TokenResponse};
use super::credential::Credential;
use super::dpop;

/// 续期并返回新凭证（调用方负责落盘）。
pub async fn refresh(client: &reqwest::Client, cred: &Credential) -> Result<Credential, HttpError> {
    let url = format!("{}{}", api::IAM_STS_OPEN, api::TOKEN_PATH);
    let form = [
        ("client_id", api::CLIENT_ID),
        ("code_verifier", cred.code_verifier.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", cred.refresh_token.as_str()),
    ];
    let token: TokenResponse = auth::request_token(client, &cred.dpop_key_pair, &url, &form).await?;
    Ok(Credential::from_token(
        token,
        cred.code_verifier.clone(),
        clone_keys(&cred.dpop_key_pair),
    ))
}

/// `KeyPair` 未实现 Clone 手动复制（PEM + JWK 都是小字符串）。
fn clone_keys(keys: &dpop::KeyPair) -> dpop::KeyPair {
    dpop::KeyPair {
        private_key_pem: keys.private_key_pem.clone(),
        public_jwk: keys.public_jwk.clone(),
    }
}
