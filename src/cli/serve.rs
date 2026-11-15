//! `ofm-server`：启动服务 + 每小时自动签到。

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::core::registry::Registry;
use crate::providers;
use crate::server::{self, AppState};

pub async fn run(root: std::path::PathBuf) -> anyhow::Result<()> {
    let config = Config::load(&root)?;
    let registry = Arc::new(providers::build_registry(&root));

    spawn_claim_loop(registry.clone(), config.claim_interval_secs);

    let state = AppState { registry };
    server::run(state, &config).await?;
    Ok(())
}

/// 启动时先跑一轮，之后按间隔重复。
fn spawn_claim_loop(registry: Arc<Registry>, interval_secs: u64) {
    tokio::spawn(async move {
        // 启动时立刻跑一轮；之后每 `interval_secs` 一轮。
        // （`interval` 的首次 `tick()` 立即就绪，故先 tick 再签到，避免启动时连跑两轮。）
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs.max(60)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            crate::cli::claim::claim_all(&registry).await;
        }
    });
}
