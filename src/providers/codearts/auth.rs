//! CodeArts 登录：PKCE(SHA-256) + DPoP(ES256) + 浏览器授权 + 本地回调。
//!
//! 流程对齐官方扩展（`provider-info/codearts.md` §1）：
//! 1. 生成 PKCE pair（verifier 为 64 位 hex）与 DPoP P-256 密钥对；
//! 2. 本地 `127.0.0.1:{port}/oauth/callback` 起回调服务（`port=0` 随机）；
//! 3. 打开门户授权页（参数为官方扩展拼接的并集）；
//! 4. 浏览器登录成功后回连本地：`?code=…&secret=…&redirect=…`；
//!    本地按官方行为再 307 跳门户 `login_succeed` 页；
//! 5. POST `{iam}/v1/oauth2/tokens`（urlencoded + DPoP 头）换 AKSK。
//!
//! 官方回调还会带 `secret` 并有 90s×3s 的 `queryAuth` 轮询兜底；本实现与
//! kernel 一致，直接用 `code` 换 token（两者等价，secret 路径是 Web 端用）。

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;

use super::api::{self, HttpError};
use super::credential::Credential;
use super::dpop::KeyPair;
use super::pkce;
use crate::cli::login;

/// 回调路径（官方 `CALLBACK_PATH`）。
pub const CALLBACK_PATH: &str = "/oauth/callback";
/// 等待回调超时（官方 kernel 为 5min）。
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// token 端点响应。兼容官方两种形态（实测为扩展版形态，见 result.log）：
/// * 扩展版（`bZe`/`Qri`）：`credentials.{access_key_id, secret_access_key,
///   security_token, expiration}` + 顶层 `refresh_token`
/// * kernel 版（`jri`）：`credential.{access, secret, securitytoken, expires_at}`
///   + 顶层 `refresh_token` / `expiration`
#[derive(Debug, Clone, Deserialize)]
pub struct TokenResponse {
    #[serde(alias = "credential")]
    pub credentials: Credentials,
    #[serde(default)]
    pub refresh_token: String,
    /// kernel 版顶层 `expiration`（仅存档）。
    #[serde(default)]
    #[allow(dead_code)]
    pub expiration: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Credentials {
    #[serde(default, alias = "access")]
    pub access_key_id: Option<String>,
    #[serde(default, alias = "secret")]
    pub secret_access_key: Option<String>,
    #[serde(default, alias = "securitytoken")]
    pub security_token: Option<String>,
    /// 扩展版叫 `expiration`，kernel 版叫 `expires_at`。
    #[serde(default, alias = "expiration")]
    pub expires_at: Option<String>,
}

impl TokenResponse {
    /// 归一化为 `(AK, SK, security_token, refresh_token, expire_at_millis)`。
    /// 必需字段缺失时返回 [`HttpError::Protocol`]。
    pub fn into_parts(self) -> Result<(String, String, String, String, Option<i64>), HttpError> {
        let c = self.credentials;
        let missing = |name: &str| {
            HttpError::Protocol(format!("token 响应缺少 {name}（原始字段形态与预期不符）"))
        };
        let ak = c
            .access_key_id
            .filter(|s| !s.is_empty())
            .ok_or_else(|| missing("access_key_id"))?;
        let sk = c
            .secret_access_key
            .filter(|s| !s.is_empty())
            .ok_or_else(|| missing("secret_access_key"))?;
        let st = c
            .security_token
            .filter(|s| !s.is_empty())
            .ok_or_else(|| missing("security_token"))?;
        let exp = c.expires_at.as_deref().and_then(iso8601_to_millis);
        Ok((ak, sk, st, self.refresh_token, exp))
    }
}

/// 登录入口。
pub async fn run(root: PathBuf) -> anyhow::Result<()> {
    let verifier = pkce::new_verifier();
    let challenge = pkce::challenge(&verifier);
    let dpop_keys = KeyPair::generate()?;

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let callback_url = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");

    let url = authorize_url(port, &callback_url, &challenge);
    println!("[codearts] 已生成授权链接，正在打开浏览器：\n{url}\n");
    if let Err(e) = login::open_browser(&url) {
        eprintln!("[codearts] 打开浏览器失败（{e}），请手动访问上面的链接");
    }
    println!("[codearts] 等待浏览器回调 {callback_url}");

    let code = wait_callback(listener).await?;
    println!("[codearts] 已获得授权码，正在换取令牌…");

    let client = api::token_client()?;
    let token = exchange_code(&client, &dpop_keys, &code, &callback_url, &verifier).await?;
    let cred = Credential::from_token(token, verifier, dpop_keys)
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    cred.store(&root)?;

    println!(
        "[codearts] 登录成功，凭证已保存到 {}",
        crate::config::Config::credential_path(&root, super::ID).display()
    );
    match cred.expire_at_millis {
        Some(ms) => println!("[codearts] 凭证有效期至 {}", crate::core::fmt_ms(ms)),
        None => println!("[codearts] 上游未返回有效期"),
    }
    Ok(())
}

/// 拼门户授权 URL（参数与官方扩展 `openAuthorizeUrl` 一致；`port` 为本地回调端口）。
pub fn authorize_url(port: u16, callback_url: &str, challenge: &str) -> String {
    let params: Vec<(&str, String)> = vec![
        ("theme", String::new()),
        ("locale", "zh-cn".into()),
        // 官方 `uri_scheme`/`client_id` 都是 `QH()` = 扩展 package.json name。
        ("uri_scheme", api::CLIENT_ID.into()),
        ("client_id", api::CLIENT_ID.into()),
        ("port", port.to_string()),
        ("code_challenge", challenge.into()),
        ("code_challenge_method", pkce::CODE_CHALLENGE_METHOD.into()),
        ("ticket_id", pkce::uuid_v4()),
        ("auth_callback_url", callback_url.into()),
        ("plugin-name", api::PLUGIN_NAME.into()),
        ("plugin-version", api::PLUGIN_VERSION.into()),
    ];
    let qs: Vec<String> = params
        .into_iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(&v)))
        .collect();
    format!("{}/portal/authorize?{}", api::PORTAL_AUTHORIZE, qs.join("&"))
}

