//! Trae 供应商。
//!
//! 已实现：登录（`auth`）、续期（`refresh`）、对话（`chat`/`stream`）。
//! `models` / `claim` 仍为占位。
//! 协议实况见 `provider-info/trae.md`。

pub mod api;
pub mod auth;
pub mod catalog;
pub mod chat;
pub mod checkin;
pub mod credential;
pub mod device;
pub mod pkce;
pub mod proof;
pub mod refresh;
pub mod stream;

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use api::{
    HttpError, APP_VERSION, APP_VERSION_CODE, HOST_API5, IDE_VERSION_CODE, UA_TTNET, UA_VSCODE,
};
use credential::Credential;

pub const ID: &str = "trae";

pub struct TraeProvider {
    root: PathBuf,
    client: reqwest::Client,
    /// 续期必须串行，避免并发刷新导致 refresh_token 轮换冲突。
    refresh_lock: Arc<Mutex<()>>,
}

impl TraeProvider {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: api::client().expect("构建 HTTP 客户端失败"),
            refresh_lock: Arc::new(Mutex::new(())),
        }
    }

    fn credential(&self) -> Result<Credential, ProviderError> {
        match Credential::load(&self.root) {
            Ok(Some(c)) => Ok(c),
            Ok(None) => Err(ProviderError::NotLoggedIn(format!(
                "{}（请先运行 `ofm-server login {ID}`）",
                crate::config::Config::credential_path(&self.root, ID).display()
            ))),
            Err(e) => Err(ProviderError::Other(e.to_string())),
        }
    }

    /// 取可用 token；JWT 已过期则先续期。
    async fn access_token(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        if !token_expired(&cred) {
            return Ok(cred.token.clone());
        }

        let _guard = self.refresh_lock.lock().await;
        // 加锁后重读：可能已被其它任务刷新。
        let cred = self.credential()?;
        if !token_expired(&cred) {
            return Ok(cred.token);
        }

        tracing::info!("[{ID}] token 已过期，正在续期");
        let new = self.do_refresh(&cred).await?;
        tracing::info!("[{ID}] 续期成功");
        Ok(new.token)
    }

    /// 续期并落盘，返回新凭证。401 翻译为「refresh_token 已失效」。
    async fn do_refresh(&self, cred: &Credential) -> Result<Credential, ProviderError> {
        let new = refresh::refresh(&self.client, cred)
            .await
            .map_err(|e| match e {
                HttpError::Status { status: 401, .. } => {
                    ProviderError::Unauthorized("refresh_token 已失效，请重新登录".to_string())
                }
                other => ProviderError::Other(format!("续期失败: {other}")),
            })?;
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        Ok(new)
    }

    fn chat_headers(&self, token: &str, cred: &Credential) -> Vec<(&'static str, String)> {
        vec![
            ("user-agent", UA_TTNET.to_string()),
            ("authorization", format!("Cloud-IDE-JWT {token}")),
            // 抓包实测 x-ide-version 与 x-app-version 同为 3.4.18（文档写 1.107.1 有误）。
            ("x-app-version", APP_VERSION.to_string()),
            ("x-app-version-code", APP_VERSION_CODE.to_string()),
            ("x-ide-version", APP_VERSION.to_string()),
            ("x-ide-version-code", IDE_VERSION_CODE.to_string()),
            ("x-machine-id", cred.machine_id.clone()),
            ("x-device-id", cred.device_id.clone()),
            ("x-device-cpu", device::cpu_model()),
            ("x-device-type", std::env::consts::OS.to_string()),
            ("x-os-version", device::os_version()),
            // 模型目录接口靠这个头做准入：缺失时 HTTP 200 但每个 function 都回
            // `error_code 2001 ... not found`（实测，见 provider-info/trae.md §4.1）。
            // 对话接口同一值放 body `app_id`，两处都要与官方客户端一致。
            ("x-app-id", chat::APP_ID.to_string()),
            ("x-flow-traceparent", flow_traceparent()),
            ("content-type", "application/json".to_string()),
            ("accept", "*/*".to_string()),
        ]
    }

    /// 请求模型目录并解析（独立方法便于单测与错误传播）。
    async fn fetch_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let url = format!("{HOST_API5}{}", catalog::ENDPOINT_PATH);

        let payload = serde_json::to_vec(&catalog::CatalogBody::build())
            .map_err(|e| ProviderError::Other(format!("序列化目录请求体失败: {e}")))?;
        let mut http = self.client.post(&url).body(payload);
        for (k, v) in self.chat_headers(&token, &cred) {
            http = http.header(k, v);
        }

        let resp = http.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &body));
        }

        let catalog: catalog::Catalog = resp
            .json()
            .await
            .map_err(|e| ProviderError::Other(format!("解析模型目录失败: {e}")))?;
        // 准入失败也是 200，不检查就会静默返回空列表。
        if let Some(msg) = catalog.first_error() {
            return Err(ProviderError::Other(format!("模型目录不可用: {msg}")));
        }
        let models = catalog::to_models(&catalog);
        tracing::debug!("[{ID}] 模型目录共 {} 个可对话模型", models.len());
        Ok(models)
    }

    // ------------------------------------------------------------ 签到（§6）
    //
    // 签到走 `api.trae.cn`，用 VSCode UA 那套头，且**不带** `x-machine-id`
    // （与对话请求的头不同，实测如此）。

    fn checkin_headers(&self, token: &str, cred: &Credential) -> Vec<(&'static str, String)> {
        vec![
            ("user-agent", UA_VSCODE.to_string()),
            ("authorization", format!("Cloud-IDE-JWT {token}")),
            ("x-app-version", APP_VERSION.to_string()),
            ("x-device-id", cred.device_id.clone()),
            ("x-device-type", std::env::consts::OS.to_string()),
            ("x-os-version", device::os_version()),
            ("content-type", "application/json".to_string()),
        ]
    }

    /// `POST /trae/api/v2/ug/checkin_credits/status`
    async fn checkin_status(
        &self,
        token: &str,
        cred: &Credential,
    ) -> Result<checkin::Status, ProviderError> {
        self.checkin_post(
            checkin::PATH_STATUS,
            &serde_json::json!({ "req_source": checkin::REQ_SOURCE }),
            token,
            cred,
        )
        .await
    }

    /// `POST /trae/api/v2/ug/checkin_credits/claim`
    async fn checkin_claim(
        &self,
        token: &str,
        cred: &Credential,
    ) -> Result<checkin::ClaimResult, ProviderError> {
        let r: checkin::ClaimResult = self
            .checkin_post(
                checkin::PATH_CLAIM,
                &serde_json::json!({ "req_source": checkin::REQ_SOURCE }),
                token,
                cred,
            )
            .await?;
        r.check().map_err(ProviderError::Protocol)?;
        Ok(r)
    }

    /// 读总余额，仅用于报告签到带来的增量。
    async fn fetch_balance(&self, token: &str, cred: &Credential) -> Result<f64, ProviderError> {
        let u: checkin::Usage = self
            .checkin_post(
                checkin::PATH_USAGE,
                &serde_json::json!({ "require_usage": true, "req_source": checkin::REQ_SOURCE }),
                token,
                cred,
            )
            .await?;
        Ok(u.total_balance)
    }

    async fn checkin_post<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
        token: &str,
        cred: &Credential,
    ) -> Result<T, ProviderError> {
        let url = format!("{}{path}", checkin::HOST);
        let payload = serde_json::to_vec(body)
            .map_err(|e| ProviderError::Other(format!("序列化签到请求体失败: {e}")))?;
        let mut http = self.client.post(&url).body(payload);
        for (k, v) in self.checkin_headers(token, cred) {
            http = http.header(k, v);
        }

        let resp = http.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &body));
        }
        resp.json()
            .await
            .map_err(|e| ProviderError::Protocol(format!("解析签到响应失败: {e}")))
    }
}

