//! 模型目录：`GET /mavis/api/v1/models?region=cn&buildEnv=prod`。
//!
//! 协议见 `provider-info/minimax.md` §4。要点：
//! * 路径比对话 baseURL 少一级 `llm/v1`，**写死**，不从 baseURL 推导；
//! * UA 是桌面浏览器完整 UA，且只带 `authorization: Bearer`；
//! * 模型 id 用 `config.models` 的 **key**（不是 `name`），顺序用 `model_order`；
//! * `whitelist` 含 `models` 里没有的 M2.7，**不用于列表**；
//! * `effort_options` / `default_effort` 是思考档位来源（M3.1-Flash 有，M3 无），
//!   这些字段是上游目录原样返回的元数据，本服务**不发送**任何思考强度字段，
//!   这里一并解析出来仅为兼容解析。

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::core::provider::ModelInfo;

use super::api;

/// 目录响应外层。
#[derive(Debug, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub providers: Vec<ProviderEntry>,
}

/// 目录响应里的供应商条目。
///
/// `provider_id` 目前只用于日志/排查，列表只取 `config`。
#[derive(Debug, Deserialize)]
pub struct ProviderEntry {
    #[serde(default, rename = "providerId")]
    #[allow(dead_code)]
    pub provider_id: String,
    pub config: ProviderConfig,
}

/// 供应商配置。
///
/// `whitelist` 与 `options` 解析出来仅为**确认它们不影响列表**：
/// `whitelist` 含 `models` 未下发的 M2.7（§4：不用于列表），
/// `options.apiKey`/`baseURL` 是占位/旧域名，一律忽略。
#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    /// 模型字典，键为上游模型 id。
    #[serde(default)]
    pub models: BTreeMap<String, ModelEntry>,
    /// 列表顺序（与 `models` 的键字典序可能相反）。
    #[serde(default)]
    pub model_order: Vec<String>,
    /// 模型白名单，含 `models` 未下发的 M2.7。**不用于列表**。
    #[serde(default)]
    #[allow(dead_code)]
    pub whitelist: Vec<String>,
    /// 含 `apiKey`/`baseURL` 占位值，一律忽略。
    #[serde(default)]
    #[allow(dead_code)]
    pub options: Option<serde_json::Value>,
}

/// 单个模型条目。
///
/// 除 `name` 外，其余字段供后续步骤使用：`thinking_config`/`effort_options`
/// 是思考强度映射（第 4 步）的输入，`modalities`/`limit` 决定图片与上下文能力。
#[derive(Debug, Deserialize)]
pub struct ModelEntry {
    /// 展示名（`M3`/`M3.1-Flash`），对外 id 不用它。
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    pub reasoning: bool,
    #[serde(default)]
    #[allow(dead_code)]
    pub attachment: bool,
    #[serde(default)]
    #[allow(dead_code)]
    pub limit: Option<Limit>,
    #[serde(default)]
    #[allow(dead_code)]
    pub modalities: Option<Modalities>,
    /// `switchable`（M3）/ `forced_on`（M3.1-Flash），上游目录原样返回。
    #[serde(default)]
    #[allow(dead_code)]
    pub thinking_config: Option<ThinkingConfig>,
    /// 可选思考档位（只有 M3.1-Flash 下发），上游目录原样返回。
    #[serde(default)]
    #[allow(dead_code)]
    pub effort_options: Option<Vec<String>>,
    #[serde(default)]
    #[allow(dead_code)]
    pub default_effort: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Limit {
    #[serde(default)]
    #[allow(dead_code)]
    pub context: Option<i64>,
    #[serde(default)]
    #[allow(dead_code)]
    pub output: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct Modalities {
    #[serde(default)]
    #[allow(dead_code)]
    pub input: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ThinkingConfig {
    /// `switchable` | `forced_on`。
    #[serde(default)]
    #[allow(dead_code)]
    pub mode: Option<String>,
}

impl ModelEntry {
    /// 是否支持图片输入（`modalities.input` 含 `image`）。
    ///
    /// 第 3 步（对话）判断图片能否透传时启用。
    #[allow(dead_code)]
    pub fn supports_image(&self) -> bool {
        self.modalities
            .as_ref()
            .is_some_and(|m| m.input.iter().any(|s| s == "image"))
    }
}

/// 拉取模型目录。
pub async fn fetch(
    client: &reqwest::Client,
    token: &str,
) -> Result<Vec<ModelInfo>, api::HttpError> {
    let url = format!(
        "{}{}?{}",
        api::AGENT_HOST,
        api::MODELS_PATH,
        api::MODELS_QUERY
    );
    let cat: Catalog = api::get_json(client, &url, token).await?;
    Ok(to_models(&cat))
}

/// 目录 → 对外模型列表。纯函数，便于单测。
///
/// 倍率（`(x0.78)` 后缀）上游目录**不提供**，故 `rate` 恒为 `None`。
/// 顺序：先按 `model_order`，未列出的追加在后（保持键序）。
pub fn to_models(catalog: &Catalog) -> Vec<ModelInfo> {
    let Some(provider) = catalog.providers.first() else {
        return Vec::new();
    };
    let cfg = &provider.config;

    let mut order: Vec<&str> = Vec::new();
    for id in &cfg.model_order {
        if cfg.models.contains_key(id) && !order.contains(&id.as_str()) {
            order.push(id);
        }
    }
    for id in cfg.models.keys() {
        if !order.contains(&id.as_str()) {
            order.push(id);
        }
    }

    order
        .into_iter()
        .filter_map(|id| cfg.models.get(id).map(|m| (id, m)))
        .map(|(id, m)| ModelInfo {
            model: id.to_string(),
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

    /// 真实响应节选（§4 抓包），验证顺序与 id 取值。
    const REAL: &str = r#"{
      "version":"1.0","ttlSeconds":300,
      "providers":[{"providerId":"minimax","config":{
        "name":"MiniMax","npm":"@ai-sdk/anthropic",
        "models":{
          "MiniMax-M3":{
            "name":"M3","attachment":true,"reasoning":true,"tool_call":true,
            "limit":{"context":512000,"output":128000},
            "modalities":{"input":["text","image","video"],"output":["text"]},
            "thinking_config":{"mode":"switchable","default_value":"true"}},
          "MiniMax-M3.1-Flash":{
            "name":"M3.1-Flash","attachment":true,"reasoning":true,"tool_call":true,
            "limit":{"context":512000,"output":128000},
            "modalities":{"input":["text","image","video"],"output":["text"]},
            "thinking_config":{"mode":"forced_on"},
            "effort_options":["default","low","medium","high","xhigh","max"],
            "default_effort":"default"}
        },
        "model_order":["MiniMax-M3.1-Flash","MiniMax-M3"],
        "whitelist":["MiniMax-M2.7-highspeed","MiniMax-M2.7","MiniMax-M3","MiniMax-M3.1-Flash"],
        "options":{"apiKey":"sk-xxx","baseURL":"https://agent.minimaxi.com/mavis/api/v1/llm/v1"}
      }}]}"#;

    /// 顺序按 model_order（M3.1-Flash 在前），id 用键而非 name。
    #[test]
    fn 顺序与_id_取键() {
        let got = to_models(&parse(REAL));
        let ids: Vec<&str> = got.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(ids, vec!["MiniMax-M3.1-Flash", "MiniMax-M3"]);
    }

    /// whitelist 里的 M2.7 不能出现在列表里。
    #[test]
    fn 不暴露_whitelist_独有模型() {
        let got = to_models(&parse(REAL));
        assert!(got.iter().all(|m| !m.model.contains("M2.7")));
    }

    /// 目录不给倍率 → rate 恒 None（对外无 ` (x…)` 后缀）。
    #[test]
    fn 倍率恒为空() {
        let got = to_models(&parse(REAL));
        assert!(got.iter().all(|m| m.rate.is_none()));
    }

    /// 未列入 model_order 的模型追加在后，不丢。
    #[test]
    fn 未排序模型不丢() {
        let json = r#"{"providers":[{"providerId":"minimax","config":{
            "models":{"A":{"name":"a"},"B":{"name":"b"}},
            "model_order":["B"]}}]}"#;
        let ids: Vec<String> = to_models(&parse(json))
            .into_iter()
            .map(|m| m.model)
            .collect();
        assert_eq!(ids, vec!["B", "A"]);
    }

