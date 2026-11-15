//! `ofm-server status <供应商id>`：读取供应商当前信息。
//!
//! 输出：接入状态（真实拉取模型目录验证）、余额、最近一次成功签到、凭证到期
//! 时间、模型列表（一行一个，与 `/v1/models` 的 `id` 格式一致）。

use anyhow::Context;

use crate::core::claimlog;
use crate::core::provider::rate_suffix;

pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root);
    let provider = registry
        .get(provider_id)
        .with_context(|| format!("未知供应商 `{provider_id}`"))?;

    // 接入状态：真实拉一次模型目录，成功即可用。
    let models = provider.models_checked().await;
    match &models {
        Ok(list) => println!("接入状态: 正常（模型目录可用，{} 个模型）", list.len()),
        Err(e) => println!("接入状态: 异常（{e}）"),
    }

    // 凭证到期时间。
    match provider.credential_expire_at() {
        Some(ms) => println!("凭证到期时间: {}", fmt_ms(ms)),
        None => println!("凭证到期时间: 未知"),
    }

    // 余额。
    match provider.balance().await {
        Ok(Some(v)) => println!("余额: {v}"),
        Ok(None) => println!("余额: 上游未提供"),
        Err(e) => println!("余额: 获取失败（{e}）"),
    }

    // 最近一次成功签到。
    match claimlog::last_success(&root, provider_id) {
        Some((time, Some(delta))) => {
            println!("最近一次成功签到: {time}（增量 {delta:+.2}）");
        }
        Some((time, None)) => {
            println!("最近一次成功签到: {time}（增量未知）");
        }
        None => println!("最近一次成功签到: 无记录"),
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

/// 毫秒时间戳 → `YYYY-MM-DD HH:MM:SS`（UTC）。
fn fmt_ms(ms: i64) -> String {
    let secs = ms / 1000;
    match time::OffsetDateTime::from_unix_timestamp(secs) {
        Ok(dt) => {
            let (y, mo, d) = dt.to_calendar_date();
            let (h, mi, s) = (dt.hour(), dt.minute(), dt.second());
            format!("{y:04}-{:02}-{d:02} {h:02}:{mi:02}:{s:02}", mo as u8)
        }
        Err(_) => ms.to_string(),
    }
}
