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

fn default_trae() -> TraeConfig {
    example().trae
}

fn default_raccoon() -> RaccoonConfig {
    example().raccoon
}

fn default_codearts() -> CodeartsConfig {
    example().codearts
}

fn default_minimax() -> MinimaxConfig {
    example().minimax
}

fn default_loomy() -> LoomyConfig {
    example().loomy
}

fn default_workbuddy() -> WorkBuddyConfig {
    // workbuddy 段尚未进 `config.example.toml`（该文件不在本阶段的可改范围内），
    // 故这里直接给默认值，**不能**走 `example()`：
    // `example()` 会解析整份配置并触发各字段的默认值函数，
    // 而本函数正是其中之一，会造成无限递归。
    WorkBuddyConfig {
        claim_intervals: DEFAULT_CLAIM_INTERVAL_SECS,
        reasoning_effort: WorkBuddyReasoningEffort::default(),
    }
}

/// 自动签到默认间隔（秒）。
///
/// 供不能走 `example()` 的默认值函数使用（见 [`default_workbuddy`]）。
const DEFAULT_CLAIM_INTERVAL_SECS: u64 = 21600;

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
    /// trae 供应商设置。
    #[serde(default = "default_trae")]
    pub trae: TraeConfig,
    /// raccoon 供应商设置。
    #[serde(default = "default_raccoon")]
    pub raccoon: RaccoonConfig,
    /// codearts 供应商设置。
    #[serde(default = "default_codearts")]
    pub codearts: CodeartsConfig,
    /// minimax 供应商设置。
    #[serde(default = "default_minimax")]
    pub minimax: MinimaxConfig,
    /// loomy 供应商设置。
    #[serde(default = "default_loomy")]
    pub loomy: LoomyConfig,
    /// workbuddy 供应商设置。
    #[serde(default = "default_workbuddy")]
    pub workbuddy: WorkBuddyConfig,
}

/// trae 供应商设置。
///
/// 每个供应商单独一个类型而非共用的 `ProviderConfig`：各家的思考强度
/// **可用档位不同**，共用类型要么得允许所有档位（失去校验），
/// 要么得在运行期补校验（错误发现得晚）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraeConfig {
    /// 自动签到间隔（秒）。
    #[serde(default = "default_claim_interval_secs")]
    pub claim_intervals: u64,
    /// 思考强度。trae 上游不支持该功能，只接受 `none`。
    #[serde(default)]
    pub reasoning_effort: TraeReasoningEffort,
}

/// raccoon 供应商设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RaccoonConfig {
    /// 自动签到间隔（秒）。
    #[serde(default = "default_claim_interval_secs")]
    pub claim_intervals: u64,
    /// 思考强度。
    #[serde(default)]
    pub reasoning_effort: RaccoonReasoningEffort,
}

/// codearts 供应商设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodeartsConfig {
    /// 自动签到间隔（秒）。
    #[serde(default = "default_claim_interval_secs")]
    pub claim_intervals: u64,
    /// 思考强度。
    #[serde(default)]
    pub reasoning_effort: CodeartsReasoningEffort,
}

/// minmax 供应商设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MinimaxConfig {
    /// 自动签到间隔（秒）。
    ///
    /// ⚠︎ 签到端点目前未知（`provider-info/minimax.md` §7：官方客户端无签到功能），
    /// 故该值暂时无效果，保留以保持配置形状一致。
    #[serde(default = "default_claim_interval_secs")]
    pub claim_intervals: u64,
    /// 思考强度。
    #[serde(default)]
    pub reasoning_effort: MinimaxReasoningEffort,
}

/// loomy 供应商设置。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoomyConfig {
    /// 自动签到间隔（秒）。
    #[serde(default = "default_claim_interval_secs")]
    pub claim_intervals: u64,
    /// 思考强度。
    #[serde(default)]
    pub reasoning_effort: LoomyReasoningEffort,
}

