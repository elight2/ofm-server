//! MiniMax 登录：OAuth 2.0 设备码授权 + PKCE。
//!
//! 协议见 `provider-info/minimax.md` §1。**无法纯命令行完成**：设备码只是第一步，
//! 账号确认页需要微信扫码 + 绑定手机号短信验证（§0 出入 4）。本模块因此只做
//! 「打印 user_code / verification_uri_complete + 轮询令牌」，浏览器侧的扫码、
//! 短信与「确认授权」由用户手动完成。
//!
//! 流程：
//! 1. `POST /oauth2/device/code`（form）→ `device_code`/`user_code`/`interval`/`expires_in`；
//! 2. 打印 `verification_uri_complete`，提示用户去浏览器完成授权；
//! 3. 每 `interval` 秒 `POST /oauth2/token`（device_code grant）一次，直到拿到令牌
//!    或 `expires_in` 超时；
//! 4. 写入 `providers/minimax/credential.toml`。

use crate::core::text::truncate_chars;
use std::time::Duration;

use serde::Deserialize;

use super::api::{self, HttpError};
use super::credential::{self, Credential};

/// 设备码响应（§1 抓包）。
#[derive(Debug, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    /// 用户确认页（不带 user_code）。
    #[serde(default)]
    pub verification_uri: String,
    /// 可直接打开的完整确认页。
    #[serde(default)]
    pub verification_uri_complete: String,
    #[serde(default = "default_expires_in")]
    pub expires_in: u64,
    #[serde(default = "default_interval")]
    pub interval: u64,
}

fn default_expires_in() -> u64 {
    300
}

fn default_interval() -> u64 {
    3
}

/// 令牌响应（设备码成功与 refresh 共用形状，§1/§2）。
#[derive(Debug, Deserialize)]
pub struct TokenGrant {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<i64>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
}

impl TokenGrant {
    /// 换算到期时间戳（毫秒）。缺 `expires_in` 时退回 12h（§1 实测的 `expires_in`）。
    pub fn expires_at(&self) -> i64 {
        let secs = self.expires_in.unwrap_or(43_200);
        credential::now_millis() + secs * 1000
    }

    /// 令牌校验，规则见 §7.1 `invalid_token_response`。
    pub fn validate(&self) -> Result<(), String> {
        if self.access_token.trim().is_empty() {
            return Err("响应缺少 access_token".into());
        }
        if self
            .refresh_token
            .as_deref()
            .unwrap_or("")
            .trim()
            .is_empty()
        {
            return Err("响应缺少 refresh_token".into());
        }
        match self.token_type.as_deref() {
            Some(t) if t.eq_ignore_ascii_case("Bearer") => {}
            Some(other) => return Err(format!("token_type 非 Bearer: {other}")),
            None => return Err("响应缺少 token_type".into()),
        }
        match self.scope.as_deref() {
            Some(s) if s.split_whitespace().any(|x| x == api::SCOPE) => {}
            Some(other) => return Err(format!("scope 不含 {}: {other}", api::SCOPE)),
            None => return Err("响应缺少 scope".into()),
        }
        Ok(())
    }
}

/// 请求设备码。
pub async fn request_device_code(
    client: &reqwest::Client,
    challenge: &str,
) -> Result<DeviceCode, HttpError> {
    let url = format!("{}{}", api::ACCOUNT_HOST, api::DEVICE_CODE_PATH);
    let form = [
        ("client_id", api::CLIENT_ID),
        ("scope", api::SCOPE),
        ("audience", api::AUDIENCE),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];
    api::post_form_json(client, &url, &form).await
}

/// 一次设备码轮询的结果。
#[derive(Debug)]
pub enum PollOutcome {
    /// 已拿到令牌。
    Granted(Box<TokenGrant>),
    /// 未授权，继续等。
    Pending,
    /// 要求放慢轮询（间隔 +5s）。
    SlowDown,
    /// 用户拒绝。
    Denied,
    /// 设备码已过期。
    Expired,
}

/// 只比较结果类别（`Granted` 内部的令牌不参与比较）。
impl PartialEq for PollOutcome {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (PollOutcome::Granted(_), PollOutcome::Granted(_))
                | (PollOutcome::Pending, PollOutcome::Pending)
                | (PollOutcome::SlowDown, PollOutcome::SlowDown)
                | (PollOutcome::Denied, PollOutcome::Denied)
                | (PollOutcome::Expired, PollOutcome::Expired)
        )
    }
}

impl Eq for PollOutcome {}

