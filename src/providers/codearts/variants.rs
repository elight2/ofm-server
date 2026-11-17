//! 思考档位（`thinking-variants`）与 `reasoning_effort` 落地。
//!
//! 上游档位**不是固定枚举**，而是逐模型由服务端下发：
//!
//! ```text
//! POST {SNAP_GATEWAY}/v1/model/thinking-variants   （Agent-Type: PromptCenter）
//! body {"model_names":["GLM-5.2", ...]}
//! → {"variants":{"GLM-5.2":{"strengths":["high","max"],"default_strength":"high"},
//!                "kimi-k3":{"strengths":[],"default_strength":""}}}
//! ```
//!
//! 实测（2026-11-16，商业版账号）：目录 12 个模型中只有 `GLM-5.2` 与
//! `glm-5.2-sft-harmony` 返回 `["high","max"]`，其余 `strengths` 为空数组。
//! 与 IDE 界面「只有两个 GLM 5.2 支持调整思考」一致。
//!
//! 因此本模块的职责是：
//! 1. 周期性（[`REFRESH_INTERVAL`]）拉取全量档位并缓存；
//! 2. 把配置里的抽象档位（[`CodeartsReasoningEffort`]）**就近映射**到该模型
//!    实际支持的档位；
//! 3. 模型没有档位（`strengths` 为空或未知）时**不发**该字段。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::config::CodeartsReasoningEffort;
use crate::core::provider::ProviderError;
use crate::providers::codearts::api::SNAP_GATEWAY;
use crate::providers::codearts::credential::Credential;
use crate::providers::codearts::sign;

/// 档位拉取路径（内核 `hD`）。
const VARIANTS_PATH: &str = "/v1/model/thinking-variants";

/// `Agent-Type` 必须为 `PromptCenter`；`AgentCenter` 会被网关拒绝
/// （实测 `400 TM.00001001 未找到合法可路由api`）。
const AGENT_TYPE: &str = "PromptCenter";

/// 单次请求最多询问的模型数（内核 `VFn=50`，超过会被上游忽略）。
const MAX_MODELS_PER_REQUEST: usize = 50;

/// 请求超时。
const TIMEOUT_SECS: u64 = 15;

/// 档位刷新间隔。上游档位随模型上下线变化，1 小时足够及时。
///
/// 单独定义成常量便于调整：改这里即可，无需动其它代码。
pub const REFRESH_INTERVAL: Duration = Duration::from_secs(3600);

