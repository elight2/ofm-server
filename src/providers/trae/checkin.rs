//! 签到（`provider-info/trae.md` §6）。
//!
//! 流程：`status` → 若 `enable && !checked_in` 则 `claim` → 复读 `status` 复核。
//!
//! **关键陷阱**：当日已签到时再调 `claim`，上游仍回
//! `HTTP 200 {"code":0,"message":"success"}`，与真正领取成功**无法区分**。
//! 因此不能靠 `claim` 的返回值判断成败：
//! 已签到就直接跳过（**不调 claim**），是否真领到只能靠 `status` 复核。
//!
//! 余额（`pay/ide_user_ent_usage`）仅用于报告增量，不作为成败判据——
//! 期间若有对话消耗，差额并不等于签到所得。

use serde::Deserialize;

/// 签到走 `api.trae.cn`（与登录用的 `api.trae.com.cn` 不同，实测后者 404）。
pub const HOST: &str = "https://api.trae.cn";
pub const PATH_STATUS: &str = "/trae/api/v2/ug/checkin_credits/status";
pub const PATH_CLAIM: &str = "/trae/api/v2/ug/checkin_credits/claim";
pub const PATH_USAGE: &str = "/trae/api/v2/pay/ide_user_ent_usage";

/// 桌面客户端为 `1`，web 为 `3`（§6）。
pub const REQ_SOURCE: u8 = 1;

#[derive(Debug, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    /// 活动是否开启。
    #[serde(default)]
    pub enable: bool,
    /// 今日是否已签到。
    #[serde(default)]
    pub checked_in: bool,
    /// 基础/额外额度（已签到时仍会回，表示该活动额度而非本次所得）。
    #[serde(default)]
    pub credits: i64,
    #[serde(default)]
    pub extra_credits: i64,
}

impl Status {
    /// 上游业务码非 0 视为失败（HTTP 200 也可能带错误）。
    pub fn check(&self) -> Result<(), String> {
        if self.code == 0 {
            Ok(())
        } else {
            Err(format!("code={} message={}", self.code, self.message))
        }
    }

    /// 今日是否可以领取。
    pub fn claimable(&self) -> bool {
        self.enable && !self.checked_in
    }
}

#[derive(Debug, Deserialize)]
pub struct ClaimResult {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

impl ClaimResult {
    pub fn check(&self) -> Result<(), String> {
        if self.code == 0 {
            Ok(())
        } else {
            Err(format!("code={} message={}", self.code, self.message))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub total_balance: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_status(json: &str) -> Status {
        serde_json::from_str(json).expect("status 解析失败")
    }

    #[test]
    fn 已签到时不可领取() {
        let s = parse_status(
            r#"{"checked_in":true,"code":0,"credits":100,"did_checked_in":false,
                "enable":true,"extra_credits":100,"message":"success"}"#,
        );
        assert!(s.checked_in);
        assert!(!s.claimable(), "已签到不应再调 claim");
        assert!(s.check().is_ok());
    }

    #[test]
    fn 活动关闭时不可领取() {
        let s = parse_status(r#"{"code":0,"enable":false,"checked_in":false}"#);
        assert!(!s.claimable());
    }

    #[test]
    fn 未签到且活动开启可领取() {
        let s = parse_status(r#"{"code":0,"enable":true,"checked_in":false}"#);
        assert!(s.claimable());
    }

    #[test]
    fn 业务错误码被判为失败() {
        let s = parse_status(r#"{"code":20101,"message":"Token 无效"}"#);
        let err = s.check().expect_err("非 0 code 应报错");
        assert!(err.contains("20101"), "{err}");
    }

    #[test]
    fn claim_成功响应无_credits_字段也能解析() {
        // 已签到时上游也回这个，故不能据此判断是否真领到。
        let r: ClaimResult = serde_json::from_str(r#"{"code":0,"message":"success"}"#).unwrap();
        assert!(r.check().is_ok());
    }

    #[test]
    fn usage_取总余额() {
        let u: Usage =
            serde_json::from_str(r#"{"total_balance":5657.82,"usage_summary":{}}"#).unwrap();
        assert!((u.total_balance - 5657.82).abs() < 1e-9);
    }
}
