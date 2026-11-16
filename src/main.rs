//! ofm-server：聚合多家 LLM 免费额度，对外提供 OpenAI Chat Completions 兼容 API。

mod cli;
mod config;
mod core;
mod providers;
mod server;

use clap::Parser;

use cli::{Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let cli = Cli::parse();
    let root = cli::root();

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => cli::serve::run(root).await,
        Command::Login { provider } => cli::login::run(root, &provider).await,
        Command::Claim { provider } => cli::claim::run(root, provider.as_deref()).await,
        Command::Status { provider } => cli::status::run(root, &provider).await,
        Command::Refresh { provider } => cli::refresh::run(root, &provider).await,
        Command::Config => {
            // 原样输出内嵌文本，保留注释。不写盘，由 shell 重定向决定去向。
            print!("{}", config::DEFAULT_CONFIG_STR);
            Ok(())
        }
    }
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}
