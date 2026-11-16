//! OpenAI 请求 → 供应商无关请求。

use crate::core::provider as p;

use super::types as wire;

/// 转换失败原因（缺字段、未知内容片段等）。
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("messages 为空")]
    EmptyMessages,
    #[error("第 {0} 条消息缺少 content")]
    MissingContent(usize),
    /// 未知 `content` 片段类型。目前未遇到，保留以覆盖未来 OpenAI 新增类型。
    #[allow(dead_code)]
    #[error("不支持的 content 片段类型")]
    UnsupportedPart,
    #[error("assistant 的 tool_calls 缺少 function.name")]
    MissingToolName,
}

pub fn to_provider_request(
    model: String,
    req: wire::ChatRequest,
) -> Result<p::ChatRequest, ConvertError> {
    if req.messages.is_empty() {
        return Err(ConvertError::EmptyMessages);
    }
    let mut messages = Vec::with_capacity(req.messages.len());
    for (idx, m) in req.messages.into_iter().enumerate() {
        let mut parts = Vec::new();
        if let Some(content) = m.content {
            match content {
                wire::MessageContent::Text(t) => parts.push(p::Part::Text(t)),
                wire::MessageContent::Parts(ps) => {
                    for part in ps {
                        match part {
                            wire::ContentPart::Text { text } => parts.push(p::Part::Text(text)),
                            wire::ContentPart::ImageUrl { image_url } => {
                                parts.push(p::Part::Image {
                                    url: image_url.url,
                                    detail: image_url.detail,
                                })
                            }
                        }
                    }
                }
            }
        }
        // 仅 tool 消息允许空 content（视为空串）。
        let role = match m.role {
            wire::Role::System | wire::Role::Developer => p::Role::System,
            wire::Role::User => p::Role::User,
            wire::Role::Assistant => p::Role::Assistant,
            wire::Role::Tool => p::Role::Tool,
        };
        if parts.is_empty() && matches!(role, p::Role::User | p::Role::System) {
            return Err(ConvertError::MissingContent(idx));
        }

        let mut tool_calls = Vec::new();
        for tc in m.tool_calls.unwrap_or_default() {
            tool_calls.push(p::ToolCall {
                id: tc.id.unwrap_or_default(),
                name: tc.function.name.ok_or(ConvertError::MissingToolName)?,
                arguments: tc.function.arguments.unwrap_or_else(|| "{}".to_string()),
            });
        }

        messages.push(p::Message {
            role,
            parts,
            tool_calls,
            tool_call_id: m.tool_call_id,
            tool_name: m.name,
        });
    }

    let tools = req
        .tools
        .unwrap_or_default()
        .into_iter()
        .map(|t| p::ToolSpec {
            name: t.function.name,
            description: t.function.description,
            parameters: t.function.parameters,
        })
        .collect();

    let require_tool =
        matches!(&req.tool_choice, Some(serde_json::Value::String(s)) if s == "required");

    Ok(p::ChatRequest {
        model,
        messages,
        tools,
        require_tool,
    })
}
