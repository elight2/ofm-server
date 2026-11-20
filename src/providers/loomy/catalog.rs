//! Loomy 模型目录解析。
//!
//! 从 `/v1/models` 端点获取模型列表，解析为 [`crate::core::provider::ModelInfo`]。

use serde_json::Value;

use crate::core::provider::ModelInfo;

/// 解析模型目录响应。
pub fn parse_models(data: &Value) -> Vec<ModelInfo> {
    let items = data
        .get("data")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    items
        .iter()
        .filter_map(|item| {
            let id = item.get("id").and_then(|v| v.as_str())?.to_string();
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or(&id)
                .to_string();
            let rate = item.get("rate").and_then(|v| v.as_f64());

            Some(ModelInfo {
                model: id,
                display_name: Some(name),
                rate,
            })
        })
        .collect()
}
