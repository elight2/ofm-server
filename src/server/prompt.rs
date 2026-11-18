//! 系统提示词：追加到客户端自带系统提示词**之后**。
//!
//! 用途见 README：部分供应商的模型表现与官方模型不同，可用一段提示词修正行为。
//! 效果等价于 pi 的 `~/.pi/agent/AGENTS.md`——**追加**，不是替换，
//! 且不区分下游是否已有系统提示词（一律注入）。
//!
//! 内容来源两级：
//! 1. `<root>/prompts/<供应商id>.md`（用户自定义，存在即用它）；
//! 2. 否则用该供应商的内嵌默认 [`crate::config::default_prompt`]
//!    （= 仓库 `default-prompts/<供应商id>.md`，每家一份）。
//!
//! 默认内容**不硬编码在代码里**，只有那些 Markdown 文件是真相；
//! 没有默认文件的供应商不注入任何内容。
//! 空白自定义文件同样回退到默认（空白注入无意义），故当前无法用空文件关掉注入；
//! 要彻底不注入需删掉该供应商的默认文件，当前不提供配置开关。

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::config::{self, Config};
use crate::core::provider as p;

/// 一个文件条目的缓存值：`(mtime, 内容)`。
///
/// `mtime` 为 `None` 表示文件不存在（此时内容恒为 `None`）。
type Entry = (Option<SystemTime>, Option<String>);

/// 提示词缓存：路径 → [`Entry`]。
type Cache = Mutex<HashMap<String, Entry>>;

/// 提示词缓存。
///
/// 稳态下每请求都要查一次，重复 `read` 同一文件的代价没必要付；
/// 用 mtime 做失效判定，用户改文件后**下一次请求即生效**，无需重启。
fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取该供应商要注入的系统提示词。
///
/// 优先级：`<root>/prompts/<provider>.md` → 该供应商的内嵌默认
/// [`config::default_prompt`]（`default-prompts/<provider>.md`）。
///
/// * 自定义文件不存在 → 回退默认（不打日志，属正常状态）；
/// * 自定义文件读取失败 → 回退默认 + `warn`（不阻断请求）；
/// * 自定义文件为空白 → 回退默认（空白注入无意义，且部分上游会拒）。
///
/// 返回 `None` 表示该供应商没有自定义也没有默认提示词（不注入）。
pub fn load(root: &Path, provider: &str) -> Option<String> {
    let path = Config::prompt_path(root, provider);
    let key = path.to_string_lossy().into_owned();

    let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

    if let Ok(guard) = cache().lock()
        && let Some((cached_mtime, cached)) = guard.get(&key)
        && *cached_mtime == mtime
    {
        return cached.clone();
    }

    let content = if mtime.is_none() {
        // 没有自定义文件：用内嵌默认。
        None
    } else {
        match std::fs::read_to_string(&path) {
            Ok(text) if text.trim().is_empty() => None,
            Ok(text) => Some(text),
            Err(e) => {
                tracing::warn!("[{provider}] 读取自定义提示词失败 {path:?}: {e}");
                None
            }
        }
    };

    if let Ok(mut guard) = cache().lock() {
        guard.insert(key, (mtime, content.clone()));
    }

    // 自定义内容优先；没有（或读失败/空白）则用该供应商的内嵌默认。
    content.or_else(|| default_for(provider))
}

/// 该供应商的内嵌默认提示词；没有或内容为空白时返回 `None`。
fn default_for(provider: &str) -> Option<String> {
    config::default_prompt(provider).and_then(non_empty)
}

/// 空白文本视为「无内容」。
fn non_empty(text: &str) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

