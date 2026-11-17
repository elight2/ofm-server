//! 余额（积分）获取。
//!
//! # 端点来源
//!
//! `packages/tui/src/account/matrix-account-client.ts` 的
//! `projectMembership()`：余额取
//! `op_credit_summary.total_remaining_amount`，回退到 `opcredit_balance`。
//! 该函数喂给 `get_membership_info` 与 `get_user_extra_info` 两个端点的响应，
//! 两者响应里都带同名 `op_credit_summary`，取其一即可。
//!
//! ```text
//! POST https://agent.minimax.cn/matrix/api/v1/commerce/get_membership_info
//! body: {}
//! ```
//!
//! # 实测（2026-11-17，本仓库 testdata 账号）
//!
//! ```json
//! {"plan_type":1,"has_token_plan":false,"op_group_id":"2109949810109846391",
//!  "opcredit_balance":0,
//!  "op_credit_summary":{"total_remaining_amount":"8437.456",
//!                       "purchased_remaining_amount":"0",
//!                       "free_remaining_amount":"8437.456"},
//!  "base_resp":{"status_code":0,"status_msg":"success"}}
//! ```
//!
//! * **`total_remaining_amount` 是字符串**，不是数字，需自行解析。
//! * `opcredit_balance` 与 `total_remains_credit` 在本账号上都是 `0`，
//!   与真实余额不符 —— 只能用 `op_credit_summary`。
//! * **签名头与 query 全部必需性为否**：去掉 `yy`/`x-timestamp`/`x-signature`、
//!   去掉全部 query（含 `timezone_offset`）、`user_id` 传 `0`/空，
//!   实测都仍返回正确余额。这与 §6 早期推断（「需签名头 + 依赖
//!   `user/info` → `get_user_extra_info` 两条前置请求」）**不符**，
//!   已按实测简化：直连 `get_membership_info`，不带签名、不带 query。
//!
//! # 与 Token Plan 额度的区别
//!
//! `/v1/api/openplatform/coding_plan/remains`（`www.minimaxi.com`）是
//! **Token Plan 订阅**的额度窗口，本账号实测返回
//! `{"model_remains":null,"base_resp":{"status_code":2062,
//! "status_msg":"no active token plan subscription"}}` ——
//! 没有订阅时它给不出数，不是余额来源，故不实现。

use serde::Deserialize;

use super::api::{AGENT_HOST, UA};
use super::credential::Credential;
use crate::core::provider::ProviderError;

/// 会员/额度信息端点（响应内含 `op_credit_summary`）。
pub const MEMBERSHIP_PATH: &str = "/matrix/api/v1/commerce/get_membership_info";

/// 业务码：成功。
const STATUS_OK: i64 = 0;

