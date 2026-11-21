//! 余额记录：`providers/<id>/balance.log`，JSONL，每行一条。
//!
//! 只记录「时间 + 余额」，不记录任何签到结果——这是与 [`crate::core::claimlog`]
//! 的分工：签到结果看 `claim.log`，余额随时间的变化看 `balance.log`。
//!
//! 记录分两种来源：
//! * **定时**：`serve` 里每 `balance_record_interval` 拉一次余额，长期跑出一条
//!   余额曲线；
//! * **签到后**：每次签到（无论成功/跳过/失败）结束后追加一次，捕捉签到前后的
//!   余额跳动。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::paths::provider_dir;

/// 记录文件名（位于 `providers/<id>/` 下，与 `claim.log` 同目录）。
pub const FILE: &str = "balance.log";
/// 保留最近多少条。与 `claim.log` 一致，避免长期运行后文件无限增长。
pub const KEEP: usize = 50;

/// 一条余额记录：时间 + 余额。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// RFC 3339 UTC 时间。
    pub time: String,
    /// 当时的余额。
    pub balance: f64,
}

fn path(root: &Path, provider: &str) -> PathBuf {
    provider_dir(root, provider).join(FILE)
}

/// 读取全部记录（按写入顺序）。文件不存在或无有效行时返回空表。
pub fn load(root: &Path, provider: &str) -> Vec<Entry> {
    let path = path(root, provider);
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(&l).ok())
        .collect()
}

/// 追加一条记录，并将文件裁剪到最近 [`KEEP`] 条。
///
/// 与 [`crate::core::claimlog::append`] 同一套「读全部 + 保留尾部 + 原子替换」，
/// 写盘失败返回 `Err`，由调用方决定是否忽略。
pub fn append(root: &Path, provider: &str, entry: Entry) -> std::io::Result<()> {
    let path = path(root, provider);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    // 单行 JSON；`Entry` 字段都可序列化，理论上不会失败。
    let line = serde_json::to_string(&entry).map_err(std::io::Error::other)?;
    let line = format!("{line}\n");

    // 常态是纯追加：一次 `write` 就够，不做「读全部 → 重写 → fsync → rename」。
    // 只有超过上限（或上次压缩中途被打断留下多行超量）时，才整体压缩一次。
    let over = match count_lines(&path) {
        Ok(n) => n + 1 > KEEP,
        Err(_) => true,
    };
    if !over {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        f.write_all(line.as_bytes())?;
        return Ok(());
    }

    let mut entries = load(root, provider);
    entries.push(entry);
    if entries.len() > KEEP {
        let start = entries.len() - KEEP;
        entries.drain(..start);
    }

    let mut text = String::new();
    for e in &entries {
        if let Ok(l) = serde_json::to_string(e) {
            text.push_str(&l);
            text.push('\n');
        }
    }

    let tmp = path.with_extension("log.tmp");
    let mut f = std::fs::File::create(&tmp)?;
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    crate::core::atomicio::rename(&tmp, &path)?;
    Ok(())
}

/// 统计现有行数（文件不存在时为 0）。用于判断是否需要压缩，不解析 JSON。
fn count_lines(path: &Path) -> std::io::Result<usize> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    Ok(BufReader::new(file).lines().count())
}

/// 最近一条余额增加（`balance > 紧邻前一条余额`）的记录：`(时间, 前余额, 当前余额)`。
///
/// 返回**紧邻前一条**的余额与当前余额，`status` 据此拼 `x+y=z`
/// （`y = 当前 − 前值`，`z = 当前`）。没有任何「余额增加」的记录时返回 `None`。
///
/// `status` 的「最后一次余额增加」用它：只有余额相对上一条真的变多时才展示，
/// 单调递减（纯消耗、无到账）或首条记录时不误报。
pub fn last_increase(root: &Path, provider: &str) -> Option<(String, f64, f64)> {
    let entries = load(root, provider);
    // 从后往前找第一条「比它紧邻的前一条余额高」的记录。
    let idx = (1..entries.len())
        .rev()
        .find(|&i| entries[i].balance > entries[i - 1].balance)?;
    Some((
        entries[idx].time.clone(),
        entries[idx - 1].balance,
        entries[idx].balance,
    ))
}