/// minimax 的思考强度。
///
/// 配置文件只接受 `low`/`medium`/`high`/`xhigh`/`max` 五档，实现按「就近映射」
/// 落到上游（`provider-info/minimax.md` §5.2/§5.4）：
///
/// * `MiniMax-M3`（`thinking_config.mode = switchable`）：上游只有开/关二值，
///   五档**全部映射为开思考**（`thinking.type=adaptive`），档位在 M3 上不生效，
///   属上游能力限制；
/// * `MiniMax-M3.1-Flash`（`forced_on`）：档位就近映射到目录下发的
///   `effort_options`（当前为 `default/low/medium/high/xhigh/max`）。
///
/// 就近映射的细节见 [`Self::nearest_effort`]。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MinimaxReasoningEffort {
    /// 低。
    #[default]
    Low,
    /// 中。
    Medium,
    /// 高。
    High,
    /// 极高。
    Xhigh,
    /// 最大。
    Max,
}

/// workbuddy 供应商设置。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkBuddyConfig {
    /// 自动签到间隔（秒）。
    #[serde(default = "default_claim_interval_secs")]
    pub claim_intervals: u64,
    /// 思考强度。
    #[serde(default)]
    pub reasoning_effort: WorkBuddyReasoningEffort,
}

/// workbuddy 的思考强度。
///
/// 上游 `reasoning_effort` 的请求层合法集合是
/// `minimal/low/medium/high/xhigh/max`（**不含** `off`/`none`），
/// 见 `provider-info/workbuddy.md` §5。实际发给上游的档位还会按
/// 该模型 `reasoning.supportedEfforts` 向下钳制。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkBuddyReasoningEffort {
    /// 关闭思考（上游取值 `none`）。
    ///
    /// 只有模型声明 `reasoning.canDisableThinking=true` 时才生效，
    /// 否则该档位被丢弃（不发 `reasoning_effort`）。
    None,
    /// 最低。
    Minimal,
    /// 低。
    Low,
    /// 中。
    #[default]
    Medium,
    /// 高。
    High,
    /// 极高。
    Xhigh,
    /// 最大。
    Max,
}

impl WorkBuddyReasoningEffort {
    /// 转成上游请求层使用的档位。
    pub fn as_effort(self) -> crate::providers::workbuddy::chat::Effort {
        use crate::providers::workbuddy::chat::Effort;
        match self {
            Self::None => Effort::None,
            Self::Minimal => Effort::Minimal,
            Self::Low => Effort::Low,
            Self::Medium => Effort::Medium,
            Self::High => Effort::High,
            Self::Xhigh => Effort::Xhigh,
            Self::Max => Effort::Max,
        }
    }
}

/// loomy 的思考强度。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoomyReasoningEffort {
    /// 低。
    #[default]
    Low,
    /// 中。
    Medium,
    /// 高。
    High,
}

impl MinimaxReasoningEffort {
    /// 五档全表，按强度升序。
    ///
    /// 上游档位名以目录 `effort_options` 为准，本常量只用于配置校验与测试。
    #[cfg(test)]
    pub const ALL: [Self; 5] = [Self::Low, Self::Medium, Self::High, Self::Xhigh, Self::Max];

    /// 档位在本档位在五档中的序号（`0`~`4`）。
    ///
    /// 就近映射的距离计算基准。
    pub fn rank(self) -> u8 {
        self as u8
    }

    /// 本档位在**上游**档位名里的「标准名」（小写）。
    ///
    /// 注意：这只是本档位的期望名，实际发给上游的值由
    /// [`Self::nearest_effort`] 在目录 `effort_options` 里就近挑选，
    /// 因目录档位集合**禁止硬编码**（§5.2）。
    pub fn effort_value(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// 就近映射：在本档位与上游目录档位表之间取最接近的一个。
    ///
    /// 上游档位集合由目录下发的 `effort_options` 决定（当前为
    /// `default/low/medium/high/xhigh/max`），**禁止硬编码**（§5.2）。
    ///
    /// 规则：
    /// 1. 上游表里有同名档位（`low`→`low`）→ 直接用它，这是绝大多数情况；
    /// 2. 否则按「五档序号距离」挑最近的，**距离相同取较低档**（更省 token）；
    /// 3. 上游表里没有任何可识别档位名时，回退到本档位的标准名。
    ///
    /// `default` 是客户端 UI 的「默认」，**不参与**距离计算——它表示
    /// 「让上游自己决定」，与本档位的具体档位不是同维度（§5.2）。
    pub fn nearest_effort(self, upstream: &[String]) -> &'static str {
        let own = self.effort_value();
        // 1. 同名直接命中
        if upstream.iter().any(|s| s == own) {
            return own;
        }
        // 2. 按序号距离就近挑；`(距离, 序号)` 取最小 → 距离相同自然落在较低档
        let want = self.rank();
        let best = upstream
            .iter()
            .filter_map(|name| rank_of_upstream(name).map(|rank| (rank.abs_diff(want), rank)))
            .min();
        if let Some((_, rank)) = best {
            return Self::name_of_rank(rank);
        }
        // 3. 上游表里没有一个可识别的档位名
        own
    }

