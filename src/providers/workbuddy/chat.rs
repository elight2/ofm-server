//! 对话请求体构造 + 思考档位映射。
//!
//! 上游是**标准 OpenAI Chat Completions**（协议见 `provider-info/workbuddy.md`
//! §3）：`messages`/`tools`/`tool_choice` 逐项生效，SSE 是纯 `data:` 帧。
//! 故请求体基本照搬 [`ChatRequest`]，只额外处理两处：
//!
//! 1. **思考档位**：`reasoning_effort` 必须落在该模型支持的集合内，
//!    否则上游报错。映射规则见 §5 与 [`effort_for_model`]。
//! 2. **`max_tokens` 字段名**：对 `o1/o3/o4/gpt-4.1/gpt-5` 前缀模型，
//!    上游要的是 `max_completion_tokens`（harness `MaxTokensFieldRule`）。

use serde_json::Value;

use crate::core::provider::{ChatRequest, Message, Part, Role};

use super::models::{CatalogModel, ReasoningSpec};

/// 思考档位（与上游 `reasoning_effort` 取值一致）。
///
/// 上游请求层的合法集合是 `minimal/low/medium/high/xhigh/max`（**不含** `off`/`none`）；
/// `none` 是「显式关闭思考」时实际发送的值（§5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Effort {
    /// 关闭思考（上游取值 `none`）。
    ///
    /// 只有 `canDisableThinking=true` 的模型才允许；否则该档位会被丢弃
    /// （不发 `reasoning_effort`），见 §5。
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl Effort {
    /// 上游取值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => EFFORT_NONE,
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }

    /// 解析上游/下游的档位字符串。`none` 视为「关闭思考」。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "minimal" => Some(Self::Minimal),
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            "xhigh" => Some(Self::Xhigh),
            "max" => Some(Self::Max),
            "none" => Some(Self::None),
            _ => None,
        }
    }

    /// 档位序号（用于「向下取最近受支持档位」）。
    ///
    /// [`Effort::None`] 没有序号——它是「关闭」而非强度档位。
    fn rank(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Minimal => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
            Self::Xhigh => 4,
            Self::Max => 5,
        }
    }

    /// 按序号还原档位。
    #[allow(dead_code)]
    fn from_rank(r: u8) -> Self {
        match r {
            0 => Self::Minimal,
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            4 => Self::Xhigh,
            _ => Self::Max,
        }
    }
}

/// 「关闭思考」在上游的实际取值。
pub const EFFORT_NONE: &str = "none";

/// 一个模型实际支持的思考档位集合（§5 的推导结果）。
///
/// `list` 为 `None` 表示**目录没有声明**支持集合（模型只声明了
/// `effort`/`defaultEffort`），此时按官方 harness 的
/// `LegacyXhighFallbackRule` 处理：`xhigh`/`max` 收敛为 `high`，其余原样。
#[derive(Debug, Clone, Default)]
pub struct SupportedEfforts {
    /// 受支持的档位（升序）；`None` = 未声明。
    pub list: Option<Vec<Effort>>,
    /// 是否允许关闭思考（`canDisableThinking`）。
    pub can_disable: bool,
}

impl SupportedEfforts {
    /// 从目录里的 `reasoning` 声明推导。
    ///
    /// 规则（§5）：
    /// * 有 `supportedEfforts` → 直接用它（并按 `canDisableThinking` 决定
    ///   能否关闭思考）；
    /// * 无 `supportedEfforts` 但有 `effort`/`defaultEffort` → 支持集合
    ///   **未知**（`list = None`），按 legacy 规则处理；
    /// * 两者都无 → 该模型**不支持 reasoning**（返回 `None`）。
    pub fn from_spec(spec: Option<&ReasoningSpec>) -> Option<Self> {
        let spec = spec?;
        if !spec.supported_efforts.is_empty() {
            let mut list: Vec<Effort> = spec
                .supported_efforts
                .iter()
                .filter_map(|s| Effort::parse(s))
                .collect();
            list.sort();
            list.dedup();
            if list.is_empty() {
                return None;
            }
            return Some(Self {
                list: Some(list),
                can_disable: spec.can_disable_thinking.unwrap_or(false),
            });
        }
        // 只声明了 `effort`/`defaultEffort`：支持集合未知。
        if spec.effective_default_effort().is_some() {
            return Some(Self {
                list: None,
                can_disable: spec.can_disable_thinking.unwrap_or(false),
            });
        }
        None
    }

