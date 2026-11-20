//! 签到结果模型。
//!
//! # 为什么区分「英文记录」与「中文展示」
//!
//! `providers/<id>/claim.log` 是**机器可读**的记录：其中的 `result` 与 `detail`
//! 一律使用英文（`success` / `skipped: already claimed` / `failed`），便于 grep、
//! 脚本统计与跨语言工具处理。
//!
//! 而面向人的输出（控制台日志、`status` 命令）仍是中文：同一个结果码在展示时
//! 通过 [`ClaimCode::zh`] / [`ClaimDetail::zh`] 还原成中文文案。
//!
//! 这样「记录用英文」与「输出看中文」两套文案只在一处维护，不会各写一遍。

/// 签到结果码。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimCode {
    /// 成功领取到额度。
    Success,
    /// 本次没领到（不是错误）。
    Skipped(SkipReason),
    /// 签到失败。
    Failed,
}

/// 跳过的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// 今日已领取。
    AlreadyClaimed,
    /// 签到活动未开启。
    NotEnabled,
    /// 今日不在可领取窗口。
    NotClaimable,
    /// 今日没有可领取的福利。
    NoBenefit,
}

impl ClaimCode {
    /// 英文结果串（写入 `claim.log`）。
    pub fn en(&self) -> &'static str {
        match self {
            ClaimCode::Success => "success",
            ClaimCode::Skipped(SkipReason::AlreadyClaimed) => "skipped: already claimed",
            ClaimCode::Skipped(SkipReason::NotEnabled) => "skipped: check-in not enabled",
            ClaimCode::Skipped(SkipReason::NotClaimable) => "skipped: not claimable today",
            ClaimCode::Skipped(SkipReason::NoBenefit) => "skipped: no benefit available",
            ClaimCode::Failed => "failed",
        }
    }

    /// 中文结果串（展示用，不含「跳过:」前缀，由调用方自行组织句子）。
    pub fn zh(&self) -> &'static str {
        match self {
            ClaimCode::Success => "签到成功",
            ClaimCode::Skipped(SkipReason::AlreadyClaimed) => "今日已签到",
            ClaimCode::Skipped(SkipReason::NotEnabled) => "签到活动未开启",
            ClaimCode::Skipped(SkipReason::NotClaimable) => "今日不可签到",
            ClaimCode::Skipped(SkipReason::NoBenefit) => "今日无可领取福利",
            ClaimCode::Failed => "签到失败",
        }
    }

    /// 是否为「跳过」。
    pub fn is_skipped(&self) -> bool {
        matches!(self, ClaimCode::Skipped(_))
    }
}
/// 签到明细（结构化，避免每个供应商各写一套文案）。
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimDetail {
    /// 名称 / 标题。英文与中文各一份（上游原文可能是中文）。
    Label { en: String, zh: String },
    /// 积分数量。
    Points(f64),
    /// 第几天。
    Day(i64),
    /// 连续签到天数。
    Streak(i64),
    /// 本周期已领 / 总天数。
    Cycle { claimed: i64, total: i64 },
    /// 额度（trae 的 credits）。
    Credits(f64),
    /// 余额变化。
    Delta(f64),
    /// 已到账待用的积分数。
    PendingPoints(f64),
    /// 待领条目（英文名 + 中文名 + 数量）。
    PendingItem { en: String, zh: String, count: i64 },
}

impl ClaimDetail {
    /// 英文明细。
    pub fn en(&self) -> String {
        match self {
            ClaimDetail::Label { en, .. } => en.clone(),
            ClaimDetail::Points(v) => format!("{} points", super::fmt_num(*v)),
            ClaimDetail::Day(v) => format!("day {v}"),
            ClaimDetail::Streak(v) => format!("streak {v} days"),
            ClaimDetail::Cycle { claimed, total } => format!("cycle {claimed}/{total}"),
            ClaimDetail::Credits(v) => format!("{} credits", super::fmt_num(*v)),
            ClaimDetail::Delta(v) => format!("balance change {v:+.2}"),
            ClaimDetail::PendingPoints(v) => format!("{} points pending", super::fmt_num(*v)),
            ClaimDetail::PendingItem { en, count, .. } => format!("{en} pending {count}"),
        }
    }

