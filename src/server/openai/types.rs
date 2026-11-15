//! OpenAI Chat Completions 线格式类型（仅覆盖本服务器需要转发的子集）。
//!
//! 设计原则：
//! * 请求侧尽可能宽松（未知字段忽略），把"上游不支持"的参数交给供应商层自行丢弃；
//! * 响应侧严格，保证下游 OpenAI 客户端能解析。

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------- 请求

#[derive(Debug, Clone, Deserialize)]
pub struct ChatRequest {
    /// `供应商id/模型名`。
    pub model: String,
    pub messages: Vec<Message>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub tools: Option<Vec<Tool>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    /// 其余字段（temperature 等）本服务器不转发，忽略。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Message {
    pub role: Role,
    #[serde(default)]
    pub content: Option<MessageContent>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// `content` 既可能是纯字符串，也可能是内容片段数组。
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(default)]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FunctionCall {
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Tool {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub function: ToolSpec,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parameters: Option<Value>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

// ---------------------------------------------------------------- 响应（非流式）

#[derive(Debug, Clone, Serialize)]
pub struct Completion {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<CompletionChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionChoice {
    pub index: u32,
    pub message: CompletionMessage,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionMessage {
    pub role: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// 扩展字段：透传上游思考增量。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallOut>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallOut {
    pub index: u32,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionCallOut,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionCallOut {
    pub name: String,
    pub arguments: String,
}

// ---------------------------------------------------------------- 响应（流式）

#[derive(Debug, Clone, Serialize)]
pub struct Chunk {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub model: String,
    pub choices: Vec<ChunkChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChunkChoice {
    pub index: u32,
    pub delta: Delta,
    pub finish_reason: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Delta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDeltaOut>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolCallDeltaOut {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallDeltaOut>,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionCallDeltaOut {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
}

// ---------------------------------------------------------------- 用量

#[derive(Debug, Clone, Default, Serialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PromptTokensDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CompletionTokensDetails {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_tokens: Option<u64>,
}

// ---------------------------------------------------------------- 模型列表

#[derive(Debug, Clone, Serialize)]
pub struct ModelList {
    pub object: &'static str,
    pub data: Vec<ModelCard>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelCard {
    pub id: String,
    pub object: &'static str,
    pub created: u64,
    pub owned_by: String,
}

// ---------------------------------------------------------------- 错误

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    pub message: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub param: Option<String>,
    pub code: Option<String>,
}
