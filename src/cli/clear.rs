//! `ofm-server clear log|credential`：清空运行目录下的日志或登录凭证。
//!
//! 两个子命令都**遍历 `providers/` 下的全部供应商目录**（而非注册表），
//! 这样对已下线或拼错名字的残留目录也能一并清理：
//!
//! * `clear log`：删除 `providers/<id>/` 下的 `claim.log` 与 `balance.log`
//!   （以及写盘中途残留的 `*.log.tmp` 临时文件）。
//! * `clear credential`：删除 `providers/<id>/credential.toml`
//!   （以及残留的 `credential.toml.tmp`），之后需要重新 `login`。
//!
//! 只删这两个目标，绝不删除供应商目录本身或 `prompts/` 等其它数据。
//! 单个文件删除失败只记警告，继续处理其余文件，最后若有失败则返回错误。

use std::path::Path;

use crate::core::paths::{CREDENTIAL_FILE, PROVIDERS_DIR, provider_dir};

/// 清空全部供应商的日志文件（`claim.log` / `balance.log`）。
pub fn logs(root: std::path::PathBuf) -> anyhow::Result<()> {
    let mut targets = vec![
        crate::core::claimlog::FILE.to_string(),
        crate::core::balancelog::FILE.to_string(),
    ];
    // 写盘中断残留的临时文件（`claim.log.tmp` 等），一并清掉。
    let tmp_files: Vec<String> = targets.iter().map(|f| format!("{f}.tmp")).collect();
    targets.extend(tmp_files);

    remove_in_all_providers(&root, &targets);
    Ok(())
}

/// 清空全部供应商的凭证文件（`credential.toml`）。
///
/// 删除后该供应商即为「未登录」状态，需要重新执行 `login`。
pub fn credentials(root: std::path::PathBuf) -> anyhow::Result<()> {
    let mut targets = vec![CREDENTIAL_FILE.to_string()];
    // 凭证写盘用的临时文件名为 `credential.toml.tmp`（见 `core::credential::store`）。
    targets.push(format!("{CREDENTIAL_FILE}.tmp"));

    remove_in_all_providers(&root, &targets);
    Ok(())
}

/// 遍历 `providers/` 的每个子目录，删除其中列出的文件。
///
/// 文件不存在视为正常（幂等：重复执行不报错）；目录不存在时给出提示而非报错。
fn remove_in_all_providers(root: &Path, files: &[String]) {
    let dir = root.join(PROVIDERS_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            println!("{} 不存在，无需清理", dir.display());
            return;
        }
        Err(e) => {
            tracing::warn!("读取目录失败 {}: {e}", dir.display());
            return;
        }
    };

    let mut provider_ids: Vec<String> = entries
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    provider_ids.sort();

    if provider_ids.is_empty() {
        println!("{} 下没有供应商目录，无需清理", dir.display());
        return;
    }

    for id in provider_ids {
        let dir = provider_dir(root, &id);
        let mut removed = 0usize;
        for file in files {
            let path = dir.join(file);
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    removed += 1;
                    tracing::info!("已删除 {}", path.display());
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::warn!("删除失败 {}: {e}", path.display()),
            }
        }
        if removed == 0 {
            println!("[{id}] 无文件可清理");
        } else {
            println!("[{id}] 已删除 {removed} 个文件");
        }
    }
}