/// 用授权码换 token（`POST /v1/oauth2/tokens`，urlencoded + DPoP）。
pub async fn exchange_code(
    client: &reqwest::Client,
    keys: &KeyPair,
    code: &str,
    callback_url: &str,
    verifier: &str,
) -> Result<TokenResponse, HttpError> {
    let url = format!("{}{}", api::IAM_STS_OPEN, api::TOKEN_PATH);
    let form = [
        ("client_id", api::CLIENT_ID),
        ("code", code),
        ("code_verifier", verifier),
        ("grant_type", "authorization_code"),
        ("redirect_uri", callback_url),
    ];
    request_token(client, keys, &url, &form).await
}

/// 等待浏览器回调并取授权码。成功后按官方行为 307 跳门户成功页。
async fn wait_callback(listener: tokio::net::TcpListener) -> anyhow::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let deadline = tokio::time::Instant::now() + CALLBACK_TIMEOUT;

    loop {
        let (mut sock, _) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| anyhow::anyhow!("等待回调超时（{}s）", CALLBACK_TIMEOUT.as_secs()))??;

        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let first_line = request.lines().next().unwrap_or_default();
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let target = parts.next().unwrap_or_default();

        let (route, query) = match target.split_once('?') {
            Some((r, q)) => (r, q),
            None => (target, ""),
        };
        if method != "GET" || route != CALLBACK_PATH {
            respond_404(&mut sock).await;
            continue;
        }

        match parse_callback(query) {
            Ok(code) => {
                // 官方：有 code/secret 时 307 到门户 login_succeed 页。
                let next = format!(
                    "{}/portal/login?login_succeed=true&locale=zh-cn&uri_scheme={}",
                    api::PORTAL_AUTHORIZE,
                    urlencoding::encode(api::CLIENT_ID)
                );
                let resp = format!(
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: {next}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
                return Ok(code);
            }
            Err(why) => {
                tracing::debug!(why, query, "回调校验失败");
                let body = "OK";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
                continue;
            }
        }
    }
}

/// 解析回调查询串。官方只要求取 `code`；带 `secret` 时为 Web 轮询路径，此处忽略。
fn parse_callback(query: &str) -> Result<String, String> {
    let code = login::query_param(query, "code").ok_or("缺少 code")?;
    if code.trim().is_empty() || code.len() > 4096 || code.chars().any(|c| c.is_control()) {
        return Err("code 非法".into());
    }
    Ok(code)
}

async fn respond_404(sock: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt;
    let _ = sock
        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 8\r\nConnection: close\r\n\r\nNot found")
        .await;
    let _ = sock.shutdown().await;
}

