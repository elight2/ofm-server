//! 运行期配置：`运行目录/config.toml`。
//!
//! 文件不存在时使用默认值，**不会自动写盘**。默认值的唯一真相是仓库根目录的
//! `config.example.toml`（编译期内嵌），它同时充当用户配置模板：
//! `ofm-server config > config.toml`。
//!
//! 默认值不在代码里重复定义，避免与示例文件漂移。

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 配置文件名（相对运行目录）。
pub const CONFIG_FILE: &str = "config.toml";

/// 内嵌的默认配置 = 仓库根的 `config.example.toml`。
///
/// 它是**默认值的唯一真相**：`Config::default()` 直接解析这份文本，
/// 代码里不再重复写一份常量。改默认值只改这个文件。
pub const DEFAULT_CONFIG_STR: &str = include_str!("../config.example.toml");

/// 某个内置供应商的内嵌默认系统提示词 = 仓库 `default-prompts/<供应商id>.md`。
///
/// 同 [`DEFAULT_CONFIG_STR`]：文件是**默认提示词的唯一真相**，运行目录下没有
/// `<root>/prompts/<供应商id>.md` 时用它，代码里不重复写一份常量。
/// 每个供应商各有一份，互不影响；未列出的供应商没有默认提示词（`None`，即不注入）。
pub fn default_prompt(provider: &str) -> Option<&'static str> {
    let text = match provider {
        crate::providers::trae::ID => include_str!("../default-prompts/trae.md"),
        crate::providers::raccoon::ID => include_str!("../default-prompts/raccoon.md"),
        crate::providers::codearts::ID => include_str!("../default-prompts/codearts.md"),
        crate::providers::minimax::ID => include_str!("../default-prompts/minimax.md"),
        crate::providers::loomy::ID => include_str!("../default-prompts/loomy.md"),
        _ => return None,
    };
    Some(text)
}

/// 未在 `[<provider>]` 表中写出 `claim_intervals` 时的兜底签到间隔（秒）。
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

fn default_trae() -> ProviderConfig {
    example().trae
}

fn default_raccoon() -> ProviderConfig {
    example().raccoon
}

fn default_codearts() -> ProviderConfig {
    example().codearts
}

fn default_minimax() -> ProviderConfig {
    example().minimax
}

fn default_loomy() -> ProviderConfig {
    example().loomy
}

fn default_workbuddy() -> ProviderConfig {
    // workbuddy 段尚未进 `config.example.toml`（该文件不在本阶段的可改范围内），
    // 故这里直接给默认值，**不能**走 `example()`：
    // `example()` 会解析整份配置并触发各字段的默认值函数，
    // 而本函数正是其中之一，会造成无限递归。
    ProviderConfig {
        claim_intervals: DEFAULT_CLAIM_INTERVAL_SECS,
    }
}

/// 自动签到默认间隔（秒）。
///
/// 供不能走 `example()` 的默认值函数使用（见 [`default_workbuddy`]）。
const DEFAULT_CLAIM_INTERVAL_SECS: u64 = 21600;

/// 用户配置模板 = 内嵌示例文件去掉遗留键后的文本。
///
/// `ofm-server config` 输出它作为用户模板。
pub fn template_str() -> String {
    strip_legacy_keys(DEFAULT_CONFIG_STR)
}

/// 解析示例文件。供 `Config::default()` 与字段级默认共用。
fn example() -> Config {
    parse_toml(DEFAULT_CONFIG_STR).expect("config.example.toml 必须可解析")
}

