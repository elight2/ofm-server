//! 原子替换文件的跨平台封装。
//!
//! Unix 上 `rename(2)` 天然是原子替换；但 Windows 上有两个坑：
//!
//! 1. 目标文件若被其他进程以独占方式打开（共享冲突），`MoveFileEx` 会失败；
//! 2. 源文件句柄仍打开时（尤其带 `FILE_SHARE_DELETE` 关闭时）同样失败。
//!
//! 典型场景是日志/凭证文件正被另一个进程读取。因此这里统一做：
//! **确保源句柄已关闭 → rename → 遇到占用错误则短暂退避重试**。

use std::io;
use std::path::Path;

/// 重试次数。
const RETRIES: usize = 20;
/// 每次重试的间隔（毫秒）。
const BACKOFF_MS: u64 = 50;

/// 用 `src` 原子替换 `dst`。
///
/// `src` 的文件句柄必须已关闭（调用方负责）。
pub fn rename(src: &Path, dst: &Path) -> io::Result<()> {
    let mut last = io::Error::other("rename 失败");
    for attempt in 0..=RETRIES {
        match std::fs::rename(src, dst) {
            Ok(()) => return Ok(()),
            Err(e) => {
                // 只有「文件被占用」这类临时性错误才值得重试。
                if !is_busy(&e) {
                    return Err(e);
                }
                last = e;
                if attempt < RETRIES {
                    std::thread::sleep(std::time::Duration::from_millis(BACKOFF_MS));
                }
            }
        }
    }
    Err(last)
}

/// 判断错误是否为「文件被占用 / 共享冲突」。
///
/// Windows 的 `ERROR_SHARING_VIOLATION`(32) / `ERROR_LOCK_VIOLATION`(33) 会被
/// `std::io` 映射为 `PermissionDenied`；Linux 上临时文件被占用则多为
/// `WouldBlock` 之类的临时错误。
fn is_busy(e: &io::Error) -> bool {
    match e.kind() {
        io::ErrorKind::PermissionDenied => true,
        io::ErrorKind::WouldBlock => true,
        _ => e
            .raw_os_error()
            .is_some_and(|code| code == 32 || code == 33),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 覆盖已有文件() {
        let dir = std::env::temp_dir().join(format!("ofm-atomic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("a.tmp");
        let dst = dir.join("b.txt");
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dst, b"old").unwrap();
        rename(&src, &dst).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "new");
        std::fs::remove_dir_all(&dir).ok();
    }
}
