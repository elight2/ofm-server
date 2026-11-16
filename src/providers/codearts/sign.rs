//! 华为云 SDK-HMAC-SHA256 签名（`Authorization: SDK-HMAC-SHA256 ...`）。
//!
//! 逐行复刻内核 x86 版的 AKSKSigner 类（`agentkernelServer-linux-x64`
//! @110394110，与华为云官方 SDK 同源），包括两个容易踩坑的细节：
//!
//! 1. canonical URI 强制追加尾斜杠（`/api/v1/gateway/config` →
//!    `/api/v1/gateway/config/`），不照抄则服务端签名校验失败；
//! 2. `urlEncode` 按 RFC3986 非保留字符集转义（`-._~` 不转义），
//!    percent 序列**大写**十六进制。
//!
//! benefit-service 的调用约定（`PFn`，@101232623）：请求对象不携带
//! body → payload hash 恒为空体 SHA256；签名用的头为
//! `content-type` / `x-snap-traceid` / `X-Security-token`（可选），
//! 签名器补 `host` 与 `X-Sdk-Date`。

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

/// 空请求体的 SHA256（官方常量原文）。
pub const EMPTY_BODY_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

type HmacSha256 = Hmac<Sha256>;

/// 待签请求。`query` 有值时以 `key=value` 拼在 URL 后（值需已转义）。
pub struct Request<'a> {
    pub method: &'a str,
    /// 完整 URL（官方签名前先解析出 host/pathname）。
    pub url: &'a str,
    /// 参与签名的业务头（小写书写）。不含 `host` / `X-Sdk-Date` / `Authorization`。
    pub headers: Vec<(&'a str, String)>,
    /// 查询串（**值需已 percent-encode**），如 `Some("offset=0&limit=100")`。
    /// 签名按官方规则重排：键排序后逐键 `urlEncode`。
    pub query: Option<&'a str>,
    /// 请求体（可选）。**实测（2026-02-18）：服务端对 POST body 的 payload
    /// hash 强校验**——发 `{}` body 必须签 `sha256("{}")`，签空体 hash 会
    /// 得 `APIG.0301`。无 body 时才用空体 SHA256。SDK 语义：有 body 哈希
    /// body，无 body 用空体 hash（SDK-HMAC-SHA256 后端不支持免签 hash）。
    pub body: Option<&'a str>,
}

