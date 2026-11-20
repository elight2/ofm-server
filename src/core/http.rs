//! HTTP 客户端构造与错误体读取。
//!
//! 各家原先各写一份 `reqwest::Client::builder()...build()`，重复的不仅是代码：
//! 其中一家（loomy）漏了 [`crate::tls::init`]，只因别的供应商先构造才侥幸没炸。
//! 统一到这里后，**任何客户端都保证先装好 TLS provider**。

use std::time::Duration;

use crate::core::text::truncate_chars;

/// 客户端构造参数。未设的项即「不施加该限制」。
#[derive(Debug, Default, Clone, Copy)]
pub struct Spec {
    /// `User-Agent`。`None` 时不发该头（保持 reqwest 默认）。
    pub user_agent: Option<&'static str>,
    /// 整请求总超时。**SSE 长连接不要设**，改用 `connect_timeout` + `read_timeout`。
    pub timeout: Option<Duration>,
    /// 连接建立超时。
    pub connect_timeout: Option<Duration>,
    /// 单次读取超时（读不到新字节即超时，不是整个响应的上限）。
    pub read_timeout: Option<Duration>,
    /// 绕过系统代理（有的上游对代理会拒绝或串味）。
    pub no_proxy: bool,
}

impl Spec {
    /// 只设 UA。
    pub fn ua(ua: &'static str) -> Self {
        Self {
            user_agent: Some(ua),
            ..Self::default()
        }
    }

    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout = Some(Duration::from_secs(secs));
        self
    }

    pub fn connect_timeout(mut self, secs: u64) -> Self {
        self.connect_timeout = Some(Duration::from_secs(secs));
        self
    }

    pub fn read_timeout(mut self, secs: u64) -> Self {
        self.read_timeout = Some(Duration::from_secs(secs));
        self
    }

    pub fn no_proxy(mut self) -> Self {
        self.no_proxy = true;
        self
    }
}

/// 按 [`Spec`] 构造客户端。
///
/// 顺带保证 TLS provider 已安装（[`crate::tls::init`] 幂等），故**不要**再在别处
/// 单独调它。
pub fn client(spec: Spec) -> reqwest::Result<reqwest::Client> {
    crate::tls::init();

    let mut b = reqwest::Client::builder();
    if let Some(ua) = spec.user_agent {
        b = b.user_agent(ua);
    }
    if let Some(t) = spec.timeout {
        b = b.timeout(t);
    }
    if let Some(t) = spec.connect_timeout {
        b = b.connect_timeout(t);
    }
    if let Some(t) = spec.read_timeout {
        b = b.read_timeout(t);
    }
    if spec.no_proxy {
        b = b.no_proxy();
    }
    b.build()
}

/// 读非 2xx 响应体，返回 `(状态码, 截断后的正文)`。
///
/// 各家错误体形态不同（`error_code` / `{code,desc}` 信封 / `{error:{...}}`），
/// 分类仍由各家自己做；这里只统一「读文本 + 截断」这一步。
/// 调用前须先确认状态非 2xx——本函数会消费 `resp`。
pub async fn error_body(resp: reqwest::Response, limit: usize) -> (u16, String) {
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    (status, truncate_chars(&text, limit))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_默认不设限制() {
        let s = Spec::ua("x");
        assert_eq!(s.user_agent, Some("x"));
        assert!(s.timeout.is_none() && s.connect_timeout.is_none());
        assert!(!s.no_proxy);
    }

    #[test]
    fn 构造客户端会装好_tls() {
        // 若 `tls::init` 未执行，rustls 会报 `No provider set`。
        client(Spec::ua("ofm-server-test").timeout(5)).expect("构造客户端");
        assert!(rustls::crypto::CryptoProvider::get_default().is_some());
    }
}
