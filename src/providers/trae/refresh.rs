//! Trae 令牌续期（`ExchangeToken` + `RefreshToken` + `DeviceProof`）。
//!
//! 请求体与官方客户端的 `exchangeTokenByRefreshToken` 一致：
//! `{ClientID, ClientSecret:"", RefreshToken, DeviceInfo, DeviceProof, IDEVersion}`，
//! `DeviceProof` 用设备私钥对**该请求体**签名（见 [`super::proof`]）。

use serde::{Deserialize, Serialize};

use crate::providers::trae::api::{self, HttpError};
use crate::providers::trae::credential::Credential;
use crate::providers::trae::proof;

const EXCHANGE_PATH: &str = "/trae/api/v3/oauth/ExchangeToken";

#[derive(Debug, Serialize)]
struct RefreshReq {
    #[serde(rename = "ClientID")]
    client_id: &'static str,
    #[serde(rename = "ClientSecret")]
    client_secret: &'static str,
    #[serde(rename = "RefreshToken")]
    refresh_token: String,
    #[serde(rename = "DeviceInfo")]
    device_info: api::DeviceInfo,
    #[serde(rename = "DeviceProof")]
    device_proof: proof::DeviceProof,
    #[serde(rename = "IDEVersion")]
    ide_version: &'static str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenResult {
    #[serde(rename = "Token")]
    pub token: String,
    #[serde(rename = "RefreshToken", default)]
    pub refresh_token: Option<String>,
    #[serde(rename = "TokenExpireAt", default)]
    pub token_expire_at: Option<i64>,
    #[serde(rename = "RefreshExpireAt", default)]
    pub refresh_expire_at: Option<i64>,
}

/// 用 refresh_token 换新 token，成功后返回更新后的凭证（未写入磁盘）。
pub async fn refresh(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<Credential, HttpError> {
    let device = cred.device();
    let device_info = api::DeviceInfo::build(&device)?;

    // 签名 payload 为 RefreshToken 本身（官方 `pFe` 的第 4 个参数）。
    let device_proof = proof::sign(
        &cred.device_private_key_pem,
        "POST",
        EXCHANGE_PATH,
        api::CLIENT_ID,
        &cred.refresh_token,
    )
    .map_err(|e| HttpError::Decode(e.to_string()))?;

    let body = RefreshReq {
        client_id: api::CLIENT_ID,
        client_secret: "",
        refresh_token: cred.refresh_token.clone(),
        device_info,
        device_proof,
        ide_version: api::APP_VERSION,
    };

    let url = format!("{}{}", api::HOST_WEB, EXCHANGE_PATH);
    let res: TokenResult =
        api::post_control(client, &url, api::UA_ELECTRON, None, &body).await?;

    Ok(Credential {
        token: res.token,
        refresh_token: res.refresh_token.unwrap_or_else(|| cred.refresh_token.clone()),
        token_expire_at: res.token_expire_at.or(cred.token_expire_at),
        refresh_expire_at: res.refresh_expire_at.or(cred.refresh_expire_at),
        ..cred.clone()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::trae::device::Device;

    /// 真实续期（需要本地已登录凭证，故默认忽略）。
    /// 运行：`cargo test -- --ignored live_refresh`
    #[tokio::test]
    #[ignore]
    async fn live_refresh() {
        let root = std::path::Path::new("tmp/login-test");
        let cred = Credential::load(root).expect("读取").expect("已登录");
        let client = api::client().unwrap();
        let new = refresh(&client, &cred).await.expect("续期成功");
        assert!(!new.token.is_empty());
        println!("新 token 前缀: {}…", &new.token[..20]);
        println!("有效期: {:?} -> {:?}", cred.token_expire_at, new.token_expire_at);
    }
}
