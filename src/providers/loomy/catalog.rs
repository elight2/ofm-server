//! Loomy 模型目录解析（`GET /models`）。
//!
//! 响应是积分接口那套信封，`data` 才是模型数组：
//! ```json
//! {"data":[{"id":"spark-x2.6","name":"Spark X2.6","type":"chat",
//!           "protocol":"openai_chat","context_length":128000,
//!           "reasoning_efforts":["none","low","medium","high","xhigh"],
//!           "capabilities":{"vision":false,"function_calling":true},...}]}
//! ```
//!
//! # 过滤规则
//!
//! * 只保留 `type == "chat"`：目录里还有 `type == "image"` 的文生图模型
//!   （`Hy-Image-3.5-preview` 等），经 `/v1/chat/completions` 转发必然 404
//!   「该模型暂未开放」。
//! * `auto` 是路由模型（实测会转发到具体模型，响应 `model` 为真实模型名），
//!   保留：它没有 `capabilities`，属正常。
//! * 上游**无倍率字段**，`rate` 恒为 `None`（不会生成 `(x…)` 后缀）。

use serde_json::Value;

use crate::core::provider::ModelInfo;

/// 解析模型目录响应（整体响应体，含信封）。
pub fn parse_models(data: &Value) -> Vec<ModelInfo> {
    // 外层信封的 `data` 才是数组；容错：直接给数组时也认。
    let items = data
        .get("data")
        .unwrap_or(data)
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or_default();

    let mut out: Vec<ModelInfo> = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        if id.is_empty() || !is_chat(item) {
            continue;
        }
        // 同名（同 id）去重，保留首个。
        if out.iter().any(|m| m.model == id) {
            continue;
        }
        out.push(ModelInfo {
            model: id.to_string(),
            display_name: item
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            rate: None,
        });
    }
    out
}

/// 是否可对话模型。
///
/// `type` 缺失时按可对话处理（路由模型 `auto` 总带 `type: chat`，这里只是容错）。
fn is_chat(item: &Value) -> bool {
    match item.get("type").and_then(|v| v.as_str()) {
        None => true,
        Some(t) => t == "chat",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实响应（2026-11-20 实测，已删减无关模型）。
    const REAL: &str = r#"{
      "reasoning_enabled": true,
      "reasoning_catalog_version": 3,
      "object": "list",
      "data": [
        {"id":"spark-x2.6","name":"Spark X2.6","object":"model","type":"chat",
         "protocol":"openai_chat","context_length":128000,
         "reasoning_efforts":["none","low","medium","high","xhigh"],
         "default_reasoning_effort":"low","is_default_model":true,
         "capabilities":{"reasoning":true,"vision":false,"function_calling":true}},
        {"id":"GLM-5.3-Flash","name":"GLM 5.3 Flash","object":"model","type":"chat",
         "protocol":"openai_chat","is_default_model":false},
        {"id":"Hy-Image-3.5-preview","name":"Hy image 3.5 preview","object":"model",
         "type":"image","protocol":"openai_chat","capabilities":{"output_modalities":["image"]}},
        {"id":"qwen-image-3.0-pro","name":"qwen image 3.0 pro","object":"model","type":"image"},
        {"id":"auto","name":"自动 · 均衡","object":"model","type":"chat","protocol":"openai_chat"}
      ]}"#;

    #[test]
    fn 只保留chat模型() {
        let v: Value = serde_json::from_str(REAL).unwrap();
        let models = parse_models(&v);
        let ids: Vec<&str> = models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(ids, ["spark-x2.6", "GLM-5.3-Flash", "auto"]);
    }

    #[test]
    fn 无倍率字段() {
        let v: Value = serde_json::from_str(REAL).unwrap();
        assert!(parse_models(&v).iter().all(|m| m.rate.is_none()));
    }

    #[test]
    fn 保留展示名() {
        let v: Value = serde_json::from_str(REAL).unwrap();
        let models = parse_models(&v);
        assert_eq!(models[0].display_name.as_deref(), Some("Spark X2.6"));
    }

    #[test]
    fn 空响应不报错() {
        assert!(parse_models(&serde_json::json!({})).is_empty());
        assert!(parse_models(&serde_json::json!({"data": []})).is_empty());
    }

    #[test]
    fn 同id去重() {
        let v = serde_json::json!({"data":[
            {"id":"a","type":"chat"},{"id":"a","type":"chat"}
        ]});
        assert_eq!(parse_models(&v).len(), 1);
    }

    #[test]
    fn type缺失按chat处理() {
        let v = serde_json::json!({"data":[{"id":"a"}]});
        assert_eq!(parse_models(&v).len(), 1);
    }
}
