//! 内部事件流 → OpenAI 线格式。
//!
//! 负责两件事：
//! 1. 流式：逐事件转 chunk；
//! 2. 非流式：聚合全部事件为单个 `chat.completion`。
//!
//! 错误处理策略（`provider-info/trae.md` §7.2）：错误出现在任何内容帧之前时，
//! 调用方仍可改 HTTP 状态码；一旦已产出内容，只能以流内 `error` 帧收尾。

use std::collections::BTreeMap;
use std::pin::Pin;

use futures_util::{Stream, StreamExt};

use crate::core::events::{self as ev, ErrorKind, Event, FinishReason};
use crate::core::provider::ProviderError;

use super::types::{self as wire, Usage as WireUsage};

/// 流式输出项。
pub enum Out {
    /// 一个 SSE chunk。
    Chunk(Box<wire::Chunk>),
    /// 流内错误（仅在已发内容后出现）。
    Abort(wire::ErrorBody),
}

pub struct Ctx {
    pub id: String,
    pub created: u64,
    pub model: String,
    pub include_usage: bool,
}

impl Ctx {
    fn chunk(&self, delta: wire::Delta, finish_reason: Option<String>) -> wire::Chunk {
        wire::Chunk {
            id: self.id.clone(),
            object: "chat.completion.chunk",
            created: self.created,
            model: self.model.clone(),
            choices: vec![wire::ChunkChoice {
                index: 0,
                delta,
                finish_reason,
            }],
            usage: None,
        }
    }

    fn usage_chunk(&self, usage: WireUsage) -> wire::Chunk {
        wire::Chunk {
            id: self.id.clone(),
            object: "chat.completion.chunk",
            created: self.created,
            model: self.model.clone(),
            choices: vec![],
            usage: Some(usage),
        }
    }
}

/// usage 是否含有有效信息。
///
/// 上游常在每个流式帧里带 `usage: null`（解析后全 0），这种帧不该发给下游：
/// 先发会把后面真正的 usage 帧盖掉（客户端只认最后一帧）。
fn has_usage(u: &ev::Usage) -> bool {
    u.prompt_tokens > 0 || u.completion_tokens > 0 || u.total_tokens > 0
}

pub fn to_wire_usage(u: &ev::Usage) -> WireUsage {
    WireUsage {
        prompt_tokens: u.prompt_tokens,
        completion_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
        prompt_tokens_details: u.cached_tokens.map(|c| wire::PromptTokensDetails {
            cached_tokens: Some(c),
        }),
        completion_tokens_details: u.reasoning_tokens.map(|r| wire::CompletionTokensDetails {
            reasoning_tokens: Some(r),
        }),
    }
}

fn error_body(e: &ev::UpstreamError) -> wire::ErrorBody {
    wire::ErrorBody {
        error: wire::ErrorDetail {
            message: e.message.clone(),
            kind: match e.kind {
                ErrorKind::Authentication => "authentication_error",
                ErrorKind::InvalidRequest => "invalid_request_error",
                ErrorKind::RateLimit => "rate_limit_error",
                ErrorKind::Api => "api_error",
            }
            .to_string(),
            param: None,
            code: e.code.clone(),
        },
    }
}

