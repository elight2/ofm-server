//! benefit-service「权益领取 / 余额 / 网关配置」复刻。
//!
//! 官方有**两套**实现（`provider-info/codearts.md` §7），端点相同、调用方式不同：
//!
//! | 来源 | 流程 |
//! |-|-|
//! | agent kernel（`aD`/`_D`） | `POST claim`（body `{}`，无 channel） |
//! | IDE 扩展（`SLc`/`Pce`） | `GET claim`，分 `benefit`/`education`/`examination` 三个渠道 |
//!
//! # 端点
//!
//! ```text
//! GET  {benefitBase}/api/v1/gateway/config       （渠道配置，含福利模型表）
//! GET  {benefitBase}/api/v1/benefit/claim        （签到，channel 走 query）
//! POST {benefitBase}/api/v1/benefit/claim        （启用签到渠道，body {channel?}）
//! GET  {benefitBase}/api/v1/user/tokens/balance  （余额）
//! ```
//!
//! `benefitBase` 生产固定 `https://opengw.developer.huaweicloud.com`
//! （`snapengine-apig.cn-north-7` 仅 HC 内部环境）。
//!
//! # 头与签名
//!
//! `Content-Type: application/json` + `x-snap-traceid`（uuid 无横线）+
//! `X-Security-token` → SDK-HMAC-SHA256（`sign` 模块）。
//! IDE 侧走 `x-auth-token` 而非 AKSK；server 只实现 AKSK 形态（实测两种形态等价）。
//!
//! # 语义（实测 2026-02-18 / 2026-11-16）
//!
//! 成功判定 `error_code === "0000"`（HTTP 恒 200，业务码在 body）。
//! 本账号对四个端点**一律**返回
//! `{"error_code":"4000","error_msg":"invalid channel, group not found"}`，
//! 与 method / channel / body 无关——说明权益渠道未开通，不是签到逻辑错误。
//!
//! 错误码：`4000` 参数错误、`4001` token 缺失/无效（**缺 `X-Security-Token` 时
//! AKSK 会报 `401 APIG.0301 access key not found`**）、`4002` token 过期、
//! `4003` 用户禁用、`4004` 权益不存在/user 不存在、`4006` 每日名额耗尽、
//! `5000` internal。
//!
//! 余额字段名按 webview 渲染层取证：`used_amount` / `total_quota`
//! （education 为 `monthly_tokens_used` / `monthly_token_limit`）。
//! 对外展示取「剩余额度 = total − used」；字段缺失返回 `None`。

use serde::Deserialize;

use crate::core::provider::{ClaimReport, ProviderError};
use crate::providers::codearts::credential::Credential;
use crate::providers::codearts::pkce;
use crate::providers::codearts::sign;

/// benefit 网关生产域名（内核 `pFn` / 扩展 `Dce()` 生产分支）。
pub const BASE_URL: &str = "https://opengw.developer.huaweicloud.com";
/// 端点前缀。生产恒为 [`BASE_URL`]；仅在单元测试里替换为本地 mock。
const ENV_BASE_URL: &str = "OFM_CODEARTS_BENEFIT_BASE";
/// HC 内部域名（扩展 `Oz()` gamma 版 / 内网构建 `IS_PROD!=="true"`）。
/// 公网不可达，仅为对照保留。
#[allow(dead_code)]
pub const BASE_URL_INTERNAL: &str = "https://snapengine-apig.cn-north-7.myhuaweicloud.com";

/// 渠道配置。
pub const CONFIG_PATH: &str = "/api/v1/gateway/config";
/// 签到（GET 领取 / POST 启用）。
pub const CLAIM_PATH: &str = "/api/v1/benefit/claim";
/// 余额。
pub const BALANCE_PATH: &str = "/api/v1/user/tokens/balance";

/// 签到渠道（IDE `SLc` 的三个渠道；kernel 只走 `benefit`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Benefit,
    Education,
    Examination,
}

impl Channel {
    pub const ALL: [Channel; 3] = [Channel::Benefit, Channel::Education, Channel::Examination];

