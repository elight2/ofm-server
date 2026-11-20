//! `ofm-server login <供应商id>`：浏览器 OAuth + 本地回调，保存凭证。
//!
//! 流程见 `provider-info/<id>.md` §1。本模块只负责与供应商无关的编排：
//! 1. 供应商给出授权 URL 与回调端口；
//! 2. 打开系统默认浏览器（Windows 走 `cmd /C start`，见 [`open_browser`]）；
//! 3. 本地 HTTP 服务接收回调；
//! 4. 供应商用回调结果换取凭证并写盘。
//!
//! * trae：PKCE S256 + `ExchangeToken`（`trae::auth`，用 [`CallbackServer`]）
//! * raccoon：loopback 兼容登录，本地收货后 `login_with_authorization_code`
//!   （`raccoon::auth`，自带一次性回调服务）
//! * codearts：PKCE(SHA-256) + DPoP(ES256)，门户授权 + STS 换 AKSK
//!   （`codearts::auth`，自带回调服务）
//! * minimax：OAuth 2.0 设备码 + PKCE，**无本地回调**，打印 `user_code`
//!   后轮询令牌（`minimax::auth`，需要用户在浏览器扫码 + 短信验证）

use std::time::Duration;

use crate::providers::{codearts, loomy, minimax, raccoon, trae};

/// 浏览器 OAuth 登录。
pub async fn run(root: std::path::PathBuf, provider_id: &str) -> anyhow::Result<()> {
    match provider_id {
        trae::ID => trae::auth::login(root).await,
        raccoon::ID => raccoon::auth::run(root).await,
        codearts::ID => codearts::auth::run(root).await,
        minimax::ID => minimax::auth::run(root).await,
        loomy::ID => loomy::auth::login(root).await,
        other => anyhow::bail!(
            "未知供应商 `{other}`，可用: {}, {}, {}, {}, {}",
            trae::ID,
            raccoon::ID,
            codearts::ID,
            minimax::ID,
            loomy::ID
        ),
    }
}

/// 用系统默认浏览器打开 URL。
///
/// Windows 走 `cmd /C start "" <url>`（不为此引入 winapi 依赖），但**必须自己给
/// URL 加引号**：`start` 会按 `&` / `|` / `^` 重新切分收到的命令行，而授权 URL 的
/// 参数就是用 `&` 连接的。
///
/// 而 `std::process::Command` 在 Windows 上只给「含空白或引号」的参数加引号——
/// 本项目拼出的授权 URL 已把空格编码为 `%20`，不含这两类字符，于是 URL 以裸形态
/// 交给 `cmd`，`&` 被当作命令分隔符：浏览器只打开第一个 `&` 之前的截断 URL，
/// 其余片段被逐条当作命令执行（报错被丢弃），其中末段的
/// `http://127.0.0.1:<回调端口>/<回调路径>` 还会额外弹出一个访问本地回调端口的
/// 标签页——Windows 上「浏览器最后一步卡在连接」即由此产生。
///
/// 显式加引号后，`cmd` 把整串当成 `start` 的第二个参数（第一个是空标题），不再切分。
pub fn open_browser(url: &str) -> anyhow::Result<()> {
    if cfg!(target_os = "windows") {
        let quoted = format!("\"{url}\"");
        return spawn_first(&[("cmd", &["/C", "start", "", quoted.as_str()])]);
    }
    if cfg!(target_os = "macos") {
        return spawn_first(&[("open", &[url])]);
    }
    spawn_first(&[("xdg-open", &[url])])
}

