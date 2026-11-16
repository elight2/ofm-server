//! Raccoon HTTP 层：域名、常量、业务信封解析。
//!
//! 协议见 `provider-info/raccoon.md`。上游统一返回
//! `{code: <int>, message: <str>, data: ...}`，**HTTP 200 也可能是错误**，
//! 因此一律先解析业务码再看 HTTP 状态。

use serde::de::DeserializeOwned;
use serde::Deserialize;

/// 主站（同时是 auth / llm / points / desktop 各前缀的 host）。
pub const HOST: &str = "https://xiaohuanxiong.com";

/// 账户与登录（`.env.electron` `NEXT_PUBLIC_AUTH_API_PREFIX`）。
pub const AUTH_PREFIX: &str = "/api/web/auth/v1";
/// 大模型网关。
pub const LLM_PREFIX: &str = "/api/web/llm/v2";
/// 积分。
pub const POINTS_PREFIX: &str = "/api/web/points";
/// 桌面端专属接口。
pub const DESKTOP_PREFIX: &str = "/api/web/desktop/v1";

/// 登录与续期用 UA：官方是 Electron 41 桌面壳。
pub const UA: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) raccoon-ai/1.2.0 Chrome/142.0.7444.235 Electron/41.0.1 Safari/537.36";

/// 客户端版本，`X-Client-Version` 用。
pub const CLIENT_VERSION: &str = "v1.2.0";
/// `X-Client-Platform` 取值（`desktop/{platform}` 约定见官方）。
pub const CLIENT_PLATFORM: &str = "desktop-linux";

/// 授权页路径（`DESKTOP_AUTH_PATH`）。
pub const AUTHORIZE_PATH: &str = "/authorize";
/// 授权页 `appname` 参数（`enterpriseName('zh')` + `客户端`）。
pub const APP_NAME: &str = "商汤小浣熊客户端";

/// `login_with_authorization_code` 错误：授权码不存在/过期/已消费。
pub const CODE_AUTH_CODE_INVALID: i64 = 200035;
/// 身份无效（`orgNotExists` / `userNotInOrg`），需切回 personal 后重试。
pub const CODES_IDENTITY: [i64; 2] = [200022, 200201];
/// 登录失效。
pub const CODES_UNAUTHORIZED: [i64; 2] = [200003, 200009];

/// 业务信封。
#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    #[serde(default)]
    pub code: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub details: Option<serde_json::Value>,
    #[serde(default = "Option::default")]
    pub data: Option<T>,
}

impl<T> Envelope<T> {
    pub fn is_ok(&self) -> bool {
        self.code.unwrap_or(0) == 0
    }

    /// 错误描述；`details` 常带具体原因，一并拼上便于排查。
    pub fn describe(&self) -> String {
        let mut s = match (&self.message, self.code) {
            (Some(m), Some(c)) => format!("{c} {m}"),
            (Some(m), None) => m.clone(),
            (None, Some(c)) => format!("业务码 {c}"),
            (None, None) => "未知业务错误".to_string(),
        };
        if let Some(d) = &self.details {
            let text = match d {
                serde_json::Value::String(s) if !s.is_empty() => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            if !text.is_empty() && text != "null" {
                s.push_str(&format!(" ({text})"));
            }
        }
        s
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("网络请求失败: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("上游 HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("响应解析失败: {0}")]
    Decode(String),
    #[error("业务错误({code:?}): {message}")]
    Business {
        code: Option<i64>,
        message: String,
    },
}

impl HttpError {
    /// 业务码（仅 `Business` 变体有）。
    pub fn business_code(&self) -> Option<i64> {
        match self {
            HttpError::Business { code, .. } => *code,
            _ => None,
        }
    }
}

impl From<HttpError> for crate::core::provider::ProviderError {
    fn from(e: HttpError) -> Self {
        use crate::core::provider::ProviderError as P;
        match e {
            HttpError::Transport(err) => P::Transport(err),
            HttpError::Status { status: 401 | 403, .. } => P::Unauthorized(e.to_string()),
            HttpError::Business { code: Some(c), .. } if CODES_UNAUTHORIZED.contains(&c) => {
                P::Unauthorized(e.to_string())
            }
            HttpError::Business { code: Some(c), .. } if CODES_IDENTITY.contains(&c) => {
                P::Unauthorized(format!("{e}（企业身份无效，需切回个人身份）"))
            }
            HttpError::Decode(_) => P::Protocol(e.to_string()),
            _ => P::Other(e.to_string()),
        }
    }
}

pub fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(300))
        .build()
}

/// 发 POST 并解信封。`bearer` 为 `None` 时不带 `Authorization`。
pub async fn post_envelope<B, R>(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
    body: &B,
) -> Result<R, HttpError>
where
    B: serde::Serialize + ?Sized,
    R: DeserializeOwned,
{
    let payload = serde_json::to_vec(body).map_err(|e| HttpError::Decode(e.to_string()))?;
    let mut http = client.post(url).body(payload);
    if let Some(t) = bearer {
        http = http.header("authorization", format!("Bearer {t}"));
    }
    let resp = http.send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    parse_envelope::<R>(status.as_u16(), &text)
}