/// 上游档位排序：越靠后表示思考越深。用于就近映射。
///
/// 上游未声明顺序，这里按 IDE 滑杆的排列（最小 → 最大）给出唯一已知顺序。
/// 未知档位按"比 high 更深"处理，宁可比用户要求的更深而非更浅。
const DEPTH_ORDER: [&str; 8] = [
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

/// 档位深度序号；未知档位返回 `DEPTH_ORDER.len()`（最深）。
fn depth(strength: &str) -> usize {
    DEPTH_ORDER
        .iter()
        .position(|s| s.eq_ignore_ascii_case(strength))
        .unwrap_or(DEPTH_ORDER.len())
}

#[derive(Debug, Deserialize)]
struct VariantsResponse {
    #[serde(default)]
    variants: HashMap<String, Variant>,
}

#[derive(Debug, Deserialize)]
struct Variant {
    #[serde(default)]
    strengths: Vec<String>,
}

/// 模型 → 支持的档位列表。
#[derive(Debug, Default, Clone)]
pub struct Variants {
    inner: HashMap<String, Vec<String>>,
}

impl Variants {
    /// 该模型支持的档位（空表示不支持）。
    pub fn strengths(&self, model: &str) -> &[String] {
        self.inner.get(model).map(Vec::as_slice).unwrap_or(&[])
    }

    /// 把配置档位就近映射为上游取值；模型不支持则返回 `None`（不发字段）。
    ///
    /// 映射规则：
    /// * `low` → 支持的档位中最浅的一个；
    /// * `high` → 支持的档位中最深的一个（"不抑制思考"的语义）。
    ///
    /// 只有两档（如 `["high","max"]`）时即 `low→high`、`high→max`。
    pub fn pick(&self, model: &str, effort: CodeartsReasoningEffort) -> Option<String> {
        let list = self.strengths(model);
        if list.is_empty() {
            return None;
        }
        let chosen = match effort {
            CodeartsReasoningEffort::Low => list.iter().min_by_key(|s| depth(s)),
            CodeartsReasoningEffort::High => list.iter().max_by_key(|s| depth(s)),
        }?;
        Some(chosen.clone())
    }
}

/// 带 TTL 的档位缓存。
///
/// 首次使用时拉取，之后每次 `get` 检查是否超过 [`REFRESH_INTERVAL`]；
/// 过期则重新拉取。拉取失败**保留旧数据**（避免上游抖动导致档位丢失）。
pub struct VariantsCache {
    client: reqwest::Client,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    variants: Variants,
    fetched_at: Option<Instant>,
}

impl VariantsCache {
    pub fn new(client: reqwest::Client) -> Arc<Self> {
        Arc::new(Self {
            client,
            state: Mutex::new(State::default()),
        })
    }

    /// 取当前档位表，必要时刷新。
    ///
    /// `models` 为待查询的模型名全集；缓存命中则不重复请求。
    pub async fn get(
        &self,
        cred: &Credential,
        models: &[String],
    ) -> Result<Variants, ProviderError> {
        let mut state = self.state.lock().await;
        let fresh = state
            .fetched_at
            .is_some_and(|t| t.elapsed() < REFRESH_INTERVAL);
        if fresh {
            return Ok(state.variants.clone());
        }

        match fetch(&self.client, cred, models).await {
            Ok(v) => {
                state.variants = v.clone();
                state.fetched_at = Some(Instant::now());
                Ok(v)
            }
            Err(e) => {
                // 保留旧数据；从未成功过则返回空表（等价"全都不支持"）。
                tracing::warn!("[{}] 拉取思考档位失败，沿用旧数据: {e}", super::ID);
                Ok(state.variants.clone())
            }
        }
    }
}

/// 拉取档位（分批，每批最多 [`MAX_MODELS_PER_REQUEST`] 个模型）。
async fn fetch(
    client: &reqwest::Client,
    cred: &Credential,
    models: &[String],
) -> Result<Variants, ProviderError> {
    let mut inner = HashMap::new();
    for chunk in models.chunks(MAX_MODELS_PER_REQUEST) {
        if chunk.is_empty() {
            continue;
        }
        let body = serde_json::json!({ "model_names": chunk }).to_string();
        let url = format!("{SNAP_GATEWAY}{VARIANTS_PATH}");
        // 与模型目录一致：只读接口用 `signSimple`（UNSIGNED-PAYLOAD）。
        let headers = vec![
            ("content-type", "application/json".to_string()),
            ("agent-type", AGENT_TYPE.to_string()),
            ("x-language", "zh-cn".to_string()),
            ("x-security-token", cred.security_token.clone()),
        ];
        let req = sign::Request {
            method: "POST",
            url: &url,
            headers,
            query: None,
            body: Some(&body),
        };
        let signed = sign::sign_unsigned(&req, &cred.access_key_id, &cred.secret_access_key)
            .map_err(|e| ProviderError::Other(format!("AKSK 签名失败: {e}")))?;

        let mut http = client
            .post(&url)
            .timeout(Duration::from_secs(TIMEOUT_SECS))
            .body(body);
        for (k, v) in &signed {
            http = http.header(k.as_str(), v.as_str());
        }

        let resp = http.send().await.map_err(ProviderError::Transport)?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            let snippet: String = text.chars().take(200).collect();
            return Err(ProviderError::Protocol(format!(
                "thinking-variants: HTTP {status}: {snippet}"
            )));
        }
        let parsed: VariantsResponse = serde_json::from_str(&text)
            .map_err(|e| ProviderError::Protocol(format!("thinking-variants 响应解析失败: {e}")))?;
        for (model, v) in parsed.variants {
            inner.insert(model, v.strengths);
        }
    }
    Ok(Variants { inner })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn variants(pairs: &[(&str, &[&str])]) -> Variants {
        Variants {
            inner: pairs
                .iter()
                .map(|(m, s)| (m.to_string(), s.iter().map(|x| x.to_string()).collect()))
                .collect(),
        }
    }

    #[test]
    fn 无档位模型不发字段() {
        let v = variants(&[("kimi-k3", &[])]);
        assert_eq!(v.pick("kimi-k3", CodeartsReasoningEffort::Low), None);
        assert_eq!(v.pick("kimi-k3", CodeartsReasoningEffort::High), None);
    }

    #[test]
    fn 未知模型不发字段() {
        let v = variants(&[("GLM-5.2", &["high", "max"])]);
        assert_eq!(v.pick("没有这个模型", CodeartsReasoningEffort::Low), None);
    }

    #[test]
    fn glm52两档就近映射() {
        let v = variants(&[("GLM-5.2", &["high", "max"])]);
        assert_eq!(
            v.pick("GLM-5.2", CodeartsReasoningEffort::Low).as_deref(),
            Some("high")
        );
        assert_eq!(
            v.pick("GLM-5.2", CodeartsReasoningEffort::High).as_deref(),
            Some("max")
        );
    }

    #[test]
    fn 多档取最浅与最深() {
        let v = variants(&[("m", &["medium", "low", "high", "max"])]);
        assert_eq!(
            v.pick("m", CodeartsReasoningEffort::Low).as_deref(),
            Some("low")
        );
        assert_eq!(
            v.pick("m", CodeartsReasoningEffort::High).as_deref(),
            Some("max")
        );
    }

    #[test]
    fn 单档时两档配置都取该值() {
        let v = variants(&[("m", &["high"])]);
        assert_eq!(
            v.pick("m", CodeartsReasoningEffort::Low).as_deref(),
            Some("high")
        );
        assert_eq!(
            v.pick("m", CodeartsReasoningEffort::High).as_deref(),
            Some("high")
        );
    }

    #[test]
    fn 未知档位视为最深() {
        let v = variants(&[("m", &["weird", "low"])]);
        assert_eq!(
            v.pick("m", CodeartsReasoningEffort::Low).as_deref(),
            Some("low")
        );
        assert_eq!(
            v.pick("m", CodeartsReasoningEffort::High).as_deref(),
            Some("weird")
        );
    }

    #[test]
    fn 解析实测响应() {
        let text = r#"{"variants":{"GLM-5.2":{"strengths":["high","max"],"default_strength":"high"},
            "kimi-k3":{"strengths":[],"default_strength":""}}}"#;
        let parsed: VariantsResponse = serde_json::from_str(text).unwrap();
        assert_eq!(parsed.variants["GLM-5.2"].strengths, vec!["high", "max"]);
        assert!(parsed.variants["kimi-k3"].strengths.is_empty());
    }
}
