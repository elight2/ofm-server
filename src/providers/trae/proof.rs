//! 设备证明签名（`DeviceProof`）。
//!
//! 与官方客户端同算法（反编译 `out/main.js` 的 `pFe`）：
//!
//! ```text
//! timestamp = floor(now / 1000)            // 秒
//! nonce     = 16 字节随机数的 hex（32 字符）
//! message   = [METHOD, PATH, ClientID, payload, timestamp, nonce].join("\n")
//! signature = base64(DER(ECDSA_SHA256(device_private_key, message)))
//! ```
//!
//! `PATH` 只含路径（如 `/trae/api/v3/oauth/ExchangeToken`），不含 schema/host。
//! `payload` 是调用方传入的业务串：续期时官方传的是 **RefreshToken**（不是 JSON body），
//! 见反编译 `exchangeTokenByRefreshToken` 中 `pFe("POST", path, clientId, e, privateKey)`
//! 的 `e` 即为 RefreshToken。

use p256::ecdsa::SigningKey;
use p256::ecdsa::signature::Signer;
use p256::pkcs8::DecodePrivateKey;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct DeviceProof {
    #[serde(rename = "Signature")]
    pub signature: String,
    #[serde(rename = "Timestamp")]
    pub timestamp: i64,
    #[serde(rename = "Nonce")]
    pub nonce: String,
}

/// 计算设备证明。`payload` 由调用方按官方客户端对应字段传入。
pub fn sign(
    private_key_pem: &str,
    method: &str,
    path: &str,
    client_id: &str,
    payload: &str,
) -> Result<DeviceProof, ProofError> {
    let key = SigningKey::from_pkcs8_pem(private_key_pem)
        .map_err(|e| ProofError::Key(e.to_string()))?;

    let timestamp = now_secs();
    let nonce = hex16();

    let message = [
        method,
        path,
        client_id,
        payload,
        &timestamp.to_string(),
        &nonce,
    ]
    .join("\n");

    let sig: p256::ecdsa::Signature = key.sign(message.as_bytes());
    let sig = sig.to_der();
    // 官方客户端用 Node 的 base64（带填充）。
    let signature = base64::Engine::encode(
        &base64::engine::general_purpose::STANDARD,
        sig.as_bytes(),
    );

    Ok(DeviceProof {
        signature,
        timestamp,
        nonce,
    })
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn hex16() -> String {
    let mut b = [0u8; 16];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum ProofError {
    #[error("设备私钥不可用: {0}")]
    Key(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;
    use p256::pkcs8::EncodePrivateKey;

    #[test]
    fn proof_verifies_and_covers_body() {
        let sk = SigningKey::random(&mut rand_core::OsRng);
        let pem = sk.to_pkcs8_pem(Default::default()).unwrap().to_string();
        let body = "rt-1234567890";
        let proof = sign(&pem, "POST", "/trae/api/v3/oauth/ExchangeToken", "abc", body).unwrap();
        assert_eq!(proof.nonce.len(), 32);
        assert!(proof.timestamp > 1_600_000_000);

        let msg = [
            "POST",
            "/trae/api/v3/oauth/ExchangeToken",
            "abc",
            body,
            &proof.timestamp.to_string(),
            &proof.nonce,
        ]
        .join("\n");
        let der = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &proof.signature,
        )
        .unwrap();
        let sig = p256::ecdsa::Signature::from_der(&der).unwrap();
        sk.verifying_key().verify(msg.as_bytes(), &sig).unwrap();

        // 换一行内容签名便不匹配（证明 body 被覆盖）。
        let other = [msg.replace("abc", "abd")];
        assert!(sk.verifying_key().verify(other[0].as_bytes(), &sig).is_err());
    }
}