/// 发 token 请求（登录与续期共用）：DPoP proof + urlencoded body。
pub(crate) async fn request_token(
    client: &reqwest::Client,
    keys: &KeyPair,
    url: &str,
    form: &[(&str, &str)],
) -> Result<TokenResponse, HttpError> {
    let proof = keys
        .proof("POST", url)
        .map_err(|e| HttpError::Protocol(e.to_string()))?;
    let resp = client
        .post(url)
        .header("DPoP", proof)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(serde_urlencoded(form))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(HttpError::Status { status, body });
    }
    serde_json::from_str(&body).map_err(|e| {
        HttpError::Protocol(format!(
            "解析 token 响应失败: {e}；响应片段: {}",
            truncate(&body, 400)
        ))
    })
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// `application/x-www-form-urlencoded` 编码。
fn serde_urlencoded(form: &[(&str, &str)]) -> String {
    form.iter()
        .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// ISO8601 时间串 → 毫秒时间戳（只处理常见的 `YYYY-MM-DDTHH:MM:SSZ` 形态）。
pub fn iso8601_to_millis(s: &str) -> Option<i64> {
    let dt = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()?;
    Some(dt.unix_timestamp() * 1000 + i64::from(dt.millisecond()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_parses() {
        let ms = iso8601_to_millis("2026-02-13T07:38:53Z").unwrap();
        assert_eq!(ms % 1000, 0);
        assert!(ms > 1_700_000_000_000);
    }

    #[test]
    fn authorize_url_has_union_params() {
        let u = authorize_url(34567, "http://127.0.0.1:34567/oauth/callback", "CHALLENGE");
        assert!(u.starts_with("https://codearts.huaweicloud.com/portal/authorize?"));
        assert!(u.contains("locale=zh-cn"));
        assert!(u.contains("uri_scheme=codearts-snap"));
        assert!(u.contains("client_id=codearts-snap"));
        assert!(u.contains("port=34567"));
        assert!(u.contains("code_challenge=CHALLENGE"));
        assert!(u.contains("code_challenge_method=SHA-256"));
        assert!(u.contains("auth_callback_url=http%3A%2F%2F127.0.0.1%3A34567%2Foauth%2Fcallback"));
        assert!(u.contains("plugin-name=snap_vscode"));
        assert!(u.contains("plugin-version=26.11.100"));
        assert!(u.contains("ticket_id="));
    }

    #[test]
    fn parse_callback_requires_code() {
        assert!(parse_callback("code=abc").unwrap() == "abc");
        assert!(parse_callback("code=abc&secret=xyz").is_ok());
        assert!(parse_callback("secret=xyz").is_err());
        assert!(parse_callback("code=").is_err());
    }

    /// 扩展版响应形态（bZe/Qri 读取的字段，实测 result.log 报错证实
    /// `expires_at` 不存在，即扩展形态）。
    #[test]
    fn parses_vscode_shape() {
        let body = r#"{
            "credentials": {
                "access_key_id": "AK",
                "secret_access_key": "SK",
                "security_token": "ST",
                "expiration": "2026-02-13T07:38:53Z"
            },
            "refresh_token": "REF"
        }"#;
        let t: TokenResponse = serde_json::from_str(body).unwrap();
        let (ak, sk, st, refresh, exp) = t.into_parts().unwrap();
        assert_eq!(ak, "AK");
        assert_eq!(sk, "SK");
        assert_eq!(st, "ST");
        assert_eq!(refresh, "REF");
        assert!(exp.is_some());
    }

    /// kernel 版响应形态（jri：credential.{access,secret,securitytoken,expires_at}）。
    #[test]
    fn parses_kernel_shape() {
        let body = r#"{
            "credential": {
                "access": "AK",
                "secret": "SK",
                "securitytoken": "ST",
                "expires_at": "2026-02-13T07:38:53Z"
            },
            "refresh_token": "REF",
            "expiration": "3600"
        }"#;
        let t: TokenResponse = serde_json::from_str(body).unwrap();
        let (ak, sk, st, refresh, exp) = t.into_parts().unwrap();
        assert_eq!((ak.as_str(), sk.as_str(), st.as_str(), refresh.as_str()),
                   ("AK", "SK", "ST", "REF"));
        assert!(exp.is_some());
    }

    #[test]
    fn rejects_missing_fields() {
        let body = r#"{ "credentials": { "access_key_id": "AK" } }"#;
        let t: TokenResponse = serde_json::from_str(body).unwrap();
        assert!(t.into_parts().is_err());
    }
}
