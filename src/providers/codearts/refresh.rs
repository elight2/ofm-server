//! CodeArts 续期：`refresh_token` 换新 AKSK。
//!
//! 官方有两条等价的续期形态（`provider-info/codearts.md` §2）：
//!
//! | 客户端 | body | 额外头 |
//! |-|-|-|
//! | IDE 扩展 | `client_id=codearts-snap` + `code_verifier` + `grant_type` + `refresh_token` | `DPoP` |
//! | agent kernel | `client_id=CodeArts_Tui`，**不带** `code_verifier` | `DPoP`、`Accept`、`x-agent-user-account` |
//!
//! 本实现以 IDE 形态为主（`client_id` 与登录一致，`code_verifier` 是登录上下文
//! 的一部分），仅在服务端明确拒绝（非 5xx、非网络错误）时回退 kernel 形态，
//! 因为 `refresh_token` 是**一次性轮换**的：先失败的那次不会消耗令牌，回退安全。
//!
//! 注意：`refresh_token` 会在响应里轮换（JWT 的 `family_id` 不变、`counter` 递增），
//! 旧值随即失效，因此刷新必须串行（调用方持 `refresh_lock`）。

use super::api::{self, HttpError};
use super::auth::{self, TokenResponse};
use super::credential::Credential;
use super::dpop;

/// 服务端返回的「refresh token 不可用」错误标识。
///
/// 华为 STS 实际用的是 `{"error_code":"STS5.1806","error_msg":"invalid refresh
/// token: '<原因>'"}`，**不含** OAuth 标准的 `invalid_grant`；只认标准码会让
/// 失效令牌被误判为可重试，进而回退 kernel 形态、最终报出误导性的
/// `invalid client id`（§2.1）。
const INVALID_GRANT: &[&str] = &[
    "invalid_grant",
    "invalid_token",
    "expired_token",
    "refresh_token_expired",
    "refresh_token_invalid",
    "STS5.1806",
    "invalid refresh token",
];

/// 续期并返回新凭证（调用方负责落盘）。
pub async fn refresh(client: &reqwest::Client, cred: &Credential) -> Result<Credential, HttpError> {
    let url = format!("{}{}", api::IAM_STS_OPEN, api::TOKEN_PATH);
    let ide = ide_form(cred);
    let first = auth::request_token(client, &cred.dpop_key_pair, &url, &ide).await;

    let token = match first {
        Ok(token) => token,
        Err(err) if is_retryable(&err) => {
            tracing::debug!(%err, "[codearts] IDE 形态续期被拒，回退 kernel 形态重试");
            let kernel = kernel_form(cred);
            auth::request_token(client, &cred.dpop_key_pair, &url, &kernel).await?
        }
        Err(err) => return Err(err),
    };

    let refresh_token = keep_refresh_token(&token, &cred.refresh_token);
    Credential::from_token(
        token,
        cred.code_verifier.clone(),
        clone_keys(&cred.dpop_key_pair),
    )
    .map(|mut c| {
        // 续期不换 refresh_token 时沿用旧值（避免把凭证写成空串）。
        if c.refresh_token.is_empty() {
            c.refresh_token = refresh_token;
        }
        c
    })
    .map_err(|e| HttpError::Protocol(e.to_string()))
}

