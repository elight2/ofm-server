//! DPoP（RFC 9449）证明 JWT——对齐官方扩展实现。
//!
//! 官方（扩展 `zti`/`Kti`）：
//! * 密钥：ECDSA P-256（`crypto.subtle.generateKey`，即 ES256）；
//! * header：`{alg:"ES256", typ:"dpop+jwt", jwk:<公钥 JWK>}`；
//! * claims：`{htm, htu, iat, jti}`，`jti` = 32 随机字节的 hex（64 字符）；
//! * 签名：JOSE 风格 ES256 = `P-256(SHA256(ascii(header.claims)))`，签名取
//!   **raw R||S（64 字节）** 再 base64url，非 DER。
//!
//! 密钥对必须持久化（扩展把 `dpopKeyPair` 与登录上下文一起保存，续期复用）。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use serde::{Deserialize, Serialize};

/// 公钥 JWK（持久化到凭证，请求头里原样携带）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Jwk {
    pub kty: String,
    pub crv: String,
    pub x: String,
    pub y: String,
}

/// DPoP 密钥对（持久化形态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyPair {
    /// PKCS#8 PEM 私钥。
    pub private_key_pem: String,
    pub public_jwk: Jwk,
}

impl KeyPair {
    /// 生成新的 P-256 密钥对。
    pub fn generate() -> Result<Self, DpopError> {
        let secret = p256::SecretKey::random(&mut rand_core::OsRng);
        let private_key_pem = secret
            .to_pkcs8_pem(LineEnding::LF)
            .map_err(|e| DpopError::Key(e.to_string()))?
            .to_string();
        let jwk = public_key_jwk(&secret.public_key())?;
        Ok(Self {
            private_key_pem,
            public_jwk: jwk,
        })
    }

    fn signing_key(&self) -> Result<SigningKey, DpopError> {
        SigningKey::from_pkcs8_pem(&self.private_key_pem).map_err(|e| DpopError::Key(e.to_string()))
    }

    /// 构造 DPoP 证明 JWT。`htu` 必须与实际请求的 URL 完全一致（含查询串）。
    pub fn proof(&self, htm: &str, htu: &str) -> Result<String, DpopError> {
        let key = self.signing_key()?;
        let header = serde_json::json!({
            "alg": "ES256",
            "typ": "dpop+jwt",
            "jwk": self.public_jwk,
        });
        let claims = serde_json::json!({
            "htm": htm,
            "htu": htu,
            "iat": crate::core::clock::now_secs(),
            "jti": jti(),
        });
        let b64 = |v: &serde_json::Value| {
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).expect("序列化 JSON 不会失败"))
        };
        let signing_input = format!("{}.{}", b64(&header), b64(&claims));
        let sig: Signature = key.sign(signing_input.as_bytes());
        // JOSE ES256：raw R||S（64 字节）。
        let mut raw = [0u8; 64];
        let (r, s) = sig.split_scalars();
        raw[..32].copy_from_slice(&r.to_bytes());
        raw[32..].copy_from_slice(&s.to_bytes());
        Ok(format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(raw)))
    }
}

/// 公钥 → JWK（RFC 7517，坐标为无填充 base64url）。
fn public_key_jwk(public: &p256::PublicKey) -> Result<Jwk, DpopError> {
    let point = public.to_encoded_point(false);
    let x = point
        .x()
        .ok_or_else(|| DpopError::Key("公钥缺少 x 坐标".into()))?;
    let y = point
        .y()
        .ok_or_else(|| DpopError::Key("公钥缺少 y 坐标".into()))?;
    Ok(Jwk {
        kty: "EC".into(),
        crv: "P-256".into(),
        x: URL_SAFE_NO_PAD.encode(x),
        y: URL_SAFE_NO_PAD.encode(y),
    })
}

/// 32 随机字节的 hex（64 字符），对齐官方 `WordArray.random(32).toString(Hex)`。
fn jti() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("获取随机数失败");
    crate::core::text::hex_lower(&buf)
}

#[derive(Debug, thiserror::Error)]
pub enum DpopError {
    #[error("DPoP 密钥处理失败: {0}")]
    Key(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
    use p256::ecdsa::VerifyingKey;

    #[test]
    fn proof_verifies_and_has_three_segments() {
        let kp = KeyPair::generate().unwrap();
        let token = kp
            .proof("POST", "https://sts.example.com/v1/oauth2/tokens")
            .unwrap();
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);

        // header/claims 可解且字段正确
        let header: serde_json::Value =
            serde_json::from_slice(&B64.decode(parts[0]).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert_eq!(header["typ"], "dpop+jwt");
        assert_eq!(header["jwk"]["crv"], "P-256");
        let claims: serde_json::Value =
            serde_json::from_slice(&B64.decode(parts[1]).unwrap()).unwrap();
        assert_eq!(claims["htm"], "POST");
        assert_eq!(claims["htu"], "https://sts.example.com/v1/oauth2/tokens");
        assert_eq!(claims["jti"].as_str().unwrap().len(), 64);

        // 签名可用 JWK 验证（raw R||S）
        let sig = Signature::from_slice(&B64.decode(parts[2]).unwrap()).unwrap();
        let x: Vec<u8> = B64.decode(kp.public_jwk.x.as_bytes()).unwrap();
        let y: Vec<u8> = B64.decode(kp.public_jwk.y.as_bytes()).unwrap();
        let pk = p256::PublicKey::from_sec1_bytes(&[[0x04].as_slice(), &x, &y].concat()).unwrap();
        let verify_key = VerifyingKey::from(pk);
        use p256::ecdsa::signature::Verifier;
        let input = format!("{}.{}", parts[0], parts[1]);
        verify_key
            .verify(input.as_bytes(), &sig)
            .expect("签名应有效");
    }

    #[test]
    fn jti_is_64_lower_hex() {
        assert_eq!(jti().len(), 64);
        assert!(
            jti()
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
    }
}