/// 拉取供应商余额并追加一条记录。
///
/// 定时余额记录与「签到后补记」共用：任何来源都只写时间+余额，不写签到结果。
/// 失败（未提供 / 拉取出错 / 写盘出错）只记日志，不返回错误——余额记录是
/// 辅助信息，不应让调用方（签到循环、定时循环）因它而中断。
pub async fn record(provider: &dyn crate::core::provider::Provider, root: &Path) {
    let id = provider.id();
    match provider.balance().await {
        Ok(Some(v)) => {
            let entry = Entry {
                time: crate::core::clock::now_rfc3339(),
                balance: crate::core::round2(v),
            };
            if let Err(e) = append(root, id, entry) {
                tracing::warn!("[{id}] 写余额记录失败: {e}");
            }
        }
        Ok(None) => tracing::debug!("[{id}] 余额未提供，跳过余额记录"),
        Err(e) => tracing::warn!("[{id}] 获取余额失败，跳过余额记录: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("ofm-balancelog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(time: &str, balance: f64) -> Entry {
        Entry {
            time: time.into(),
            balance,
        }
    }

    #[test]
    fn 追加后可读回() {
        let root = tmp_root("append");
        append(&root, "p", entry("t1", 100.0)).unwrap();
        append(&root, "p", entry("t2", 120.0)).unwrap();
        let e = load(&root, "p");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].balance, 100.0);
        assert_eq!(e[1].time, "t2");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 只保留最近_keep_条() {
        let root = tmp_root("keep");
        for i in 0..(KEEP + 10) {
            append(&root, "p", entry(&format!("t{i}"), i as f64)).unwrap();
        }
        let e = load(&root, "p");
        assert_eq!(e.len(), KEEP);
        assert_eq!(e[0].time, "t10");
        assert_eq!(e[KEEP - 1].time, format!("t{}", KEEP + 9));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 无记录时返回_none() {
        let root = tmp_root("none");
        assert!(last_increase(&root, "p").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 单调递减不算增加() {
        let root = tmp_root("decrease");
        append(&root, "p", entry("t1", 100.0)).unwrap();
        append(&root, "p", entry("t2", 90.0)).unwrap();
        append(&root, "p", entry("t3", 80.0)).unwrap();
        assert!(last_increase(&root, "p").is_none(), "纯消耗不应算增加");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 最近一次增加取时间与前后余额() {
        let root = tmp_root("increase");
        append(&root, "p", entry("t1", 100.0)).unwrap();
        append(&root, "p", entry("t2", 90.0)).unwrap(); // 下降
        append(&root, "p", entry("t3", 150.0)).unwrap(); // 增加
        append(&root, "p", entry("t4", 140.0)).unwrap(); // 下降
        let (time, prev, cur) = last_increase(&root, "p").unwrap();
        assert_eq!(time, "t3");
        assert_eq!(prev, 90.0);
        assert_eq!(cur, 150.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 下降序列里的回升才算增加() {
        // 100 → 60 → 90 → 70：只有 60→90 是增加，末尾 70 是下降，不能误判。
        let root = tmp_root("rebound");
        append(&root, "p", entry("t1", 100.0)).unwrap();
        append(&root, "p", entry("t2", 60.0)).unwrap();
        append(&root, "p", entry("t3", 90.0)).unwrap();
        append(&root, "p", entry("t4", 70.0)).unwrap();
        let (time, prev, cur) = last_increase(&root, "p").unwrap();
        assert_eq!(time, "t3");
        assert_eq!(prev, 60.0);
        assert_eq!(cur, 90.0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 首次记录不算增加() {
        let root = tmp_root("first");
        append(&root, "p", entry("t1", 100.0)).unwrap();
        assert!(
            last_increase(&root, "p").is_none(),
            "最早一条无前序，不算增加"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 相等不算增加() {
        let root = tmp_root("equal");
        append(&root, "p", entry("t1", 100.0)).unwrap();
        append(&root, "p", entry("t2", 100.0)).unwrap();
        assert!(last_increase(&root, "p").is_none(), "余额不变不算增加");
        let _ = std::fs::remove_dir_all(&root);
    }
}
