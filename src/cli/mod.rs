//! CLI 子命令：`login` / `claim` / `serve`（默认）。
//!
//! 运行目录固定为当前工作目录（配置与 `providers/` 都在其中）。

pub mod claim;
pub mod login;
pub mod serve;
pub mod status;

use clap::{Parser, Subcommand};

/// 运行目录：当前工作目录。
pub fn root() -> std::path::PathBuf {
    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
}

#[derive(Debug, Parser)]
#[command(
    name = "ofm-server",
    version,
    about = "聚合多家 LLM 免费额度，对外提供 OpenAI 兼容 API"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// 启动服务（默认）
    Serve,
    /// 获取供应商登录凭证并保存
    Login {
        /// 供应商 id（如 trae）
        provider: String,
    },
    /// 签到所有供应商
    Claim,
    /// 获取供应商当前信息（目前只输出余额）
    Status {
        /// 供应商 id（如 trae）
        provider: String,
    },
    /// 打印默认配置（内嵌的 config.example.toml），可直接重定向为配置
    ///
    /// 用法：`ofm-server config > config.toml`
    Config,
}