    /// model_order 引用不存在的键时忽略，不 panic。
    #[test]
    fn model_order_含不存在的键() {
        let json = r#"{"providers":[{"providerId":"minimax","config":{
            "models":{"A":{"name":"a"}},
            "model_order":["Ghost","A"]}}]}"#;
        let ids: Vec<String> = to_models(&parse(json))
            .into_iter()
            .map(|m| m.model)
            .collect();
        assert_eq!(ids, vec!["A"]);
    }

    /// 空目录 / 缺 providers 都返回空表。
    #[test]
    fn 空目录返回空表() {
        assert!(to_models(&parse(r#"{"version":"1.0","providers":[]}"#)).is_empty());
        assert!(to_models(&parse(r#"{"version":"1.0"}"#)).is_empty());
    }

    /// 思考档位与开关形态是上游目录原样返回的元数据，必须能解析出来。
    #[test]
    fn 解析思考配置() {
        let c = parse(REAL);
        let cfg = &c.providers[0].config;
        let m3 = &cfg.models["MiniMax-M3"];
        assert_eq!(
            m3.thinking_config.as_ref().unwrap().mode.as_deref(),
            Some("switchable")
        );
        assert_eq!(m3.effort_options, None);
        assert!(m3.supports_image());

        let flash = &cfg.models["MiniMax-M3.1-Flash"];
        assert_eq!(
            flash.thinking_config.as_ref().unwrap().mode.as_deref(),
            Some("forced_on")
        );
        assert_eq!(flash.effort_options.as_ref().unwrap().len(), 6);
        assert_eq!(flash.default_effort.as_deref(), Some("default"));
        assert_eq!(flash.limit.as_ref().unwrap().context, Some(512_000));
        assert_eq!(flash.limit.as_ref().unwrap().output, Some(128_000));
    }

    /// 目录里的旧 baseURL 必须被忽略（不参与任何拼接）。
    #[test]
    fn 目录_base_url_不影响路径() {
        let url = format!(
            "{}{}?{}",
            api::AGENT_HOST,
            api::MODELS_PATH,
            api::MODELS_QUERY
        );
        assert_eq!(
            url,
            "https://agent.minimax.cn/mavis/api/v1/models?region=cn&buildEnv=prod"
        );
        assert!(!url.contains("minimaxi.com"));
        assert!(!url.contains("llm/v1"));
    }
}
