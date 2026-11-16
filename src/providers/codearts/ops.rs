//! 限时福利 / 每日签到（`/v1/ops/*`，snap-access 网关）。
//!
//! 这才是 IDE 里「限时福利 → 每日签到领 100 积分」的真实协议，
//! 与 [`super::benefit`]（benefit-service，福利模型额度）**完全无关**。
//!
//! # 端点（`provider-info/codearts.md` §7.2）
//!
//! ```text
//! GET  {base}/v1/ops/delivery?channel=IDE            活动列表
//! GET  {base}/v1/ops/tasks/conversation?channel=IDE  会话任务
//! POST {base}/v1/ops/claim                           领取
//! POST {base}/v1/ops/confirm                         到账确认
//! POST {base}/v1/ops/invite/generate                 邀请码
//! POST {base}/v1/ops/events/trigger                  事件上报
//! GET  {base}/v1/ops/credit/usage?type=&id=          积分用量
//! ```
//!
//! `base` = `product.json` 的 `snapEngineUrl`（workbench）/ `policyCenterDomain`
//! （扩展），商用版均为 `https://snap-access.cn-north-4.myhuaweicloud.com`
//! （国际站 `ap-southeast-1`）。
//!
//! # 认证（实测 2026-11-16）
//!
//! 只需 **AKSK SDK-HMAC-SHA256 签名**，不需要 `x-auth-token`。
//! 客户端 `Tr()` 在商用版返回 `""`（`isDefaultToken=false` /
//! `isInnerVersion=false`），`x-auth-token` 仅在 `scenario===HW`（华为内网）
//! 时由 `getW3AuthHeaders` 追加，商用版根本不发。
//!
//! 必备头：
//! * `Agent-Type`（如 `PromptCenter`）——**缺失返回
//!   `400 TM.00001001 请求头Agent-Type为空`**，是唯一的业务头硬要求；
//! * `X-Language`（`zh-cn`/`en-us`）；
//! * `x-snap-traceid`、`Content-Type`、`X-Security-Token`（签名）。
//!
//! 与 benefit 的签名差异：**query 必须参与签名**（`channel=IDE` 编码进
//! canonical query）。不签会得 `401 APIG.0301 verify ak sk signature fail`。
//!
//! # 响应
//!
//! `{code, message, data}`，成功判定 `code===0`（另 `status===0` 仅 workbench
//! 侧额外校验）。`data.items[]` 每项含 `campaignId` / `title` / `type` /
//! `status` / `claimable` / `benefitAmount` / `benefitUnit` / `displayConfig`
//! （**JSON 字符串**） / `pendingCount` / `pendingTotalAmount` / `extra`。
//!
//! `status` 枚举：`ELIGIBLE`（可领）→ `CLAIMED`（已领待确认）→
//! `CONFIRMED`（已到账）→ `CONSUMED`（已用完）。空 `null` 表示不满足条件
//! （如学生认证未做）。`claimable` 为服务端给出的「本次可领取」判据。
//!
//! # 领取流程（UI `SLc`）
//!
//! `GET delivery` → 取 `claimable===true` 的 `campaignId` →
//! `POST claim {campaignId, channel:"IDE"}` → `POST confirm {campaignId}`。
//! `claim` 成功返回 `status:"CLAIMED"`（`remainingAmount` 已计入），
//! `confirm` 后才变 `CONFIRMED` 并计入账户积分。

use serde::Deserialize;

use crate::core::provider::{ClaimReport, ProviderError};
use crate::providers::codearts::credential::Credential;
use crate::providers::codearts::pkce;
use crate::providers::codearts::sign;

/// 生产域名（`product.json` 的 `snapEngineDomain` / `policyCenterDomain`）。
pub const BASE_URL: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";
/// 端点前缀。生产恒为 [`BASE_URL`]；仅在单元测试里替换为本地 mock。
const ENV_BASE_URL: &str = "OFM_CODEARTS_OPS_BASE";
/// 国际站域名（`ap-southeast-1`），仅为对照保留。
#[allow(dead_code)]
pub const BASE_URL_INTL: &str = "https://snap-access.ap-southeast-1.myhuaweicloud.com";

/// 活动列表。
pub const DELIVERY_PATH: &str = "/v1/ops/delivery";
/// 领取。
pub const CLAIM_PATH: &str = "/v1/ops/claim";
/// 到账确认。
pub const CONFIRM_PATH: &str = "/v1/ops/confirm";
/// 积分用量（UI 用量页用，本 server 暂不调用）。
#[allow(dead_code)]
pub const USAGE_PATH: &str = "/v1/ops/credit/usage";

