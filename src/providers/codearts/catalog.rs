//! PromptCenter 内置模型目录（商业版真实路径，实测验证通过）。
//!
//! 内核 `provider-models-plugin` 的 `$D`（`agentkernelServer-linux-x64`
//! @101236408）：`GET {agentCenterApiUrl}/v1/model/builtin`（snap-access
//! 网关），AKSK `signSimple` 签名（payload 固定 `UNSIGNED-PAYLOAD`，
//! `X-Security-Token` 参与签名）+ `Agent-Type: PromptCenter` + `X-Language`。
//! 响应 `{count, builtinModels: [...]}`。
//!
//! 注：extension 侧的 benefit-service（`opengw.../api/v1/gateway/config`）
//! 需账号开通权益（PromptCenter `benefit-gateway-config.enabled`），未开通
//! 时恒报 `4000 invalid channel`（实测如此），故商业版模型目录实际来自本路径。
//! 实测时间 2026-02-18：返回 12 个模型，含计费倍率与上下文窗口。

use serde::Deserialize;

use crate::core::provider::{ModelInfo, ProviderError};
use crate::providers::codearts::api::SNAP_GATEWAY;
use crate::providers::codearts::credential::Credential;
use crate::providers::codearts::sign;

/// 内置模型目录路径（内核 `IFn`）。
pub const BUILTIN_PATH: &str = "/v1/model/builtin";

/// 请求超时（内核用 axios 默认，无显式超时；此处取 15s）。
const TIMEOUT_SECS: u64 = 15;

/// `GET {SNAP_GATEWAY}/v1/model/builtin` 响应。
#[derive(Debug, Deserialize)]
pub struct BuiltinCatalog {
    pub count: Option<u32>,
    #[serde(rename = "builtinModels", default)]
    pub builtin_models: Vec<BuiltinModel>,
}

#[derive(Debug, Deserialize)]
pub struct BuiltinModel {
    pub model_id: Option<String>,
    pub model_name: Option<String>,
    /// 计费倍率（时段费率），`ratio_display` 如 `"0.7x"`。
    #[serde(default)]
    pub credit: Vec<CreditSlot>,
    pub enable: Option<bool>,
    /// 上下文/输出上限与图片能力（chat 实现时使用）。
    #[allow(dead_code)]
    pub context_window: Option<i64>,
    #[allow(dead_code)]
    pub max_tokens: Option<i64>,
    #[allow(dead_code)]
    pub supports_images: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct CreditSlot {
    /// 倍率数字（如 `"0.05"`）。
    #[serde(default)]
    pub ratio: Option<String>,
    /// 展示倍率（如 `"0.7x"`，ratio 缺失时的备用来源）。
    #[allow(dead_code)]
    #[serde(default)]
    pub ratio_display: Option<String>,
}

impl BuiltinModel {
    /// 展示名：`model_name` 非空优先，否则回退 `model_id`。
    fn display(&self) -> Option<&str> {
        match self.model_name.as_deref() {
            Some(n) if !n.is_empty() => Some(n),
            _ => self.model_id.as_deref().filter(|s| !s.is_empty()),
        }
    }

    /// 计费倍率：取首个费率时段的 `ratio` 数字。官方支持按时段切换倍率
    /// （`time_slots`），server 侧仅取一个静态值。
    fn rate(&self) -> Option<f64> {
        self.credit
            .first()
            .and_then(|c| c.ratio.as_deref())?
            .parse()
            .ok()
    }
}

/// 拉取内置模型目录并解析（独立函数便于单测与错误传播）。
pub async fn fetch_models(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<Vec<ModelInfo>, ProviderError> {
    let url = format!("{SNAP_GATEWAY}{BUILTIN_PATH}");
    // `X-Security-Token` 必须参与签名（内核 `vD`/`signSimple`：accessToken →
    // `X-Security-Token`）；payload hash 是 `UNSIGNED-PAYLOAD`，不是空体 SHA256。
    let headers = vec![
        ("content-type", "application/json".to_string()),
        ("agent-type", "PromptCenter".to_string()),
        ("x-language", "zh-cn".to_string()),
        ("x-security-token", cred.security_token.clone()),
    ];
    let req = sign::Request {
        method: "GET",
        url: &url,
        headers,
        query: None,
        body: None,
    };
    let signed = sign::sign_unsigned(&req, &cred.access_key_id, &cred.secret_access_key)
        .map_err(|e| ProviderError::Other(format!("AKSK 签名失败: {e}")))?;

    let mut http = client
        .get(&url)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS));
    for (k, v) in &signed {
        http = http.header(k.as_str(), v.as_str());
    }

