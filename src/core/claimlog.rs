//! 签到记录：`providers/<id>/claim.log`，JSONL，每行一条。
//!
//! 只保留最近 [`KEEP`] 条，追加超出后裁剪。记录字段有时间、结果、当时余额、增量。
//!
//! **记录内容一律英文**：`code` 写 `success` / `skipped` / `failed`，跳过原因单列
//! 在 `reason`（如 `already claimed`），明细单列在 `detail`。
//!
//! 结果**不拼成一个字符串**：拼成 `"success: 100 credits"` 后，判「是否成功」只能靠
//! 字符串匹配，改明细文案就会静默地弄坏判断（历史 bug：`== "success"` 永远不成立，
//! `status` 恒显示「无成功签到」）。现在 `code` 是一个取值封闭的枚举词。
//!
//! 失败的签到也会写一条（`code=failed`，`detail` 为英文原因），便于排查
//! 「今天为什么没签上」。
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
/// `code` 字段：成功。
pub const CODE_SUCCESS: &str = "success";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// RFC 3339 本地时间。
    pub time: String,
    /// 结果码：`success` / `skipped` / `failed`（取值封闭，见 [`CODE_SUCCESS`]）。
    pub code: String,
    /// 跳过原因（仅 `code=skipped` 时有值），如 `already claimed`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// 明细（英文，逗号分隔），如 `100 credits, streak 29 days`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
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
        .filter_map(|l| parse_line(&l))
        .collect()
}

/// 解析一行。新格式直读；旧格式（单 `result` 串，如 `"success: 100 credits"`）
/// 拆成 `code` / `reason` / `detail`。
///
/// 过渡期保留：升级前写入的 `claim.log` 里已有真实成功记录，不能因格式变更
/// 把历史抹掉（那会让 `status` 显示「无记录」）。写入永远用新格式，旧行只在
/// 下一次 [`append`] 触发裁剪时消失。
fn parse_line(line: &str) -> Option<Entry> {
    if let Ok(e) = serde_json::from_str::<Entry>(line) {
        return Some(e);
    }
    #[derive(Deserialize)]
    struct Legacy {
        time: String,
        result: String,
        #[serde(default)]
        balance: Option<f64>,
        #[serde(default)]
        delta: Option<f64>,
    }
    let l: Legacy = serde_json::from_str(line).ok()?;
    let (code, reason, detail) = split_legacy_result(&l.result);
    Some(Entry {
        time: l.time,
        code,
        reason,
        detail,
        balance: l.balance,
        delta: l.delta,
    })
}

/// 旧 `result` 串 → `(code, reason, detail)`。
///
/// 旧格式形如 `success`、`success: <明细>`、`skipped: <原因>: <明细>`、
/// `failed: <原因>`。只用于读旧文件，新记录不经此路径。
fn split_legacy_result(result: &str) -> (String, Option<String>, Option<String>) {
    let mut parts = result.splitn(2, ':');
    let code = parts.next().unwrap_or_default().trim().to_string();
    let rest = parts.next().map(str::trim).filter(|s| !s.is_empty());
    match code.as_str() {
        "skipped" => {
            // 原因本身就是一段英文（无冒号分隔的结构），整段当 reason。
            (code, rest.map(str::to_string), None)
        }
        _ => (code, None, rest.map(str::to_string)),
    }
}

/// 追加一条记录，并将文件裁剪到最近 [`KEEP`] 条。
///
/// 裁剪采用「读全部 + 保留尾部 + 原子替换」，避免长期运行后文件无限增长。
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

