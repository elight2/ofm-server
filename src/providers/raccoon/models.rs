//! 模型目录：`GET /api/web/llm/v2/model_catalog`。
//!
//! 响应形如 `{code:0, data:{categories:[{type, default_model, models:[...]}]}}`。
//! 只取 `type == "chat"` 的分类；同 `name` 去重；`visible == false` 的模型
//! 不对外暴露（它们是内部路由模型，如 `raccoon-090dd3`）。
//!
//! 协议实况见 `provider-info/raccoon.md` §4。

use serde::Deserialize;

use crate::core::provider::ModelInfo;

use super::api::{self, LLM_PREFIX};

/// 模型目录路径（相对 `api::LLM_PREFIX`）。
pub const PATH: &str = "/model_catalog";

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub categories: Vec<Category>,
}

#[derive(Debug, Deserialize)]
pub struct Category {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub models: Vec<Model>,
}

#[derive(Debug, Deserialize)]
pub struct Model {
    pub name: String,
    /// 上游显式给出 `false` 才隐藏；字段缺失视为可见。
    #[serde(default)]
    pub visible: Option<bool>,
    /// 已计入折扣的实际计费倍率。`0` 表示限免。
    #[serde(default)]
    pub billing_effective_multiplier: Option<f64>,
    #[serde(default)]
    pub display_description: Option<String>,
}

/// 拉取并转换模型目录。
///
/// `get_envelope` 直接返回解出的 `data`（错误处理与 POST 共用）。
pub async fn fetch(
    client: &reqwest::Client,
    headers: &[(&str, String)],
) -> Result<Vec<ModelInfo>, api::HttpError> {
    let url = format!("{}{LLM_PREFIX}{PATH}", api::HOST);
    let catalog: Catalog = api::get_envelope(client, &url, headers).await?;
    Ok(to_models(catalog))
}

/// 目录 → 对外模型列表。纯函数，便于单测。
pub fn to_models(catalog: Catalog) -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = Vec::new();
    for cat in catalog.categories {
        if cat.kind != "chat" {
            continue;
        }
        for m in cat.models {
            // 上游显式 visible=false 的不暴露。
            if m.visible == Some(false) {
                continue;
            }
            if out.iter().any(|x| x.model == m.name) {
                continue;
            }
            out.push(ModelInfo {
                model: m.name,
                display_name: m.display_description,
                rate: m.billing_effective_multiplier,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Catalog {
        serde_json::from_str(json).unwrap()
    }

    /// 真实响应节选：3 个 hidden + 2 个 visible。
    #[test]
    fn 过滤隐藏模型() {
        let c = parse(
            r#"{"categories":[{"type":"chat","default_model":"raccoon-090dd3","models":[
                {"name":"raccoon-090dd3","visible":false,"billing_effective_multiplier":1},
                {"name":"sn-glm-5-3","visible":true,"billing_effective_multiplier":0.75}
            ]}]}"#,
        );
        let got = to_models(c);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model, "sn-glm-5-3");
        assert_eq!(got[0].rate, Some(0.75));
    }

    /// 只取 chat 分类，忽略其它类型。
    #[test]
    fn 只取chat分类() {
        let c = parse(
            r#"{"categories":[
                {"type":"embedding","models":[{"name":"emb-1","visible":true}]},
                {"type":"chat","models":[{"name":"m1","visible":true}]}
            ]}"#,
        );
        let got = to_models(c);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].model, "m1");
    }

    /// 同 name 出现多次时只保留第一个。
    #[test]
    fn 同名去重() {
        let c = parse(
            r#"{"categories":[{"type":"chat","models":[
                {"name":"m1","visible":true,"billing_effective_multiplier":1},
                {"name":"m1","visible":true,"billing_effective_multiplier":2}
            ]}]}"#,
        );
        let got = to_models(c);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].rate, Some(1.0));
    }

    /// visible 字段缺失时视为可见（不因上游省略字段而丢模型）。
    #[test]
    fn visible缺失视为可见() {
        let c = parse(r#"{"categories":[{"type":"chat","models":[{"name":"m1"}]}]}"#);
        assert_eq!(to_models(c).len(), 1);
    }

    /// 限免模型倍率为 0，需原样保留（0 不等于 None）。
    #[test]
    fn 限免倍率为零保留() {
        let c = parse(
            r#"{"categories":[{"type":"chat","models":[
                {"name":"free","visible":true,"billing_effective_multiplier":0}
            ]}]}"#,
        );
        let got = to_models(c);
        assert_eq!(got[0].rate, Some(0.0));
    }

    /// 真实响应完整解析（9 个模型，6 个可见）。
    #[test]
    fn 真实响应节选() {
        let c = parse(
            r#"{"categories":[{"type":"chat","default_model":"raccoon-090dd3","models":[
                {"name":"raccoon-090dd3","visible":false,"billing_effective_multiplier":1},
                {"name":"raccoon-19b265","visible":false,"billing_effective_multiplier":1},
                {"name":"raccoon-405a1c","visible":false,"billing_effective_multiplier":1},
                {"name":"sn-sensenova-6-8-flash","visible":true,"billing_effective_multiplier":0},
                {"name":"sn-sensenova-6-8-flash-lite","visible":true,"billing_effective_multiplier":0},
                {"name":"sn-glm-5-3","visible":true,"billing_effective_multiplier":0.75},
                {"name":"sn-kimi-k3","visible":true,"billing_effective_multiplier":1},
                {"name":"sn-glm-5-3-flash","visible":true,"billing_effective_multiplier":0.2},
                {"name":"sn-deepseek-v4-1-flash-1017","visible":true,"billing_effective_multiplier":0.25}
            ]}]}"#,
        );
        let got = to_models(c);
        assert_eq!(got.len(), 6);
        assert!(got.iter().all(|m| !m.model.starts_with("raccoon-")));
    }
}
