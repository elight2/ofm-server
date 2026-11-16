//! 对话请求体（Chat Completions → 上游 `llm_utils_chat`）。
//!
//! 字段形态严格照 `provider-info/trae.md` §3.2 的实测结论：
//! `messages[].content` 必须是数组、`tools[].function.parameters` 必须字符串化、
//! assistant 的工具调用键名是 `function_call`。

use serde::Serialize;

use crate::core::provider::{ChatRequest, Message, Part, Role};

pub const ENDPOINT_PATH: &str = "/api/agent/v3/llm_utils_chat";

/// `app_id` 必填（`provider-info/trae.md` §3.2，`[curl]` 缺失 → `400 expr_path=app_id`）。
pub const APP_ID: &str = "6eefa01c-1036-4c7e-9ca5-d891f63bfcd8";

/// `function` 是模型准入闸门；对话定稿 `solo_agent`（§3.2）。
const FUNCTION: &str = "solo_agent";

#[derive(Debug, Serialize)]
pub struct ChatBody {
    pub app_id: &'static str,
    pub model: String,
    pub config_name: String,
    pub function: &'static str,
    /// 上游对 `stream:false` 仍回 SSE，故恒发 `true`，非流式由下游聚合。
    pub stream: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolDef>,
    // ⚠️ 故意**不发送** `tool_choice`：上游 `LLMUtilsChatRequest` 虽有该字段（string 类型），
    // 但实测 `required` / `none` / 指定函数三者**均不生效**，行为与不传完全一致
    // （见 `provider-info/trae.md` §3.2.2）。CC 侧语义在 trae 预设模型上无法表达。
    pub messages: Vec<UpMessage>,
}

#[derive(Debug, Serialize)]
pub struct ToolDef {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionDef,
}

#[derive(Debug, Serialize)]
pub struct FunctionDef {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// **字符串化的 JSON Schema**（对象形态会被拒）。
    pub parameters: String,
}

#[derive(Debug, Serialize)]
pub struct UpMessage {
    pub role: &'static str,
    pub content: Vec<UpContent>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<UpToolCall>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum UpContent {
    #[serde(rename = "text")]
    Text { text: String },
    /// 唯一可用形态：`{type:"image_url", image_url:{url}}`；
    /// `url` 支持外链与 `data:` URL（§3.2）。
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Serialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Serialize)]
pub struct UpToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// 注意是 `function_call`，不是 Chat Completions 的 `function`。
    pub function_call: UpFunctionCall,
}

#[derive(Debug, Serialize)]
pub struct UpFunctionCall {
    pub name: String,
    pub arguments: String,
}

impl ChatBody {
    /// `req.tool_choice` 被**有意忽略**，原因见结构体字段注释。
    pub fn build(req: &ChatRequest) -> Self {
        Self {
            app_id: APP_ID,
            model: req.model.clone(),
            config_name: req.model.clone(),
            function: FUNCTION,
            stream: true,
            tools: req
                .tools
                .iter()
                .map(|t| ToolDef {
                    kind: "function",
                    function: FunctionDef {
                        name: t.name.clone(),
                        description: t.description.clone(),
                        // 缺省时必须给合法 JSON Schema 字符串，不能省略该字段。
                        parameters: t
                            .parameters
                            .as_ref()
                            .map(|p| p.to_string())
                            .unwrap_or_else(|| {
                                r#"{"type":"object","properties":{},"required":[]}"#.to_string()
                            }),
                    },
                })
                .collect(),
            messages: req.messages.iter().map(convert_message).collect(),
        }
    }
}

fn convert_message(m: &Message) -> UpMessage {
    let mut out = UpMessage {
        role: role_str(m.role),
        content: Vec::new(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        name: None,
    };

    for part in &m.parts {
        match part {
            Part::Text(t) => out.content.push(UpContent::Text { text: t.clone() }),
            Part::Image { url, .. } => out.content.push(UpContent::ImageUrl {
                image_url: ImageUrl { url: url.clone() },
            }),
        }
    }

    for call in &m.tool_calls {
        out.tool_calls.push(UpToolCall {
            id: call.id.clone(),
            kind: "function",
            function_call: UpFunctionCall {
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            },
        });
    }

    if m.role == Role::Tool {
        out.tool_call_id = m.tool_call_id.clone();
        out.name = m.tool_name.clone();
    }

    out
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

    #[test]
    fn content_is_always_array_and_params_stringified() {
        let body = ChatBody::build(&ChatRequest {
            model: "glm-5.2".into(),
            messages: vec![Message {
                role: Role::User,
                parts: vec![Part::Text("你好".into())],
                tool_calls: vec![],
                tool_call_id: None,
                tool_name: None,
            }],
            tools: vec![ToolSpec {
                name: "get_time".into(),
                description: Some("取时间".into()),
                parameters: Some(serde_json::json!({"type": "object", "properties": {}})),
            }],
            tool_choice: Default::default(),
        });

        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["messages"][0]["content"][0]["type"], "text");
        assert_eq!(v["messages"][0]["content"][0]["text"], "你好");
        // parameters 必须是字符串
        let params = v["tools"][0]["function"]["parameters"].as_str().unwrap();
        assert!(params.starts_with('{'));
        assert_eq!(v["function"], "solo_agent");
        assert_eq!(v["stream"], true);
        assert_eq!(v["app_id"], APP_ID);
    }

    #[test]
    fn assistant_tool_call_uses_function_call_key() {
        let body = ChatBody::build(&ChatRequest {
            model: "glm-5.2".into(),
            messages: vec![
                Message {
                    role: Role::Assistant,
                    parts: vec![],
                    tool_calls: vec![ToolCall {
                        id: "call_1".into(),
                        name: "get_time".into(),
                        arguments: "{}".into(),
                    }],
                    tool_call_id: None,
                    tool_name: None,
                },
                Message {
                    role: Role::Tool,
                    parts: vec![Part::Text("12:00".into())],
                    tool_calls: vec![],
                    tool_call_id: Some("call_1".into()),
                    tool_name: Some("get_time".into()),
                },
            ],
            tools: vec![],
            tool_choice: Default::default(),
        });

        let v = serde_json::to_value(&body).unwrap();
        let call = &v["messages"][0]["tool_calls"][0];
        assert!(call.get("function_call").is_some());
        assert!(call.get("function").is_none());
        assert_eq!(call["function_call"]["arguments"], "{}");
        // assistant 带工具调用时 content 为空数组
        assert_eq!(v["messages"][0]["content"].as_array().unwrap().len(), 0);
        // tool 消息带 tool_call_id / name
        assert_eq!(v["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(v["messages"][1]["name"], "get_time");
    }

    #[test]
    fn image_uses_image_url_object() {
        let body = ChatBody::build(&ChatRequest {
            model: "glm-5.2".into(),
            messages: vec![Message {
                role: Role::User,
                parts: vec![Part::Image {
                    url: "data:image/png;base64,AAA".into(),
                    detail: Some("high".into()),
                }],
                tool_calls: vec![],
                tool_call_id: None,
                tool_name: None,
            }],
            tools: vec![],
            tool_choice: Default::default(),
        });
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["messages"][0]["content"][0]["type"], "image_url");
        assert_eq!(
            v["messages"][0]["content"][0]["image_url"]["url"],
            "data:image/png;base64,AAA"
        );
    }
}
