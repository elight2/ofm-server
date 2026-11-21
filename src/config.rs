//! 运行期配置：`运行目录/config.toml`。
//!
//! 文件不存在时使用默认值，**不会自动写盘**。默认值的唯一真相是仓库根目录的
//! `config.example.toml`（编译期内嵌），它同时充当用户配置模板：
//! `ofm-server config > config.toml`。
//!
//! 默认值不在代码里重复定义，避免与示例文件漂移。

use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

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
/// 每个内置供应商各有一份，互不影响；文件内容为空白表示「该供应商默认提示词
/// 为空」（不注入），是合法状态，不是漏写。未列出的供应商返回 `None`。
pub fn default_prompt(provider: &str) -> Option<&'static str> {
    let text = match provider {
        crate::providers::trae::ID => include_str!("../default-prompts/trae.md"),
        crate::providers::raccoon::ID => include_str!("../default-prompts/raccoon.md"),
        crate::providers::codearts::ID => include_str!("../default-prompts/codearts.md"),
        crate::providers::minimax::ID => include_str!("../default-prompts/minimax.md"),
        crate::providers::loomy::ID => include_str!("../default-prompts/loomy.md"),
        crate::providers::workbuddy::ID => include_str!("../default-prompts/workbuddy.md"),
        _ => return None,
    };
    Some(text)
}

/// 间隔类配置允许的下限（小时）。低于此值视为配置错误。
///
/// 单位是小时，故最小为 1h（不支持小数：签到/刷新都没有分钟级需求）。
pub const MIN_INTERVAL_HOURS: u64 = 1;

/// 间隔类配置允许的上限（小时，一年）。
///
/// 上限只为挡住溢出与明显的笔误（如把秒当小时写进 21600）；
/// 真要一年以上不签到，改代码比改配置更合适。
pub const MAX_INTERVAL_HOURS: u64 = 24 * 365;

/// 间隔类配置允许的下限（分钟）。低于此值视为配置错误。
///
/// 余额记录是分钟级任务（用户要「每过一段时间」记录），故下限比签到的小时级更细。
pub const MIN_BALANCE_INTERVAL_MINUTES: u64 = 1;

/// 余额记录间隔允许的上限（分钟，约一年）。
///
/// 与 [`MAX_INTERVAL_HOURS`] 同理：只为挡住溢出与明显的笔误。
pub const MAX_BALANCE_INTERVAL_MINUTES: u64 = 60 * 24 * 365;

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

fn default_claim_interval() -> u64 {
    example().claim_interval
}

fn default_refresh_interval() -> u64 {
    example().refresh_interval
}

fn default_balance_record_interval() -> u64 {
    example().balance_record_interval
}

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

/// 旧版配置里的「供应商设置」表名（每家的签到间隔曾写在各自表里）。
const LEGACY_PROVIDER_TABLES: &[&str] = &[
    crate::providers::trae::ID,
    crate::providers::raccoon::ID,
    crate::providers::codearts::ID,
    crate::providers::minimax::ID,
    crate::providers::loomy::ID,
    crate::providers::workbuddy::ID,
];

