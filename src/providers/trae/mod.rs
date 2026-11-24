//! Trae 供应商。
//!
//! 已实现：登录（`auth`）、续期（`refresh`）、模型目录（`catalog`）、
//! 对话（`chat`/`stream`）、签到与余额（`checkin`）。
//! 协议实况见 `provider-info/trae.md`。

pub mod api;
pub mod auth;
pub mod catalog;
pub mod chat;
pub mod checkin;
pub mod client_device;
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

use crate::core::claim::{ClaimDetail, ClaimOutcome, SkipReason};
use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};

use api::{
    APP_VERSION, APP_VERSION_CODE, HOST_API5, HttpError, IDE_VERSION_CODE, UA_TTNET, UA_VSCODE,
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

    /// 取可用 token；JWT 已过期则先续期。
    async fn access_token(&self) -> Result<String, ProviderError> {
        let client = &self.client;
        let root = &self.root;
        let cred = crate::core::credential::renew_if_expired(
            root,
            ID,
            &self.refresh_lock,
            |c: &Credential| token_expired(c),
            |c: Credential| async move { Self::renew(client, root, c).await },
        )
        .await?;
        Ok(cred.token)
    }

    /// 换令牌并落盘。401 翻译为「refresh_token 已失效」。
    async fn renew(
        client: &reqwest::Client,
        root: &std::path::Path,
        cred: Credential,
    ) -> Result<Credential, ProviderError> {
        let new = refresh::refresh(client, &cred).await.map_err(|e| match e {
            HttpError::Status { status: 401, .. } => {
                ProviderError::Unauthorized("refresh_token 已失效，请重新登录".to_string())
            }
            other => ProviderError::Other(format!("续期失败: {other}")),
        })?;
        new.store(root)
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
            // `error_code 2001 ... not found`（见 provider-info/trae.md §4）。
            // 对话接口同一值放 body `app_id`，两处都要与官方客户端一致。
            ("x-app-id", chat::APP_ID.to_string()),
            ("x-flow-traceparent", flow_traceparent()),
            ("content-type", "application/json".to_string()),
            ("accept", "*/*".to_string()),
        ]
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
            // 签到后台按设备号做风控：随机设备号会被回 `9074`，官方客户端的 aha
            // 设备号才放行（§6.1）。故这里用 [`Credential::checkin_device_id`]，
            // 而不是对话用的 `cred.device_id`。
            ("x-device-id", cred.checkin_device_id()),
            ("x-device-type", std::env::consts::OS.to_string()),
            ("x-os-version", device::os_version()),
            ("content-type", "application/json".to_string()),
        ]
    }

    /// `POST /trae/api/v2/ug/checkin_credits/status`
    ///
    /// 客户端这里 `retry` 默认为 0；但 `9074`（参与用户太多）同属瞬时拥挤，
    /// 命令行无人可点“再试一次”，故本实现对该码退避重试几次。
    async fn checkin_status(
        &self,
        token: &str,
        cred: &Credential,
    ) -> Result<checkin::Status, ProviderError> {
        const ATTEMPTS: u32 = 3;
        let body = serde_json::json!({ "req_source": checkin::REQ_SOURCE });
        for n in 1..=ATTEMPTS {
            let s = self
                .checkin_post::<checkin::Status>(checkin::PATH_STATUS, &body, token, cred)
                .await?;
            match s.code_error() {
                None => return Ok(s),
                Some(e) if e.code == checkin::CODE_BUSY && n < ATTEMPTS => {
                    let wait = 1u64 << n;
                    tracing::info!("[trae] status 拥挤（{e}），{wait}s 后重试 {n}/{ATTEMPTS}");
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                }
                // 其它业务码原样返回，由调用方统一映射成错误。
                Some(_) => return Ok(s),
            }
        }
        unreachable!("循环在末次迭代必返回")
    }

    /// `POST /trae/api/v2/ug/checkin_credits/claim`
    ///
    /// 失败时把上游原始响应写进错误消息，便于事后定位（如 `code=9074`）。
    ///
    /// 对 `9074`（当前参与用户太多）**额外重试**：客户端在这里传 `retry: 0`，
    /// 靠用户看到错误弹窗后再点一次；命令行无人可点，所以由本实现代替。
    /// 实测 2026-11-22 出现过 `status` 说可领、`claim` 却回 9074 的瞬时拥挤，
    /// 同期用户手动重试即成功。其它业务码（含凭证类）不重试。
    async fn checkin_claim(
        &self,
        token: &str,
        cred: &Credential,
    ) -> Result<checkin::ClaimResult, ProviderError> {
        const ATTEMPTS: u32 = 4;
        let body = serde_json::json!({ "req_source": checkin::REQ_SOURCE });
        for n in 1..=ATTEMPTS {
            let r = self
                .checkin_post::<checkin::ClaimResult>(checkin::PATH_CLAIM, &body, token, cred)
                .await?;
            match r.code_error() {
                None => return Ok(r),
                Some(e) if e.code == checkin::CODE_BUSY && n < ATTEMPTS => {
                    // 拥挤码：退避后重试（2s/4s/8s）。
                    let wait = 1u64 << n;
                    tracing::info!("[trae] claim 拥挤（{e}），{wait}s 后重试 {n}/{ATTEMPTS}");
                    tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                }
                Some(e) => return Err(map_code_error(&e)),
            }
        }
        unreachable!("循环在末次迭代必返回")
    }

    /// 读总余额，仅用于报告签到带来的增量。
    async fn fetch_balance(&self, token: &str, cred: &Credential) -> Result<f64, ProviderError> {
        let u: checkin::Usage = self
            .checkin_post(
                checkin::PATH_USAGE,
                &serde_json::json!({
                    "require_usage": true,
                    "req_source": checkin::REQ_SOURCE
                }),
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
        if !resp.status().is_success() {
            let (status, body) = crate::core::http::error_body(resp, 300).await;
            return Err(map_http_error(status, &body));
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
    format!(
        "04-{}-{}-01",
        crate::core::text::hex_lower(&buf[..16]),
        crate::core::text::hex_lower(&buf[16..24])
    )
}

#[async_trait]
impl Provider for TraeProvider {
    fn id(&self) -> &'static str {
        ID
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
        if !resp.status().is_success() {
            let (status, body) = crate::core::http::error_body(resp, 300).await;
            return Err(map_http_error(status, &body));
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
        if !resp.status().is_success() {
            let (status, body) = crate::core::http::error_body(resp, 300).await;
            return Err(map_http_error(status, &body));
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
        let new = Self::renew(&self.client, &self.root, cred).await?;
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
        tracing::info!(
            "[trae] 签到设备号 {}（{}）",
            cred.checkin_device_id(),
            if cred
                .checkin_device_id
                .as_deref()
                .is_some_and(|v| !v.is_empty())
            {
                "credential.toml 指定"
            } else if cred.checkin_device_id() != cred.device_id {
                "取自本机官方客户端"
            } else {
                "本工具登录设备，可能被风控回 9074"
            }
        );

        // 判定口径对齐客户端 `o1e`/`E0`：`status` 带非 0 `code` 即失败（客户端也判
        // `unknown`、不给签到），不做特例放行；错误文案保留原始 code/message。
        let status = self.checkin_status(&token, &cred).await?;
        if let Some(e) = status.code_error() {
            return Err(map_code_error(&e));
        }

        if !status.enabled() {
            let balance = self.fetch_balance(&token, &cred).await.ok();
            return Ok(ClaimReport {
                outcome: ClaimOutcome::skipped(SkipReason::NotEnabled, vec![]),
                credits_delta: None,
                balance,
            });
        }
        if status.is_checked_in() {
            // 已签到（含 `did_checked_in`）就不调 claim：上游对重复领取也回 success，
            // 会把「已签到」误报成「本次领到」（实测余额不变、无副作用）。
            let balance = self.fetch_balance(&token, &cred).await.ok();
            return Ok(ClaimReport {
                outcome: ClaimOutcome::skipped(SkipReason::AlreadyClaimed, vec![]),
                credits_delta: None,
                balance,
            });
        }
        debug_assert!(status.claimable(), "走到这里应可领取");

        // 领取前记一次余额，仅用于报告增量（不参与成败判定）。
        let before = self.fetch_balance(&token, &cred).await.ok();
        let claimed = self.checkin_claim(&token, &cred).await;

        // claim 的返回值无法区分「真领到」与「今日已签过」，只能复读 status 复核。
        // 这一步对**失败**同样适用：claim 回 9074 时上游可能已经发过了（或别的路径
        // 刚发过），按 `claim` 的错误码直接报失败会误杀。故先复读 status，只要
        // 它变成已签到就算成功，否则才把 claim 的错误抛出去。
        let after = self.checkin_status(&token, &cred).await?;
        if let Some(e) = after.code_error() {
            // status 自己也失败：优先报 claim 的业务码（更能说明问题），其次报 status 的。
            return Err(match claimed {
                Err(e) => e,
                Ok(_) => map_code_error(&e),
            });
        }
        if !after.is_checked_in() {
            return Err(match claimed {
                Err(e) => e,
                Ok(_) => ProviderError::Protocol(
                    "claim 返回成功，但复核 status 仍显示未签到".to_string(),
                ),
            });
        }

        let after_balance = self.fetch_balance(&token, &cred).await.ok();
        let delta = match (before, after_balance) {
            (Some(b), Some(a)) => Some(a - b),
            _ => None,
        };
        let credits = after.credits + after.extra_credits;
        let mut details = vec![ClaimDetail::Credits(credits as f64)];
        if let Some(d) = delta {
            details.push(ClaimDetail::Delta(d));
        }
        Ok(ClaimReport {
            outcome: ClaimOutcome::success(details),
            credits_delta: delta,
            balance: after_balance,
        })
    }
}

/// HTTP 层错误映射（§7.1）。`body` 已由 [`crate::core::http::error_body`] 截断。
fn map_http_error(status: u16, body: &str) -> ProviderError {
    match status {
        401 | 403 => ProviderError::Unauthorized(body.to_string()),
        _ => ProviderError::Other(format!("上游 HTTP {status}: {body}")),
    }
}

/// 上游业务码 → [`ProviderError`]。
///
/// `1002`（客户端 `E0` 认定的鉴权码）与本实现额外识别的 `1001`（凭证失效）都映射成
/// `Unauthorized`，以触发续期/重登；其余保留原始 `code=… message=…`，便于事后定位。
fn map_code_error(e: &checkin::CodeError) -> ProviderError {
    let text = e.to_string();
    if e.kind == checkin::CodeKind::Auth || e.code == checkin::CODE_AUTH {
        ProviderError::Unauthorized(text)
    } else {
        ProviderError::Protocol(text)
    }
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
            checkin_device_id: None,
        };
        assert!(!token_expired(&cred));
        cred.token_expire_at = Some(0);
        assert!(token_expired(&cred));
        cred.token_expire_at = Some(i64::MAX);
        assert!(!token_expired(&cred));
    }
}
