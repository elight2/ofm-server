//! PKCE（RFC 7636）与授权 URL 构造。

use base64::Engine;
const VERIFIER_LEN: usize = 64;

/// 生成 `code_verifier`：43..128 位 base64url 字符集。
pub fn new_verifier() -> String {
    let mut buf = vec![0u8; VERIFIER_LEN];
    // getrandom 失败时降级为时间派生（仍能完成登录，仅随机性下降）。
    if getrandom::fill(&mut buf).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((nanos >> (i % 16 * 8)) as u8) ^ (i as u8);
        }
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&buf)
}

/// `BASE64URL(SHA256(ASCII(verifier)))`，无填充。
pub use crate::core::text::s256_challenge as challenge;

/// 随机 UUID v4 形式（`login_trace_id` 用）。
pub use crate::core::text::uuid_v4;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s256_matches_rfc7636_example() {
        // RFC 7636 附录 B 的官方用例。
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_charset_ok() {
        let v = new_verifier();
        assert!(v.len() >= 43 && v.len() <= 128);
        assert!(
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }

    #[test]
    fn uuid_shape() {
        let u = uuid_v4();
        assert_eq!(u.len(), 36);
        assert_eq!(u.as_bytes()[14], b'4');
    }
}
