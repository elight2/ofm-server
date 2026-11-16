//! CodeArts 登录：PKCE(SHA-256) + DPoP(ES256) + 浏览器授权 + 本地回调。
//!
//! 流程对齐官方扩展（`provider-info/codearts.md` §1）：
//! 1. 生成 PKCE pair（verifier 为 128 位 hex）与 DPoP P-256 密钥对；
//! 2. 本地 `127.0.0.1:{port}/oauth/callback` 起回调服务（`port=0` 随机）；
//! 3. 打开门户授权页（参数为官方扩展拼接的并集）；
//! 4. 浏览器登录成功后回连本地：`?code=…`，或旧版 `?secret=…&redirect=…`；
//!    本地按官方行为再 307 跳门户 `login_succeed` 页（secret 带 redirect 时跳该地址）；
//! 5. code 路径 POST `{iam}/v1/oauth2/tokens`（urlencoded + DPoP 头）换 AKSK；
//!    secret 路径 GET `{snap}/snap-manager/v1/login/ticket` 换票。
//!
//! 官方扩展对 secret 路径会做 90×3s 的 `queryAuth` 轮询；本实现取 kernel 的
//! 10×1s 重试，避免登录命令长时间无反馈。

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
/// 旧版 secret 换票端点（`snap-manager`）。
const TICKET_PATH: &str = "/snap-manager/v1/login/ticket";
/// secret 换票最大尝试次数（官方 kernel 为 10 次）。
const TICKET_ATTEMPTS: u32 = 10;
/// secret 换票重试间隔（官方 kernel 为 1s）。
const TICKET_RETRY_DELAY: Duration = Duration::from_secs(1);

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

    let (url, ticket_id) = authorize_url(port, &callback_url, &challenge);
    println!("[codearts] 已生成授权链接，正在打开浏览器：\n{url}\n");
    if let Err(e) = login::open_browser(&url) {
        eprintln!("[codearts] 打开浏览器失败（{e}），请手动访问上面的链接");
    }
    println!("[codearts] 等待浏览器回调 {callback_url}");

    let (callback, mut sock) = wait_callback(listener).await?;
    let client = api::token_client()?;

    let (cred, redirect) = match callback {
        Callback::Code { code } => {
            println!("[codearts] 已获得授权码，正在换取令牌…");
            let token =
                match exchange_code(&client, &dpop_keys, &code, &callback_url, &verifier).await {
                    Ok(t) => t,
                    Err(e) => {
                        respond_redirect(&mut sock, false, None).await;
                        return Err(anyhow::anyhow!("换取令牌失败: {e}"));
                    }
                };
            let cred = match Credential::from_token(token, verifier, dpop_keys) {
                Ok(c) => c,
                Err(e) => {
                    respond_redirect(&mut sock, false, None).await;
                    return Err(anyhow::anyhow!(e.to_string()));
                }
            };
            (cred, None)
        }
        Callback::Secret { secret, redirect } => {
            println!("[codearts] 已获得旧版 secret，正在换票…");
            match exchange_ticket(&client, &ticket_id, &secret, verifier, dpop_keys).await {
                Ok(c) => (c, redirect),
                Err(e) => {
                    respond_redirect(&mut sock, false, redirect.as_deref()).await;
                    return Err(anyhow::anyhow!("secret 换票失败: {e}"));
                }
            }
        }
    };

    if let Err(e) = cred.store(&root) {
        respond_redirect(&mut sock, false, redirect.as_deref()).await;
        return Err(e.into());
    }
    respond_redirect(&mut sock, true, redirect.as_deref()).await;

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
///
/// 返回 `(授权 URL, ticket_id)`；`ticket_id` 供回调走旧版 `secret` 路径时换票。
pub fn authorize_url(port: u16, callback_url: &str, challenge: &str) -> (String, String) {
    let ticket_id = pkce::uuid_v4();
    let params: Vec<(&str, String)> = vec![
        ("theme", String::new()),
        ("locale", "zh-cn".into()),
        // 官方 `uri_scheme`/`client_id` 都是 `QH()` = 扩展 package.json name。
        ("uri_scheme", api::CLIENT_ID.into()),
        ("client_id", api::CLIENT_ID.into()),
        ("port", port.to_string()),
        ("code_challenge", challenge.into()),
        ("code_challenge_method", pkce::CODE_CHALLENGE_METHOD.into()),
        ("ticket_id", ticket_id.clone()),
        ("auth_callback_url", callback_url.into()),
        ("plugin-name", api::PLUGIN_NAME.into()),
        ("plugin-version", api::PLUGIN_VERSION.into()),
    ];
    let qs: Vec<String> = params
        .into_iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(&v)))
        .collect();
    (
        format!(
            "{}/portal/authorize?{}",
            api::PORTAL_AUTHORIZE,
            qs.join("&")
        ),
        ticket_id,
    )
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