/// 去掉配置文本里的**遗留键** `reasoning_effort` 及其配套注释。
///
/// 思考强度已从所有供应商移除（不再向下游发送任何相关字段，
/// 一律保持上游默认状态），但旧版 `config.example.toml` 与用户
/// 存量 `config.toml` 里仍写着该键。为让这些文件继续可用，
/// 解析前把它们删掉。
///
/// 只删除**整行**形如 `reasoning_effort = ...` 的键；同时删掉它**紧邻上方**
/// 的档位注释行（否则会留下 `# low high` 这种悬空注释）。
pub fn strip_legacy_keys(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for line in lines {
        let trimmed = line.trim_start();
        let is_legacy = trimmed
            .strip_prefix("reasoning_effort")
            .is_some_and(|rest| rest.trim_start().starts_with('='));
        if is_legacy {
            // 上一行若是「档位注释」则一并删掉。
            if let Some(last) = out.last()
                && is_effort_comment(last)
            {
                out.pop();
            }
            continue;
        }
        out.push(line);
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

/// 该注释行是否是「思考档位」的说明。
///
/// 形如 `# low high`、`# 模型思考强度(trae的这个设置不生效)`。
fn is_effort_comment(line: &str) -> bool {
    let t = line.trim();
    let Some(body) = t.strip_prefix('#') else {
        return false;
    };
    let body = body.trim();
    if body.is_empty() {
        return false;
    }
    // 出现「思考 / 强度 / 档位 / effort」等字样即认为是档位注释。
    if ["思考", "强度", "档位", "effort", "Effort"]
        .iter()
        .any(|k| body.contains(k))
    {
        return true;
    }
    // 或者整行只是若干个档位名。
    body.split_whitespace().all(|w| {
        [
            "none", "low", "medium", "high", "xhigh", "max", "minimal", "ultra",
        ]
        .contains(&w.to_ascii_lowercase().as_str())
    })
}

/// 解析配置文本（自动去掉遗留键）。
fn parse_toml(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(&strip_legacy_keys(text))
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
    /// trae 供应商设置。
    #[serde(default = "default_trae")]
    pub trae: ProviderConfig,
    /// raccoon 供应商设置。
    #[serde(default = "default_raccoon")]
    pub raccoon: ProviderConfig,
    /// codearts 供应商设置。
    #[serde(default = "default_codearts")]
    pub codearts: ProviderConfig,
    /// minimax 供应商设置。
    #[serde(default = "default_minimax")]
    pub minimax: ProviderConfig,
    /// loomy 供应商设置。
    #[serde(default = "default_loomy")]
    pub loomy: ProviderConfig,
    /// workbuddy 供应商设置。
    #[serde(default = "default_workbuddy")]
    pub workbuddy: ProviderConfig,
}

/// 字段级默认：签到间隔（秒）。
///
/// 取兜底值，不走 `Config::default()`（会递归）。
fn default_claim_interval_secs() -> u64 {
    FALLBACK_CLAIM_INTERVAL_SECS
}

/// 供应商设置。
///
/// 每家只有一个字段（签到间隔），故共用一个类型。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    /// 自动签到间隔（秒）。
    #[serde(default = "default_claim_interval_secs")]
    pub claim_intervals: u64,
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
        parse_toml(&text).map_err(|e| ConfigError::Parse { path, source: e })
    }

    pub fn socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.host, self.port)
    }

    /// 某供应商的签到间隔（秒）。
    ///
    /// 三家的设置类型不同（档位各异），但都有 `claim_intervals`，故在此处
    /// 分支取值。未知供应商用兜底值。
    ///
    /// 值小于 `MIN_CLAIM_INTERVAL_SECS` 视为配置错误——不静默纠正，
    /// 避免用户写了 `10` 却被默默改成 60 而不自知。
    pub fn claim_interval_for(&self, provider: &str) -> Result<u64, ConfigError> {
        let secs = match provider {
            crate::providers::trae::ID => self.trae.claim_intervals,
            crate::providers::raccoon::ID => self.raccoon.claim_intervals,
            crate::providers::codearts::ID => self.codearts.claim_intervals,
            crate::providers::minimax::ID => self.minimax.claim_intervals,
            crate::providers::loomy::ID => self.loomy.claim_intervals,
            crate::providers::workbuddy::ID => self.workbuddy.claim_intervals,
            _ => FALLBACK_CLAIM_INTERVAL_SECS,
        };
        if secs < MIN_CLAIM_INTERVAL_SECS {
            return Err(ConfigError::IntervalTooSmall {
                provider: provider.to_string(),
                secs,
                min: MIN_CLAIM_INTERVAL_SECS,
            });
        }
        Ok(secs)
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
        parse_toml(text).map_err(|source| ConfigError::Parse {
            path: PathBuf::from("test"),
            source,
        })
    }

    /// 内嵌示例文件必须可解析——它是默认值的唯一真相。
    #[test]
    fn 示例文件可解析() {
        let c: Config = parse_toml(DEFAULT_CONFIG_STR).expect("示例文件损坏");
        assert_eq!(c.port, 3060);
        assert_eq!(c.host.to_string(), "127.0.0.1");
    }

    /// `Config::default()` 就是解析示例文件的结果，二者不得有分歧。
    #[test]
    fn 默认值来自示例文件() {
        let from_str: Config = parse_toml(DEFAULT_CONFIG_STR).unwrap();
        let from_default = Config::default();
        assert_eq!(from_default.host, from_str.host);
        assert_eq!(from_default.port, from_str.port);
        assert_eq!(from_default.trae, from_str.trae);
        assert_eq!(from_default.raccoon, from_str.raccoon);
        assert_eq!(from_default.codearts, from_str.codearts);
        assert_eq!(from_default.minimax, from_str.minimax);
    }

    /// 遗留键 `reasoning_effort` 必须被忽略（思考强度已移除）。
    #[test]
    fn 遗留思考强度键被忽略() {
        let c = parse("[trae]\nreasoning_effort = \"low\"\n").unwrap();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 6 * 3600);
        // 旧配置里的各种取值都能被静默丢弃
        for v in ["low", "high", "max", "none", "ultra"] {
            let t = format!("[minimax]\nreasoning_effort = \"{v}\"\n");
            assert!(parse(&t).is_ok(), "遗留取值 {v} 应被忽略");
        }
    }

    /// 用户配置模板（`ofm-server config` 输出）不得含遗留键。
    #[test]
    fn 模板不含遗留键() {
        let t = template_str();
        assert!(!t.contains("reasoning_effort"), "模板仍含遗留键");
        // 模板本身可解析
        let c = parse(&t).unwrap();
        assert_eq!(c.port, 3060);
        assert_eq!(c.claim_interval_for("trae").unwrap(), 6 * 3600);
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

    /// 兜底值与示例文件里最短的显式间隔一致，避免只改一处。
    #[test]
    fn 注释里的兜底值与常量一致() {
        assert!(
            DEFAULT_CONFIG_STR.contains("21600"),
            "示例文件应写明 6h = 21600"
        );
    }

    /// 用户配置可覆盖单个供应商，未写的供应商用**该供应商自己的**示例值。
    #[test]
    fn 部分覆盖() {
        let c = parse("[trae]\nclaim_intervals = 3600\n").unwrap();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 3600);
        // raccoon 未写 → 取其自己的默认值（示例文件的 12h），而不是兜底值
        assert_eq!(c.claim_interval_for("raccoon").unwrap(), 12 * 3600);
    }

    /// 间隔过小必须报错，不静默纠正。
    #[test]
    fn 间隔过小报错() {
        let c = parse("[trae]\nclaim_intervals = 10\n").unwrap();
        let err = c.claim_interval_for("trae").unwrap_err();
        assert!(err.to_string().contains("trae") && err.to_string().contains("10"));
    }

    #[test]
    fn 边界值六十秒可用() {
        let c = parse("[trae]\nclaim_intervals = 60\n").unwrap();
        assert_eq!(c.claim_interval_for("trae").unwrap(), 60);
    }

    #[test]
    fn 与其它字段共存() {
        let c =
            parse("host = \"0.0.0.0\"\nport = 8080\n[raccoon]\nclaim_intervals = 86400\n").unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.claim_interval_for("raccoon").unwrap(), 86400);
    }

    /// 旧的扁平 `[claim_intervals]` 表已移除，应报错而不是被忽略。
    #[test]
    fn 旧扁平表被拒() {
        assert!(parse("[claim_intervals]\ntrae = 3600\n").is_err());
    }

    /// 空配置用示例文件默认值。
    #[test]
    fn 空配置取默认() {
        let c = parse("").unwrap();
        assert_eq!(c.port, 3060);
        assert_eq!(c.claim_interval_for("trae").unwrap(), 6 * 3600);
    }

    /// 写错供应商名应报错（不是被当未知供应商静默接受）。
    #[test]
    fn 写错供应商名被拒() {
        assert!(parse("[racocon]\nclaim_intervals = 3600\n").is_err());
    }
}
