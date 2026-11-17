//! benefit-service：**福利模型额度**（`maas_type: benefit`）的网关配置。
//!
//! # 这不是签到
//!
//! 本模块只负责一件事：拉取 `GET /api/v1/gateway/config`，即「福利模型」的模型表。
//! 它与 UI 上的「限时福利 → 每日签到领 100 积分」**完全无关**——签到走
//! [`super::ops`]（`/v1/ops/*`），余额也走 `ops::fetch_balance`
//! （`/snap-manager/v1/statistics/plugin`）。
//!
//! 早期实现曾把 benefit-service 的 `POST /api/v1/benefit/claim` 当作「签到」、
//! `GET /api/v1/user/tokens/balance` 当作「余额」，二者都是误判，已删除。
//! 真实语义（kernel `tTn()` 加载福利模型）：
//!
//! ```text
//! mD()  GET  /api/v1/gateway/config    → 有 models 才继续
//! aD()  POST /api/v1/benefit/claim     → 失败则「跳过全部福利模型」并 log
//!                                         claimBenefit failed, skip loading benefit models
//!       成功则把 config.models 并入模型表，标记 is_free_benefit
//! ```
//!
//! 即 `claim` 是「福利模型渠道能否加载」的前置开关，不是积分签到。
//!
//! # 端点
//!
//! ```text
//! GET {benefitBase}/api/v1/gateway/config     （渠道配置，含福利模型表）
//! ```
//!
//! `benefitBase` 生产固定 `https://opengw.developer.huaweicloud.com`
//! （`snapengine-apig.cn-north-7` 仅 HC 内部环境）。
//!
//! # 头与签名
//!
//! `Content-Type: application/json` + `x-snap-traceid`（uuid 无横线）+
//! `X-Security-token` → SDK-HMAC-SHA256（[`super::sign`]）。
//!
//! 与 [`super::ops`] 的差异：**query 不参与签名**（官方 `y2i` 的 `HttpRequest`
//! 不解析 query，canonical query 恒为空；实测两种写法服务端都接受）。
//!
//! # 语义（实测 2026-11-16）
//!
//! 成功判定 `error_code === "0000"`（HTTP 恒 200，业务码在 body）。
//! 本账号对该端点恒返回
//! `{"error_code":"4000","error_msg":"invalid channel, group not found"}`
//! ——福利模型渠道未开通，`fetch_config` 视为 `Ok(None)`（不是故障）。
//!
//! > ⚠️ 福利模型接入 chat 仍受 `InferHub.APIG.0101.404` 阻塞（需 `maas_type: benefit`
//! > 头与额外签名），`provider-info/codearts.md` §5.2。本模块目前无调用点。

use serde::Deserialize;

use crate::core::provider::ProviderError;
use crate::providers::codearts::credential::Credential;
use crate::providers::codearts::pkce;
use crate::providers::codearts::sign;

/// benefit 网关生产域名（内核 `pFn` / 扩展 `Dce()` 生产分支）。
pub const BASE_URL: &str = "https://opengw.developer.huaweicloud.com";
/// 端点前缀。生产恒为 [`BASE_URL`]；仅在单元测试里替换为本地 mock。
const ENV_BASE_URL: &str = "OFM_CODEARTS_BENEFIT_BASE";
/// HC 内部域名（扩展 `Oz()` gamma 版 / 内网构建）。公网不可达，仅对照保留。
#[allow(dead_code)]
pub const BASE_URL_INTERNAL: &str = "https://snapengine-apig.cn-north-7.myhuaweicloud.com";

/// 渠道配置（含福利模型表）。
pub const CONFIG_PATH: &str = "/api/v1/gateway/config";

/// 福利渠道（IDE `SLc` 的三个渠道；kernel 只走 `benefit`）。
///
/// 仅 `fetch_config` 用；后者当前无调用点，故构造点暂缺。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Benefit,
    Education,
    Examination,
}

#[allow(dead_code)]
impl Channel {
    /// 线上取值：`benefit` 渠道**不带** channel 参数（官方 GET 与 POST 均不带）。
    /// 另两个渠道以 `channel=<值>` 走 query。
    fn query(self) -> Option<&'static str> {
        match self {
            Channel::Benefit => None,
            Channel::Education => Some("channel=education"),
            Channel::Examination => Some("channel=examination"),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Channel::Benefit => "benefit",
            Channel::Education => "education",
            Channel::Examination => "examination",
        }
    }
}

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
    /// 业务错误 → `ProviderError`，并给 `4000` 补上可读解释。
    fn error(&self, code: &str) -> ProviderError {
        let msg = self.error_msg.clone().unwrap_or_default();
        match code {
            "4000" => ProviderError::Protocol(format!(
                "benefit: 4000 {msg}（福利模型渠道未开通；只影响福利模型额度，与每日签到无关）"
            )),
            "4001" => ProviderError::Unauthorized(format!("benefit: 4001 {msg}")),
            "4002" => ProviderError::Unauthorized(format!("benefit: 4002 {msg}（凭证过期）")),
            "4003" => ProviderError::Other(format!("benefit: 4003 {msg}（用户被禁用）")),
            "4004" => ProviderError::Protocol(format!("benefit: 4004 {msg}（权益不存在）")),
            "4006" => ProviderError::Other(format!("benefit: 4006 {msg}（今日名额耗尽）")),
            _ => ProviderError::Protocol(format!("benefit: {code} {msg}")),
        }
    }
}

