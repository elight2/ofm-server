//! WorkBuddy HTTP 层：域名、路径、业务信封与错误映射。
//!
//! 协议见 `provider-info/workbuddy.md` §1–§2。上游一律用腾讯系信封
//! `{"code":<int>,"msg":"…","data":{…}}`，**HTTP 200 也可能是业务失败**
//! （例如轮询令牌时回 `11217` 表示「用户尚未完成登录」），故一律先解信封。

use crate::core::text::truncate_chars;
use serde::Deserialize;
use serde::de::DeserializeOwned;

/// 主 endpoint（中国版）。
pub const HOST: &str = "https://www.workbuddy.cn";

/// 登录平台标识（`platform` 参数）。
pub const PLATFORM: &str = "workbuddy";

/// 授权入口（`POST {HOST}/v2/plugin/auth/state`）。
pub const AUTH_STATE_PATH: &str = "/v2/plugin/auth/state";
/// 令牌轮询（`GET {HOST}/v2/plugin/auth/token`）。
pub const AUTH_TOKEN_PATH: &str = "/v2/plugin/auth/token";
/// 令牌续期（`POST {HOST}/v2/plugin/auth/token/refresh`）。
pub const AUTH_REFRESH_PATH: &str = "/v2/plugin/auth/token/refresh";
/// 账号信息（`GET {HOST}/v2/plugin/account`）。
pub const ACCOUNT_PATH: &str = "/v2/plugin/account";

/// 登录轮询间隔（秒）。官方客户端固定 1s。
pub const POLL_INTERVAL_SECS: u64 = 1;
/// 登录轮询总超时（秒）。官方客户端固定 5 分钟。
pub const POLL_TIMEOUT_SECS: u64 = 300;

/// 业务码：令牌尚未就绪（用户还没在网页完成登录）。
pub const CODE_TOKEN_PENDING: i64 = 11217;
/// 业务码：账号尚未就绪。
pub const CODE_ACCOUNT_PENDING: i64 = 12151;

/// endpoint 主机名（官方 `enterpriseHeaders()` 在缺省时用它当 `X-Domain`）。
pub const DOMAIN: &str = "www.workbuddy.cn";

/// 客户端 UA。官方为 `WorkBuddy/<version>`。
pub const UA: &str = "WorkBuddy/5.11.3";

/// 业务信封。
#[derive(Debug, Deserialize)]
#[serde(
    rename_all = "camelCase",
    bound(deserialize = "T: serde::de::DeserializeOwned")
)]
pub struct Envelope<T> {
    /// 业务码，`0` 为成功。
    #[serde(default)]
    pub code: Option<i64>,
    /// 错误文案（失败时）。
    #[serde(default)]
    pub msg: Option<String>,
    /// 双语文案（腾讯系特有）。
    #[serde(default)]
    pub display_msg: Option<serde_json::Value>,
    /// 业务数据。
    #[serde(default)]
    pub data: Option<T>,
}

impl<T> Envelope<T> {
    pub fn is_ok(&self) -> bool {
        self.code.unwrap_or(0) == 0
    }

    /// 错误描述：`msg` 优先，缺失时用 `displayMsg`。
    pub fn describe(&self) -> String {
        let base = self
            .msg
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| match &self.display_msg {
                Some(v) => display_msg_text(v),
                None => "未知业务错误".to_string(),
            });
        match self.code {
            Some(c) if c != 0 => format!("{c} {base}"),
            _ => base,
        }
    }
}

/// 取 `displayMsg` 里的文案（对象形态取 `zh`，否则取第一个字符串值）。
pub fn display_msg_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Object(m) => m
            .get("zh")
            .and_then(|x| x.as_str())
            .or_else(|| m.values().find_map(|x| x.as_str()))
            .unwrap_or_default()
            .to_string(),
        other => other.to_string(),
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
    /// 业务错误（信封 `code != 0`）。
    #[error("业务错误({code:?}): {message}")]
    Business { code: Option<i64>, message: String },
}

impl HttpError {
    /// 业务码（仅 `Business` 变体有）。对话阶段用于区分渠道/账号类错误。
    #[allow(dead_code)]
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
            ref other @ HttpError::Status {
                status: 401 | 403, ..
            } => P::Unauthorized(other.to_string()),
            HttpError::Business { code: Some(c), .. }
                if c == CODE_TOKEN_PENDING || c == CODE_ACCOUNT_PENDING =>
            {
                P::Other(e.to_string())
            }
            HttpError::Decode(_) => P::Protocol(e.to_string()),
            _ => P::Other(e.to_string()),
        }
    }
}

