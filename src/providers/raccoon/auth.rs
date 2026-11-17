//! Raccoon 登录：浏览器授权 + 本地 loopback 回调 → 授权码换 token。
//!
//! 协议见 `provider-info/raccoon.md` §1「loopback 兼容登录」。流程：
//! 1. 本地 `127.0.0.1:0` 起一次性服务，路径 `/auth/callback`；
//! 2. 打开 `https://xiaohuanxiong.com/authorize`
//!    （`login_source=desktop`、`desktop_callback=loopback`、`redirect`、`state`）；
//! 3. 浏览器回连本地，取 `code`；
//! 4. `login_with_authorization_code` 换 `access_token`/`refresh_token`。
//!
//! 校验规则照抄官方 `webLoginLoopback.js`：`state` 必须为 64 位小写 hex 且
//! 与本地值相等，query 只允许 `code`/`state`，只接受 `127.0.0.1` 来源。

use std::time::Duration;

use serde::Deserialize;

use super::api::{self, HttpError};
use super::credential::{self, Credential};
use crate::cli::login;

/// 回调路径（官方 `parseCallback` 硬编码）。
pub const CALLBACK_PATH: &str = "/auth/callback";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// `login_with_authorization_code` 响应 `data`。
#[derive(Debug, Deserialize)]
pub struct LoginData {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub office_identity: Option<String>,
    #[serde(default)]
    pub office_org_name: Option<String>,
    #[serde(default)]
    pub office_org_role: Option<String>,
}

/// 登录入口。
pub async fn run(root: std::path::PathBuf) -> anyhow::Result<()> {
    let state = random_state();
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let authority = format!("127.0.0.1:{port}");

    let url = authorize_url(&redirect, &state);
    println!("[raccoon] 已生成授权链接，正在打开浏览器：\n{url}\n");
    if let Err(e) = login::open_browser(&url) {
        eprintln!("[raccoon] 打开浏览器失败（{e}），请手动访问上面的链接");
    }
    println!("[raccoon] 等待浏览器回调 {redirect}");

    let code = wait_callback(listener, &authority, &state).await?;
    println!("[raccoon] 已获得授权码，正在换取令牌…");

    let client = api::client()?;
    let data = exchange_code(&client, &code).await?;
    let cred = Credential {
        access_token: data.access_token,
        refresh_token: data.refresh_token,
        office_identity: data.office_identity.filter(|s| !s.trim().is_empty()),
        office_org_name: data.office_org_name.filter(|s| !s.trim().is_empty()),
        office_org_role: data.office_org_role.filter(|s| !s.trim().is_empty()),
        last_auth_at: Some(credential::now_millis()),
    };
    cred.store(&root)?;

    let path = credential::PROVIDER;
    println!(
        "[raccoon] 登录成功，凭证已保存到 {}",
        crate::config::Config::credential_path(&root, path).display()
    );
    match credential::jwt_exp(&cred.access_token) {
        Some(exp) => println!("[raccoon] 访问令牌有效期至 {}", fmt_unix(exp)),
        None => println!("[raccoon] 访问令牌非 JWT，无法读取有效期"),
    }
    if cred.is_org() {
        println!(
            "[raccoon] 企业身份：{}（请求将带 X-Org-Code）",
            cred.office_org_name.as_deref().unwrap_or("-")
        );
    } else {
        println!("[raccoon] 个人身份");
    }
    Ok(())
}

/// 用授权码换 token。
///
/// 授权码无效时上游回 **HTTP 400 + `code:200035`**（实测），
/// 这里翻成可读提示，避免用户只看到一行 HTTP 错误。
pub async fn exchange_code(client: &reqwest::Client, code: &str) -> Result<LoginData, HttpError> {
    let url = format!(
        "{}{}/login_with_authorization_code",
        api::HOST,
        api::AUTH_PREFIX
    );
    let body = serde_json::json!({ "authorization_code": code });
    api::post_envelope::<_, LoginData>(client, &url, None, &body)
        .await
        .map_err(|e| match e.business_code() {
            Some(api::CODE_AUTH_CODE_INVALID) => HttpError::Business {
                code: Some(api::CODE_AUTH_CODE_INVALID),
                message: "授权码不存在或已过期/已使用，请重新发起登录".to_string(),
            },
            _ => e,
        })
}

/// 拼授权 URL。参数顺序与官方一致，`desktop_callback=loopback` 必需，
/// 否则网页按深链（`office-raccoon://`）回调，服务器收不到。
pub fn authorize_url(redirect: &str, state: &str) -> String {
    let params: [(&str, &str); 5] = [
        ("login_source", "desktop"),
        ("appname", api::APP_NAME),
        ("desktop_callback", "loopback"),
        ("redirect", redirect),
        ("state", state),
    ];
    let qs: Vec<String> = params
        .into_iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(v)))
        .collect();
    format!("{}{}?{}", api::HOST, api::AUTHORIZE_PATH, qs.join("&"))
}

