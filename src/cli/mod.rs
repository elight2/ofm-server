//! CLI 子命令：`login` / `claim` / `status` / `refresh` / `clear` / `config` / `serve`（默认）。
//!
//! 运行目录固定为当前工作目录（配置与 `providers/` 都在其中）。

pub mod claim;
pub mod clear;
pub mod login;
pub mod refresh;
pub mod serve;
pub mod status;

use clap::{Parser, Subcommand};

/// 「全部供应商」的占位参数：`claim` / `status` / `refresh` 传入该值时遍历所有供应商。
pub const ALL: &str = "all";

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
    /// 签到（不带参数或写 `all` 签所有供应商，带 `provider` 只签该供应商）
    Claim {
        /// 供应商 id（如 codearts），写 `all` 签所有供应商
        provider: Option<String>,
    },
    /// 获取供应商当前信息（状态、凭证到期、余额、签到记录、模型列表），写 `all` 输出全部供应商的简要信息
    Status {
        /// 供应商 id（如 trae），写 `all` 输出全部供应商的简要信息
        provider: String,
    },
    /// 打印默认配置（内嵌的 config.example.toml），可直接重定向为配置
    ///
    /// 用法：`ofm-server config > config.toml`
    Config,
    /// 刷新供应商登录令牌（写 `all` 刷新全部）
    Refresh {
        /// 供应商 id（如 trae），写 `all` 刷新全部
        provider: String,
    },
    /// 清空运行目录下的日志或登录凭证
    Clear {
        /// 清空目标
        #[command(subcommand)]
        target: ClearTarget,
    },
}

/// `clear` 的清空目标。
#[derive(Debug, Subcommand)]
pub enum ClearTarget {
    /// 清空所有 log 文件（`claim.log` / `balance.log`）
    Log,
    /// 清空所有凭证文件（`credential.toml`，需要全部重新登录）
    Credential,
}
