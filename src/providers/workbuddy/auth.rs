//! WorkBuddy 登录：浏览器授权 + 轮询换令牌。
//!
//! 协议见 `provider-info/workbuddy.md` §1。登录类型是
//! `account-scoped-external-link`：**没有本地回调**，授权发生在上游网页，
//! 客户端只能拿着 `state` 轮询令牌。
//!
//! 流程：
//! 1. `POST /v2/plugin/auth/state?platform=workbuddy` → `state` + `authUrl`；
//! 2. 打开 `authUrl`（浏览器），用户在网页完成登录；
//! 3. 每秒 `GET /v2/plugin/auth/token?state=…`，直到拿到 `accessToken`
//!    （未就绪时上游回业务码 `11217`）；
//! 4. `GET /v2/plugin/account` 取账号（`uid` / `enterpriseId` / `domain` …）；
//! 5. 写入 `providers/workbuddy/credential.toml`。

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::api::{self, HttpError};
use super::credential::{self, Credential};

/// `auth/state` 响应 `data`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthState {
    /// 轮询用的会话标识。
    pub state: String,
    /// 授权页地址（用户在这里完成登录）。
    pub auth_url: String,
}

/// `auth/token` 响应 `data`。
///
/// 上游字段是 camelCase；个别部署回 snake_case，故两者都认。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthToken {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub token_type: Option<String>,
    /// 访问令牌有效期（秒）。
    #[serde(default)]
    pub expires_in: Option<i64>,
    /// 刷新令牌有效期（秒）。
    #[serde(default)]
    pub refresh_expires_in: Option<i64>,
    /// 租户域名（`X-Domain`）。
    #[serde(default)]
    pub domain: Option<String>,
    /// 企业 id（个人账号为空）。
    #[serde(default, alias = "enterprise_id")]
    pub enterprise_id: Option<String>,
}

/// `auth/token` 的原始信封（轮询时要读业务码）。
#[derive(Debug, Deserialize)]
struct TokenEnvelope {
    #[serde(default)]
    code: Option<i64>,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<AuthToken>,
}

/// `account` 响应 `data`（只取用得到的字段）。
///
/// 上游字段是 camelCase；`/account` 与 `/login/account` 两处形态略有差异，
/// 故对关键字段同时接受 snake_case 别名。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default, alias = "enterprise_id")]
    pub enterprise_id: String,
    #[serde(default)]
    pub domain: String,
    /// 版本类型（`personal` / `enterprise` 等），后续请求头用。
    #[serde(default)]
    #[allow(dead_code)]
    pub edition_type: String,
    /// 账号类型（`personal` / `ultimate` 等），后续余额/套餐用。
    #[serde(default)]
    #[allow(dead_code)]
    pub account_type: String,
}

impl AuthToken {
    /// 访问令牌到期时间（毫秒）。
    ///
    /// 官方 `calculateExpiresAt`：`expiresAt = now + expiresIn * 1000`。
    pub fn expires_at(&self) -> i64 {
        credential::now_millis() + self.expires_in.unwrap_or(0) * 1000
    }

    /// 刷新令牌到期时间（毫秒）。
    pub fn refresh_expires_at(&self) -> i64 {
        credential::now_millis() + self.refresh_expires_in.unwrap_or(0) * 1000
    }

    /// 令牌类型校验。官方只认 `Bearer`。
    pub fn validate(&self) -> Result<(), String> {
        if self.access_token.trim().is_empty() {
            return Err("响应缺少 accessToken".into());
        }
        match self.token_type.as_deref() {
            Some(t) if t.eq_ignore_ascii_case("Bearer") => Ok(()),
            Some(other) => Err(format!("tokenType 非 Bearer: {other}")),
            None => Ok(()),
        }
    }
}

