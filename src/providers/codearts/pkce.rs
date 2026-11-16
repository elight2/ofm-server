//! PKCE（RFC 7636）——对齐官方扩展实现。
//!
//! 官方用 CryptoJS：verifier = `WordArray.random(64).toString(Hex)`（**64 个
//! hex 字符**，不是 RFC 建议的 base64url），challenge = base64url(SHA256(verifier))，
//! `code_challenge_method` 常量名为 **`SHA-256`**（不是 `S256`）。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use sha2::{Digest, Sha256};

/// verifier 长度（hex 字符数）。
const VERIFIER_HEX_LEN: usize = 64;
/// `code_challenge_method` 参数值（官方常量原文）。
pub const CODE_CHALLENGE_METHOD: &str = "SHA-256";

/// 生成 `code_verifier`：64 个小写 hex 字符（32 随机字节）。
pub fn new_verifier() -> String {
    let mut buf = [0u8; VERIFIER_HEX_LEN / 2];
    getrandom::fill(&mut buf).expect("获取随机数失败");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// 随机 UUID v4（`ticket_id` 用，官方为 `crypto.randomUUID()`）。
pub fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: Vec<String> = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        h[0..4].concat(),
        h[4..6].concat(),
        h[6..8].concat(),
        h[8..10].concat(),
        h[10..16].concat()
    )
}

/// `BASE64URL(SHA256(ASCII(verifier)))`，无填充。
pub fn challenge(verifier: &str) -> String {
    let mut h = Sha256::new();
    h.update(verifier.as_bytes());
    URL_SAFE_NO_PAD.encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_is_64_lower_hex() {
        let v = new_verifier();
        assert_eq!(v.len(), VERIFIER_HEX_LEN);
        assert!(v.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_ne!(v, new_verifier());
    }

    #[test]
    fn challenge_matches_rfc7636_example() {
        // RFC 7636 附录 B：输入即 ASCII verifier，S256 与 SHA-256 等价。
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(challenge(verifier), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }
}