/// 发 GET 并解信封。
///
/// 模型目录、余额都是 GET，且同样可能业务错误回非 2xx，
/// 故解析逻辑与 [`post_envelope`] 完全共用 [`parse_envelope`]。
pub async fn get_envelope<R>(
    client: &reqwest::Client,
    url: &str,
    headers: &[(&str, String)],
) -> Result<R, HttpError>
where
    R: DeserializeOwned,
{
    let mut http = client.get(url);
    for (k, v) in headers {
        http = http.header(*k, v);
    }
    let resp = http.send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    parse_envelope::<R>(status.as_u16(), &text)
}

/// 把 HTTP 状态 + 原文解析成 `data`。两种请求方式的公共部分。
fn parse_envelope<R: DeserializeOwned>(status: u16, text: &str) -> Result<R, HttpError> {
    // ⚠️ 业务错误可能返回非 2xx（实测 200035 → HTTP 400），
    // 所以先尝试按信封解析，能拿到业务码就报业务错，保留错误码。
    if let Ok(env) = serde_json::from_str::<Envelope<R>>(text) {
        if !env.is_ok() {
            return Err(HttpError::Business {
                code: env.code,
                message: env.describe(),
            });
        }
    }
    if !(200..300).contains(&status) {
        return Err(HttpError::Status {
            status,
            body: truncate(text, 300),
        });
    }
    let env: Envelope<R> = serde_json::from_str(text)
        .map_err(|e| HttpError::Decode(format!("{e}; 原文: {}", truncate(text, 300))))?;
    // 2xx 但缺 data：换 token 这类端点 data 必有，缺失即异常。
    env.data
        .ok_or_else(|| HttpError::Decode("响应缺少 data".to_string()))
}

/// 同上，用于没有 `data` 或 `data` 可选的端点（如 `login/points/grant`）。
pub async fn post_raw<B>(
    client: &reqwest::Client,
    url: &str,
    bearer: Option<&str>,
    body: &B,
) -> Result<serde_json::Value, HttpError>
where
    B: serde::Serialize + ?Sized,
{
    post_envelope::<B, serde_json::Value>(client, url, bearer, body).await
}

/// 字符串截断到 `max` 字节（保持 UTF-8 边界）。
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_ok_when_code_zero() {
        let e: Envelope<serde_json::Value> = serde_json::from_str(r#"{"code":0,"data":{}}"#).unwrap();
        assert!(e.is_ok());
    }

    #[test]
    fn envelope_error_keeps_details() {
        let e: Envelope<serde_json::Value> =
            serde_json::from_str(r#"{"code":200035,"message":"x","details":"param token invalid"}"#)
                .unwrap();
        assert!(!e.is_ok());
        assert!(e.describe().contains("200035"));
        assert!(e.describe().contains("param token invalid"));
    }

    #[test]
    fn missing_code_treated_as_ok() {
        let e: Envelope<serde_json::Value> = serde_json::from_str(r#"{"data":1}"#).unwrap();
        assert!(e.is_ok());
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// 起一个只回固定响应的本地服务，返回 URL。
    async fn mock(status_line: &'static str, body: &'static str) -> String {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => return,
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let resp = format!(
                        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        format!("http://{addr}/x")
    }

    #[derive(Debug, Deserialize)]
    struct Dummy {
        #[allow(dead_code)]
        v: i64,
    }

    /// 实测行为：业务错误回 HTTP 400，业务码在 body 里，必须被识别。
    #[tokio::test]
    async fn business_code_survives_non_2xx() {
        let url = mock(
            "400 Bad Request",
            r#"{"code":200035,"message":"authorization_code_not_found_error","details":"authorization code not found"}"#,
        )
        .await;
        let client = client().unwrap();
        let err = post_envelope::<_, Dummy>(&client, &url, None, &serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(err.business_code(), Some(200035));
        assert!(err.to_string().contains("authorization code not found"));
    }

    #[tokio::test]
    async fn ok_body_parses_data() {
        let url = mock("200 OK", r#"{"code":0,"data":{"v":7}}"#).await;
        let client = client().unwrap();
        let d: Dummy = post_envelope(&client, &url, None, &serde_json::json!({}))
            .await
            .unwrap();
        assert_eq!(d.v, 7);
    }

    /// 非 JSON 的错误体仍报 HTTP 状态。
    #[tokio::test]
    async fn non_json_error_reports_status() {
        let url = mock("502 Bad Gateway", "upstream down").await;
        let client = client().unwrap();
        let err = post_envelope::<_, Dummy>(&client, &url, None, &serde_json::json!({}))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Status { status: 502, .. }));
    }
}
