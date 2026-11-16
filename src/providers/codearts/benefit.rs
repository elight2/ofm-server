//! benefit-service「权益领取 / 余额」复刻（内核 `_D`/`aD`，`agentkernelServer-linux-x64`
//! @101232800–101234299）。
//!
//! # 端点（独立网关，与 chat 域名不同）
//!
//! ```text
//! POST {benefitBase}/api/v1/benefit/claim        （签到；空 body `{}`）
//! GET  {benefitBase}/api/v1/user/tokens/balance  （余额）
//! ```
//!
//! `benefitBase`：生产固定 `https://opengw.developer.huaweicloud.com`
//! （内核内部备用域名 `snapengine-apig.cn-north-7` 仅 `IS_PROD!=="true"` 的 HC
//! 内部环境生效，公网不可达）。
//!
//! # 头与签名
//!
//! `PFn()`：`Content-Type: application/json` + `x-snap-traceid`（uuid 无横线）+
//! `X-Security-token`（若有）→ SDK-HMAC-SHA256（`sign` 模块）。
//!
//! # 语义（实测 2026-02-18）
//!
//! 成功判定 `error_code === "0000"`（HTTP 恒 200，业务码在 body）。
//! **未开通权益的账号三端点恒报 `{"error_code":"4000","error_msg":"invalid
//! channel, group not found"}`**（已排除 channel 参数 / 头 / body 变量）。
//! 注意响应错误字段名是 `error_msg`（无 `error_message`）。
//! 余额字段名按 webview 渲染层取证：benefit/examination 为
//! `used_amount` / `total_quota`（education 由 `monthly_tokens_used` /
//! `monthly_token_limit` 映射）。对外展示取「剩余额度 = total − used」；
//! 字段缺失时返回 `None`（`status` 显示「上游未提供」）。
//! `4006` = 今日已签（`daily claim limit exhausted`）→ `skipped`，不是错误。

use serde::Deserialize;

use crate::core::provider::{ClaimReport, ProviderError};
use crate::providers::codearts::credential::Credential;
use crate::providers::codearts::pkce;
use crate::providers::codearts::sign;

/// benefit 网关生产域名（内核 `pFn`）。
pub const BASE_URL: &str = "https://opengw.developer.huaweicloud.com";
/// 签到路径（内核 `kFn.claim`）。
pub const CLAIM_PATH: &str = "/api/v1/benefit/claim";
/// 余额路径（内核 `kFn.balance`）。
pub const BALANCE_PATH: &str = "/api/v1/user/tokens/balance";

/// 请求超时（内核 `setTimeout(…, 1e4)`）。
const TIMEOUT_SECS: u64 = 10;

/// benefit 统一响应壳。错误字段实测为 `error_msg`。
#[derive(Debug, Deserialize)]
pub struct Envelope {
    pub error_code: Option<String>,
    pub error_msg: Option<String>,
    pub result: Option<serde_json::Value>,
}

impl Envelope {
    /// 成功判定（内核 `error_code === "0000"`），取 `result`。
    fn into_result(self) -> Result<serde_json::Value, ProviderError> {
        match self.error_code.as_deref() {
            Some("0000") => self.result.ok_or_else(|| {
                ProviderError::Protocol("benefit: error_code=0000 但缺 result".to_string())
            }),
            Some(code) => {
                let msg = self.error_msg.unwrap_or_default();
                Err(ProviderError::Protocol(format!("benefit: {code} {msg}")))
            }
            None => Err(ProviderError::Protocol(
                "benefit: 响应缺 error_code".to_string(),
            )),
        }
    }
}

/// 余额 `result`（字段名按 webview 渲染层取证）。
#[derive(Debug, Default, Deserialize)]
pub struct BalanceResult {
    #[serde(default)]
    pub used_amount: Option<f64>,
    #[serde(default)]
    pub total_quota: Option<f64>,
    /// education 渠道字段；benefit/examination 不出现，防御性兼容。
    #[serde(default)]
    pub monthly_tokens_used: Option<f64>,
    #[serde(default)]
    pub monthly_token_limit: Option<f64>,
    /// 渠道标记（`benefit`/`education`/`examination`），仅存档不参与计算。
    #[allow(dead_code)]
    #[serde(default)]
    pub maas_type: Option<String>,
}

impl BalanceResult {
    /// 剩余额度 = 总量 − 已用（两字段齐才可算，缺一返回 `None`）。
    fn remaining(&self) -> Option<f64> {
        let used = self.used_amount.or(self.monthly_tokens_used)?;
        let total = self.total_quota.or(self.monthly_token_limit)?;
        Some(total - used)
    }
}

