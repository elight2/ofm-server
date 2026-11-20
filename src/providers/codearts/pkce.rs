//! PKCE（RFC 7636）——对齐官方扩展实现。
//!
//! 官方用 CryptoJS：`VERIFIER_LENGTH = 128`，verifier =
//! `WordArray.random(128 / 2).toString(Hex)`，即 **128 个 hex 字符（64 字节）**，
//! 不是 RFC 建议的 base64url。challenge = base64url(SHA256(verifier))，
//! `code_challenge_method` 常量名为 **`SHA-256`**（不是 `S256`）。

/// verifier 长度（hex 字符数）。官方 `VERIFIER_LENGTH = 128`。
const VERIFIER_HEX_LEN: usize = 128;
/// `code_challenge_method` 参数值（官方常量原文）。
pub const CODE_CHALLENGE_METHOD: &str = "SHA-256";

/// 生成 `code_verifier`：128 个小写 hex 字符（64 随机字节）。
pub fn new_verifier() -> String {
    let mut buf = [0u8; VERIFIER_HEX_LEN / 2];
    getrandom::fill(&mut buf).expect("获取随机数失败");
    crate::core::text::hex_lower(&buf)
}

/// 随机 UUID v4（`ticket_id` 用，官方为 `crypto.randomUUID()`）。
pub use crate::core::text::uuid_v4;

/// `BASE64URL(SHA256(ASCII(verifier)))`，无填充。
pub use crate::core::text::s256_challenge as challenge;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_is_128_lower_hex() {
        let v = new_verifier();
        assert_eq!(v.len(), VERIFIER_HEX_LEN);
        assert!(
            v.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_ne!(v, new_verifier());
    }

    #[test]
    fn challenge_matches_rfc7636_example() {
        // RFC 7636 附录 B：输入即 ASCII verifier，S256 与 SHA-256 等价。
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