/// IDE 扩展形态：`refreshToken()` 里的 body，带 `code_verifier`。
fn ide_form(cred: &Credential) -> [(&'static str, &str); 4] {
    [
        ("client_id", api::CLIENT_ID),
        ("code_verifier", cred.code_verifier.as_str()),
        ("grant_type", "refresh_token"),
        ("refresh_token", cred.refresh_token.as_str()),
    ]
}

/// kernel 形态：不带 `code_verifier`（`x-agent-user-account` 头由本实现省略——
/// 它是用户账号 id，凭证里没有该字段，官方在缺失时传空串）。
fn kernel_form(cred: &Credential) -> [(&'static str, &str); 3] {
    [
        ("client_id", api::CLIENT_ID_KERNEL),
        ("grant_type", "refresh_token"),
        ("refresh_token", cred.refresh_token.as_str()),
    ]
}

/// 是否值得换一种形态重试：网络错误、5xx、或请求被拒（4xx 而非令牌失效）。
///
/// 令牌确实失效（`invalid_grant` 等）时不重试——换哪种形态都救不回来。
fn is_retryable(err: &HttpError) -> bool {
    match err {
        HttpError::Transport(_) => true,
        HttpError::Status { status, body } => {
            *status >= 500 || !INVALID_GRANT.iter().any(|code| body.contains(code))
        }
        HttpError::Protocol(_) => false,
    }
}

/// 取响应里的新 `refresh_token`，为空则回退旧值。
fn keep_refresh_token(token: &TokenResponse, previous: &str) -> String {
    if token.refresh_token.is_empty() {
        previous.to_string()
    } else {
        token.refresh_token.clone()
    }
}

/// `KeyPair` 未实现 Clone 手动复制（PEM + JWK 都是小字符串）。
fn clone_keys(keys: &dpop::KeyPair) -> dpop::KeyPair {
    dpop::KeyPair {
        private_key_pem: keys.private_key_pem.clone(),
        public_jwk: keys.public_jwk.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred() -> Credential {
        Credential {
            access_key_id: "AK".into(),
            secret_access_key: "SK".into(),
            security_token: "ST".into(),
            refresh_token: "RT".into(),
            expire_at_millis: Some(0),
            code_verifier: "V".repeat(128),
            dpop_key_pair: dpop::KeyPair::generate().unwrap(),
            last_auth_at: None,
        }
    }

    #[test]
    fn ide_form_carries_code_verifier() {
        let c = cred();
        let f = ide_form(&c);
        assert_eq!(f[0], ("client_id", "codearts-snap"));
        assert_eq!(f[1].0, "code_verifier");
        assert_eq!(f[1].1.len(), 128);
        assert_eq!(f[2], ("grant_type", "refresh_token"));
        assert_eq!(f[3], ("refresh_token", "RT"));
    }

    #[test]
    fn kernel_form_omits_code_verifier() {
        let c = cred();
        let f = kernel_form(&c);
        assert_eq!(f[0], ("client_id", "CodeArts_Tui"));
        assert!(f.iter().all(|(k, _)| *k != "code_verifier"));
        assert_eq!(f[2], ("refresh_token", "RT"));
    }

    #[test]
    fn invalid_grant_is_not_retried() {
        let err = HttpError::Status {
            status: 400,
            body: r#"{"error":"invalid_grant","error_description":"refresh token expired"}"#.into(),
        };
        assert!(!is_retryable(&err));
    }

    /// 华为 STS 的失效形态不带 OAuth 标准码，必须同样判为不可重试，
    /// 否则会白白回退 kernel 形态并报出误导性的 `invalid client id`。
    #[test]
    fn huawei_sts_invalid_refresh_token_is_not_retried() {
        let used = HttpError::Status {
            status: 400,
            body: r#"{"error_code":"STS5.1806","error_msg":"invalid refresh token: 'the refresh token has been used'"}"#.into(),
        };
        assert!(!is_retryable(&used), "已用过的令牌不应重试");
        let mismatch = HttpError::Status {
            status: 400,
            body: r#"{"error_code":"STS5.1806","error_msg":"invalid refresh token: 'invalid client id: codearts-snap'"}"#.into(),
        };
        assert!(!is_retryable(&mismatch));
    }

    #[test]
    fn server_errors_and_transport_are_retried() {
        assert!(is_retryable(&HttpError::Status {
            status: 502,
            body: String::new()
        }));
        assert!(is_retryable(&HttpError::Status {
            status: 400,
            body: r#"{"error":"invalid_request"}"#.into()
        }));
    }

    #[test]
    fn empty_refresh_token_keeps_previous() {
        let mut token = TokenResponse {
            credentials: auth::Credentials {
                access_key_id: Some("a".into()),
                secret_access_key: Some("b".into()),
                security_token: Some("c".into()),
                expires_at: None,
            },
            refresh_token: String::new(),
            expiration: None,
        };
        assert_eq!(keep_refresh_token(&token, "OLD"), "OLD");
        token.refresh_token = "NEW".into();
        assert_eq!(keep_refresh_token(&token, "OLD"), "NEW");
    }
}