/// 构造签名后的头（复刻内核 `PFn`）。
///
/// `X-Security-token` 必须在**签名前**加入头集合：官方签名器对
/// `SignedHeaders` 内的每个键都重算值，缺该头会被网关按 AK 校验失败
/// （实测 `401 APIG.0301 ... access key is not found`）。
fn signed_headers(
    url: &str,
    method: &str,
    cred: &Credential,
) -> Result<Vec<(String, String)>, ProviderError> {
    let mut headers = vec![
        ("content-type", "application/json".to_string()),
        ("x-snap-traceid", pkce::uuid_v4().replace('-', "")),
    ];
    if !cred.security_token.is_empty() {
        headers.push(("x-security-token", cred.security_token.clone()));
    }
    let req = sign::Request {
        method,
        url,
        headers,
        query: None,
        body: None,
    };
    let mut signed = sign::sign(&req, &cred.access_key_id, &cred.secret_access_key)
        .map_err(|e| ProviderError::Other(format!("AKSK 签名失败: {e}")))?;
    // 签名器返回的是小写键，改用线上惯用写法发送（签名与发送头一一对应）。
    for (k, _) in signed.iter_mut() {
        if k == "x-security-token" {
            *k = "X-Security-token".to_string();
        }
    }
    Ok(signed)
}

/// benefit 请求公共部分：签名头 + 超时 + 状态码/壳解析。
async fn send(
    client: &reqwest::Client,
    cred: &Credential,
    path: &str,
    query: Option<&str>,
) -> Result<Envelope, ProviderError> {
    let base = std::env::var(ENV_BASE_URL)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| BASE_URL.to_string());
    let base = base.as_str();
    let mut url = format!("{base}{path}");
    if let Some(q) = query {
        url.push('?');
        url.push_str(q);
    }
    // query 不参与签名（见模块注释）。
    let unsigned_url = format!("{base}{path}");
    let headers = signed_headers(&unsigned_url, "GET", cred)?;
    let mut http = client
        .request(reqwest::Method::GET, &url)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS));
    for (k, v) in &headers {
        http = http.header(k.as_str(), v.as_str());
    }
    // 注意：content-type 已在签名头里，不能再补一次（重复头会导致服务端
    // 签名重算值不符 → APIG.0301）。
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

/// 福利模型渠道配置（`GET /api/v1/gateway/config`，可带 channel）。
///
/// 返回 `result` 原样（含 `models` 表）；渠道未开通（`4000`/`4004`）返回 `Ok(None)`。
///
/// 目前**无调用点**：福利模型接入 chat 仍受 `InferHub.APIG.0101.404` 阻塞
/// （需 `maas_type: benefit`），见 `provider-info/codearts.md` §5.2。
#[allow(dead_code)]
pub async fn fetch_config(
    client: &reqwest::Client,
    cred: &Credential,
    channel: Channel,
) -> Result<Option<serde_json::Value>, ProviderError> {
    let env = send(client, cred, CONFIG_PATH, channel.query()).await?;
    match env.error_code.as_deref() {
        Some("0000") => Ok(env.result),
        Some("4000") | Some("4004") => {
            tracing::debug!(
                "[codearts] {} 渠道未开通: {:?}",
                channel.label(),
                env.error_msg
            );
            Ok(None)
        }
        Some(code) => Err(env.error(code)),
        None => Err(ProviderError::Protocol(
            "benefit: 配置响应缺 error_code".to_string(),
        )),
    }
}

/// 截断长文本（错误信息用）。
fn truncate(s: &str) -> String {
    const MAX: usize = 300;
    if s.chars().count() <= MAX {
        return s.to_string();
    }
    let cut: String = s.chars().take(MAX).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cred() -> Credential {
        Credential {
            access_key_id: "AKTEST".to_string(),
            secret_access_key: "SKTEST".to_string(),
            security_token: "TOKTEST".to_string(),
            refresh_token: "RT".to_string(),
            expire_at_millis: Some(4_000_000_000_000),
            code_verifier: "v".to_string(),
            dpop_key_pair: crate::providers::codearts::dpop::KeyPair::generate()
                .expect("生成测试密钥失败"),
            last_auth_at: Some(0),
        }
    }

    fn envelope(raw: &str) -> Envelope {
        serde_json::from_str(raw).expect("解析响应壳失败")
    }

    #[test]
    fn envelope_error_msg_surfaced() {
        let env = envelope(r#"{"error_code":"4000","error_msg":"invalid channel"}"#);
        let err = env.error("4000").to_string();
        assert!(err.contains("invalid channel"), "{err}");
        // 4000 只影响福利模型，文案不得把它说成「签到」（「与每日签到无关」除外）。
        assert!(err.contains("福利模型"), "{err}");
        assert!(err.contains("与每日签到无关"), "{err}");
    }

    #[test]
    fn envelope_success_allows_null_result() {
        let env = envelope(r#"{"error_code":"0000","result":null}"#);
        assert_eq!(env.error_code.as_deref(), Some("0000"));
        assert!(env.result.is_none());
    }

    #[test]
    fn benefit_channel_has_no_query() {
        assert_eq!(Channel::Benefit.query(), None);
        assert_eq!(Channel::Education.query(), Some("channel=education"));
        assert_eq!(Channel::Examination.query(), Some("channel=examination"));
    }

    /// 签名头必须带 `X-Security-token`，且 query 不参与签名（签名值相同）。
    #[test]
    fn signed_headers_shape() {
        let url = format!("{BASE_URL}{CONFIG_PATH}");
        let h = signed_headers(&url, "GET", &test_cred()).expect("签名失败");
        let get = |name: &str| {
            h.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("X-Security-token"), Some("TOKTEST"));
        assert!(
            get("authorization")
                .unwrap()
                .starts_with("SDK-HMAC-SHA256 Access=AKTEST")
        );
        assert!(get("x-snap-traceid").is_some(), "缺 x-snap-traceid");
    }
}
