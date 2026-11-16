//! `ofm-server login <供应商id>`：浏览器 OAuth + 本地回调，保存凭证。
//!
//! 流程见 `provider-info/<id>.md` §1。本模块只负责与供应商无关的编排：
//! 1. 供应商给出授权 URL 与回调端口；
//! 2. 用 `xdg-open` 打开浏览器；
//! 3. 本地 HTTP 服务接收回调；
//! 4. 供应商用回调结果换取凭证并写盘。
//!
//! * trae：PKCE S256 + `ExchangeToken`（`trae::auth`）
//! * raccoon：loopback 兼容登录，本地收货后 `login_with_authorization_code`
//!   （`raccoon::auth`，自带一次性回调服务，不走 `wait_callback_on`）
//! * codearts：PKCE(SHA-256) + DPoP(ES256)，门户授权 + STS 换 AKSK
//!   （`codearts::auth`，自带回调服务）

use std::time::Duration;

use crate::providers::{codearts, raccoon, trae};

/// 浏览器 OAuth 登录。
pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    match provider_id {
        trae::ID => trae::auth::login(root).await,
        raccoon::ID => raccoon::auth::run(root).await,
        codearts::ID => codearts::auth::run(root).await,
        other => anyhow::bail!(
            "未知供应商 `{other}`，可用: {}, {}, {}",
            trae::ID,
            raccoon::ID,
            codearts::ID
        ),
    }
}

/// 用系统默认浏览器打开 URL。
pub fn open_browser(url: &str) -> anyhow::Result<()> {
    let candidates: &[(&str, &[&str])] = if cfg!(target_os = "macos") {
        &[("open", &[])]
    } else if cfg!(target_os = "windows") {
        &[("cmd", &["/C", "start", ""])]
    } else {
        &[("xdg-open", &[])]
    };
    for (cmd, prefix) in candidates {
        match std::process::Command::new(cmd)
            .args(*prefix)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => return Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        }
    }
    anyhow::bail!("找不到可用的浏览器打开命令（例如 xdg-open）")
}

/// 同上，但只接受指定路径的请求（其它路径返回 404 并继续等待）。
pub async fn wait_callback_on(
    port: u16,
    expect_path: &str,
    timeout: Duration,
) -> anyhow::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let (mut sock, _) = tokio::time::timeout_at(deadline, listener.accept())
            .await
            .map_err(|_| anyhow::anyhow!("等待回调超时（{}s）", timeout.as_secs()))??;

        let mut buf = vec![0u8; 8192];
        let n = match tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await {
            Ok(Ok(n)) => n,
            _ => continue,
        };
        let request = String::from_utf8_lossy(&buf[..n]);
        let first_line = request.lines().next().unwrap_or_default();
        let path = first_line.split_whitespace().nth(1).unwrap_or("/");
        let (route, query) = match path.split_once('?') {
            Some((r, q)) => (r, q.to_string()),
            None => (path, String::new()),
        };

        if route != expect_path {
            let _ = sock
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await;
            let _ = sock.shutdown().await;
            continue;
        }

        let body = "<html><body>登录完成，可以关闭此页面。</body></html>";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        let _ = sock.write_all(resp.as_bytes()).await;
        let _ = sock.shutdown().await;

        return Ok(query);
    }
}

/// 从 `application/x-www-form-urlencoded` 查询串取某个键。
pub fn query_param(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k == key {
            Some(urlencoding::decode(v).ok()?.into_owned())
        } else {
            None
        }
    })
}
