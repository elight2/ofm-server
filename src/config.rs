//! 运行期配置：`运行目录/config.toml`。
//!
//! 文件不存在时使用默认值，**不会自动写盘**。默认值的唯一真相是仓库根目录的
//! `config.example.toml`（编译期内嵌），它同时充当用户配置模板：
//! `ofm-server config > config.toml`。
//!
//! 默认值不在代码里重复定义，避免与示例文件漂移。

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 配置文件名（相对运行目录）。
pub const CONFIG_FILE: &str = "config.toml";
/// 供应商数据目录名（相对运行目录）。
pub const PROVIDERS_DIR: &str = "providers";
/// 凭证文件名（位于 `providers/<id>/` 下）。
pub const CREDENTIAL_FILE: &str = "credential.toml";

/// 内嵌的默认配置 = 仓库根的 `config.example.toml`。
///
/// 它是**默认值的唯一真相**：`Config::default()` 直接解析这份文本，
/// 代码里不再重复写一份常量。改默认值只改这个文件。
pub const DEFAULT_CONFIG_STR: &str = include_str!("../config.example.toml");

/// 未在 `[claim_intervals]` 中列出的供应商所用的兜底签到间隔（秒）。
///
/// 取值为 6 小时，与 `config.example.toml` 的兜底说明保持一致。
/// 该常量**不重复**定义已知供应商的间隔——那些写在示例文件里。
pub const FALLBACK_CLAIM_INTERVAL_SECS: u64 = 6 * 3600;

/// 签到间隔下限（秒）。低于此值视为配置错误。
pub const MIN_CLAIM_INTERVAL_SECS: u64 = 60;

/// 字段级默认：分别从示例文件读取，**不走** `Config::default()`。
///
/// 不能用容器级 `#[serde(default)]`：那样缺字段会调 `Config::default()`，
/// 而后者又要解析本文件，形成无限递归（实测会栈溢出）。
fn default_host() -> IpAddr {
    example().host
}

fn default_port() -> u16 {
    example().port
}

fn default_claim_intervals() -> HashMap<String, u64> {
    example().claim_intervals.clone()
}

/// 解析示例文件。供 `Config::default()` 与字段级默认共用。
fn example() -> Config {
    toml::from_str(DEFAULT_CONFIG_STR).expect("config.example.toml 必须可解析")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// 绑定 IP。
    #[serde(default = "default_host")]
    pub host: IpAddr,
    /// 监听端口。
    #[serde(default = "default_port")]
    pub port: u16,
    /// 按供应商覆盖的签到间隔（秒）。
    ///
    /// 未列出的供应信用 `FALLBACK_CLAIM_INTERVAL_SECS`。各供应商**独立计时**。
    #[serde(default = "default_claim_intervals")]
    pub claim_intervals: HashMap<String, u64>,
}

impl Default for Config {
    /// 解析内嵌的 `config.example.toml`。
    ///
    /// 内嵌文本是编译期常量且有单测覆盖，若解析失败说明仓库里的示例文件
    /// 已损坏，属于开发期错误，panic 是合适的。
    fn default() -> Self {
        example()
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

    /// 某供应商的签到间隔（秒）。未配置则用兜底值。
    ///
    /// 值小于 `MIN_CLAIM_INTERVAL_SECS` 视为配置错误——不静默纠正，
    /// 避免用户写了 `10` 却被默默改成 60 而不自知。
    pub fn claim_interval_for(&self, provider: &str) -> Result<u64, ConfigError> {
        let secs = self
            .claim_intervals
            .get(provider)
            .copied()
            .unwrap_or(FALLBACK_CLAIM_INTERVAL_SECS);
        if secs < MIN_CLAIM_INTERVAL_SECS {
            return Err(ConfigError::IntervalTooSmall {
                provider: provider.to_string(),
                secs,
                min: MIN_CLAIM_INTERVAL_SECS,
            });
        }
        Ok(secs)
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

    /// 内嵌示例文件必须可解析——它是默认值的唯一真相。
    #[test]
    fn 示例文件可解析() {
        let c: Config = toml::from_str(DEFAULT_CONFIG_STR).expect("示例文件损坏");
        assert_eq!(c.port, 3060);
        assert_eq!(c.host.to_string(), "127.0.0.1");
    }

    /// `Config::default()` 就是解析示例文件的结果，二者不得有分歧。
    #[test]
    fn 默认值来自示例文件() {
        let from_str: Config = toml::from_str(DEFAULT_CONFIG_STR).unwrap();
        let from_default = Config::default();
        assert_eq!(from_default.host, from_str.host);
        assert_eq!(from_default.port, from_str.port);
        assert_eq!(from_default.claim_intervals, from_str.claim_intervals);
    }

    /// 示例文件里 trae=6h、raccoon=12h。
    #[test]
    fn 示例文件里的供应商间隔() {
        let c = Config::default();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 6 * 3600);
        assert_eq!(c.claim_interval_for("raccoon").unwrap(), 12 * 3600);
    }

    /// 示例文件未列出的供应商用兜底 6h。
    #[test]
    fn 未知供应商用兜底值() {
        let c = Config::default();
        assert_eq!(
            c.claim_interval_for("not-exist").unwrap(),
            FALLBACK_CLAIM_INTERVAL_SECS
        );
        assert_eq!(FALLBACK_CLAIM_INTERVAL_SECS, 21600);
    }

    /// 示例文件的注释里声明的兜底值与代码常量必须一致，防止只改一处。
    #[test]
    fn 注释里的兜底值与常量一致() {
        assert!(
            DEFAULT_CONFIG_STR.contains("21600"),
            "示例文件应写明 6h = 21600"
        );
    }

    /// 用户配置可覆盖、可部分覆盖。
    #[test]
    fn 部分覆盖() {
        let c = parse("[claim_intervals]\ntrae = 3600\n").unwrap();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 3600);
        // raccoon 未覆盖 → 兜底值，而非示例文件里的 12h
        assert_eq!(
            c.claim_interval_for("raccoon").unwrap(),
            FALLBACK_CLAIM_INTERVAL_SECS
        );
    }

    /// 间隔过小必须报错，不静默纠正。
    #[test]
    fn 间隔过小报错() {
        let c = parse("[claim_intervals]\ntrae = 10\n").unwrap();
        let err = c.claim_interval_for("trae").unwrap_err();
        assert!(err.to_string().contains("trae") && err.to_string().contains("10"));
    }

    #[test]
    fn 边界值六十秒可用() {
        let c = parse("[claim_intervals]\ntrae = 60\n").unwrap();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 60);
    }

    #[test]
    fn 与其它字段共存() {
        let c = parse("host = \"0.0.0.0\"\nport = 8080\n[claim_intervals]\nraccoon = 86400\n").unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.claim_interval_for("raccoon").unwrap(), 86400);
    }

    /// 旧字段已移除，应报错而不是被忽略。
    #[test]
    fn 旧字段被拒() {
        assert!(parse("claim_interval_secs = 3600\n").is_err());
    }

    /// 空配置用示例文件默认值。
    #[test]
    fn 空配置取默认() {
        let c = parse("").unwrap();
        assert_eq!(c.port, 3060);
        assert_eq!(c.claim_interval_for("trae").unwrap(), 6 * 3600);
    }
}
