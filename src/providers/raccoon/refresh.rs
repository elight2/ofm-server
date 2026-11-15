//! Raccoon 令牌续期：`POST /api/web/auth/v1/refresh`。
//!
//! 见 `provider-info/raccoon.md` §2。`refresh_token` 可能轮换（响应带回新的
//! 才替换），响应里没有则沿用旧值。IAM token 无 `refresh_token`，只能重新登录。

use serde::Deserialize;

use super::api::{self, HttpError};
use super::credential::{self, Credential};

#[derive(Debug, Deserialize)]
pub struct RefreshData {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
}

/// 续期并返回新凭证（不写盘，由调用方落盘）。
pub async fn refresh(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<Credential, HttpError> {
    if cred.refresh_token.trim().is_empty() {
        return Err(HttpError::Business {
            code: None,
            message: "缺少 refresh_token（IAM 登录需重新登录）".to_string(),
        });
    }
    let url = format!("{}{}/refresh", api::HOST, api::AUTH_PREFIX);
    let body = serde_json::json!({ "refresh_token": cred.refresh_token });
    let data: RefreshData = api::post_envelope(client, &url, None, &body).await?;

    // refresh_token 轮换：响应带了新的才替换，避免把旧值冲成空。
    let refresh_token = data
        .refresh_token
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| cred.refresh_token.clone());

    Ok(Credential {
        access_token: data.access_token,
        refresh_token,
        office_identity: cred.office_identity.clone(),
        office_org_name: cred.office_org_name.clone(),
        office_org_role: cred.office_org_role.clone(),
        last_auth_at: Some(credential::now_millis()),
    })
}
