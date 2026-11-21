//! HTTP 服务：`/v1/chat/completions`、`/v1/models`、`/health`。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::StreamExt;

pub mod auth;
pub mod openai;
pub mod prompt;

use std::path::PathBuf;

use crate::config::Config;
use crate::core::registry::Registry;
use crate::core::throttle::Throttle;
use crate::server::openai::{request as reqconv, translate, types};

#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<Registry>,
    /// 运行目录，用于定位 `<root>/prompts/<供应商id>.md`。
    pub root: PathBuf,
    /// 每供应商一份对话请求节流器，见 [`crate::core::throttle`]。
    pub throttles: Arc<BTreeMap<&'static str, Throttle>>,
    /// 下游 api key 白名单（空 = 不校验），见 [`auth`]。
    pub api_keys: auth::ApiKeys,
}

impl AppState {
    /// 按配置为每个供应商建一份节流器（同一个 `min_interval`）。
    ///
    /// 各供应商独立计时：trae 的间隔不会拖慢 codearts。
    pub fn new(
        registry: Arc<Registry>,
        root: PathBuf,
        min_interval: Duration,
        api_keys: auth::ApiKeys,
    ) -> Self {
        let throttles = registry
            .all()
            .map(|p| (p.id(), Throttle::new(min_interval)))
            .collect();
        Self {
            registry,
            root,
            throttles: Arc::new(throttles),
            api_keys,
        }
    }
}

pub fn router(state: AppState) -> Router {
    // `/health` 不校验 key（探活用），其余出口接口受 `api_keys` 保护。
    let guarded = Router::new()
        .route("/v1/models", get(list_models))
        .route("/v1/chat/completions", post(chat_completions))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::require_key,
        ));

    Router::new()
        .route("/health", get(health))
        .merge(guarded)
        .with_state(state)
}

async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({"status": "ok"}))
}

async fn list_models(State(app): State<AppState>) -> Response {
    let created = crate::core::clock::now_secs() as u64;
    let mut data = Vec::new();
    for (provider_id, models) in app.registry.list_models().await {
        for m in models {
            // 格式见 README：`供应商id/模型名 (x倍率)`；无倍率时省略后缀。
            let suffix = crate::core::provider::rate_suffix(m.rate);
            data.push(types::ModelCard {
                id: format!("{provider_id}/{}{suffix}", m.model),
                object: "model",
                created,
                owned_by: provider_id.to_string(),
            });
        }
    }
    axum::Json(types::ModelList {
        object: "list",
        data,
    })
    .into_response()
}

