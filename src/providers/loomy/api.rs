//! Loomy HTTP 客户端与共享常量。
//!
//! 模型侧（对话 / 目录 / 积分）走讯飞网关 `loomyad.xunfei.cn`，**不是**
//! `api.loomy.ai`（那个域名只用于客户端升级检查 `/api/v1/version/check`，
//! 见 `provider-info/loomy.md` §0）。
//!
//! # 两套错误形态
//!
//! 上游同一个网关上有两种完全不同的错误表达，必须分别处理：
//!
//! * **积分类接口**（`/points/*`、`/pet-work*`、`/models`）：恒 HTTP 200 +
//!   `{"code":"000000","desc":"成功","data":{...}}` 信封；`code != "000000"`
//!   即失败，`desc` 是中文文案。
//! * **对话接口**（`/chat/completions`）：认证失败仍是 **HTTP 200** +
//!   `{"code":"100002","desc":"登录已失效，请重新登录"}`（非 SSE 帧）；
//!   参数错误是 HTTP 400 + `{"error":{"message":...}}`。

use reqwest::Client;

/// 构建 HTTP 客户端。
///
/// 对话最长可能挂很久（上游推理模型首帧可达几十秒），故不设总超时，
/// 只设连接超时避免挂死。
pub fn client() -> Result<Client, reqwest::Error> {
    Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
}

/// 客户端版本（`loomy-version` 请求头）。
pub const VERSION: &str = "0.9.39";

/// 模型 API 基地址。
pub const MODEL_BASE_URL: &str = "https://loomyad.xunfei.cn/api/v1";

/// 信封成功码。
pub const OK_CODE: &str = "000000";

/// 积分/宠物打工接口的统一信封。
#[derive(Debug, serde::Deserialize)]
// 手写 bound：`#[serde(default)]` 的 `Option<T>` 字段会让 derive 误加 `T: Default`。
#[serde(bound(deserialize = "T: serde::Deserialize<'de>"))]
pub struct Envelope<T> {
    /// 业务码，`000000` 为成功。
    #[serde(default)]
    pub code: String,
    /// 中文描述（失败时即错误文案）。
    #[serde(default)]
    pub desc: String,
    /// 业务数据。
    #[serde(default)]
    pub data: Option<T>,
}

impl<T> Envelope<T> {
    /// 取出成功时的数据；业务失败时返回中文错误。
    pub fn into_data(self) -> Result<T, String> {
        if self.code.is_empty() || self.code == OK_CODE {
            return self
                .data
                .ok_or_else(|| "上游响应缺少 data 字段".to_string());
        }
        Err(format!("{}: {}", self.code, self.desc))
    }
}

/// 解析业务错误信封（`code`/`desc`）。
///
/// 返回 `None` 表示这不是错误信封。
pub fn biz_error(v: &serde_json::Value) -> Option<(String, String)> {
    let code = v.get("code").and_then(|c| c.as_str())?;
    if code == OK_CODE {
        return None;
    }
    let desc = v
        .get("desc")
        .and_then(|d| d.as_str())
        .unwrap_or("未知错误")
        .to_string();
    Some((code.to_string(), desc))
}

/// 信封类接口的错误串（`"<code>: <中文 desc>"`）。
///
/// 用 `String` 而非自定义枚举：调用方一律把它包成 [`crate::core::provider::ProviderError::Protocol`]，
/// 不必再区分传输错误与业务错误——上游业务错误也走 HTTP 200。
pub type EnvelopeErr = String;

/// 发请求并按信封解出 `data`。
///
/// 网络失败与「HTTP 非 2xx」都归为非信封错误（业务码只在 200 下可信）。
pub async fn get_envelope<T>(http: reqwest::RequestBuilder) -> Result<T, EnvelopeErr>
where
    T: serde::de::DeserializeOwned,
{
    let resp = http.send().await.map_err(|e| format!("请求失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("读取响应失败: {e}"))?;
    parse_envelope_text::<T>(status.as_u16(), &text)
}

/// 从响应体文本解信封（拆出来便于单测）。
pub fn parse_envelope_text<T>(status: u16, text: &str) -> Result<T, EnvelopeErr>
where
    T: serde::de::DeserializeOwned,
{
    let env: Envelope<T> = serde_json::from_str(text)
        .map_err(|e| format!("解析响应失败(HTTP {status}): {e}: {}", truncate(text, 200)))?;
    if status >= 400 && env.code.is_empty() {
        return Err(format!("上游 HTTP {status}: {}", truncate(text, 200)));
    }
    env.into_data()
}

/// 截断到 `max` 字节（按字符边界），便于放进错误信息。
pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 成功信封取数据() {
        let env: Envelope<serde_json::Value> =
            serde_json::from_str(r#"{"code":"000000","desc":"成功","data":{"balance":1}}"#)
                .unwrap();
        assert_eq!(env.into_data().unwrap()["balance"], 1);
    }

    #[test]
    fn 失败信封带中文文案() {
        let env: Envelope<serde_json::Value> =
            serde_json::from_str(r#"{"code":"100002","desc":"登录已失效"}"#).unwrap();
        assert_eq!(env.into_data().unwrap_err(), "100002: 登录已失效");
    }

    #[test]
    fn 识别业务错误信封() {
        let v: serde_json::Value =
            serde_json::from_str(r#"{"code":"210105","desc":"验证码错误"}"#).unwrap();
        assert_eq!(biz_error(&v).unwrap().0, "210105");
        let ok: serde_json::Value = serde_json::from_str(r#"{"code":"000000"}"#).unwrap();
        assert!(biz_error(&ok).is_none());
        assert!(biz_error(&serde_json::json!({"data":[]})).is_none());
    }

    #[test]
    fn 解析信封文本() {
        let v: serde_json::Value =
            parse_envelope_text(200, r#"{"code":"000000","data":{"a":1}}"#).unwrap();
        assert_eq!(v["a"], 1);
        // 非 2xx 且无业务码 → 归为 HTTP 错误。
        let e = parse_envelope_text::<serde_json::Value>(500, "boom").unwrap_err();
        assert!(e.contains("HTTP 500"), "实际: {e}");
    }

    #[test]
    fn 截断按字符边界() {
        assert_eq!(truncate("你好世界", 0), "…");
        assert_eq!(truncate("你好世界", 6), "你好…");
        assert_eq!(truncate("abc", 10), "abc");
    }
}