/// 浏览器回调解析结果。
#[derive(Debug)]
enum Callback {
    Code {
        code: String,
    },
    Secret {
        secret: String,
        redirect: Option<String>,
    },
}

/// 等待浏览器回调。拿到凭据后**不立即响应**，由调用方在换取令牌后回 307，
/// 这样换 token 失败时门户能显示失败页，而不是先报成功再在终端报错。
async fn wait_callback(
    listener: tokio::net::TcpListener,
) -> anyhow::Result<(Callback, tokio::net::TcpStream)> {
    use tokio::io::AsyncReadExt;

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
        if method == "OPTIONS" {
            respond_204(&mut sock).await;
            continue;
        }
        if method != "GET" || route != CALLBACK_PATH {
            respond_404(&mut sock).await;
            continue;
        }

        match parse_callback(query) {
            Ok(cb) => return Ok((cb, sock)),
            Err(why) => {
                tracing::debug!(why, query, "回调校验失败");
                respond_400(&mut sock).await;
                continue;
            }
        }
    }
}

/// 解析回调查询串。官方优先 `code`，旧版回调只有 `secret`。
fn parse_callback(query: &str) -> Result<Callback, String> {
    // 官方按 truthiness 判断：空串视为没有，继续看 secret。
    if let Some(code) = login::query_param(query, "code").filter(|s| !s.is_empty()) {
        validate_callback_value("code", &code)?;
        return Ok(Callback::Code { code });
    }
    if let Some(secret) = login::query_param(query, "secret").filter(|s| !s.is_empty()) {
        validate_callback_value("secret", &secret)?;
        let redirect = login::query_param(query, "redirect").filter(|s| !s.trim().is_empty());
        return Ok(Callback::Secret { secret, redirect });
    }
    Err("缺少 code/secret".into())
}

fn validate_callback_value(name: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() || value.len() > 4096 || value.chars().any(|c| c.is_control()) {
        return Err(format!("{name} 非法"));
    }
    Ok(())
}

