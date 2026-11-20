//! `ofm-server claim [供应商id]`：签到。
//!
//! 不带参数时串行签到所有供应商；带参数只签该供应商（便于单测与排障）。
//!
//! codearts 的签到走「限时福利 / 每日签到」协议（`/v1/ops/*`，见
//! `providers::codearts::ops`），不是 benefit-service 的福利渠道检查。
//!
//! 串行而非并发：签到要读余额前后对比，并发会让对账混在一起。
//!
//! 每次签到（领取/已领/未开通/失败）都写一条 `providers/<id>/claim.log` 记录。
//!
//! 记录内容用英文（`success` / `skipped: ...` / `failed: ...`），控制台输出用中文。

use std::path::Path;
use std::sync::Arc;

use crate::core::claimlog::{self, Entry};
use crate::core::provider::Provider;
use crate::core::registry::Registry;

pub async fn run(root: std::path::PathBuf, provider_id: Option<&str>) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root, &crate::config::Config::load(&root)?)?;
    match provider_id {
        Some(id) => {
            let provider = registry
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("未知供应商 `{id}`"))?;
            claim_one(&provider, &root).await;
        }
        None => claim_all(&registry, &root).await,
    }
    Ok(())
}

/// 串行签到所有供应商（手动 `claim` 子命令用）。
pub async fn claim_all(registry: &Registry, root: &Path) {
    for provider in registry.all() {
        claim_one(provider, root).await;
    }
}

/// 签到单个供应商。结果均写一条 `claim.log`。
///
/// 自动签到按供应商独立计时，直接调本函数。
pub async fn claim_one(provider: &Arc<dyn Provider>, root: &Path) {
    {
        let id = provider.id();
        let (result_en, balance, delta) = match provider.claim().await {
            Ok(report) => {
                // `skipped` 覆盖两种「本次没领到」：今日已领、渠道未开通；
                // 具体原因在明细里，不要一概记成「成功」。
                if report.skipped() {
                    tracing::info!("[{id}] 签到跳过: {}", report.message_zh());
                } else {
                    tracing::info!("[{id}] 签到完成: {}", report.message_zh());
                }
                (report.result_en(), report.balance, report.credits_delta)
            }
            Err(e) => {
                tracing::warn!("[{id}] 签到失败: {e}");
                // 文件里只留英文原因（错误详情可能含中文，进控制台即可）。
                (
                    format!("failed: {}", crate::core::claim::failure_reason(&e)),
                    None,
                    None,
                )
            }
        };

        record(root, id, result_en, balance, delta);
    }
}

/// 追加一条签到记录；写盘失败只记日志，不影响签到本身。
fn record(root: &Path, provider: &str, result: String, balance: Option<f64>, delta: Option<f64>) {
    let entry = Entry {
        time: now_rfc3339(),
        result,
        balance,
        delta,
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
