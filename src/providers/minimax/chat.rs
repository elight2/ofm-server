//! 对话请求体：Chat Completions → 上游 anthropic-messages。
//!
//! 协议见 `provider-info/minimax.md` §3。上游是 **anthropic-messages**，与
//! raccoon / trae 的 OpenAI 兼容网关不同，需要完整改写消息结构：
//!
//! * `system`/`developer` 消息**抽出**拼成顶层 `system` 数组（不是留在 messages 里）；
//! * `role:tool` 消息转成 `role:user` + `tool_result` 块（Anthropic 语义）；
//! * assistant 的 `tool_calls` 转成 `tool_use` 块；
//! * 图片转 `{type:image, source:{type:base64,…}}`，**只支持 base64**（§3.2）。
//!
//! 上游恒 `stream:true`（§3.2），非流式由下游聚合。

use base64::Engine;
use serde::Serialize;

use crate::config::MinimaxReasoningEffort;
use crate::core::provider::{ChatRequest, Message, Part, Role, ToolChoice};

/// 对话路径（相对 [`super::api::AGENT_HOST`]）。
///
/// 注意比模型目录多一级 `llm/v1`（§4 与 §3.1 的路径差异）。
pub const PATH: &str = "/mavis/api/v1/llm/v1/messages";

/// Anthropic API 版本头（§3.1）。
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// 输出上限。
///
/// 目录里 `limit.output = 128000`，官方客户端正常对话也用 128000（§3.2），
/// 故直接给满。下游 CC 的 `max_tokens` 目前不解析（`ChatRequest` 无该字段）。
pub const MAX_TOKENS: u64 = 128_000;

/// 图片 media_type 白名单（§3.2 源码）。
const IMAGE_TYPES: [&str; 4] = ["image/jpeg", "image/png", "image/gif", "image/webp"];

/// 仅图无文字时补的占位文本（§3.2 源码）。
const IMAGE_PLACEHOLDER: &str = "(see attached image)";

#[derive(Debug, Serialize)]
pub struct ChatBody {
    pub model: String,
    pub max_tokens: u64,
    /// 上游恒回 SSE，故恒发 `true`。
    pub stream: bool,
    /// 抽出的系统提示。**缺失时不发该字段**（§3.2：8/26 次完全不带 `system`）。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<UpTextBlock>,
    pub messages: Vec<UpMessage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<UpTool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<UpToolChoice>,
    /// 思考落点之一：M3（`switchable`）用 `thinking`（§5.1）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Thinking>,
    /// 思考落点之二：M3.1-Flash（`forced_on`）用 `output_config.effort`（§5.1）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<OutputConfig>,
}

#[derive(Debug, Serialize)]
pub struct UpTextBlock {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct Thinking {
    #[serde(rename = "type")]
    pub kind: &'static str,
}

#[derive(Debug, Serialize)]
pub struct OutputConfig {
    pub effort: &'static str,
}

#[derive(Debug, Serialize)]
pub struct UpMessage {
    pub role: &'static str,
    pub content: Vec<UpBlock>,
}

/// 内容块并集（§3.2 抓包块类型：`text`/`image`/`tool_use`/`tool_result`）。
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum UpBlock {
    #[serde(rename = "text")]
    Text { text: String },
    /// 图片。**只支持 base64**，无 `url` 来源类型（§3.2）。
    #[serde(rename = "image")]
    Image { source: ImageSource },
    /// assistant 发起的工具调用。
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    /// 工具结果。**进 `role:user`**，`content` 是纯字符串（§3.4）。
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

#[derive(Debug, Serialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub media_type: String,
    pub data: String,
}

#[derive(Debug, Serialize)]
pub struct UpTool {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// 对象形态的 JSON Schema（**不要**字符串化，§3.4）。
    pub input_schema: serde_json::Value,
    /// 抓包里 18/18 次带 tools 的请求都有此字段；是否必需未验证（§3.4）。
    pub eager_input_streaming: bool,
}

/// Anthropic 的 `tool_choice`（与 CC 形态不同，§3.4 实测四种均 200）。
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum UpToolChoice {
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "any")]
    Any,
    #[serde(rename = "none")]
    None,
    #[serde(rename = "tool")]
    Tool { name: String },
}

