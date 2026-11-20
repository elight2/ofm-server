//! 模型目录：`GET /v2/enterprises/personal/models`。
//!
//! 协议见 `provider-info/workbuddy.md` §4。要点：
//! * **对话侧用这个端点**，不是 `/v3/config`。`/v3/config` 的 `data.models[]`
//!   是桌面端自己的模型表，混着图像/视频/补全类模型，且其中一部分模型在
//!   `/v2/chat/completions` 上并没有接入（回 `code=11102`
//!   `model [x] service info not found`）。
//! * `data.agents[]` 里每个 agent 的 `models[]` 是该 agent 可选模型。
//!   `cli` agent 的清单是「对话侧可调用」的权威判据：只有出现在这里的模型
//!   才能在 `/v2/chat/completions` 上正常返回 SSE。
//! * `data.models[]` 提供每个模型的 `reasoning.supportedEfforts` /
//!   `defaultEffort` / `canDisableThinking` 声明，是思考强度映射的输入（§5）。

use serde::Deserialize;

use crate::core::provider::ModelInfo;

use super::api;

/// 目录响应 `data`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    /// 全部模型条目（含图像/视频/补全类）。
    #[serde(default)]
    pub models: Vec<CatalogModel>,
    /// 各 agent 的可选模型清单。
    #[serde(default)]
    pub agents: Vec<Agent>,
}

/// 一个 agent 的声明。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Agent {
    /// agent 名，如 `cli`。
    pub name: String,
    /// 该 agent 可选的模型 id 列表。
    #[serde(default)]
    pub models: Vec<String>,
}

/// 单个模型条目。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    /// 上游模型 id（对外暴露的 id 就是它）。
    pub id: String,
    /// 展示名。
    #[serde(default)]
    pub name: Option<String>,
    /// 计费权重文案，如 `x0.51 credits`。非倍率，仅记录。
    #[serde(default)]
    #[allow(dead_code)]
    pub credits: Option<String>,
    /// 上下文窗口。
    #[serde(default)]
    pub max_input_tokens: Option<i64>,
    /// 输出上限。
    #[serde(default)]
    pub max_output_tokens: Option<i64>,
    /// 是否支持图片输入。
    #[serde(default)]
    #[allow(dead_code)]
    pub supports_images: bool,
    /// 是否支持工具调用。
    #[serde(default)]
    #[allow(dead_code)]
    pub supports_tool_call: bool,
    /// 是否支持思考。
    #[serde(default)]
    pub supports_reasoning: bool,
    /// 思考档位声明。
    #[serde(default)]
    pub reasoning: Option<ReasoningSpec>,
}

/// `reasoning` 对象。
///
/// 两种形态并存：
/// * `{"effort":"high","summary":"auto"}`（deepseek/auto 系，未列 supportedEfforts）；
/// * `{"defaultEffort":"high","supportedEfforts":["low","high","max"],
///    "canDisableThinking":true}`（glm/kimi/hy4 系）。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningSpec {
    /// 默认档位（`defaultEffort`）。
    #[serde(default)]
    pub default_effort: Option<String>,
    /// 默认档位（`effort`），与 `default_effort` 二选一存在。
    #[serde(default)]
    pub effort: Option<String>,
    /// 支持的档位集合。
    #[serde(default)]
    pub supported_efforts: Vec<String>,
    /// 是否允许关闭思考（关闭时发 `reasoning_effort: "none"`）。
    #[serde(default)]
    pub can_disable_thinking: Option<bool>,
}

impl ReasoningSpec {
    /// 生效的默认档位：`defaultEffort` 优先于 `effort`。
    pub fn effective_default_effort(&self) -> Option<&str> {
        self.default_effort.as_deref().or(self.effort.as_deref())
    }
}

impl CatalogModel {
    /// 是否可作为 chat 模型暴露。
    ///
    /// 判定依据（§4.2）：没有 `maxInputTokens` 的条目不是对话模型
    /// （图像/视频/补全类），`maxOutputTokens` 极小的也排除。
    pub fn is_chat_model(&self) -> bool {
        let Some(ctx) = self.max_input_tokens else {
            return false;
        };
        if ctx <= 0 {
            return false;
        }
        // 输出上限 256 的是补全/嵌入类，不是对话模型。
        !matches!(self.max_output_tokens, Some(out) if out > 0 && out <= 256)
    }
}

/// `cli` agent 名，模型可调用判据。
const CLI_AGENT: &str = "cli";

impl Catalog {
    /// `cli` agent 的可调用模型 id 集合。
    ///
    /// 找不到 `cli` agent 时返回 `None`（表示「无从判断」，调用方应保守地
    /// 退回按 `data.models[]` 过滤）。
    pub fn cli_model_ids(&self) -> Option<Vec<&str>> {
        self.agents
            .iter()
            .find(|a| a.name == CLI_AGENT)
            .map(|a| a.models.iter().map(String::as_str).collect())
    }
}

