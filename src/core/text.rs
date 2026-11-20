//! 文本与编码小工具：错误信息截断、十六进制编码。
//!
//! 这些都是「协议无关的纯函数」，各供应商原先各写一份（`truncate`/`snippet`
//! 共 15 处定义，且字节截断与字符截断混用），统一到这里。

/// 按**字节**截断到 `max` 以内，超出时补 `…`。
///
/// 保持字节上限（而非字符数）：各家原本就是按字节算的 `MAX`，日志/错误信息
/// 里截断长度要一致。为避免在多字节字符中间开口，回退到最近的字符边界。
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// 同 [`truncate_chars`]，但先 `trim`（用于把整段响应体塞进日志）。
pub fn truncate_trimmed(s: &str, max: usize) -> String {
    truncate_chars(s.trim(), max)
}

/// PKCE `S256(code_verifier)` = `BASE64URL(SHA256(ASCII(verifier)))`，无填充。
///
/// trae / codearts 两家的 `challenge()` 原本逐字相同，故合并到此。
pub fn s256_challenge(verifier: &str) -> String {
    use base64::Engine;
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// 小写十六进制编码。
pub fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // 写 String 不会失败。
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// `n` 字节随机数据的十六进制串；随机源失败时退化为时间派生（仍是合法 hex）。
pub fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    if getrandom::fill(&mut buf).is_err() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = ((nanos >> (i % 16 * 8)) as u8) ^ (i as u8);
        }
    }
    hex_lower(&buf)
}

/// 随机 UUID v4 形式（`xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx`）。
///
/// 随机源失败时退化为非 v4 位（仍可用作请求标识）。
pub fn uuid_v4() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    format!(
        "{}-{}-{}-{}-{}",
        hex_lower(&b[0..4]),
        hex_lower(&b[4..6]),
        hex_lower(&b[6..8]),
        hex_lower(&b[8..10]),
        hex_lower(&b[10..16])
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 截断不超长且不破字符() {
        assert_eq!(truncate_chars("abc", 10), "abc");
        assert_eq!(truncate_chars("abc", 3), "abc");
        assert_eq!(truncate_chars("abcd", 3), "abc…");
        assert_eq!(truncate_chars("你好世界", 0), "…");
        // 6 字节约两个汉字，第 7 字节正好在字符边界上。
        assert_eq!(truncate_chars("你好世界", 6), "你好…");
        assert_eq!(truncate_chars("你好世界", 2), "…");
        // `max` 落在多字节字符中间时回退到最近的边界，不产生非法 UTF-8。
        assert_eq!(truncate_chars("汉", 1), "…");
        assert_eq!(truncate_chars("汉", 3), "汉");
        assert_eq!(truncate_chars("", 5), "");
    }

    #[test]
    fn 截断先_trim() {
        assert_eq!(truncate_trimmed("  hi  ", 10), "hi");
        assert_eq!(truncate_trimmed("\n\t", 10), "");
    }

    #[test]
    fn hex_小写且定长() {
        assert_eq!(hex_lower(&[0x00, 0x0f, 0xff]), "000fff");
        assert_eq!(hex_lower(&[]), "");
        assert_eq!(random_hex(4).len(), 8);
        assert!(random_hex(4).bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn uuid_形状正确() {
        let u = uuid_v4();
        let parts: Vec<&str> = u.split('-').collect();
        assert_eq!(
            parts.iter().map(|p| p.len()).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert_eq!(&parts[2][..1], "4", "版本位应为 4");
        assert!(
            matches!(&parts[3][..1], "8" | "9" | "a" | "b"),
            "变体位非法: {u}"
        );
        assert_ne!(u, uuid_v4());
    }
}
