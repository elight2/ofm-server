//! Trae 供应商。
//!
//! 已实现：登录（`auth`）、续期（`refresh`）、对话（`chat`/`stream`）。
//! `models` / `claim` 仍为占位。
//! 协议实况见 `provider-info/trae.md`。

pub mod api;
pub mod auth;
pub mod chat;
pub mod credential;
pub mod device;
pub mod pkce;
pub mod proof;
pub mod refresh;
pub mod stream;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use futures_util::StreamExt;
use tokio::sync::Mutex;

use crate::core::events::{Event, EventStream, FinishReason, UpstreamError};
use crate::core::provider::{ChatRequest, ClaimReport, ModelInfo, Provider, ProviderError};
use crate::core::sse::SseDecoder;

use api::{HttpError, APP_VERSION, APP_VERSION_CODE, HOST_API5, IDE_VERSION_CODE, UA_TTNET};
use credential::Credential;

pub const ID: &str = "trae";

pub struct TraeProvider {
    root: PathBuf,
    client: reqwest::Client,
    /// 续期必须串行，避免并发刷新导致 refresh_token 轮换冲突。
    refresh_lock: Arc<Mutex<()>>,
    /// JWT 缺失/过期时的 warning 只报一次，避免刷屏。
    warned_no_cred: Arc<std::sync::atomic::AtomicBool>,
}

impl TraeProvider {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: api::client().expect("构建 HTTP 客户端失败"),
            refresh_lock: Arc::new(Mutex::new(())),
            warned_no_cred: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    fn credential(&self) -> Result<Credential, ProviderError> {
        match Credential::load(&self.root) {
            Ok(Some(c)) => Ok(c),
            Ok(None) => Err(ProviderError::NotLoggedIn(format!(
                "{}（请先运行 `ofm-server login {ID}`）",
                self.root.join("providers/trae/credential.toml").display()
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
        let new = refresh::refresh(&self.client, &cred)
            .await
            .map_err(|e| match e {
                HttpError::Status { status: 401, .. } => {
                    ProviderError::Unauthorized("refresh_token 已失效，请重新登录".to_string())
                }
                other => ProviderError::Other(format!("续期失败: {other}")),
            })?;
        new.store(&self.root)
            .map_err(|e| ProviderError::Other(format!("保存续期凭证失败: {e}")))?;
        tracing::info!("[{ID}] 续期成功");
        Ok(new.token)
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
            ("x-flow-traceparent", flow_traceparent()),
            ("content-type", "application/json".to_string()),
            ("accept", "*/*".to_string()),
        ]
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

/// 流式解析状态。
struct StreamState {
    decoder: SseDecoder,
    parser: stream::FrameParser,
    pending: std::collections::VecDeque<Event>,
    finished: bool,
}

#[async_trait]
impl Provider for TraeProvider {
    fn id(&self) -> &'static str {
        ID
    }

    async fn models(&self) -> Vec<ModelInfo> {
        // TODO(协议): POST /api/ide/v1/batch_get_detail_param，
        // 取 function == "solo_agent" 且 usage == "chat_completion" 的 config_name。
        ["glm-5.2", "deepseek-v4.1-flash", "qwen3.8-flash"]
            .into_iter()
            .map(|m| ModelInfo {
                model: m.to_string(),
                display_name: None,
            })
            .collect()
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
        let mut bytes_stream = resp.bytes_stream();
        let state = StreamState {
            decoder: SseDecoder::new(),
            parser: stream::FrameParser::new(),
            pending: std::collections::VecDeque::new(),
            finished: false,
        };

        // `unfold` 保存解析状态：每次取出一个事件，待发队列空且无终止事件时继续读流。
        let out = futures_util::stream::unfold(
            (bytes_stream, state),
            |(mut bytes_stream, mut st)| async move {
                loop {
                    if let Some(ev) = st.pending.pop_front() {
                        let terminal = matches!(ev, Event::Finish(_) | Event::Error(_));
                        if terminal {
                            st.finished = true;
                        }
                        return Some((ev, (bytes_stream, st)));
                    }
                    if st.finished {
                        return None;
                    }

                    match bytes_stream.next().await {
                        Some(Ok(chunk)) => {
                            for ev in st.decoder.push(&chunk) {
                                st.pending.extend(st.parser.parse(&ev.event, &ev.data));
                            }
                        }
                        Some(Err(e)) => {
                            st.finished = true;
                            return Some((
                                Event::Error(UpstreamError {
                                    code: None,
                                    message: format!("读取上游流失败: {e}"),
                                    kind: crate::core::events::ErrorKind::Api,
                                }),
                                (bytes_stream, st),
                            ));
                        }
                        None => {
                            // 流结束：冲掉残余帧。
                            if let Some(ev) = st.decoder.finish() {
                                st.pending.extend(st.parser.parse(&ev.event, &ev.data));
                            }
                            if st.pending.is_empty() {
                                // 上游未发 done 就断流：按 stop 收尾，避免下游挂住。
                                st.finished = true;
                                return Some((
                                    Event::Finish(FinishReason::Stop),
                                    (bytes_stream, st),
                                ));
                            }
                        }
                    }
                }
            },
        );

        Ok(Box::pin(out))
    }

    async fn claim(&self) -> Result<ClaimReport, ProviderError> {
        // TODO(协议): status → claim → 复核 status/usage。
        Ok(ClaimReport {
            skipped: true,
            message: "[trae 占位] 今日已签到，跳过".to_string(),
            credits_delta: None,
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

/// 供 `main` 判断是否已有登录凭证。
pub fn has_credential(root: &Path) -> bool {
    matches!(Credential::load(root), Ok(Some(_)))
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