/// 依次尝试候选命令，`NotFound` 时继续下一个。
fn spawn_first(candidates: &[(&str, &[&str])]) -> anyhow::Result<()> {
    for (cmd, args) in candidates {
        match std::process::Command::new(cmd)
            .args(*args)
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

/// 已绑定的回调监听器。
///
/// 调用方必须**先 `bind` 再开浏览器**：否则浏览器/log 里的链接抢在 `bind` 之前
/// 连接时会得到 `ERR_CONNECTION_REFUSED`（Windows 上尤其容易，`cmd /C start`
/// 拉起浏览器只需几十毫秒）。
pub struct CallbackServer {
    listener: tokio::net::TcpListener,
    port: u16,
}

impl CallbackServer {
    /// 绑定 `127.0.0.1:<port>`。
    pub async fn bind(port: u16) -> anyhow::Result<Self> {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
            .await
            .map_err(|e| anyhow::anyhow!("无法监听回调端口 {port}: {e}"))?;
        let port = listener.local_addr()?.port();
        Ok(Self { listener, port })
    }

    /// 实际监听的端口（`bind(0)` 时为系统分配的端口）。
    pub fn port(&self) -> u16 {
        self.port
    }

    /// 等待指定路径的回调，返回其查询串（不含 `?`）。
    ///
    /// 连接**并发**处理：浏览器会先发探测/favicon 等零星连接，若串行处理，真正的
    /// 回调会被排在这些连接后面（每个最长 5s），标签页看起来一直「连接中」。
    /// 取得目标路径的查询串后立即返回，其余连接在后台自行收拾。
    pub async fn wait(self, expect_path: &str, timeout: Duration) -> anyhow::Result<String> {
        let deadline = tokio::time::Instant::now() + timeout;
        // `spawn` 要求 `'static`，目标路径很短且每家只有一个，克隆一份给后台任务。
        let expect = expect_path.to_string();

        loop {
            let (sock, _) = tokio::time::timeout_at(deadline, self.listener.accept())
                .await
                .map_err(|_| anyhow::anyhow!("等待回调超时（{}s）", timeout.as_secs()))??;

            // 后台处理单条连接：只关心是否命中目标路径，命中就把查询串送回来。
            // 故意不等待 handle 结束——未命中的连接（favicon、预连接）要能自生自灭。
            let (tx, rx) = tokio::sync::oneshot::channel::<String>();
            let expect = expect.clone();
            tokio::spawn(async move {
                if let Some(query) = handle_conn(sock, &expect).await {
                    let _ = tx.send(query);
                }
            });

            match tokio::time::timeout_at(deadline, rx).await {
                Ok(Ok(query)) => return Ok(query),
                // 连接未命中目标路径就结束了（或发送端被丢弃），继续等下一个。
                Ok(Err(_)) => continue,
                Err(_) => {
                    return Err(anyhow::anyhow!("等待回调超时（{}s）", timeout.as_secs()));
                }
            }
        }
    }
}

/// 单条回调连接：命中 `expect_path` 返回查询串，否则回 404（或直接丢弃）。
async fn handle_conn(mut sock: tokio::net::TcpStream, expect_path: &str) -> Option<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // 浏览器可能与 xmlns 预连接同开，读取超时只是避免长等，不设上限。
    let mut buf = vec![0u8; 8192];
    let n = match tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => return None,
    };
    if n == 0 {
        return None;
    }

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
        return None;
    }

    // 先回页再返回：保证浏览器拿到响应后能立刻结束「连接中」状态。
    let body = "<html><meta charset=\"utf-8\"><body>登录完成，可以关闭此页面。</body></html>";
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = sock.write_all(resp.as_bytes()).await;
    let _ = sock.shutdown().await;

    Some(query)
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// 未命中路径的连接（浏览器预连接/favicon）不得阻塞真正的回调。
    ///
    /// 旧实现串行处理连接：先挂着一条「发完请求头却不断开」的连接，真正的回调
    /// 就会排在它后面，直到 5s 读超时才被 accept——Windows 上表现为标签页一直
    /// 「连接中」。此处先制造这样一条连接，再发真回调，要求立刻拿到结果。
    #[tokio::test]
    async fn 静默连接不阻塞回调() {
        let server = CallbackServer::bind(0).await.expect("绑定端口");
        let port = server.port();

        // 打洞连接 1：发一条非目标路径请求，然后**保持连接不关闭**。
        let mut hang = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("连接");
        hang.write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .expect("写入");

        let waiter =
            tokio::spawn(async move { server.wait("/authorize", Duration::from_secs(5)).await });

        // 让上面那条连接先被 accept，确保它在处理中（复现串行阻塞的条件）。
        tokio::time::sleep(Duration::from_millis(200)).await;

        // 真正的回调。
        let mut cb = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("连接");
        cb.write_all(b"GET /authorize?code=abc&host=https%3A%2F%2Fapi.trae.cn HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .expect("写入");

        // 必须在远小于 5s 内返回。
        let query = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("回调被其它连接阻塞")
            .expect("任务未 panic")
            .expect("等待回调失败");
        assert_eq!(query_param(&query, "code").as_deref(), Some("abc"));

        // 浏览器侧应当收到 200 与完成页。
        let mut resp = String::new();
        cb.read_to_string(&mut resp).await.expect("读取响应");
        assert!(resp.starts_with("HTTP/1.1 200 OK"), "响应: {resp}");
        assert!(resp.contains("登录完成"), "响应: {resp}");

        drop(hang);
    }

    /// 回调端口在开浏览器前就应处于监听状态。
    #[tokio::test]
    async fn 绑定即可连() {
        let server = CallbackServer::bind(0).await.expect("绑定端口");
        let port = server.port();
        assert!(
            tokio::net::TcpStream::connect(("127.0.0.1", port))
                .await
                .is_ok()
        );
    }
}
