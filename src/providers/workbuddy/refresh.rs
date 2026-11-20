//! WorkBuddy 令牌续期：`refresh_token` 换新令牌。
//!
//! 协议见 `provider-info/workbuddy.md` §2：
//!
//! ```text
//! POST {HOST}/v2/plugin/auth/token/refresh
//! X-Refresh-Token: <refresh_token>
//! X-Auth-Refresh-Source: plugin
//! ```
//!
//! 官方在续期后会重新拉一次账号（`fetchCurrentAccount`）并合并会话；这里
//! 同样在续期后回读账号，好让 `uid` / `enterpriseId` 保持最新。

use super::api::{self, HttpError};
use super::credential::Credential;

/// 续期并返回新凭证（调用方负责落盘）。
pub async fn refresh(client: &reqwest::Client, cred: &Credential) -> Result<Credential, HttpError> {
    if cred.refresh_token.trim().is_empty() {
        return Err(HttpError::Business {
            code: None,
            message: "缺少 refresh_token，无法续期（请重新登录）".to_string(),
        });
    }
    if cred.refresh_expired() {
        return Err(HttpError::Business {
            code: None,
            message: "刷新令牌已过期，请重新登录".to_string(),
        });
    }

    let url = format!("{}{}", api::HOST, api::AUTH_REFRESH_PATH);
    let mut headers = api::auth_headers(&cred.access_token, &cred.domain);
    headers.push(("X-Refresh-Token", cred.refresh_token.clone()));
    headers.push(("X-Auth-Refresh-Source", "plugin".to_string()));
    if cred.is_enterprise() {
        headers.push(("X-Enterprise-Id", cred.enterprise_id.clone()));
        headers.push(("X-Tenant-Id", cred.enterprise_id.clone()));
    }

    let token: super::auth::AuthToken =
        api::post_envelope(client, &url, &headers, &serde_json::json!({})).await?;
    token.validate().map_err(|e| HttpError::Business {
        code: None,
        message: e,
    })?;

    // 续期后回读账号（官方同款做法），失败不致命：保留旧账号信息。
    let mut account = None;
    match super::auth::fetch_account(client, &token.access_token, &cred.domain).await {
        Ok(a) => account = Some(a),
        Err(e) => tracing::debug!("[workbuddy] 续期后回读账号失败（沿用旧信息）: {e}"),
    }

    let enterprise_id = account
        .as_ref()
        .map(|a| a.enterprise_id.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| token.enterprise_id.clone().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| cred.enterprise_id.clone());

    let domain = account
        .as_ref()
        .map(|a| a.domain.clone())
        .filter(|s| !s.trim().is_empty())
        .or_else(|| token.domain.clone().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| cred.domain.clone());

    let nickname = account
        .as_ref()
        .map(|a| {
            if a.nickname.trim().is_empty() {
                a.display_name.clone()
            } else {
                a.nickname.clone()
            }
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| cred.nickname.clone());

    let uid = account
        .as_ref()
        .map(|a| a.uid.clone())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| cred.uid.clone());

    Ok(Credential {
        access_token: token.access_token.clone(),
        // 上游可能不轮换 refresh_token；缺失时沿用旧的。
        refresh_token: if token.refresh_token.trim().is_empty() {
            cred.refresh_token.clone()
        } else {
            token.refresh_token.clone()
        },
        expires_at: token.expires_at(),
        refresh_expires_at: {
            let v = token.refresh_expires_at();
            if v > 0 { v } else { cred.refresh_expires_at }
        },
        uid,
        enterprise_id,
        domain,
        nickname,
    })
}