/// 登录入口。
pub async fn run(root: PathBuf) -> anyhow::Result<()> {
    let client = api::client()?;

    // ① 拿 state 与授权页地址。
    let state = fetch_auth_state(&client).await?;
    println!(
        "[workbuddy] 已生成授权链接，正在打开浏览器：\n{}\n",
        state.auth_url
    );
    if let Err(e) = crate::login_util::open_browser(&state.auth_url) {
        eprintln!("[workbuddy] 打开浏览器失败（{e}），请手动访问上面的链接");
    }
    println!(
        "[workbuddy] 请在浏览器中完成登录，最多等待 {} 秒…",
        api::POLL_TIMEOUT_SECS
    );

    // ②③ 轮询令牌。
    let token = loop_get_token(&client, &state.state).await?;
    token
        .validate()
        .map_err(|e| anyhow::anyhow!("登录失败: {e}"))?;

    // ④ 取账号信息。
    let account = fetch_account(
        &client,
        &token.access_token,
        token.domain.as_deref().unwrap_or(""),
    )
    .await?;

    // ⑤ 落盘。
    let cred = Credential {
        access_token: token.access_token.clone(),
        refresh_token: token.refresh_token.clone(),
        expires_at: token.expires_at(),
        refresh_expires_at: token.refresh_expires_at(),
        uid: account.uid.clone(),
        enterprise_id: Some(account.enterprise_id.clone())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| token.enterprise_id.clone().filter(|s| !s.trim().is_empty()))
            .unwrap_or_default(),
        domain: Some(account.domain.clone())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| token.domain.clone().filter(|s| !s.trim().is_empty()))
            .unwrap_or_default(),
        nickname: if !account.nickname.trim().is_empty() {
            account.nickname.clone()
        } else {
            account.display_name.clone()
        },
    };
    cred.store(&root)?;

    println!(
        "[workbuddy] 登录成功，凭证已保存到 {}",
        crate::core::paths::credential_path(&root, credential::PROVIDER).display()
    );
    if cred.expires_at > 0 {
        println!(
            "[workbuddy] 访问令牌有效期至 {}",
            crate::core::fmt_secs(cred.expires_at / 1000)
        );
    }
    if cred.refresh_expires_at > 0 {
        println!(
            "[workbuddy] 刷新令牌有效期至 {}",
            crate::core::fmt_secs(cred.refresh_expires_at / 1000)
        );
    }
    println!(
        "[workbuddy] 身份：{}{}",
        if cred.nickname.is_empty() {
            "未知名"
        } else {
            &cred.nickname
        },
        if cred.is_enterprise() {
            "（企业账号，请求将带 X-Enterprise-Id）"
        } else {
            "（个人账号）"
        }
    );
    Ok(())
}

/// 取 `state` 与授权页地址。
pub async fn fetch_auth_state(client: &reqwest::Client) -> Result<AuthState, HttpError> {
    let url = format!(
        "{}{}{}?platform={}",
        api::HOST,
        api::AUTH_STATE_PATH,
        "",
        api::PLATFORM
    );
    let headers = api::anonymous_headers();
    api::post_envelope::<_, AuthState>(client, &url, &headers, &serde_json::json!({})).await
}

