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
        // 最小 60s，防止配置写 0 导致忙等。
        let mut ticker = tokio::time::interval(Duration::from_secs(interval_secs.max(60)));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            crate::cli::claim::claim_all(&registry).await;
            ticker.tick().await;
        }
    });
}