    /// 序号 → 上游标准档位名。
    pub fn name_of_rank(rank: u8) -> &'static str {
        match rank {
            0 => "low",
            1 => "medium",
            2 => "high",
            3 => "xhigh",
            _ => "max",
        }
    }
}

/// 上游档位名 → 五档序号。
///
/// `default` 与未知名返回 `None`（不参与距离计算，§5.2）。
fn rank_of_upstream(name: &str) -> Option<u8> {
    match name {
        "low" => Some(0),
        "medium" => Some(1),
        "high" => Some(2),
        "xhigh" => Some(3),
        "max" => Some(4),
        _ => None,
    }
}

fn default_claim_interval_secs() -> u64 {
    example().trae.claim_intervals
}

/// trae 的思考强度。
///
/// trae 上游**不认**任何思考强度字段（实测 `reasoning_effort` 被静默忽略，
/// 见 `provider-info/trae.md` §3.2）。因此只有一个档位，仅为配置对称。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TraeReasoningEffort {
    /// 不发送任何思考强度字段。
    #[default]
    None,
}

/// raccoon 的思考强度。
///
/// 对应官方客户端「快速 / 深度思考」开关的两档：
/// - `low`：向上游发 `reasoning_effort: "low"`，实测思考被完全抑制。
/// - `high`：**不发送**该字段，跟随上游默认思考行为。实测发 `high`
///   反而比不发时思考更少，故「深度思考」用不发送实现。
///
/// 实测数据见 `provider-info/raccoon.md` §3.7。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RaccoonReasoningEffort {
    /// 快速：思考被抑制。
    #[default]
    Low,
    /// 深度思考：不发送该字段。
    High,
}