    let resp = http.send().await?;
    let status = resp.status().as_u16();
    let body = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(ProviderError::Protocol(format!(
            "builtin models: HTTP {status}: {}",
            truncate(&body)
        )));
    }

    let catalog: BuiltinCatalog = serde_json::from_str(&body).map_err(|e| {
        ProviderError::Protocol(format!(
            "builtin models: 解析失败: {e}: {}",
            truncate(&body)
        ))
    })?;
    let models = to_models(&catalog);
    if models.is_empty() {
        return Err(ProviderError::Protocol(format!(
            "builtin models: 目录为空（count={:?}）",
            catalog.count
        )));
    }
    tracing::debug!("[codearts] PromptCenter builtin 模型 {} 个", models.len());
    Ok(models)
}

/// 目录 → `ModelInfo`：过滤 `enable === false` / 缺 `model_id`，去重保序。
/// `supports_images` 不过滤（文本对话不依赖）。
pub fn to_models(catalog: &BuiltinCatalog) -> Vec<ModelInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for m in &catalog.builtin_models {
        let Some(id) = m.model_id.as_deref().filter(|s| !s.is_empty()) else {
            continue;
        };
        if m.enable == Some(false) {
            continue;
        }
        if !seen.insert(id.to_string()) {
            continue;
        }
        out.push(ModelInfo {
            model: id.to_string(),
            display_name: m.display().map(|s| s.to_string()),
            rate: m.rate(),
        });
    }
    out
}

fn truncate(s: &str) -> String {
    if s.len() <= 300 {
        return s.to_string();
    }
    let mut end = 300;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_models_filters_disabled_and_dedups() {
        let catalog: BuiltinCatalog = serde_json::from_str(
            r#"{
                "count": 4,
                "builtinModels": [
                    {"model_id": "m1", "model_name": "Model One",
                     "credit": [{"ratio": "0.05", "ratio_display": "0.7x"}]},
                    {"model_id": "m1", "model_name": "dup"},
                    {"model_id": "m2", "enable": false},
                    {"model_name": "no-id"},
                    {"model_id": "m3", "model_name": ""}
                ]
            }"#,
        )
        .expect("解析失败");
        let models = to_models(&catalog);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model, "m1");
        assert_eq!(models[0].display_name.as_deref(), Some("Model One"));
        assert_eq!(models[0].rate, Some(0.05));
        assert_eq!(models[1].model, "m3");
        // display_name 回退到 model_id。
        assert_eq!(models[1].display_name.as_deref(), Some("m3"));
    }

    #[test]
    fn real_response_shape_parses() {
        // 2026-02-18 实测响应片段（截取前两个模型）。
        let raw = r#"{"count":12,"builtinModels":[{"model_id":"GLM-5.2","model_name":"GLM-5.2","model_category":"文本生成","enable":true,"display_enabled":true,"model_desc":"最新旗舰模型","model_sku":null,"provider":"internal","channel_source":"codearts","credit":[{"input_from":0,"input_to":-1,"output_from":0,"output_to":-1,"ratio":"0.05","input_price":"8","cache_price":"2","output_price":"28","status":"0","ratio_display":"0.7x","time_slot_id":null,"before_display_ratio":null}],"time_slots":{},"intervals":[],"supports_images":false,"max_tokens":65536,"context_window":202752},{"model_id":"openpangu-2.0-flash","model_name":"OpenPangu-2.0-Flash","model_category":"文本生成","enable":true,"display_enabled":true,"model_desc":"均衡推理效果与性能","model_sku":null,"provider":"internal","channel_source":"codearts","credit":[{"input_from":0,"input_to":-1,"output_from":0,"output_to":-1,"ratio":"0.011","input_price":"0.8","cache_price":"0.2","output_price":"1.6","status":"0","ratio_display":"0.32x","time_slot_id":null,"before_display_ratio":null}],"time_slots":{},"intervals":[],"supports_images":false,"max_tokens":65535,"context_window":524288}]}"#;
        let catalog: BuiltinCatalog = serde_json::from_str(raw).expect("解析失败");
        assert_eq!(catalog.count, Some(12));
        assert_eq!(catalog.builtin_models.len(), 2);
        let models = to_models(&catalog);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].model, "GLM-5.2");
        assert_eq!(models[0].rate, Some(0.05));
        assert_eq!(models[1].model, "openpangu-2.0-flash");
        assert_eq!(models[1].rate, Some(0.011));
    }
}
