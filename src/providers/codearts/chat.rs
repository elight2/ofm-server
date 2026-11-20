//! CodeArts Chat Completions 请求。
//!
//! 上游就是 OpenAI CC 兼容端点：`POST {SNAP_GATEWAY}/api/v2/chat/completions`。
//! 按 `provider-info/codearts.md` §3 与已实测的 ofm 插件实现，本模块只负责：
//! * 内部 [`ChatRequest`] → 上游 CC JSON；
//! * AKSK 签名（body hash + `X-Security-token`）；
//! * 发送并把字节流交给通用 SSE 驱动器。
//!
//! 加密链路（SM4-GCM）不是本实现的必经路径：实测明文请求可正常出流；
//! 若后续需要强制加密，可在本模块的发送层加拦截，见文档 §3.2。

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::core::events::EventStream;
use crate::core::provider::{ChatRequest, Message, Part, ProviderError, Role};
use crate::core::text::truncate_chars;

use super::api::SNAP_GATEWAY;
use super::credential::Credential;
use super::pkce::uuid_v4;
use super::sign;

pub const CHAT_PATH: &str = "/api/v2/chat/completions";
const CHAT_UA: &str = "codearts/1.0.0 (ofm-server)";
const CLIENT_VERSION: &str = "1.0.0";
const DEFAULT_MAX_TOKENS: u64 = 65536;

/// 请求体。字段对齐 CodeArts 网关（明文分支）：
/// * `tool_stream` 让工具参数分片流式返回，避免大参数被网关掐断；
/// * `max_tokens` 给足输出空间（参考实现默认 65536）。
#[derive(Debug, Serialize)]
struct ChatBody {
    model: String,
    messages: Vec<serde_json::Value>,
    stream: bool,
    max_tokens: u64,
    tool_stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<serde_json::Value>,
    /// 前缀缓存 / 会话标识；缺失时缓存命中恒为 0。
    ///
    /// 取值由**对话前缀**派生（见 [`cache_key`】）：同一会话的前缀不变时 key 不变，
    /// 上游才可能命中前缀缓存。
    prompt_cache_key: String,
}

impl ChatBody {
    fn build(req: &ChatRequest) -> Self {
        Self {
            model: req.model.clone(),
            messages: req.messages.iter().map(convert_message).collect(),
            stream: true,
            max_tokens: DEFAULT_MAX_TOKENS,
            tool_stream: true,
            tools: req
                .tools
                .iter()
                .map(|t| {
                    let mut f = serde_json::Map::new();
                    f.insert("name".into(), serde_json::Value::String(t.name.clone()));
                    if let Some(d) = &t.description {
                        f.insert("description".into(), serde_json::Value::String(d.clone()));
                    }
                    if let Some(p) = &t.parameters {
                        f.insert("parameters".into(), p.clone());
                    }
                    serde_json::json!({
                        "type": "function",
                        "function": serde_json::Value::Object(f),
                    })
                })
                .collect(),
            tool_choice: req.tool_choice.to_json(),
            prompt_cache_key: cache_key(req),
        }
    }
}

/// 前缀缓存 key：由**对话前缀**派生。
///
/// 之前每请求随机生成 `ofm-<uuid>`，前缀缓存永远不可能命中（服务端按 key 归组）。
/// 改为对「首条 system + 首条 user」的内容取哈希：
///
/// * 同一会话内前缀不变 → key 不变 → 上游可命中前缀缓存；
/// * 不同会话首条消息不同 → key 不同 → 不会串味；
/// * 只取**首条消息**（通常是 system 提示词或首个用户消息）做哈希，
///   后续追加的消息不进哈希，故会话变长不会改变 key
///   （前缀缓存本来就是按前缀命中的）。
fn cache_key(req: &ChatRequest) -> String {
    let mut h = Sha256::new();
    h.update(req.model.as_bytes());
    for m in req.messages.iter().take(1) {
        h.update([0u8]);
        h.update(m.role.as_str().as_bytes());
        for p in &m.parts {
            match p {
                Part::Text(t) => h.update(t.as_bytes()),
                Part::Image { url, .. } => h.update(url.as_bytes()),
            }
            h.update([0u8]);
        }
    }
    let digest = h.finalize();
    format!("ofm-{}", crate::core::text::hex_lower(&digest))
}

