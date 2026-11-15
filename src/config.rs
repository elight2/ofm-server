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
/// 签到默认间隔：6 小时（README 规定）。
/// 未在 `[claim_intervals]` 中配置的供应商都用这个值。
pub const DEFAULT_CLAIM_INTERVAL_SECS: u64 = 6 * 3600;
/// 签到间隔下限（秒）。低于此值视为配置错误。
pub const MIN_CLAIM_INTERVAL_SECS: u64 = 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// 绑定 IP。
    pub host: IpAddr,
    /// 监听端口。
    pub port: u16,
    /// 按供应商覆盖的签到间隔（秒）。
    ///
    /// 未列出的供应商用 `DEFAULT_CLAIM_INTERVAL_SECS`。
    /// 各供应商**独立计时**，互不影响。
    ///
    /// ```toml
    /// [claim_intervals]
    /// trae = 3600
    /// raccoon = 86400
    /// ```
    pub claim_intervals: std::collections::HashMap<String, u64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host: DEFAULT_HOST.parse().expect("默认 host 合法"),
            port: DEFAULT_PORT,
            claim_intervals: std::collections::HashMap::new(),
        }
    }
}

impl Config {
    /// 某供应商的签到间隔（秒）。未配置则用默认 6h。
    ///
    /// 值小于 `MIN_CLAIM_INTERVAL_SECS` 视为配置错误——不静默纠正，
    /// 避免用户写了 `10` 却被默默改成 60 而不自知。
    pub fn claim_interval_for(&self, provider: &str) -> Result<u64, ConfigError> {
        let secs = self
            .claim_intervals
            .get(provider)
            .copied()
            .unwrap_or(DEFAULT_CLAIM_INTERVAL_SECS);
        if secs < MIN_CLAIM_INTERVAL_SECS {
            return Err(ConfigError::IntervalTooSmall {
                provider: provider.to_string(),
                secs,
                min: MIN_CLAIM_INTERVAL_SECS,
            });
        }
        Ok(secs)
    }

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
    #[error("供应商 `{provider}` 的签到间隔 {secs}s 过小（最小 {min}s）")]
    IntervalTooSmall {
        provider: String,
        secs: u64,
        min: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Config, ConfigError> {
        toml::from_str::<Config>(text).map_err(|source| ConfigError::Parse {
            path: PathBuf::from("test"),
            source,
        })
    }

    /// 不写 `[claim_intervals]`：全部供应商用 6h。
    #[test]
    fn 未配置时默认六小时() {
        let c = parse("").unwrap();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 6 * 3600);
        assert_eq!(c.claim_interval_for("raccoon").unwrap(), 6 * 3600);
        assert_eq!(DEFAULT_CLAIM_INTERVAL_SECS, 21600);
    }

    /// 只配一个供应商，其余仍用默认。
    #[test]
    fn 部分覆盖() {
        let c = parse("[claim_intervals]\ntrae = 3600\n").unwrap();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 3600);
        assert_eq!(c.claim_interval_for("raccoon").unwrap(), 6 * 3600);
    }

    /// 不认识的供应商 id 也用默认，不报错（配置里多写几个无妨）。
    #[test]
    fn 未知供应商也用默认() {
        let c = parse("[claim_intervals]\ntrae = 60\n").unwrap();
        assert_eq!(c.claim_interval_for("not-exist").unwrap(), 6 * 3600);
    }

    /// 间隔过小必须报错，不静默纠正。
    #[test]
    fn 间隔过小报错() {
        let c = parse("[claim_intervals]\ntrae = 10\n").unwrap();
        let err = c.claim_interval_for("trae").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("trae") && msg.contains("10"), "{msg}");
    }

    #[test]
    fn 边界值六十秒可用() {
        let c = parse("[claim_intervals]\ntrae = 60\n").unwrap();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 60);
    }

    /// host/port 与签到间隔可以共存。
    #[test]
    fn 与其它字段共存() {
        let c = parse("host = \"0.0.0.0\"\nport = 8080\n[claim_intervals]\nraccoon = 86400\n").unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.claim_interval_for("raccoon").unwrap(), 86400);
        assert_eq!(c.claim_interval_for("trae").unwrap(), 6 * 3600);
    }

    /// 旧字段 `claim_interval_secs` 已移除，应报错而不是被忽略。
    #[test]
    fn 旧字段被拒() {
        assert!(parse("claim_interval_secs = 3600\n").is_err());
    }
}
