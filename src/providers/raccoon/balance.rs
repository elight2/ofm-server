//! 余额：`GET /api/web/points/v1/balance`。
//!
//! 响应 `data` 中各分池字段见 `provider-info/raccoon.md` §5。
//! 对外展示的主数字取 `available_points`（= 各分池之和，实测核对过）。
//!
//! 注意：该接口**不含任何到期字段**，积分到期信息只能从账单
//! `GET /api/web/points/v2/bills` 的 `expire_at` 取。

use serde::Deserialize;

use super::api::{self, POINTS_PREFIX};

/// 余额路径（`api::POINTS_PREFIX` + 版本段）。
pub const PATH: &str = "/v1/balance";

/// 余额响应。字段很多，只解需要的部分；未知字段由 serde 默认忽略。
#[derive(Debug, Default, Deserialize)]
pub struct Balance {
    /// 可用总积分（各分池之和）。
    #[serde(default)]
    pub available_points: f64,
    /// 每日积分（当天/未过期的那部分）。分池明细仅存档，`status` 只展示总额。
    #[serde(default)]
    #[allow(dead_code)]
    pub daily_points: f64,
    /// 每月积分。
    #[serde(default)]
    #[allow(dead_code)]
    pub monthly_points: f64,
    /// 奖励积分（无到期字段）。
    #[serde(default)]
    #[allow(dead_code)]
    pub reward_points: f64,
    /// 加油包积分。
    #[serde(default)]
    #[allow(dead_code)]
    pub topup_points: f64,
}

/// 拉取余额。
pub async fn fetch(
    client: &reqwest::Client,
    headers: &[(&str, String)],
) -> Result<Balance, api::HttpError> {
    let url = format!("{}{POINTS_PREFIX}{PATH}", api::HOST);
    api::get_envelope(client, &url, headers).await
}

impl Balance {
    /// 对外展示的主数字。
    pub fn main_points(&self) -> f64 {
        self.available_points
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实响应（2026-11-16 实测）。
    #[test]
    fn 解析真实响应() {
        let b: Balance = serde_json::from_str(
            r#"{"available_points":7713,"daily_points":300,"monthly_points":0,
                "reward_points":7413,"topup_points":0,"enterprise_topup_points":0,
                "total_chargeable_points":7713,"enterprise_reserved":false,
                "topup_frozen":false,"owner_type":"users","owner_id":7657055}"#,
        )
        .unwrap();
        assert_eq!(b.main_points(), 7713.0);
        assert_eq!(b.daily_points, 300.0);
        assert_eq!(b.reward_points, 7413.0);
        // 分池之和等于 available_points
        assert_eq!(
            b.daily_points + b.monthly_points + b.reward_points + b.topup_points,
            b.available_points
        );
    }

    /// 未来若上游删掉某些字段，不能整条解析失败（余额是可选展示项）。
    #[test]
    fn 缺字段不报错() {
        let b: Balance = serde_json::from_str(r#"{"available_points":100}"#).unwrap();
        assert_eq!(b.main_points(), 100.0);
        assert_eq!(b.daily_points, 0.0);
    }

    #[test]
    fn 空对象取零() {
        let b: Balance = serde_json::from_str("{}").unwrap();
        assert_eq!(b.main_points(), 0.0);
    }
}
