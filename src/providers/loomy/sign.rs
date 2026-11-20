//! 讯飞账号网关（`account.xfinfr.com`）请求签名。
//!
//! 复刻 Loomy 客户端 `xfyun/sign.js`：走讯飞标准 `SDK-HMAC-SHA1`，待签名为
//! 9 行字符串（`\n` 连接）：
//!
//! ```text
//! METHOD
//! escapedPath
//! escapedQuery
//! Content-MD5(base64)
//! Content-Type
//! Date(RFC1123 GMT)
//! Nonce(uuid4)
//! SignedHeaders(本协议无 x-* 头，空)
//! CanonicalizedHeaders(空)
//! ```
//!
//! 三个易错点（对齐客户端实现）：
//! 1. `Content-MD5` 是待签名串的第 4 行，**必须**有（空 body 时才为空）；
//! 2. `escapedPath`/`escapedQuery` 在 `encodeURIComponent` 之外还要把
//!    `! ' ( ) *` 转成 `%21 %27 %28 %29 %2A`（RFC3986 §3.3/§3.4）；
//! 3. `Date` 必须是 RFC1123 的 GMT 形式，且与请求头里的值完全一致。

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use md5::Md5;
use sha1::Sha1;
use sha2::Digest;

/// 请求头集合（顺序无关，`reqwest` 自行排序）。
#[derive(Debug, Clone)]
pub struct SignedHeaders {
    pub authorization: String,
    pub date: String,
    pub nonce: String,
    pub content_md5: String,
    pub content_type: String,
}

/// 生成签名与配套请求头。
///
/// `body` 必须是**实际发送的字节**（紧凑 JSON），签名用的 MD5 与 Content-MD5
/// 头都基于它。
pub fn sign(
    access_key_id: &str,
    access_key_secret: &str,
    method: &str,
    path: &str,
    query: &[(String, String)],
    body: &[u8],
    content_type: &str,
    magic_name: &str,
) -> SignedHeaders {
    let date = rfc1123_now();
    let nonce = uuid4_string();
    let md5 = content_md5(body);

    let string_to_sign = string_to_sign(method, path, query, &md5, content_type, &date, &nonce);

    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(access_key_secret.as_bytes())
        .expect("HMAC 接受任意长度密钥");
    mac.update(string_to_sign.as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());

    SignedHeaders {
        authorization: format!("{magic_name} {access_key_id}:{signature}"),
        date,
        nonce,
        content_md5: md5,
        content_type: content_type.to_string(),
    }
}

/// 拼装待签名字符串（见模块文档的 9 行格式）。
pub fn string_to_sign(
    method: &str,
    path: &str,
    query: &[(String, String)],
    content_md5: &str,
    content_type: &str,
    date: &str,
    nonce: &str,
) -> String {
    let parts = [
        method.to_uppercase(),
        escaped_path(path),
        escaped_query(query),
        content_md5.to_string(),
        content_type.to_string(),
        date.to_string(),
        nonce.to_string(),
        // 本协议不带 x-* 参与签名的头，SignedHeaders 与 CanonicalizedHeaders 均为空。
        String::new(),
        String::new(),
    ];
    parts.join("\n")
}

/// body 的 base64(MD5)；空 body 返回空串。
pub fn content_md5(body: &[u8]) -> String {
    if body.is_empty() {
        return String::new();
    }
    STANDARD.encode(Md5::digest(body))
}