fn convert_message(m: &Message) -> serde_json::Value {
    let only_text = m.parts.iter().all(|p| matches!(p, Part::Text(_)));
    let content = if only_text {
        serde_json::Value::String(
            m.parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect::<String>(),
        )
    } else {
        let parts: Vec<serde_json::Value> = m
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => serde_json::json!({"type": "text", "text": t}),
                Part::Image { url, .. } => {
                    serde_json::json!({"type": "image_url", "image_url": {"url": url}})
                }
            })
            .collect();
        serde_json::Value::Array(parts)
    };

    let mut obj = serde_json::Map::new();
    obj.insert(
        "role".into(),
        serde_json::Value::String(m.role.as_str().into()),
    );
    obj.insert("content".into(), content);

    // deepseek-v4 后端要求历史 assistant 消息必须带 reasoning_content 字段；
    // 内部没有持久化思考内容，给空串即可。
    if m.role == Role::Assistant {
        obj.insert(
            "reasoning_content".into(),
            serde_json::Value::String(String::new()),
        );
    }

    if !m.tool_calls.is_empty() {
        obj.insert(
            "tool_calls".into(),
            serde_json::Value::Array(
                m.tool_calls
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id,
                            "type": "function",
                            "function": {"name": c.name, "arguments": c.arguments}
                        })
                    })
                    .collect(),
            ),
        );
    }
    if m.role == Role::Tool {
        if let Some(id) = &m.tool_call_id {
            obj.insert("tool_call_id".into(), serde_json::Value::String(id.clone()));
        }
        if let Some(name) = &m.tool_name {
            obj.insert("name".into(), serde_json::Value::String(name.clone()));
        }
    }
    serde_json::Value::Object(obj)
}

/// **不要给 chat 请求加 `maas_type: benefit`。**
///
/// 实测（2026-11-16）：本账号带 `maas_type:benefit` 的 chat 请求对**任何模型**都返回
/// `404 InferHub.APIG.0101.404`（含本来正常的 `GLM-5.2`），不带则 200。
/// 该头是网关的路由开关，指向本账号不存在的 benefit 后端。
///
/// 官方客户端的做法是：仅当模型出现在 `gateway/config` 下发的 `models` 表（`hw()`）时
/// 才加该头，**并且改用 `gateway/config` 下发的独立 `endpoint` 作为 chat 基址**。
/// 本账号 `gateway/config` 的 `result` 为 `null`（无福利模型），故永远不该加。
///
/// 早期实现按模型名前缀猜测（`glm-5.3-flash`/`deepseek-v4.1-flash`）属误判，已删除。
/// 见 `provider-info/codearts.md` §5.2。
fn sha256_hex(bytes: &[u8]) -> String {
    crate::core::text::hex_lower(&Sha256::digest(bytes))
}

fn header_map(
    pairs: impl IntoIterator<Item = (String, String)>,
) -> Result<HeaderMap, ProviderError> {
    let mut map = HeaderMap::new();
    for (k, v) in pairs {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| ProviderError::Other(format!("非法请求头 {k}: {e}")))?;
        let value = HeaderValue::from_str(&v)
            .map_err(|e| ProviderError::Other(format!("非法请求头值 {k}: {e}")))?;
        map.insert(name, value);
    }
    Ok(map)
}