#[derive(Debug, Deserialize)]
struct Envelope {
    #[serde(default)]
    base_resp: Option<BaseResp>,
    #[serde(default)]
    op_credit_summary: Option<CreditSummary>,
    /// 回退字段（字符串或数字都可能是，用 `Value` 兼容）。
    #[serde(default)]
    opcredit_balance: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct BaseResp {
    #[serde(default)]
    status_code: i64,
    #[serde(default)]
    status_msg: String,
}

#[derive(Debug, Deserialize)]
struct CreditSummary {
    /// 剩余总额，**字符串**（如 `"8437.456"`）。
    #[serde(default)]
    total_remaining_amount: Option<String>,
}

/// 取可用积分。
///
/// 优先 `op_credit_summary.total_remaining_amount`，回退 `opcredit_balance`；
/// 两者都取不到时返回 `None`（不是错误：账号可能尚未开通计费）。
pub async fn fetch(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<Option<f64>, ProviderError> {
    let url = format!("{AGENT_HOST}{MEMBERSHIP_PATH}");
    let resp = client
        .post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("user-agent", UA)
        .header("authorization", format!("Bearer {}", cred.access_token))
        .body("{}")
        .send()
        .await?;

    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status.as_u16()) {
        return Err(ProviderError::Unauthorized(format!(
            "上游 HTTP {}: {}",
            status.as_u16(),
            truncate(&text, 200)
        )));
    }
    let env: Envelope = serde_json::from_str(&text)
        .map_err(|e| ProviderError::Protocol(format!("解析余额响应失败: {e}")))?;

    if let Some(base) = &env.base_resp
        && base.status_code != STATUS_OK
    {
        return Err(ProviderError::Protocol(format!(
            "业务码 {}: {}",
            base.status_code, base.status_msg
        )));
    }

    Ok(env
        .op_credit_summary
        .and_then(|s| s.total_remaining_amount)
        .and_then(|s| parse_amount(&s))
        .or_else(|| env.opcredit_balance.as_ref().and_then(parse_value)))
}

/// 解析金额字符串（可能是 `"8437.456"` / `"1,234.5"` / `"0"`）。
fn parse_amount(raw: &str) -> Option<f64> {
    let cleaned: String = raw
        .chars()
        .filter(|c| *c != ',' && !c.is_whitespace())
        .collect();
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<f64>().ok()
}

/// 解析 JSON 值：字符串或数字都接受。
fn parse_value(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::String(s) => parse_amount(s),
        serde_json::Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析实测响应() {
        let raw = r#"{"plan_type":1,"has_token_plan":false,
            "op_group_id":"2109949810109846391","opcredit_balance":0,
            "op_credit_summary":{"total_remaining_amount":"8437.456",
                                 "purchased_remaining_amount":"0",
                                 "free_remaining_amount":"8437.456"},
            "base_resp":{"status_code":0,"status_msg":"success"}}"#;
        let env: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(env.base_resp.as_ref().unwrap().status_code, 0);
        let amount = env
            .op_credit_summary
            .and_then(|s| s.total_remaining_amount)
            .and_then(|s| parse_amount(&s));
        assert_eq!(amount, Some(8437.456));
    }

    #[test]
    fn 汇总缺失时回退_opcredit_balance() {
        let raw = r#"{"opcredit_balance":123.5,"base_resp":{"status_code":0}}"#;
        let env: Envelope = serde_json::from_str(raw).unwrap();
        assert!(env.op_credit_summary.is_none());
        assert_eq!(
            env.opcredit_balance.as_ref().and_then(parse_value),
            Some(123.5)
        );
    }

    #[test]
    fn 回退字段可以是字符串() {
        let raw = r#"{"opcredit_balance":"88.25","base_resp":{"status_code":0}}"#;
        let env: Envelope = serde_json::from_str(raw).unwrap();
        assert_eq!(
            env.opcredit_balance.as_ref().and_then(parse_value),
            Some(88.25)
        );
    }

    #[test]
    fn 金额含千分位与空格() {
        assert_eq!(parse_amount("1,234.5"), Some(1234.5));
        assert_eq!(parse_amount(" 0 "), Some(0.0));
        assert_eq!(parse_amount("8437.456"), Some(8437.456));
    }

    #[test]
    fn 非法金额返回_none() {
        assert_eq!(parse_amount(""), None);
        assert_eq!(parse_amount("abc"), None);
    }

    #[test]
    fn 业务码非零可识别() {
        let raw = r#"{"base_resp":{"status_code":1001,"status_msg":"fail"}}"#;
        let env: Envelope = serde_json::from_str(raw).unwrap();
        let base = env.base_resp.unwrap();
        assert_ne!(base.status_code, STATUS_OK);
        assert_eq!(base.status_msg, "fail");
    }

    #[test]
    fn 两边都缺失时返回_none() {
        let raw = r#"{"base_resp":{"status_code":0}}"#;
        let env: Envelope = serde_json::from_str(raw).unwrap();
        let amount = env
            .op_credit_summary
            .and_then(|s| s.total_remaining_amount)
            .and_then(|s| parse_amount(&s))
            .or_else(|| env.opcredit_balance.as_ref().and_then(parse_value));
        assert_eq!(amount, None);
    }
}
