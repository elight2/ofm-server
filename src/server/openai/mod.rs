//! OpenAI 兼容层：线格式类型、请求转换、响应/流编码。

pub mod request;
pub mod translate;
pub mod types;

use axum::response::IntoResponse;

/// 生成 OpenAI 风格错误响应（非流式或流开始前的错误）。
pub fn error_response(
    status: axum::http::StatusCode,
    message: impl Into<String>,
    kind: &str,
    code: Option<String>,
) -> axum::response::Response {
    let body = types::ErrorBody {
        error: types::ErrorDetail {
            message: message.into(),
            kind: kind.to_string(),
            param: None,
            code,
        },
    };
    let mut resp = axum::Json(body).into_response();
    *resp.status_mut() = status;
    resp
}
