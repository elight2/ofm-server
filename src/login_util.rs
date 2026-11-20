//! 登录流程的公共部件：本地回调服务、打开浏览器、查询串解析。
//!
//! 放在顶层而非 `cli`：供应商实现（`providers::<id>::auth`）需要这些能力，
//! 不应反向依赖 CLI 层。`cli::login` 只负责按供应商分发。

use std::time::Duration;

/// 回调完成页（浏览器侧唯一可见的提示）。
pub const COMPLETE_PAGE: &str =
    "<html><meta charset=\"utf-8\"><body>登录完成，可以关闭此页面。</body></html>";

/// 等待浏览器回调的默认上限（5 分钟，与各家官方客户端一致）。
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);

/// 用系统默认浏览器打开 URL。
///
/// Windows 走 `cmd /C start "" <url>`（不为此引入 winapi 依赖）。这里有两个陷阱，
/// 都必须用 `raw_arg` 自行拼命令行才能同时躲开：
///
/// 1. `start` 会按 `&` / `|` / `^` 重新切分收到的命令行，而授权 URL 的参数就是用
///    `&` 连接的。必须先给它加上引号，否则浏览器只打开第一个 `&` 之前的截断 URL，
///    其余片段被逐条当命令执行（报错被丢弃），其中末段的
///    `http://127.0.0.1:<回调端口>/<回调路径>` 还会额外弹出一个访问本地回调端口的
///    标签页（历史症状：「浏览器最后一步卡在连接」）。
/// 2. 但**不能**把加了引号的字符串当成普通 `.arg()` 传：`.arg()` 走 Rust 的 Windows
///    参数转义，引号会被再转义成 `\"`。而 `std::process::Command` 平时只给「含空白
///    或引号」的参数加引号，本项目拼出的 URL 把空格编码为 `%20`，本来不触发，一旦
///    手工加引号反而触发二次转义，`cmd` 收到 `"https://…"`（带字面反斜杠），报
///    「Windows 找不到文件 xxx」。
///
/// 故用 `raw_arg` 原样传 `/C start "" "<url>"`：引号只出现在我们亲手拼的串里，
/// 不再经 `.arg()` 的转义。已用 wine 验证 `cmd` 收到的 URL 与输入逐字节相同
/// （`%20`/`%3A`/`%2F` 均未被展开）。
pub fn open_browser(url: &str) -> anyhow::Result<()> {
    if cfg!(target_os = "windows") {
        return open_browser_windows(url);
    }
    if cfg!(target_os = "macos") {
        return spawn_first(&[("open", &[url])]);
    }
    spawn_first(&[("xdg-open", &[url])])
}

/// `cmd /C` 后接的命令行（`start` + 空标题 + 加引号的 URL）。
///
/// 单独成函以便测试锁住拼接形状；见 [`open_browser`] 的两个陷阱说明。
/// 非 Windows 平台只用测试引用，故允许 dead_code。
#[cfg_attr(not(windows), allow(dead_code))]
fn windows_start_cmdline(url: &str) -> String {
    format!("/C start \"\" \"{url}\"")
}

/// Windows 的 `start` 启动器：见 [`open_browser`] 的两个陷阱说明。
#[cfg(windows)]
fn open_browser_windows(url: &str) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;

    std::process::Command::new("cmd")
        .raw_arg(windows_start_cmdline(url))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("启动浏览器失败: {e}"))
}

/// 非 Windows 平台的占位，保证 `cfg!(target_os = "windows")` 分支在其它平台上也能编译。
#[cfg(not(windows))]
fn open_browser_windows(_url: &str) -> anyhow::Result<()> {
    anyhow::bail!("内部错误：Windows 分支在非 Windows 平台上被调用")
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

/// 已绑定的一行 HTTP 请求（回调）。
pub struct HttpRequest {
    /// 请求方法，如 `GET`。
    pub method: String,
    /// 路径部分（不含查询串）。
    pub path: String,
    /// 查询串（不含 `?`），无查询串时为空。
    pub query: String,
    /// `Host` 头，缺失时为空串。
    pub host: String,
}

/// 从回调连接读一行 HTTP 请求并解析。
///
/// 读取超时 5s 只是避免长等（浏览器会先发 favicon / 预连接），
/// 解析不出来时返回 `None` 交给调用方忽略。
pub async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<HttpRequest> {
    use tokio::io::AsyncReadExt;

    let mut buf = vec![0u8; 8192];
    let n = match tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        _ => return None,
    };
    if n == 0 {
        return None;
    }

    let request = String::from_utf8_lossy(&buf[..n]);
    let mut lines = request.lines();
    let first_line = lines.next().unwrap_or_default();
    let mut parts = first_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or("/");

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target.to_string(), String::new()),
    };
    let host = lines
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("host").then(|| v.trim().to_string())
        })
        .unwrap_or_default();

    Some(HttpRequest {
        method,
        path,
        query,
        host,
    })
}

/// 回一个 `200 OK`（含提示页或空体）并关闭连接。
pub async fn respond_ok(sock: &mut tokio::net::TcpStream, body: &str) {
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    write_and_close(sock, &resp).await;
}

/// 回一个只有状态码的空响应。
pub async fn respond_status(
    sock: &mut tokio::net::TcpStream,
    status: u16,
    reason: &str,
    body: &str,
) {
    let resp = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    write_and_close(sock, &resp).await;
}

