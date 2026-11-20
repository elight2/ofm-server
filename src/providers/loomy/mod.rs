//! Loomy 供应商（讯飞 / iFlyTek）。
//!
//! 已实现：登录（`auth`，讯飞账号手机号 + 短信验证码）、模型目录（`catalog`）、
//! 对话（`chat` + `stream`）、思考强度映射、余额（`balance`）、每日福利
//! （`claim`，即「宠物打工奖励」）。
//!
//! 协议实况见 `provider-info/loomy.md`；登录签名见 `sign`，账号网关见 `account`。

pub mod account;
pub mod api;
pub mod auth;
pub mod balance;
pub mod catalog;
pub mod chat;
pub mod claim;
pub mod credential;
pub mod sign;
pub mod stream;

use std::path::PathBuf;

use async_trait::async_trait;

use crate::config::LoomyReasoningEffort;
use crate::core::claim::SkipReason;
use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};
use crate::core::text::truncate_chars;

use api::VERSION;
use credential::Credential;

pub const ID: &str = "loomy";

pub struct LoomyProvider {
    root: PathBuf,
    client: reqwest::Client,
    /// 思考强度。
    effort: LoomyReasoningEffort,
}

impl LoomyProvider {
    pub fn new(root: PathBuf, effort: LoomyReasoningEffort) -> anyhow::Result<Self> {
        Ok(Self {
            root,
            client: api::client()?,
            effort,
        })
    }

    fn credential(&self) -> Result<Credential, ProviderError> {
        crate::core::credential::require(&self.root, ID)
    }

    /// 取可用 session。
    ///
    /// 讯飞账号登录态 14 天有效且无法续期，过期只能重新登录，故这里先本地判过期，
    /// 免得把注定失败的请求打到上游。
    async fn access_token(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        if cred.is_expired() {
            return Err(ProviderError::Unauthorized(format!(
                "登录态已过期，请重新运行 `ofm-server login {ID}`"
            )));
        }
        Ok(cred.session.clone())
    }

    /// 鉴权头：`Authorization: Bearer` 与 `token` 同时发登录态。
    ///
    /// 实测两者各自单独生效，但官方客户端（`useSessionAuth`）两个都发，照抄最稳。
    fn auth_headers(&self, token: &str) -> Vec<(&'static str, String)> {
        vec![
            ("Authorization", format!("Bearer {token}")),
            ("token", token.to_string()),
            ("Accept", "application/json".to_string()),
        ]
    }

    /// 对话请求头。
    ///
    /// 客户端会额外带 `traceparent` / `ChatId` / `MsgId` / `loomy-version`。
    /// 实测这些**都不是必需**（缺失时仍正常返回），但带上与官方客户端一致，
    /// 便于上游排障时对齐链路。
    fn chat_headers(&self, token: &str) -> Vec<(&'static str, String)> {
        let mut headers = self.auth_headers(token);
        headers.push(("Content-Type", "application/json".to_string()));
        headers.push(("traceparent", sign::traceparent()));
        headers.push(("ChatId", sign::trace_id()));
        headers.push(("MsgId", format!("msg-{}", sign::trace_id())));
        headers.push(("loomy-version", VERSION.to_string()));
        headers
    }

    /// 校验登录态是否仍被上游接受：`GET /models`。
    ///
    /// 业务端点（`/models`、`/points/*`）只认小写 `token` 头，故这里**只发
    /// `token`**；它与对话接口共用同一会话，但不受对话限流影响。
    ///
    /// 返回 `Some(true)` 有效、`Some(false)` 明确失效、`None` 无法判断
    /// （网络失败、限流码、非信封响应等）。
    async fn session_alive(&self, token: &str) -> Option<bool> {
        let url = format!("{}/models", api::MODEL_BASE_URL);
        let resp = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .header("token", token)
            .send()
            .await
            .ok()?;
        let status = resp.status().as_u16();
        let text = resp.text().await.ok()?;
        probe_result(status, &text)
    }

    /// 对话接口的业务信封 → [`ProviderError`]。
    ///
    /// 上游对**限流 / 风控**也回 `100002`（文案固定「登录已失效，请重新登录」），
    /// 与真正的登录态失效无法从响应体区分。此类错误先探一次 `/models`：
    /// 登录态仍有效即按限流处理（下游 429），否则才是真的需要重新登录。
    async fn chat_biz_error(&self, token: &str, code: &str, desc: &str) -> ProviderError {
        use ProviderError::*;
        match code {
            "100002" | "020002" | "200003" => {
                // 「缺少 token」是请求头没带上，不是登录态问题，别拿 /models 的
                // 成功结果把它洗成限流。
                if desc.contains("缺少") {
                    return Protocol(format!("{code}: {desc}"));
                }
                let alive = self.session_alive(token).await;
                if alive == Some(true) {
                    tracing::warn!(
                        "[{ID}] 对话被上游以 {code} 拒绝（{desc}），但 /models 校验登录态有效，按限流处理"
                    );
                }
                classify_auth_error(code, desc, alive)
            }
            "100003" | "100005" | "429" => RateLimit(format!("{code}: {desc}")),
            _ => Protocol(format!("{code}: {desc}")),
        }
    }
}

