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
}

impl Credential {
    pub fn load(root: &Path) -> Result<Option<Self>, credential::CredentialError> {
        credential::load(root, PROVIDER)
    }

    pub fn store(&self, root: &Path) -> Result<(), credential::CredentialError> {
        credential::store(root, PROVIDER, self)
    }

    /// 由登录/续期响应构造凭证；设备字段来自 [`Device`]。
    pub fn from_auth(device: &Device, res: ExchangeResult, _prev: Option<&Self>) -> Self {
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