    /// 线上取值：`benefit` 渠道**不带** channel 参数（官方 GET 与 POST 均不带）。
    /// 另两个渠道以 `channel=<值>` 走 query（IDE 传 `params`）。
    fn query(self) -> Option<&'static str> {
        match self {
            Channel::Benefit => None,
            Channel::Education => Some("channel=education"),
            Channel::Examination => Some("channel=examination"),
        }
    }

    /// POST 启用时的 body（`{"channel":"education"}`；benefit 用 `{}`）。
    fn enable_body(self) -> &'static str {
        match self {
            Channel::Benefit => "{}",
            Channel::Education => r#"{"channel":"education"}"#,
            Channel::Examination => r#"{"channel":"examination"}"#,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Channel::Benefit => "benefit",
            Channel::Education => "education",
            Channel::Examination => "examination",
        }
    }
}

/// 请求超时（内核 `setTimeout(…, 1e4)`）。
const TIMEOUT_SECS: u64 = 10;

/// benefit 统一响应壳。错误字段实测为 `error_msg`。
#[derive(Debug, Deserialize)]
pub struct Envelope {
    pub error_code: Option<String>,
    pub error_msg: Option<String>,
    pub result: Option<serde_json::Value>,
}

impl Envelope {
    /// 成功判定（官方 `error_code === "0000"`），取 `result`。
    fn into_result(self) -> Result<serde_json::Value, ProviderError> {
        match self.error_code.as_deref() {
            Some("0000") => Ok(self.result.unwrap_or(serde_json::Value::Null)),
            Some(code) => Err(self.error(code)),
            None => Err(ProviderError::Protocol(
                "benefit: 响应缺 error_code".to_string(),
            )),
        }
    }

    /// 业务错误 → `ProviderError`，并给 `4000` 补上可读解释。
    fn error(&self, code: &str) -> ProviderError {
        let msg = self.error_msg.clone().unwrap_or_default();
        match code {
            "4000" => ProviderError::Protocol(format!(
                "benefit: 4000 {msg}（账号未开通该权益渠道，签到/余额不可用）"
            )),
            "4001" => ProviderError::Unauthorized(format!("benefit: 4001 {msg}")),
            "4002" => ProviderError::Unauthorized(format!("benefit: 4002 {msg}（凭证过期）")),
            "4003" => ProviderError::Other(format!("benefit: 4003 {msg}（用户被禁用）")),
            "4004" => ProviderError::Protocol(format!("benefit: 4004 {msg}（权益不存在）")),
            "4006" => ProviderError::Other(format!("benefit: 4006 {msg}（今日名额耗尽）")),
            _ => ProviderError::Protocol(format!("benefit: {code} {msg}")),
        }
    }
}

/// 签到结果。
#[derive(Debug)]
pub enum ClaimOutcome {
    /// `0000`：领取成功（今日尚未领取）。
    Claimed,
    /// `4006`：今日已领。
    Already,
    /// `4000`：渠道未开通，跳过（不是故障）。
    NotProvisioned,
}

/// 余额 `result`（字段名按 webview 渲染层取证）。
#[derive(Debug, Default, Deserialize)]
pub struct BalanceResult {
    #[serde(default)]
    pub used_amount: Option<f64>,
    #[serde(default)]
    pub total_quota: Option<f64>,
    /// education 渠道字段；benefit/examination 不出现，防御性兼容。
    #[serde(default)]
    pub monthly_tokens_used: Option<f64>,
    #[serde(default)]
    pub monthly_token_limit: Option<f64>,
    /// 渠道标记（`benefit`/`education`/`examination`），仅存档不参与计算。
    #[allow(dead_code)]
    #[serde(default)]
    pub maas_type: Option<String>,
}

impl BalanceResult {
    /// 剩余额度 = 总量 − 已用（两字段齐才可算，缺一返回 `None`）。
    fn remaining(&self) -> Option<f64> {
        let used = self.used_amount.or(self.monthly_tokens_used)?;
        let total = self.total_quota.or(self.monthly_token_limit)?;
        Some(total - used)
    }
}