impl ChatBody {
    /// `reasoning_effort` 与模型形态共同决定思考字段落点。
    ///
    /// * `forced_on`（M3.1-Flash）：档位**就近映射**到目录的 `effort_options`，
    ///   只发 `output_config.effort`，**不发** `thinking`
    ///   （该模型发 `thinking.type=disabled` 会被 400 拒绝，实测）。
    /// * `switchable`（M3）：五档全部等价于开，只发 `thinking.type=adaptive`。
    ///
    /// 模型形态从目录里查不到时按 `switchable` 处理：明确发 `thinking.type`
    /// 比猜测 `effort` 更安全（发错档位值会被 400 拒）。
    pub fn build(
        req: &ChatRequest,
        effort: MinimaxReasoningEffort,
        thinking: &ThinkingShape,
    ) -> Self {
        let (system, messages) = split_messages(&req.messages);
        let (thinking_field, output_config) = thinking_fields(effort, thinking);
        Self {
            model: req.model.clone(),
            max_tokens: MAX_TOKENS,
            stream: true,
            system,
            messages,
            tools: req
                .tools
                .iter()
                .map(|t| UpTool {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    input_schema: t
                        .parameters
                        .clone()
                        .unwrap_or_else(|| serde_json::json!({"type": "object", "properties": {}})),
                    eager_input_streaming: true,
                })
                .collect(),
            tool_choice: tool_choice(req),
            thinking: thinking_field,
            output_config,
        }
    }
}

/// 模型在目录里的思考形态（§5.1）。
///
/// 目录拉不到或模型不在目录时用 [`ThinkingShape::default`]（按 `switchable` 处理）。
#[derive(Debug, Clone, Default)]
pub struct ThinkingShape {
    /// `thinking_config.mode`：`switchable` | `forced_on` | 空（未知）。
    pub mode: String,
    /// 目录下发的可选档位（只有 `forced_on` 模型有），§5.2。
    pub effort_options: Vec<String>,
}

/// 思考字段落点（§5.1/§5.4）。
///
/// 返回 `(thinking, output_config)`，二者恰好一个为 `Some`。
fn thinking_fields(
    effort: MinimaxReasoningEffort,
    shape: &ThinkingShape,
) -> (Option<Thinking>, Option<OutputConfig>) {
    if shape.mode == "forced_on" {
        // M3.1-Flash：档位就近映射到目录档位表（§5.2）。
        return (
            None,
            Some(OutputConfig {
                effort: effort.nearest_effort(&shape.effort_options),
            }),
        );
    }
    // M3（switchable）：无档位，五档一律等价于开思考。
    (Some(Thinking { kind: "adaptive" }), None)
}

/// `tool_choice` 映射（CC 四值 → Anthropic 四型）。
///
/// `Auto` 不发送该字段。实测上游四种形态均 200（本次实测，§3.4）。
fn tool_choice(req: &ChatRequest) -> Option<UpToolChoice> {
    // 没有工具时发 tool_choice 无意义，且 `any` 会因无工具被拒。
    if req.tools.is_empty() {
        return None;
    }
    match &req.tool_choice {
        ToolChoice::Auto => Some(UpToolChoice::Auto),
        ToolChoice::None => Some(UpToolChoice::None),
        ToolChoice::Required => Some(UpToolChoice::Any),
        ToolChoice::Function(name) => Some(UpToolChoice::Tool { name: name.clone() }),
    }
}