/// 路径转义：逐段 `encodeURIComponent` + 额外转义 `! ' ( ) *`。
fn escaped_path(path: &str) -> String {
    let clean = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let clean = if clean.len() > 1 && clean.ends_with('/') {
        clean[..clean.len() - 1].to_string()
    } else {
        clean
    };
    clean
        .split('/')
        .map(escape_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// 查询串转义：`k=v&k=v`，键与值都按 RFC3986 转义。
fn escaped_query(query: &[(String, String)]) -> String {
    query
        .iter()
        .map(|(k, v)| format!("{}={}", escape_component(k), escape_component(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// RFC3986 转义（等价前端 `encodeURIComponent` 后再补转义四个保留字符）。
fn escape_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            // `encodeURIComponent` 不转义这四个，但讯飞签名要求转义。
            b'!' => out.push_str("%21"),
            b'\'' => out.push_str("%27"),
            b'(' => out.push_str("%28"),
            b')' => out.push_str("%29"),
            b'*' => out.push_str("%2A"),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// 32 位十六进制随机串（等价客户端 `randomUUID().replace(/-/g, "")`）。
pub fn trace_id() -> String {
    uuid4_string()
}

/// W3C `traceparent`：`00-<32hex>-<16hex>-01`，缺失时上游对话会挂死。
pub fn traceparent() -> String {
    let mut span = [0u8; 8];
    if getrandom::fill(&mut span).is_err() {
        span = (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0))
        .to_be_bytes();
    }
    let span: String = span.iter().map(|b| format!("{b:02x}")).collect();
    format!("00-{}-{span}-01", trace_id())
}

fn uuid4_string() -> String {
    let mut buf = [0u8; 16];
    if getrandom::fill(&mut buf).is_err() {
        // 随机源失败概率极低；退化为时间派生，仍能完成登录。
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        buf = (nanos as u128).to_be_bytes();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 当前时间的 RFC1123 GMT 表示，如 `Fri, 20 Nov 2026 00:44:14 GMT`。
///
/// 不用 `time` 的格式化接口，避免为一个头部引入额外 feature。
fn rfc1123_now() -> String {
    const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];

    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0) as i64;

    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // 1970-01-01 是周四。
    let weekday = WEEKDAYS[(days.rem_euclid(7) as usize + 4) % 7];
    let (year, month, day) = civil_from_days(days);

    format!(
        "{weekday}, {day:02} {} {year} {:02}:{:02}:{:02} GMT",
        MONTHS[(month - 1) as usize],
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// 由「1970-01-01 起的天数」还原公历年月日（Howard Hinnant 算法）。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 转义保留字符() {
        assert_eq!(escape_component("a!b'c(d)e*f"), "a%21b%27c%28d%29e%2Af");
        assert_eq!(escape_component("a b"), "a%20b");
        assert_eq!(escape_component("a-b_c.d~e"), "a-b_c.d~e");
    }

    #[test]
    fn 路径转义() {
        assert_eq!(
            escaped_path("/login/phone/sendMsgCode"),
            "/login/phone/sendMsgCode"
        );
        assert_eq!(escaped_path("login/phone"), "/login/phone");
        assert_eq!(escaped_path("/login/phone/"), "/login/phone");
    }

    #[test]
    fn 待签名串格式() {
        let sts = string_to_sign(
            "POST",
            "/login/phone/sendMsgCode",
            &[],
            "WcASA9vCJ+9aUzl/8lWMog==",
            "application/json",
            "Fri, 20 Nov 2026 00:44:14 GMT",
            "af9cda1b-f50a-47eb-b219-8fb2c98580ca",
        );
        assert_eq!(
            sts,
            "POST\n/login/phone/sendMsgCode\n\nWcASA9vCJ+9aUzl/8lWMog==\napplication/json\n\
             Fri, 20 Nov 2026 00:44:14 GMT\naf9cda1b-f50a-47eb-b219-8fb2c98580ca\n\n"
        );
    }

    #[test]
    fn 公历还原() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_720), (2026, 9, 24));
    }

    #[test]
    fn 日期是_rfc1123() {
        let d = rfc1123_now();
        // 形如 `Fri, 20 Nov 2026 00:44:14 GMT`
        assert_eq!(d.len(), 29, "{d}");
        assert!(d.ends_with(" GMT"), "{d}");
        assert_eq!(&d[3..5], ", ");
        assert_eq!(&d[19..20], ":");
    }
}
#[cfg(test)]
mod vector_tests {
    use super::*;

    /// 用 Python 参考实现算出的固定向量，锁定 HMAC 结果。
    #[test]
    fn 签名向量() {
        let sts = string_to_sign(
            "POST",
            "/login/phone/sendMsgCode",
            &[],
            "WcASA9vCJ+9aUzl/8lWMog==",
            "application/json",
            "Fri, 20 Nov 2026 00:44:14 GMT",
            "af9cda1b-f50a-47eb-b219-8fb2c98580ca",
        );
        assert_eq!(
            sts,
            "POST\n/login/phone/sendMsgCode\n\nWcASA9vCJ+9aUzl/8lWMog==\napplication/json\n\
             Fri, 20 Nov 2026 00:44:14 GMT\naf9cda1b-f50a-47eb-b219-8fb2c98580ca\n\n"
        );

        let mut mac =
            <Hmac<Sha1> as Mac>::new_from_slice(b"zsak6eadrbawz683wf5r3m2snrwj868r").unwrap();
        mac.update(sts.as_bytes());
        assert_eq!(
            STANDARD.encode(mac.finalize().into_bytes()),
            "N5hq1gByTw/CB1iq4IupgCR4hTk="
        );
    }

    #[test]
    fn content_md5_与参考实现一致() {
        assert_eq!(
            content_md5(br#"{"base":{"appid":"GM3LOOMY"}}"#),
            "ZXFsDbtKIeoeelrBB+sO7g=="
        );
        assert_eq!(content_md5(b""), "");
    }
}