/// 构造签名后的头（复刻内核 `PFn`）；POST body 参与签名（见 `sign::Request::body`）。
fn signed_headers(
    url: &str,
    method: &str,
    cred: &Credential,
    body: Option<&str>,
) -> Result<Vec<(String, String)>, ProviderError> {
    let headers = vec![
        ("content-type", "application/json".to_string()),
        ("x-snap-traceid", pkce::uuid_v4().replace('-', "")),
    ];
    let req = sign::Request {
        method,
        url,
        headers,
        query: None,
        body,
    };
    let mut signed = sign::sign(&req, &cred.access_key_id, &cred.secret_access_key)
        .map_err(|e| ProviderError::Other(format!("AKSK 签名失败: {e}")))?;
    if !cred.security_token.is_empty() {
        signed.push(("X-Security-Token".to_string(), cred.security_token.clone()));
    }
    Ok(signed)
}

/// benefit 请求公共部分：签名头 + 超时 + 状态码/壳解析。
/// body 以序列化后的文本传入并原样发送（签名与实际报文一致，避免
/// serde 二次序列化差异导致 payload hash 不符）。
async fn send(
    client: &reqwest::Client,
    cred: &Credential,
    method: reqwest::Method,
    url: &str,
    body: Option<&str>,
) -> Result<Envelope, ProviderError> {
    let headers = signed_headers(url, method.as_str(), cred, body)?;
    let mut http = client
        .request(method, url)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS));
    for (k, v) in &headers {
        http = http.header(k.as_str(), v.as_str());
    }
    // 注意：content-type 已在签名头里，不能再补一次（重复头会导致服务端
    // 签名重算值不符 → APIG.0301）。body 用与签名相同的文本原样发送。
    if let Some(t) = body {
        http = http.body(t.to_string());
    }
    let resp = http.send().await?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(ProviderError::Protocol(format!(
            "benefit: HTTP {status}: {}",
            truncate(&text)
        )));
    }
    serde_json::from_str(&text).map_err(|e| {
        ProviderError::Protocol(format!("benefit: 解析失败: {e}: {}", truncate(&text)))
    })
}

/// 签到（内核 `aD` → `_D("claim","POST")`，body `{}`）。
///
/// `4006`（今日已签）→ `skipped: true`；成功/跳过后读一次余额供 `claim.log` 对账。
pub async fn claim(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<ClaimReport, ProviderError> {
    let url = format!("{BASE_URL}{CLAIM_PATH}");
    let env = send(client, cred, reqwest::Method::POST, &url, Some("{}")).await?;
    match env.error_code.as_deref() {
        Some("0000") => Ok(ClaimReport {
            skipped: false,
            message: "权益领取成功".to_string(),
            credits_delta: None,
            balance: fetch_balance(client, cred).await.ok().flatten(),
        }),
        Some("4006") => Ok(ClaimReport {
            skipped: true,
            message: "今日已领取（daily claim limit exhausted）".to_string(),
            credits_delta: None,
            balance: fetch_balance(client, cred).await.ok().flatten(),
        }),
        Some(code) => Err(ProviderError::Protocol(format!(
            "claim: {code} {}",
            env.error_msg.unwrap_or_default()
        ))),
        None => Err(ProviderError::Protocol(
            "claim: 响应缺 error_code".to_string(),
        )),
    }
}

/// 余额（内核 `_D("balance","GET")`）。返回剩余额度（总量 − 已用）；
/// 字段缺失返回 `Ok(None)`。账号未开通权益时上游报 4000，原样转错误便于排障。
pub async fn fetch_balance(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<Option<f64>, ProviderError> {
    let url = format!("{BASE_URL}{BALANCE_PATH}");
    let env = send(client, cred, reqwest::Method::GET, &url, None).await?;
    let result = env.into_result()?;
    let parsed: BalanceResult =
        serde_json::from_value(result).map_err(|e| ProviderError::Protocol(format!(
            "benefit: 余额字段解析失败: {e}"
        )))?;
    Ok(parsed.remaining())
}

fn truncate(s: &str) -> String {
    if s.len() <= 300 {
        s.to_string()
    } else {
        format!("{}…", &s[..300])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_error_msg_surfaced() {
        // 实测错误形态（2026-02-18）。
        let env: Envelope = serde_json::from_str(
            r#"{"error_code":"4000","error_msg":"invalid channel, group not found","result":null}"#,
        )
        .expect("解析失败");
        let err = env.into_result().unwrap_err().to_string();
        assert!(err.contains("4000"), "{err}");
        assert!(err.contains("invalid channel"), "{err}");
    }

    #[test]
    fn balance_remaining_uses_webview_field_names() {
        let b: BalanceResult = serde_json::from_str(
            r#"{"used_amount": 1234.0, "total_quota": 10000.0, "maas_type": "benefit"}"#,
        )
        .expect("解析失败");
        assert_eq!(b.remaining(), Some(8766.0));
    }

    #[test]
    fn balance_falls_back_to_education_fields() {
        let b: BalanceResult = serde_json::from_str(
            r#"{"monthly_tokens_used": 100.0, "monthly_token_limit": 500.0}"#,
        )
        .expect("解析失败");
        assert_eq!(b.remaining(), Some(400.0));
    }

    #[test]
    fn balance_missing_field_is_none() {
        let b: BalanceResult =
            serde_json::from_str(r#"{"used_amount": 100.0}"#).expect("解析失败");
        assert_eq!(b.remaining(), None);
    }
}
