//! Loomy 对话。
//!
//! 请求体构造与响应解析。

use serde_json::Value;

use crate::config::LoomyReasoningEffort;
use crate::core::provider::{ChatRequest, Part, Role};

/// 由 [`ChatRequest`] 构造上游请求体（OpenAI 兼容）。
pub fn build_body(req: &ChatRequest, effort: LoomyReasoningEffort) -> Value {
    let messages: Vec<Value> = req
        .messages
        .iter()
        .map(|m| {
            let role = match m.role {
                Role::System => "system",
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::Tool => "tool",
            };
            let content = if m.role == Role::Tool {
                // tool 消息的 content 是工具结果字符串
                m.tool_calls
                    .first()
                    .map(|tc| format!("{{\"name\":\"{}\",\"result\":{}}}", tc.name, tc.arguments))
                    .unwrap_or_default()
            } else {
                // 普通消息：拼接文本部分
                m.parts
                    .iter()
                    .filter_map(|p| match p {
                        Part::Text(t) => Some(t.clone()),
                        Part::Image { url, .. } => Some(format!("[图片: {url}]", url = url)),
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            };

            let mut obj = serde_json::json!({
                "role": role,
                "content": content,
            });

            if !m.tool_calls.is_empty() {
                let tool_calls: Vec<Value> = m
                    .tool_calls
                    .iter()
                    .map(|tc| {
                        serde_json::json!({
                            "id": tc.id,
                            "type": "function",
                            "function": {
                                "name": tc.name,
                                "arguments": tc.arguments,
                            }
                        })
                    })
                    .collect();
                obj["tool_calls"] = Value::Array(tool_calls);
            }
            if let Some(tool_call_id) = &m.tool_call_id {
                obj["tool_call_id"] = Value::String(tool_call_id.clone());
            }

            obj
        })
        .collect();

    let mut body = serde_json::json!({
        "model": req.model,
        "messages": messages,
        "temperature": 0.7,
        "max_tokens": 4096,
        "stream": false,
    });

    // 思考强度
    match effort {
        LoomyReasoningEffort::Low => {
            body["reasoning_effort"] = Value::String("low".to_string());
        }
        LoomyReasoningEffort::Medium => {
            body["reasoning_effort"] = Value::String("medium".to_string());
        }
        LoomyReasoningEffort::High => {
            body["reasoning_effort"] = Value::String("high".to_string());
        }
    }

    // 工具
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
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
            .collect();
        body["tools"] = Value::Array(tools);

        // tool_choice
        if let Some(v) = req.tool_choice.to_json() {
            body["tool_choice"] = v;
        }
    }

    body
}

/// 响应解析器。
pub struct ResponseParser {
    /// 累积的 content。
    pub content: String,
}

impl ResponseParser {
    pub fn new() -> Self {
        Self {
            content: String::new(),
        }
    }

    /// 解析响应。
    pub fn parse(&mut self, _event: &str, data: &str) -> Vec<crate::core::events::Event> {
        // Loomy 返回 JSON，不是 SSE
        let json: Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(_) => return Vec::new(),
        };

        if let Some(error) = json.get("error") {
            let msg = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return vec![crate::core::events::Event::Error(
                crate::core::events::UpstreamError {
                    code: None,
                    message: msg.to_string(),
                    kind: crate::core::events::ErrorKind::Api,
                },
            )];
        }

        let choice = json
            .get("choices")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first());

        if let Some(choice) = choice {
            let message = choice.get("message").and_then(|m| m.as_object());

            if let Some(msg) = message {
                if let Some(content) = msg.get("content").and_then(|v| v.as_str()) {
                    self.content.push_str(content);
                    return vec![crate::core::events::Event::Text(content.to_string())];
                }

                if let Some(tool_calls) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                    let mut events = Vec::new();
                    for (idx, tc) in tool_calls.iter().enumerate() {
                        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
                        let func = tc.get("function").and_then(|f| f.as_object());
                        let name = func
                            .and_then(|f| f.get("name"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let args = func
                            .and_then(|f| f.get("arguments"))
                            .and_then(|v| v.as_str())
                            .unwrap_or("");

                        events.push(crate::core::events::Event::ToolCall(
                            crate::core::events::ToolCallDelta {
                                index: idx as u32,
                                id: Some(id.to_string()),
                                name: Some(name.to_string()),
                                arguments: Some(args.to_string()),
                            },
                        ));
                    }
                    return events;
                }
            }
        }

        Vec::new()
    }
}