/// 解析一次轮询的响应。
///
/// 上游用 **HTTP 400 + `error`** 表示待授权（§1 轮询分支），另有兼容实现用
/// **HTTP 200 + `status`**。两种都要认。
pub fn classify(status: u16, body: &str) -> PollOutcome {
    let json: Option<serde_json::Value> = serde_json::from_str(body).ok();
    let get = |k: &str| -> Option<String> {
        json.as_ref()
            .and_then(|v| v.get(k))
            .and_then(|v| v.as_str())
            .map(str::to_string)
    };

    // 兼容形态：HTTP 200 + status 标记。
    if let Some(s) = get("status") {
        match s.as_str() {
            "pending" => return PollOutcome::Pending,
            "slow_down" => return PollOutcome::SlowDown,
            "denied" => return PollOutcome::Denied,
            "expired" => return PollOutcome::Expired,
            _ => {}
        }
    }

    if (200..300).contains(&status) {
        return match serde_json::from_str::<TokenGrant>(body) {
            Ok(g) if !g.access_token.trim().is_empty() => PollOutcome::Granted(Box::new(g)),
            _ => PollOutcome::Expired,
        };
    }

    match get("error").as_deref() {
        Some("authorization_pending") => PollOutcome::Pending,
        Some("slow_down") => PollOutcome::SlowDown,
        Some("access_denied") => PollOutcome::Denied,
        Some("expired_token") => PollOutcome::Expired,
        other => {
            tracing::warn!(?other, body = truncate_chars(body, 200), "未识别的轮询响应");
            PollOutcome::Expired
        }
    }
}

/// 轮询令牌直到成功、失败或超时。
///
/// `interval` 累计 `slow_down` 增量；截止时间为收到设备码时刻 + `expires_in`。
pub async fn poll_token(
    client: &reqwest::Client,
    device: &DeviceCode,
    verifier: &str,
) -> anyhow::Result<TokenGrant> {
    let url = format!("{}{}", api::ACCOUNT_HOST, api::TOKEN_PATH);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(device.expires_in.max(1));
    let mut interval = device.interval.max(1);
    let mut polls = 0u32;

    loop {
        // 先睡再问：上游要求按 interval 轮询（实测 3.04~3.08s）。
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("设备码已过期（{}s），请重新登录", device.expires_in);
        }

        let form = [
            ("grant_type", api::GRANT_DEVICE_CODE),
            ("device_code", device.device_code.as_str()),
            ("client_id", api::CLIENT_ID),
            ("code_verifier", verifier),
        ];
        let resp = client
            .post(&url)
            .header("accept", "application/json")
            .form(&form)
            .send()
            .await;
        let (status, body) = match resp {
            Ok(r) => {
                let s = r.status().as_u16();
                (s, r.text().await.unwrap_or_default())
            }
            Err(e) => {
                // 网络抖动不应中断整个登录流程，记一条继续轮询。
                tracing::warn!("轮询令牌失败（将重试）: {e}");
                continue;
            }
        };

        polls += 1;
        match classify(status, &body) {
            PollOutcome::Granted(g) => return Ok(*g),
            PollOutcome::Pending => {
                // 每 10 次（约 30s）提示一次，让用户知道还在等。
                if polls.is_multiple_of(10) {
                    println!("[minimax] 仍在等待授权（已查询 {polls} 次）…");
                }
                tracing::debug!(status, polls, "轮询: 尚未授权");
                continue;
            }
            PollOutcome::SlowDown => {
                interval += api::SLOW_DOWN_INCREMENT_SECS;
                tracing::debug!("上游要求放慢轮询，间隔调整为 {interval}s");
            }
            PollOutcome::Denied => anyhow::bail!("用户拒绝了授权"),
            PollOutcome::Expired => {
                anyhow::bail!("设备码已过期或授权失败，请重新登录")
            }
        }
    }
}

/// 尝试打开浏览器，失败只提示不中断。
///
/// `login_util::open_browser` 只检查 `spawn()` 是否成功，在**无图形会话**
/// （`XDG_SESSION_TYPE=tty`、无 `DISPLAY`/`WAYLAND_DISPLAY`）下 `xdg-open`
/// 能被启动但什么都不会打开，于是它会误报成功。这里先自己判一次环境，
/// 避免用户误以为「已经打开浏览器了」。
fn open_browser_safely(url: &str) {
    // Windows 没有 DISPLAY/WAYLAND_DISPLAY，但只要是交互式会话就能开浏览器。
    #[cfg(windows)]
    let has_display = true;
    #[cfg(not(windows))]
    let has_display = std::env::var("WAYLAND_DISPLAY").is_ok_and(|v| !v.is_empty())
        || std::env::var("DISPLAY").is_ok_and(|v| !v.is_empty());
    if !has_display {
        println!("[minimax] 当前会话没有图形环境，未尝试打开浏览器，请手动复制上面的链接");
        return;
    }
    if let Err(e) = crate::login_util::open_browser(url) {
        eprintln!("[minimax] 打开浏览器失败（{e}），请手动访问上面的链接");
    }
}

