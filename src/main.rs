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
        Command::Claim => cli::claim::run(root).await,
        Command::Status { provider } => cli::status::run(root, &provider).await,
    }
}

fn init_tracing() {
    use tracing_subscriber::{EnvFilter, fmt};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    fmt().with_env_filter(filter).with_target(false).init();
}