/// codearts 的思考强度。
///
/// ⚠︎ 暂未实测：档位与行为先假定与 raccoon 一致，待实测后修正。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodeartsReasoningEffort {
    /// 快速：预期思考被抑制。
    #[default]
    Low,
    /// 深度思考：预期跟随上游默认。
    High,
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
        assert_eq!(from_default.trae, from_str.trae);
        assert_eq!(from_default.raccoon, from_str.raccoon);
        assert_eq!(from_default.codearts, from_str.codearts);
        assert_eq!(from_default.minimax, from_str.minimax);
    }

    /// 五档全表按强度升序，`rank()` 与下标一致（就近映射的距离基准）。
    #[test]
    fn minimax_五档序号() {
        use MinimaxReasoningEffort as E;
        assert_eq!(E::ALL.len(), 5);
        for (i, e) in E::ALL.iter().enumerate() {
            assert_eq!(e.rank(), i as u8, "{e:?} 序号错");
            assert_eq!(E::name_of_rank(e.rank()), e.effort_value());
        }
    }

    /// 档位名与 §5.4 / 目录 `effort_options` 对齐（小写往返）。
    #[test]
    fn minimax_档位名与目录一致() {
        #[derive(serde::Deserialize)]
        struct W {
            v: MinimaxReasoningEffort,
        }
        let c = Config::default();
        assert_eq!(c.minimax.reasoning_effort, MinimaxReasoningEffort::Low);
        for want in MinimaxReasoningEffort::ALL {
            let s = want.effort_value();
            let got: W = toml::from_str(&format!("v = \"{s}\"")).unwrap();
            assert_eq!(got.v, want, "档位 {s} 解析错");
            assert_eq!(serde_json::to_string(&want).unwrap(), format!("\"{s}\""));
        }
    }

    /// 就近映射：上游档位表齐全时每档命中同名。
    #[test]
    fn minimax_就近映射同名命中() {
        use MinimaxReasoningEffort as E;
        let full: Vec<String> = ["default", "low", "medium", "high", "xhigh", "max"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for e in E::ALL {
            assert_eq!(e.nearest_effort(&full), e.effort_value());
        }
    }

    /// 就近映射：上游缺档时取最近；距离相同取较低档。
    #[test]
    fn minimax_就近映射缺档() {
        use MinimaxReasoningEffort as E;
        let opts: Vec<String> = ["low", "high"].iter().map(|s| s.to_string()).collect();
        assert_eq!(E::Low.nearest_effort(&opts), "low");
        // medium 到 low/high 距离都是 1 → 取较低档 low
        assert_eq!(E::Medium.nearest_effort(&opts), "low");
        assert_eq!(E::High.nearest_effort(&opts), "high");
        // xhigh/max 到 high 更近
        assert_eq!(E::Xhigh.nearest_effort(&opts), "high");
        assert_eq!(E::Max.nearest_effort(&opts), "high");

        // 只有 low 一档时，全部落到 low
        let only_low: Vec<String> = ["low"].iter().map(|s| s.to_string()).collect();
        for e in E::ALL {
            assert_eq!(e.nearest_effort(&only_low), "low", "{e:?} 应落到唯一档位");
        }
    }

    /// 就近映射：上游档位表全是不可识别的名字时回退到标准名。
    #[test]
    fn minimax_就近映射不可识别() {
        use MinimaxReasoningEffort as E;
        for opts in [
            vec![],
            vec!["default".to_string()],
            vec!["ultra".to_string()],
        ] {
            for e in E::ALL {
                assert_eq!(
                    e.nearest_effort(&opts),
                    e.effort_value(),
                    "{e:?} 应回退标准名"
                );
            }
        }
    }

    /// 未知档位字符串必须报错（`deny_unknown_fields` + 枚举无 `other`）。
    #[test]
    fn minimax_未知档位报错() {
        assert!(parse("[minimax]\nreasoning_effort = \"ultra\"\n").is_err());
        assert!(parse("[minimax]\nclaim_intervals = 10\n").is_ok());
        let c = parse("[minimax]\nclaim_intervals = 10\n").unwrap();
        assert!(c.claim_interval_for("minimax").is_err());
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

    /// 示例文件里三家的思考强度档位各不相同。
    #[test]
    fn 示例文件的思考强度() {
        let c = Config::default();
        assert_eq!(c.trae.reasoning_effort, TraeReasoningEffort::None);
        assert_eq!(c.raccoon.reasoning_effort, RaccoonReasoningEffort::Low);
        assert_eq!(c.codearts.reasoning_effort, CodeartsReasoningEffort::Low);
        assert_eq!(c.minimax.reasoning_effort, MinimaxReasoningEffort::Low);
    }

    /// 各供应商的档位集合**互相独立**：给 trae 写 `low` 必须报错。
    #[test]
    fn 不合法的档位被拒() {
        assert!(parse("[trae]\nreasoning_effort = \"low\"\n").is_err());
        assert!(parse("[raccoon]\nreasoning_effort = \"none\"\n").is_err());
        assert!(parse("[codearts]\nreasoning_effort = \"max\"\n").is_err());
        // minimax 有 `max`，但没有 `minimal`/`ultra`
        assert!(parse("[minimax]\nreasoning_effort = \"minimal\"\n").is_err());
        assert!(parse("[minimax]\nreasoning_effort = \"max\"\n").is_ok());
    }

    /// 合法档位可解析。
    #[test]
    fn 合法档位可解析() {
        let c = parse("[raccoon]\nreasoning_effort = \"high\"\n").unwrap();
        assert_eq!(c.raccoon.reasoning_effort, RaccoonReasoningEffort::High);
        let c = parse("[codearts]\nreasoning_effort = \"high\"\n").unwrap();
        assert_eq!(c.codearts.reasoning_effort, CodeartsReasoningEffort::High);
    }

    /// 写错供应商名应报错（不是被当未知供应商静默接受）。
    #[test]
    fn 写错供应商名被拒() {
        assert!(parse("[racocon]\nclaim_intervals = 3600\n").is_err());
    }
}
