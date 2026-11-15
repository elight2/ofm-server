//! 供应商无关的核心层：事件模型、供应商 trait、注册表、凭证存储、SSE 解析。

pub mod credential;
pub mod events;
pub mod provider;
pub mod registry;
pub mod sse;