/// 回一个 CORS 预检响应（`OPTIONS` 请求）。
///
/// 部分门户的登录页会先 `fetch` 回调地址做探测，浏览器此时发 `OPTIONS` 预检；
/// 必须给出 `Access-Control-Allow-*`，否则真正的跳转会被 CORS 拦下。
pub async fn respond_preflight(sock: &mut tokio::net::TcpStream) {
    write_and_close(
        sock,
        "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\n\
Access-Control-Allow-Methods: GET, OPTIONS\r\n\
Access-Control-Allow-Headers: Content-Type\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    )
    .await;
}

async fn write_and_close(sock: &mut tokio::net::TcpStream, text: &str) {
    use tokio::io::AsyncWriteExt;

    let _ = sock.write_all(text.as_bytes()).await;
    let _ = sock.shutdown().await;
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
    /// 绑定 `127.0.0.1:<port>`（`port` 为 `0` 时由系统分配）。
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
        let (query, mut sock) = self.wait_with(expect_path, timeout, |_, _| true).await?;
        // 调用方不需要再说什么时，这里直接把浏览器放掉，免得标签页一直转圈。
        respond_ok(&mut sock, COMPLETE_PAGE).await;
        Ok(query)
    }

    /// 同 [`CallbackServer::wait`]，但额外做来源校验，并把连接留给调用方收尾。
    ///
    /// `check` 返回 `false` 时丢弃该连接（例如只接受 loopback 来源、
    /// 或 `Host` 必须等于本地 authority 以防 DNS rebinding）。
    /// 命中时连同**尚未响应的连接**一起返回，由调用方决定回什么（
    /// codearts 要在换到令牌后才回 307，好让失败时门户显示失败页）。
    pub async fn wait_with<F>(
        self,
        expect_path: &str,
        timeout: Duration,
        check: F,
    ) -> anyhow::Result<(String, tokio::net::TcpStream)>
    where
        F: Fn(&HttpRequest, std::net::SocketAddr) -> bool + Send + Sync + 'static,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        // `spawn` 要求 `'static`，目标路径很短且每家只有一个，克隆一份给后台任务。
        let expect = expect_path.to_string();
        let check = std::sync::Arc::new(check);

        loop {
            let (mut sock, peer) = tokio::time::timeout_at(deadline, self.listener.accept())
                .await
                .map_err(|_| anyhow::anyhow!("等待回调超时（{}s）", timeout.as_secs()))??;

            // 后台处理单条连接：只关心是否命中目标路径，命中就把查询串送回来。
            // 故意不等待 handle 结束——未命中的连接（favicon、预连接）要能自生自灭。
            let (tx, rx) = tokio::sync::oneshot::channel();
            let expect = expect.clone();
            let check = check.clone();
            tokio::spawn(async move {
                let Some(req) = read_request(&mut sock).await else {
                    return;
                };
                // 预检不算命中：回 CORS 头后继续等真正的 GET。
                if req.method == "OPTIONS" {
                    respond_preflight(&mut sock).await;
                    return;
                }
                if req.path != expect || !check(&req, peer) {
                    respond_status(&mut sock, 404, "Not Found", "").await;
                    return;
                }
                let _ = tx.send((req.query, sock));
            });

            match tokio::time::timeout_at(deadline, rx).await {
                Ok(Ok(hit)) => return Ok(hit),
                // 连接未命中目标路径就结束了（或发送端被丢弃），继续等下一个。
                Ok(Err(_)) => continue,
                Err(_) => {
                    return Err(anyhow::anyhow!("等待回调超时（{}s）", timeout.as_secs()));
                }
            }
        }
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

    /// `wait_with` 的校验不通过时必须继续等，而不是把连接当命中。
    #[tokio::test]
    async fn 校验失败继续等待() {
        let server = CallbackServer::bind(0).await.expect("绑定端口");
        let port = server.port();

        let waiter = tokio::spawn(async move {
            server
                .wait_with("/cb", Duration::from_secs(5), |req, _| {
                    // 只接受带 `ok=1` 的回调。
                    req.query.contains("ok=1")
                })
                .await
        });
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 先发一条不满足校验的。
        let mut bad = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("连接");
        bad.write_all(b"GET /cb?ok=0 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .expect("写入");
        let mut resp = String::new();
        bad.read_to_string(&mut resp).await.expect("读取响应");
        assert!(resp.starts_with("HTTP/1.1 404"), "响应: {resp}");

        // 再发一条满足的。
        let mut good = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .expect("连接");
        good.write_all(b"GET /cb?ok=1 HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .await
            .expect("写入");

        let (query, mut sock) = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("未命中校验失败")
            .expect("任务未 panic")
            .expect("等待回调失败");
        assert_eq!(query, "ok=1");
        // 连接留给调用方收尾。
        respond_ok(&mut sock, "done").await;
    }

    /// Windows 的 `start` 命令行必须用 `raw_arg` 原样拼、引号只出现一层。
    ///
    /// 回归点：曾把手动加引号的 URL 当普通 `.arg()` 传，Rust 二次转义成 `\"`，
    /// cmd 得到带字面反斜杠引号的无效路径，报「Windows 找不到文件 xxx」。
    #[test]
    fn windows_start命令行只有一层引号() {
        let url = "https://www.trae.cn/authorization?login_version=1&auth_from=trae\
&x_os_version=Arch%20Linux&code_challenge=abc";
        let line = super::windows_start_cmdline(url);
        // 必须恰好是 `/C start "" "<url>"`，且 `&` 落在引号内。
        assert_eq!(line, format!("/C start \"\" \"{url}\""));
        // 关键：不得出现 `\"`（Rust 二次转义后的形态）。
        assert!(!line.contains("\\\""), "命令行里出现反斜杠转义引号: {line}");
        // `&` 全部落在引号内：去掉包住 URL 的那对引号后，内容应与 URL 完全相同。
        assert!(
            line.ends_with(&format!("\"{url}\"")),
            "URL 未被引号完整包住: {line}"
        );
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