/// 渠道（固定 `IDE`）。
const CHANNEL: &str = "IDE";
/// 语言头（服务端按此返回文案）。
const LANG: &str = "zh-cn";

/// 请求超时。
const TIMEOUT_SECS: u64 = 15;

/// `/v1/ops/*` 统一响应壳。
#[derive(Debug, Deserialize)]
pub struct Envelope {
    #[serde(default)]
    pub code: Option<i64>,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub data: Option<serde_json::Value>,
}

impl Envelope {
    fn into_data(self) -> Result<serde_json::Value, ProviderError> {
        match self.code {
            Some(0) => Ok(self.data.unwrap_or(serde_json::Value::Null)),
            other => Err(ProviderError::Protocol(format!(
                "ops: code={other:?} message={}",
                self.message.unwrap_or_default()
            ))),
        }
    }
}

/// `delivery` 里的单个活动。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Campaign {
    pub campaign_id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub claimable: bool,
    #[serde(default)]
    pub benefit_amount: f64,
    #[serde(default)]
    pub benefit_unit: String,
    #[serde(default)]
    pub pending_count: i64,
    #[serde(default)]
    pub pending_total_amount: f64,
    /// 未解析的 `displayConfig` 原始 JSON 字符串（UI 文案，服务端不用）。
    #[serde(default)]
    #[allow(dead_code)]
    pub display_config: Option<String>,
}


/// `delivery` 响应里的 `data`。
#[derive(Debug, Deserialize)]
pub struct Delivery {
    #[serde(default)]
    pub items: Vec<Campaign>,
}

/// 解析 base URL（测试可覆盖）。
fn base_url() -> String {
    std::env::var(ENV_BASE_URL)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| BASE_URL.to_string())
}

/// 签名头。`query` 传入原始查询串（**必须参与签名**，见模块注释）。
fn signed_headers(
    url: &str,
    method: &str,
    query: Option<&str>,
    cred: &Credential,
    body: Option<&str>,
) -> Result<Vec<(String, String)>, ProviderError> {
    let mut headers = vec![
        ("content-type", "application/json".to_string()),
        ("x-snap-traceid", pkce::uuid_v4().replace('-', "")),
        ("agent-type", "PromptCenter".to_string()),
        ("x-language", LANG.to_string()),
    ];
    if !cred.security_token.is_empty() {
        headers.push(("x-security-token", cred.security_token.clone()));
    }
    let req = sign::Request {
        method,
        url,
        headers,
        query,
        body,
    };
    sign::sign(&req, &cred.access_key_id, &cred.secret_access_key)
        .map_err(|e| ProviderError::Other(format!("AKSK 签名失败: {e}")))
}

/// 请求公共部分。
async fn send(
    client: &reqwest::Client,
    cred: &Credential,
    method: reqwest::Method,
    path: &str,
    query: Option<&str>,
    body: Option<&str>,
) -> Result<serde_json::Value, ProviderError> {
    let base = base_url();
    let mut url = format!("{base}{path}");
    if let Some(q) = query {
        url.push('?');
        url.push_str(q);
    }
    // 签名用不带 query 的 URL，query 单列（签名器会重排编码）。
    let unsigned_url = format!("{base}{path}");
    let headers = signed_headers(&unsigned_url, method.as_str(), query, cred, body)?;
    let mut http = client
        .request(method, &url)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS));
    for (k, v) in &headers {
        http = http.header(k.as_str(), v.as_str());
    }
    if let Some(t) = body {
        http = http.body(t.to_string());
    }
    let resp = http.send().await?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(ProviderError::Protocol(format!(
            "ops: HTTP {status}: {}",
            truncate(&text)
        )));
    }
    let env: Envelope = serde_json::from_str(&text).map_err(|e| {
        ProviderError::Protocol(format!("ops: 解析失败: {e}: {}", truncate(&text)))
    })?;
    env.into_data()
}