#[async_trait]
impl Provider for LoomyProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 请求模型目录。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let token = self.access_token().await?;
        let url = format!("{}/models", api::MODEL_BASE_URL);

        let mut http = self.client.get(&url);
        for (k, v) in self.auth_headers(&token) {
            http = http.header(k, v);
        }

        let data: serde_json::Value = api::get_envelope(http)
            .await
            .map_err(ProviderError::Protocol)?;

        let models = catalog::parse_models(&data);
        tracing::debug!("[{ID}] 模型目录共 {} 个可对话模型", models.len());
        Ok(models)
    }

    /// 登录态到期时间。
    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .and_then(|c| c.expire_at)
    }

    /// 对话（上游恒回 SSE，非流式由下游聚合）。
    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError> {
        let token = self.access_token().await?;
        let body = chat::build_body(&req, self.effort);
        let url = format!("{}/chat/completions", api::MODEL_BASE_URL);

        // 自己序列化并 `.body()`：避免 `.json()` 追加第二个 content-type 头。
        let payload = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Other(format!("序列化请求体失败: {e}")))?;
        let mut http = self.client.post(&url).body(payload);
        for (k, v) in self.chat_headers(&token) {
            http = http.header(k, v);
        }

        let resp = http.send().await?;
        if !resp.status().is_success() {
            let (status, text) = crate::core::http::error_body(resp, 300).await;
            return Err(map_http_error(status, &text));
        }

        // 认证失效是 HTTP 200 + 业务信封（非 SSE）。此时 `content-type` 是
        // `application/json`，SSE 解码器也解不出帧，必须在这里拦下。
        if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE)
            && !ct.to_str().unwrap_or("").contains("event-stream")
        {
            let text = resp.text().await.unwrap_or_default();
            let v: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| ProviderError::Protocol(format!("上游返回未知响应: {e}")))?;
            return Err(match api::biz_error(&v) {
                Some((code, desc)) => self.chat_biz_error(&token, &code, &desc).await,
                None => ProviderError::Protocol(format!(
                    "上游返回预期外响应: {}",
                    truncate_chars(&text, 200)
                )),
            });
        }

        let mut parser = stream::FrameParser::new();
        Ok(crate::core::stream::drive(resp.bytes_stream(), move |ev| {
            parser.parse(ev)
        }))
    }

    /// 主动刷新登录态。
    ///
    /// 讯飞账号不提供续期令牌：`session` 有效期 14 天，过期只能重新登录
    /// （`ofm-server login loomy`）。
    async fn refresh(&self) -> Result<String, ProviderError> {
        Err(ProviderError::Unsupported(ID))
    }

    /// 当前余额（`/points/balance` 的 `availableBalance`）。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        let token = self.access_token().await?;
        let b = balance::fetch(&self.client, &self.auth_headers(&token))
            .await
            .map_err(ProviderError::Protocol)?;
        Ok(Some(b.main_points()))
    }

    /// 领取今日福利（宠物打工奖励）。
    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        let token = self.access_token().await?;
        let claimed = claim::claim(&self.client, &self.auth_headers(&token))
            .await
            .map_err(ProviderError::Protocol)?;

        // 领到分之后余额自然是新值，顺手回读，便于 `claim.log` 对账。
        let balance = if claimed.outcome.code == crate::core::claim::ClaimCode::Success {
            self.balance().await.ok().flatten()
        } else {
            None
        };

        Ok(ClaimReport {
            outcome: claimed.outcome,
            credits_delta: claimed.credits_delta,
            balance,
        })
    }
}

/// HTTP 层错误映射（对话接口用；积分类接口走 `api::get_envelope`）。
fn map_http_error(status: u16, body: &str) -> ProviderError {
    // 兼容 `{"error":{"message":...}}` 与业务信封两种形态。
    let msg = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|v| {
            api::biz_error(&v)
                .map(|(c, d)| format!("{c}: {d}"))
                .or_else(|| {
                    v.get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string())
                })
        })
        .unwrap_or_else(|| truncate_chars(body, 300));

    match status {
        401 | 403 => ProviderError::Unauthorized(msg),
        429 => ProviderError::RateLimit(msg),
        404 => ProviderError::Protocol(format!("模型不可用: {msg}")),
        _ => ProviderError::Protocol(format!("上游 HTTP {status}: {msg}")),
    }
}

