//! CLI 子命令：`login` / `claim` / `serve`（默认）。

pub mod claim;
pub mod login;
pub mod serve;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ofm-server",
    version,
    about = "聚合多家 LLM 免费额度，对外提供 OpenAI 兼容 API"
)]
pub struct Cli {
    /// 运行目录（配置文件与 providers/ 所在目录），默认当前目录。
    #[arg(long, global = true, default_value = ".", value_name = "DIR")]
    pub root: PathBuf,

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
}
