//! MiniMax HTTP 层：域名、固定参数、PKCE、OAuth 表单请求。
//!
//! 协议见 `provider-info/minimax.md` §1。登录走 OAuth 2.0 设备码授权 + PKCE，
//! 认证域（`account.minimax.cn`）与 API 域（`agent.minimax.cn`）分离，
//! 且域名**写死预设**，不读模型目录下发的旧 `baseURL`（§0 出入 1）。

use serde::de::DeserializeOwned;

/// 账号中心（OAuth 设备码、令牌、撤销）。
pub const ACCOUNT_HOST: &str = "https://account.minimax.cn";
/// API 域（模型目录、对话）。
///
/// 目录下发的 `options.baseURL` 是旧域名 `agent.minimaxi.com`，按 §0 出入 1 忽略。
pub const AGENT_HOST: &str = "https://agent.minimax.cn";
/// 设备码端点。
pub const DEVICE_CODE_PATH: &str = "/oauth2/device/code";
/// 令牌端点（设备码轮询与 refresh_token 续期共用）。
pub const TOKEN_PATH: &str = "/oauth2/token";
/// 模型目录路径。
///
/// 注意：**不能**从对话 baseURL 推导（源码会拼成 `…/llm/v1/models`），
/// 抓包的真实路径比它少一级 `llm/v1`（§4）。
pub const MODELS_PATH: &str = "/mavis/api/v1/models";
/// 模型目录的必需 query（§4）。
pub const MODELS_QUERY: &str = "region=cn&buildEnv=prod";

/// OAuth 固定参数。
pub const CLIENT_ID: &str = "mcode-public";
/// 授权范围。
pub const SCOPE: &str = "agent.default";
/// 令牌受众。
pub const AUDIENCE: &str = "agent-backend";

/// 设备码授权类型（设备码轮询用）。
pub const GRANT_DEVICE_CODE: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// `slow_down` 时轮询间隔的增量（秒），见 §1.2。
pub const SLOW_DOWN_INCREMENT_SECS: u64 = 5;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("网络请求失败: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("上游 HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("响应解析失败: {0}")]
    Decode(String),
    /// OAuth 标准错误体：`{"error":"…","error_description":"…"}`。
    #[error("OAuth 错误({error}): {description}")]
    OAuth { error: String, description: String },
}

impl From<HttpError> for crate::core::provider::ProviderError {
    fn from(e: HttpError) -> Self {
        use crate::core::provider::ProviderError as P;
        match e {
            HttpError::Transport(err) => P::Transport(err),
            ref other @ HttpError::Status {
                status: 401 | 403, ..
            } => P::Unauthorized(other.to_string()),
            ref other @ HttpError::Decode(_) => P::Protocol(other.to_string()),
            ref other => P::Other(other.to_string()),
        }
    }
}

/// OAuth 端点用 UA。抓包里客户端侧请求不校验 UA，用简洁标识即可。
pub const UA: &str = "MiniMaxCode";

/// 模型目录用 UA。
///
/// 与对话接口不同：目录接口要**桌面浏览器完整 UA**（§4 抓包），
/// 对话接口用 `MiniMaxAgent`。
pub const MODELS_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) MiniMax/3.1.16 Chrome/148.0.7778.280 Electron/42.11.16 Safari/537.36";

/// 构建 HTTP 客户端。
pub fn client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(UA)
        .timeout(std::time::Duration::from_secs(60))
        .build()
}

/// 构建模型目录用的 HTTP 客户端（UA 不同）。
pub fn models_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(MODELS_UA)
        .timeout(std::time::Duration::from_secs(60))
        .build()
}

/// 发 GET 并解 JSON。
///
/// 目录接口只带 `authorization: Bearer`，**不带** `x-api-key`、
/// `anthropic-version`、`x-mavis-*`（§4 抓包定论）。
pub async fn get_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<T, HttpError> {
    let resp = client
        .get(url)
        .header("accept", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status.as_u16()) {
        // 目录接口无业务码信封，HTTP 非 200 即失败（§8.3）。
        return Err(HttpError::Status {
            status: status.as_u16(),
            body: truncate(&text, 300),
        });
    }
    serde_json::from_str(&text)
        .map_err(|e| HttpError::Decode(format!("{e}; 原文: {}", truncate(&text, 300))))
}

/// 生成 PKCE `(code_verifier, code_challenge)`。
///
/// 32 字节随机 → base64url 无 padding（43 字符）作 verifier，
/// challenge = base64url(sha256(verifier))（同样 43 字符），见 §1.1。
pub fn pkce_pair() -> (String, String) {
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("获取随机数失败");
    let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let verifier = engine.encode(buf);
    let challenge = engine.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// 发 `application/x-www-form-urlencoded` POST 并解 JSON。
///
/// 成功返回原文字节（调用方按端点自行反序列化），失败按 OAuth 错误体解析。
/// 注意：OAuth 端点**不带** cookie、不带 `origin`（§1.3）。
pub async fn post_form(
    client: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<String, HttpError> {
    let resp = client
        .post(url)
        .header("accept", "application/json")
        .form(form)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if (200..300).contains(&status.as_u16()) {
        return Ok(text);
    }
    // OAuth 标准错误体优先；解析不出就报 HTTP 状态。
    if let Ok(OAuthErrorBody {
        error: Some(error),
        error_description,
    }) = serde_json::from_str::<OAuthErrorBody>(&text)
    {
        return Err(HttpError::OAuth {
            error,
            description: error_description.unwrap_or_default(),
        });
    }
    Err(HttpError::Status {
        status: status.as_u16(),
        body: truncate(&text, 300),
    })
}

/// 发 form POST 并反序列化成功响应。
pub async fn post_form_json<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    form: &[(&str, &str)],
) -> Result<T, HttpError> {
    let text = post_form(client, url, form).await?;
    serde_json::from_str(&text)
        .map_err(|e| HttpError::Decode(format!("{e}; 原文: {}", truncate(&text, 300))))
}

/// OAuth 错误响应体（两种键名变体都要兼容，见 §8.1），仅用于解出错误码。
#[derive(Debug, serde::Deserialize)]
struct OAuthErrorBody {
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    error_description: Option<String>,
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

    /// verifier 与 challenge 都是 43 字符 base64url，且互不相同。
    #[test]
    fn pkce_形状正确() {
        let (v, c) = pkce_pair();
        assert_eq!(v.len(), 43);
        assert_eq!(c.len(), 43);
        assert_ne!(v, c);
        let ok = |s: &str| {
            s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        };
        assert!(ok(&v) && ok(&c));
        assert_ne!(v, pkce_pair().0);
    }

    /// challenge 必须等于 sha256(verifier) 的 base64url。
    #[test]
    fn pkce_challenge_是_verifier_的_hash() {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let (v, c) = pkce_pair();
        let expect =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(v.as_bytes()));
        assert_eq!(c, expect);
    }

    #[test]
    fn 截断保持_utf8_边界() {
        assert_eq!(truncate("abc", 10), "abc");
        let s = "汉字";
        assert!(s.len() == 6);
        assert_eq!(truncate(s, 4), "汉…");
    }
}
