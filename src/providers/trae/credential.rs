//! Trae 凭证结构（`providers/trae/credential.toml`）。
//!
//! 字段定义见 `provider-info/trae.md` §1。当前登录流程为占位实现，
//! 结构先固定下来，供 CLI 与后续协议实现共用。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::core::credential;

pub const PROVIDER: &str = "trae";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// Cloud-IDE JWT。
    pub token: String,
    pub refresh_token: String,
    #[serde(default)]
    pub refresh_expire_at: Option<String>,
    #[serde(default)]
    pub machine_id: Option<String>,
    #[serde(default)]
    pub device_id: Option<String>,
    /// EC prime256v1 私钥 PEM，须与已绑定公钥配对，不能重新生成。
    #[serde(default)]
    pub device_private_key_pem: Option<String>,
}

impl Credential {
    pub fn load(root: &Path) -> Result<Option<Self>, credential::CredentialError> {
        credential::load(root, PROVIDER)
    }

    pub fn store(&self, root: &Path) -> Result<(), credential::CredentialError> {
        credential::store(root, PROVIDER, self)
    }
}
