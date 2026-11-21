//! `ofm-server status <供应商id|all>`：读取供应商当前信息。
//!
//! 写 `all` 时输出全部供应商的**简要信息**，每个供应商一段（以 `===== id =====`
//! 开头，段间空一行），不列模型，也不拉模型目录：
//!
//! ```text
//! ===== trae =====
//! 登录状态: 已登录
//! 登录凭证到期时间: 2026-11-18 02:19:34
//! 余额: 8437.46
//! 最后一次余额增加: 2026-11-17 14:40:57 (10680.00 + 54.00 = 10734.00)
//! 最后成功签到: 2026-11-18 02:19:34
//! ```
//!
//! 单个供应商的输出格式（固定顺序）：
//!
//! ```text
//! 状态: 正常(2个模型)        或  状态: 未接入
//! 登录凭证到期时间: 2026-11-18 02:19:34
//! 余额: 8437.46
//! 最后一次余额增加: 2026-11-17 14:40:57 (10680.00 + 54.00 = 10734.00)
//! 最后成功签到: 2026-11-18 02:19:34
//! 模型列表:
//! minimax/MiniMax-M3.1-Flash
//! ```
//!
//! 时间一律 **UTC**（`fmt_ms` 与 `fmt_rfc3339` 同一口径，记录存的也是 UTC）。
//! 余额与增量均按金额口径显示，固定小数点后 2 位。
//! 「最后一次余额增加」取自 `balance.log`：余额相对紧邻前一条变多时展示
//! `(x + y = z)`（`x` 为前一条余额、`y` 为增量、`z` 为当前余额）；无增加记录
//! （纯消耗或尚无记录）显示 `无记录`。
//! 「最后成功签到」取自 `claim.log`，**只认 `success`**：`skipped`（今日已领等）
//! 不算成功，否则该行永远显示今天；无成功记录显示 `无记录`。
//! 两个日志都只保留最近 50 条，故最多只能回溯这 50 次。

use anyhow::Context;

use crate::cli::ALL;
use crate::core::balancelog;
use crate::core::claimlog;
use crate::core::fmt_amount;
use crate::core::provider::{Provider, rate_suffix};

pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root)?;

    if provider_id == ALL {
        let mut first = true;
        for provider in registry.all() {
            if !first {
                println!();
            }
            first = false;
            print_brief(&root, provider.as_ref()).await;
        }
        return Ok(());
    }

    let provider = registry
        .get(provider_id)
        .with_context(|| format!("未知供应商 `{provider_id}`"))?;

    // 状态：真实拉一次模型目录，成功即可用。
    let models = provider.models_checked().await;
    match &models {
        Ok(list) => println!("状态: 正常({}个模型)", list.len()),
        Err(_) => println!("状态: 未接入"),
    }

    // 登录凭证到期时间。
    println!("登录凭证到期时间: {}", expire_text(provider.as_ref()));

    // 余额。
    println!("余额: {}", balance_text(provider.as_ref()).await);

    // 最后一次余额增加、最后成功签到。时间与上面到期时间同格式。
    println!("{}", last_increase_text(&root, provider_id));
    println!("{}", last_claim_success_text(&root, provider_id));

    // 模型列表：一行一个，格式与 `/v1/models` 的 id 一致。
    match models {
        Ok(list) if !list.is_empty() => {
            println!("模型列表:");
            for m in list {
                println!("{provider_id}/{}{}", m.model, rate_suffix(m.rate));
            }
        }
        Ok(_) => println!("模型列表: 无"),
        Err(e) => println!("模型列表: 获取失败（{e}）"),
    }

    Ok(())
}

/// `status all` 的单供应商简要信息：标题行 + 登录状态/凭证到期/余额 + 最后两条。
///
/// 不拉模型目录（`all` 仅要求简要信息；串行拉六家目录太慢）。
/// 登录状态看凭证文件是否存在（不联网），不依赖 `credential_expire_at`
/// ——部分供应商没有到期字段，`None` 同时表示「未知」而非「未登录」。
async fn print_brief(root: &std::path::Path, provider: &dyn Provider) {
    let id = provider.id();
    println!("===== {id} =====");
    let logged_in = crate::core::paths::credential_path(root, id).is_file();
    println!("登录状态: {}", if logged_in { "已登录" } else { "未登录" });
    println!("登录凭证到期时间: {}", expire_text(provider));
    println!("余额: {}", balance_text(provider).await);
    println!("{}", last_increase_text(root, id));
    println!("{}", last_claim_success_text(root, id));
}

/// 凭证到期时间文本（`未知` 兜底）。
fn expire_text(provider: &dyn Provider) -> String {
    match provider.credential_expire_at() {
        Some(ms) => crate::core::fmt_ms(ms),
        None => "未知".to_string(),
    }
}

/// 余额文本：`未提供` / `获取失败（…）` / 金额。
async fn balance_text(provider: &dyn Provider) -> String {
    match provider.balance().await {
        Ok(Some(v)) => fmt_amount(v),
        Ok(None) => "未提供".to_string(),
        Err(e) => format!("获取失败（{e}）"),
    }
}

/// 「最后一次余额增加」整行文本。
///
/// 来自 `balance.log`，仅当余额相对紧邻前一条变多时展示
/// `(x + y = z)`（`x` 为前一条余额、`y` 为增量、`z` 为当前余额）；
/// 无增加记录（纯消耗或尚无记录）显示 `无记录`。
fn last_increase_text(root: &std::path::Path, provider_id: &str) -> String {
    match balancelog::last_increase(root, provider_id) {
        Some((time, prev, cur)) => format!(
            "最后一次余额增加: {} ({} + {} = {})",
            crate::core::fmt_rfc3339(&time),
            fmt_amount(prev),
            fmt_amount(cur - prev),
            fmt_amount(cur)
        ),
        None => "最后一次余额增加: 无记录".to_string(),
    }
}

/// 「最后成功签到」整行文本。
///
/// 来自 `claim.log`，只认 `success`；`skipped`（今日已领等）与 `failed` 都不算。
fn last_claim_success_text(root: &std::path::Path, provider_id: &str) -> String {
    match claimlog::last_success(root, provider_id) {
        Some(time) => format!("最后成功签到: {}", crate::core::fmt_rfc3339(&time)),
        None => "最后成功签到: 无记录".to_string(),
    }
}
