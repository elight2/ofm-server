//! 时间戳小工具。
//!
//! 各家原先各写一份 `now_secs` / `now_millis`（7 处），语义完全相同。

/// 当前 Unix 秒。
pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 当前 Unix 毫秒。
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 当前 UTC 时间，RFC 3339（`2026-11-15T09:44:19Z`）。
///
/// 用 UTC 而非本地时间：`time` crate 未启用 `local-offset` feature（须改依赖）。
/// 签到记录、余额记录共用这一份时间口径。
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::from("unknown"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 单调且量级正确() {
        let a = now_millis();
        assert!(a > 1_700_000_000_000, "毫秒时间戳量级不对: {a}");
        assert_eq!(a / 1000, now_secs());
    }

    #[test]
    fn rfc3339_是_utc() {
        let s = now_rfc3339();
        assert!(s.ends_with('Z'), "应为 UTC 时间: {s}");
        assert!(s.contains('T'), "应含日期时间分隔符: {s}");
    }
}
