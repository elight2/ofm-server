//! WorkBuddy 供应商（腾讯 WorkBuddy 中国版）。
//!
//! 已实现：登录（`auth`，浏览器授权 + 轮询换令牌）、模型目录（`models`）、
//! 对话（`chat` + `stream`）、令牌续期（`refresh`）、余额（`balance`）、
//! 每日签到（`claim`）。
//!
//! 协议实况见 `provider-info/workbuddy.md`。

pub mod api;
pub mod auth;
pub mod balance;
pub mod chat;
pub mod claim;
pub mod credential;
pub mod models;
pub mod refresh;
pub mod stream;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use credential::Credential;

pub const ID: &str = "workbuddy";

pub struct WorkBuddyProvider {
    root: PathBuf,
    client: reqwest::Client,
    /// 续期必须串行，避免并发刷新导致 refresh_token 轮换冲突。
    refresh_lock: Arc<Mutex<()>>,
}

impl WorkBuddyProvider {
    pub fn new(root: PathBuf) -> anyhow::Result<Self> {
        Ok(Self {
            root,
            client: api::client()?,
            refresh_lock: Arc::new(Mutex::new(())),
        })
    }

    fn credential(&self) -> Result<Credential, ProviderError> {
        crate::core::credential::require(&self.root, ID)
    }

    /// 取可用凭证；已进入刷新窗口则先续期。
    async fn live_credential(&self) -> Result<Credential, ProviderError> {
        let client = &self.client;
        let root = &self.root;
        crate::core::credential::renew_if_expired(
            root,
            ID,
            &self.refresh_lock,
            |c: &Credential| c.needs_refresh(),
            |c: Credential| async move {
                let new = refresh::refresh(client, &c)
                    .await
                    .map_err(ProviderError::from)?;
                new.store(root)
                    .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
                Ok(new)
            },
        )
        .await
    }

    /// 拉取模型目录（含推理能力），供思考档位映射使用。
    async fn catalog(&self, cred: &Credential) -> Result<models::Catalog, ProviderError> {
        models::fetch_catalog(&self.client, &cred.access_token, &cred.domain, &cred.uid)
            .await
            .map_err(ProviderError::from)
    }

    /// 对话请求头（身份 + 归属/风控）。
    fn chat_headers(&self, cred: &Credential) -> Vec<(&'static str, String)> {
        api::chat_headers(
            &cred.access_token,
            &cred.domain,
            &cred.uid,
            &cred.enterprise_id,
        )
    }
}

#[async_trait]
impl Provider for WorkBuddyProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 模型目录（`GET /v2/enterprises/personal/models`）。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let cred = self.live_credential().await?;
        let cat = self.catalog(&cred).await?;
        let models = models::to_models(&cat);
        tracing::debug!("[workbuddy] 模型目录共 {} 个可对话模型", models.len());
        Ok(models)
    }

    /// 访问令牌到期时间。
    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .map(|c| c.expires_at)
            .filter(|v| *v > 0)
    }

    /// 刷新登录令牌。
    async fn refresh(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        let new = refresh::refresh(&self.client, &cred)
            .await
            .map_err(ProviderError::from)?;
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        Ok(format!(
            "续期成功，访问令牌有效期至 {}",
            crate::core::fmt_secs(new.expires_at / 1000)
        ))
    }

    /// 对话（上游恒回 SSE）。
    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError> {
        let cred = self.live_credential().await?;

        let body = chat::build_body(&req);
        let url = format!("{}{}", api::HOST, api::CHAT_PATH);

        let payload = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Other(format!("序列化请求体失败: {e}")))?;
        let mut http = self.client.post(&url).body(payload);
        for (k, v) in self.chat_headers(&cred) {
            http = http.header(k, v);
        }

        let resp = http.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &text));
        }

        // 认证失效时上游回的是 JSON 业务信封（不是 SSE），必须在这里拦下。
        if let Some(ct) = resp.headers().get(reqwest::header::CONTENT_TYPE)
            && !ct.to_str().unwrap_or("").contains("event-stream")
        {
            let text = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &text));
        }

        let mut parser = stream::FrameParser::new();
        Ok(crate::core::stream::drive(resp.bytes_stream(), move |ev| {
            parser.parse(ev)
        }))
    }

    /// 每日签到。
    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        let cred = self.live_credential().await?;
        let headers = api::auth_headers(&cred.access_token, &cred.domain);

        // 先看活动状态：没开启就不必再打签到接口。
        let status = claim::activity_status(&self.client, &headers)
            .await
            .map_err(ProviderError::from)?;
        if !status.active {
            return Ok(ClaimReport {
                outcome: claim::to_report(claim::CheckinStatus::EventEnded),
                credits_delta: None,
                balance: None,
            });
        }
        if status.today_checked_in {
            return Ok(ClaimReport {
                outcome: claim::to_report(claim::CheckinStatus::AlreadyClaimed),
                credits_delta: None,
                balance: None,
            });
        }

        match claim::daily_checkin(&self.client, &headers).await {
            Ok(r) => {
                let balance = self.balance().await.ok().flatten();
                Ok(ClaimReport {
                    outcome: crate::core::claim::ClaimOutcome::success(vec![
                        crate::core::claim::ClaimDetail::Credits(r.credit),
                        crate::core::claim::ClaimDetail::Streak(r.streak_days),
                    ]),
                    credits_delta: Some(r.credit),
                    balance,
                })
            }
            Err(e) => {
                // 业务码 1001/1002/1003 是「本次没领到」，不是错误。
                let code = e.business_code().unwrap_or(0);
                let st = claim::CheckinStatus::from_code(code);
                if st != claim::CheckinStatus::UnknownBizError {
                    return Ok(ClaimReport {
                        outcome: claim::to_report(st),
                        credits_delta: None,
                        balance: None,
                    });
                }
                Err(ProviderError::from(e))
            }
        }
    }

    /// 余额。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        let cred = self.live_credential().await?;
        let v = balance::fetch(
            &self.client,
            &cred.access_token,
            &cred.domain,
            &cred.enterprise_id,
        )
        .await
        .map_err(ProviderError::from)?;
        Ok(Some(v))
    }
}

