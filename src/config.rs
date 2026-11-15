//! 运行期配置：`运行目录/config.toml`。
//!
//! 文件不存在时使用默认值（`127.0.0.1:3060`），不会自动写盘。

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 配置文件名（相对运行目录）。
pub const CONFIG_FILE: &str = "config.toml";
/// 供应商数据目录名（相对运行目录）。
pub const PROVIDERS_DIR: &str = "providers";
/// 凭证文件名（位于 `providers/<id>/` 下）。
pub const CREDENTIAL_FILE: &str = "credential.toml";

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 3060;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 绑定 IP。
    pub host: IpAddr,
    /// 监听端口。
    pub port: u16,
    /// 自动签到的检查间隔（秒）。README 要求 `1h`。
    pub claim_interval_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.parse().expect("默认 host 合法"),
            port: DEFAULT_PORT,
            claim_interval_secs: 3600,
        }
    }
}

impl Config {
    /// 从 `<root>/config.toml` 读取；文件不存在返回默认值。
    pub fn load(root: &Path) -> Result<Self, ConfigError> {
        let path = root.join(CONFIG_FILE);
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(ConfigError::Io { path, source: e }),
        };
        toml::from_str(&text).map_err(|e| ConfigError::Parse { path, source: e })
    }

    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }

    /// 某供应商的数据目录 `<root>/providers/<id>`。
    pub fn provider_dir(root: &Path, provider: &str) -> PathBuf {
        root.join(PROVIDERS_DIR).join(provider)
    }

    /// 某供应商的凭证文件 `<root>/providers/<id>/credential.toml`。
    pub fn credential_path(root: &Path, provider: &str) -> PathBuf {
        Self::provider_dir(root, provider).join(CREDENTIAL_FILE)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("读取配置失败 {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("解析配置失败 {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}
