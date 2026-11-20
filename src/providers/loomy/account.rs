//! 讯飞账号网关（`account.xfinfr.com`）调用层。
//!
//! Loomy 客户端不自己维护账号体系，登录全部打讯飞账号网关。协议见
//! `provider-info/loomy.md` §1，签名见 [`super::sign`]。
//!
//! 业务信封统一为 `{"code":"000000","desc":"成功","data":{...}}`，
//! `code != 000000` 即失败（`desc` 为可直接展示的中文原因）。

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde_json::{Value, json};

use super::sign;

/// 账号网关基地址（生产）。
pub const BASE_URL: &str = "https://account.xfinfr.com";

/// 客户端打包在 `.env.prod` 里的应用凭证，服务端按 appid 校验。
const APP_ID: &str = "GM3LOOMY";
const ACCESS_KEY_ID: &str = "2thryby66wxi53sk";
const ACCESS_KEY_SECRET: &str = "zsak6eadrbawz683wf5r3m2snrwj868r";

/// 客户端写死的 modelid / devid 与 UA。
const MODEL_ID: &str = "Web";
const DEVICE_ID: &str = "web";
const UA: &str = "Loomy|Desktop|Electron|macOS";

/// 短信验证码有效期（秒），与客户端一致。
const SMS_EXPIRE_SECS: u64 = 300;

/// 登录态有效期（秒），14 天，与客户端一致。
pub const SESSION_EXPIRE_SECS: i64 = 14 * 24 * 3600;

/// 账号网关客户端。
pub struct AccountClient {
    client: Client,
}

impl AccountClient {
    pub fn new() -> Result<Self, reqwest::Error> {
        crate::tls::init();
        Ok(Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
        })
    }

    /// 发送短信验证码，返回 `msgid`（校验验证码时要带回）。
    pub async fn send_sms_code(&self, phone: &str) -> Result<String> {
        let body = json!({
            "base": base(),
            "param": { "ccode": "86", "phone": phone, "expire": SMS_EXPIRE_SECS },
        });
        let data = self.post("/login/phone/sendMsgCode", &body).await?;
        data.get("msgid")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("账号网关未返回 msgid")
    }

    /// 用验证码换取登录态。
    pub async fn login_by_sms_code(&self, phone: &str, code: &str, msgid: &str) -> Result<Session> {
        let body = json!({
            "base": base(),
            "param": {
                "ccode": "86",
                "phone": phone,
                "mcode": code,
                "msgid": msgid,
                "expire": SESSION_EXPIRE_SECS,
            },
        });
        let data = self.post("/login/phone/checkCode", &body).await?;
        let session = data
            .get("session")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("账号网关未返回 session")?;
        let userid = data
            .get("userid")
            .and_then(Value::as_str)
            .map(str::to_string)
            .context("账号网关未返回 userid")?;
        Ok(Session { session, userid })
    }

    /// 校验登录态是否仍有效（返回账号基础信息）。
    ///
    /// 失效时网关回 `020002` / `100002`，此处以错误形式返回。
    pub async fn query_base_info(&self, session: &str) -> Result<Value> {
        let body = json!({
            "base": base(),
            "param": { "session": session },
        });
        self.post("/userinfo/query/baseInfo", &body).await
    }

    /// 签名并发送一次 POST，返回信封里的 `data`。
    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let body = serde_json::to_vec(body).context("序列化请求体失败")?;
        let headers = sign::sign(
            ACCESS_KEY_ID,
            ACCESS_KEY_SECRET,
            "POST",
            path,
            &[],
            &body,
            "application/json",
            "account",
        );

        let mut http = self
            .client
            .post(format!("{BASE_URL}{path}"))
            .header("Authorization", &headers.authorization)
            .header("Date", &headers.date)
            .header("Nonce", &headers.nonce)
            .header("Content-Type", &headers.content_type)
            .body(body);
        if !headers.content_md5.is_empty() {
            http = http.header("Content-MD5", &headers.content_md5);
        }

        let resp = http.send().await.context("请求账号网关失败")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        let envelope: Value = serde_json::from_str(&text)
            .with_context(|| format!("账号网关返回非 JSON（HTTP {status}）: {}", snippet(&text)))?;

        let code = envelope.get("code").and_then(Value::as_str).unwrap_or("");
        if code != "000000" {
            let desc = envelope
                .get("desc")
                .and_then(Value::as_str)
                .unwrap_or("未知错误");
            bail!("账号网关返回 {code}: {desc}");
        }

        Ok(envelope.get("data").cloned().unwrap_or(Value::Null))
    }
}

/// 登录成功后的会话。
#[derive(Debug, Clone)]
pub struct Session {
    /// 会话令牌，同时用于账号网关与模型 API 的鉴权。
    pub session: String,
    /// 讯飞账号 id。
    pub userid: String,
}

/// 每次请求都要重造的 `base` 字段。
fn base() -> Value {
    json!({
        "appid": APP_ID,
        "modelid": MODEL_ID,
        "version": "1.0.0",
        "devid": DEVICE_ID,
        "ua": UA,
        "traceid": sign::trace_id(),
    })
}

fn snippet(s: &str) -> String {
    let t = s.trim();
    if t.chars().count() <= 200 {
        return t.to_string();
    }
    t.chars().take(200).collect::<String>() + "…"
}

#[cfg(test)]
mod live {
    use super::*;

    /// 真连生产网关：假 session 必须被判定为登录态无效（020002）。
    /// 需要网络，默认不跑：`cargo test live_ -- --ignored`。
    #[tokio::test]
    #[ignore]
    async fn live_签名与鉴权() {
        let c = AccountClient::new().unwrap();
        let err = c
            .query_base_info("bogus-session")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("020002"), "{err}");
    }
}
