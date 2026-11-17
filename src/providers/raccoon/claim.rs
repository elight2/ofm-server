//! raccoon 的「签到」：移动端登录积分领取。
//!
//! # 端点来源（APK v1.1.0 实测）
//!
//! `assets/public/_next/.../app/layout-*.js` 模块 93305：
//!
//! ```js
//! static async grantLoginPoints(e, t) {
//!   return (await u.nC.fetchWithAuth(
//!     `${u.hj()}/api/web/mobile/v1/login/points/grant`,
//!     { method: "POST",
//!       headers: { "X-Client-Platform": `app-${e}`,
//!                  "X-Client-Version": `v${t.replace(/^v/i, "")}` } })).data
//! }
//! ```
//!
//! `e` 来自 `getPlatform()`，只会是 `android` / `ios`
//! （模块 84935：`o = getPlatform() in {ios,android} ? 平台 : null`，非移动端不调用）。
//!
//! # 语义：不是「每天签到」，而是「每次登录领取」
//!
//! 服务端**幂等**：同一天重复调用返回 `granted: false`，不再发放。
//! 客户端每次 App 回到前台都会调一次，靠该字段去重。
//!
//! 实测一次成功领取（2026-11-16 09:43:22 移动端登录）同时产生两条账单：
//! * `daily_grant` 300 分，`expire_at` = `grant_at` + 7 天
//! * `reward_grant` 3000 分「移动端登录奖励」（每号一次）
//!
//! 与桌面端的 `POST /api/web/desktop/v1/login/points/grant` 是**不同端点**，
//! 发放内容也不同（桌面端只发首登奖励，不发每日积分）。
//!
//! # 为什么不把它实现成「刷新令牌顺便签到」
//!
//! 实测拉余额、续期 token、发对话**都不触发**每日积分，只有真实登录才触发。
//! 故本模块是唯一能主动拿每日积分的入口。

use serde::Deserialize;

use super::api::{HOST, UA, get_envelope, post_envelope_with_headers};
use super::credential::Credential;
use crate::core::provider::{ClaimReport, ProviderError};

/// 领取端点：`/api/web/mobile/v1/login/points/grant`。
pub const GRANT_PATH: &str = "/api/web/mobile/v1/login/points/grant";
/// 余额端点（用于报告领取后余额）。
const BALANCE_PATH: &str = "/api/web/points/v1/balance";

/// 模拟 Android 客户端。服务端只校验 header，不校验凭证里的客户端类型
/// （token 的 `aud` 仅 `xiaohuanxiong.com`，无客户端绑定）。
const CLIENT_PLATFORM: &str = "app-android";
/// 移动端版本号，取分析用的 APK（`raccoon-1.1.0`）。
///
/// 不复用桌面版的 `api::CLIENT_VERSION`（`v1.2.0`）：那是 Electron 客户端版本，
/// 冒充移动端却报桌面版本自相矛盾。
const MOBILE_VERSION: &str = "v1.1.0";

/// 领取响应。
#[derive(Debug, Default, Deserialize)]
pub struct Grant {
    /// 本次是否真的发放。`false` = 今天已领过（幂等），不是错误。
    #[serde(default)]
    pub granted: bool,
    /// 发放详情。仅在 `granted == true` 时出现。
    #[serde(default)]
    pub popup: Option<Popup>,
}

/// 发放弹窗内容，字段名以 APK 文案映射为准
/// （`account.pointGrant.sources.{daily,monthly,old_user,new_user,desktop_login,mobile_login}`）。
#[derive(Debug, Default, Deserialize)]
pub struct Popup {
    #[serde(default)]
    pub points: Option<f64>,
    #[serde(default)]
    pub display_name: Option<String>,
    /// 来源：`daily` / `mobile_login` / `new_user` / `old_user` / `monthly` / `desktop_login`。
    #[serde(default)]
    pub source: Option<String>,
}

