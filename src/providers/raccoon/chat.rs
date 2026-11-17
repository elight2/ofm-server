//! 对话请求体：`POST /api/web/llm/v2/chat/completions`。
//!
//! **上游就是标准 OpenAI Chat Completions**：请求体与响应帧都不需要做格式
//! 转换（官方客户端把 base 设为 `.../llm/v2`，按 `openai` 协议直连）。
//! 实测见 `provider-info/raccoon.md` §3。
//!
//! 因此这里只需把内部的 [`ChatRequest`] 序列化成 OpenAI 形态。

use serde::Serialize;

use crate::config::RaccoonReasoningEffort;
use crate::core::provider::{ChatRequest, Message, Part, Role};

/// 对话路径（相对 `api::LLM_PREFIX`）。
pub const PATH: &str = "/chat/completions";

#[derive(Debug, Serialize)]
pub struct ChatBody {
    pub model: String,
    /// 上游恒回 SSE；非流式由下游聚合。故恒发 `true`。
    pub stream: bool,
    pub messages: Vec<UpMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<UpTool>,
    /// `tool_choice` 透传。上游是标准 OpenAI 语义，四种取值均实测生效
    /// （见 `provider-info/raccoon.md` §3.6）。`Auto` 时不发送该字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<serde_json::Value>,
    /// 思考强度。只有 `low` 才发送；`high` 不发送该字段。
    ///
    /// 实测见 `provider-info/raccoon.md` §3.7：发 `"low"` 思考被完全抑制，
    /// 发 `"high"` 反而比不发时思考更少，故「深度思考」用不发送实现。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<&'static str>,
}

#[derive(Debug, Serialize)]
pub struct UpTool {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: UpFunction,
}

#[derive(Debug, Serialize)]
pub struct UpFunction {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 对象形态的 JSON Schema（与 trae 不同，这里**不要**字符串化）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct UpMessage {
    pub role: &'static str,
    /// 纯文本时给字符串，含图片时给内容数组（OpenAI 两种形态都接受）。
    pub content: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<UpToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct UpToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// 标准 OpenAI 形态：`function`（非 trae 的 `function_call`）。
    pub function: UpFunctionCall,
}

#[derive(Debug, Serialize)]
pub struct UpFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum UpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Serialize)]
pub struct ImageUrl {
    pub url: String,
}

impl ChatBody {
    pub fn build(req: &ChatRequest, effort: RaccoonReasoningEffort) -> Self {
        Self {
            model: req.model.clone(),
            stream: true,
            messages: req.messages.iter().map(convert_message).collect(),
            tools: req
                .tools
                .iter()
                .map(|t| UpTool {
                    kind: "function",
                    function: UpFunction {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        parameters: t.parameters.clone(),
                    },
                })
                .collect(),
            tool_choice: req.tool_choice.to_json(),
            reasoning_effort: effort_field(effort),
        }
    }
}

/// 档位 → 上游 `reasoning_effort` 取值。`None` 表示不发送该字段。
fn effort_field(effort: RaccoonReasoningEffort) -> Option<&'static str> {
    match effort {
        RaccoonReasoningEffort::Low => Some("low"),
        RaccoonReasoningEffort::High => None,
    }
}

