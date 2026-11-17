//! 签到记录：`providers/<id>/claim.log`，JSONL，每行一条。
//!
//! 只保留最近 [`KEEP`] 条，追加超出后裁剪。记录字段只有时间、结果、当前余额。
//!
//! 失败的签到也会写一条（结果为失败描述），便于排查「今天为什么没签上」。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// 记录文件名（位于 `providers/<id>/` 下）。
pub const FILE: &str = "claim.log";
/// 保留最近多少条。
pub const KEEP: usize = 50;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// RFC 3339 本地时间。
    pub time: String,
    /// 结果：`成功` / `跳过: <原因>` / `失败: <错误>`。
    pub result: String,
    /// 当时的余额（`None` 表示未取到）。
    #[serde(default)]
    pub balance: Option<f64>,
    /// 本次签到上游明确告知的增量（`None` 表示上游未告知，只能靠余额差分）。
    #[serde(default)]
    pub delta: Option<f64>,
}

impl Entry {
    /// 「成功签到」且余额变化可算的条目（`result` 不以「跳过/失败」开头）。
    pub fn is_success(&self) -> bool {
        self.result == "成功"
    }
}

fn path(root: &Path, provider: &str) -> PathBuf {
    Config::provider_dir(root, provider).join(FILE)
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
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// 最近一次成功签到的时间与增量（`status` 用）。
///
/// 增量 = 该条成功记录的余额 − 它之前最近一条记录的余额（无前一条时为 `None`）。
pub fn last_success(root: &Path, provider: &str) -> Option<(String, Option<f64>)> {
    let entries = load(root, provider);
    let idx = entries.iter().rposition(Entry::is_success)?;
    // 优先用上游明确告知的增量：余额差分受首次签到、中途失败等影响，不一定准。
    if let Some(d) = entries[idx].delta {
        return Some((entries[idx].time.clone(), Some(d)));
    }
    let cur = entries[idx].balance;
    let prev = entries[..idx].iter().rev().find_map(|e| e.balance);
    let delta = match (cur, prev) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    };
    Some((entries[idx].time.clone(), delta))
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
        append(&root, "p", entry("t1", "成功", Some(100.0))).unwrap();
        append(&root, "p", entry("t2", "跳过: 今日已签到", Some(100.0))).unwrap();
        let e = load(&root, "p");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].result, "成功");
        assert_eq!(e[1].balance, Some(100.0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 只保留最近_keep_条() {
        let root = tmp_root("keep");
        for i in 0..(KEEP + 10) {
            append(&root, "p", entry(&format!("t{i}"), "成功", Some(i as f64))).unwrap();
        }
        let e = load(&root, "p");
        assert_eq!(e.len(), KEEP);
        // 首条应是第 10 条（0..KEEP+10 保留尾部）。
        assert_eq!(e[0].time, "t10");
        assert_eq!(e[KEEP - 1].time, format!("t{}", KEEP + 9));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 无记录时返回_none() {
        let root = tmp_root("none");
        assert!(last_success(&root, "p").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 最近成功取时间与增量() {
        let root = tmp_root("last");
        append(&root, "p", entry("t1", "成功", Some(100.0))).unwrap();
        append(&root, "p", entry("t2", "跳过: 今日已签到", Some(100.0))).unwrap();
        append(&root, "p", entry("t3", "成功", Some(150.0))).unwrap();
        let (time, delta) = last_success(&root, "p").unwrap();
        assert_eq!(time, "t3");
        // 增量相对 t2 的余额 100 → +50
        assert_eq!(delta, Some(50.0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 首次成功无前值则增量为_none() {
        let root = tmp_root("first");
        append(&root, "p", entry("t1", "成功", Some(100.0))).unwrap();
        let (time, delta) = last_success(&root, "p").unwrap();
        assert_eq!(time, "t1");
        assert_eq!(delta, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 上游告知的增量优先于余额差分() {
        let root = tmp_root("delta-preferred");
        append(&root, "p", entry("t1", "成功", Some(100.0))).unwrap();
        // t2 余额差分应为 +50，但上游明确告知 300（中间可能有过消耗）。
        let mut e = entry("t2", "成功", Some(400.0));
        e.delta = Some(300.0);
        append(&root, "p", e).unwrap();
        let (time, delta) = last_success(&root, "p").unwrap();
        assert_eq!(time, "t2");
        assert_eq!(delta, Some(300.0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 失败记录不算成功() {
        let root = tmp_root("fail");
        append(&root, "p", entry("t1", "失败: 网络错误", None)).unwrap();
        assert!(last_success(&root, "p").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