/// HTTP 层错误映射。
///
/// 腾讯系对话错误体是 `{"code":<数字>,"msg":"…","displayMsg":{…}}`
/// （**不是** OpenAI 的 `error` 对象），故两种形态都要认。
fn map_http_error(status: u16, body: &str) -> ProviderError {
    let v = serde_json::from_str::<serde_json::Value>(body).ok();
    let msg = v
        .as_ref()
        .map(|v| {
            let code = v.get("code").and_then(|c| c.as_i64());
            let raw = v.get("msg").and_then(|m| m.as_str()).unwrap_or("");
            let display = v
                .get("displayMsg")
                .and_then(api::display_msg_text)
                .unwrap_or_default();
            let display = display.as_str();
            let text = if !raw.is_empty() { raw } else { display };
            match code {
                Some(c) if !text.is_empty() => format!("{c}: {text}"),
                Some(c) => c.to_string(),
                None => {
                    if let Some(e) = v
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|m| m.as_str())
                    {
                        e.to_string()
                    } else {
                        crate::core::text::truncate_chars(body, 300)
                    }
                }
            }
        })
        .unwrap_or_else(|| crate::core::text::truncate_chars(body, 300));

    match status {
        401 | 403 => ProviderError::Unauthorized(msg),
        429 => ProviderError::RateLimit(msg),
        404 => ProviderError::Protocol(format!("模型不可用: {msg}")),
        _ => ProviderError::Protocol(format!("上游 HTTP {status}: {msg}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 供应商id固定() {
        assert_eq!(ID, "workbuddy");
    }

    /// 未登录时 `live_credential` 应给出「请先登录」的提示。
    #[tokio::test]
    async fn 未登录时报未接入() {
        let dir = std::env::temp_dir().join("ofm-workbuddy-provider-test");
        let _ = std::fs::remove_dir_all(&dir);
        let p = WorkBuddyProvider::new(dir.clone()).unwrap();
        let err = p.live_credential().await.unwrap_err();
        assert!(
            matches!(&err, ProviderError::NotLoggedIn(m) if m.contains("login workbuddy")),
            "{err:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 腾讯系错误体（`code`/`msg`）要能认出中文文案。
    #[test]
    fn 腾讯系错误体映射() {
        let e = map_http_error(
            400,
            r#"{"code":11141,"msg":"模型已下线","displayMsg":{"zh":"该模型已下线"}}"#,
        );
        assert!(
            matches!(&e, ProviderError::Protocol(m) if m.contains("模型已下线")),
            "{e:?}"
        );
    }

    /// OpenAI 风格 `error` 对象也要认。
    #[test]
    fn openai错误体映射() {
        let e = map_http_error(400, r#"{"error":{"message":"参数错误"}}"#);
        assert!(
            matches!(&e, ProviderError::Protocol(m) if m.contains("参数错误")),
            "{e:?}"
        );
    }

    #[test]
    fn http_401_映射为未授权() {
        let e = map_http_error(401, r#"{"code":401,"msg":"登录失效"}"#);
        assert!(
            matches!(&e, ProviderError::Unauthorized(m) if m.contains("登录失效")),
            "{e:?}"
        );
    }

    #[test]
    fn http_429_映射为限流() {
        let e = map_http_error(429, r#"{"code":429,"msg":"请求过于频繁"}"#);
        assert!(matches!(&e, ProviderError::RateLimit(_)), "{e:?}");
    }
}
