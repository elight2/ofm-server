//! Trae 凭证（`providers/trae/credential.toml`）。
//!
//! 字段见 `provider-info/trae.md` §1。设备相关字段（`machine_id` / `device_id` /
//! `device_private_key_pem`）必须与已绑定的公钥配对，续期时原样使用。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::credential;
use crate::providers::trae::auth::ExchangeResult;
use crate::providers::trae::device::Device;

pub const PROVIDER: &str = "trae";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// Cloud-IDE JWT。
    pub token: String,
    pub refresh_token: String,
    /// 毫秒时间戳（RFC 3339 也接受，读时统一解析）。
    #[serde(default)]
    pub token_expire_at: Option<i64>,
    #[serde(default)]
    pub refresh_expire_at: Option<i64>,
    pub machine_id: String,
    pub device_id: String,
    pub device_private_key_pem: String,
    #[serde(default)]
    pub bound_device_id: Option<String>,
    #[serde(default)]
    pub device_bind_status: Option<String>,
    /// 签到专用设备号覆盖值（`provider-info/trae.md` §6.1）。
    ///
    /// 签到后台按 `x-device-id` 做风控，只认官方客户端注册过的 aha 设备号。
    /// 留空时自动采用本机官方客户端的 did（见 [`super::client_device`]），
    /// 再取不到才回退到 [`Self::device_id`]。手工填写可覆盖自动探测。
    #[serde(default)]
    pub checkin_device_id: Option<String>,
}

impl Credential {
    pub fn load(root: &Path) -> Result<Option<Self>, credential::CredentialError> {
        credential::load(root, PROVIDER)
    }

    pub fn store(&self, root: &Path) -> Result<(), credential::CredentialError> {
        credential::store(root, PROVIDER, self)
    }

    /// 由登录/续期响应构造凭证；设备字段来自 [`Device`]。
    pub fn from_auth(device: &Device, res: ExchangeResult, prev: Option<&Self>) -> Self {
        Self {
            token: res.token,
            refresh_token: res.refresh_token,
            token_expire_at: res.token_expire_at,
            refresh_expire_at: res.refresh_expire_at,
            machine_id: device.machine_id.clone(),
            device_id: device.device_id.clone(),
            device_private_key_pem: device.device_private_key_pem.clone(),
            bound_device_id: res.bound_device_id,
            device_bind_status: res.device_bind_status,
            checkin_device_id: prev.and_then(|p| p.checkin_device_id.clone()),
        }
    }

    /// 签到请求要带的 `x-device-id`。
    ///
    /// 优先级：显式覆盖值 → 本机官方客户端的 aha 设备号 → 本实现登录时上报的
    /// `device_id`。理由见 [`super::client_device`]。
    pub fn checkin_device_id(&self) -> String {
        if let Some(v) = self.checkin_device_id.as_deref()
            && !v.is_empty()
        {
            return v.to_string();
        }
        match super::client_device::device_id() {
            Some(v) => v.to_string(),
            None => self.device_id.clone(),
        }
    }

    pub fn device(&self) -> Device {
        Device {
            machine_id: self.machine_id.clone(),
            device_id: self.device_id.clone(),
            device_private_key_pem: self.device_private_key_pem.clone(),
            device_name: String::new(),
            os_version: String::new(),
            device_cpu: String::new(),
        }
    }
}