/// 拆出 `system` 并把消息转成 Anthropic 形态。
///
/// 三条硬约束：
/// * `system`/`developer` 一律抽出，**不进** `messages`（§3.2）；
/// * `role:tool` → `role:user` + `tool_result` 块（§3.4）；
/// * 相邻的 tool 结果**并入同一条 user 消息**（抓包见一次两个，§3.4）。
fn split_messages(msgs: &[Message]) -> (Vec<UpTextBlock>, Vec<UpMessage>) {
    let mut system: Vec<String> = Vec::new();
    let mut out: Vec<UpMessage> = Vec::new();

    for m in msgs {
        match m.role {
            Role::System => {
                let text = plain_text(&m.parts);
                if !text.is_empty() {
                    system.push(text);
                }
            }
            Role::User | Role::Assistant => {
                let mut blocks = content_blocks(&m.parts);
                if m.role == Role::Assistant {
                    blocks.extend(m.tool_calls.iter().map(|c| UpBlock::ToolUse {
                        id: c.id.clone(),
                        name: c.name.clone(),
                        input: parse_arguments(&c.arguments),
                    }));
                }
                if blocks.is_empty() {
                    continue;
                }
                out.push(UpMessage {
                    role: role_str(m.role),
                    content: blocks,
                });
            }
            Role::Tool => {
                let block = UpBlock::ToolResult {
                    tool_use_id: m.tool_call_id.clone().unwrap_or_default(),
                    content: plain_text(&m.parts),
                    // CC 的 tool 消息没有错误标志，恒 false。
                    is_error: false,
                };
                // 上一条已是 user（且全是 tool_result）→ 并入，符合 §3.4 抓包形态。
                if let Some(last) = out.last_mut()
                    && last.role == "user"
                    && last
                        .content
                        .iter()
                        .all(|b| matches!(b, UpBlock::ToolResult { .. }))
                {
                    last.content.push(block);
                    continue;
                }
                out.push(UpMessage {
                    role: "user",
                    content: vec![block],
                });
            }
        }
    }

    // 系统提示恒为**恰好 1 个** text 块（§3.2 强约束：18/18 都是 1 个）。
    let system_blocks = if system.is_empty() {
        Vec::new()
    } else {
        vec![UpTextBlock {
            kind: "text",
            text: system.join("\n\n"),
        }]
    };
    (system_blocks, out)
}

/// 消息片段 → 内容块。图片与文字按原序保留。
fn content_blocks(parts: &[Part]) -> Vec<UpBlock> {
    let has_image = parts.iter().any(|p| matches!(p, Part::Image { .. }));

    let mut out = Vec::new();
    // 只有图没有文字时，上游客户端会在最前面补占位文本（§3.2）。
    if has_image && plain_text(parts).is_empty() {
        out.push(UpBlock::Text {
            text: IMAGE_PLACEHOLDER.to_string(),
        });
    }
    for p in parts {
        match p {
            Part::Text(t) => {
                if !t.is_empty() {
                    out.push(UpBlock::Text { text: t.clone() });
                }
            }
            Part::Image { url, .. } => {
                if let Some(block) = image_block(url) {
                    out.push(block);
                }
            }
        }
    }
    out
}

/// `image_url` → Anthropic image 块。
///
/// 上游**只接受 base64**（§3.2）：`data:` URL 直接解出 media_type 与 data；
/// 外链（`http(s)://`）无法在此同步下载（`chat` 是 async，但转换函数是纯函数），
/// 故丢弃并记日志——宁可少一张图，也不要让整个请求 400。
fn image_block(url: &str) -> Option<UpBlock> {
    let Some(rest) = url.strip_prefix("data:") else {
        tracing::warn!(
            url = super::api::truncate(url, 80),
            "上游只接受 base64 图片，已丢弃外链图片"
        );
        return None;
    };
    let (meta, data) = rest.split_once(',')?;
    if !meta.contains(";base64") {
        tracing::warn!("图片 data URL 非 base64，已丢弃");
        return None;
    }
    let media_type = meta.split(';').next().unwrap_or_default().to_string();
    if !IMAGE_TYPES.contains(&media_type.as_str()) {
        tracing::warn!(%media_type, "图片 media_type 不在白名单，已丢弃");
        return None;
    }
    // data 段必须是合法 base64，否则上游会 400；这里先校验并规整。
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(data.trim())
        .ok()?;
    Some(UpBlock::Image {
        source: ImageSource {
            kind: "base64",
            media_type,
            data: base64::engine::general_purpose::STANDARD.encode(decoded),
        },
    })
}

/// 拼接纯文本片段。
fn plain_text(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.as_str()),
            Part::Image { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("")
}

