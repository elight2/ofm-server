//! minimax 的「每日签到」。
//!
//! # 端点来源（TUI 源码）
//!
//! `packages/tui/src/checkin/http-gateway.ts`：
//!
//! ```text
//! const STATUS_PATH = '/minimax-cloud/api/v1/signin/status';   // GET
//! const CLAIM_PATH  = '/minimax-cloud/api/v1/signin/claim';    // POST {}
//! ```
//!
//! 主机取 `publicGatewayOrigin()` 的 `cn/prod` = `https://agent.minimax.cn`
//! （`packages/tui/src/runtime/public-gateway.ts`）。UI 文案「每日签到」/「今日已签到」
//! 对应的就是这两个端点。
//!
//! # 实测（2026-11-17，本仓库 testdata 账号）
//!
//! * `GET status` → `{"data":{"scene":2,"days":[7 天...]},"base_resp":{"status_code":0}}`
//! * `POST claim` → `{"data":{"claim_id":"…","claim_result":2,"day_no":1,"points":400,
//!   "expire_at_ms":…,"panel":{…}},"base_resp":{…}}`
//! * 今日已签时 `claim_result = 2`（`AlreadyClaimed`），**服务端幂等**、不报错。
//! * 两个端点都只需要 `Authorization: Bearer`；源码里的 `yy`/`x-timestamp`/
//!   `x-signature` 归属头**不是必需**（去掉仍 200），故本实现不发。
//! * **`timezone_offset` 查询参数必需**：缺失时上游回 HTTP 200 +
//!   `{"base_resp":{"status_code":1406010011,"status_msg":"invalid timezone_id"}}`。
//!   其余 query 参数（`user_id`、`device_platform` 等）均可省略（逐个删测过）。
//!
//! # 状态码语义（`packages/shared/src/daily-signin.ts`）
//!
//! 天状态：`1` 未到 / `2` 可领 / `3` 已领 / `4` 停用。
//! 领取结果：`1` 本次领取 / `2` 今日已领。

use serde::Deserialize;

use super::api::{AGENT_HOST, HttpError, UA};
use super::ID;
use super::credential::Credential;
use crate::core::provider::{ClaimReport, ProviderError};

/// 签到状态端点。
pub const STATUS_PATH: &str = "/minimax-cloud/api/v1/signin/status";
/// 签到领取端点。
pub const CLAIM_PATH: &str = "/minimax-cloud/api/v1/signin/claim";

/// 天状态：可领取。
const DAY_CLAIMABLE: i64 = 2;
/// 天状态：已领取。
const DAY_CLAIMED: i64 = 3;
/// 领取结果：本次领取成功。
const RESULT_CLAIMED: i64 = 1;

/// 周期总天数（上游固定 7 天）。
const CYCLE_DAYS: i64 = 7;

/// 时区偏移（秒），东八区。
///
/// 上游要求该参数合法（`invalid timezone_id`），本实现不读本机时区：
/// 签到周期按账号所在地（国内账号 = +8）算，用固定值更稳定。
const TIMEZONE_OFFSET_SECS: i32 = 28_800;

/// `base_resp` 业务码信封。
#[derive(Debug, Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    base_resp: Option<BaseResp>,
    data: Option<T>,
}

#[derive(Debug, Deserialize)]
struct BaseResp {
    #[serde(default)]
    status_code: i64,
    #[serde(default)]
    status_msg: String,
}

/// 签到面板。
#[derive(Debug, Default, Deserialize)]
struct Panel {
    #[serde(default)]
    days: Vec<Day>,
}

impl Panel {
    /// 今日那一格。
    fn today(&self) -> Option<&Day> {
        self.days.iter().find(|d| d.is_today)
    }

    /// 已签天数。
    fn claimed_count(&self) -> i64 {
        self.days.iter().filter(|d| d.status == DAY_CLAIMED).count() as i64
    }

