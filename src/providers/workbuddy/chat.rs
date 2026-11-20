//! 对话请求体构造 + 思考档位映射。
//!
//! 上游是**标准 OpenAI Chat Completions**（协议见 `provider-info/workbuddy.md`
//! §3）：`messages`/`tools`/`tool_choice` 逐项生效，SSE 是纯 `data:` 帧。
//! 故请求体基本照搬 [`ChatRequest`]，只额外处理两处：
//!
//! 1. **`max_tokens` 字段名**：对 `o1/o3/o4/gpt-4.1/gpt-5` 前缀模型，
//!    上游要的是 `max_completion_tokens`（harness `MaxTokensFieldRule`）。
//!
//! **不发送**思考强度字段：保持上游默认状态。

use serde_json::Value;

use crate::core::provider::{ChatRequest, Message, Part, Role};

/// 由 [`ChatRequest`] 构造上游请求体。
///
/// 恒发 `stream: true`：上游 SSE 分帧正常，非流式由下游聚合，没必要两套代码。
pub fn build_body(req: &ChatRequest) -> Value {
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": req.messages.iter().map(convert_message).collect::<Vec<_>>(),
        "stream": true,
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

        if let Some(v) = req.tool_choice.to_json() {
            body["tool_choice"] = v;
        }
    }

    body
}

/// 单条消息 → 上游形态。
///
/// 纯文本且只有一段时给字符串（最兼容）；含图片时给内容数组。
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
            model: "glm-5.3-flash".into(),
            messages,
            tools: vec![],
            tool_choice: crate::core::provider::ToolChoice::Auto,
        }
    }

    #[test]
    fn 纯文本走字符串() {
        let b = build_body(&req(vec![msg(Role::User, vec![Part::Text("你好".into())])]));
        assert_eq!(b["messages"][0]["content"], "你好");
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["stream"], true);
    }

    #[test]
    fn 图片走内容数组() {
        let b = build_body(&req(vec![msg(
            Role::User,
            vec![
                Part::Text("看图".into()),
                Part::Image {
                    url: "data:image/png;base64,AA".into(),
                    detail: None,
                },
            ],
        )]));
        let content = b["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image_url");
    }

    #[test]
    fn 工具与选择约束() {
        let mut r = req(vec![msg(Role::User, vec![Part::Text("天气".into())])]);
        r.tools = vec![crate::core::provider::ToolSpec {
            name: "get_weather".into(),
            description: Some("查天气".into()),
            parameters: Some(serde_json::json!({"type":"object"})),
        }];
        r.tool_choice = crate::core::provider::ToolChoice::Required;
        let b = build_body(&r);
        assert_eq!(b["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(b["tool_choice"], "required");

        r.tool_choice = crate::core::provider::ToolChoice::Auto;
        let b = build_body(&r);
        assert!(b.get("tool_choice").is_none());
    }

    #[test]
    fn 回传工具调用与结果() {
        let mut assistant = msg(Role::Assistant, vec![]);
        assistant.tool_calls = vec![crate::core::provider::ToolCall {
            id: "call_1".into(),
            name: "get_weather".into(),
            arguments: r#"{"city":"北京"}"#.into(),
        }];
        let mut tool = msg(Role::Tool, vec![Part::Text("晴".into())]);
        tool.tool_call_id = Some("call_1".into());

        let b = build_body(&req(vec![assistant, tool]));
        assert_eq!(b["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(b["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(b["messages"][1]["content"], "晴");
    }

    /// **不发送**任何思考强度字段：保持上游默认状态。
    #[test]
    fn 不发送思考强度字段() {
        let b = build_body(&req(vec![msg(Role::User, vec![Part::Text("x".into())])]));
        assert!(b.get("reasoning_effort").is_none());
        assert!(b.get("thinking").is_none());
        assert!(b.get("output_config").is_none());
    }
}
