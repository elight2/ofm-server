//! 模型目录（`/v1/models` 的数据来源）。
//!
//! 协议见 `provider-info/trae.md` §4：请求体是固定常量（11 个 function），
//! 响应约 2.7 MB，只需从中取 `function == "solo_agent"` 且
//! `usage == "chat_completion"` 的 `config_name`。
//!
//! **不过滤 `is_invisible_to_user`**：该标志只表示客户端 UI 不展示
//! （56 个可对话条目里有 38 个为 true，含实测可用的 `glm-5.2`）。

use serde::{Deserialize, Serialize};

use crate::core::provider::ModelInfo;

pub const ENDPOINT_PATH: &str = "/api/ide/v1/batch_get_detail_param";

/// 对话使用的 function，与 [`super::chat`] 保持一致（准入闸门，见 §4.3）。
const FUNCTION: &str = "solo_agent";
/// 只保留可对话的条目；其余 usage 是嵌入/补全等非对话用途。
const USAGE_CHAT: &str = "chat_completion";

/// 请求体是固定常量（§4.1，`[抓包]` 358 字节）。
#[derive(Debug, Serialize)]
pub struct CatalogBody {
    pub functions: [&'static str; 11],
    pub agent_type: &'static str,
    pub current_config_info: CurrentConfig,
    pub mode_type: u8,
    pub access_type: u8,
    pub ab_force_vids: &'static str,
    pub ab_autotest_advanced_mode: u8,
}

#[derive(Debug, Serialize)]
pub struct CurrentConfig {
    pub config_name: &'static str,
    pub is_custom_model: bool,
}

impl CatalogBody {
    pub fn build() -> Self {
        Self {
            functions: [
                "personal_assistant",
                "assistant",
                "solo_agent",
                "solo_agent_lite",
                "solo_coder",
                "solo_agent_remote",
                "solo_work_lite",
                "solo_work_remote",
                "solo_design_lite",
                "solo_design_remote",
                "builder",
            ],
            agent_type: "",
            current_config_info: CurrentConfig {
                config_name: "",
                is_custom_model: false,
            },
            mode_type: 0,
            access_type: 1,
            ab_force_vids: "",
            ab_autotest_advanced_mode: 0,
        }
    }
}

// ---------------------------------------------------------------- 响应

#[derive(Debug, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub function_configs: Vec<FunctionConfig>,
}

