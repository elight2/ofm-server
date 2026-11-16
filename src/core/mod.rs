//! 供应商无关的核心层：事件模型、供应商 trait、注册表、凭证存储、SSE 解析。

pub mod claimlog;
pub mod credential;
pub mod events;
pub mod provider;
pub mod registry;
pub mod sse;
pub mod stream;

/// 毫秒时间戳 → `YYYY-MM-DD HH:MM:SS`（UTC）。
///
/// 用 UTC 而非本地时间：`time` crate 未启用 `local-offset` feature。
pub fn fmt_ms(ms: i64) -> String {
    use time::OffsetDateTime;
    let secs = ms / 1000;
    match OffsetDateTime::from_unix_timestamp(secs) {
        Ok(dt) => {
            let (y, mo, d) = dt.to_calendar_date();
            let (h, mi, s) = (dt.hour(), dt.minute(), dt.second());
            format!("{y:04}-{:02}-{d:02} {h:02}:{mi:02}:{s:02}", mo as u8)
        }
        Err(_) => ms.to_string(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn 时间戳格式化() {
        // 2026-11-16T01:00:00Z
        assert_eq!(super::fmt_ms(1_794_790_800_000), "2026-11-16 01:00:00");
    }
}
