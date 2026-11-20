//! 运行目录下的固定路径约定。
//!
//! 单独成模块而非放在 `config`：`core` 层的凭证、签到记录只需要拼路径，
//! 不该为了一个 `join` 依赖上层配置模块。`config` 复用它。

use std::path::{Path, PathBuf};

/// 供应商数据目录名（相对运行目录）。
pub const PROVIDERS_DIR: &str = "providers";
/// 凭证文件名（位于 `providers/<id>/` 下）。
pub const CREDENTIAL_FILE: &str = "credential.toml";
/// 自定义系统提示词目录。`<root>/prompts/<供应商id>.md` 追加到系统提示词之后。
pub const PROMPTS_DIR: &str = "prompts";

/// 某供应商的数据目录 `<root>/providers/<id>`。
pub fn provider_dir(root: &Path, provider: &str) -> PathBuf {
    root.join(PROVIDERS_DIR).join(provider)
}

/// 某供应商的凭证文件 `<root>/providers/<id>/credential.toml`。
pub fn credential_path(root: &Path, provider: &str) -> PathBuf {
    provider_dir(root, provider).join(CREDENTIAL_FILE)
}

/// 某供应商的自定义系统提示词 `<root>/prompts/<id>.md`。
///
/// 文件不存在时回退到内嵌默认（`default-prompts/<id>.md`），无需预先创建。
pub fn prompt_path(root: &Path, provider: &str) -> PathBuf {
    root.join(PROMPTS_DIR).join(format!("{provider}.md"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 路径形状固定() {
        let root = Path::new("/run");
        assert_eq!(provider_dir(root, "trae"), Path::new("/run/providers/trae"));
        assert_eq!(
            credential_path(root, "trae"),
            Path::new("/run/providers/trae/credential.toml")
        );
        assert_eq!(prompt_path(root, "trae"), Path::new("/run/prompts/trae.md"));
    }
}