    /// 该模型声明的默认档位（`defaultEffort` / `effort`）。
    ///
    /// 目录里没有该字段时回退到支持集合的最高档。
    #[allow(dead_code)]
    pub fn default_effort(&self, spec: Option<&ReasoningSpec>) -> Option<Effort> {
        spec.and_then(|s| s.effective_default_effort())
            .and_then(Effort::parse)
            .or_else(|| self.list.as_ref().and_then(|l| l.last().copied()))
    }

    /// 把请求的档位落到受支持集合内。
    ///
    /// `clampEffortToSupported` 语义（§5）：
    /// * 目标档位受支持 → 原样；
    /// * 不受支持 → **向下**取最近的受支持档位；一个都没有 → 取集合最低档；
    /// * 集合为空 → 返回 `None`（调用方应**删除** `reasoning_effort`）。
    ///
    /// `xhigh`/`max` 在模型没有 `thinkingLevelMap` 时会被官方收敛为 `high`
    /// （`LegacyXhighFallbackRule`）；这里等价地实现为：若集合里最高只到 `high`，
    /// 则 `xhigh`/`max` 落到 `high`。
    pub fn clamp(&self, want: Effort) -> Option<Effort> {
        // 「关闭思考」不受支持集合约束，但模型必须允许关闭（§5）。
        if want == Effort::None {
            return if self.can_disable {
                Some(Effort::None)
            } else {
                None
            };
        }
        let Some(list) = &self.list else {
            // 支持集合未知：官方 `LegacyXhighFallbackRule` 把
            // `xhigh`/`max` 收敛为 `high`，其余原样发送。
            return Some(match want {
                Effort::Xhigh | Effort::Max => Effort::High,
                other => other,
            });
        };
        if list.is_empty() {
            return None;
        }
        if list.contains(&want) {
            return Some(want);
        }
        // 向下取：找最大且 <= want 的档位。
        let lower: Vec<Effort> = list
            .iter()
            .copied()
            .filter(|e| e.rank() <= want.rank())
            .collect();
        if let Some(e) = lower.last().copied() {
            return Some(e);
        }
        // 请求档位比集合最低还低 → 取最低档。
        list.first().copied()
    }
}

