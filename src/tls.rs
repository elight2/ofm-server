//! TLS 加密提供者初始化。
//!
//! `reqwest` 以 `rustls-tls-webpki-roots-no-provider` 特性启用 rustls，
//! 该模式**不自带任何加密算法实现**，必须由应用显式安装 `CryptoProvider`，
//! 否则构造客户端时会报 `No provider set`。
//!
//! 这里选用 RustCrypto 的纯 Rust 实现（`rustls-rustcrypto`），
//! 好处是依赖树里不再有 `ring` / `aws-lc-rs` 这类含 C/汇编代码的 crate，
//! 交叉编译 Windows 时无需额外的 C 编译器与 NASM/Perl 工具链。

use std::sync::Once;

static INIT: Once = Once::new();

/// 安装 rustls 的纯 Rust 加密提供者。
///
/// 进程内只需执行一次，重复调用是安全的（内部用 [`Once`] 保证幂等）。
/// `install_default` 只能成功一次，且失败时通常意味着别处已装过 provider，
/// 此处忽略错误即可。
pub fn init() {
    INIT.call_once(|| {
        let _ = rustls::crypto::CryptoProvider::install_default(rustls_rustcrypto::provider());
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn provider_可重复安装且不报错() {
        super::init();
        super::init();
        // 安装后 rustls 应能拿到默认 provider。
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
