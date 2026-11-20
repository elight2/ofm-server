//! 签到：每日签到活动。
//!
//! 协议见 `provider-info/workbuddy.md` §7。桌面端 `billingPrefix = "/v2"`：
//!
//! ```text
//! ① POST /v2/billing/meter/checkin-activity-status
//!    → { active, today_checked_in, daily_credit, streak_days, ... }
//! ② POST /v2/billing/meter/daily-checkin
//!    → { credit, streak_days, is_streak_day }
//! ```
//!
//! 失败业务码：`1001` 已领取 / `1002` 不符合条件 / `1003` 活动已结束。
//!
//! **设备令牌**：官方会带图灵 SDK 的 `X-Device-Token`（取不到时带
//! `X-Device-Token-Error`）。中国版是否强制该头 `[未验证]`，故这里**先不带**
//! 尝试；上游若以设备令牌为由拒绝，再退回「跳过」而不是反复重试。

use serde::Deserialize;

use crate::core::claim::ClaimOutcome;

use super::api::{self, HttpError};

/// 活动状态响应 `data`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ActivityStatus {
    /// 活动是否开启。
    #[serde(default)]
    pub active: bool,
    /// 今日是否已签到。
    #[serde(default)]
    pub today_checked_in: bool,
    /// 每日可得额度。
    #[serde(default)]
    #[allow(dead_code)]
    pub daily_credit: f64,
    /// 已连续签到天数。
    #[serde(default)]
    #[allow(dead_code)]
    pub streak_days: i64,
    /// 今日可得额度（有些活动当天加码）。
    #[serde(default)]
    #[allow(dead_code)]
    pub today_credit: f64,
}

/// 签到结果响应 `data`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CheckinResult {
    /// 本次获得的额度。
    #[serde(default)]
    pub credit: f64,
    /// 连续签到天数。
    #[serde(default)]
    pub streak_days: i64,
}

/// 业务码 → 签到语义（§7 的 `mapCheckinStatus`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckinStatus {
    /// 已领取。
    AlreadyClaimed,
    /// 不符合条件（今日不在可领取窗口 / 未开通）。
    NotEligible,
    /// 活动已结束。
    EventEnded,
    /// 其它业务错误。
    UnknownBizError,
}

impl CheckinStatus {
    /// 由业务码映射。
    pub fn from_code(code: i64) -> Self {
        match code {
            1001 => Self::AlreadyClaimed,
            1002 => Self::NotEligible,
            1003 => Self::EventEnded,
            _ => Self::UnknownBizError,
        }
    }
}

/// 查活动状态。
pub async fn activity_status(
    client: &reqwest::Client,
    headers: &[(&str, String)],
) -> Result<ActivityStatus, HttpError> {
    api::post_envelope::<_, ActivityStatus>(
        client,
        &format!("{}{}", api::HOST, api::CHECKIN_STATUS_PATH),
        headers,
        &serde_json::json!({}),
    )
    .await
}

/// 每日签到。
///
/// 返回 `Err` 时业务码可由 [`HttpError::business_code`] 取出（`1001/1002/1003`）。
pub async fn daily_checkin(
    client: &reqwest::Client,
    headers: &[(&str, String)],
) -> Result<CheckinResult, HttpError> {
    api::post_envelope::<_, CheckinResult>(
        client,
        &format!("{}{}", api::HOST, api::DAILY_CHECKIN_PATH),
        headers,
        &serde_json::json!({}),
    )
    .await
}

/// 把签到结果整理成统一的报告。
pub fn to_report(status: CheckinStatus) -> ClaimOutcome {
    match status {
        CheckinStatus::AlreadyClaimed => {
            ClaimOutcome::skipped(crate::core::claim::SkipReason::AlreadyClaimed, vec![])
        }
        CheckinStatus::NotEligible => {
            ClaimOutcome::skipped(crate::core::claim::SkipReason::NotClaimable, vec![])
        }
        CheckinStatus::EventEnded => {
            ClaimOutcome::skipped(crate::core::claim::SkipReason::NotEnabled, vec![])
        }
        CheckinStatus::UnknownBizError => ClaimOutcome::failed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 业务码映射() {
        assert_eq!(
            CheckinStatus::from_code(1001),
            CheckinStatus::AlreadyClaimed
        );
        assert_eq!(CheckinStatus::from_code(1002), CheckinStatus::NotEligible);
        assert_eq!(CheckinStatus::from_code(1003), CheckinStatus::EventEnded);
        assert_eq!(
            CheckinStatus::from_code(9999),
            CheckinStatus::UnknownBizError
        );
    }

    #[test]
    fn 解析活动状态() {
        let s: ActivityStatus = serde_json::from_str(
            r#"{"active":true,"today_checked_in":false,"daily_credit":100,
                "today_credit":0,"streak_days":3}"#,
        )
        .unwrap();
        assert!(s.active);
        assert!(!s.today_checked_in);
        assert_eq!(s.daily_credit, 100.0);
        assert_eq!(s.streak_days, 3);
    }

    #[test]
    fn 解析签到结果() {
        let r: CheckinResult =
            serde_json::from_str(r#"{"credit":100,"streak_days":4,"is_streak_day":true}"#).unwrap();
        assert_eq!(r.credit, 100.0);
        assert_eq!(r.streak_days, 4);
    }

    #[test]
    fn 已领取映射为跳过() {
        let o = to_report(CheckinStatus::AlreadyClaimed);
        assert!(o.code.is_skipped());
    }
}