/// 登录入口。
pub async fn run(root: std::path::PathBuf) -> anyhow::Result<()> {
    let client = api::client()?;
    let (verifier, challenge) = api::pkce_pair();

    let device = request_device_code(&client, &challenge)
        .await
        .map_err(|e| anyhow::anyhow!("请求设备码失败: {e}"))?;

    let url = if device.verification_uri_complete.is_empty() {
        format!("{}?user_code={}", device.verification_uri, device.user_code)
    } else {
        device.verification_uri_complete.clone()
    };

    println!("[minimax] 请在浏览器打开下面的链接完成授权：");
    println!("  {url}");
    println!("[minimax] 用户码: {}", device.user_code);
    println!(
        "[minimax] 有效期 {}s，每 {}s 查询一次授权状态…",
        device.expires_in, device.interval
    );
    open_browser_safely(&url);

    let grant = poll_token(&client, &device, &verifier).await?;
    grant
        .validate()
        .map_err(|e| anyhow::anyhow!("令牌校验失败: {e}"))?;

    let expires_at = grant.expires_at();
    let cred = Credential {
        access_token: grant.access_token,
        refresh_token: grant.refresh_token.unwrap_or_default(),
        expires_at,
    };
    cred.store(&root)?;

    println!(
        "[minimax] 登录成功，凭证已保存到 {}",
        crate::core::paths::credential_path(&root, credential::PROVIDER).display()
    );
    println!(
        "[minimax] 访问令牌有效期至 {}",
        crate::core::fmt_ms(cred.expires_at)
    );
    Ok(())
}

/// 用 `refresh_token` 续期（§2）。返回新令牌，调用方负责落盘。
///
/// **RT 单次使用 + 轮换**：成功一次后旧 RT 立即 `invalid_grant`，
/// 所以调用方必须串行且必须落盘（见 [`super::MinimaxProvider::refresh_locked`]）。
pub async fn refresh(
    client: &reqwest::Client,
    refresh_token: &str,
) -> Result<TokenGrant, HttpError> {
    let url = format!("{}{}", api::ACCOUNT_HOST, api::TOKEN_PATH);
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", api::CLIENT_ID),
        ("scope", api::SCOPE),
        ("audience", api::AUDIENCE),
    ];
    api::post_form_json(client, &url, &form).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 待授权是_400_pending() {
        let body = r#"{"error":"authorization_pending","error_description":"the user has not finished authorizing yet"}"#;
        assert_eq!(classify(400, body), PollOutcome::Pending);
    }

    #[test]
    fn slow_down_与拒绝与过期() {
        assert_eq!(
            classify(400, r#"{"error":"slow_down"}"#),
            PollOutcome::SlowDown
        );
        assert_eq!(
            classify(400, r#"{"error":"access_denied"}"#),
            PollOutcome::Denied
        );
        assert_eq!(
            classify(400, r#"{"error":"expired_token"}"#),
            PollOutcome::Expired
        );
    }

    #[test]
    fn 兼容_200_带_status() {
        assert_eq!(
            classify(200, r#"{"status":"pending"}"#),
            PollOutcome::Pending
        );
        assert_eq!(
            classify(200, r#"{"status":"slow_down"}"#),
            PollOutcome::SlowDown
        );
        assert_eq!(classify(200, r#"{"status":"denied"}"#), PollOutcome::Denied);
        assert_eq!(
            classify(200, r#"{"status":"expired"}"#),
            PollOutcome::Expired
        );
    }

    #[test]
    fn 成功响应被判为已授权() {
        let body = r#"{"access_token":"at","expires_in":43200,"refresh_token":"rt","scope":"agent.default","token_type":"Bearer"}"#;
        match classify(200, body) {
            PollOutcome::Granted(g) => {
                assert_eq!(g.access_token, "at");
                assert_eq!(g.refresh_token.as_deref(), Some("rt"));
                assert!(g.validate().is_ok());
            }
            other => panic!("应判为已授权，实际 {other:?}"),
        }
    }

    #[test]
    fn 校验拒绝不完整令牌() {
        let g = TokenGrant {
            access_token: "at".into(),
            refresh_token: Some("rt".into()),
            expires_in: Some(1),
            scope: Some("other.scope".into()),
            token_type: Some("Bearer".into()),
        };
        assert!(g.validate().is_err());

        let g = TokenGrant {
            access_token: "at".into(),
            refresh_token: Some("rt".into()),
            expires_in: Some(1),
            scope: Some("agent.default".into()),
            token_type: Some("bearer".into()),
        };
        assert!(g.validate().is_ok(), "token_type 大小写不敏感");
    }

    #[test]
    fn expires_at_缺省取十二小时() {
        let g = TokenGrant {
            access_token: "a".into(),
            refresh_token: None,
            expires_in: None,
            scope: None,
            token_type: None,
        };
        let now = credential::now_millis();
        let at = g.expires_at();
        assert!(at > now + 43_000_000 && at < now + 43_300_000);
    }

    #[test]
    fn 设备码响应字段可解析() {
        let body = r#"{"device_code":"dc","expires_in":300,"interval":3,
            "user_code":"WVC4-3DE3",
            "verification_uri":"https://account.minimax.cn/oauth-authorize",
            "verification_uri_complete":"https://account.minimax.cn/oauth-authorize?user_code=WVC4-3DE3"}"#;
        let d: DeviceCode = serde_json::from_str(body).unwrap();
        assert_eq!(d.user_code, "WVC4-3DE3");
        assert_eq!(d.interval, 3);
        assert_eq!(d.expires_in, 300);
    }
}