/// 毫秒时间戳是否已过期（留 60 秒余量）。
fn token_expired(cred: &Credential) -> bool {
    let Some(exp) = cred.token_expire_at else {
        return false;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    now + 60_000 >= exp
}

/// `04-<trace32hex>-<span16hex>-01`（W3C traceparent 的简化形态，照抄官方）。
fn flow_traceparent() -> String {
    let mut buf = [0u8; 24];
    let _ = getrandom::fill(&mut buf);
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    format!("04-{}-{}-01", hex(&buf[..16]), hex(&buf[16..24]))
}

#[async_trait]
impl Provider for TraeProvider {
    fn id(&self) -> &'static str {
        ID
    }

    /// 拉取上游模型目录（`provider-info/trae.md` §4）。
    ///
    /// 失败时记日志并返回空列表：`/v1/models` 是聚合端点，
    /// 单个供应商不可用不应让整体失败。
    async fn models(&self) -> Vec<ModelInfo> {
        match self.fetch_models().await {
            Ok(models) => models,
            Err(e) => {
                tracing::warn!("[{ID}] 获取模型目录失败: {e}");
                Vec::new()
            }
        }
    }

    /// 带错误的目录拉取：`status` 用它真实探测接入是否可用。
    async fn models_checked(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        self.fetch_models().await
    }

    /// JWT 到期时间（毫秒）。
    fn credential_expire_at(&self) -> Option<i64> {
        Credential::load(&self.root)
            .ok()
            .flatten()
            .and_then(|c| c.token_expire_at)
    }

    async fn chat(&self, req: ChatRequest) -> Result<EventStream, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        let body = chat::ChatBody::build(&req);
        let url = format!("{HOST_API5}{}", chat::ENDPOINT_PATH);

        let headers = self.chat_headers(&token, &cred);
        // 自己序列化并 .body()：避免 .json() 追加第二个 content-type 头。
        let payload = serde_json::to_vec(&body)
            .map_err(|e| ProviderError::Other(format!("序列化请求体失败: {e}")))?;
        let mut http = self.client.post(&url).body(payload);
        for (k, v) in headers {
            http = http.header(k, v);
        }

        let resp = http.send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(map_http_error(status.as_u16(), &body));
        }

        // 上游恒回 SSE；分帧后逐帧转成中间事件。
        let mut parser = stream::FrameParser::new();
        Ok(crate::core::stream::drive(resp.bytes_stream(), move |ev| {
            parser.parse(&ev.event, &ev.data)
        }))
    }

    /// 主动刷新令牌（`ofm-server refresh trae`）。
    async fn refresh(&self) -> Result<String, ProviderError> {
        let cred = self.credential()?;
        let new = self.do_refresh(&cred).await?;
        Ok(match new.token_expire_at {
            Some(ms) => format!("续期成功，新凭证到期时间 {}", crate::core::fmt_ms(ms)),
            None => "续期成功".to_string(),
        })
    }

    /// 读当前余额（`pay/ide_user_ent_usage` 的 `total_balance`）。
    async fn balance(&self) -> Result<Option<f64>, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;
        Ok(Some(self.fetch_balance(&token, &cred).await?))
    }

    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        let cred = self.credential()?;
        let token = self.access_token().await?;

        let status = self.checkin_status(&token, &cred).await?;
        status.check().map_err(ProviderError::Protocol)?;

        if !status.enable {
            let balance = self.fetch_balance(&token, &cred).await.ok();
            return Ok(ClaimReport {
                skipped: true,
                message: "签到活动未开启".to_string(),
                credits_delta: None,
                balance,
            });
        }
        if status.checked_in {
            // 已签到就不调 claim：上游对重复领取也回 success，会把「已签到」
            // 误报成「本次领到」（实测余额不变、无副作用）。
            let balance = self.fetch_balance(&token, &cred).await.ok();
            return Ok(ClaimReport {
                skipped: true,
                message: "今日已签到".to_string(),
                credits_delta: None,
                balance,
            });
        }
        debug_assert!(status.claimable(), "走到这里应可领取");

        // 领取前记一次余额，仅用于报告增量（不参与成败判定）。
        let before = self.fetch_balance(&token, &cred).await.ok();
        self.checkin_claim(&token, &cred).await?;

        // claim 的返回值无法区分「真领到」与「今日已签过」，只能复读 status 复核。
        let after = self.checkin_status(&token, &cred).await?;
        after.check().map_err(ProviderError::Protocol)?;
        if !after.checked_in {
            return Err(ProviderError::Protocol(
                "claim 返回成功，但复核 status 仍显示未签到".to_string(),
            ));
        }

        let after_balance = self.fetch_balance(&token, &cred).await.ok();
        let delta = match (before, after_balance) {
            (Some(b), Some(a)) => Some(a - b),
            _ => None,
        };
        let credits = after.credits + after.extra_credits;
        Ok(ClaimReport {
            skipped: false,
            message: match delta {
                Some(d) => format!("签到成功，额度 {credits}，余额变化 {d:+.2}"),
                None => format!("签到成功，额度 {credits}"),
            },
            credits_delta: delta,
            balance: after_balance,
        })
    }
}

/// HTTP 层错误映射（§5.1）。
fn map_http_error(status: u16, body: &str) -> ProviderError {
    let snippet = truncate(body, 300);
    match status {
        401 | 403 => ProviderError::Unauthorized(snippet),
        _ => ProviderError::Other(format!("上游 HTTP {status}: {snippet}")),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traceparent_shape() {
        let t = flow_traceparent();
        let parts: Vec<&str> = t.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0], "04");
        assert_eq!(parts[1].len(), 32);
        assert_eq!(parts[2].len(), 16);
        assert_eq!(parts[3], "01");
        assert!(parts[1].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn expired_detection() {
        let mut cred = Credential {
            token: "t".into(),
            refresh_token: "r".into(),
            token_expire_at: None,
            refresh_expire_at: None,
            machine_id: "m".into(),
            device_id: "d".into(),
            device_private_key_pem: String::new(),
            bound_device_id: None,
            device_bind_status: None,
        };
        assert!(!token_expired(&cred));
        cred.token_expire_at = Some(0);
        assert!(token_expired(&cred));
        cred.token_expire_at = Some(i64::MAX);
        assert!(!token_expired(&cred));
    }
}
