//! 本机官方客户端的 aha 设备号（`did`）。
//!
//! 签到（`checkin_credits/claim`）按 `x-device-id` 做风控，实测口径见
//! `provider-info/trae.md` §6.1：同一个 JWT、同一份请求体，只换 `x-device-id`，
//! 工具自己生成的 16 位随机号被回 `9074`，官方客户端的 aha 设备号则回 `code:0`。
//! 后台只认「官方客户端注册过的设备」，故本模块在**本机装有官方客户端**时
//! 直接复用它的 `did`（同一台机器、同一个用户，本来就是同一个设备）。
//!
//! 取法按可靠性排序，全部只读、纯本地：
//!
//! 1. `User/globalStorage/storage.json` 里的 `iCubeAuthInfo://icube-dc:<did>` 键
//!    （官方按设备存凭据，键名自带 did）；
//! 2. `logs/aha_log/aha_electron_*.log` 的 `did: <did>`（ICDRS / DeviceService 打点）。
//!
//! 都取不到（例如服务器上没装客户端）时返回 `None`，调用方回退到凭证里的
//! [`crate::providers::trae::device::Device::device_id`]。

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// 官方客户端的用户数据目录名（国际版无 ` CN` 后缀）。
const APP_DIRS: [&str; 2] = ["Trae CN", "Trae"];

/// 本机官方客户端的 aha 设备号；取不到时 `None`（进程内缓存一次）。
pub fn device_id() -> Option<&'static str> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE.get_or_init(discover).as_deref()
}

fn discover() -> Option<String> {
    roots()
        .iter()
        .find_map(|root| from_storage_json(root).or_else(|| from_aha_log(root)))
}

/// 候选的用户数据目录：Linux 走 `~/.config`，Windows 走 `%APPDATA%`。
fn roots() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    if cfg!(windows)
        && let Ok(appdata) = std::env::var("APPDATA")
        && !appdata.is_empty()
    {
        bases.push(PathBuf::from(appdata));
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        bases.push(PathBuf::from(xdg));
    }
    if let Some(home) = home_dir() {
        bases.push(home.join(".config"));
    }
    let mut out = Vec::new();
    for base in bases {
        for name in APP_DIRS {
            let dir = base.join(name);
            if !out.contains(&dir) {
                out.push(dir);
            }
        }
    }
    out
}

fn home_dir() -> Option<PathBuf> {
    let var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// `storage.json` 里 `iCubeAuthInfo://icube-dc:<did>` 的键名。
fn from_storage_json(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("User/globalStorage/storage.json")).ok()?;
    let did = scan_digits_after(&text, "icube-dc:")?;
    plausible(did).then(|| did.to_string())
}

/// `aha_log` 里形如 `did: 1694826430742544` 的打点（`ICDRS` / `DeviceService`）。
fn from_aha_log(root: &Path) -> Option<String> {
    let dir = root.join("logs/aha_log");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("aha_electron_"))
        })
        .collect();
    // 日志按日期命名，新的排在后面；从最新的一份开始找。
    files.sort();
    for path in files.iter().rev() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for marker in ["device_id: ", "did: "] {
            let mut rest = text.as_str();
            while let Some(found) = scan_digits_after(rest, marker) {
                if plausible(found) {
                    return Some(found.to_string());
                }
                // 跳过这次命中继续找（`ldid: aha-…` 之类不是设备号）。
                let skip = rest.find(marker)? + marker.len() + found.len();
                rest = &rest[skip..];
            }
        }
    }
    None
}

/// 取标记之后的第一段连续数字。
fn scan_digits_after<'a>(text: &'a str, marker: &str) -> Option<&'a str> {
    let start = text.find(marker)? + marker.len();
    let rest = &text[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    (end > 0).then(|| &rest[..end])
}

/// aha 设备号是 16 位十进制数（`毫秒时间戳 * 1000 + 序号` 的形态）。
fn plausible(did: &str) -> bool {
    did.len() == 16 && did.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 数字串提取() {
        let text = r#"{"iCubeAuthInfo://icube-dc:1694826430742544":"xx"}"#;
        assert_eq!(
            scan_digits_after(text, "icube-dc:"),
            Some("1694826430742544")
        );
        let log = "[ICDRS] (init) initialization done, did: 1694826430742544, ldid: aha-1, rdid: 1";
        assert_eq!(scan_digits_after(log, "did: "), Some("1694826430742544"));
        assert_eq!(scan_digits_after(log, "缺"), None);
    }

    #[test]
    fn 本机探测结果形状合法() {
        // 没装官方客户端时返回 None，装了就必须是 16 位数字。
        if let Some(did) = device_id() {
            assert!(plausible(did), "探测到的设备号不合法: {did}");
            println!("本机官方客户端 did = {did}");
        }
    }

    #[test]
    fn 设备号形状() {
        assert!(plausible("1694826430742544"));
        assert!(!plausible("169482643074254")); // 15 位
        assert!(!plausible("16948264307425440")); // 17 位
        assert!(!plausible("aha-7a95a869"));
    }
}