/// 构造签名后的头（复刻内核 `PFn`）；POST body 参与签名。
///
/// `X-Security-token` 必须在**签名前**加入头集合：官方签名器对
/// `SignedHeaders` 内的每个键都重算值，缺该头会被网关按 AK 校验失败
/// （实测 `401 APIG.0301 ... access key is not found`）。
fn signed_headers(
    url: &str,
    method: &str,
    cred: &Credential,
    body: Option<&str>,
) -> Result<Vec<(String, String)>, ProviderError> {
    let mut headers = vec![
        ("content-type", "application/json".to_string()),
        ("x-snap-traceid", pkce::uuid_v4().replace('-', "")),
    ];
    if !cred.security_token.is_empty() {
        headers.push(("x-security-token", cred.security_token.clone()));
    }
    let req = sign::Request {
        method,
        url,
        headers,
        query: None,
        body,
    };
    let mut signed = sign::sign(&req, &cred.access_key_id, &cred.secret_access_key)
        .map_err(|e| ProviderError::Other(format!("AKSK 签名失败: {e}")))?;
    // 签名器返回的是小写键，改用线上惯用写法发送（签名与发送头一一对应）。
    for (k, _) in signed.iter_mut() {
        if k == "x-security-token" {
            *k = "X-Security-token".to_string();
        }
    }
    Ok(signed)
}

/// benefit 请求公共部分：签名头 + 超时 + 状态码/壳解析。
///
/// body 以序列化后的文本传入并原样发送（签名与实际报文一致，避免 serde
/// 二次序列化差异导致 payload hash 不符）。
async fn send(
    client: &reqwest::Client,
    cred: &Credential,
    base: &str,
    method: reqwest::Method,
    path: &str,
    query: Option<&str>,
    body: Option<&str>,
) -> Result<Envelope, ProviderError> {
    let base = std::env::var(ENV_BASE_URL)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| base.to_string());
    let base = base.as_str();
    let mut url = format!("{base}{path}");
    if let Some(q) = query {
        url.push('?');
        url.push_str(q);
    }
    let unsigned_url = format!("{base}{path}");
    // 说明：`url` 带 query（channel 等），签名只按 `base+path` 计算 canonical URI。
    //
    // 官方 `y2i` 的 `HttpRequest` 不解析 query，canonical query 恒为空——但
    // **"签不签 query" 对本次成功的请求无关**：实测两种写法（canonical query 留空 /
    // 把 `channel=…` 编码进 canonical query）都返回 `error_code 0000/4000`，
    // 而发出去的 HTTP 报文完全相同（线上 `Authorization` 的 SignedHeaders 不含
    // query）。因此这里保持与官方一致的「query 不参与签名」，不用改签名器。
    let headers = signed_headers(&unsigned_url, method.as_str(), cred, body)?;
    let mut http = client
        .request(method, &url)
        .timeout(std::time::Duration::from_secs(TIMEOUT_SECS));
    for (k, v) in &headers {
        http = http.header(k.as_str(), v.as_str());
    }
    // 注意：content-type 已在签名头里，不能再补一次（重复头会导致服务端
    // 签名重算值不符 → APIG.0301）。body 用与签名相同的文本原样发送。
    if let Some(t) = body {
        http = http.body(t.to_string());
    }
    let resp = http.send().await?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status) {
        return Err(ProviderError::Protocol(format!(
            "benefit: HTTP {status}: {}",
            truncate(&text)
        )));
    }
    serde_json::from_str(&text).map_err(|e| {
        ProviderError::Protocol(format!("benefit: 解析失败: {e}: {}", truncate(&text)))
    })
}

/// 网关配置（`GET /api/v1/gateway/config`，可带 channel）。未开通时返回 `Ok(None)`。
#[allow(dead_code)]
pub async fn fetch_config(
    client: &reqwest::Client,
    cred: &Credential,
    channel: Channel,
) -> Result<Option<serde_json::Value>, ProviderError> {
    let env = send(
        client,
        cred,
        BASE_URL,
        reqwest::Method::GET,
        CONFIG_PATH,
        channel.query(),
        None,
    )
    .await?;
    match env.error_code.as_deref() {
        Some("0000") => Ok(env.result),
        Some("4000") | Some("4004") => {
            tracing::debug!("[codearts] {} 渠道未开通: {:?}", channel.label(), env.error_msg);
            Ok(None)
        }
        Some(code) => Err(env.error(code)),
        None => Err(ProviderError::Protocol(
            "benefit: 配置响应缺 error_code".to_string(),
        )),
    }
}