/// 发起 chat 请求，返回内部事件流。
///
/// 不发送任何思考强度字段：保持上游默认状态。
pub async fn stream(
    client: &reqwest::Client,
    cred: &Credential,
    req: &ChatRequest,
) -> Result<EventStream, ProviderError> {
    let url = format!("{SNAP_GATEWAY}{CHAT_PATH}");
    for attempt in 0..=QUEUE_MAX_ATTEMPTS {
        if attempt > 0 {
            tracing::warn!(
                "[codearts] 并发会话已满（TM.00001041），{}s 后重试（{attempt}/{QUEUE_MAX_ATTEMPTS}）",
                QUEUE_RETRY_SECS
            );
            tokio::time::sleep(std::time::Duration::from_secs(QUEUE_RETRY_SECS)).await;
        }

        let body = serde_json::to_string(&ChatBody::build(req))
            .map_err(|e| ProviderError::Other(format!("序列化 chat 请求失败: {e}")))?;
        tracing::debug!("[codearts] model={}", req.model);
        let body_hash = sha256_hex(body.as_bytes());

        // 与已验证的 ofm 插件一致：content-type、body hash、security token 参与签名；
        // trace/model 头在签名后追加（不参与验签）。
        // 不发送 `maas_type`（见 `sha256_hex` 上方注释）。
        let signed_headers: Vec<(&'static str, String)> = vec![
            ("content-type", "application/json".to_string()),
            ("x-sdk-content-sha256", body_hash),
            ("x-security-token", cred.security_token.clone()),
        ];
        let signed = sign::sign(
            &sign::Request {
                method: "POST",
                url: &url,
                headers: signed_headers,
                query: None,
                body: Some(&body),
            },
            &cred.access_key_id,
            &cred.secret_access_key,
        )
        .map_err(|e| ProviderError::Other(format!("AKSK 签名失败: {e}")))?;

        let session_id = uuid_v4().replace('-', "");
        let created = timestamp_ms();
        let msg_id = format!("{session_id}-{created}");
        let mut extra: Vec<(String, String)> = vec![
            ("user-agent".into(), CHAT_UA.into()),
            ("accept".into(), "text/event-stream".into()),
            ("app-id".into(), "CodeAgent3.0".into()),
            ("is-need-queue".into(), "true".into()),
            ("x-language".into(), "zh-cn".into()),
            ("x-ot-client-type".into(), "IDE".into()),
            ("x-ot-client-version".into(), CLIENT_VERSION.into()),
            ("x-ot-function".into(), "agent-tui".into()),
            ("x-ot-trace-id".into(), session_id.clone()),
            ("x-ot-span-id".into(), session_id.clone()),
            ("x-snap-traceid".into(), session_id.clone()),
            ("x-ot-session-id".into(), session_id.clone()),
            ("user-session-id".into(), session_id.clone()),
            ("user-msg-id".into(), msg_id),
            ("created-time".into(), created.to_string()),
            ("model-id".into(), req.model.clone()),
            ("model-name".into(), req.model.clone()),
        ];
        // 签名结果优先；附加头不参与验签，可覆盖同名默认值。
        extra.retain(|(k, _)| !signed.iter().any(|(sk, _)| sk.eq_ignore_ascii_case(k)));
        let mut pairs: Vec<(String, String)> = signed;
        pairs.extend(extra);
        let headers = header_map(pairs)?;

        let resp = client
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(ProviderError::Transport)?;
        let status = resp.status();
        if status.is_success() {
            let mut parser = super::stream::FrameParser::new();
            return Ok(crate::core::stream::drive(resp.bytes_stream(), move |ev| {
                parser.parse(ev)
            }));
        }

        let text = resp.text().await.unwrap_or_default();
        if is_queue_error(status.as_u16(), &text) && attempt < QUEUE_MAX_ATTEMPTS {
            continue;
        }
        let snippet = truncate_chars(&text, 500);
        return Err(match status.as_u16() {
            401 | 403 => {
                ProviderError::Unauthorized(format!("codearts chat HTTP {status}: {snippet}"))
            }
            400 if is_queue_error(400, &text) => {
                ProviderError::Protocol(format!("codearts 排队超时: {snippet}"))
            }
            _ => ProviderError::Protocol(format!("codearts chat HTTP {status}: {snippet}")),
        });
    }
    unreachable!("循环必然在成功或重试耗尽时返回")
}

/// 并发会话满：HTTP 400 + `TM.00001041`（有时伴随 peak usage 文案）。
fn is_queue_error(status: u16, body: &str) -> bool {
    status == 400
        && (body.contains("TM.00001041")
            || body.to_ascii_lowercase().contains("peak usage")
            || body.to_ascii_lowercase().contains("high demand"))
}

