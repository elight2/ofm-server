//! `ofm-server claim`：串行签到所有供应商。
//!
//! 串行而非并发：签到要读余额前后对比，并发会让对账混在一起。

use crate::core::registry::Registry;

pub async fn run(root: std::path::PathBuf) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root);
    claim_all(&registry).await;
    Ok(())
}

pub async fn claim_all(registry: &Registry) {
    for provider in registry.all() {
        let id = provider.id();
        match provider.claim().await {
            Ok(report) => {
                if report.skipped {
                    tracing::info!("[{id}] 签到跳过: {}", report.message);
                } else {
                    tracing::info!("[{id}] 签到完成: {}", report.message);
                }
            }
            Err(e) => tracing::warn!("[{id}] 签到失败: {e}"),
        }
    }
}
