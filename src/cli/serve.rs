//! `ofm-server`：启动服务 + 自动签到 / 自动刷新令牌。
//!
//! 两项定时任务都是**每个供应商各一个独立计时循环**，间隔取全局配置：
//! `claim_interval`（小时）与 `refresh_interval`（小时）。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::config::Config;
use crate::core::provider::ProviderError;
use crate::core::registry::Registry;
use crate::providers;
use crate::server::{self, AppState};

pub async fn run(root: PathBuf) -> anyhow::Result<()> {
    let config = Config::load(&root)?;
    // 间隔先校验：非法配置在启动时就报错，不进入「服务已起、定时任务没跑」的半状态。
    let claim_interval = config.claim_interval()?;
    let refresh_interval = config.refresh_interval()?;

    let registry = Arc::new(providers::build_registry(&root, &config)?);

    spawn_claim_loops(registry.clone(), claim_interval, root.clone());
    spawn_refresh_loops(registry.clone(), refresh_interval);

    let state = AppState { registry, root };
    server::run(state, &config).await?;
    Ok(())
}

/// 间隔转成日志里的「Nh」。
fn hours(interval: Duration) -> u64 {
    interval.as_secs() / 3600
}

/// 给每个供应商起一个**独立计时**的签到循环，间隔统一取全局 `claim_interval`。
///
/// 独立计时意味着各供应商互不阻塞：某个账号签到慢或失败，不影响其它供应商的
/// 节奏。间隔非法时 [`Config::claim_interval`] 已在启动前报错，故这里不再返回错误。
fn spawn_claim_loops(registry: Arc<Registry>, interval: Duration, root: PathBuf) {
    tracing::info!("自动签到间隔 {}h", hours(interval));
    for provider in registry.all() {
        let provider = provider.clone();
        let root = root.clone();
        tokio::spawn(async move {
            // `interval` 首次 `tick()` 立即就绪 ⇒ 启动时先跑一轮。
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                crate::cli::claim::claim_one(&provider, &root).await;
            }
        });
    }
}

/// 给每个供应商起一个**独立计时**的令牌刷新循环，间隔统一取全局
/// `refresh_interval`。
///
/// 首轮**不**立即刷新：签到需要在启动时先跑一轮（今日可能还没签），但令牌
/// 刚登录时通常还有效，启动即强制轮换没有必要。故先等满一个间隔再刷新。
///
/// 不支持自动刷新的供应商（[`ProviderError::Unsupported`]，如 loomy）打一条
/// debug 后结束自己的循环，不影响其它供应商。
fn spawn_refresh_loops(registry: Arc<Registry>, interval: Duration) {
    tracing::info!("登录令牌刷新间隔 {}h", hours(interval));
    for provider in registry.all() {
        let id = provider.id();
        let provider = provider.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // 跳过首次立即就绪的 tick，等满一个间隔再刷新。
            ticker.tick().await;
            loop {
                ticker.tick().await;
                match provider.refresh().await {
                    Ok(msg) => tracing::info!("[{id}] {msg}"),
                    Err(ProviderError::Unsupported(_)) => {
                        tracing::debug!("[{id}] 此供应商不支持刷新令牌，停止自动刷新");
                        return;
                    }
                    // 刷新失败只记日志：凭证可能只是暂时过期，下一轮再试。
                    Err(e) => tracing::warn!("[{id}] 自动刷新令牌失败: {e}"),
                }
            }
        });
    }
}
