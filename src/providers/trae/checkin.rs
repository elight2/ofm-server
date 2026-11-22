//! 签到（`provider-info/trae.md` §6）。
//!
//! 流程：`status` → 若 `enable && !checked_in` 则 `claim` → 复读 `status` 复核。
//!
//! **陷阱一：`status` 的 `code` 不能当失败判据。** 上游在高峰期会对
//! `checkin_credits/status` 回 `{"code":9074,"message":"当前参与用户太多，请稍后再试"}`，
//! 但 `enable` / `checked_in` **字段照常返回**。官方客户端此时完全忽略 `code`，
//! 照常渲染 UI 并允许签到；只有 `checkin_claim` 才把 `code != 0` 当业务失败
//! （见 `main.js` `CheckinCredits.mb` 里的失败分类器）。
//! 本实现同样只校验 HTTP 200 + 两个布尔字段存在，`code` 仅用于日志/重试提示。
//!
//! **陷阱二：当日已签到时再调 `claim`**，上游仍回
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

/// 「当前参与用户太多，请稍后再试」的业务码。
///
/// 上游对 `status` 回该码时仍带 `enable`/`checked_in`，官方客户端忽略它照常签到。
pub const CODE_BUSY: i64 = 9074;

/// 凭证失效的业务码。上游在签到接口上可能以 HTTP 200 下发。
pub const CODE_AUTH: i64 = 1001;

/// `status` 响应。
///
/// `enable` / `checked_in` 用 `Option<bool>`：**字段缺失**才是真正的协议异常
/// （客户端同样以此判定 `protocol`），而 `code != 0` 不是。
#[derive(Debug, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    /// 活动是否开启。
    #[serde(default)]
    pub enable: Option<bool>,
    /// 今日是否已签到。
    #[serde(default)]
    pub checked_in: Option<bool>,
    /// 基础/额外额度（已签到时仍会回，表示该活动额度而非本次所得）。
    #[serde(default)]
    pub credits: i64,
    #[serde(default)]
    pub extra_credits: i64,
}

impl Status {
    /// 响应字段是否齐全（`enable` / `checked_in` 两个布尔都在）。
    ///
    /// 注意：字段齐全**不等于**响应成功——凭证失效时上游会在 HTTP 200 下回
    /// `code=1001` 且 `enable:false`，那仍是失败。是否可忽略 `code` 见
    /// [`Self::soft_error`]。
    pub fn usable(&self) -> Result<(), String> {
        match (self.enable, self.checked_in) {
            (Some(_), Some(_)) => Ok(()),
            _ => Err(format!(
                "code={} message={}（缺少 enable/checked_in 字段）",
                self.code, self.message
            )),
        }
    }

    /// 可忽略的业务码（返回提示文案）；需要当作失败时返回 `None`。
    ///
    /// 只有**已知的瞬时拥挤码**可忽略：上游在高峰期对 `status` 回 `code=9074`
    /// （当前参与用户太多）但仍带 `enable`/`checked_in`，官方客户端同样忽略它
    /// 继续签到。其余非 0 `code`（如凭证失效 `1001`）仍按失败处理，
    /// 不能因字段齐全就当成「活动未开启」——那会把凭证过期误报成正常跳过。
    pub fn soft_error(&self) -> Option<String> {
        (self.code == CODE_BUSY).then(|| format!("code={} message={}", self.code, self.message))
    }

    /// 除可忽略的拥挤码外，是否带了失败业务码。
    pub fn hard_error(&self) -> Option<String> {
        (self.code != 0 && !self.is_busy())
            .then(|| format!("code={} message={}", self.code, self.message))
    }

    /// 是否为「参与用户太多」这类瞬时拥挤码。
    pub fn is_busy(&self) -> bool {
        self.code == CODE_BUSY
    }

    /// 活动是否开启（缺失视为未开启）。
    pub fn enabled(&self) -> bool {
        self.enable.unwrap_or(false)
    }

    /// 今日是否已签到（缺失视为未签到）。
    pub fn is_checked_in(&self) -> bool {
        self.checked_in.unwrap_or(false)
    }

    /// 今日是否可以领取。
    pub fn claimable(&self) -> bool {
        self.enabled() && !self.is_checked_in()
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
        assert!(s.is_checked_in());
        assert!(!s.claimable(), "已签到不应再调 claim");
        assert!(s.usable().is_ok());
        assert_eq!(s.soft_error(), None);
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

    /// 回归：上游高峰期对 status 回 `code=9074` 但 `enable`/`checked_in` 照常返回，
    /// 官方客户端完全忽略 `code`。本实现也必须照常允许签到。
    #[test]
    fn 参与用户太多不影响可领取判定() {
        let s = parse_status(
            r#"{"code":9074,"message":"当前参与用户太多，请稍后再试",
                "enable":true,"checked_in":false}"#,
        );
        assert!(s.usable().is_ok(), "字段齐全就不该报协议错误");
        assert!(s.claimable(), "9074 不应阻止签到");
        assert!(s.is_busy());
        assert_eq!(s.hard_error(), None, "拥挤码不是硬错误");
        let w = s.soft_error().expect("应能取出提示文案");
        assert!(w.contains("9074"), "{w}");
    }

    /// 凭证失效（HTTP 200 + `code=1001`，字段也齐但 `enable:false`）必须当失败，
    /// 不能因为字段齐全就误报「活动未开启」。
    #[test]
    fn 凭证失效不被当成活动未开启() {
        let s = parse_status(
            r#"{"code":1001,"message":"not able to authenticate you",
                "enable":false,"checked_in":false}"#,
        );
        assert!(s.usable().is_ok());
        assert!(!s.is_busy());
        let err = s.hard_error().expect("1001 应是硬错误");
        assert!(err.contains("1001"), "{err}");
        assert_eq!(s.soft_error(), None);
    }

    #[test]
    fn 缺少布尔字段才是真异常() {
        // 上游只回错误码、不带 enable/checked_in 时，是字段层面的协议异常。
        let s = parse_status(r#"{"code":20101,"message":"Token 无效"}"#);
        let err = s.usable().expect_err("缺字段应报错");
        assert!(err.contains("20101"), "{err}");
    }

    #[test]
    fn 字段缺失时视为不可领取() {
        let s = parse_status(r#"{"code":0,"enable":true}"#);
        assert!(!s.is_checked_in());
        assert!(s.claimable(), "未给 checked_in 视为未签到");
        let s = parse_status(r#"{"code":0,"checked_in":false}"#);
        assert!(!s.claimable(), "未给 enable 视为未开启");
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