/// 最近一次**成功**签到的时间（RFC 3339），没有成功记录时返回 `None`。
///
/// 判定看 `code == "success"`（完整的枚举词），不与明细/原因拼串。
/// `skipped` / `failed` 都不算成功：签到的「已领」是常态，
/// 把 `skipped` 当成成功会让「最后成功签到」永远显示今天，失去意义。
pub fn last_success(root: &Path, provider: &str) -> Option<String> {
    load(root, provider)
        .iter()
        .rev()
        .find(|e| e.code == CODE_SUCCESS)
        .map(|e| e.time.clone())
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

    fn entry(time: &str, code: &str, balance: Option<f64>) -> Entry {
        Entry {
            time: time.into(),
            code: code.into(),
            reason: None,
            detail: None,
            balance,
            delta: None,
        }
    }

    /// 带明细的成功记录（历史 bug 的真实形状：`result` 曾为 `success: ...`）。
    fn success_with_detail(time: &str, detail: &str) -> Entry {
        Entry {
            time: time.into(),
            code: CODE_SUCCESS.into(),
            reason: None,
            detail: Some(detail.into()),
            balance: None,
            delta: None,
        }
    }

    #[test]
    fn 追加后可读回() {
        let root = tmp_root("append");
        append(&root, "p", entry("t1", CODE_SUCCESS, Some(100.0))).unwrap();
        append(&root, "p", entry("t2", "skipped", Some(100.0))).unwrap();
        let e = load(&root, "p");
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].code, "success");
        assert_eq!(e[1].balance, Some(100.0));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 记录分字段存盘() {
        let root = tmp_root("fields");
        append(
            &root,
            "p",
            Entry {
                time: "t1".into(),
                code: "skipped".into(),
                reason: Some("already claimed".into()),
                detail: Some("day 2".into()),
                balance: None,
                delta: None,
            },
        )
        .unwrap();
        let raw = std::fs::read_to_string(path(&root, "p")).unwrap();
        // 码、原因、明细各占一个字段，没有拼成一个串。
        assert!(raw.contains(r#""code":"skipped""#), "{raw}");
        assert!(raw.contains(r#""reason":"already claimed""#), "{raw}");
        assert!(raw.contains(r#""detail":"day 2""#), "{raw}");
        let e = &load(&root, "p")[0];
        assert_eq!(e.reason.as_deref(), Some("already claimed"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 只保留最近_keep_条() {
        let root = tmp_root("keep");
        for i in 0..(KEEP + 10) {
            append(
                &root,
                "p",
                entry(&format!("t{i}"), CODE_SUCCESS, Some(i as f64)),
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
    fn 最后成功签到只认_success() {
        let root = tmp_root("last-success");
        append(&root, "p", entry("t1", CODE_SUCCESS, None)).unwrap();
        append(&root, "p", entry("t2", "skipped", None)).unwrap();
        append(&root, "p", entry("t3", "failed", None)).unwrap();
        assert_eq!(last_success(&root, "p").as_deref(), Some("t1"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 带明细的成功记录也算成功() {
        // 回归：历史 bug —— 存的是 `success: <明细>` 拼串，`== "success"` 不成立，
        // `status` 恒显示「无记录」。分字段后码就是裸 `success`。
        let root = tmp_root("last-success-detail");
        append(&root, "p", entry("t1", "skipped", None)).unwrap();
        append(
            &root,
            "p",
            success_with_detail("t2", "100 credits, streak 29 days"),
        )
        .unwrap();
        assert_eq!(last_success(&root, "p").as_deref(), Some("t2"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 无成功记录时最后成功签到为空() {
        let root = tmp_root("last-success-none");
        assert_eq!(last_success(&root, "p"), None);
        append(&root, "p", entry("t1", "skipped", None)).unwrap();
        assert_eq!(last_success(&root, "p"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 旧格式行被拆成新字段读回() {
        // 升级前的文件里只有 `result`，不能因格式变更丢掉历史成功记录。
        let root = tmp_root("legacy");
        let p = path(&root, "p");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(
            &p,
            concat!(
                "{\"time\":\"t0\",\"result\":\"success: 100 credits, streak 29 days\",",
                "\"balance\":3432.62,\"delta\":100.0}\n",
                "{\"time\":\"t1\",\"result\":\"skipped: already claimed\",",
                "\"balance\":null,\"delta\":null}\n",
                "{\"time\":\"t2\",\"result\":\"failed: credential invalid or expired\",",
                "\"balance\":null,\"delta\":null}\n",
            ),
        )
        .unwrap();
        let e = load(&root, "p");
        assert_eq!(e.len(), 3);
        assert_eq!(e[0].code, CODE_SUCCESS);
        assert_eq!(e[0].detail.as_deref(), Some("100 credits, streak 29 days"));
        assert_eq!(e[0].balance, Some(3432.62));
        assert_eq!(e[1].code, "skipped");
        assert_eq!(e[1].reason.as_deref(), Some("already claimed"));
        assert_eq!(e[2].code, "failed");
        assert_eq!(
            e[2].detail.as_deref(),
            Some("credential invalid or expired")
        );
        // 关键：旧的成功记录能被认成成功签到。
        assert_eq!(last_success(&root, "p").as_deref(), Some("t0"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 裁剪时旧行被重写成新格式() {
        // 超过 KEEP 触发全量重写：旧行不能以旧字段形状留在文件里。
        let root = tmp_root("migrate");
        let p = path(&root, "p");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        let legacy = (0..KEEP)
            .map(|i| {
                format!(
                    "{{\"time\":\"t{i}\",\"result\":\"success: x\",\"balance\":null,\"delta\":null}}\n"
                )
            })
            .collect::<String>();
        std::fs::write(&p, legacy).unwrap();
        // 第 KEEP+1 条：触发裁剪。
        append(&root, "p", entry("tnew", CODE_SUCCESS, None)).unwrap();
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(!raw.contains(r#""result""#), "{raw}");
        assert_eq!(raw.lines().count(), KEEP);
        assert_eq!(last_success(&root, "p").as_deref(), Some("tnew"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn 旧结果串拆分正确() {
        assert_eq!(
            split_legacy_result("success"),
            ("success".into(), None, None)
        );
        assert_eq!(
            split_legacy_result("success: daily check-in +100"),
            ("success".into(), None, Some("daily check-in +100".into()))
        );
        assert_eq!(
            split_legacy_result("skipped: already claimed"),
            ("skipped".into(), Some("already claimed".into()), None)
        );
        assert_eq!(
            split_legacy_result("failed: not logged in"),
            ("failed".into(), None, Some("not logged in".into()))
        );
    }

    #[test]
    fn 无记录时读回空表() {
        let root = tmp_root("none");
        assert!(load(&root, "p").is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }
}