async fn chat_completions(State(app): State<AppState>, body: String) -> Response {
    let req: types::ChatRequest = match serde_json::from_str(&body) {
        Ok(r) => r,
        Err(e) => {
            return openai::error_response(
                StatusCode::BAD_REQUEST,
                format!("请求体解析失败: {e}"),
                "invalid_request_error",
                None,
            );
        }
    };

    // 路由 `供应商id/模型名`。
    let (provider_id, model) = match Registry::route(&req.model) {
        Some(r) => (r.provider.to_string(), r.model.to_string()),
        None => {
            return openai::error_response(
                StatusCode::BAD_REQUEST,
                format!("模型名须为 `供应商id/模型名`，收到 `{}`", req.model),
                "invalid_request_error",
                Some("model_not_found".into()),
            );
        }
    };
    let Some(provider) = app.registry.get(&provider_id) else {
        return openai::error_response(
            StatusCode::BAD_REQUEST,
            format!("未知供应商 `{provider_id}`"),
            "invalid_request_error",
            Some("model_not_found".into()),
        );
    };

    let stream = req.stream;
    let include_usage = req.stream_options.map(|o| o.include_usage).unwrap_or(false);
    let requested_model = req.model.clone();

    let mut preq = match reqconv::to_provider_request(model, req) {
        Ok(p) => p,
        Err(e) => {
            return openai::error_response(
                StatusCode::BAD_REQUEST,
                e.to_string(),
                "invalid_request_error",
                None,
            );
        }
    };

    // 自定义系统提示词：追加在客户端自带 system 之后（README「配置信息」）。
    // 读文件失败/不存在都只是不注入，绝不阻断请求。
    if let Some(prompt) = prompt::load(&app.root, &provider_id) {
        prompt::apply(&mut preq, &prompt);
    }

    let ctx = translate::Ctx {
        id: gen_id(),
        created: crate::core::clock::now_secs() as u64,
        model: requested_model,
        include_usage,
    };

    // 节流：同一供应商两次请求的发出时刻至少相隔配置的间隔。
    // 只延迟「发出」这一个动作，拿到许可后请求即发出并各自跑完，不构成串行。
    if let Some(throttle) = app.throttles.get(provider_id.as_str()) {
        throttle.acquire().await;
    }

    // 拿流；供应商层整体失败（未进入事件流）时直接映射 HTTP 错误。
    let events = match provider.chat(preq).await {
        Ok(s) => s,
        Err(e) => {
            let status = StatusCode::from_u16(translate::provider_err_status(&e))
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let body = translate::provider_err_body(e);
            return (status, axum::Json(body)).into_response();
        }
    };

    // peek 首帧：错误且在内容之前 → 仍可返回 HTTP 错误。
    let (first, rest) = {
        let mut s = events;
        let first = s.next().await;
        (first, s)
    };

    if let Some(ev) = &first
        && let Some((status, body)) = translate::classify_first(ev)
    {
        let code = status_from_u16(status);
        return (code, axum::Json(body)).into_response();
    }

    if !stream {
        return match translate::aggregate(ctx, first, rest).await {
            Ok(completion) => axum::Json(completion).into_response(),
            Err(body) => {
                let status = translate::status_for_error_type(&body.error.kind);
                (status, axum::Json(body)).into_response()
            }
        };
    }

    // 流式：SSE 编码。`[DONE]` 统一在流末尾发一次（含错误帧之后）。
    let out = translate::stream(ctx, first, rest);
    let frames = out.map(|item| -> Result<bytes::Bytes, std::io::Error> {
        let frame = match item {
            translate::Out::Chunk(chunk) => {
                let data = serde_json::to_string(&chunk).unwrap_or_else(|_| "{}".into());
                format!("data: {data}\n\n")
            }
            translate::Out::Abort(body) => {
                let data = serde_json::to_string(&body).unwrap_or_else(|_| "{}".into());
                format!("data: {data}\n\n")
            }
        };
        Ok(bytes::Bytes::from(frame))
    });

    let tail = futures_util::stream::once(async {
        Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"data: [DONE]\n\n"))
    });
    let body = axum::body::Body::from_stream(frames.chain(tail));

    Response::builder()
        .status(StatusCode::OK)
        .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
        .header(axum::http::header::CACHE_CONTROL, "no-cache")
        .header(axum::http::header::CONNECTION, "keep-alive")
        .body(body)
        .expect("SSE 响应构造失败")
}

fn status_from_u16(code: u16) -> StatusCode {
    StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_GATEWAY)
}

/// 请求 id：`chatcmpl-` + 12 位十六进制随机串。
fn gen_id() -> String {
    let mut buf = [0u8; 6];
    let _ = getrandom::fill(&mut buf);
    let hex = crate::core::text::hex_lower(&buf);
    format!("chatcmpl-{hex}")
}

/// 启动服务并阻塞。自动签到任务由 `cli::serve` 负责编排。
pub async fn run(state: AppState, config: &Config) -> std::io::Result<()> {
    let addr = config.socket_addr();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("ofm-server 监听 http://{addr}");
    axum::serve(listener, router(state))
        .with_graceful_shutdown(shutdown_signal())
        .await
}

async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut sig) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            sig.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("收到退出信号，正在关闭");
}
