//! Loomy 对话请求体构造。
//!
//! 上游 `/chat/completions` 是**标准 OpenAI 语义**（实测见 `provider-info/loomy.md`
//! §3）：`messages`/`tools`/`tool_choice`/`reasoning_effort` 全部逐项生效，
//! 流式响应是纯 `data:` 帧。故请求体只需照搬 `ChatRequest`，不做协议翻译。

use serde_json::Value;

use crate::config::LoomyReasoningEffort;
use crate::core::provider::{ChatRequest, Message, Part, Role};

/// 由 [`ChatRequest`] 构造上游请求体。
///
/// 恒发 `stream: true`：上游 SSE 分帧正常，非流式由下游聚合，没必要两套代码。
pub fn build_body(req: &ChatRequest, effort: LoomyReasoningEffort) -> Value {
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": req.messages.iter().map(convert_message).collect::<Vec<_>>(),
        "stream": true,
        "reasoning_effort": effort_field(effort),
    });

    if !req.tools.is_empty() {
        body["tools"] = Value::Array(
            req.tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description.clone().unwrap_or_default(),
                            "parameters": t.parameters.clone().unwrap_or(Value::Null),
                        }
                    })
                })
                .collect(),
        );

        // 上游四种 `tool_choice` 均实测生效（`none`/`required`/指定函数）；
        // `Auto` 不发送该字段。
        if let Some(v) = req.tool_choice.to_json() {
            body["tool_choice"] = v;
        }
    }

    body
}

/// 档位 → 上游 `reasoning_effort` 取值。
///
/// 上游目录共五档（`none/low/medium/high/xhigh`，`default_reasoning_effort = low`），
/// 本项目配置只暴露三档，直接逐档透传即可。
fn effort_field(effort: LoomyReasoningEffort) -> &'static str {
    match effort {
        LoomyReasoningEffort::Low => "low",
        LoomyReasoningEffort::Medium => "medium",
        LoomyReasoningEffort::High => "high",
    }
}

/// 单条消息 → 上游形态。
///
/// 纯文本且只有一段时给字符串（最兼容）；含图片时给内容数组——
/// **只有 `vision: true` 的模型能收图片**，`capabilities.input_modalities`
/// 不含 `image` 的模型（如 `spark-x2.6`）会回 HTTP 400「请求参数错误」。
fn convert_message(m: &Message) -> Value {
    let only_text = m.parts.iter().all(|p| matches!(p, Part::Text(_)));
    let content = if only_text && m.parts.len() <= 1 {
        Value::String(
            m.parts
                .iter()
                .filter_map(|p| match p {
                    Part::Text(t) => Some(t.as_str()),
                    _ => None,
                })
                .collect::<String>(),
        )
    } else {
        Value::Array(
            m.parts
                .iter()
                .map(|p| match p {
                    Part::Text(t) => serde_json::json!({"type": "text", "text": t}),
                    Part::Image { url, .. } => serde_json::json!({
                        "type": "image_url",
                        "image_url": {"url": url},
                    }),
                })
                .collect(),
        )
    };

    let mut obj = serde_json::json!({ "role": m.role.as_str(), "content": content });

    if !m.tool_calls.is_empty() {
        obj["tool_calls"] = Value::Array(
            m.tool_calls
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "id": c.id,
                        "type": "function",
                        "function": {"name": c.name, "arguments": c.arguments},
                    })
                })
                .collect(),
        );
    }
    // 上游只在 tool 消息上认 `tool_call_id`。
    if m.role == Role::Tool
        && let Some(id) = &m.tool_call_id
    {
        obj["tool_call_id"] = Value::String(id.clone());
    }

    obj
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::provider::{ToolCall, ToolChoice, ToolSpec};

    fn msg(role: Role, parts: Vec<Part>) -> Message {
        Message {
            role,
            parts,
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
        }
    }

    fn req(messages: Vec<Message>) -> ChatRequest {
        ChatRequest {
            model: "spark-x2.6".into(),
            messages,
            tools: vec![],
            tool_choice: ToolChoice::Auto,
        }
    }

    #[test]
    fn 纯文本走字符串() {
        let b = build_body(
            &req(vec![msg(Role::User, vec![Part::Text("你好".into())])]),
            Default::default(),
        );
        assert_eq!(b["messages"][0]["content"], "你好");
        assert_eq!(b["messages"][0]["role"], "user");
    }

    #[test]
    fn 图片走内容数组() {
        let b = build_body(
            &req(vec![msg(
                Role::User,
                vec![
                    Part::Text("看图".into()),
                    Part::Image {
                        url: "data:image/png;base64,AA".into(),
                        detail: None,
                    },
                ],
            )]),
            Default::default(),
        );
        let content = b["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
        assert_eq!(content[1]["image_url"]["url"], "data:image/png;base64,AA");
    }

    #[test]
    fn 恒为流式且带思考强度() {
        let b = build_body(
            &req(vec![msg(Role::User, vec![Part::Text("x".into())])]),
            LoomyReasoningEffort::High,
        );
        assert_eq!(b["stream"], true);
        assert_eq!(b["reasoning_effort"], "high");
    }

    #[test]
    fn 工具与选择约束() {
        let mut r = req(vec![msg(Role::User, vec![Part::Text("天气".into())])]);
        r.tools = vec![ToolSpec {
            name: "get_weather".into(),
            description: Some("查天气".into()),
            parameters: Some(serde_json::json!({"type":"object"})),
        }];
        r.tool_choice = ToolChoice::Required;
        let b = build_body(&r, Default::default());
        assert_eq!(b["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(b["tool_choice"], "required");

        // Auto 不发该字段。
        r.tool_choice = ToolChoice::Auto;
        let b = build_body(&r, Default::default());
        assert!(b.get("tool_choice").is_none());

        // 无工具时也不发 tools。
        r.tools.clear();
        let b = build_body(&r, Default::default());
        assert!(b.get("tools").is_none());
    }

    #[test]
    fn 回传工具调用与结果() {
        let mut assistant = msg(Role::Assistant, vec![]);
        assistant.tool_calls = vec![ToolCall {
            id: "call_1".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"北京"}"#.into(),
        }];
        let mut tool = msg(Role::Tool, vec![Part::Text("晴".into())]);
        tool.tool_call_id = Some("call_1".into());

        let b = build_body(&req(vec![assistant, tool]), Default::default());
        assert_eq!(b["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(
            b["messages"][0]["tool_calls"][0]["function"]["name"],
            "get_weather"
        );
        assert_eq!(b["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(b["messages"][1]["content"], "晴");
    }

    #[test]
    fn 非tool消息不带tool_call_id() {
        let mut m = msg(Role::User, vec![Part::Text("x".into())]);
        m.tool_call_id = Some("call_1".into());
        let b = build_body(&req(vec![m]), Default::default());
        assert!(b["messages"][0].get("tool_call_id").is_none());
    }
}
