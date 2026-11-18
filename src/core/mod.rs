//! 供应商无关的核心层：事件模型、供应商 trait、注册表、凭证存储、SSE 解析。

pub mod atomicio;
pub mod claim;
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
        Ok(dt) => fmt_datetime(dt),
        Err(_) => ms.to_string(),
    }
}

/// RFC 3339 字符串（如 `2026-11-17T14:40:57.510Z`）→ 与 [`fmt_ms`] 同格式。
///
/// 时区仍按 UTC 显示（不换本地），保证与凭证到期时间口径一致。
/// 解析失败时原样返回，不丢信息。
pub fn fmt_rfc3339(s: &str) -> String {
    use time::OffsetDateTime;
    use time::format_description::well_known::Rfc3339;
    match OffsetDateTime::parse(s, &Rfc3339) {
        // 带偏移的串要先换算到 UTC，否则 `dt.hour()` 取的是原偏移下的钟点。
        Ok(dt) => fmt_datetime(dt.to_offset(time::UtcOffset::UTC)),
        Err(_) => s.to_string(),
    }
}

/// `YYYY-MM-DD HH:MM:SS`。
fn fmt_datetime(dt: time::OffsetDateTime) -> String {
    let (y, mo, d) = dt.to_calendar_date();
    let (h, mi, s) = (dt.hour(), dt.minute(), dt.second());
    format!("{y:04}-{:02}-{d:02} {h:02}:{mi:02}:{s:02}", mo as u8)
}

#[cfg(test)]
mod tests {
    #[test]
    fn 时间戳格式化() {
        // 2026-11-16T01:00:00Z
        assert_eq!(super::fmt_ms(1_794_790_800_000), "2026-11-16 01:00:00");
    }

    #[test]
    fn rfc3339_转同格式() {
        // 带纳秒与 Z 后缀（`claim.log` 里的真实形状）。
        assert_eq!(
            super::fmt_rfc3339("2026-11-17T14:40:57.510609465Z"),
            "2026-11-17 14:40:57"
        );
        assert_eq!(
            super::fmt_rfc3339("2026-11-17T14:40:57Z"),
            "2026-11-17 14:40:57"
        );
        // 带时区偏移也按 UTC 显示（转换后）。
        assert_eq!(
            super::fmt_rfc3339("2026-11-17T22:40:57+08:00"),
            "2026-11-17 14:40:57"
        );
    }

    #[test]
    fn 非法时间串原样返回() {
        assert_eq!(super::fmt_rfc3339("unknown"), "unknown");
    }
}
