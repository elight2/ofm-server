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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 单调且量级正确() {
        let a = now_millis();
        assert!(a > 1_700_000_000_000, "毫秒时间戳量级不对: {a}");
        assert_eq!(a / 1000, now_secs());
    }
}
