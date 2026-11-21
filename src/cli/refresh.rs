//! `ofm-server refresh <供应商id>`：刷新供应商登录令牌。
//!
//! 只刷新指定供应商；成功时凭证已落盘。供应商无续期机制时报
//! 「此供应商不支持」，并以非零退出码结束（便于脚本判断）。

use anyhow::Context;

use crate::core::provider::ProviderError;

pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    let registry = crate::providers::build_registry(&root)?;
    let provider = registry
        .get(provider_id)
        .with_context(|| format!("未知供应商 `{provider_id}`"))?;

    match provider.refresh().await {
        Ok(msg) => {
            println!("[{provider_id}] {msg}");
            Ok(())
        }
        // 「不支持」不是错误状态，单独提示但不制造恐慌。
        Err(ProviderError::Unsupported(id)) => {
            println!("[{id}] 此供应商不支持刷新令牌");
            // 用 Err 让退出码非零，便于脚本区分。
            anyhow::bail!("此供应商不支持刷新令牌")
        }
        Err(e) => {
            println!("[{provider_id}] 刷新失败: {e}");
            Err(anyhow::anyhow!("刷新失败: {e}"))
        }
    }
}
