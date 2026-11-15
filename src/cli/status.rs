//! `ofm-server status <供应商id>`：读取供应商当前信息。
//!
//! 目前输出：余额、最近一次成功签到的时间与增量。

use anyhow::Context;

use crate::core::claimlog;

pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root);
    let provider = registry
        .get(provider_id)
        .with_context(|| format!("未知供应商 `{provider_id}`"))?;

    match provider.balance().await? {
        Some(v) => println!("余额: {v}"),
        None => println!("余额: 上游未提供"),
    }

    match claimlog::last_success(&root, provider_id) {
        Some((time, Some(delta))) => {
            println!("最近一次成功签到: {time}（增量 {delta:+.2}）");
        }
        Some((time, None)) => {
            println!("最近一次成功签到: {time}（增量未知）");
        }
        None => println!("最近一次成功签到: 无记录"),
    }
    Ok(())
}