    /// 连续签到天数：从今日往前数连续 `status == 3` 的格子。
    ///
    /// 今日可领（`status == 2`）时从**昨天**起算，与客户端
    /// `getCurrentSigninStreak` 一致。
    fn streak(&self) -> i64 {
        let mut days: Vec<&Day> = self.days.iter().collect();
        days.sort_by_key(|d| d.day_no);
        let Some(idx) = days.iter().position(|d| d.is_today) else {
            return 0;
        };
        let start = match days[idx].status {
            DAY_CLAIMED => idx as i64,
            DAY_CLAIMABLE => idx as i64 - 1,
            _ => return 0,
        };
        let mut streak = 0;
        let mut i = start;
        while i >= 0 {
            if days[i as usize].status != DAY_CLAIMED {
                break;
            }
            streak += 1;
            i -= 1;
        }
        streak
    }
}

#[derive(Debug, Default, Deserialize)]
struct Day {
    #[serde(default)]
    day_no: i64,
    /// 当日积分。上游的 `bonus_points`（额外赠送）**已包含在此值内**
    /// （`packages/shared/src/daily-signin.ts` 注释），故不再单独解析/相加。
    #[serde(default)]
    points: f64,
    #[serde(default)]
    status: i64,
    #[serde(default)]
    is_today: bool,
}

/// 领取响应里的 `data`。
#[derive(Debug, Deserialize)]
struct ClaimData {
    #[serde(default)]
    claim_result: i64,
    #[serde(default)]
    day_no: i64,
    #[serde(default)]
    points: f64,
    #[serde(default)]
    panel: Panel,
}

