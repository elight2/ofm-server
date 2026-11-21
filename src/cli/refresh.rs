//! `ofm-server refresh <供应商id|all>`：刷新供应商登录令牌。
//!
//! 写 `all` 时串行刷新全部供应商；供应商无续期机制时报「此供应商不支持」。
//! 单个供应商成功时凭证已落盘。任一供应商失败（含不支持）时最终以非零退出码
//! 结束（便于脚本判断）；`all` 模式下不会因一家失败而中断其余供应商。

use anyhow::Context;

use crate::cli::ALL;
use crate::core::provider::ProviderError;

pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root)?;

    if provider_id == ALL {
        let mut ok = 0usize;
        let mut failed = Vec::new();
        for provider in registry.all() {
            let id = provider.id().to_string();
            if refresh_one(&id, provider.as_ref()).await {
                ok += 1;
            } else {
                failed.push(id);
            }
        }
        println!("刷新完成: {} 成功，{} 失败", ok, failed.len());
        if failed.is_empty() {
            return Ok(());
        }
        anyhow::bail!("以下供应商刷新失败: {}", failed.join(", "))
    }

    let provider = registry
        .get(provider_id)
        .with_context(|| format!("未知供应商 `{provider_id}`"))?;
    if refresh_one(provider_id, provider.as_ref()).await {
        Ok(())
    } else {
        anyhow::bail!("[{provider_id}] 刷新失败")
    }
}

/// 刷新单个供应商，打印结果；返回是否成功。
///
/// 「不支持」不是错误状态，单独提示但不制造恐慌，但仍计为未成功。
async fn refresh_one(provider_id: &str, provider: &dyn crate::core::provider::Provider) -> bool {
    match provider.refresh().await {
        Ok(msg) => {
            println!("[{provider_id}] {msg}");
            true
        }
        Err(ProviderError::Unsupported(id)) => {
            println!("[{id}] 此供应商不支持刷新令牌");
            false
        }
        Err(e) => {
            println!("[{provider_id}] 刷新失败: {e}");
            false
        }
    }
}
