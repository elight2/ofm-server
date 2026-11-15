//! Trae HTTP 层：端点、UA、控制面响应信封。
//!
//! 分两套 UA（`provider-info/trae.md` §0）：对话用 `TraeClient/TTNet`，
//! 控制面（登录/续期）用 Electron 桌面壳 UA，签到用 VSCode UA。

use serde::de::DeserializeOwned;
use serde::Deserialize;

pub const HOST_API5: &str = "https://api5-normal.mchost.guru";
pub const HOST_WEB: &str = "https://api.trae.cn";
pub const HOST_CONSOLE: &str = "https://www.trae.cn";
/// 图片/视频真实上传主机（抓包仅由 session_key 得知名，本实现不走该链路）。
pub const HOST_ICUBE: &str = "https://api.trae.com.cn";

pub const CLIENT_ID: &str = "ono9krqynydwx5";
pub const APP_VERSION: &str = "3.4.18";
pub const APP_VERSION_CODE: &str = "20261110";
pub const IDE_VERSION: &str = "1.107.1";
pub const IDE_VERSION_CODE: &str = "20261110";

pub const UA_TTNET: &str = "TraeClient/TTNet";
pub const UA_VSCODE: &str = "VSCode 1.107.1 (Trae CN)";
pub const UA_ELECTRON: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) TraeCN/1.107.1 Chrome/142.0.7444.235 Electron/39.2.7 Safari/537.36";

/// 控制面响应信封（与 `/api/agent/v3/*` 的 `{"code":…}` 不同）。
#[derive(Debug, Deserialize)]
pub struct ControlEnvelope<T> {
    #[serde(rename = "ResponseMetadata", default)]
    pub metadata: Metadata,
    #[serde(rename = "Result", default = "Option::default")]
    pub result: Option<T>,
    #[serde(default)]
    pub success: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Metadata {
    #[serde(default)]
    pub error: Option<ControlError>,
    #[serde(default)]
    pub trace_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ControlError {
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub standard_code: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl ControlError {
    pub fn describe(&self) -> String {
        match (&self.code, &self.message) {
            (Some(c), Some(m)) => format!("{c} {m}"),
            (None, Some(m)) => m.clone(),
            (Some(c), None) => c.clone(),
            (None, None) => "未知控制面错误".to_string(),
        }
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
    #[error("控制面错误: {0}")]
    Control(String),
}

/// 统一发请求并解 `ControlEnvelope`。
pub async fn post_control<B, R>(
    client: &reqwest::Client,
    url: &str,
    ua: &str,
    token: Option<&str>,
    body: &B,
) -> Result<R, HttpError>
where
    B: serde::Serialize + ?Sized,
    R: DeserializeOwned,
{
    let mut req = client
        .post(url)
        .header("user-agent", ua)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/plain, */*")
        .header("x-app-version", APP_VERSION)
        .header("x-app-version-code", APP_VERSION_CODE)
        .header("x-ide-version", IDE_VERSION)
        .header("x-ide-version-code", IDE_VERSION_CODE)
        .json(body);
    if let Some(t) = token {
        req = req.header("authorization", format!("Cloud-IDE-JWT {t}"));
    }

    let resp = req.send().await?;
    let status = resp.status().as_u16();
    let text = resp.text().await?;

    if !(200..300).contains(&status) {
        return Err(HttpError::Status { status, body: text });
    }

    let env: ControlEnvelope<R> = serde_json::from_str(&text)
        .map_err(|e| HttpError::Decode(format!("{e}; 原文={}", snippet(&text))))?;
    if let Some(err) = env.metadata.error {
        return Err(HttpError::Control(err.describe()));
    }
    match env.result {
        Some(r) => Ok(r),
        None => Err(HttpError::Decode(format!(
            "信封缺少 Result; 原文={}",
            snippet(&text)
        ))),
    }
}

fn snippet(s: &str) -> String {
    const MAX: usize = 300;
    if s.len() <= MAX {
        s.to_string()
    } else {
        let mut end = MAX;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

/// 设备信息（`DeviceInfo`），登录与续期共用；字段顺序与官方客户端一致。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DeviceInfo {
    #[serde(rename = "DeviceID")]
    pub device_id: String,
    #[serde(rename = "MachineID")]
    pub machine_id: String,
    #[serde(rename = "PlatformCode")]
    pub platform_code: &'static str,
    #[serde(rename = "DeviceType")]
    pub device_type: &'static str,
    #[serde(rename = "DeviceName")]
    pub device_name: String,
    #[serde(rename = "DeviceModel")]
    pub device_model: String,
    #[serde(rename = "ClientVersion")]
    pub client_version: &'static str,
    #[serde(rename = "DevicePublicKey")]
    pub device_public_key: String,
    #[serde(rename = "DeviceBrand")]
    pub device_brand: String,
    #[serde(rename = "DeviceCPU")]
    pub device_cpu: String,
    #[serde(rename = "OSInfo")]
    pub os_info: String,
    #[serde(rename = "OSVersion")]
    pub os_version: String,
}

impl DeviceInfo {
    pub fn build(device: &crate::providers::trae::device::Device) -> Result<Self, HttpError> {
        Ok(Self {
            device_id: device.device_id.clone(),
            machine_id: device.machine_id.clone(),
            platform_code: "IDE_PC",
            device_type: "PC",
            device_name: device.device_name.clone(),
            device_model: String::new(),
            client_version: APP_VERSION,
            device_public_key: device
                .public_key_pem()
                .map_err(|e| HttpError::Decode(format!("生成设备公钥失败: {e}")))?,
            device_brand: String::new(),
            device_cpu: device.device_cpu.clone(),
            os_info: os_name(),
            os_version: device.os_version.clone(),
        })
    }
}

/// `OSInfo`：官方取系统的 osName（Linux 即 `Linux`）。
pub fn os_name() -> String {
    std::env::consts::OS.to_string()
}

/// 签到等接口用的 `ReqSource`。
pub const REQ_SOURCE: &str = "IDE";

/// 共享的 HTTP 客户端（连接复用）。
///
/// 只设 `read_timeout`（两次数据到达的最大间隔）而非整体 `timeout`：
/// 对话是 SSE 长连接，长思考响应整体耗时可能远超 60s，用整体超时会中途断流。
pub fn client() -> Result<reqwest::Client, HttpError> {
    Ok(reqwest::Client::builder()
        .user_agent(UA_TTNET)
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(300))
        .build()?)
}
