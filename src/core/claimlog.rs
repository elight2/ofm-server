//! 签到记录：`providers/<id>/claim.log`，JSONL，每行一条。
//!
//! 只保留最近 [`KEEP`] 条，追加超出后裁剪。记录字段有时间、结果、当时余额、增量。
//!
//! **记录内容一律英文**：`result` 字段写 `success` / `skipped: already claimed` /
//! `failed` 等英文串，便于 grep 与脚本处理。
//!
//! 失败的签到也会写一条（结果为 `failed: <原因>`），便于排查「今天为什么没签上」。
//!
//! 本模块只负责**记录**与**读取**，不参与展示：余额的时序曲线与「最后一次余额增加」
//! 由 [`crate::core::balancelog`] 负责，`status` 读的是 `balance.log`。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::core::paths::provider_dir;

/// 记录文件名（位于 `providers/<id>/` 下）。
pub const FILE: &str = "claim.log";
/// 保留最近多少条。
pub const KEEP: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// RFC 3339 本地时间。
    pub time: String,
    /// 结果（英文）：`success` / `skipped: <原因>` / `failed: <错误>`。
    pub result: String,
    /// 当时的余额（`None` 表示未取到）。
    #[serde(default)]
    pub balance: Option<f64>,
    /// 本次签到上游明确告知的增量（`None` 表示上游未告知，只能靠余额差分）。
    #[serde(default)]
    pub delta: Option<f64>,
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
/// 裁剪采用「读全部 + 保留尾部 + 原子替换」，避免长期运行后文件无限增长。
pub fn append(root: &Path, provider: &str, entry: Entry) -> std::io::Result<()> {
    let path = path(root, provider);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }

    let mut entries = load(root, provider);
    entries.push(entry);
    if entries.len() > KEEP {
        let start = entries.len() - KEEP;
        entries.drain(..start);
    }

    let mut text = String::new();
    for e in &entries {
        // 序列化单行 JSON；失败的行跳过（Entry 字段都可序列化，理论不失败）。
        if let Ok(line) = serde_json::to_string(e) {
            text.push_str(&line);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ofm-claimlog-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(time: &str, result: &str, balance: Option<f64>) -> Entry {
        Entry {
            time: time.into(),
            result: result.into(),
            balance,
            delta: None,
        }
    }

    #[test]
    fn 追加后可读回() {
        let root = tmp_root("append");
        append(&root, "p", entry("t1", "success", Some(100.0))).unwrap();
        append(
            &root,
            "p",
            entry("t2", "skipped: already claimed", Some(100.0)),
        )
        .unwrap();
        let e = load(&root, "p");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].result, "success");
        assert_eq!(e[1].balance, Some(100.0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 只保留最近_keep_条() {
        let root = tmp_root("keep");
        for i in 0..(KEEP + 10) {
            append(
                &root,
                "p",
                entry(&format!("t{i}"), "success", Some(i as f64)),
            )
            .unwrap();
        }
        let e = load(&root, "p");
        assert_eq!(e.len(), KEEP);
        // 首条应是第 10 条（0..KEEP+10 保留尾部）。
        assert_eq!(e[0].time, "t10");
        assert_eq!(e[KEEP - 1].time, format!("t{}", KEEP + 9));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 无记录时读回空表() {
        let root = tmp_root("none");
        assert!(load(&root, "p").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