/// 执行签到（`Provider::claim`）。
///
/// **先查状态再决定是否领取**：今日已签时直接返回 `skipped`，不发 POST。
/// 上游虽然幂等（`claim_result = 2`），但没必要白跑一次写请求。
pub async fn claim(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<ClaimReport, ProviderError> {
    let panel = fetch_status(client, cred).await?;
    let today = panel.today();
    let already = today.map(|d| d.status == DAY_CLAIMED).unwrap_or(false);
    let claimable = today.map(|d| d.status == DAY_CLAIMABLE).unwrap_or(false);

    if already {
        return Ok(with_balance(
            skip(format!("今日已签到（{}）", progress(&panel))),
            client,
            cred,
        )
        .await);
    }
    if !claimable {
        // 面板里今日既非已签也非可领：处于停用/未开放窗口，不猜原因。
        return Ok(with_balance(
            skip(format!("今日不可签到（{}）", describe(&panel))),
            client,
            cred,
        )
        .await);
    }

    let data = post_claim(client, cred).await?;
    Ok(with_balance(verdict(&data), client, cred).await)
}

/// 补上当前余额，让 `claim.log` 有 `balance` 字段。
///
/// 余额拿不到不算签到失败（§6），只记日志并保持 `None`。
async fn with_balance(
    mut report: ClaimReport,
    client: &reqwest::Client,
    cred: &Credential,
) -> ClaimReport {
    match super::balance::fetch(client, cred).await {
        Ok(v) => report.balance = v,
        Err(e) => tracing::debug!("[{ID}] 签到后取余额失败（忽略）: {e}"),
    }
    report
}

/// 把领取响应转成报告（纯函数，便于单测）。
fn verdict(data: &ClaimData) -> ClaimReport {
    // 并发下可能被别处抢先领掉，上游对同一格仍会回 AlreadyClaimed。
    if data.claim_result != RESULT_CLAIMED {
        return skip(format!("今日已签到（{}）", progress(&data.panel)));
    }
    // `points` 已含 `bonus_points`（见 `Day::points` 注释），不再相加。
    let points = data.points;
    ClaimReport {
        skipped: false,
        message: format!(
            "签到成功：第 {} 天 +{} 积分（{}）",
            data.day_no,
            fmt_points(points),
            progress(&data.panel)
        ),
        credits_delta: Some(points),
        balance: None,
    }
}

/// 未领到时的统一报告（不是错误，避免 `claim.log` 假警报）。
fn skip(message: String) -> ClaimReport {
    ClaimReport {
        skipped: true,
        message,
        credits_delta: None,
        balance: None,
    }
}

/// `GET /signin/status`。
async fn fetch_status(client: &reqwest::Client, cred: &Credential) -> Result<Panel, ProviderError> {
    let url = url(STATUS_PATH);
    let resp = client
        .get(&url)
        .header("accept", "application/json")
        .header("user-agent", UA)
        .header("authorization", format!("Bearer {}", cred.access_token))
        .send()
        .await?;
    parse(resp, |d: Panel| d).await
}

/// `POST /signin/claim`（服务端要求 body 为 `{}`）。
async fn post_claim(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<ClaimData, ProviderError> {
    let url = url(CLAIM_PATH);
    let resp = client
        .post(&url)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .header("user-agent", UA)
        .header("authorization", format!("Bearer {}", cred.access_token))
        .body("{}")
        .send()
        .await?;
    parse(resp, |d: ClaimData| d).await
}

/// 拼 URL。`timezone_offset` 必带（见模块文档）。
fn url(path: &str) -> String {
    format!("{AGENT_HOST}{path}?timezone_offset={TIMEZONE_OFFSET_SECS}")
}

/// 解信封：HTTP 非 2xx → `Status`；`base_resp.status_code != 0` → `OAuth`/
/// `Protocol`；`data` 缺失 → `Protocol`。
async fn parse<T, U>(resp: reqwest::Response, pick: impl Fn(T) -> U) -> Result<U, ProviderError>
where
    T: for<'de> Deserialize<'de>,
{
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !(200..300).contains(&status.as_u16()) {
        return Err(HttpError::Status {
            status: status.as_u16(),
            body: truncate(&text, 300),
        }
        .into());
    }
    let env: Envelope<T> = serde_json::from_str(&text)
        .map_err(|e| HttpError::Decode(format!("{e}; 原文: {}", truncate(&text, 300))))?;
    if let Some(base) = &env.base_resp
        && base.status_code != 0
    {
        return Err(ProviderError::Protocol(format!(
            "业务码 {}: {}",
            base.status_code, base.status_msg
        )));
    }
    let data = env
        .data
        .ok_or_else(|| ProviderError::Protocol("响应缺少 data 字段".to_string()))?;
    Ok(pick(data))
}

/// 一行进度摘要：「连续 N 天 · 本周期 M/7」。
fn progress(panel: &Panel) -> String {
    let streak = panel.streak();
    let claimed = panel.claimed_count();
    if streak > 0 {
        format!("连续 {streak} 天 · 本周期 {claimed}/{CYCLE_DAYS}")
    } else {
        format!("本周期 {claimed}/{CYCLE_DAYS}")
    }
}

/// 完整描述面板，用于「今日不可签到」这类需要上下文的消息。
fn describe(panel: &Panel) -> String {
    let today = match panel.today() {
        Some(d) => format!("今日第 {} 天 {} 积分", d.day_no, fmt_points(d.points)),
        None => "面板无今日格子".to_string(),
    };
    format!("{today} · {}", progress(panel))
}

/// 积分为整数时不显示 `.0`。
fn fmt_points(v: f64) -> String {
    if (v.fract()).abs() < f64::EPSILON {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel(days: &[(i64, i64, bool)]) -> Panel {
        Panel {
            days: days
                .iter()
                .map(|&(day_no, status, is_today)| Day {
                    day_no,
                    status,
                    is_today,
                    points: 400.0,
                })
                .collect(),
        }
    }

    #[test]
    fn 连续天数_今日已签() {
        // 第 1、2 天已签，今日是第 2 天。
        let p = panel(&[(1, 3, false), (2, 3, true), (3, 1, false)]);
        assert_eq!(p.streak(), 2);
        assert_eq!(p.claimed_count(), 2);
    }

    #[test]
    fn 连续天数_今日可领从昨天起算() {
        let p = panel(&[(1, 3, false), (2, 3, false), (3, 2, true)]);
        assert_eq!(p.streak(), 2);
    }

    #[test]
    fn 连续天数_断签归零() {
        // 今日可领，昨天未签 → 没有连续天数。
        let p = panel(&[(1, 3, false), (2, 1, false), (3, 2, true)]);
        assert_eq!(p.streak(), 0);
    }

    #[test]
    fn 连续天数_今日未开放归零() {
        let p = panel(&[(1, 3, false), (2, 4, true)]);
        assert_eq!(p.streak(), 0);
    }

    #[test]
    fn 面板无今日格子时不崩溃() {
        let p = panel(&[(1, 3, false)]);
        assert_eq!(p.streak(), 0);
        assert!(p.today().is_none());
    }

    #[test]
    fn 解析真实状态响应() {
        let raw = r#"{"data":{"scene":2,"days":[
            {"day_no":1,"points":400,"status":3,"is_today":true,"bonus_points":0},
            {"day_no":2,"points":400,"status":1,"is_today":false,"bonus_points":0}],
            },"base_resp":{"status_code":0,"status_msg":"ok"}}"#;
        // 上面故意多一个逗号会解析失败，这里用合法 JSON 重试。
        assert!(serde_json::from_str::<Envelope<Panel>>(raw).is_err());

        let raw = r#"{"data":{"scene":2,"days":[
            {"day_no":1,"points":400,"status":3,"is_today":true,"bonus_points":0},
            {"day_no":2,"points":400,"status":1,"is_today":false,"bonus_points":0}],
            "base_resp_x":0},"base_resp":{"status_code":0,"status_msg":"ok"}}"#;
        let env: Envelope<Panel> = serde_json::from_str(raw).unwrap();
        assert_eq!(env.base_resp.unwrap().status_code, 0);
        let p = env.data.unwrap();
        assert_eq!(p.today().unwrap().day_no, 1);
        assert_eq!(p.streak(), 1);
    }

    #[test]
    fn 解析真实领取响应() {
        let raw = r#"{"data":{"claim_id":"2122579540063585031","claim_result":2,"day_no":1,
            "points":400,"expire_at_ms":1797436800000,"panel":{"scene":2,"days":[
            {"day_no":1,"points":400,"status":3,"is_today":true,"bonus_points":0}]}},
            "base_resp":{"status_code":0,"status_msg":"ok"}}"#;
        let env: Envelope<ClaimData> = serde_json::from_str(raw).unwrap();
        let d = env.data.unwrap();
        assert_eq!(d.claim_result, 2);
        assert_eq!(d.points, 400.0);
        assert_eq!(d.panel.streak(), 1);
    }

    #[test]
    fn 时区参数必须出现在_url_里() {
        assert!(url(STATUS_PATH).ends_with("timezone_offset=28800"));
        assert!(url(CLAIM_PATH).contains("agent.minimax.cn"));
    }

    #[test]
    fn 领取成功报告含天数与积分() {
        let data = ClaimData {
            claim_result: RESULT_CLAIMED,
            day_no: 3,
            points: 1000.0,
            panel: panel(&[(1, 3, false), (2, 3, false), (3, 3, true)]),
        };
        let r = verdict(&data);
        assert!(!r.skipped);
        assert_eq!(r.credits_delta, Some(1000.0));
        assert!(r.message.contains("第 3 天"), "{}", r.message);
        assert!(r.message.contains("+1000"), "{}", r.message);
        assert!(r.message.contains("连续 3 天"), "{}", r.message);
    }

    #[test]
    fn 领取结果已领不算失败() {
        let data = ClaimData {
            claim_result: 2,
            day_no: 1,
            points: 400.0,
            panel: panel(&[(1, 3, true)]),
        };
        let r = verdict(&data);
        assert!(r.skipped, "已领应跳过而不是成功");
        assert_eq!(r.credits_delta, None);
        assert!(r.message.contains("今日已签到"));
    }

    #[test]
    fn 跳过报告不带增量() {
        let r = skip("x".to_string());
        assert!(r.skipped && r.credits_delta.is_none() && r.balance.is_none());
    }

    #[test]
    fn 整数积分不带小数() {
        assert_eq!(fmt_points(400.0), "400");
        assert_eq!(fmt_points(400.5), "400.5");
    }
}
