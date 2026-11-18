//! Loomy 凭证管理。
//!
//! 凭证文件格式（TOML）：
//! ```toml
//! token = "<access_token>"
//! base_url = "https://api.loomy.ai"
//! token_expire_at = 1700000000000
//! refresh_token = "<refresh_token>"
//! refresh_expire_at = 1700000000000
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::Config;

/// 凭证。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Credential {
    /// 访问令牌。
    pub token: String,
    /// API 基础 URL。
    pub base_url: String,
    /// token 到期时间（毫秒时间戳）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_expire_at: Option<i64>,
    /// 刷新令牌。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// refresh_token 到期时间（毫秒时间戳）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_expire_at: Option<i64>,
}

impl Credential {
    /// 从运行目录加载凭证。
    pub fn load(root: &Path) -> Result<Option<Self>, anyhow::Error> {
        let path = Config::credential_path(root, "loomy");
        if !path.exists() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)
            .with_context(|| format!("读取凭证失败: {}", path.display()))?;
        let cred: Credential = toml::from_str(&text)
            .with_context(|| format!("解析凭证失败: {}", path.display()))?;
        Ok(Some(cred))
    }

    /// 落盘。
    pub fn store(&self, root: &Path) -> Result<(), anyhow::Error> {
        let dir = Config::provider_dir(root, "loomy");
        fs::create_dir_all(&dir)
            .with_context(|| format!("创建数据目录失败: {}", dir.display()))?;
        let path = dir.join("credential.toml");
        let text = toml::to_string_pretty(self)
            .with_context(|| format!("序列化凭证失败"))?;
        fs::write(&path, text)
            .with_context(|| format!("写入凭证失败: {}", path.display()))?;
        Ok(())
    }
}
