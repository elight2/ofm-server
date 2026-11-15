//! `ofm-server status <供应商id>`：读取供应商当前信息。
//!
//! 目前只输出余额（README 规划的信息集后续补齐）。

use anyhow::Context;

pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root);
    let provider = registry
        .get(provider_id)
        .with_context(|| format!("未知供应商 `{provider_id}`"))?;

    match provider.balance().await? {
        Some(v) => println!("余额: {v}"),
        None => println!("余额: 上游未提供"),
    }
    Ok(())
}