/// 轮询令牌，直到拿到 `accessToken`。
///
/// 上游在用户尚未完成登录时回业务码 `11217`（`LOGIN_TOKEN_PENDING_CODE`），
/// 这不是错误，继续等。其它业务码一律当作失败。
pub async fn loop_get_token(client: &reqwest::Client, state: &str) -> Result<AuthToken, HttpError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(api::POLL_TIMEOUT_SECS);
    loop {
        tokio::time::sleep(Duration::from_secs(api::POLL_INTERVAL_SECS)).await;

        let url = format!(
            "{}{}{}?state={}",
            api::HOST,
            api::AUTH_TOKEN_PATH,
            "",
            state
        );
        let headers = api::anonymous_headers();

        match poll_once(client, &url, &headers).await {
            Ok(Some(token)) => return Ok(token),
            Ok(None) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(HttpError::Business {
                        code: Some(api::CODE_TOKEN_PENDING),
                        message: "等待登录超时（用户未在网页完成登录）".to_string(),
                    });
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// 一次轮询。`Ok(None)` 表示「还没就绪，继续等」。
async fn poll_once(
    client: &reqwest::Client,
    url: &str,
    headers: &[(&str, String)],
) -> Result<Option<AuthToken>, HttpError> {
    let mut http = client.get(url);
    for (k, v) in headers {
        http = http.header(*k, v);
    }
    let resp = http.send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();

    // 信封形态（业务码 + data）。
    if let Ok(env) = serde_json::from_str::<TokenEnvelope>(&text) {
        if env.code == Some(api::CODE_TOKEN_PENDING) {
            return Ok(None);
        }
        if env.code.unwrap_or(0) != 0 {
            return Err(HttpError::Business {
                code: env.code,
                message: env.msg.unwrap_or_else(|| "获取登录令牌失败".to_string()),
            });
        }
        if let Some(t) = env.data {
            return Ok(Some(t));
        }
    }

    if !(200..300).contains(&status.as_u16()) {
        return Err(HttpError::Status {
            status: status.as_u16(),
            body: crate::core::text::truncate_chars(&text, 300),
        });
    }
    Err(HttpError::Decode(format!(
        "未识别响应: {}",
        crate::core::text::truncate_chars(&text, 200)
    )))
}

/// 取账号信息。
pub async fn fetch_account(
    client: &reqwest::Client,
    access_token: &str,
    domain: &str,
) -> Result<Account, HttpError> {
    let url = format!("{}{}", api::HOST, api::ACCOUNT_PATH);
    let headers = api::auth_headers(access_token, domain);
    api::get_envelope::<Account>(client, &url, &headers).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 令牌到期时间按秒换算() {
        let t = AuthToken {
            access_token: "a".into(),
            refresh_token: "r".into(),
            token_type: Some("Bearer".into()),
            expires_in: Some(3600),
            refresh_expires_in: Some(7200),
            domain: None,
            enterprise_id: None,
        };
        let now = credential::now_millis();
        assert!((t.expires_at() - now - 3_600_000).abs() < 2000);
        assert!((t.refresh_expires_at() - now - 7_200_000).abs() < 2000);
    }

    #[test]
    fn 缺到期字段时为零() {
        let t = AuthToken {
            access_token: "a".into(),
            refresh_token: String::new(),
            token_type: None,
            expires_in: None,
            refresh_expires_in: None,
            domain: None,
            enterprise_id: None,
        };
        assert_eq!(t.expires_at(), credential::now_millis());
        assert!(t.validate().is_ok());
    }

    #[test]
    fn 空令牌不合法() {
        let t = AuthToken {
            access_token: "  ".into(),
            refresh_token: String::new(),
            token_type: Some("Bearer".into()),
            expires_in: None,
            refresh_expires_in: None,
            domain: None,
            enterprise_id: None,
        };
        assert!(t.validate().is_err());
    }

    #[test]
    fn 非bearer令牌不合法() {
        let t = AuthToken {
            access_token: "a".into(),
            refresh_token: String::new(),
            token_type: Some("mac".into()),
            expires_in: None,
            refresh_expires_in: None,
            domain: None,
            enterprise_id: None,
        };
        assert!(t.validate().is_err());
    }

    #[test]
    fn 待处理信封能识别业务码() {
        let env: TokenEnvelope =
            serde_json::from_str(r#"{"code":11217,"msg":"login ing...","requestId":"x"}"#).unwrap();
        assert_eq!(env.code, Some(api::CODE_TOKEN_PENDING));
        assert!(env.data.is_none());
    }

    #[test]
    fn 令牌信封能解析camel_case() {
        let env: TokenEnvelope = serde_json::from_str(
            r#"{"code":0,"data":{"accessToken":"at","refreshToken":"rt","tokenType":"Bearer","expiresIn":3600,"refreshExpiresIn":7200}}"#,
        )
        .unwrap();
        assert_eq!(env.code, Some(0));
        let t = env.data.unwrap();
        assert_eq!(t.access_token, "at");
        assert_eq!(t.refresh_token, "rt");
        assert_eq!(t.expires_in, Some(3600));
        assert_eq!(t.refresh_expires_in, Some(7200));
    }

    #[test]
    fn 账号字段可解析() {
        let a: Account = serde_json::from_str(
            r#"{"uid":"u1","nickname":"n","enterpriseId":"e1","editionType":"pro"}"#,
        )
        .unwrap();
        assert_eq!(a.uid, "u1");
        assert_eq!(a.nickname, "n");
        assert_eq!(a.enterprise_id, "e1");
        assert_eq!(a.edition_type, "pro");
    }
}
