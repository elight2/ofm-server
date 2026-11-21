//! `ofm-server status <供应商id>`：读取供应商当前信息。
//!
//! 输出格式（固定顺序）：
//!
//! ```text
//! 状态: 正常(2个模型)        或  状态: 未接入
//! 登录凭证到期时间: 2026-11-18 02:19:34
//! 余额: 8437.46
//! 最后一次余额增加: 2026-11-17 14:40:57 (10680.00 + 54.00 = 10734.00)
//! 模型列表:
//! minimax/MiniMax-M3.1-Flash
//! ```
//!
//! 时间一律 **UTC**（`fmt_ms` 与 `fmt_rfc3339` 同一口径，记录存的也是 UTC）。
//! 余额与增量均按金额口径显示，固定小数点后 2 位。
//! 「最后一次余额增加」取自 `balance.log`：余额相对紧邻前一条变多时展示
//! `(x + y = z)`（`x` 为前一条余额、`y` 为增量、`z` 为当前余额）；无增加记录
//! （纯消耗或尚无记录）显示 `无记录`。

use anyhow::Context;

use crate::core::balancelog;
use crate::core::fmt_amount;
use crate::core::provider::rate_suffix;

pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root, &crate::config::Config::load(&root)?)?;
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
    match provider.credential_expire_at() {
        Some(ms) => println!("登录凭证到期时间: {}", crate::core::fmt_ms(ms)),
        None => println!("登录凭证到期时间: 未知"),
    }

    // 余额。
    match provider.balance().await {
        Ok(Some(v)) => println!("余额: {}", fmt_amount(v)),
        Ok(None) => println!("余额: 未提供"),
        Err(e) => println!("余额: 获取失败（{e}）"),
    }

    // 最后一次余额增加。时间与上面到期时间同格式。
    // 来自 balance.log，仅当余额相对紧邻前一条变多时展示。
    match balancelog::last_increase(&root, provider_id) {
        Some((time, prev, cur)) => {
            println!(
                "最后一次余额增加: {} ({} + {} = {})",
                crate::core::fmt_rfc3339(&time),
                fmt_amount(prev),
                fmt_amount(cur - prev),
                fmt_amount(cur)
            );
        }
        None => println!("最后一次余额增加: 无记录"),
    }

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
