//! ofm-server：聚合多家 LLM 免费额度，对外提供 OpenAI Chat Completions 兼容 API。

mod cli;
mod config;
mod core;
mod login_util;
mod providers;
mod server;
mod tls;

use clap::Parser;

use cli::{ClearTarget, Cli, Command};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tls::init();
    init_tracing();

    let cli = Cli::parse();
    let root = cli::root();

    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => cli::serve::run(root).await,
        Command::Login { provider } => cli::login::run(root, &provider).await,
        Command::Claim { provider } => cli::claim::run(root, provider.as_deref()).await,
        Command::Status { provider } => cli::status::run(root, &provider).await,
        Command::Refresh { provider } => cli::refresh::run(root, &provider).await,
        Command::Clear { target } => match target {
            ClearTarget::Log => cli::clear::logs(root),
            ClearTarget::Credential => cli::clear::credentials(root),
        },
        Command::Config => {
            // 原样输出内嵌文本，保留注释。不写盘，由 shell 重定向决定去向。
            //
            // 思考强度已从所有供应商移除，故剔除遗留键 `reasoning_effort`
            // （示例文件是历史产物，不在本次可改范围内）。
            print!("{}", config::template_str());
            Ok(())
        }
    }
}

fn init_tracing() {
    use std::io::IsTerminal;
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    // 只有终端才上色：重定向到文件、或由 systemd 接入 journald 时不上色。
    //
    // `tracing-subscriber` 的 `Layer::default()` 把 `is_ansi` 硬编码为 `true`
    // （只看编译期 `ansi` feature 与 `NO_COLOR`，**不看 stdout 是不是 tty**），
    // 不管输出到哪都写 `\x1b[2m` 之类转义。journald 会把这些控制字符原样存进
    // 日志正文（`journalctl -o cat` / `-o json` 可见），日志收集器拿到的是脏数据。
    let ansi = std::io::stdout().is_terminal()
        && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());

    fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_ansi(ansi)
        .init();
}