    /// 中文明细。
    pub fn zh(&self) -> String {
        match self {
            ClaimDetail::Label { zh, .. } => zh.clone(),
            ClaimDetail::Points(v) => format!("{v} 积分"),
            ClaimDetail::Day(v) => format!("第 {v} 天"),
            ClaimDetail::Streak(v) => format!("连续 {v} 天"),
            ClaimDetail::Cycle { claimed, total } => format!("本周期 {claimed}/{total}"),
            ClaimDetail::Credits(v) => format!("{v} 额度"),
            ClaimDetail::Delta(v) => format!("余额变化 {v:+.2}"),
            ClaimDetail::PendingPoints(v) => format!("已到账待用 {v} 积分"),
            ClaimDetail::PendingItem { zh, count, .. } => format!("{zh} 待领 {count}"),
        }
    }
}

/// 签到结果：结果码 + 明细。
#[derive(Debug, Clone)]
pub struct ClaimOutcome {
    pub code: ClaimCode,
    pub details: Vec<ClaimDetail>,
}

impl Default for ClaimOutcome {
    fn default() -> Self {
        Self::failed()
    }
}

impl ClaimOutcome {
    /// 成功。
    pub fn success(details: Vec<ClaimDetail>) -> Self {
        Self {
            code: ClaimCode::Success,
            details,
        }
    }

    /// 跳过。
    pub fn skipped(reason: SkipReason, details: Vec<ClaimDetail>) -> Self {
        Self {
            code: ClaimCode::Skipped(reason),
            details,
        }
    }

    /// 失败。
    pub fn failed() -> Self {
        Self {
            code: ClaimCode::Failed,
            details: Vec::new(),
        }
    }

    /// 写入 `claim.log` 的英文结果串（含明细）。
    pub fn result_en(&self) -> String {
        join(
            self.code.en(),
            &self.details.iter().map(|d| d.en()).collect::<Vec<_>>(),
        )
    }

    /// 展示用的中文结果串（含明细）。
    pub fn result_zh(&self) -> String {
        join(
            self.code.zh(),
            &self.details.iter().map(|d| d.zh()).collect::<Vec<_>>(),
        )
    }

    /// 展示用的一句话（中文）。
    ///
    /// 不带「跳过:」前缀：调用方（如 CLI）自己组织句子，避免出现
    /// 「签到跳过: 跳过: …」这样的重复。
    pub fn message_zh(&self) -> String {
        self.result_zh()
    }
}
/// 结果码 + 明细拼成一句。
fn join(code: &str, details: &[String]) -> String {
    if details.is_empty() {
        return code.to_string();
    }
    format!("{code}: {}", details.join(", "))
}

/// 签到失败原因（英文，写 `claim.log`）。
///
/// 错误详情（多为中文）只进控制台日志，文件里只留英文原因，便于 grep。
pub fn failure_reason(e: &crate::core::provider::ProviderError) -> &'static str {
    use crate::core::provider::ProviderError;
    match e {
        ProviderError::NotLoggedIn(_) => "not logged in",
        ProviderError::Unauthorized(_) => "credential invalid or expired",
        ProviderError::RateLimit(_) => "upstream rate limited",
        ProviderError::Transport(_) => "network request failed",
        ProviderError::Protocol(_) => "unexpected upstream response",
        ProviderError::Unsupported(_) => "not supported by provider",
        ProviderError::Other(_) => "internal error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 英文结果串不含中文() {
        let o = ClaimOutcome::success(vec![
            ClaimDetail::Day(3),
            ClaimDetail::Points(1000.0),
            ClaimDetail::Streak(3),
            ClaimDetail::Cycle {
                claimed: 3,
                total: 7,
            },
        ]);
        let s = o.result_en();
        assert_eq!(s, "success: day 3, 1000 points, streak 3 days, cycle 3/7");
        assert!(!s.chars().any(|c| c as u32 > 0x2FFF));
    }

    #[test]
    fn 中文结果串() {
        let o = ClaimOutcome::skipped(SkipReason::AlreadyClaimed, vec![ClaimDetail::Day(2)]);
        assert_eq!(o.result_zh(), "今日已签到: 第 2 天");
        assert_eq!(o.message_zh(), "今日已签到: 第 2 天");
        assert_eq!(o.result_en(), "skipped: already claimed: day 2");
    }

    #[test]
    fn 无明细时不加冒号() {
        let o = ClaimOutcome::skipped(SkipReason::NotEnabled, vec![]);
        assert_eq!(o.result_en(), "skipped: check-in not enabled");
        assert_eq!(o.result_zh(), "签到活动未开启");
        assert_eq!(o.message_zh(), "签到活动未开启");
    }
}