/// 工具参数：CC 给的是 JSON **字符串**，Anthropic 要 **对象**（§3.4）。
///
/// 解析失败时给空对象，不要让整个请求失败——历史消息里的坏参数不该阻断当前轮。
fn parse_arguments(raw: &str) -> serde_json::Value {
    if raw.trim().is_empty() {
        return serde_json::json!({});
    }
    match serde_json::from_str(raw) {
        Ok(v @ serde_json::Value::Object(_)) => v,
        Ok(_) => serde_json::json!({}),
        Err(e) => {
            tracing::warn!(error = %e, "工具参数不是合法 JSON，按空对象处理");
            serde_json::json!({})
        }
    }
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "user",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::provider::{ToolCall, ToolSpec};

    fn msg(role: Role, text: &str) -> Message {
        Message {
            role,
            parts: vec![Part::Text(text.into())],
            tool_calls: vec![],
            tool_call_id: None,
            tool_name: None,
        }
    }

    /// 构造思考形态。`mode` 为 `""` 表示目录拉取失败/模型不在目录。
    fn shape(mode: &str, options: &[&str]) -> ThinkingShape {
        ThinkingShape {
            mode: mode.to_string(),
            effort_options: options.iter().map(|s| s.to_string()).collect(),
        }
    }

    /// 上游实际下发的档位表（§5.2）。
    const FLASH_OPTIONS: &[&str] = &["default", "low", "medium", "high", "xhigh", "max"];

    fn body(req: &ChatRequest, effort: MinimaxReasoningEffort, mode: &str) -> serde_json::Value {
        let s = match mode {
            "forced_on" => shape(mode, FLASH_OPTIONS),
            other => shape(other, &[]),
        };
        serde_json::to_value(ChatBody::build(req, effort, &s)).unwrap()
    }

    fn req(messages: Vec<Message>) -> ChatRequest {
        ChatRequest {
            model: "MiniMax-M3".into(),
            messages,
            tools: vec![],
            tool_choice: Default::default(),
        }
    }

    /// 上游恒流式、路径多一级 `llm/v1`、max_tokens 给满。
    #[test]
    fn 基本形态() {
        let v = body(
            &req(vec![msg(Role::User, "你好")]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        assert_eq!(v["model"], "MiniMax-M3");
        assert_eq!(v["stream"], true);
        assert_eq!(v["max_tokens"], 128000);
        assert_eq!(PATH, "/mavis/api/v1/llm/v1/messages");
    }

    /// system 消息**抽出**成顶层数组，且恰好 1 个 text 块（§3.2 强约束）。
    #[test]
    fn system_抽出且合并成单块() {
        let v = body(
            &req(vec![
                msg(Role::System, "规则一"),
                msg(Role::User, "hi"),
                msg(Role::System, "规则二"),
            ]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        let sys = v["system"].as_array().unwrap();
        assert_eq!(sys.len(), 1);
        assert_eq!(sys[0]["type"], "text");
        assert!(sys[0]["text"].as_str().unwrap().contains("规则一"));
        assert!(sys[0]["text"].as_str().unwrap().contains("规则二"));
        // messages 里不能残留 system 角色
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0]["role"], "user");
    }

    /// 无 system 时**不发**该字段（不是空数组，§3.2）。
    #[test]
    fn 无system时不发字段() {
        let v = body(
            &req(vec![msg(Role::User, "hi")]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        assert!(v.get("system").is_none());
    }

    /// 用户消息的内容必须是**数组**（§3.2 主流形态）。
    #[test]
    fn 内容恒为数组() {
        let v = body(
            &req(vec![msg(Role::User, "hi")]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        let c = &v["messages"][0]["content"];
        assert!(c.is_array());
        assert_eq!(c[0]["type"], "text");
        assert_eq!(c[0]["text"], "hi");
    }

    /// assistant 的 tool_calls → `tool_use` 块，arguments 字符串变对象。
    #[test]
    fn 工具调用转tool_use() {
        let v = body(
            &req(vec![Message {
                role: Role::Assistant,
                parts: vec![],
                tool_calls: vec![ToolCall {
                    id: "call_1".into(),
                    name: "get_weather".into(),
                    arguments: r#"{"city":"北京"}"#.into(),
                }],
                tool_call_id: None,
                tool_name: None,
            }]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        let blk = &v["messages"][0]["content"][0];
        assert_eq!(blk["type"], "tool_use");
        assert_eq!(blk["id"], "call_1");
        assert_eq!(blk["name"], "get_weather");
        // 必须是对象，不能是字符串
        assert_eq!(blk["input"]["city"], "北京");
    }

    /// `role:tool` → `role:user` + `tool_result`，content 是**字符串**。
    #[test]
    fn 工具结果转user加tool_result() {
        let v = body(
            &req(vec![Message {
                role: Role::Tool,
                parts: vec![Part::Text("晴 25 度".into())],
                tool_calls: vec![],
                tool_call_id: Some("call_1".into()),
                tool_name: Some("get_weather".into()),
            }]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        let m = &v["messages"][0];
        assert_eq!(m["role"], "user");
        let blk = &m["content"][0];
        assert_eq!(blk["type"], "tool_result");
        assert_eq!(blk["tool_use_id"], "call_1");
        // 字符串，不是数组
        assert!(blk["content"].is_string());
        assert_eq!(blk["content"], "晴 25 度");
        assert_eq!(blk["is_error"], false);
    }

    /// 相邻多个工具结果**并入同一条 user 消息**（§3.4 抓包见一次两个）。
    #[test]
    fn 多个工具结果并入一条消息() {
        let tool = |id: &str, text: &str| Message {
            role: Role::Tool,
            parts: vec![Part::Text(text.into())],
            tool_calls: vec![],
            tool_call_id: Some(id.into()),
            tool_name: None,
        };
        let v = body(
            &req(vec![tool("call_1", "a"), tool("call_2", "b")]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        let msgs = v["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 1, "两条工具结果必须合并");
        assert_eq!(msgs[0]["content"].as_array().unwrap().len(), 2);
    }

    /// data URL 图片 → base64 source，media_type 从 URL 取。
    #[test]
    fn 图片转base64块() {
        let v = body(
            &req(vec![Message {
                role: Role::User,
                parts: vec![Part::Image {
                    url: "data:image/png;base64,QUJD".into(),
                    detail: Some("high".into()),
                }],
                tool_calls: vec![],
                tool_call_id: None,
                tool_name: None,
            }]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        let c = v["messages"][0]["content"].as_array().unwrap();
        // 只有图 → 先补占位文本（§3.2）
        assert_eq!(c[0]["type"], "text");
        assert_eq!(c[0]["text"], "(see attached image)");
        assert_eq!(c[1]["type"], "image");
        assert_eq!(c[1]["source"]["type"], "base64");
        assert_eq!(c[1]["source"]["media_type"], "image/png");
        assert_eq!(c[1]["source"]["data"], "QUJD");
    }

    /// 外链图片被丢弃（上游无 url 来源类型），且不 panic。
    #[test]
    fn 外链图片被丢弃() {
        let v = body(
            &req(vec![Message {
                role: Role::User,
                parts: vec![
                    Part::Text("看图".into()),
                    Part::Image {
                        url: "https://example.com/a.png".into(),
                        detail: None,
                    },
                ],
                tool_calls: vec![],
                tool_call_id: None,
                tool_name: None,
            }]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        let c = v["messages"][0]["content"].as_array().unwrap();
        assert_eq!(c.len(), 1, "外链图应被丢弃，只留文本");
        assert_eq!(c[0]["type"], "text");
    }

    /// 非法 base64 的 data URL 被丢弃。
    #[test]
    fn 非法base64被丢弃() {
        let v = body(
            &req(vec![Message {
                role: Role::User,
                parts: vec![
                    Part::Text("看图".into()),
                    Part::Image {
                        url: "data:image/png;base64,!!!不是base64!!!".into(),
                        detail: None,
                    },
                ],
                tool_calls: vec![],
                tool_call_id: None,
                tool_name: None,
            }]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        assert_eq!(v["messages"][0]["content"].as_array().unwrap().len(), 1);
    }

    /// 白名单外的 media_type 被丢弃。
    #[test]
    fn 非白名单类型被丢弃() {
        assert!(image_block("data:image/bmp;base64,QUJD").is_none());
        assert!(image_block("data:application/pdf;base64,QUJD").is_none());
        assert!(image_block("data:image/png;base64,QUJD").is_some());
    }

    /// 工具定义：input_schema 必须是对象、带 eager_input_streaming。
    #[test]
    fn 工具定义形态() {
        let mut r = req(vec![msg(Role::User, "hi")]);
        r.tools = vec![ToolSpec {
            name: "get_weather".into(),
            description: Some("查天气".into()),
            parameters: Some(serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}}
            })),
        }];
        let v = body(&r, MinimaxReasoningEffort::Low, "switchable");
        let t = &v["tools"][0];
        assert_eq!(t["name"], "get_weather");
        assert_eq!(t["eager_input_streaming"], true);
        // 对象，不是字符串
        assert!(t["input_schema"].is_object());
        assert_eq!(t["input_schema"]["type"], "object");
    }

    /// 无参数的工具给合法空 schema，不发 null。
    #[test]
    fn 无参数工具给空schema() {
        let mut r = req(vec![msg(Role::User, "hi")]);
        r.tools = vec![ToolSpec {
            name: "now".into(),
            description: None,
            parameters: None,
        }];
        let v = body(&r, MinimaxReasoningEffort::Low, "switchable");
        assert!(v["tools"][0]["input_schema"].is_object());
        assert_eq!(v["tools"][0]["input_schema"]["type"], "object");
    }

    /// CC 四值 → Anthropic 四型（实测均 200）。
    #[test]
    fn tool_choice_映射() {
        let mut r = req(vec![msg(Role::User, "hi")]);
        r.tools = vec![ToolSpec {
            name: "t".into(),
            description: None,
            parameters: None,
        }];
        let cases = [
            (ToolChoice::Auto, serde_json::json!({"type": "auto"})),
            (ToolChoice::None, serde_json::json!({"type": "none"})),
            (ToolChoice::Required, serde_json::json!({"type": "any"})),
            (
                ToolChoice::Function("t".into()),
                serde_json::json!({"type": "tool", "name": "t"}),
            ),
        ];
        for (choice, want) in cases {
            r.tool_choice = choice;
            let v = body(&r, MinimaxReasoningEffort::Low, "switchable");
            assert_eq!(v["tool_choice"], want);
        }
    }

    /// 没有工具时不发 `tool_choice`（`any` 会因无工具被拒）。
    #[test]
    fn 无工具不发tool_choice() {
        let mut r = req(vec![msg(Role::User, "hi")]);
        r.tool_choice = ToolChoice::Required;
        let v = body(&r, MinimaxReasoningEffort::Low, "switchable");
        assert!(v.get("tool_choice").is_none());
    }

    /// M3（switchable）：五档全部等价于开思考，不发 output_config。
    #[test]
    fn m3五档都开思考() {
        let r = req(vec![msg(Role::User, "hi")]);
        for effort in MinimaxReasoningEffort::ALL {
            let v = body(&r, effort, "switchable");
            assert_eq!(v["thinking"]["type"], "adaptive", "{effort:?} 应当开思考");
            assert!(
                v.get("output_config").is_none(),
                "{effort:?} 不应发 output_config"
            );
        }
    }

    /// M3.1-Flash（forced_on）：只发 output_config.effort，**绝不发** thinking。
    ///
    /// 实测该模型发 `thinking.type=disabled` 会被 400 拒绝（本次实测）。
    #[test]
    fn flash只用output_config() {
        let r = req(vec![msg(Role::User, "hi")]);
        let v = body(&r, MinimaxReasoningEffort::Max, "forced_on");
        assert_eq!(v["output_config"]["effort"], "max");
        assert!(v.get("thinking").is_none(), "forced_on 模型不能发 thinking");
    }

    /// Flash 恒发 output_config、恒不发 thinking——即使目录档位表是空的。
    #[test]
    fn flash恒发output_config() {
        let r = req(vec![msg(Role::User, "hi")]);
        let empty = shape("forced_on", &[]);
        for effort in MinimaxReasoningEffort::ALL {
            let v = serde_json::to_value(ChatBody::build(&r, effort, &empty)).unwrap();
            assert_eq!(
                v["output_config"]["effort"],
                effort.effort_value(),
                "{effort:?} 档位表为空时回退标准名"
            );
            assert!(v.get("thinking").is_none(), "forced_on 模型不能发 thinking");
        }
    }

    /// 就近映射：上游档位表完整时，每档命中同名档位。
    #[test]
    fn flash档位同名命中的() {
        let r = req(vec![msg(Role::User, "hi")]);
        let opts = shape("forced_on", FLASH_OPTIONS);
        for (effort, want) in [
            (MinimaxReasoningEffort::Low, "low"),
            (MinimaxReasoningEffort::Medium, "medium"),
            (MinimaxReasoningEffort::High, "high"),
            (MinimaxReasoningEffort::Xhigh, "xhigh"),
            (MinimaxReasoningEffort::Max, "max"),
        ] {
            let v = serde_json::to_value(ChatBody::build(&r, effort, &opts)).unwrap();
            assert_eq!(
                v["output_config"]["effort"], want,
                "{effort:?} 应当发 {want}"
            );
        }
    }

    /// 就近映射：上游档位表缺档时选最近的一档。
    #[test]
    fn flash档位就近映射的() {
        let r = req(vec![msg(Role::User, "hi")]);
        // 只有 low / high：medium 距离 0 和 2 各 1，取较低档 low
        let sparse = shape("forced_on", &["low", "high"]);
        let cases = [
            (MinimaxReasoningEffort::Low, "low"),
            (MinimaxReasoningEffort::Medium, "low"),
            (MinimaxReasoningEffort::High, "high"),
            (MinimaxReasoningEffort::Xhigh, "high"),
            (MinimaxReasoningEffort::Max, "high"),
        ];
        for (effort, want) in cases {
            let v = serde_json::to_value(ChatBody::build(&r, effort, &sparse)).unwrap();
            assert_eq!(
                v["output_config"]["effort"], want,
                "{effort:?} 应当就近到 {want}"
            );
        }
    }

    /// 就近映射：上游档位表里只有 `default`（不可识别档位）时回退到标准名。
    #[test]
    fn flash档位表不可识别时回退的() {
        let r = req(vec![msg(Role::User, "hi")]);
        let only_default = shape("forced_on", &["default"]);
        let v = serde_json::to_value(ChatBody::build(
            &r,
            MinimaxReasoningEffort::Xhigh,
            &only_default,
        ))
        .unwrap();
        assert_eq!(v["output_config"]["effort"], "xhigh");
    }

    /// 就近映射：`default` 不参与距离计算（它是「让上游自己决定」，不是档位）。
    #[test]
    fn flash的default不参与距离计算() {
        let opts: Vec<String> = ["default", "low", "max"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        for effort in MinimaxReasoningEffort::ALL {
            let got = effort.nearest_effort(&opts);
            assert_ne!(got, "default", "{effort:?} 不应映射到 default");
        }
    }

    /// 目录查不到形态时按 switchable 处理（发 thinking 比猜 effort 安全）。
    #[test]
    fn 未知形态按switchable() {
        let r = req(vec![msg(Role::User, "hi")]);
        let v = body(&r, MinimaxReasoningEffort::Low, "");
        assert_eq!(v["thinking"]["type"], "adaptive");
        assert!(v.get("output_config").is_none());
    }

    /// 坏的工具参数字符串按空对象处理，不 panic。
    #[test]
    fn 坏参数给空对象() {
        assert_eq!(parse_arguments("not json"), serde_json::json!({}));
        assert_eq!(parse_arguments(""), serde_json::json!({}));
        assert_eq!(parse_arguments("[1,2]"), serde_json::json!({}));
        assert_eq!(parse_arguments(r#"{"a":1}"#), serde_json::json!({"a":1}));
    }

    /// 空内容的 user 消息被丢弃（Anthropic 不接受空 content 数组）。
    #[test]
    fn 空内容消息被丢弃() {
        let v = body(
            &req(vec![
                Message {
                    role: Role::User,
                    parts: vec![],
                    tool_calls: vec![],
                    tool_call_id: None,
                    tool_name: None,
                },
                msg(Role::User, "hi"),
            ]),
            MinimaxReasoningEffort::Low,
            "switchable",
        );
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    }
}