fn convert_message(m: &Message) -> UpMessage {
    // 全为文本且只有一个文本片段 → 给字符串，最兼容。
    // 其余情况用内容数组（图片必须走数组形态）。
    let only_text = m.parts.iter().all(|p| matches!(p, Part::Text(_)));
    let content = if only_text && m.parts.len() <= 1 {
        let text = m
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .collect::<String>();
        serde_json::Value::String(text)
    } else {
        let arr: Vec<UpContent> = m
            .parts
            .iter()
            .map(|p| match p {
                Part::Text(t) => UpContent::Text { text: t.clone() },
                Part::Image { url, .. } => UpContent::ImageUrl {
                    image_url: ImageUrl { url: url.clone() },
                },
            })
            .collect();
        serde_json::to_value(arr).unwrap_or(serde_json::Value::String(String::new()))
    };

    UpMessage {
        role: role_str(m.role),
        content,
        tool_calls: m
            .tool_calls
            .iter()
            .map(|c| UpToolCall {
                id: c.id.clone(),
                kind: "function",
                function: UpFunctionCall {
                    name: c.name.clone(),
                    arguments: c.arguments.clone(),
                },
            })
            .collect(),
        tool_call_id: if m.role == Role::Tool {
            m.tool_call_id.clone()
        } else {
            None
        },
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::provider::{ToolCall, ToolSpec};

    fn user_msg(text: &str) -> Message {
        Message {
            role: Role::User,
            parts: vec![Part::Text(text.into())],
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// 单段文本用字符串形态；恒流式。
    #[test]
    fn 纯文本用字符串且恒流式() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "sn-glm-5-3".into(),
                messages: vec![user_msg("你好")],
                tools: vec![],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["model"], "sn-glm-5-3");
        assert_eq!(v["stream"], true);
        assert_eq!(v["messages"][0]["content"], "你好");
        // 无工具时不出现 tools 字段
        assert!(v.get("tools").is_none());
    }

    /// 含图片时 content 必须是数组。
    #[test]
    fn 图片走内容数组() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![Message {
                    role: Role::User,
                    parts: vec![
                        Part::Text("看图".into()),
                        Part::Image {
                            url: "data:image/png;base64,AAA".into(),
                            detail: None,
                        },
                    ],
                    tool_calls: vec![],
                    tool_call_id: None,
                    tool_name: None,
                }],
                tools: vec![],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        let arr = v["messages"][0]["content"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "image_url");
        assert_eq!(arr[1]["image_url"]["url"], "data:image/png;base64,AAA");
    }

    /// 工具定义用对象形态的 parameters（不要字符串化）。
    #[test]
    fn 工具参数保持对象形态() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![user_msg("查天气")],
                tools: vec![ToolSpec {
                    name: "get_weather".into(),
                    description: Some("查天气".into()),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {"city": {"type": "string"}}
                    })),
                }],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["tools"][0]["type"], "function");
        assert_eq!(v["tools"][0]["function"]["name"], "get_weather");
        // 必须是对象，不能是字符串
        assert!(v["tools"][0]["function"]["parameters"].is_object());
    }

    /// assistant 的工具调用键名是标准 `function`。
    #[test]
    fn 工具调用用function键() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![Message {
                    role: Role::Assistant,
                    parts: vec![],
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "get_weather".into(),
                        arguments: r#"{"city":"北京"}"#.into(),
                    }],
                    tool_call_id: None,
                    tool_name: None,
                }],
                tools: vec![],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        let tc = &v["messages"][0]["tool_calls"][0];
        assert_eq!(tc["id"], "call_1");
        assert_eq!(tc["type"], "function");
        assert_eq!(tc["function"]["name"], "get_weather");
        assert!(tc.get("function_call").is_none());
    }

    /// 工具结果消息需要 tool_call_id，且不带 tool_calls。
    #[test]
    fn 工具结果带tool_call_id() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![Message {
                    role: Role::Tool,
                    parts: vec![Part::Text("晴 25 度".into())],
                    tool_calls: vec![],
                    tool_call_id: Some("call_1".into()),
                    tool_name: Some("get_weather".into()),
                }],
                tools: vec![],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["messages"][0]["role"], "tool");
        assert_eq!(v["messages"][0]["tool_call_id"], "call_1");
        assert!(v["messages"][0].get("tool_calls").is_none());
    }

    /// 多段文本合并成一条数组内容（不丢片段）。
    #[test]
    fn 多段文本走数组() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![Message {
                    role: Role::User,
                    parts: vec![Part::Text("a".into()), Part::Text("b".into())],
                    tool_calls: vec![],
                    tool_call_id: None,
                    tool_name: None,
                }],
                tools: vec![],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        let arr = v["messages"][0]["content"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["text"], "a");
        assert_eq!(arr[1]["text"], "b");
    }

    /// 无参数的工具有效：省略 parameters 字段，不发空对象。
    #[test]
    fn 无参数工具省略parameters() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![user_msg("hi")],
                tools: vec![ToolSpec {
                    name: "now".into(),
                    description: None,
                    parameters: None,
                }],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        assert!(v["tools"][0]["function"].get("parameters").is_none());
    }

    /// `Auto` 不发送 `tool_choice` 字段——那是有工具时的默认行为。
    #[test]
    fn tool_choice_auto_不发送() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![user_msg("hi")],
                tools: vec![],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        assert!(v.get("tool_choice").is_none());
    }

    /// 其余三种取值按标准 OpenAI 形态透传（实测上游逐项生效）。
    #[test]
    fn tool_choice_透传三种取值() {
        use crate::core::provider::ToolChoice;
        let cases = [
            (ToolChoice::None, serde_json::json!("none")),
            (ToolChoice::Required, serde_json::json!("required")),
            (
                ToolChoice::Function("get_weather".into()),
                serde_json::json!({"type":"function","function":{"name":"get_weather"}}),
            ),
        ];
        for (choice, want) in cases {
            let body = ChatBody::build(
                &ChatRequest {
                    model: "m".into(),
                    messages: vec![user_msg("hi")],
                    tools: vec![],
                    tool_choice: choice,
                },
                RaccoonReasoningEffort::Low,
            );
            let v = serde_json::to_value(&body).unwrap();
            assert_eq!(v["tool_choice"], want);
        }
    }

    /// `low` 档向上游发送 `reasoning_effort: "low"`。
    #[test]
    fn low档发送reasoning_effort() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![user_msg("hi")],
                tools: vec![],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::Low,
        );
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["reasoning_effort"], "low");
    }

    /// `high` 档**不发送**该字段（实测发送反而思考更少）。
    #[test]
    fn high档不发送reasoning_effort() {
        let body = ChatBody::build(
            &ChatRequest {
                model: "m".into(),
                messages: vec![user_msg("hi")],
                tools: vec![],
                tool_choice: Default::default(),
            },
            RaccoonReasoningEffort::High,
        );
        let v = serde_json::to_value(&body).unwrap();
        assert!(v.get("reasoning_effort").is_none());
    }
}