impl Popup {
    /// 人类可读的来源描述。
    fn source_label(&self) -> &str {
        match self.source.as_deref() {
            Some("daily") => "每日积分",
            Some("mobile_login") => "移动端登录奖励",
            Some("desktop_login") => "桌面端登录奖励",
            Some("new_user") => "新人礼包",
            Some("old_user") => "老用户礼包",
            Some("monthly") => "会员积分",
            _ => "积分",
        }
    }
}

/// 领取移动端登录积分。
///
/// 成功但今天已领过时返回 `skipped: true`，**不是错误** ——
/// 服务端幂等语义如此，当作失败会让 `claim.log` 每天记一次假警报。
pub async fn claim(
    client: &reqwest::Client,
    cred: &Credential,
) -> Result<ClaimReport, ProviderError> {
    let url = format!("{HOST}{GRANT_PATH}");

    let empty = serde_json::json!({});
    let grant: Grant =
        post_envelope_with_headers(client, &url, Some(&cred.access_token), &empty, &headers())
            .await?;

    // 顺便读一次余额，让 `claim.log` 能记下领取前后变化。
    let balance = fetch_balance(client, cred).await.ok();

    if !grant.granted {
        return Ok(ClaimReport {
            skipped: true,
            message: "今日已领取（服务端幂等，每次登录只发一次）".to_string(),
            credits_delta: None,
            balance,
        });
    }

    // 服务端下发的 `display_name` 比本地 `source` 映射准确（可能含活动名），优先用。
    let (label, points) = match &grant.popup {
        Some(p) => (
            p.display_name
                .as_deref()
                .unwrap_or_else(|| p.source_label()),
            p.points,
        ),
        None => ("积分", None),
    };
    let message = match points {
        Some(n) => format!("领取成功：{label} {n}"),
        None => format!("领取成功：{label}"),
    };

    Ok(ClaimReport {
        skipped: false,
        message,
        credits_delta: points,
        balance,
    })
}

/// 读余额。失败返回 `Err`，由调用方决定是否忽略。
async fn fetch_balance(client: &reqwest::Client, cred: &Credential) -> Result<f64, ProviderError> {
    let url = format!("{HOST}{BALANCE_PATH}");
    let headers = [
        ("authorization", format!("Bearer {}", cred.access_token)),
        ("x-raccoon-language", "zh".to_string()),
        ("accept", "*/*".to_string()),
    ];
    let data: super::balance::Balance = get_envelope(client, &url, &headers).await?;
    Ok(data.available_points)
}

/// 构造请求头（供测试断言）。
fn headers() -> Vec<(&'static str, String)> {
    vec![
        ("user-agent", UA.to_string()),
        ("x-client-platform", CLIENT_PLATFORM.to_string()),
        ("x-client-version", MOBILE_VERSION.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 请求头含移动端标识() {
        let h = headers();
        let find = |k: &str| h.iter().find(|(n, _)| *n == k).map(|(_, v)| v.as_str());
        assert_eq!(find("x-client-platform"), Some("app-android"));
        assert_eq!(find("x-client-version"), Some("v1.1.0"));
    }

    #[test]
    fn 授权失败不是已领取() {
        // 解析 grant=false 时应归类为「已领」，此处只验证结构默认值。
        let g: Grant = serde_json::from_str(r#"{"granted":false}"#).unwrap();
        assert!(!g.granted);
        assert!(g.popup.is_none());
    }

    #[test]
    fn 解析发放弹窗() {
        let raw = r#"{"granted":true,"popup":{"points":300,"source":"daily",
                       "display_name":"每日积分"}}"#;
        let g: Grant = serde_json::from_str(raw).unwrap();
        assert!(g.granted);
        let p = g.popup.unwrap();
        assert_eq!(p.points, Some(300.0));
        assert_eq!(p.source_label(), "每日积分");
    }

    #[test]
    fn 未知来源回退为积分() {
        let p = Popup {
            source: Some("something_new".into()),
            ..Default::default()
        };
        assert_eq!(p.source_label(), "积分");
    }
}