/// 由 [`ChatRequest`] 构造上游请求体。
///
/// 恒发 `stream: true`：上游 SSE 分帧正常，非流式由下游聚合，没必要两套代码。
pub fn build_body(
    req: &ChatRequest,
    effort: Option<Effort>,
    supported: Option<&SupportedEfforts>,
) -> Value {
    let mut body = serde_json::json!({
        "model": req.model,
        "messages": req.messages.iter().map(convert_message).collect::<Vec<_>>(),
        "stream": true,
    });

    // 思考档位：先按模型支持集合钳制，再决定是否发字段。
    //
    // `supported` 为 `None` 有两种含义：
    // * 该模型**不支持** reasoning → 不发 `reasoning_effort`；
    // * 目录没拉到（网络失败）→ 保守起见也不发，避免上游因非法档位报错。
    // 两种情况都不发，行为一致。
    if let Some(e) = effort
        && let Some(s) = supported
        && let Some(e) = s.clamp(e)
    {
        body["reasoning_effort"] = Value::String(e.as_str().to_string());
    }

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

/// 该模型的上游档位集合（从目录条目推导）。
pub fn supported_of(model: &CatalogModel) -> Option<SupportedEfforts> {
    if !model.supports_reasoning {
        return None;
    }
    SupportedEfforts::from_spec(model.reasoning.as_ref())
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

    fn spec(efforts: &[&str], can_disable: bool) -> ReasoningSpec {
        ReasoningSpec {
            default_effort: efforts.first().map(|s| s.to_string()),
            effort: None,
            supported_efforts: efforts.iter().map(|s| s.to_string()).collect(),
            can_disable_thinking: Some(can_disable),
        }
    }

    #[test]
    fn 纯文本走字符串() {
        let b = build_body(
            &req(vec![msg(Role::User, vec![Part::Text("你好".into())])]),
            None,
            None,
        );
        assert_eq!(b["messages"][0]["content"], "你好");
        assert_eq!(b["messages"][0]["role"], "user");
        assert_eq!(b["stream"], true);
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
            None,
            None,
        );
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
        let b = build_body(&r, None, None);
        assert_eq!(b["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(b["tool_choice"], "required");

        r.tool_choice = crate::core::provider::ToolChoice::Auto;
        let b = build_body(&r, None, None);
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

        let b = build_body(&req(vec![assistant, tool]), None, None);
        assert_eq!(b["messages"][0]["tool_calls"][0]["id"], "call_1");
        assert_eq!(b["messages"][1]["tool_call_id"], "call_1");
        assert_eq!(b["messages"][1]["content"], "晴");
    }

    /// 有 `supportedEfforts` 时按其钳制。
    #[test]
    fn 档位按支持集合钳制() {
        let s = SupportedEfforts::from_spec(Some(&spec(&["low", "high", "max"], true))).unwrap();
        assert_eq!(s.clamp(Effort::High), Some(Effort::High));
        // medium 不受支持 → 向下取 low。
        assert_eq!(s.clamp(Effort::Medium), Some(Effort::Low));
        // minimal 比最低档还低 → 取最低档 low。
        assert_eq!(s.clamp(Effort::Minimal), Some(Effort::Low));
        // xhigh 不受支持 → 向下取 high。
        assert_eq!(s.clamp(Effort::Xhigh), Some(Effort::High));
        assert_eq!(s.clamp(Effort::Max), Some(Effort::Max));
    }

    /// 只声明 `effort`（deepseek 系）→ 支持集合未知，xhigh/max 收敛为 high。
    #[test]
    fn 仅声明effort时按legacy收敛() {
        let s = ReasoningSpec {
            default_effort: None,
            effort: Some("high".into()),
            supported_efforts: vec![],
            can_disable_thinking: None,
        };
        let s = SupportedEfforts::from_spec(Some(&s)).unwrap();
        assert!(s.list.is_none());
        assert_eq!(s.clamp(Effort::Max), Some(Effort::High));
        assert_eq!(s.clamp(Effort::Xhigh), Some(Effort::High));
        assert_eq!(s.clamp(Effort::Medium), Some(Effort::Medium));
        assert_eq!(s.clamp(Effort::Low), Some(Effort::Low));
    }

    /// 两者都无 → 不支持 reasoning。
    #[test]
    fn 无声明视为不支持() {
        let s = ReasoningSpec {
            default_effort: None,
            effort: None,
            supported_efforts: vec![],
            can_disable_thinking: None,
        };
        assert!(SupportedEfforts::from_spec(Some(&s)).is_none());
        assert!(SupportedEfforts::from_spec(None).is_none());
    }

    /// `xhigh`/`max` 在只支持到 `high` 的模型上收敛为 `high`（§5）。
    #[test]
    fn 高档收敛到high() {
        let s = SupportedEfforts::from_spec(Some(&spec(&["high"], false))).unwrap();
        assert_eq!(s.clamp(Effort::Xhigh), Some(Effort::High));
        assert_eq!(s.clamp(Effort::Max), Some(Effort::High));
    }

    /// 请求体里的 `reasoning_effort` 取钳制后的值。
    #[test]
    fn 请求体写钳制后的档位() {
        let s = SupportedEfforts::from_spec(Some(&spec(&["low", "high", "max"], true))).unwrap();
        let b = build_body(
            &req(vec![msg(Role::User, vec![Part::Text("x".into())])]),
            Some(Effort::Medium),
            Some(&s),
        );
        assert_eq!(b["reasoning_effort"], "low");
    }

    /// 不支持 reasoning 的模型 → 不发 `reasoning_effort`。
    #[test]
    fn 无支持集合时删字段() {
        let b = build_body(
            &req(vec![msg(Role::User, vec![Part::Text("x".into())])]),
            Some(Effort::High),
            None,
        );
        assert!(b.get("reasoning_effort").is_none());
    }

    /// `supportsReasoning=false` 的模型即使带 reasoning 声明也不认。
    #[test]
    fn 不支持思考的模型忽略声明() {
        let m = CatalogModel {
            id: "glm-4.6".into(),
            name: None,
            credits: None,
            max_input_tokens: Some(100_000),
            max_output_tokens: Some(4096),
            supports_images: false,
            supports_tool_call: true,
            supports_reasoning: false,
            reasoning: Some(spec(&["high"], true)),
        };
        assert!(supported_of(&m).is_none());
    }
}