/// 拉取原始目录条目（含推理能力声明）。
///
/// 对话转发需要 `reasoning` 声明来钳制思考档位，故这里返回原始条目而非
/// 对外的 [`ModelInfo`]。
///
/// 主源是 `/v2/enterprises/personal/models`；它失败时退回 `/v3/config`
/// （桌面端产品配置，模型表较旧且缺 `supportedEfforts`，但至少有列表）。
pub async fn fetch_catalog(
    client: &reqwest::Client,
    access_token: &str,
    domain: &str,
    uid: &str,
) -> Result<Catalog, api::HttpError> {
    let mut headers = api::auth_headers(access_token, domain);
    headers.push(("Connection", "close".to_string()));
    if !uid.trim().is_empty() {
        headers.push(("X-User-Id", uid.to_string()));
    }

    let primary = format!("{}{}", api::HOST, api::PERSONAL_MODELS_PATH);
    match api::get_envelope::<Catalog>(client, &primary, &headers).await {
        Ok(cat) if !cat.models.is_empty() => return Ok(cat),
        Ok(_) => {}
        Err(e) => {
            tracing::warn!("[workbuddy] 个人模型目录拉取失败，退回 /v3/config: {e}");
        }
    }

    let fallback = format!("{}{}", api::HOST, api::CONFIG_PATH);
    api::get_envelope::<Catalog>(client, &fallback, &headers).await
}

/// 目录 → 对外模型列表。纯函数，便于单测。
///
/// 过滤规则（§4.2）：
/// 1. 必须是 `cli` agent 可选的模型（否则对话端点回 `code=11102`）；
/// 2. 必须是对话模型（有 `maxInputTokens`，且输出上限 > 256）。
///
/// 倍率：上游 `credits` 是 `x0.51 credits` 这类**计费权重文案**，不是倍率，
/// 故 `rate` 恒为 `None`（对外不出现 ` (x…)` 后缀）。
pub fn to_models(catalog: &Catalog) -> Vec<ModelInfo> {
    let cli = catalog.cli_model_ids();
    catalog
        .models
        .iter()
        .filter(|m| m.is_chat_model())
        .filter(|m| match &cli {
            Some(ids) => ids.contains(&m.id.as_str()),
            None => true,
        })
        .map(|m| ModelInfo {
            model: m.id.clone(),
            display_name: m.name.clone(),
            rate: None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Catalog {
        serde_json::from_str(json).unwrap()
    }

    /// 真实响应 `data` 节选（§4.1），覆盖两种 reasoning 声明形态。
    const REAL: &str = r#"{
      "agents":[
        {"name":"cli","models":["auto","hy4std","hy4-preview","glm-5.3","deepseek-v4-pro"]},
        {"name":"CodeCompletion","models":["codewise-completions"]}
      ],
      "models":[
        {"id":"deepseek-v4-pro","name":"DeepSeek V4 Pro","credits":"x0.51 credits",
         "maxInputTokens":128000,"maxOutputTokens":8192,
         "supportsImages":false,"supportsToolCall":true,"supportsReasoning":true,
         "reasoning":{"effort":"high","summary":"auto"}},
        {"id":"glm-5.3-flash","name":"GLM 5.3 Flash","maxInputTokens":200000,
         "maxOutputTokens":32000,"supportsImages":true,"supportsReasoning":true,
         "reasoning":{"defaultEffort":"high","supportedEfforts":["low","high","max"],
                      "canDisableThinking":true}},
        {"id":"hy4-preview","maxInputTokens":64000,"maxOutputTokens":8192,
         "supportsReasoning":true,
         "reasoning":{"supportedEfforts":["high"],"canDisableThinking":false}},
        {"id":"glm-4.6","name":"GLM 4.6","maxInputTokens":168000,"maxOutputTokens":32000},
        {"id":"hunyuan-image-3","name":"混元图像","maxOutputTokens":256},
        {"id":"codewise-complete","maxOutputTokens":256}
      ]}"#;

    /// 只保留 `cli` agent 可调用且是对话模型的条目。
    #[test]
    fn 过滤非可调用模型() {
        let got = to_models(&parse(REAL));
        let ids: Vec<&str> = got.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(ids, vec!["deepseek-v4-pro", "hy4-preview"]);
    }

    /// 没有 `cli` agent 时退回「按对话模型过滤」，不误杀。
    #[test]
    fn 无cli_agent时退回按模型过滤() {
        let c = parse(
            r#"{"models":[
          {"id":"a","maxInputTokens":1000,"maxOutputTokens":1000},
          {"id":"b","maxOutputTokens":256}]}"#,
        );
        let got = to_models(&c);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model, "a");
    }

    /// 倍率恒为空：上游 `credits` 是计费权重文案，不是倍率。
    #[test]
    fn 倍率恒为空() {
        let got = to_models(&parse(REAL));
        assert!(got.iter().all(|m| m.rate.is_none()));
    }

    /// 展示名取 `name`。
    #[test]
    fn 展示名取目录字段() {
        let got = to_models(&parse(REAL));
        assert_eq!(got[0].display_name.as_deref(), Some("DeepSeek V4 Pro"));
        assert_eq!(got[1].display_name, None);
    }

    /// 两种 reasoning 声明形态都要能解析。
    #[test]
    fn 解析思考声明() {
        let c = parse(REAL);
        let ds = &c.models[0].reasoning.as_ref().unwrap();
        assert_eq!(ds.effective_default_effort(), Some("high"));
        assert!(ds.supported_efforts.is_empty());

        let glm = &c.models[1].reasoning.as_ref().unwrap();
        assert_eq!(glm.effective_default_effort(), Some("high"));
        assert_eq!(glm.supported_efforts, vec!["low", "high", "max"]);
        assert_eq!(glm.can_disable_thinking, Some(true));

        let hy = &c.models[2].reasoning.as_ref().unwrap();
        assert_eq!(hy.supported_efforts, vec!["high"]);
        assert_eq!(hy.can_disable_thinking, Some(false));
    }

    /// 空目录返回空表。
    #[test]
    fn 空目录返回空表() {
        assert!(to_models(&parse(r#"{"models":[]}"#)).is_empty());
        assert!(to_models(&parse(r#"{}"#)).is_empty());
    }
}
