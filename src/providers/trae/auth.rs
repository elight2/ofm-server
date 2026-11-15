//! Trae 登录：PKCE S256 → 浏览器授权 → `GetPCAuthCode` → `ExchangeToken`。
//!
//! 流程与字段见 `provider-info/trae.md` §1。与官方客户端的已知出入：
//! 回调端口沿用官方 `40909`（参考实现用 18080，此处取官方值）。
//!
//! 回调只拿到浏览器跳转的查询串；其中的授权码键名由授权页决定，
//! 因此同时接受 `auth_code` / `code` / `authCode` 三种写法，取第一个命中的非空值。

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::cli::login;
use crate::providers::trae::api::{self, HttpError};
use crate::providers::trae::credential::Credential;
use crate::providers::trae::device::Device;
use crate::providers::trae::pkce;

const EXCHANGE_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";

/// 官方硬编码回调端口。
pub const CALLBACK_PORT: u16 = 40909;
/// 回调路径（官方 `AUTHORIZE`）。
pub const CALLBACK_PATH: &str = "/authorize";
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// 登录入口。
pub async fn login(root: PathBuf) -> anyhow::Result<()> {
    let existing = Credential::load(&root)?;
    let device = Device::load_or_create(
        existing.as_ref().map(|c| c.machine_id.clone()),
        existing.as_ref().map(|c| c.device_id.clone()),
        existing.as_ref().map(|c| c.device_private_key_pem.clone()),
    )?;

    let verifier = pkce::new_verifier();
    let challenge = pkce::challenge(&verifier);
    let trace_id = pkce::uuid_v4();

    let url = authorize_url(&device, &challenge, &trace_id);
    println!("[trae] 已生成授权链接，正在打开浏览器：\n{url}\n");
    if let Err(e) = login::open_browser(&url) {
        eprintln!("[trae] 打开浏览器失败（{e}），请手动访问上面的链接");
    }

    println!("[trae] 等待浏览器回调 http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}");
    let query = login::wait_callback_on(CALLBACK_PORT, CALLBACK_PATH, CALLBACK_TIMEOUT).await?;

    if let Some(err) = login::query_param(&query, "error") {
        anyhow::bail!("授权页返回错误: {err}");
    }
    tracing::debug!(query = %query, "收到登录回调");

    // 回调把授权码放在 `authCodeInfo`（URL 编码的 JSON），另有 `host` 指明 API 主机。
    let info = callback_auth_code(&query)?;
    println!("[trae] 已获得授权码（{} 内有效），正在换取令牌…", fmt_ms(info.expire_at));

    let client = api::client()?;
    let host = login::query_param(&query, "host")
        .filter(|h| h.starts_with("https://"))
        .unwrap_or_else(|| api::HOST_WEB.to_string());
    let token = exchange_token(&client, &host, &device, &info.auth_code, &verifier).await?;
    let cred = Credential::from_auth(&device, token, existing.as_ref());
    cred.store(&root)?;

    println!("[trae] 登录成功，凭证已保存到 {}", root.join("providers/trae/credential.toml").display());
    println!("[trae] 令牌有效期至 {}", fmt_ms(cred.token_expire_at));
    Ok(())
}

/// 拼授权 URL（参数顺序与官方客户端一致）。
pub fn authorize_url(device: &Device, challenge: &str, trace_id: &str) -> String {
    let callback = format!("http://127.0.0.1:{CALLBACK_PORT}{CALLBACK_PATH}");
    let params: Vec<(&str, String)> = vec![
        ("login_version", "1".into()),
        ("auth_from", "trae".into()),
        ("login_channel", "native_ide".into()),
        ("plugin_version", "2.3.102387".into()),
        ("auth_type", "local".into()),
        ("client_id", api::CLIENT_ID.to_string()),
        ("redirect", "0".into()),
        ("login_trace_id", trace_id.to_string()),
        ("auth_callback_url", callback),
        ("machine_id", device.machine_id.clone()),
        ("device_id", device.device_id.clone()),
        ("x_device_id", device.device_id.clone()),
        ("x_machine_id", device.machine_id.clone()),
        ("x_device_brand", String::new()),
        ("x_device_type", "linux".into()),
        ("x_os_version", device.os_version.clone()),
        ("x_env", String::new()),
        ("x_app_version", api::APP_VERSION.into()),
        ("x_app_type", "stable".into()),
        ("code_challenge", challenge.to_string()),
        ("code_challenge_method", "S256".into()),
        ("channel_name", "common".into()),
    ];
    let qs: Vec<String> = params
        .into_iter()
        .map(|(k, v)| format!("{k}={}", urlencoding::encode(&v)))
        .collect();
    format!("{}/authorization?{}", api::HOST_CONSOLE, qs.join("&"))
}

