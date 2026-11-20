//! 余额：资源包汇总（个人）/ 企业额度（企业）。
//!
//! 协议见 `provider-info/workbuddy.md` §6。
//!
//! * **个人账号**：`POST /billing/meter/get-user-resource-summary`（**无** `/v2` 前缀，
//!   桌面端 `resourcePrefix` 为空）。返回各资源包的 `CycleTotalCapacity` /
//!   `CycleRemainCapacity`，余额 = Σ 剩余，总量 = Σ 总量。
//! * **企业账号**：`POST /v2/billing/meter/get-enterprise-user-usage`
//!   （带 `/v2` 前缀），返回 `data.limit_num`（总额度）/ `data.used_num`（已用）。
//!   `limit_num == -1` 表示无限额度。

use serde::{Deserialize, Deserializer};

use super::api::{self, HttpError};

/// 数量字段：上游有时给数字，有时给字符串（如 `"2262"`）。
fn num<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Num {
        F(f64),
        S(String),
    }
    match Num::deserialize(d)? {
        Num::F(v) => Ok(v),
        Num::S(s) => s.trim().parse::<f64>().map_err(serde::de::Error::custom),
    }
}

/// 资源包汇总响应 `data`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ResourceSummary {
    /// 各资源包。
    #[serde(default)]
    pub packages: Vec<Package>,
    /// 是否付费用户。
    #[serde(default)]
    #[allow(dead_code)]
    pub is_paid_user: bool,
}

/// 单个资源包。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Package {
    /// 资源包编码。
    #[serde(default)]
    #[allow(dead_code)]
    pub package_code: String,
    /// 本周期总量。
    #[serde(default, deserialize_with = "num")]
    #[allow(dead_code)]
    pub cycle_total_capacity: f64,
    /// 本周期剩余。
    #[serde(default, deserialize_with = "num")]
    pub cycle_remain_capacity: f64,
    /// 本周期已用。
    #[serde(default, deserialize_with = "num")]
    #[allow(dead_code)]
    pub cycle_used_capacity: f64,
}

impl ResourceSummary {
    /// 剩余额度合计。
    pub fn remain(&self) -> f64 {
        self.packages.iter().map(|p| p.cycle_remain_capacity).sum()
    }

    /// 总量合计。
    #[allow(dead_code)]
    pub fn total(&self) -> f64 {
        self.packages.iter().map(|p| p.cycle_total_capacity).sum()
    }
}

/// 企业用量响应 `data`。
#[derive(Debug, Deserialize)]
pub struct EnterpriseUsage {
    /// 总额度；`-1` 表示无限。
    ///
    /// 上游字段名是 snake_case（`limit_num`），与同响应里的 camelCase 字段不同。
    #[serde(default, rename = "limit_num")]
    pub limit_num: Option<f64>,
    /// 已用额度。
    #[serde(default, rename = "used_num")]
    pub used_num: Option<f64>,
}

impl EnterpriseUsage {
    /// 剩余额度。无限额度返回 `None`（无法用数字表示）。
    pub fn remain(&self) -> Option<f64> {
        let limit = self.limit_num?;
        if limit < 0.0 {
            return None;
        }
        Some(limit - self.used_num.unwrap_or(0.0))
    }
}

/// 拉取余额。
///
/// 企业账号走企业用量端点；个人账号走资源包汇总。
pub async fn fetch(
    client: &reqwest::Client,
    access_token: &str,
    domain: &str,
    enterprise_id: &str,
) -> Result<f64, HttpError> {
    let headers = api::auth_headers(access_token, domain);
    if !enterprise_id.trim().is_empty() {
        let v = api::post_envelope::<_, EnterpriseUsage>(
            client,
            &format!("{}{}", api::HOST, api::ENTERPRISE_USAGE_PATH),
            &with_enterprise(headers, enterprise_id),
            &serde_json::json!({}),
        )
        .await?;
        return v
            .remain()
            .ok_or_else(|| HttpError::Decode("企业额度为无限，无法折算数值".into()));
    }

    let v = api::post_envelope::<_, ResourceSummary>(
        client,
        &format!("{}{}", api::HOST, api::RESOURCE_SUMMARY_PATH),
        &headers,
        &serde_json::json!({}),
    )
    .await?;
    Ok(v.remain())
}

/// 给头部加上企业 id。
fn with_enterprise(
    mut headers: Vec<(&'static str, String)>,
    enterprise_id: &str,
) -> Vec<(&'static str, String)> {
    headers.push(("X-Enterprise-Id", enterprise_id.to_string()));
    headers.push(("X-Tenant-Id", enterprise_id.to_string()));
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 解析资源包汇总() {
        let s: ResourceSummary = serde_json::from_str(
            r#"{"Packages":[
                 {"PackageCode":"TCACA_code_002_AkiJS3ZHF5",
                  "CycleTotalCapacity":1000,"CycleRemainCapacity":750,"CycleUsedCapacity":250},
                 {"PackageCode":"TCACA_code_003_x",
                  "CycleTotalCapacity":500,"CycleRemainCapacity":100,"CycleUsedCapacity":400}],
               "IsPaidUser":true}"#,
        )
        .unwrap();
        assert_eq!(s.remain(), 850.0);
        assert_eq!(s.total(), 1500.0);
        assert!(s.is_paid_user);
    }

    /// 上游把数量字段当字符串发（`"2262"`），也要能解析。
    #[test]
    fn 解析字符串数量() {
        let s: ResourceSummary = serde_json::from_str(
            r#"{"Packages":[
                 {"PackageCode":"a","CycleTotalCapacity":"2262",
                  "CycleRemainCapacity":"1823.52000267","CycleUsedCapacity":"438.47999733"}]}"#,
        )
        .unwrap();
        assert!((s.remain() - 1823.52000267).abs() < 1e-6);
        assert!((s.total() - 2262.0).abs() < 1e-6);
    }

    #[test]
    fn 无资源包时余额为零() {
        let s: ResourceSummary = serde_json::from_str(r#"{"Packages":[]}"#).unwrap();
        assert_eq!(s.remain(), 0.0);
    }

    #[test]
    fn 解析企业用量() {
        let e: EnterpriseUsage =
            serde_json::from_str(r#"{"limit_num":1000,"used_num":400}"#).unwrap();
        assert_eq!(e.remain(), Some(600.0));
    }

    /// `limit_num == -1` 是无限额度，折不成数字。
    #[test]
    fn 无限额度无法折算() {
        let e: EnterpriseUsage = serde_json::from_str(r#"{"limit_num":-1,"used_num":0}"#).unwrap();
        assert_eq!(e.remain(), None);
    }
}