/// 门户成功/失败页。`custom` 仅用于旧版 secret 回调携带的 `redirect`。
async fn respond_redirect(sock: &mut tokio::net::TcpStream, success: bool, custom: Option<&str>) {
    use tokio::io::AsyncWriteExt;
    let next = match custom {
        Some(url) if !url.is_empty() => url.to_string(),
        _ => format!(
            "{}/portal/login?login_succeed={success}&locale=zh-cn&uri_scheme={}",
            api::PORTAL_AUTHORIZE,
            urlencoding::encode(api::CLIENT_ID)
        ),
    };
    let resp = format!(
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: {next}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
}

async fn respond_204(sock: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt;
    let _ = sock
        .write_all(
            b"HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: GET, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await;
    let _ = sock.shutdown().await;
}

async fn respond_400(sock: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt;
    let body = "Missing code/secret";
    let resp = format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
}

async fn respond_404(sock: &mut tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt;
    let _ = sock
        .write_all(
            b"HTTP/1.1 404 Not Found\r\nContent-Length: 8\r\nConnection: close\r\n\r\nNot found",
        )
        .await;
    let _ = sock.shutdown().await;
}

/// 旧版 `secret` 回调换票（`GET /snap-manager/v1/login/ticket`）。
///
/// 与扩展 `queryAuth` 等价，但采用 kernel 的 10×1s 重试。响应直接为
/// `{domain_id, user_id, user_name, credential:{access,secret,securitytoken,expires_at}}`，
/// 部分网关会再包一层 `{data: …}`；`TokenResponse` 已兼容 `credential` 字段。
async fn exchange_ticket(
    client: &reqwest::Client,
    ticket_id: &str,
    secret: &str,
    verifier: String,
    keys: KeyPair,
) -> Result<Credential, HttpError> {
    let url = format!("{}{}", api::SNAP_GATEWAY, TICKET_PATH);
    let mut last = HttpError::Protocol("secret 换票未执行".into());

    for attempt in 1..=TICKET_ATTEMPTS {
        let resp = client
            .get(&url)
            .query(&[("ticket_id", ticket_id), ("secret", secret)])
            .header("Content-Type", "application/json;charset=UTF-8")
            .header("plugin-name", api::PLUGIN_NAME)
            .header("plugin-version", api::PLUGIN_VERSION)
            .header("Accept", "application/json")
            .send()
            .await;

        match resp {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp.text().await.unwrap_or_default();
                if (200..300).contains(&status) {
                    match parse_ticket_body(&body) {
                        Ok(token) => {
                            return Credential::from_token(token, verifier, keys)
                                .map_err(|e| HttpError::Protocol(e.to_string()));
                        }
                        Err(e) => last = e,
                    }
                } else {
                    last = HttpError::Status { status, body };
                }
            }
            Err(e) => last = HttpError::Transport(e),
        }

        if attempt < TICKET_ATTEMPTS {
            tracing::info!(
                "[{}] secret 换票未成功（{last}），{attempt}/{TICKET_ATTEMPTS}，{}s 后重试",
                super::ID,
                TICKET_RETRY_DELAY.as_secs()
            );
            tokio::time::sleep(TICKET_RETRY_DELAY).await;
        }
    }
    Err(last)
}

fn parse_ticket_body(body: &str) -> Result<TokenResponse, HttpError> {
    let value: serde_json::Value = serde_json::from_str(body).map_err(|e| {
        HttpError::Protocol(format!(
            "解析 ticket 响应失败: {e}；响应片段: {}",
            truncate(body, 400)
        ))
    })?;
    let value = value.get("data").cloned().unwrap_or(value);
    serde_json::from_value(value).map_err(|e| {
        HttpError::Protocol(format!(
            "解析 ticket 响应字段失败: {e}；响应片段: {}",
            truncate(body, 400)
        ))
    })
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
        .header("Accept", "application/json")
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
        let (u, ticket) =
            authorize_url(34567, "http://127.0.0.1:34567/oauth/callback", "CHALLENGE");
        assert!(!ticket.is_empty());
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
        assert!(u.contains(&format!("ticket_id={ticket}")));
    }

    #[test]
    fn parse_callback_accepts_code_and_secret() {
        assert!(matches!(
            parse_callback("code=abc").unwrap(),
            Callback::Code { code, .. } if code == "abc"
        ));
        assert!(matches!(
            parse_callback("secret=xyz").unwrap(),
            Callback::Secret { secret, .. } if secret == "xyz"
        ));
        // 两者同时存在时官方优先 code。
        assert!(matches!(
            parse_callback("code=abc&secret=xyz").unwrap(),
            Callback::Code { .. }
        ));
        assert!(parse_callback("code=").is_err());
        assert!(parse_callback("").is_err());
    }

    #[test]
    fn parse_callback_keeps_redirect() {
        assert!(matches!(
            parse_callback("secret=s&redirect=https%3A%2F%2Fexample.com%2Fok").unwrap(),
            Callback::Secret { redirect: Some(url), .. } if url == "https://example.com/ok"
        ));
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
        assert_eq!(
            (ak.as_str(), sk.as_str(), st.as_str(), refresh.as_str()),
            ("AK", "SK", "ST", "REF")
        );
        assert!(exp.is_some());
    }

    #[test]
    fn rejects_missing_fields() {
        let body = r#"{ "credentials": { "access_key_id": "AK" } }"#;
        let t: TokenResponse = serde_json::from_str(body).unwrap();
        assert!(t.into_parts().is_err());
    }

    /// secret 换票响应：字段名是 `credential.access` 等，通常没有 refresh_token。
    #[test]
    fn parses_ticket_shape() {
        let body = r#"{
            "domain_id": "d",
            "user_id": "u",
            "user_name": "n",
            "credential": {
                "access": "AK",
                "secret": "SK",
                "securitytoken": "ST",
                "expires_at": "2026-02-13T07:38:53Z"
            }
        }"#;
        let t = parse_ticket_body(body).unwrap();
        let (ak, sk, st, refresh, exp) = t.into_parts().unwrap();
        assert_eq!((ak.as_str(), sk.as_str(), st.as_str()), ("AK", "SK", "ST"));
        assert_eq!(refresh, "");
        assert!(exp.is_some());
    }

    #[test]
    fn parses_ticket_wrapped_in_data() {
        let body = r#"{"data":{
            "credential": {
                "access": "AK",
                "secret": "SK",
                "securitytoken": "ST",
                "expires_at": "2026-02-13T07:38:53Z"
            }
        }}"#;
        assert!(parse_ticket_body(body).is_ok());
    }
}