/// 回调查询串里的 `authCodeInfo`。
#[derive(Debug, Clone, Deserialize)]
pub struct AuthCodeInfo {
    #[serde(rename = "AuthCode")]
    pub auth_code: String,
    /// 过期时间（毫秒时间戳）。
    #[serde(rename = "ExpireAt", default)]
    pub expire_at: Option<i64>,
    /// 有效期（毫秒）。
    #[serde(rename = "ExpireDuration", default)]
    pub expire_duration: Option<i64>,
}

/// 从回调查询串解析授权码。
fn callback_auth_code(query: &str) -> anyhow::Result<AuthCodeInfo> {
    let raw = login::query_param(query, "authCodeInfo")
        .ok_or_else(|| anyhow::anyhow!("回调缺少 authCodeInfo，原始查询串: {query}"))?;
    serde_json::from_str::<AuthCodeInfo>(&raw)
        .map_err(|e| anyhow::anyhow!("解析 authCodeInfo 失败: {e}; 原文: {raw}"))
}

#[derive(Debug, Serialize)]
struct ExchangeReq<'a> {
    #[serde(rename = "ClientID")]
    client_id: &'a str,
    #[serde(rename = "AuthCode")]
    auth_code: &'a str,
    #[serde(rename = "CodeVerifier")]
    code_verifier: &'a str,
    #[serde(rename = "DeviceInfo")]
    device_info: api::DeviceInfo,
    #[serde(rename = "IDEVersion")]
    ide_version: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeResult {
    #[serde(rename = "Token")]
    pub token: String,
    #[serde(rename = "RefreshToken")]
    pub refresh_token: String,
    #[serde(rename = "RefreshExpireAt", default)]
    pub refresh_expire_at: Option<i64>,
    #[serde(rename = "TokenExpireAt", default)]
    pub token_expire_at: Option<i64>,
    #[serde(rename = "BoundDeviceID", default)]
    pub bound_device_id: Option<String>,
    #[serde(rename = "DeviceBindStatus", default)]
    pub device_bind_status: Option<String>,
}

async fn exchange_token(
    client: &reqwest::Client,
    host: &str,
    device: &Device,
    auth_code: &str,
    verifier: &str,
) -> Result<ExchangeResult, HttpError> {
    let url = format!("{host}{EXCHANGE_PATH}");
    let body = ExchangeReq {
        client_id: api::CLIENT_ID,
        auth_code,
        code_verifier: verifier,
        device_info: api::DeviceInfo::build(device)?,
        ide_version: api::APP_VERSION,
    };
    api::post_control::<_, ExchangeResult>(client, &url, api::UA_ELECTRON, None, &body).await
}

fn fmt_ms(ms: Option<i64>) -> String {
    match ms {
        Some(ms) => {
            let secs = ms / 1000;
            match time::OffsetDateTime::from_unix_timestamp(secs) {
                Ok(dt) => {
                    let (y, mo, d) = dt.to_calendar_date();
                    let (h, mi, s) = (dt.hour(), dt.minute(), dt.second());
                    format!("{y:04}-{:02}-{d:02} {h:02}:{mi:02}:{s:02}", mo as u8)
                }
                Err(_) => ms.to_string(),
            }
        }
        None => "未知".to_string(),
    }
}

/// 保存路径（供提示信息复用）。
pub fn credential_path(root: &Path) -> PathBuf {
    crate::config::Config::credential_path(root, crate::providers::trae::ID)
}
