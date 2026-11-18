//! Loomy HTTP 客户端与共享常量。

use reqwest::Client;

/// 构建 HTTP 客户端。
pub fn client() -> Result<Client, reqwest::Error> {
    Ok(Client::new())
}

/// 客户端版本。
pub const VERSION: &str = "0.9.39";

/// HTTP 错误。
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("{0}")]
    Transport(#[from] reqwest::Error),
}