/// 活动列表（`GET /v1/ops/delivery?channel=IDE`）。
pub async fn fetch_delivery(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<Vec<Campaign>, ProviderError> {
    let query = format!("channel={CHANNEL}");
    let data = send(
        client,
        cred,
        reqwest::Method::GET,
        DELIVERY_PATH,
        Some(&query),
        None,
    )
    .await?;
    let d: Delivery = serde_json::from_value(data).map_err(|e| {
        ProviderError::Protocol(format!("ops: delivery 结构不符: {e}"))
    })?;
    Ok(d.items)
}

/// 领取（`POST /v1/ops/claim`）。
///
/// body 与官方一致：`{"campaignId":N,"channel":"IDE","idempotentKey":"..."}`，
/// 扩展在 `benefitId` 存在时附加，单活动领取不带。
pub async fn claim_campaign(
    client: &reqwest::Client,
    cred: &Credential,
    campaign_id: i64,
) -> Result<serde_json::Value, ProviderError> {
    let key = format!("claim_{campaign_id}_all_{}", now_millis());
    let body = serde_json::json!({
        "campaignId": campaign_id,
        "channel": CHANNEL,
        "idempotentKey": key,
    })
    .to_string();
    send(
        client,
        cred,
        reqwest::Method::POST,
        CLAIM_PATH,
        None,
        Some(&body),
    )
    .await
}

/// 到账确认（`POST /v1/ops/confirm`）。
pub async fn confirm_campaign(
    client: &reqwest::Client,
    cred: &Credential,
    campaign_id: i64,
) -> Result<serde_json::Value, ProviderError> {
    let body = serde_json::json!({ "campaignId": campaign_id }).to_string();
    send(
        client,
        cred,
        reqwest::Method::POST,
        CONFIRM_PATH,
        None,
        Some(&body),
    )
    .await
}

/// 签到入口：拉列表 → 领可领的 → 确认。
///
/// 已领过的活动 `claimable === false`，直接跳过并汇总说明，不算失败。
pub async fn claim(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<ClaimReport, ProviderError> {
    let items = fetch_delivery(client, cred).await?;
    let targets: Vec<&Campaign> = items.iter().filter(|c| c.claimable).collect();
    if targets.is_empty() {
        return Ok(ClaimReport {
            skipped: true,
            message: skip_message(&items),
            credits_delta: None,
            balance: None,
        });
    }

    let mut gained = 0.0_f64;
    let mut max_balance: Option<f64> = None;
    let mut done: Vec<String> = Vec::new();
    for c in &targets {
        let claimed = claim_campaign(client, cred, c.campaign_id).await?;
        let amount = claimed
            .get("totalAmount")
            .and_then(|v| v.as_f64())
            .or_else(|| claimed.get("remainingAmount").and_then(|v| v.as_f64()))
            .unwrap_or(c.benefit_amount);
        let remaining = claimed.get("remainingAmount").and_then(|v| v.as_f64());
        // confirm 失败不影响已领取的钱，但要如实报出来。
        confirm_campaign(client, cred, c.campaign_id).await?;
        gained += amount;
        if let Some(r) = remaining {
            max_balance = Some(max_balance.map_or(r, |b: f64| b.max(r)));
        }
        done.push(format!("{} +{amount} {}", c.title, c.benefit_unit));
    }

    Ok(ClaimReport {
        skipped: false,
        message: format!("领取成功: {}", done.join("; ")),
        credits_delta: Some(gained),
        balance: max_balance,
    })
}

/// 无可领取时的说明：优先报「今日已签」，其次报已到账待用积分。
fn skip_message(items: &[Campaign]) -> String {
    // `CLAIMED`/`CONFIRMED` 里 `USER_LOGIN`（每日签到）说明今天已经签过。
    let today: Vec<&Campaign> = items
        .iter()
        .filter(|c| {
            c.r#type == "USER_LOGIN"
                && matches!(c.status.as_deref(), Some("CLAIMED") | Some("CONFIRMED"))
        })
        .collect();
    if !today.is_empty() {
        let name = if today[0].title.is_empty() {
            "每日签到"
        } else {
            &today[0].title
        };
        return format!("今日已签到（{}）", name);
    }
    let pending: f64 = items.iter().map(|c| c.pending_total_amount).sum();
    if pending > 0.0 {
        return format!("今日无可领取福利（已到账待用 {pending} 积分）");
    }
    let units: Vec<String> = items
        .iter()
        .filter(|c| c.pending_count > 0)
        .map(|c| format!("{} 待领 {}", c.title, c.pending_count))
        .collect();
    if units.is_empty() {
        "今日无可领取福利".to_string()
    } else {
        format!("今日无可领取福利（{}）", units.join("; "))
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

/// 当前毫秒时间戳（`idempotentKey` 用）。
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// p-256 密钥对（仅签名用，不参与 `/v1/ops/*`）。
    fn cred() -> Credential {
        Credential {
            access_key_id: "AK".to_string(),
            secret_access_key: "SK".to_string(),
            security_token: "ST".to_string(),
            refresh_token: "R".to_string(),
            expire_at_millis: None,
            code_verifier: "v".to_string(),
            dpop_key_pair: crate::providers::codearts::dpop::KeyPair::generate().unwrap(),
            last_auth_at: None,
        }
    }

    #[test]
    fn envelope_success_takes_data() {
        let env: Envelope = serde_json::from_str(r#"{"code":0,"message":"ok","data":{"x":1}}"#)
            .unwrap();
        assert_eq!(env.into_data().unwrap()["x"], 1);
    }

    #[test]
    fn envelope_nonzero_is_error() {
        let env: Envelope =
            serde_json::from_str(r#"{"code":4000,"message":"bad"}"#).unwrap();
        let err = env.into_data().unwrap_err().to_string();
        assert!(err.contains("4000"), "{err}");
        assert!(err.contains("bad"), "{err}");
    }

    #[test]
    fn delivery_parses_campaign() {
        let raw = r#"{"items":[{"campaignId":34,"title":"每日签到领100积分",
            "type":"USER_LOGIN","status":"ELIGIBLE","claimable":true,
            "benefitAmount":100.0,"benefitUnit":"CREDIT","pendingCount":1,
            "pendingTotalAmount":100.0,
            "displayConfig":"{\"claimButtonText\":{\"ELIGIBLE\":\"每日领取\"}}"}]}"#;
        let d: Delivery = serde_json::from_str(raw).unwrap();
        assert_eq!(d.items.len(), 1);
        let c = &d.items[0];
        assert_eq!(c.campaign_id, 34);
        assert!(c.claimable);
        assert_eq!(c.status.as_deref(), Some("ELIGIBLE"));
    }

    #[test]
    fn campaign_defaults_when_fields_missing() {
        let c: Campaign =
            serde_json::from_str(r#"{"campaignId":1,"displayConfig":null}"#).unwrap();
        assert!(!c.claimable);
        assert_eq!(c.status, None);
    }

    /// 已签到的每日签到活动要能识别出来（决定「今日已签到」文案）。
    #[test]
    fn skip_message_reports_already_signed() {
        let items: Vec<Campaign> = serde_json::from_str(
            r#"[{"campaignId":34,"title":"每日签到领100积分","type":"USER_LOGIN",
                 "status":"CONFIRMED","claimable":false},
                {"campaignId":4,"title":"新人礼","type":"NEW_USER_REGISTER",
                 "status":"CLAIMED","claimable":false,"pendingTotalAmount":4000.0}]"#,
        )
        .unwrap();
        let m = skip_message(&items);
        assert!(m.contains("今日已签到"), "{m}");
    }

    /// 没有签到活动时退化为「待领/待用」说明。
    #[test]
    fn skip_message_without_login_campaign() {
        let items: Vec<Campaign> = serde_json::from_str(
            r#"[{"campaignId":2,"type":"STUDENT_CERTIFIED","status":null,
                 "claimable":false,"pendingCount":0}]"#,
        )
        .unwrap();
        assert_eq!(skip_message(&items), "今日无可领取福利");
    }

    /// 请求头必须带 `Agent-Type`（缺则 400）与 `x-language`。
    #[test]
    fn signed_headers_carry_agent_type() {
        let h = signed_headers(
            "https://snap-access.cn-north-4.myhuaweicloud.com/v1/ops/delivery",
            "GET",
            Some("channel=IDE"),
            &cred(),
            None,
        )
        .unwrap();
        let get = |name: &str| {
            h.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("agent-type"), Some("PromptCenter"));
        assert_eq!(get("x-language"), Some("zh-cn"));
        assert_eq!(get("x-security-token"), Some("ST"));
        let auth = get("authorization").expect("缺 Authorization");
        assert!(auth.starts_with("SDK-HMAC-SHA256 Access=AK"), "{auth}");
        // query 必须进签名：SignedHeaders 不含 query，但签名值要随 query 变化。
        let h2 = signed_headers(
            "https://snap-access.cn-north-4.myhuaweicloud.com/v1/ops/delivery",
            "GET",
            Some("channel=WEB"),
            &cred(),
            None,
        )
        .unwrap();
        let auth2 = h2
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("authorization"))
            .map(|(_, v)| v.as_str())
            .unwrap();
        assert_ne!(auth, auth2, "query 未参与签名");
    }
}
