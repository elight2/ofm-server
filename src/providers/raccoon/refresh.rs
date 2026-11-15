//! Raccoon 令牌续期：`POST /api/web/auth/v1/refresh`。
//!
//! 见 `provider-info/raccoon.md` §2。实测（2025-11）该端点对 IAM token 同样有效：
//! 回 `code:0` 且带 `access_token`/`refresh_token`。注意本客户端签发的
//! `access_token` 与 `refresh_token` **字符串完全相同**（同一个 `jti`），
//! 且未临近过期时续期会**原样返还**（`exp` 不变）。
//!
//! `refresh_token` 可能轮换：响应带了新的才替换，否则沿用旧值。

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
            message: "缺少 refresh_token，需重新登录".to_string(),
        });
    }
    let url = format!("{}{}/refresh", api::HOST, api::AUTH_PREFIX);
    let body = serde_json::json!({ "refresh_token": cred.refresh_token });
    let data: RefreshData = api::post_envelope(client, &url, None, &body).await?;

    // 实测：未临近过期时服务端原样返还同一 token。记录下来，
    // 否则过期前反复刷新会毫无效果却看不出异常。
    if data.access_token == cred.access_token {
        tracing::debug!("[raccoon] 续期返回与原 token 相同（服务端未重新签发）");
    }

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

#[cfg(test)]
mod live {
    /// 真实续期（需 `testdata/providers/raccoon/credential.toml`）。
    /// `cargo test raccoon_refresh_live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn raccoon_refresh_live() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        let cred = super::Credential::load(&root).unwrap().expect("未登录");
        let client = super::api::client().unwrap();
        let new = super::refresh(&client, &cred).await.expect("续期失败");
        assert!(!new.access_token.is_empty());
        println!(
            "续期成功：token 是否变化 = {}，refresh_token 是否变化 = {}",
            new.access_token != cred.access_token,
            new.refresh_token != cred.refresh_token
        );
    }
}
