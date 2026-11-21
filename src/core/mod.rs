//! 供应商无关的核心层：事件模型、供应商 trait、注册表、凭证存储、SSE 解析。

pub mod atomicio;
pub mod balancelog;
pub mod chunk;
pub mod claim;
pub mod claimlog;
pub mod clock;
pub mod credential;
pub mod events;
pub mod http;
pub mod paths;
pub mod provider;
pub mod registry;
pub mod sse;
pub mod stream;
pub mod text;

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

/// 秒时间戳 → 与 [`fmt_ms`] 同格式（UTC）。
///
/// raccoon 的到期时间以秒给出，格式化结果需与毫秒口径完全一致。
pub fn fmt_secs(secs: i64) -> String {
    use time::OffsetDateTime;
    match OffsetDateTime::from_unix_timestamp(secs) {
        Ok(dt) => fmt_datetime(dt),
        Err(_) => secs.to_string(),
    }
}

/// 毫秒时间戳 → [`fmt_ms`]；`None` 显示「未知」。
///
/// 登录/状态输出里到期时间是可选的（上游不一定会给），单独包一层免得各家重复。
pub fn fmt_ms_opt(ms: Option<i64>) -> String {
    match ms {
        Some(ms) => fmt_ms(ms),
        None => "未知".to_string(),
    }
}

/// 数值展示：整数不带小数点，其余原样（与各家的签到明细一致）。
pub fn fmt_num(v: f64) -> String {
    if v.fract().abs() < f64::EPSILON {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// 金额展示：固定小数点后 2 位（四舍五入）。
///
/// 与 [`fmt_num`] 不同，这是**金额**口径：整数也显示 `100.00`，避免
/// 「余额是 100 还是 100.00」的歧义。
pub fn fmt_amount(v: f64) -> String {
    format!("{v:.2}")
}

/// 金额取整到小数点后 2 位（四舍五入）。
///
/// 写盘前调用，保证 `claim.log` 里的余额/增量与 [`fmt_amount`] 展示口径一致，
/// 不残留多余小数。
pub fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
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
    fn 整数的浮点显示() {
        assert_eq!(super::fmt_num(300.0), "300");
        assert_eq!(super::fmt_num(12.5), "12.5");
        assert_eq!(super::fmt_num(-0.0), "0");
    }

    #[test]
    fn 金额固定两位小数() {
        assert_eq!(super::fmt_amount(100.0), "100.00");
        assert_eq!(super::fmt_amount(8437.456), "8437.46");
        assert_eq!(super::fmt_amount(8352.91), "8352.91");
        assert_eq!(super::fmt_amount(-25.5), "-25.50");
        assert_eq!(super::fmt_amount(0.0), "0.00");
    }

    #[test]
    fn 金额取整到两位() {
        assert_eq!(super::round2(8437.456), 8437.46);
        assert_eq!(super::round2(100.0), 100.0);
        assert_eq!(super::round2(0.005), 0.01);
        assert_eq!(super::round2(-25.555), -25.56);
    }

    #[test]
    fn 秒与毫秒口径一致() {
        // 同一个时刻：1794790800 秒 == 1794790800000 毫秒。
        assert_eq!(
            super::fmt_secs(1_794_790_800),
            super::fmt_ms(1_794_790_800_000)
        );
        assert_eq!(super::fmt_ms_opt(None), "未知");
        assert_eq!(
            super::fmt_ms_opt(Some(1_794_790_800_000)),
            "2026-11-16 01:00:00"
        );
    }

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
