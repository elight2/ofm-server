//! `ofm-server`：启动服务 + 按供应商独立计时自动签到。

use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::core::registry::Registry;
use crate::providers;
use crate::server::{self, AppState};

pub async fn run(root: std::path::PathBuf) -> anyhow::Result<()> {
    let config = Config::load(&root)?;
    let registry = Arc::new(providers::build_registry(&root, &config)?);

    spawn_claim_loops(registry.clone(), &config, root.clone())?;

    let state = AppState { registry, root };
    server::run(state, &config).await?;
    Ok(())
}

/// 给每个供应商起一个**独立计时**的签到循环。
///
/// 间隔取自 `[claim_intervals]`，未配置则默认 6h。独立计时意味着各供应商
/// 互不阻塞：某个账号签到慢或失败，不影响其它供应商的节奏。
///
/// 间隔非法时**直接返回错误**终止启动，不静默纠正——否则用户写了 `10`
/// 却被偷偷改成 60 秒，永远不知道自己配错了。
fn spawn_claim_loops(
    registry: Arc<Registry>,
    config: &Config,
    root: std::path::PathBuf,
) -> anyhow::Result<()> {
    for provider in registry.all() {
        let id = provider.id();
        let secs = config.claim_interval_for(id)?;
        tracing::info!("[{id}] 签到间隔 {}s", secs);

        let provider = provider.clone();
        let root = root.clone();
        tokio::spawn(async move {
            // `interval` 首次 `tick()` 立即就绪 ⇒ 启动时先跑一轮。
            let mut ticker = tokio::time::interval(Duration::from_secs(secs));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                crate::cli::claim::claim_one(&provider, &root).await;
            }
        });
    }
    Ok(())
}