/// 32 字节随机数的 hex（64 位小写），与官方 `randomBytes(32).toString('hex')` 一致。
fn random_state() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("获取随机数失败");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 等待浏览器回调并取授权码。校验规则见模块注释。
async fn wait_callback(
    listener: tokio::net::TcpListener,
    authority: &str,
    state: &str,
) -> anyhow::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let deadline = tokio::time::Instant::now() + CALLBACK_TIMEOUT;

    loop {
        let (mut sock, peer) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| anyhow::anyhow!("等待回调超时（{}s）", CALLBACK_TIMEOUT.as_secs()))??;

        // 只接受本机来源（官方同样校验 remoteAddress === '127.0.0.1'）。
        if !peer.ip().is_loopback() {
            let _ = sock.shutdown().await;
            continue;
        }

        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let request = String::from_utf8_lossy(&buf[..n]).to_string();
        let mut lines = request.lines();
        let first_line = lines.next().unwrap_or_default();
        let mut parts = first_line.split_whitespace();
        let method = parts.next().unwrap_or_default();
        let target = parts.next().unwrap_or_default();

        // Host 必须等于本地 authority，防 DNS rebinding。
        let host = request
            .lines()
            .find_map(|l| {
                let (k, v) = l.split_once(':')?;
                k.eq_ignore_ascii_case("host").then(|| v.trim().to_string())
            })
            .unwrap_or_default();
        if method != "GET" || host != authority {
            respond(&mut sock, 403, "无效的登录回调").await;
            continue;
        }

        let (route, query) = match target.split_once('?') {
            Some((r, q)) => (r, q),
            None => (target, ""),
        };
        if route != CALLBACK_PATH {
            respond(&mut sock, 404, "404").await;
            continue;
        }

        match parse_callback(query, state) {
            Ok(code) => {
                respond(&mut sock, 200, "登录完成，可以关闭此页面。").await;
                return Ok(code);
            }
            Err(why) => {
                tracing::debug!(why, query, "回调校验失败");
                respond(&mut sock, 400, "无效的登录回调，请从客户端重新发起登录。").await;
                continue;
            }
        }
    }
}

/// 解析并校验回调查询串，返回授权码。
///
/// 官方规则：query 只允许 `code`/`state`、各出现一次、`state` 为 64 位 hex 且相等、
/// `code` 非空且不含控制字符。
fn parse_callback(query: &str, state: &str) -> Result<String, String> {
    let mut code: Option<String> = None;
    let mut got_state: Option<String> = None;
    for kv in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let v = urlencoding::decode(v)
            .map_err(|e| format!("参数解码失败: {e}"))?
            .into_owned();
        match k {
            "code" => {
                if code.replace(v).is_some() {
                    return Err("code 重复".into());
                }
            }
            "state" => {
                if got_state.replace(v).is_some() {
                    return Err("state 重复".into());
                }
            }
            other => return Err(format!("非预期参数 `{other}`")),
        }
    }
    let code = code.ok_or("缺少 code")?;
    let got_state = got_state.ok_or("缺少 state")?;
    if got_state != state {
        return Err("state 不匹配".into());
    }
    if got_state.len() != 64
        || !got_state
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        return Err("state 格式非法".into());
    }
    if code.trim().is_empty() || code.len() > 4096 {
        return Err("code 长度非法".into());
    }
    if code.chars().any(|c| c.is_control()) {
        return Err("code 含控制字符".into());
    }
    Ok(code)
}

async fn respond(sock: &mut tokio::net::TcpStream, status: u16, message: &str) {
    use tokio::io::AsyncWriteExt;
    let body = format!(
        "<!doctype html><html lang=\"zh\"><meta charset=\"utf-8\"><title>商汤小浣熊登录</title><h1>{message}</h1></html>"
    );
    let head = match status {
        200 => "200 OK",
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        _ => "404 Not Found",
    };
    let resp = format!(
        "HTTP/1.1 {head}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;
}

fn fmt_unix(secs: i64) -> String {
    match time::OffsetDateTime::from_unix_timestamp(secs) {
        Ok(dt) => {
            let (y, mo, d) = dt.to_calendar_date();
            format!(
                "{y:04}-{:02}-{d:02} {:02}:{:02}:{:02}",
                mo as u8,
                dt.hour(),
                dt.minute(),
                dt.second()
            )
        }
        Err(_) => secs.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn state_is_64_lower_hex() {
        let s = random_state();
        assert_eq!(s.len(), 64);
        assert!(
            s.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_ne!(s, random_state());
    }

    #[test]
    fn authorize_url_has_required_params() {
        let u = authorize_url("http://127.0.0.1:1234/auth/callback", ST);
        assert!(u.starts_with("https://xiaohuanxiong.com/authorize?"));
        assert!(u.contains("login_source=desktop"));
        assert!(u.contains("desktop_callback=loopback"));
        assert!(u.contains("redirect=http%3A%2F%2F127.0.0.1%3A1234%2Fauth%2Fcallback"));
        assert!(u.contains(&format!("state={ST}")));
        assert!(u.contains("appname="));
    }

    #[test]
    fn parse_ok() {
        let q = format!("code=abc123&state={ST}");
        assert_eq!(parse_callback(&q, ST).unwrap(), "abc123");
    }

    #[test]
    fn parse_rejects_state_mismatch() {
        let q = format!("code=abc&state={}", "f".repeat(64));
        assert!(parse_callback(&q, ST).is_err());
    }

    #[test]
    fn parse_rejects_missing_and_duplicate() {
        assert!(parse_callback(&format!("state={ST}"), ST).is_err());
        assert!(parse_callback(&format!("code=a&code=b&state={ST}"), ST).is_err());
    }

    #[test]
    fn parse_rejects_extra_param() {
        let q = format!("code=a&state={ST}&evil=1");
        assert!(parse_callback(&q, ST).is_err());
    }

    #[test]
    fn parse_rejects_bad_state_format() {
        assert!(parse_callback("code=a&state=SHORT", "SHORT").is_err());
        // 大写 hex 不被接受
        let upper = "A".repeat(64);
        assert!(parse_callback(&format!("code=a&state={upper}"), &upper).is_err());
    }

    #[test]
    fn parse_rejects_control_chars_in_code() {
        let q = format!("code=a%00b&state={ST}");
        assert!(parse_callback(&q, ST).is_err());
    }

    #[test]
    fn parse_decodes_percent_encoding() {
        let q = format!("code=a%2Fb%3D&state={ST}");
        assert_eq!(parse_callback(&q, ST).unwrap(), "a/b=");
    }
}
