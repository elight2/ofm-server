//! 余额：`GET /points/balance`。
//!
//! 响应：`{"code":"000000","desc":"成功","data":{"balance":6000,
//! "dailyBalance":4981,"availableBalance":10981}}`
//!
//! 对外展示的主数字取 `availableBalance`（= 永久分 `balance` + 当日分
//! `dailyBalance`，实测核对过）。`balance` 是**永久积分**，用户当日刷新的
//! 每日额度单独记在 `dailyBalance`，只看 `balance` 会少算。

use super::api::{self, MODEL_BASE_URL};

/// 余额路径。
pub const PATH: &str = "/points/balance";

/// 余额响应数据。
#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    /// 永久积分。
    #[serde(default)]
    pub balance: f64,
    /// 当日剩余额度。
    #[serde(default)]
    pub daily_balance: f64,
    /// 可用总额（= 永久 + 当日）。
    #[serde(default)]
    pub available_balance: f64,
}

impl Balance {
    /// 对外展示的主数字。
    ///
    /// 上游未给 `availableBalance` 时退化为两池之和，避免展示 0。
    pub fn main_points(&self) -> f64 {
        if self.available_balance > 0.0 {
            self.available_balance
        } else {
            self.balance + self.daily_balance
        }
    }
}

/// 拉取余额。
pub async fn fetch(
    client: &reqwest::Client,
    headers: &[(&str, String)],
) -> Result<Balance, api::EnvelopeErr> {
    let url = format!("{MODEL_BASE_URL}{PATH}");
    let mut http = client.get(&url);
    for (k, v) in headers {
        http = http.header(*k, v);
    }
    api::get_envelope(http).await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实响应（2026-11-20 实测）。
    #[test]
    fn 解析真实响应() {
        let b: Balance = serde_json::from_str(
            r#"{"balance":6000,"dailyBalance":4981,"availableBalance":10981}"#,
        )
        .unwrap();
        assert_eq!(b.main_points(), 10981.0);
        assert_eq!(b.balance + b.daily_balance, b.available_balance);
    }

    #[test]
    fn 缺available时退化为求和() {
        let b: Balance = serde_json::from_str(r#"{"balance":10,"dailyBalance":5}"#).unwrap();
        assert_eq!(b.main_points(), 15.0);
    }

    #[test]
    fn 空对象取零() {
        assert_eq!(Balance::default().main_points(), 0.0);
    }
}