#[derive(Debug, Deserialize)]
pub struct FunctionConfig {
    #[serde(default)]
    pub function: String,
    #[serde(default)]
    pub config_info_list: Vec<ConfigInfo>,
    /// 失败时上游仍回 HTTP 200，错误在这里（实测缺 `x-app-id` 头 → `2001`）。
    #[serde(default)]
    pub error_code: Option<i64>,
    #[serde(default)]
    pub error_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ConfigInfo {
    #[serde(default)]
    pub config_name: String,
    #[serde(default)]
    pub usage: String,
    /// 字符串化 JSON，仅在存在时才解析（2.7 MB 里大部分条目用不到）。
    #[serde(default)]
    pub display_config: Option<serde_json::Value>,
    /// 字符串化 JSON：`access` / `consumption_rate` / `multimodal` / `reasoning`（§4.2）。
    #[serde(default)]
    pub display_contact_config: Option<serde_json::Value>,
}

impl Catalog {
    /// 上次请求若失败，上游把错误塞在 200 响应里。
    ///
    /// 返回第一个非零 `error_code` 的描述；全部正常时返回 `None`。
    pub fn first_error(&self) -> Option<String> {
        self.function_configs
            .iter()
            .find(|f| f.error_code.is_some_and(|c| c != 0))
            .map(|f| {
                format!(
                    "{} (function={}, code={})",
                    f.error_message.as_deref().unwrap_or("未知错误"),
                    f.function,
                    f.error_code.unwrap_or_default()
                )
            })
    }
}

/// 从目录里挑出可用于对话的模型。
///
/// 顺序保持上游返回顺序，`config_name` 去重（实测无重复，防御性处理）。
pub fn to_models(catalog: &Catalog) -> Vec<ModelInfo> {
    let Some(func) = catalog
        .function_configs
        .iter()
        .find(|f| f.function == FUNCTION)
    else {
        return Vec::new();
    };

    let mut seen = std::collections::HashSet::new();
    func.config_info_list
        .iter()
        .filter(|c| c.usage == USAGE_CHAT)
        .filter(|c| !c.config_name.is_empty())
        .filter(|c| seen.insert(c.config_name.as_str()))
        .map(|c| ModelInfo {
            model: c.config_name.clone(),
            display_name: display_name(c),
            rate: consumption_rate(c),
        })
        .collect()
}

/// `display_contact_config` 可能是对象也可能是字符串化 JSON（§4.2），两种都取。
fn contact_value(info: &ConfigInfo) -> Option<serde_json::Value> {
    match info.display_contact_config.as_ref()? {
        obj @ serde_json::Value::Object(_) => Some(obj.clone()),
        serde_json::Value::String(s) => serde_json::from_str(s).ok(),
        _ => None,
    }
}

/// 计费倍率：`display_contact_config.consumption_rate.data.rate`（§4.2）。
///
/// `consumption_rate.enable == false` 时视为未提供。
fn consumption_rate(info: &ConfigInfo) -> Option<f64> {
    let cr = contact_value(info)?.get("consumption_rate")?.clone();
    if cr.get("enable").and_then(|v| v.as_bool()) == Some(false) {
        return None;
    }
    cr.get("data")?.get("rate")?.as_f64()
}


/// `display_config` 可能是对象也可能是字符串化 JSON（§4.2），两种都取。
fn display_name(info: &ConfigInfo) -> Option<String> {
    let value = match info.display_config.as_ref()? {
        obj @ serde_json::Value::Object(_) => obj,
        serde_json::Value::String(s) => return from_value(serde_json::from_str(s).ok()?),
        _ => return None,
    };
    from_value(value.clone())
}

fn from_value(value: serde_json::Value) -> Option<String> {
    value
        .get("display_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_with(json: &str) -> Catalog {
        serde_json::from_str(json).expect("目录 JSON 解析失败")
    }

    #[test]
    fn 只取_solo_agent_的可对话条目() {
        let c = catalog_with(
            r#"{"function_configs":[
              {"function":"solo_work_lite","config_info_list":[
                {"config_name":"other","usage":"chat_completion"}]},
              {"function":"solo_agent","config_info_list":[
                {"config_name":"glm-5.2","usage":"chat_completion"},
                {"config_name":"embed-1","usage":"embedding"}]}]}"#,
        );
        let m = to_models(&c);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].model, "glm-5.2");
    }

    #[test]
    fn 不过滤_is_invisible_to_user() {
        // 该标志只影响 UI 展示，过滤会砍掉实测可用的 glm-5.2。
        let c = catalog_with(
            r#"{"function_configs":[{"function":"solo_agent","config_info_list":[
              {"config_name":"glm-5.2","usage":"chat_completion","is_invisible_to_user":true}]}]}"#,
        );
        assert_eq!(to_models(&c).len(), 1);
    }

    #[test]
    fn display_name_支持对象与字符串化两种形态() {
        let c = catalog_with(
            r#"{"function_configs":[{"function":"solo_agent","config_info_list":[
              {"config_name":"a","usage":"chat_completion",
               "display_config":{"display_name":"Alpha"}},
              {"config_name":"b","usage":"chat_completion",
               "display_config":"{\"display_name\":\"Beta\"}"},
              {"config_name":"c","usage":"chat_completion"}]}]}"#,
        );
        let m = to_models(&c);
        assert_eq!(m[0].display_name.as_deref(), Some("Alpha"));
        assert_eq!(m[1].display_name.as_deref(), Some("Beta"));
        assert_eq!(m[2].display_name, None);
    }

    #[test]
    fn 去重且保持原顺序() {
        let c = catalog_with(
            r#"{"function_configs":[{"function":"solo_agent","config_info_list":[
              {"config_name":"b","usage":"chat_completion"},
              {"config_name":"a","usage":"chat_completion"},
              {"config_name":"b","usage":"chat_completion"}]}]}"#,
        );
        let names: Vec<_> = to_models(&c).into_iter().map(|m| m.model).collect();
        assert_eq!(names, ["b", "a"]);
    }

    #[test]
    fn 解析计费倍率() {
        let c = catalog_with(
            r#"{"function_configs":[{"function":"solo_agent","config_info_list":[
              {"config_name":"a","usage":"chat_completion",
               "display_contact_config":"{\"consumption_rate\":{\"enable\":true,\"data\":{\"rate\":0.78}}}"},
              {"config_name":"b","usage":"chat_completion",
               "display_contact_config":{"consumption_rate":{"enable":true,"data":{"rate":0.2}}}},
              {"config_name":"c","usage":"chat_completion",
               "display_contact_config":"{\"consumption_rate\":{\"enable\":false,\"data\":{\"rate\":0.5}}}"},
              {"config_name":"d","usage":"chat_completion"}]}]}"#,
        );
        let m = to_models(&c);
        assert_eq!(m[0].rate, Some(0.78));
        assert_eq!(m[1].rate, Some(0.2));
        assert_eq!(m[2].rate, None, "enable=false 视为未提供");
        assert_eq!(m[3].rate, None, "缺字段视为未提供");
    }

    #[test]
    fn 倍率后缀格式() {
        assert_eq!(crate::core::provider::rate_suffix(Some(0.78)), " (x0.78)");
        assert_eq!(crate::core::provider::rate_suffix(Some(1.0)), " (x1)");
        assert_eq!(crate::core::provider::rate_suffix(None), "");
    }

    #[test]
    fn 缺少_solo_agent_时返回空列表() {
        let c = catalog_with(r#"{"function_configs":[]}"#);
        assert!(to_models(&c).is_empty());
        let c = catalog_with("{}");
        assert!(to_models(&c).is_empty());
    }

    #[test]
    fn 检测出_200_响应里的错误码() {
        // 缺 x-app-id 头时上游回 200 但每个 function 都带 error_code 2001。
        let c = catalog_with(
            r#"{"function_configs":[{"function":"solo_agent","error_code":2001,
              "error_message":"Config detail for function: solo_agent not found"}]}"#,
        );
        let msg = c.first_error().expect("应识别为错误");
        assert!(msg.contains("2001"), "错误信息应含错误码: {msg}");
        assert!(msg.contains("solo_agent"));
        assert!(to_models(&c).is_empty());
    }

    #[test]
    fn 正常目录无错误码() {
        let c = catalog_with(
            r#"{"function_configs":[{"function":"solo_agent","error_code":0,
              "config_info_list":[{"config_name":"m","usage":"chat_completion"}]}]}"#,
        );
        assert!(c.first_error().is_none());
    }

    /// 真实抓包回归：`tmp/trae-capture/flows.jsonl` 的 2757110 字节响应。
    #[test]
    fn 真实抓包得到_56_个模型() {
        let Ok(text) = std::fs::read_to_string("tmp/trae-capture/flows.jsonl") else {
            return;
        };
        let Some(body) = text.lines().find_map(|line| {
            let v: serde_json::Value = serde_json::from_str(line).ok()?;
            let url = v.get("url")?.as_str()?;
            if !url.contains("batch_get_detail_param") {
                return None;
            }
            let body = v.get("resp_body")?.as_str()?;
            Some(body.to_string())
        }) else {
            return;
        };
        let catalog: Catalog = serde_json::from_str(&body).expect("真实响应解析失败");
        let models = to_models(&catalog);
        assert_eq!(models.len(), 56, "§4.3 实测口径为 56 个可对话模型");
        assert!(
            models.iter().any(|m| m.model == "glm-5.2"),
            "glm-5.2 已实测可用，不应被过滤"
        );
        let glm = models.iter().find(|m| m.model == "glm-5.2").unwrap();
        assert_eq!(glm.display_name.as_deref(), Some("GLM-5.2"));
        assert_eq!(glm.rate, Some(0.78), "§4.2 实测 glm-5.2 倍率 0.78");
        assert!(
            models.iter().all(|m| m.rate.is_some()),
            "真实目录 56 条均带 consumption_rate"
        );
    }
}