const QUEUE_RETRY_SECS: u64 = 10;
const QUEUE_MAX_ATTEMPTS: usize = 180;

fn timestamp_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::provider::{Message, Part, Role, ToolCall, ToolChoice, ToolSpec};

    fn user(text: &str) -> Message {
        Message {
            role: Role::User,
            parts: vec![Part::Text(text.into())],
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
        }
    }

    #[test]
    fn body_is_standard_cc_and_streams() {
        let req = ChatRequest {
            model: "GLM-5.2".into(),
            messages: vec![user("你好")],
            tools: vec![ToolSpec {
                name: "get_time".into(),
                description: Some("取时间".into()),
                parameters: Some(serde_json::json!({"type": "object"})),
            }],
            tool_choice: ToolChoice::Auto,
        };
        let v = serde_json::to_value(ChatBody::build(&req)).unwrap();
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["content"], "你好");
        assert_eq!(v["tools"][0]["function"]["parameters"]["type"], "object");
        assert!(v.get("tool_choice").is_none());
    }

    /// 前缀缓存 key：同一会话前缀不变 → key 不变；不同前缀 → key 不同。
    #[test]
    fn cache_key_随前缀稳定() {
        let a = ChatRequest {
            model: "GLM-5.2".into(),
            messages: vec![user("你好")],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
        };
        // 同一前缀，后续追加消息不应改变 key。
        let mut b = a.clone();
        b.messages.push(user("再问一句"));
        assert_eq!(cache_key(&a), cache_key(&b));

        // 不同会话（首条 user 不同）→ key 不同。
        let mut c = a.clone();
        c.messages = vec![user("别的会话")];
        assert_ne!(cache_key(&a), cache_key(&c));

        // 不同模型 → key 不同。
        let mut d = a.clone();
        d.model = "kimi-k3".into();
        assert_ne!(cache_key(&a), cache_key(&d));
    }

    /// **不发送**任何思考强度字段：保持上游默认状态。
    #[test]
    fn 不发送思考强度字段() {
        let req = ChatRequest {
            model: "GLM-5.2".into(),
            messages: vec![user("hi")],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
        };
        let v = serde_json::to_value(ChatBody::build(&req)).unwrap();
        assert!(v.get("reasoning_effort").is_none());
        assert!(v.get("thinking").is_none());
        assert!(v.get("output_config").is_none());
    }

    #[test]
    fn assistant_always_has_reasoning_content_and_tool_calls() {
        let req = ChatRequest {
            model: "deepseek-v4.1-flash".into(),
            messages: vec![Message {
                role: Role::Assistant,
                parts: vec![],
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "get_time".into(),
                    arguments: "{}".into(),
                }],
                tool_call_id: None,
                tool_name: None,
            }],
            tools: vec![],
            tool_choice: ToolChoice::Auto,
        };
        let v = serde_json::to_value(ChatBody::build(&req)).unwrap();
        assert_eq!(v["messages"][0]["reasoning_content"], "");
        assert_eq!(
            v["messages"][0]["tool_calls"][0]["function"]["name"],
            "get_time"
        );
    }

    #[test]
    fn queue_error_detection() {
        assert!(is_queue_error(400, r#"{"error_code":"TM.00001041"}"#));
        assert!(!is_queue_error(
            400,
            r#"{"error_code":"InferHub.4291.200"}"#
        ));
        assert!(!is_queue_error(401, "TM.00001041"));
    }

    #[test]
    fn 不发送maas_type() {
        // 网关对带 maas_type:benefit 的请求恒返回 APIG.0101.404（实测），故绝不能发。
        for model in ["glm-5.3-flash", "deepseek-v4.1-flash", "GLM-5.2"] {
            let req = ChatRequest {
                model: model.into(),
                messages: vec![user("hi")],
                tools: vec![],
                tool_choice: ToolChoice::Auto,
            };
            let body = serde_json::to_string(&ChatBody::build(&req)).unwrap();
            assert!(!body.contains("maas_type"), "{model} 不应带 maas_type");
        }
    }
}