/// 签名结果：返回应实际发送的头（追加 `host` / `X-Sdk-Date` / `Authorization`）。
///
/// 与官方一致：返回值即请求头全集，调用方原样作为 fetch headers。
pub fn sign(req: &Request<'_>, ak: &str, sk: &str) -> Result<Vec<(String, String)>, String> {
    let parsed = url::Url::parse(req.url).map_err(|e| format!("解析 URL 失败: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| "URL 缺少 host".to_string())?
        .to_string();
    let host = match (parsed.scheme(), parsed.port()) {
        ("http", Some(p)) if p != 80 => format!("{host}:{p}"),
        ("https", Some(p)) if p != 443 => format!("{host}:{p}"),
        _ => host,
    };
    // 官方用 `url.parse(endpoint, true)`（query2=false）后取 pathname：
    // 即「?」之前的部分；#fragment 不出现在请求 URL 中。
    let full = parsed.path();
    let pathname = match full.split_once('?') {
        Some((p, _)) => p.to_string(),
        None => full.to_string(),
    };
    // 官方 parse 的 pathname 恒以 / 开头；防御空路径。
    let pathname = if pathname.starts_with('/') {
        pathname
    } else {
        format!("/{pathname}")
    };

    // X-Sdk-Date：未提供则取当前 UTC 时间（BasicDateFormat）。
    let (sdk_date, provided_date) = match req
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-sdk-date"))
    {
        Some((_, v)) => (normalize_sdk_date(v), true),
        None => (sdk_date_now(), false),
    };

    // 合并签名头：业务头（含调用方提供的 X-Sdk-Date，官方语义：始终参与签名）
    // + host + X-Sdk-Date（仅当本次生成时补加）。
    let mut signed: Vec<(String, String)> = req
        .headers
        .iter()
        .map(|(k, v)| (k.to_lowercase(), v.clone()))
        .collect();
    signed.push(("host".to_string(), host));
    if !provided_date {
        signed.push(("x-sdk-date".to_string(), sdk_date.clone()));
    }

    // SignedHeaders：键按 lower-case 排序（稳定排序，重复键按原始顺序），
    // 重复键只出现在列表一次（官方 `Object.keys` 语义）。
    let mut order: Vec<String> = Vec::new();
    for (k, _) in &signed {
        if !order.iter().any(|o| o == k) {
            order.push(k.clone());
        }
    }
    order.sort_by_key(|k| k.to_lowercase());
    let signed_headers = order.join(";");

    // CanonicalHeaders：与 SignedHeaders 同序，`k:v\n`。
    let mut canonical_headers = String::new();
    for k in &order {
        let v = signed
            .iter()
            .find(|(sk, _)| sk == k)
            .map(|(_, v)| v.as_str())
            .unwrap_or("");
        canonical_headers.push_str(&format!("{k}:{v}\n"));
    }

    let canonical_uri = canonical_uri(&pathname);
    let canonical_query = canonical_query(req.query);
    // payload hash：有 body 哈希 body，无 body 用空体 SHA256（见 Request::body 注释）。
    let payload_hash = match req.body {
        Some(b) => hex_lower(&Sha256::digest(b.as_bytes())),
        None => EMPTY_BODY_SHA256.to_string(),
    };
    let canonical_request = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        req.method, canonical_uri, canonical_query, canonical_headers, signed_headers, payload_hash
    );

    let hashed = hex_lower(&Sha256::digest(canonical_request.as_bytes()));
    let string_to_sign = format!("SDK-HMAC-SHA256\n{sdk_date}\n{hashed}");
    let mut mac = HmacSha256::new_from_slice(sk.as_bytes())
        .map_err(|e| format!("HMAC 密钥无效: {e}"))?;
    mac.update(string_to_sign.as_bytes());
    let signature = hex_lower(&mac.finalize().into_bytes());

    let authorization = format!(
        "SDK-HMAC-SHA256 Access={ak}, SignedHeaders={signed_headers}, Signature={signature}"
    );

    let mut out: Vec<(String, String)> = signed;
    out.push(("authorization".to_string(), authorization));
    Ok(out)
}

/// CanonicalURI：逐段转义后**强制追加尾斜杠**（官方实现如此，勿"修复"）。
fn canonical_uri(pathname: &str) -> String {
    let joined = pathname
        .split('/')
        .map(url_encode)
        .collect::<Vec<_>>()
        .join("/");
    if joined.ends_with('/') {
        joined
    } else {
        format!("{joined}/")
    }
}

/// CanonicalQueryString：键排序，逐对 `urlEncode(k)=urlEncode(v)`。
/// 输入是已拼好的 query 串（值已转义），先拆开再按官方规则重新转义。
fn canonical_query(query: Option<&str>) -> String {
    let Some(q) = query else {
        return String::new();
    };
    let mut pairs: Vec<(String, String)> = q
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k.to_string(), v.to_string()),
            None => (pair.to_string(), String::new()),
        })
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
        .into_iter()
        .map(|(k, v)| format!("{}={}", url_encode(&k), url_encode(&v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// RFC3986 非保留字符不转义，其余转 `%HH`（大写 hex）。逐字节处理，
/// 对非 ASCII 按官方 hexTable 的 UTF-8 字节路径转义。
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// `X-Sdk-Date` 当前 UTC 时间：`YYYYMMDDTHHMMSSZ`。
fn sdk_date_now() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        now.month() as u8,
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

/// 调用方显式提供的 `X-Sdk-Date` 原样传给签名器时的行为（官方 `sign`）：
/// 仅在可解析为日期时规范化为 `YYYYMMDDTHHMMSSZ`，否则原样使用。
/// 我们只生成本地头，不依赖此分支，但保持语义完整。
fn normalize_sdk_date(v: &str) -> String {
    if let Ok(dt) = time::OffsetDateTime::parse(
        v,
        &time::format_description::well_known::Rfc3339,
    ) {
        return format!(
            "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
            dt.year(),
            dt.month() as u8,
            dt.day(),
            dt.hour(),
            dt.minute(),
            dt.second()
        );
    }
    v.to_string()
}

fn hex_lower(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Python 参照实现（同算法离线计算）生成的固定向量。
    const EXPECTED_AUTH: &str = "SDK-HMAC-SHA256 Access=test-ak, SignedHeaders=content-type;host;x-sdk-date;x-snap-traceid, Signature=37e22403cc9a9068a1cd8ce6baf4182e03811fc6d26820bcf9bb41080f65405c";

    fn benefit_like_headers() -> Vec<(&'static str, String)> {
        vec![
            ("content-type", "application/json".to_string()),
            (
                "x-snap-traceid",
                "0123456789abcdef0123456789abcdef".to_string(),
            ),
            // 固定签名时间以匹配离线参考向量。
            ("x-sdk-date", "20260218T000000Z".to_string()),
        ]
    }

    #[test]
    fn sign_gateway_config_matches_reference() {
        let req = Request {
            method: "GET",
            url: "https://opengw.developer.huaweicloud.com/api/v1/gateway/config",
            headers: benefit_like_headers(),
            query: None,
            body: None,
        };
        let out = sign(&req, "test-ak", "test-sk").expect("签名失败");
        let auth = out
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.clone())
            .expect("缺 Authorization");
        assert_eq!(auth, EXPECTED_AUTH);
        // 签名器补齐的头。
        for (k, _) in [("host", ""), ("x-sdk-date", "")] {
            assert!(out.iter().any(|(kk, _)| kk == k), "缺头 {k}");
        }
        // 官方语义：签名的头 + Authorization 就是实际发送的头。
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn post_body_hash_differs_from_empty() {
        // 实测：POST body={} 时 payload hash 必须是 sha256("{}")，
        // 签空体 hash 会被服务端拒（APIG.0301）。
        let url = "https://opengw.developer.huaweicloud.com/api/v1/benefit/claim";
        let headers = vec![
            ("content-type", "application/json".to_string()),
            ("x-sdk-date", "20260218T000000Z".to_string()),
        ];
        let empty = Request { method: "POST", url, headers: headers.clone(), query: None, body: None };
        let with_body = Request { method: "POST", url, headers, query: None, body: Some("{}") };
        let auth = |r: &Request| {
            sign(r, "test-ak", "test-sk")
                .expect("签名失败")
                .into_iter()
                .find(|(k, _)| k == "authorization")
                .map(|(_, v)| v)
                .expect("缺 Authorization")
        };
        let a = auth(&empty);
        let b = auth(&with_body);
        assert_ne!(a, b, "body 参与签名未生效");
    }

    #[test]
    fn canonical_uri_appends_trailing_slash() {
        assert_eq!(
            canonical_uri("/api/v1/gateway/config"),
            "/api/v1/gateway/config/"
        );
        assert_eq!(canonical_uri("/a/b/"), "/a/b/");
    }

    #[test]
    fn url_encode_rfc3986() {
        assert_eq!(url_encode("aB9-._~"), "aB9-._~");
        assert_eq!(url_encode("a b"), "a%20b");
        assert_eq!(url_encode("值"), "%E5%80%BC");
    }

    #[test]
    fn canonical_query_sorted_and_encoded() {
        assert_eq!(
            canonical_query(Some("offset=0&limit=100")),
            "limit=100&offset=0"
        );
        assert_eq!(canonical_query(Some("k=a b")), "k=a%20b");
    }
}