/// 单渠道签到（IDE `SLc` 的 GET + 启用 POST）。
///
/// 顺序与官方一致：`GET claim?channel=…` 领取 → 成功后 `POST claim {channel}` 启用渠道
/// → 读余额。`benefit` 渠道的 GET/POST 均不带 channel。
pub async fn claim_channel(
    client: &reqwest::Client,
    cred: &Credential,
    channel: Channel,
) -> Result<ClaimOutcome, ProviderError> {
    let env = send(
        client,
        cred,
        BASE_URL,
        reqwest::Method::GET,
        CLAIM_PATH,
        channel.query(),
        None,
    )
    .await?;
    match env.error_code.as_deref() {
        Some("0000") => {}
        Some("4006") => return Ok(ClaimOutcome::Already),
        Some("4000") => return Ok(ClaimOutcome::NotProvisioned),
        Some(code) => return Err(env.error(code)),
        None => {
            return Err(ProviderError::Protocol(
                "benefit: claim 响应缺 error_code".to_string(),
            ))
        }
    }

    // 领取成功 → 启用该渠道（benefit 用 `{}`，教育/考试渠道带 channel 字段）。
    enable(client, cred, channel, channel.enable_body()).await?;
    Ok(ClaimOutcome::Claimed)
}

/// `POST /api/v1/benefit/claim`（启用渠道）。失败只记日志，不影响「已领取」的结论。
async fn enable(
    client: &reqwest::Client,
    cred: &Credential,
    channel: Channel,
    body: &str,
) -> Result<(), ProviderError> {
    let env = send(
        client,
        cred,
        BASE_URL,
        reqwest::Method::POST,
        CLAIM_PATH,
        None,
        Some(body),
    )
    .await?;
    match env.error_code.as_deref() {
        Some("0000") => Ok(()),
        Some(code) => {
            tracing::warn!(
                "[codearts] 启用权益渠道 {} 失败: {code} {:?}（签到结果不受影响）",
                channel.label(),
                env.error_msg
            );
            Ok(())
        }
        None => Ok(()),
    }
}

/// 签到（遍历三个渠道，汇总成 `ClaimReport`）。
///
/// * `4006` → 今日已签（`skipped`）
/// * `4000` → 渠道未开通（`skipped`，不是故障）
/// * 全部渠道都未开通时才返回错误，便于 `status`/CLI 显示原因
pub async fn claim(client: &reqwest::Client, cred: &Credential) -> Result<ClaimReport, ProviderError> {
    let mut claimed = Vec::new();
    let mut already = Vec::new();
    let mut missing = Vec::new();
    let mut first_error: Option<ProviderError> = None;

    for channel in Channel::ALL {
        match claim_channel(client, cred, channel).await {
            Ok(ClaimOutcome::Claimed) => claimed.push(channel.label()),
            Ok(ClaimOutcome::Already) => already.push(channel.label()),
            Ok(ClaimOutcome::NotProvisioned) => missing.push(channel.label()),
            Err(e) => {
                tracing::warn!("[codearts] {} 渠道签到失败: {e}", channel.label());
                first_error.get_or_insert(e);
            }
        }
    }

    let balance = fetch_balance(client, cred).await.ok().flatten();
    let message = claim_message(&claimed, &already, &missing);

    if claimed.is_empty() && already.is_empty() {
        if let Some(e) = first_error {
            return Err(e);
        }
        // 全部渠道未开通：不是故障，但要让调用方知道今天没领到任何东西。
        return Ok(ClaimReport {
            skipped: true,
            message,
            credits_delta: None,
            balance,
        });
    }

    Ok(ClaimReport {
        skipped: claimed.is_empty(),
        message,
        credits_delta: None,
        balance,
    })
}

/// 汇总文案（中文短句，风格对齐 trae：「签到成功」/「今日已签到」/「签到活动未开启」）。
///
/// 渠道名只在**非默认路径**出现，正常账号看到的仍是三字短句。
fn claim_message(claimed: &[&str], already: &[&str], missing: &[&str]) -> String {
    if !claimed.is_empty() {
        return "签到成功".to_string();
    }
    if !already.is_empty() {
        return "今日已签到".to_string();
    }
    if missing.len() == Channel::ALL.len() {
        return "权益渠道未开通（签到不可用）".to_string();
    }
    format!("未领取（渠道未开通：{}）", missing.join("、"))
}