/// 把自定义提示词追加到请求的系统提示词之后。
///
/// 落点选择（与 pi 的 `AGENTS.md` 一致）：
/// * 客户端**已有**系统提示词 → 追加在其后，用空行分隔。放前面会被客户端
///   自己的规则覆盖，放后面才是「补充修正」。
/// * 客户端**没有**系统提示词 → 插到消息最前面（`system` 必须在首位）。
///
/// 只处理 `ChatRequest` 层面，供应商层无需感知——四家都会把 `Role::System`
/// 收进各自的上游字段（minimax 抽出成顶层 `system`，其余透传 `messages`）。
pub fn apply(req: &mut p::ChatRequest, prompt: &str) {
    let extra = prompt.trim_end();
    if extra.is_empty() {
        return;
    }

    // 找最后一条系统消息（可能有多条：`system` 与 `developer` 会被合并成同角色）。
    if let Some(last) = req.messages.iter().rposition(|m| m.role == p::Role::System) {
        let parts = &mut req.messages[last].parts;
        // 已有非空文本 → 追加到该段之后；否则直接放进去。
        match parts.iter_mut().rev().find_map(|part| match part {
            p::Part::Text(t) => Some(t),
            p::Part::Image { .. } => None,
        }) {
            Some(text) => {
                // 统一成「恰好一个空行」分隔：先削掉原尾部空白再补两换行，
                // 避免客户端提示词自带换行时出现三四个连续空行。
                if !text.trim_end().is_empty() {
                    text.truncate(text.trim_end().len());
                    text.push_str("\n\n");
                } else {
                    text.clear();
                }
                text.push_str(extra);
            }
            None => parts.push(p::Part::Text(extra.to_string())),
        }
        return;
    }

    req.messages.insert(
        0,
        p::Message {
            role: p::Role::System,
            parts: vec![p::Part::Text(extra.to_string())],
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: p::Role, text: &str) -> p::Message {
        p::Message {
            role,
            parts: vec![p::Part::Text(text.to_string())],
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
        }
    }

    fn req(messages: Vec<p::Message>) -> p::ChatRequest {
        p::ChatRequest {
            model: "m".into(),
            messages,
            tools: Vec::new(),
            tool_choice: p::ToolChoice::Auto,
        }
    }

    fn text_of(m: &p::Message) -> String {
        m.parts
            .iter()
            .filter_map(|p| match p {
                p::Part::Text(t) => Some(t.as_str()),
                p::Part::Image { .. } => None,
            })
            .collect()
    }

    /// 已有系统提示词：追加在其末尾，用空行分隔，不新建消息。
    #[test]
    fn 追加到已有系统提示词后() {
        let mut r = req(vec![
            msg(p::Role::System, "客户端规则"),
            msg(p::Role::User, "hi"),
        ]);
        apply(&mut r, "自定义修正");
        assert_eq!(r.messages.len(), 2, "不应新增消息");
        assert_eq!(text_of(&r.messages[0]), "客户端规则\n\n自定义修正");
        assert_eq!(r.messages[0].role, p::Role::System);
    }

    /// 客户端没有系统提示词：插到最前面（`system` 必须在首位）。
    #[test]
    fn 无系统提示词时插到最前() {
        let mut r = req(vec![msg(p::Role::User, "hi")]);
        apply(&mut r, "自定义修正");
        assert_eq!(r.messages.len(), 2);
        assert_eq!(r.messages[0].role, p::Role::System);
        assert_eq!(text_of(&r.messages[0]), "自定义修正");
        assert_eq!(text_of(&r.messages[1]), "hi");
    }

    /// 多条系统消息（`system` + `developer` 合并后可能相邻）：只追加到最后一条。
    #[test]
    fn 多条系统消息追加到最后一条() {
        let mut r = req(vec![
            msg(p::Role::System, "规则一"),
            msg(p::Role::System, "规则二"),
            msg(p::Role::User, "hi"),
        ]);
        apply(&mut r, "修正");
        assert_eq!(r.messages.len(), 3, "不应新增消息");
        assert_eq!(text_of(&r.messages[0]), "规则一");
        assert_eq!(text_of(&r.messages[1]), "规则二\n\n修正");
    }

    /// 系统消息里没有文本片段（只有图片）：补一个文本片段，不留空追加。
    #[test]
    fn 系统消息无非文本时补片段() {
        let mut r = req(vec![p::Message {
            role: p::Role::System,
            parts: Vec::new(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            tool_name: None,
        }]);
        apply(&mut r, "修正");
        assert_eq!(text_of(&r.messages[0]), "修正");
    }

    /// 系统提示词自带尾部空白时统一成「恰好一个空行」，不留下三四个连续空行。
    #[test]
    fn 尾部空白被规范化() {
        for original in ["规则", "规则\n", "规则  \n\n\n"] {
            let mut r = req(vec![msg(p::Role::System, original)]);
            apply(&mut r, "修正");
            assert_eq!(text_of(&r.messages[0]), "规则\n\n修正", "原文 {original:?}");
        }
    }

    /// 系统消息内容是纯空白时，直接换成提示词本身（不留前导空行）。
    #[test]
    fn 纯空白系统消息被替换() {
        let mut r = req(vec![msg(p::Role::System, "   \n\t ")]);
        apply(&mut r, "修正");
        assert_eq!(text_of(&r.messages[0]), "修正");
    }

    /// 空白提示词不改变请求（避免注入空串被上游拒）。
    #[test]
    fn 空白提示词不注入() {
        let mut r = req(vec![msg(p::Role::User, "hi")]);
        apply(&mut r, "   \n\t ");
        assert_eq!(r.messages.len(), 1);
    }

    /// 文件不存在 → 回退到该供应商的内嵌默认，不报错。
    #[test]
    fn 无自定义文件用默认() {
        let dir = std::env::temp_dir().join("ofm-prompt-test-none");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let want = default_for("trae").expect("trae 应有内嵌默认");
        assert_eq!(load(&dir, "trae"), Some(want));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 内嵌默认内容非空白（否则「回退默认」等于没注入），且每个内置供应商都有。
    #[test]
    fn 默认提示词非空白() {
        for id in ["trae", "raccoon", "codearts", "minimax"] {
            let text = config::default_prompt(id).unwrap_or_else(|| panic!("{id} 缺默认提示词"));
            assert!(!text.trim().is_empty(), "{id} 默认提示词为空白");
        }
    }

    /// 未知供应商没有内嵌默认 → 不注入。
    #[test]
    fn 未知供应商无默认() {
        assert!(config::default_prompt("unknown-provider").is_none());
        let dir = std::env::temp_dir().join("ofm-prompt-test-unknown");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(load(&dir, "unknown-provider"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 文件存在 → 读到内容；内容改动后（mtime 变化）下次读到新内容。
    #[test]
    fn 提示词按mtime刷新() {
        let dir = std::env::temp_dir().join("ofm-prompt-test-reload");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("prompts")).unwrap();
        let path = dir.join("prompts").join("trae.md");
        std::fs::write(&path, "第一版").unwrap();
        assert_eq!(load(&dir, "trae").as_deref(), Some("第一版"));

        // 改内容并把 mtime 明确推到未来，避免同秒内写入 mtime 不变。
        std::fs::write(&path, "第二版").unwrap();
        let future = SystemTime::now() + std::time::Duration::from_secs(2);
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(future).unwrap();
        drop(f);
        assert_eq!(load(&dir, "trae").as_deref(), Some("第二版"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 空白自定义文件 → 回退默认（不能变成「不注入」的意外后门）。
    #[test]
    fn 空白文件回退默认() {
        let dir = std::env::temp_dir().join("ofm-prompt-test-blank");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("prompts")).unwrap();
        std::fs::write(dir.join("prompts").join("trae.md"), "  \n\t\n").unwrap();
        assert_eq!(load(&dir, "trae"), default_for("trae"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 自定义文件优先于内嵌默认。
    #[test]
    fn 自定义优先于默认() {
        let dir = std::env::temp_dir().join("ofm-prompt-test-priority");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("prompts")).unwrap();
        std::fs::write(dir.join("prompts").join("trae.md"), "自定义内容").unwrap();
        assert_eq!(load(&dir, "trae").as_deref(), Some("自定义内容"));
        // 另一个供应商没有自定义文件 → 仍是它自己的默认
        assert_eq!(load(&dir, "raccoon"), default_for("raccoon"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不同供应商读到各自的文件。
    #[test]
    fn 按供应商隔离() {
        let dir = std::env::temp_dir().join("ofm-prompt-test-per-provider");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("prompts")).unwrap();
        std::fs::write(dir.join("prompts").join("trae.md"), "trae 的").unwrap();
        std::fs::write(dir.join("prompts").join("minimax.md"), "minimax 的").unwrap();
        assert_eq!(load(&dir, "trae").as_deref(), Some("trae 的"));
        assert_eq!(load(&dir, "minimax").as_deref(), Some("minimax 的"));
        // 未写自定义的供应商回退自己的默认，不会串到别人的内容
        assert_eq!(load(&dir, "raccoon"), default_for("raccoon"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