/// 构建 HTTP 客户端。
pub fn client() -> Result<reqwest::Client, reqwest::Error> {
    crate::core::http::client(crate::core::http::Spec::ua(UA).timeout(60))
}

/// 构造 `auth/state` / `auth/token` 用的「无身份」请求头。
///
/// 官方在这两个端点上显式声明不带身份（`X-No-*` 头），避免带上旧的
/// Authorization / 用户 id 被服务端误判成「已登录」而直接返回旧令牌。
pub fn anonymous_headers() -> Vec<(&'static str, String)> {
    vec![
        ("X-No-Authorization", "true".to_string()),
        ("X-No-User-Id", "true".to_string()),
        ("X-No-Enterprise-Id", "true".to_string()),
        ("X-No-Department-Info", "true".to_string()),
        ("Accept", "application/json".to_string()),
        ("Content-Type", "application/json".to_string()),
    ]
}

/// 构造带身份的请求头（`/account`、续期用）。
///
/// 官方 `enterpriseHeaders()` 还会带 `X-Domain`（租户域名，缺省用 endpoint 主机名）。
pub fn auth_headers(access_token: &str, domain: &str) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("Authorization", format!("Bearer {access_token}")),
        ("Accept", "application/json".to_string()),
        ("Content-Type", "application/json".to_string()),
    ];
    let domain = domain.trim();
    headers.push((
        "X-Domain",
        if domain.is_empty() {
            DOMAIN.to_string()
        } else {
            domain.to_string()
        },
    ));
    headers
}

/// 发 POST 并解信封。
pub async fn post_envelope<B, R>(
    client: &reqwest::Client,
    url: &str,
    headers: &[(&str, String)],
    body: &B,
) -> Result<R, HttpError>
where
    B: serde::Serialize + ?Sized,
    R: DeserializeOwned,
{
    let payload = serde_json::to_vec(body).map_err(|e| HttpError::Decode(e.to_string()))?;
    let mut http = client.post(url).body(payload);
    for (k, v) in headers {
        http = http.header(*k, v);
    }
    let resp = http.send().await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    parse_envelope::<R>(status.as_u16(), &text)
}

/// 发 GET 并解信封。
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

/// 把 HTTP 状态 + 原文解析成 `data`。
///
/// 业务错误可能回非 2xx，故先按信封解析：能拿到业务码就报业务错（保留错误码）。
pub fn parse_envelope<R: DeserializeOwned>(status: u16, text: &str) -> Result<R, HttpError> {
    if let Ok(env) = serde_json::from_str::<Envelope<R>>(text)
        && !env.is_ok()
    {
        return Err(HttpError::Business {
            code: env.code,
            message: env.describe(),
        });
    }
    if !(200..300).contains(&status) {
        return Err(HttpError::Status {
            status,
            body: truncate_chars(text, 300),
        });
    }
    let env: Envelope<R> = serde_json::from_str(text)
        .map_err(|e| HttpError::Decode(format!("{e}; 原文: {}", truncate_chars(text, 300))))?;
    env.data
        .ok_or_else(|| HttpError::Decode("响应缺少 data".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_ok_when_code_zero() {
        let e: Envelope<serde_json::Value> =
            serde_json::from_str(r#"{"code":0,"data":{}}"#).unwrap();
        assert!(e.is_ok());
    }

    #[test]
    fn 业务错误保留码与文案() {
        let e: Envelope<serde_json::Value> =
            serde_json::from_str(r#"{"code":11217,"msg":"pending"}"#).unwrap();
        assert!(!e.is_ok());
        assert!(e.describe().contains("11217"));
        assert!(e.describe().contains("pending"));
    }

    #[test]
    fn display_msg_优先中文() {
        let e: Envelope<serde_json::Value> = serde_json::from_str(
            r#"{"code":11140,"displayMsg":{"zh":"账号异常","en":"abnormal"}}"#,
        )
        .unwrap();
        assert!(e.describe().contains("账号异常"), "{}", e.describe());
    }

    #[test]
    fn 缺码视为成功() {
        let e: Envelope<serde_json::Value> = serde_json::from_str(r#"{"data":1}"#).unwrap();
        assert!(e.is_ok());
    }
}