/// 解析 `/models` 探测结果。
///
/// `Some(true)`：信封成功且带 `data`；`Some(false)`：明确回登录态失效码；
/// 其它（含 504 HTML、限流码）一律 `None`——不能据此判断登录态。
fn probe_result(status: u16, text: &str) -> Option<bool> {
    let env: api::Envelope<serde_json::Value> = serde_json::from_str(text).ok()?;
    if status == 200 && (env.code.is_empty() || env.code == api::OK_CODE) {
        return env.data.map(|_| true);
    }
    match env.code.as_str() {
        "100002" | "020002" | "200003" => Some(false),
        _ => None,
    }
}

/// 登录态类业务码的最终归类。
///
/// `alive == Some(true)` 表示 `/models` 证明凭证本身没问题，此时上游的
/// 「登录已失效」只能是限流 / 风控，不能把下游引去重新登录。
fn classify_auth_error(code: &str, desc: &str, alive: Option<bool>) -> ProviderError {
    if alive == Some(true) {
        ProviderError::RateLimit(format!(
            "{desc}（上游返回 {code}，但 /models 校验登录态仍有效：疑似限流，请稍后重试）"
        ))
    } else {
        ProviderError::Unauthorized(desc.to_string())
    }
}

/// 签到未启用时的报告（供将来对齐 `Provider` 语义时使用）。
#[allow(dead_code)]
fn not_enabled() -> ClaimReport {
    ClaimReport {
        outcome: crate::core::claim::ClaimOutcome::skipped(SkipReason::NotEnabled, vec![]),
        credits_delta: None,
        balance: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 上游 400 参数错误框 → Protocol 错误里带中文文案。
    #[test]
    fn 参数错误映射() {
        let e = map_http_error(
            400,
            r#"{"error":{"message":"请求参数错误","type":"invalid_request_error","code":400}}"#,
        );
        assert!(
            matches!(&e, ProviderError::Protocol(m) if m.contains("请求参数错误")),
            "{e:?}"
        );
    }

    /// 业务信封形态的错误也应认出中文 desc。
    #[test]
    fn 业务信封错误映射() {
        let e = map_http_error(200, r#"{"code":"100002","desc":"登录已失效"}"#);
        assert!(
            matches!(&e, ProviderError::Protocol(m) if m.contains("登录已失效")),
            "{e:?}"
        );
    }

    #[test]
    fn 非json错误体截断() {
        let e = map_http_error(502, "<html>bad gateway</html>");
        assert!(
            matches!(&e, ProviderError::Protocol(m) if m.contains("HTTP 502")),
            "{e:?}"
        );
    }

    /// 上游 HTTP 429 必须原样落成限流，而不是「上游响应异常」。
    #[test]
    fn http_429_映射为限流() {
        let e = map_http_error(429, r#"{"error":{"message":"请求过于频繁"}}"#);
        assert!(
            matches!(&e, ProviderError::RateLimit(m) if m.contains("请求过于频繁")),
            "{e:?}"
        );
    }

    /// `/models` 探测结果：成功信封 → true，登录态失效码 → false，其余不可判断。
    #[test]
    fn 登录态探测结果() {
        assert_eq!(
            probe_result(200, r#"{"code":"000000","desc":"成功","data":[]}"#),
            Some(true)
        );
        assert_eq!(
            probe_result(200, r#"{"code":"100002","desc":"登录已失效"}"#),
            Some(false)
        );
        assert_eq!(probe_result(504, "<html>504 Gateway Time-out</html>"), None);
        // 限流码不能证明登录态好坏。
        assert_eq!(
            probe_result(200, r#"{"code":"100005","desc":"请求过于频繁"}"#),
            None
        );
    }

    /// 登录态仍有效时，100002 按限流处理；无法判断时保持原来的认证失败。
    #[test]
    fn 登录态有效则_100002_归为限流() {
        let e = classify_auth_error("100002", "登录已失效，请重新登录", Some(true));
        assert!(
            matches!(&e, ProviderError::RateLimit(m) if m.contains("疑似限流")),
            "{e:?}"
        );

        for alive in [Some(false), None] {
            let e = classify_auth_error("100002", "登录已失效，请重新登录", alive);
            assert!(
                matches!(&e, ProviderError::Unauthorized(m) if m == "登录已失效，请重新登录"),
                "{alive:?} → {e:?}"
            );
        }
    }
}
