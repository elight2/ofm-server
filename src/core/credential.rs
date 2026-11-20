//! 通用凭证读写：`providers/<id>/credential.toml`。
//!
//! 供应商自己定义结构体，这里只负责路径、权限（0600）与原子替换。

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::core::paths::credential_path;
use crate::core::provider::ProviderError;

#[derive(Debug, thiserror::Error)]
pub enum CredentialError {
    /// 文件不存在时统一由调用方转成 `Ok(None)`，故本变体暂无构造点；
    /// 保留以便区分「不存在」与「读不了」。
    #[allow(dead_code)]
    #[error("凭证不存在: {path}")]
    Missing { path: PathBuf },
    #[error("读取凭证失败 {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("解析凭证失败 {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
    #[error("序列化凭证失败: {0}")]
    Serialize(toml::ser::Error),
    #[error("写入凭证失败 {path}: {source}")]
    Write {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// 取凭证；文件不存在时给出「请先登录」的统一提示。
///
/// 五家供应商原本各写一份完全相同的 `credential()`，提示文案也一字不差，
/// 故上提到这里。
pub fn require<T: DeserializeOwned>(root: &Path, provider: &str) -> Result<T, ProviderError> {
    match load(root, provider) {
        Ok(Some(c)) => Ok(c),
        Ok(None) => Err(ProviderError::NotLoggedIn(format!(
            "{}（请先运行 `ofm-server login {provider}`）",
            credential_path(root, provider).display()
        ))),
        Err(e) => Err(ProviderError::Other(e.to_string())),
    }
}

/// 双检锁续期：过期才续，续期前后各读一次（并发下可能已被别人换过）。
///
/// 五家的「读凭证 → 判过期 → 加锁 → 重读 → 判过期 → 换令牌 → 落盘」结构同构，
/// 只有「怎么判过期」「怎么换」不同，故由调用方给两个闭包。
/// `renew` 拿到的是**刚重读的那份**凭证（按值传入，可直接取 refresh_token）。
pub async fn renew_if_expired<T, E, R, Fut>(
    root: &Path,
    provider: &str,
    refresh_lock: &tokio::sync::Mutex<()>,
    expired: E,
    renew: R,
) -> Result<T, ProviderError>
where
    T: DeserializeOwned + Clone,
    E: Fn(&T) -> bool,
    R: FnOnce(T) -> Fut,
    Fut: std::future::Future<Output = Result<T, ProviderError>>,
{
    let first = require::<T>(root, provider)?;
    if !expired(&first) {
        return Ok(first);
    }

    let _guard = refresh_lock.lock().await;
    // 加锁后重读：可能已被其它任务刷新。
    let again = require::<T>(root, provider)?;
    if !expired(&again) {
        return Ok(again);
    }

    tracing::info!("[{provider}] 凭证已过期，正在续期");
    let new = renew(again).await?;
    tracing::info!("[{provider}] 续期成功");
    Ok(new)
}

/// 读取并反序列化凭证。`Ok(None)` 表示文件不存在。
pub fn load<T: DeserializeOwned>(
    root: &Path,
    provider: &str,
) -> Result<Option<T>, CredentialError> {
    let path = credential_path(root, provider);
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(CredentialError::Read { path, source: e }),
    };
    toml::from_str(&text)
        .map(Some)
        .map_err(|e| CredentialError::Parse { path, source: e })
}

/// 原子写入凭证（先写临时文件再 rename），并设为 0600。
pub fn store<T: Serialize>(root: &Path, provider: &str, value: &T) -> Result<(), CredentialError> {
    let path = credential_path(root, provider);
    let dir = path.parent().expect("credential 路径必有父目录");
    std::fs::create_dir_all(dir).map_err(|e| CredentialError::Write {
        path: dir.to_path_buf(),
        source: e,
    })?;

    let text = toml::to_string_pretty(value).map_err(CredentialError::Serialize)?;
    let tmp = path.with_extension("toml.tmp");
    write_private(&tmp, text.as_bytes())?;
    crate::core::atomicio::rename(&tmp, &path).map_err(|e| CredentialError::Write {
        path: path.clone(),
        source: e,
    })?;
    Ok(())
}

fn write_private(path: &Path, data: &[u8]) -> Result<(), CredentialError> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| CredentialError::Write {
                path: path.to_path_buf(),
                source: e,
            })?;
        f.write_all(data).map_err(|e| CredentialError::Write {
            path: path.to_path_buf(),
            source: e,
        })?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, data).map_err(|e| CredentialError::Write {
            path: path.to_path_buf(),
            source: e,
        })
    }
}