/// 余额（内核 `_D("balance","GET")`）。返回剩余额度（总量 − 已用）；
/// 字段缺失返回 `Ok(None)`。账号未开通权益时上游报 4000，原样转错误便于排障。
pub async fn fetch_balance(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<Option<f64>, ProviderError> {
    let env = send(
        client,
        cred,
        BASE_URL,
        reqwest::Method::GET,
        BALANCE_PATH,
        None,
        None,
    )
    .await?;
    let result = env.into_result()?;
    let parsed: BalanceResult = serde_json::from_value(result)
        .map_err(|e| ProviderError::Protocol(format!("benefit: 余额字段解析失败: {e}")))?;
    Ok(parsed.remaining())
}

fn truncate(s: &str) -> String {
    if s.len() <= 300 {
        s.to_string()
    } else {
        let mut end = 300;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- 本地 mock 集成测试：验证 method / 顺序 / 签名 / token 是否真的对 ----

    use std::sync::Mutex;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// mock 收到的请求（路径 + 是否带合法签名 + body）。
    type Captured = std::sync::Arc<Mutex<Vec<(String, String, Option<String>)>>>;

    fn test_cred() -> Credential {
        Credential {
            access_key_id: "AKTEST".to_string(),
            secret_access_key: "SKTEST".to_string(),
            security_token: "TOKTEST".to_string(),
            refresh_token: "RT".to_string(),
            expire_at_millis: Some(4_000_000_000_000),
            code_verifier: "v".to_string(),
            dpop_key_pair: crate::providers::codearts::dpop::KeyPair::generate()
                .expect("生成测试密钥失败"),
            last_auth_at: Some(0),
        }
    }

    /// 把 `ENV_BASE_URL` 串行化：环境变量是进程级的，并行测试会互相踩。
    /// 用 tokio Mutex（持有跨 await，不能用 std 的）。
    async fn env_lock() -> tokio::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
            .lock()
            .await
    }

    /// 极简 HTTP mock：按路径顺序返回 JSON，并记录 (method path, body)。
    async fn spawn_mock(responses: Vec<(String, String)>) -> (String, Captured) {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let captured: Captured = std::sync::Arc::new(Mutex::new(Vec::new()));
        let cap = captured.clone();
        tokio::spawn(async move {
            for (resp_body, status) in responses {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                // 读到请求头结束 + body（Content-Length 小，一次读够）。
                loop {
                    let n = match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(head_end) = text.find("\r\n\r\n") {
                        let len = text
                            .lines()
                            .find_map(|l| {
                                l.strip_prefix("content-length: ")
                                    .or_else(|| l.strip_prefix("Content-Length: "))
                                    .and_then(|v| v.trim().parse::<usize>().ok())
                            })
                            .unwrap_or(0);
                        if buf.len() >= head_end + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                let (head, body) = match text.split_once("\r\n\r\n") {
                    Some((h, b)) => (h.to_string(), b.to_string()),
                    None => (text.clone(), String::new()),
                };
                let first = head.lines().next().unwrap_or("").to_string();
                let parts: Vec<&str> = first.split(' ').collect();
                let method = parts.first().copied().unwrap_or("").to_string();
                let path = parts.get(1).copied().unwrap_or("").to_string();
                // 校核：Authorization 存在、含 SignedHeaders 与 Security-Token。
                let auth = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .unwrap_or("")
                    .to_string();
                let tok = head
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("x-security-token:"))
                    .unwrap_or("")
                    .to_string();
                let signed_ok = auth.contains("SDK-HMAC-SHA256 Access=AKTEST")
                    && auth.contains("SignedHeaders=")
                    && !tok.is_empty();
                let body_opt = (!body.trim().is_empty()).then_some(body.clone());
                cap.lock()
                    .expect("lock")
                    .push((format!("{method} {path}"), signed_ok.to_string(), body_opt));
                let resp = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{resp_body}",
                    resp_body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });
        (format!("http://{addr}"), captured)
    }

    /// 签到成功路径：GET claim → POST 启用 → 余额。
    #[tokio::test]
    async fn claim_then_enable_then_balance() {
        // 三个渠道各两次 claim 请求 + 一次余额。
        // benefit: GET=0000 → POST=0000
        // education/examination: GET=4000（未开通，不触发 POST）
        let (base, cap) = spawn_mock(vec![
            (r#"{"error_code":"0000","result":null}"#.to_string(), "200 OK".to_string()),
            (r#"{"error_code":"0000","result":null}"#.to_string(), "200 OK".to_string()),
            (
                r#"{"error_code":"4000","error_msg":"invalid channel, group not found"}"#.to_string(),
                "200 OK".to_string(),
            ),
            (
                r#"{"error_code":"4000","error_msg":"invalid channel, group not found"}"#.to_string(),
                "200 OK".to_string(),
            ),
            (
                r#"{"error_code":"0000","result":{"used_amount":0,"total_quota":100}}"#.to_string(),
                "200 OK".to_string(),
            ),
        ])
        .await;
        // SAFETY：测试内串行设置环境变量（Rust 2024 要求 unsafe）。
        let _env = env_lock().await;
        unsafe { std::env::set_var(ENV_BASE_URL, &base) };
        let client = reqwest::Client::new();
        let report = claim(&client, &test_cred()).await.expect("签到失败");
        unsafe { std::env::remove_var(ENV_BASE_URL) };
        assert!(!report.skipped, "{}", report.message);
        assert_eq!(report.message, "签到成功");
        assert_eq!(report.balance, Some(100.0));

        let calls = cap.lock().expect("lock").clone();
        let paths: Vec<String> = calls.iter().map(|c| c.0.clone()).collect();
        assert_eq!(
            paths,
            vec![
                "GET /api/v1/benefit/claim".to_string(),
                "POST /api/v1/benefit/claim".to_string(),
                "GET /api/v1/benefit/claim?channel=education".to_string(),
                "GET /api/v1/benefit/claim?channel=examination".to_string(),
                "GET /api/v1/user/tokens/balance".to_string(),
            ]
        );
        // 签名与 token 头在每次请求里都在。
        for c in &calls {
            assert_eq!(c.1, "true", "{} 缺残留签名/token", c.0);
        }
        // 启用请求的 body 是 `{}`（benefit 不带 channel）。
        assert_eq!(calls[1].2.as_deref(), Some("{}"));
    }

    /// 今日已签：4006 → skipped，且不触发 POST。
    #[tokio::test]
    async fn already_claimed_is_skipped() {
        let already = r#"{"error_code":"4006","error_msg":"daily claim limit exhausted"}"#;
        let (base, cap) = spawn_mock(vec![
            ((already).to_string(), ("200 OK".to_string()).to_string()),
            ((already).to_string(), ("200 OK".to_string()).to_string()),
            ((already).to_string(), ("200 OK".to_string()).to_string()),
            (
                r#"{"error_code":"0000","result":{"used_amount":0,"total_quota":100}}"#.to_string(),
                "200 OK".to_string(),
            ),
        ])
        .await;
        let _env = env_lock().await;
        unsafe { std::env::set_var(ENV_BASE_URL, &base) };
        let report = claim(&reqwest::Client::new(), &test_cred())
            .await
            .expect("签到失败");
        unsafe { std::env::remove_var(ENV_BASE_URL) };
        assert!(report.skipped);
        assert_eq!(report.message, "今日已签到");
        let paths: Vec<String> = cap
            .lock()
            .expect("lock")
            .iter()
            .map(|c| c.0.clone())
            .collect();
        assert!(!paths.iter().any(|p| p.starts_with("POST")), "{paths:?}");
    }

    /// 渠道全部未开通：4000 → skipped，不报错，但文案说清原因。
    #[tokio::test]
    async fn all_channels_missing_is_skipped() {
        let missing = r#"{"error_code":"4000","error_msg":"invalid channel, group not found"}"#;
        let (base, _cap) = spawn_mock(vec![
            ((missing).to_string(), ("200 OK".to_string()).to_string()),
            ((missing).to_string(), ("200 OK".to_string()).to_string()),
            ((missing).to_string(), ("200 OK".to_string()).to_string()),
        ])
        .await;
        let _env = env_lock().await;
        unsafe { std::env::set_var(ENV_BASE_URL, &base) };
        let report = claim(&reqwest::Client::new(), &test_cred())
            .await
            .expect("签到失败");
        unsafe { std::env::remove_var(ENV_BASE_URL) };
        assert!(report.skipped);
        assert_eq!(report.message, "权益渠道未开通（签到不可用）");
        assert_eq!(report.balance, None);
    }

    /// HTTP 401（签名/token 错）必须报错，不能被当成「未开通」吞掉。
    #[tokio::test]
    async fn http_error_is_reported() {
        let (base, _cap) = spawn_mock(vec![
            (
                r#"{"error_code":"APIG.0301","error_msg":"verify ak sk signature fail"}"#.to_string(),
                "401 Unauthorized".to_string(),
            ),
            (
                r#"{"error_code":"APIG.0301","error_msg":"verify ak sk signature fail"}"#.to_string(),
                "401 Unauthorized".to_string(),
            ),
            (
                r#"{"error_code":"APIG.0301","error_msg":"verify ak sk signature fail"}"#.to_string(),
                "401 Unauthorized".to_string(),
            ),
        ])
        .await;
        let _env = env_lock().await;
        unsafe { std::env::set_var(ENV_BASE_URL, &base) };
        let err = claim(&reqwest::Client::new(), &test_cred())
            .await
            .expect_err("应当失败");
        unsafe { std::env::remove_var(ENV_BASE_URL) };
        assert!(err.to_string().contains("401"), "{err}");
    }

    #[test]
    fn envelope_error_msg_surfaced() {
        // 实测错误形态（2026-02-18）。
        let env: Envelope = serde_json::from_str(
            r#"{"error_code":"4000","error_msg":"invalid channel, group not found","result":null}"#,
        )
        .expect("解析失败");
        let err = env.into_result().unwrap_err().to_string();
        assert!(err.contains("4000"), "{err}");
        assert!(err.contains("invalid channel"), "{err}");
        assert!(err.contains("未开通"), "{err}");
    }

    #[test]
    fn envelope_success_allows_null_result() {
        let env: Envelope =
            serde_json::from_str(r#"{"error_code":"0000","result":null}"#).expect("解析失败");
        assert!(env.into_result().is_ok());
    }

    #[test]
    fn benefit_channel_has_no_query() {
        assert_eq!(Channel::Benefit.query(), None);
        assert_eq!(Channel::Education.query(), Some("channel=education"));
        assert_eq!(Channel::Examination.query(), Some("channel=examination"));
        assert_eq!(Channel::Benefit.enable_body(), "{}");
        assert_eq!(
            Channel::Education.enable_body(),
            r#"{"channel":"education"}"#
        );
    }

    #[test]
    fn claim_message_covers_all_buckets() {
        assert_eq!(claim_message(&["benefit"], &["education"], &[]), "签到成功");
        assert_eq!(claim_message(&[], &["benefit"], &[]), "今日已签到");
        assert_eq!(
            claim_message(&[], &[], &["benefit", "education", "examination"]),
            "权益渠道未开通（签到不可用）"
        );
        assert_eq!(
            claim_message(&[], &[], &["education"]),
            "未领取（渠道未开通：education）"
        );
    }

    #[test]
    fn balance_remaining_uses_webview_field_names() {
        let b: BalanceResult = serde_json::from_str(
            r#"{"used_amount": 1234.0, "total_quota": 10000.0, "maas_type": "benefit"}"#,
        )
        .expect("解析失败");
        assert_eq!(b.remaining(), Some(8766.0));
    }

    #[test]
    fn balance_falls_back_to_education_fields() {
        let b: BalanceResult =
            serde_json::from_str(r#"{"monthly_tokens_used": 100.0, "monthly_token_limit": 500.0}"#)
                .expect("解析失败");
        assert_eq!(b.remaining(), Some(400.0));
    }

    #[test]
    fn balance_missing_field_is_none() {
        let b: BalanceResult =
            serde_json::from_str(r#"{"used_amount": 100.0}"#).expect("解析失败");
        assert_eq!(b.remaining(), None);
    }
}