/// 首帧为错误时，返回 HTTP 状态码与错误体（此时尚未发出任何内容）。
pub fn classify_first(event: &Event) -> Option<(u16, wire::ErrorBody)> {
    match event {
        Event::Error(e) => {
            let status = match e.kind {
                ErrorKind::Authentication => 401,
                ErrorKind::InvalidRequest => 400,
                ErrorKind::RateLimit => 429,
                ErrorKind::Api => 502,
            };
            Some((status, error_body(e)))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------- 流式

struct State {
    ctx: Ctx,
    role_sent: bool,
    saw_tool_call: bool,
    done: bool,
    /// 已发过 usage 帧。
    ///
    /// 部分上游（如 codearts）把 usage 帧放在 `finish_reason` **之后**，
    /// 而 `drive` 在见到终止事件后就停止读流，故 usage 帧会在 `Event::Finish`
    /// 之后才到达 —— 此时不能整体丢弃，只放行 usage（与 OpenAI 的
    /// 「usage 帧在 finish 帧之后」一致）。
    usage_sent: bool,
}

impl State {
    /// 把一个上游事件翻译成 0..n 个输出项。
    fn map(&mut self, event: Event) -> Vec<Out> {
        // 终止事件之后只放行 usage（见 `usage_sent` 注释）。
        if self.done && !matches!(event, Event::Usage(_)) {
            return Vec::new();
        }
        let mut out: Vec<Out> = Vec::new();

        // 首个内容帧前先补 role delta（与 OpenAI 行为一致）。
        if matches!(
            event,
            Event::Text(_) | Event::Reasoning(_) | Event::ToolCall(_)
        ) && !self.role_sent
        {
            self.role_sent = true;
            out.push(Out::Chunk(Box::new(self.ctx.chunk(
                wire::Delta {
                    role: Some("assistant"),
                    ..Default::default()
                },
                None,
            ))));
        }

        match event {
            Event::Text(t) if !t.is_empty() => {
                out.push(Out::Chunk(Box::new(self.ctx.chunk(
                    wire::Delta {
                        content: Some(t),
                        ..Default::default()
                    },
                    None,
                ))));
            }
            Event::Reasoning(r) if !r.is_empty() => {
                out.push(Out::Chunk(Box::new(self.ctx.chunk(
                    wire::Delta {
                        reasoning_content: Some(r),
                        ..Default::default()
                    },
                    None,
                ))));
            }
            Event::ToolCall(tc) => {
                self.saw_tool_call = true;
                let args = tc.arguments.unwrap_or_default();
                let out_id = tc.id.clone().filter(|s| !s.is_empty());
                let out_name = tc.name.clone().filter(|s| !s.is_empty());
                // 无新增内容（无 id / name / 空参数）时省略该帧，避免噪音。
                if out_id.is_some() || out_name.is_some() || !args.is_empty() {
                    out.push(Out::Chunk(Box::new(self.ctx.chunk(
                        wire::Delta {
                            tool_calls: Some(vec![wire::ToolCallDeltaOut {
                                index: tc.index,
                                id: out_id.clone(),
                                kind: out_id.as_ref().map(|_| "function"),
                                function: Some(wire::FunctionCallDeltaOut {
                                    name: out_name,
                                    arguments: if args.is_empty() { None } else { Some(args) },
                                }),
                            }]),
                            ..Default::default()
                        },
                        None,
                    ))));
                }
            }
            Event::Usage(u) => {
                // 只发最后一次（有值的）usage：中间帧常带全 0 的 usage
                // （上游每帧都带 `usage: null`），先发会把真值盖掉。
                if self.ctx.include_usage && !self.usage_sent && has_usage(&u) {
                    self.usage_sent = true;
                    out.push(Out::Chunk(Box::new(
                        self.ctx.usage_chunk(to_wire_usage(&u)),
                    )));
                }
            }
            Event::Finish(reason) => {
                self.done = true;
                let finish = if self.saw_tool_call {
                    FinishReason::ToolCalls
                } else {
                    reason
                };
                out.push(Out::Chunk(Box::new(self.ctx.chunk(
                    wire::Delta::default(),
                    Some(finish.as_str().to_string()),
                ))));
            }
            Event::Error(e) => {
                self.done = true;
                out.push(Out::Abort(error_body(&e)));
            }
            // 空增量（Text("") / Reasoning("")）：忽略。
            _ => {}
        }
        out
    }
}

/// 流式转换。`first` 是提前 peek 的首个事件（已确认不是错误帧）。
pub fn stream(
    ctx: Ctx,
    first: Option<Event>,
    rest: ev::EventStream,
) -> Pin<Box<dyn Stream<Item = Out> + Send>> {
    let state = State {
        ctx,
        role_sent: false,
        saw_tool_call: false,
        done: false,
        usage_sent: false,
    };
    let all = rejoin(first, rest);

    Box::pin(futures_util::stream::unfold(
        (all, state, Vec::<Out>::new()),
        |(mut input, mut state, mut buf)| async move {
            loop {
                if !buf.is_empty() {
                    let item = buf.remove(0);
                    return Some((item, (input, state, buf)));
                }
                match input.next().await {
                    Some(event) => buf = state.map(event),
                    None => return None,
                }
            }
        },
    ))
}

/// 把「首个事件」和「余下流」重新拼成一个流。
pub fn rejoin(first: Option<Event>, rest: ev::EventStream) -> ev::EventStream {
    let head = futures_util::stream::iter(first);
    Box::pin(head.chain(rest))
}

// ---------------------------------------------------------------- 非流式

pub async fn aggregate(
    ctx: Ctx,
    first: Option<Event>,
    rest: ev::EventStream,
) -> Result<wire::Completion, wire::ErrorBody> {
    let mut content = String::new();
    let mut reasoning = String::new();
    let mut tool_calls: BTreeMap<u32, wire::ToolCallOut> = BTreeMap::new();
    let mut usage: Option<WireUsage> = None;
    let mut finish = FinishReason::Stop;
    let mut saw_tool = false;

    let all = rejoin(first, rest);
    futures_util::pin_mut!(all);
    while let Some(event) = all.next().await {
        match event {
            Event::Text(t) => content.push_str(&t),
            Event::Reasoning(r) => reasoning.push_str(&r),
            Event::ToolCall(tc) => {
                saw_tool = true;
                let entry = tool_calls
                    .entry(tc.index)
                    .or_insert_with(|| wire::ToolCallOut {
                        index: tc.index,
                        id: String::new(),
                        kind: "function",
                        function: wire::FunctionCallOut {
                            name: String::new(),
                            arguments: String::new(),
                        },
                    });
                if let Some(id) = tc.id.filter(|s| !s.is_empty()) {
                    entry.id = id;
                }
                if let Some(name) = tc.name.filter(|s| !s.is_empty()) {
                    entry.function.name = name;
                }
                if let Some(args) = tc.arguments {
                    entry.function.arguments.push_str(&args);
                }
            }
            // 与流式一致：全 0 的 usage（上游每帧都带 `usage: null`）不覆盖已有真值。
            Event::Usage(u) => {
                if has_usage(&u) || usage.is_none() {
                    usage = Some(to_wire_usage(&u));
                }
            }
            Event::Finish(f) => finish = f,
            Event::Error(e) => return Err(error_body(&e)),
        }
    }

    if saw_tool {
        finish = FinishReason::ToolCalls;
    }

    let tool_calls: Vec<_> = tool_calls.into_values().collect();
    Ok(wire::Completion {
        id: ctx.id,
        object: "chat.completion",
        created: ctx.created,
        model: ctx.model,
        choices: vec![wire::CompletionChoice {
            index: 0,
            message: wire::CompletionMessage {
                role: "assistant",
                content: if content.is_empty() && !tool_calls.is_empty() {
                    None
                } else {
                    Some(content)
                },
                reasoning_content: if reasoning.is_empty() {
                    None
                } else {
                    Some(reasoning)
                },
                tool_calls: if tool_calls.is_empty() {
                    None
                } else {
                    Some(tool_calls)
                },
            },
            finish_reason: Some(finish.as_str().to_string()),
        }],
        usage,
    })
}

/// 供应商整体失败（未进入事件流）时构造错误体。
pub fn provider_err_body(e: ProviderError) -> wire::ErrorBody {
    let kind = match &e {
        ProviderError::NotLoggedIn(_) | ProviderError::Unauthorized(_) => "authentication_error",
        ProviderError::RateLimit(_) => "rate_limit_error",
        _ => "api_error",
    };
    wire::ErrorBody {
        error: wire::ErrorDetail {
            message: e.to_string(),
            kind: kind.to_string(),
            param: None,
            code: None,
        },
    }
}

/// 供应商整体失败对应的 HTTP 状态码。
pub fn provider_err_status(e: &ProviderError) -> u16 {
    match e {
        ProviderError::NotLoggedIn(_) | ProviderError::Unauthorized(_) => 401,
        ProviderError::RateLimit(_) => 429,
        _ => 502,
    }
}

/// 由 `error.type` 反查 HTTP 状态码（用于非流式聚合失败）。
pub fn status_for_error_type(kind: &str) -> axum::http::StatusCode {
    use axum::http::StatusCode;
    match kind {
        "authentication_error" => StatusCode::UNAUTHORIZED,
        "invalid_request_error" => StatusCode::BAD_REQUEST,
        "rate_limit_error" => StatusCode::TOO_MANY_REQUESTS,
        _ => StatusCode::BAD_GATEWAY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::Usage;

    fn ctx(include_usage: bool) -> Ctx {
        Ctx {
            id: "chatcmpl-test".into(),
            created: 0,
            model: "p/m".into(),
            include_usage,
        }
    }

    fn usage(prompt: u64, completion: u64) -> Usage {
        Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
            total_tokens: prompt + completion,
            cached_tokens: None,
            reasoning_tokens: None,
        }
    }

    /// 终止事件之后到达的 usage 帧必须仍能下发（codearts 把 usage 放在 finish 之后）。
    #[test]
    fn finish_之后的_usage_仍下发() {
        let mut st = State {
            ctx: ctx(true),
            role_sent: false,
            saw_tool_call: false,
            done: false,
            usage_sent: false,
        };
        // 先收到 Finish，再收到 usage。
        let out = st.map(Event::Finish(ev::FinishReason::Stop));
        assert_eq!(out.len(), 1, "finish 帧应照常下发");
        assert!(st.done);

        let out = st.map(Event::Usage(usage(118, 31)));
        assert_eq!(out.len(), 1, "finish 之后的 usage 不能被丢弃");
    }

    /// 中间帧的全 0 usage 不下发（否则会盖掉后面的真值）。
    #[test]
    fn 全零_usage_不下发() {
        let mut st = State {
            ctx: ctx(true),
            role_sent: false,
            saw_tool_call: false,
            done: false,
            usage_sent: false,
        };
        assert!(st.map(Event::Usage(usage(0, 0))).is_empty());
        // 真值仍可下发。
        assert_eq!(st.map(Event::Usage(usage(118, 31))).len(), 1);
    }

    /// 只发一次 usage（多个真值帧时取第一个）。
    #[test]
    fn usage_只发一次() {
        let mut st = State {
            ctx: ctx(true),
            role_sent: false,
            saw_tool_call: false,
            done: false,
            usage_sent: false,
        };
        assert_eq!(st.map(Event::Usage(usage(118, 31))).len(), 1);
        assert!(st.map(Event::Usage(usage(118, 99))).is_empty());
    }

    /// 未开启 include_usage 时不下发 usage。
    #[test]
    fn 不含_usage_时不下发() {
        let mut st = State {
            ctx: ctx(false),
            role_sent: false,
            saw_tool_call: false,
            done: false,
            usage_sent: false,
        };
        assert!(st.map(Event::Usage(usage(118, 31))).is_empty());
    }

    /// 供应商层限流 → 下游 429 + `rate_limit_error`。
    #[test]
    fn 限流错误映射为_429() {
        let e = ProviderError::RateLimit("登录已失效（疑似限流）".into());
        assert_eq!(provider_err_status(&e), 429);
        let body = provider_err_body(e);
        assert_eq!(body.error.kind, "rate_limit_error");
        assert!(body.error.message.contains("疑似限流"));
    }
}