/// 去掉配置文本里的**遗留键**及其配套注释，让存量 `config.toml` 继续可用。
///
/// 两批遗留内容：
/// * `[<provider>]` 表及其中的 `claim_intervals`（秒）：签到间隔已改为全局
///   `claim_interval`（小时），旧写法不再有意义；
/// * `reasoning_effort`：思考强度已从所有供应商移除（不再向下游发送任何相关
///   字段，一律保持上游默认状态）。
///
/// 只删除**整行**，同时删掉**紧邻上方**的说明注释（否则会留下 `# 签到间隔(s)`、
/// `# low high` 这种悬空注释）。新配置里的全局 `claim_interval` **不**受影响
/// （键名不同，且精确匹配到 `=`）。
pub fn strip_legacy_keys(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<&str> = Vec::with_capacity(lines.len());
    for line in lines {
        if is_legacy_provider_table(line) {
            continue;
        }
        let is_legacy = is_key(line, "claim_intervals") || is_key(line, "reasoning_effort");
        if is_legacy {
            // 上一行若是「遗留键的说明注释」则一并删掉。
            if let Some(last) = out.last()
                && is_legacy_comment(last)
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

/// 该行是否是旧版的 `[<provider>]` 表头。
fn is_legacy_provider_table(line: &str) -> bool {
    let t = line.trim();
    let Some(inner) = t.strip_prefix('[').and_then(|x| x.strip_suffix(']')) else {
        return false;
    };
    LEGACY_PROVIDER_TABLES.contains(&inner.trim())
}

/// 该行是否是 `键 = 值`（键名精确匹配，避免把 `claim_interval` 当成 `claim_intervals`）。
fn is_key(line: &str, key: &str) -> bool {
    line.trim_start()
        .strip_prefix(key)
        .is_some_and(|rest| rest.trim_start().starts_with('='))
}

/// 该注释行是否是遗留键的说明。
///
/// 形如 `# low high`、`# 模型思考强度(trae的这个设置不生效)`、`# 签到间隔(s)`。
fn is_legacy_comment(line: &str) -> bool {
    let t = line.trim();
    let Some(body) = t.strip_prefix('#') else {
        return false;
    };
    let body = body.trim();
    if body.is_empty() {
        return false;
    }
    // 出现「思考 / 强度 / 档位 / effort / 签到」等字样即认为是遗留键的注释。
    if ["思考", "强度", "档位", "effort", "Effort", "签到"]
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
    /// 自动签到间隔（小时）。所有供应商共用这一份。
    #[serde(default = "default_claim_interval")]
    pub claim_interval: u64,
    /// 登录令牌自动刷新间隔（小时）。所有供应商共用这一份。
    #[serde(default = "default_refresh_interval")]
    pub refresh_interval: u64,
    /// 余额记录间隔（分钟）。所有供应商共用这一份。
    ///
    /// 定时任务与签到后都会按此节奏记录余额，供 `status` 展示最后一次余额增加。
    #[serde(default = "default_balance_record_interval")]
    pub balance_record_interval: u64,
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

    /// 自动签到间隔。全局一份，所有供应商共用。
    ///
    /// 单位小时；超出范围视为配置错误——不静默纠正，避免用户写了 `0`
    /// 却被默默改成下限而不自知。
    pub fn claim_interval(&self) -> Result<Duration, ConfigError> {
        interval("claim_interval", self.claim_interval)
    }

    /// 登录令牌自动刷新间隔。全局一份，所有供应商共用。
    ///
    /// 不支持自动刷新的供应商由 `serve` 自行跳过。
    pub fn refresh_interval(&self) -> Result<Duration, ConfigError> {
        interval("refresh_interval", self.refresh_interval)
    }

    /// 余额记录间隔。全局一份，所有供应商共用。
    ///
    /// 单位分钟；超出范围视为配置错误。
    pub fn balance_record_interval(&self) -> Result<Duration, ConfigError> {
        interval_minutes("balance_record_interval", self.balance_record_interval)
    }
}

/// 小时 → 时长的统一校验。
///
/// 两个间隔共用一套上下限，避免各写一份判断。
fn interval(key: &'static str, hours: u64) -> Result<Duration, ConfigError> {
    if !(MIN_INTERVAL_HOURS..=MAX_INTERVAL_HOURS).contains(&hours) {
        return Err(ConfigError::IntervalOutOfRange {
            key,
            hours,
            min: MIN_INTERVAL_HOURS,
            max: MAX_INTERVAL_HOURS,
        });
    }
    Ok(Duration::from_secs(hours * 3600))
}

/// 分钟 → 时长的统一校验。余额记录间隔专用。
fn interval_minutes(key: &'static str, minutes: u64) -> Result<Duration, ConfigError> {
    if !(MIN_BALANCE_INTERVAL_MINUTES..=MAX_BALANCE_INTERVAL_MINUTES).contains(&minutes) {
        return Err(ConfigError::IntervalOutOfRange {
            key,
            hours: minutes,
            min: MIN_BALANCE_INTERVAL_MINUTES,
            max: MAX_BALANCE_INTERVAL_MINUTES,
        });
    }
    Ok(Duration::from_secs(minutes * 60))
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
    #[error("配置项 `{key}` 的间隔 {hours}h 超出范围（允许 {min}h ~ {max}h）")]
    IntervalOutOfRange {
        key: &'static str,
        hours: u64,
        min: u64,
        max: u64,
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
        assert_eq!(c.claim_interval, 6);
        assert_eq!(c.refresh_interval, 36);
        assert_eq!(c.balance_record_interval, 30);
    }

    /// `Config::default()` 就是解析示例文件的结果，二者不得有分歧。
    #[test]
    fn 默认值来自示例文件() {
        let from_str: Config = parse_toml(DEFAULT_CONFIG_STR).unwrap();
        let from_default = Config::default();
        assert_eq!(from_default.host, from_str.host);
        assert_eq!(from_default.port, from_str.port);
        assert_eq!(from_default.claim_interval, from_str.claim_interval);
        assert_eq!(from_default.refresh_interval, from_str.refresh_interval);
        assert_eq!(
            from_default.balance_record_interval,
            from_str.balance_record_interval
        );
    }

    /// 旧版配置（每家一张表、秒为单位、带思考强度）必须继续可用。
    #[test]
    fn 旧版供应商表被忽略() {
        let c = parse(
            "[trae]\n# 签到间隔(s)\nclaim_intervals = 21600\n# low high\nreasoning_effort = \"low\"\n\n[raccoon]\nclaim_intervals = 43200\n",
        )
        .unwrap();
        // 间隔改由全局键决定，旧表里的值不再生效。
        assert_eq!(c.claim_interval, 6);
        assert_eq!(c.claim_interval().unwrap(), Duration::from_secs(6 * 3600));
        // 旧配置里的各种思考强度取值都能被静默丢弃
        for v in ["low", "high", "max", "none", "ultra"] {
            let t = format!("[minimax]\nclaim_intervals = 21600\nreasoning_effort = \"{v}\"\n");
            assert!(parse(&t).is_ok(), "遗留取值 {v} 应被忽略");
        }
    }

    /// 用户配置模板（`ofm-server config` 输出）不得含遗留键。
    #[test]
    fn 模板不含遗留键() {
        let t = template_str();
        assert!(!t.contains("reasoning_effort"), "模板仍含遗留键");
        assert!(!t.contains("claim_intervals"), "模板仍含遗留键");
        // 新键仍在
        assert!(t.contains("claim_interval = 6"));
        assert!(t.contains("refresh_interval = 36"));
        assert!(t.contains("balance_record_interval = 30"));
        // 模板本身可解析
        let c = parse(&t).unwrap();
        assert_eq!(c.port, 3060);
        assert_eq!(c.claim_interval, 6);
        assert_eq!(c.refresh_interval, 36);
        assert_eq!(c.balance_record_interval, 30);
    }

    /// 全局间隔与供应商无关：任何一个 id 都取同一个值。
    #[test]
    fn 全局间隔() {
        let c = parse("claim_interval = 12\nrefresh_interval = 48\n").unwrap();
        assert_eq!(c.claim_interval().unwrap(), Duration::from_secs(12 * 3600));
        assert_eq!(
            c.refresh_interval().unwrap(),
            Duration::from_secs(48 * 3600)
        );
    }

    /// 间隔过小必须报错，不静默纠正。
    #[test]
    fn 间隔过小报错() {
        let c = parse("claim_interval = 0\n").unwrap();
        let err = c.claim_interval().unwrap_err();
        assert!(err.to_string().contains("claim_interval") && err.to_string().contains('0'));
    }

    /// 间隔过大也要报错（多半是把秒当小时写了）。
    #[test]
    fn 间隔过大报错() {
        let c = parse("refresh_interval = 21600\n").unwrap();
        let err = c.refresh_interval().unwrap_err();
        assert!(err.to_string().contains("refresh_interval"));
    }

    /// 边界值可用。
    #[test]
    fn 边界值可用() {
        let c = parse("claim_interval = 1\nrefresh_interval = 8760\n").unwrap();
        assert_eq!(c.claim_interval().unwrap(), Duration::from_secs(3600));
        assert_eq!(
            c.refresh_interval().unwrap(),
            Duration::from_secs(8760 * 3600)
        );
    }

    /// 余额记录间隔以分钟计，单位与小时级间隔不同。
    #[test]
    fn 余额记录间隔以分钟计() {
        let c = parse("balance_record_interval = 30\n").unwrap();
        assert_eq!(
            c.balance_record_interval().unwrap(),
            Duration::from_secs(30 * 60)
        );
        // 默认 30 分钟
        assert_eq!(Config::default().balance_record_interval, 30);
    }

    /// 余额记录间隔过小必须报错，不静默纠正。
    #[test]
    fn 余额记录间隔过小报错() {
        let c = parse("balance_record_interval = 0\n").unwrap();
        let err = c.balance_record_interval().unwrap_err();
        assert!(
            err.to_string().contains("balance_record_interval") && err.to_string().contains('0')
        );
    }

    #[test]
    fn 与其它字段共存() {
        let c = parse("host = \"0.0.0.0\"\nport = 8080\nclaim_interval = 24\n").unwrap();
        assert_eq!(c.port, 8080);
        assert_eq!(c.host.to_string(), "0.0.0.0");
        assert_eq!(c.claim_interval().unwrap(), Duration::from_secs(24 * 3600));
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
        assert_eq!(c.claim_interval, 6);
        assert_eq!(c.refresh_interval, 36);
    }

    /// 写错供应商名应报错（不是被当遗留表静默接受）。
    #[test]
    fn 写错供应商名被拒() {
        assert!(parse("[racocon]\nclaim_intervals = 3600\n").is_err());
    }

    /// 键名写错（`claim_intervals` 而非 `claim_interval`）是全局未知键，应用默认值。
    ///
    /// 注：顶层 `claim_intervals = 60` 会被当作遗留键剥掉，故这里用带后缀的错键验证。
    #[test]
    fn 顶层遗留键剥掉后取默认() {
        let c = parse("claim_intervals = 60\n").unwrap();
        assert_eq!(c.claim_interval, 6);
    }
}
