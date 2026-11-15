//! `ofm-server claim`：串行签到所有供应商。
//!
//! 串行而非并发：签到要读余额前后对比，并发会让对账混在一起。
//!
//! 每次签到（成功/跳过/失败）都写一条 `providers/<id>/claim.log` 记录。

use std::path::Path;

use crate::core::claimlog::{self, Entry};
use crate::core::registry::Registry;

pub async fn run(root: std::path::PathBuf) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root);
    claim_all(&registry, &root).await;
    Ok(())
}

pub async fn claim_all(registry: &Registry, root: &Path) {
    for provider in registry.all() {
        let id = provider.id();
        let (result, balance) = match provider.claim().await {
            Ok(report) => {
                if report.skipped {
                    tracing::info!("[{id}] 签到跳过: {}", report.message);
                    (format!("跳过: {}", report.message), report.balance)
                } else {
                    tracing::info!("[{id}] 签到完成: {}", report.message);
                    ("成功".to_string(), report.balance)
                }
            }
            Err(e) => {
                tracing::warn!("[{id}] 签到失败: {e}");
                (format!("失败: {e}"), None)
            }
        };

        record(root, id, result, balance);
    }
}

/// 追加一条签到记录；写盘失败只记日志，不影响签到本身。
fn record(root: &Path, provider: &str, result: String, balance: Option<f64>) {
    let entry = Entry {
        time: now_rfc3339(),
        result,
        balance,
    };
    if let Err(e) = claimlog::append(root, provider, entry) {
        tracing::warn!("[{provider}] 写签到记录失败: {e}");
    }
}

/// UTC 时间 RFC 3339（`2026-11-15T09:44:19Z`）。
///
/// 用 UTC 而非本地时间：`time` crate 未启用 `local-offset` feature（须改依赖）。
fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| String::from("unknown"))
}
