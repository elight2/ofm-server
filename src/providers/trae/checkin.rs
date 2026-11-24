//! 签到（`provider-info/trae.md` §6）。
//!
//! 流程：`status` → 若可领取则 `claim` → 复读 `status` 复核。
//!
//! 判定口径**逐条对齐官方客户端**（`resources/app/out/main.js`）：
//!
//! - `E0(t)`：业务码分类。`code != 0` 一律算失败，并按码细分
//!   （`9090` 不可用 / `9102` 未签到 / `1002` 鉴权 / `4001` 参数 / 其它 unknown）。
//!   该函数对 `status` 也生效——`o1e` 第一行就是 `if (E0(t)) return "unknown"`。
//! - `o1e(t)`：UI 的「能否领取」判定：
//!   `checked_in === true || did_checked_in === true` → 已签到；
//!   `enable === false` → 未开启；`enable === true && checked_in === false` → 可领取。
//!   **`did_checked_in` 也视为已签到**，本实现必须一并判断，否则会重复调 claim。
//! - `a1e(t)`：claim 成功的唯一标准 = HTTP 200 且无 `E0` 错误（即 `code === 0`）。
//!
//! 重试：客户端 `fetchWithHeaders` 的 `retry` 默认 0（claim 也显式传 `retry:0`），
//! 碰到 `9074` 靠用户看到弹窗后再点一次。命令行无人可点，故本实现对 `9074`
//! （参与用户太多）在 `status`/`claim` 两处都退避重试几次。
//!
//! **陷阱一**：`code=9074` 与其它非 0 `code` 一样，**不能**当成“可领取”而忽略：
//! 客户端 `o1e` 判到此码会给 `unknown`（不让签到）；`a1e` 也判 claim 失败。
//! 但重试耗尽后**也不能**只凭 `claim` 的错误码判失败——上游可能已经发过了，
//! 真实结果要看复读的 `status`（调用方负责，见 `mod.rs::claim`）。
//!
//! **陷阱二**：当日已签到时再调 `claim`，上游仍回
//! `HTTP 200 {"code":0,"message":"success"}`，与真正领取成功**无法区分**。
//! 因此不能靠 `claim` 的返回值判断成败：已签到就直接跳过（**不调 claim**），
//! 是否真领到只能靠复读 `status` 复核。
//!
//! 余额（`pay/ide_user_ent_usage`）仅用于报告增量，不作为成败判据——
//! 期间若有对话消耗，差额并不等于签到所得。
//!
//! 请求体是**裸 JSON**（如 `{"req_source":1}`，16 字节），不包 `{"data": …}` 外壳。
//! 2026-11-24 用 mitmproxy 抓 Windows 客户端明文确认；早先据
//! `monitor/parfait` 的 `sent_body_bytes`（25 字节）反推的"data 外壳"是错的，
//! 那个字段量的是 TTNet 加密压缩后的载荷。详见 `provider-info/trae.md` §6。

use serde::Deserialize;

/// 签到走 `api.trae.cn`（与登录用的 `api.trae.com.cn` 不同，实测后者 404）。
pub const HOST: &str = "https://api.trae.cn";
pub const PATH_STATUS: &str = "/trae/api/v2/ug/checkin_credits/status";
pub const PATH_CLAIM: &str = "/trae/api/v2/ug/checkin_credits/claim";
pub const PATH_USAGE: &str = "/trae/api/v2/pay/ide_user_ent_usage";

/// 桌面客户端为 `1`，web 为 `3`（§6）。SOLO 系列客户端会传 `2`。
///
/// 客户端的取值是 `ii(this.X) ? 2 : 1`，而 `ii()` 判的是 `packageType` 是否属于
/// SOLO 系列（`SOLO_CN` / `SOLO_I18N` / `SOLO_CN_ENTERPRISE`）。本实现面向 IDE 版
/// Trae CN，故固定 `1`。
pub const REQ_SOURCE: u8 = 1;

/// 凭证失效的业务码。不在客户端 `E0` 分类表里，但上游会以 HTTP 200 下发，
/// 本实现额外把它当鉴权失败（触发续期/重登）。
pub const CODE_AUTH: i64 = 1001;

/// 「当前参与用户太多，请稍后再试」的业务码。
///
/// 实测它会出现在 `checkin_credits/claim` 上：同一时刻 `status` 说可领取、
/// `claim` 却回 9074，隔几秒重试就能成功（客户端靠用户手动再点一次）。
pub const CODE_BUSY: i64 = 9074;

/// `code` 的语义（对齐客户端 `E0`）。
///
/// 只有 [`CodeKind::None`] 表示成功；其余都不算成功，区别仅在错误文案与
/// 是否需要重新登录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeKind {
    /// `code == 0`，成功。
    None,
    /// `1002`，鉴权失败。
    Auth,
    /// `9090`，活动不可用。
    Unavailable,
    /// `9102`，未签到。
    NotSigned,
    /// `4001`，参数错误。
    Parameters,
    /// 其它非 0 码（如 `9074` 参与用户太多、`1001` 凭证失效）。
    Other,
}

impl CodeKind {
    /// 按客户端 `E0` 的分类表映射。
    pub fn from_code(code: i64) -> Self {
        match code {
            0 => CodeKind::None,
            1002 => CodeKind::Auth,
            9090 => CodeKind::Unavailable,
            9102 => CodeKind::NotSigned,
            4001 => CodeKind::Parameters,
            _ => CodeKind::Other,
        }
    }

    /// 是否成功。
    pub fn is_ok(self) -> bool {
        matches!(self, CodeKind::None)
    }
}

/// 上游业务响应里的通用错误（`status` / `claim` 共用）。
#[derive(Debug, Clone)]
pub struct CodeError {
    pub code: i64,
    pub message: String,
    pub kind: CodeKind,
}

impl std::fmt::Display for CodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "code={} message={}", self.code, self.message)
    }
}

/// `status` 响应。
///
/// `enable` / `checked_in` 用 `Option<bool>`：客户端 `o1e` 靠「字段是否为
/// `boolean`」来分 `not_signed` / `disabled` / `unknown`，缺失即是 `unknown`
/// （不可领取），而不是协议异常。
#[derive(Debug, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
    /// 活动是否开启。
    #[serde(default)]
    pub enable: Option<bool>,
    /// 今日是否已签到。
    #[serde(default)]
    pub checked_in: Option<bool>,
    /// 本次请求是否触发了签到（客户端 `o1e` 也把它当「已签到」）。
    #[serde(default)]
    pub did_checked_in: Option<bool>,
    /// 基础/额外额度（已签到时仍会回，表示该活动额度而非本次所得）。
    #[serde(default)]
    pub credits: i64,
    #[serde(default)]
    pub extra_credits: i64,
}

impl Status {
    /// 业务码错误；`code == 0` 返回 `None`。
    ///
    /// 与客户端 `E0` 一致：`status` 带非 0 `code` 就是失败（`o1e` 会判 `unknown`）。
    pub fn code_error(&self) -> Option<CodeError> {
        code_error(self.code, &self.message)
    }

    /// 是否已签到：`checked_in` 或 `did_checked_in` 任一为 `true`。
    ///
    /// 对齐客户端 `o1e`：`e?.checked_in===!0 || e?.did_checked_in===!0` → `signed`。
    pub fn is_checked_in(&self) -> bool {
        self.checked_in == Some(true) || self.did_checked_in == Some(true)
    }

    /// 活动是否开启。
    pub fn enabled(&self) -> bool {
        self.enable == Some(true)
    }

    /// 今日是否可以领取（对齐客户端 `o1e` 的 `not_signed`）。
    ///
    /// 要求 `enable === true && checked_in === false && !is_checked_in()`，
    /// 且无业务码错误。后一项保证 `did_checked_in === true` 时不再重复领取。
    pub fn claimable(&self) -> bool {
        self.code_error().is_none()
            && self.enabled()
            && self.checked_in == Some(false)
            && !self.is_checked_in()
    }
}

/// `claim` 响应。
///
/// 成功判据对齐客户端 `a1e`：HTTP 200 且 `code === 0`。
#[derive(Debug, Deserialize)]
pub struct ClaimResult {
    #[serde(default)]
    pub code: i64,
    #[serde(default)]
    pub message: String,
}

impl ClaimResult {
    pub fn code_error(&self) -> Option<CodeError> {
        code_error(self.code, &self.message)
    }
}

/// `pay/ide_user_ent_usage` 只取总余额。
#[derive(Debug, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub total_balance: f64,
}

/// 构造业务码错误（`code == 0` 时返回 `None`）。
fn code_error(code: i64, message: &str) -> Option<CodeError> {
    let kind = CodeKind::from_code(code);
    if kind.is_ok() {
        return None;
    }
    Some(CodeError {
        code,
        message: message.to_string(),
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_status(json: &str) -> Status {
        serde_json::from_str(json).expect("status 解析失败")
    }

    #[test]
    fn 已签到时不可领取() {
        let s = parse_status(
            r#"{"checked_in":true,"code":0,"credits":100,"did_checked_in":false,
                "enable":true,"extra_credits":100,"message":"success"}"#,
        );
        assert!(s.is_checked_in());
        assert!(!s.claimable(), "已签到不应再调 claim");
        assert!(s.code_error().is_none());
    }

    /// 客户端 `o1e` 把 `did_checked_in === true` 也当已签到，本实现必须一致，
    /// 否则会在上游已发放的情况下重复调 claim。
    #[test]
    fn did_checked_in_也算已签到() {
        let s = parse_status(
            r#"{"checked_in":false,"code":0,"did_checked_in":true,
                "enable":true,"message":"success"}"#,
        );
        assert!(s.is_checked_in(), "did_checked_in 为 true 应视为已签到");
        assert!(!s.claimable());
    }

    #[test]
    fn 活动关闭时不可领取() {
        let s = parse_status(r#"{"code":0,"enable":false,"checked_in":false}"#);
        assert!(!s.claimable());
    }

    #[test]
    fn 未签到且活动开启可领取() {
        let s = parse_status(r#"{"code":0,"enable":true,"checked_in":false}"#);
        assert!(s.claimable());
    }

    /// 客户端 `o1e` 第一行是 `if (E0(t)) return "unknown"`，即 status 带非 0 码
    /// 就不给签到。故 9074 同样不可领取，只是文案要能看懂。
    #[test]
    fn 参与用户太多不可领取() {
        let s = parse_status(
            r#"{"code":9074,"message":"当前参与用户太多，请稍后再试",
                "enable":true,"checked_in":false}"#,
        );
        let e = s.code_error().expect("9074 应算失败");
        assert_eq!(e.code, 9074);
        assert_eq!(e.kind, CodeKind::Other);
        assert!(e.to_string().contains("9074"));
        assert!(!s.claimable(), "客户端此时也不给签到");
    }

    #[test]
    fn 凭证失效映射为鉴权类() {
        // 1001 不在客户端 E0 的分类表里，仍是 Other；我方额外把它当鉴权失败。
        let s = parse_status(
            r#"{"code":1001,"message":"not able to authenticate you",
                "enable":false,"checked_in":false}"#,
        );
        let e = s.code_error().expect("1001 应算失败");
        assert_eq!(e.kind, CodeKind::Other);
        assert_eq!(e.code, CODE_AUTH);

        // 1002 是客户端认定的鉴权码。
        let s = parse_status(r#"{"code":1002,"message":"auth","enable":false}"#);
        assert_eq!(s.code_error().unwrap().kind, CodeKind::Auth);
    }

    #[test]
    fn 其它业务码分类对齐客户端() {
        assert_eq!(CodeKind::from_code(0), CodeKind::None);
        assert_eq!(CodeKind::from_code(9090), CodeKind::Unavailable);
        assert_eq!(CodeKind::from_code(9102), CodeKind::NotSigned);
        assert_eq!(CodeKind::from_code(1002), CodeKind::Auth);
        assert_eq!(CodeKind::from_code(4001), CodeKind::Parameters);
        assert_eq!(CodeKind::from_code(9074), CodeKind::Other);
        assert!(CodeKind::from_code(0).is_ok());
        assert!(!CodeKind::from_code(9102).is_ok());
    }

    #[test]
    fn 字段缺失时不可领取() {
        // 缺 enable/checked_in 时不报错，只是不可领取（客户端 o1e → unknown）。
        let s = parse_status(r#"{"code":0,"enable":true}"#);
        assert!(!s.claimable(), "缺 checked_in 无法判定可领取");
        assert!(!s.is_checked_in());

        let s = parse_status(r#"{"code":0,"checked_in":false}"#);
        assert!(!s.claimable(), "缺 enable 视为未开启");
    }

    #[test]
    fn claim_成功响应无_credits_字段也能解析() {
        let r: ClaimResult = serde_json::from_str(r#"{"code":0,"message":"success"}"#).unwrap();
        assert!(r.code_error().is_none());
    }

    #[test]
    fn claim_失败时带上游原文() {
        let r: ClaimResult =
            serde_json::from_str(r#"{"code":9074,"message":"当前参与用户太多，请稍后再试"}"#)
                .unwrap();
        let e = r.code_error().expect("非 0 code 应失败");
        assert_eq!(e.code, CODE_BUSY);
        assert!(e.to_string().contains("9074"), "{e}");
        assert!(e.to_string().contains("太多"), "应保留上游文案: {e}");
    }

    /// 9074 是瞬时拥挤码，调用方会重试；其它码不重试。
    #[test]
    fn 只有拥挤码可重试() {
        assert_eq!(CodeKind::from_code(CODE_BUSY), CodeKind::Other);
        assert_eq!(CODE_BUSY, 9074);
        assert_eq!(CODE_AUTH, 1001);
    }

    /// 请求体是裸 JSON，字节数须与 2026-11-24 mitmproxy 抓到的客户端明文一致。
    #[test]
    fn 请求体与客户端明文字节数一致() {
        let status = serde_json::json!({ "req_source": REQ_SOURCE }).to_string();
        assert_eq!(status, r#"{"req_source":1}"#);
        assert_eq!(status.len(), 16, "客户端 status 为 16 字节");

        let usage = serde_json::json!({
            "require_usage": true,
            "req_source": REQ_SOURCE
        })
        .to_string();
        assert_eq!(usage.len(), 37, "客户端 usage 为 37 字节");
        assert!(!usage.contains("\"data\""), "不得再包 data 外壳");
    }

    #[test]
    fn usage_取总余额() {
        let u: Usage =
            serde_json::from_str(r#"{"total_balance":5657.82,"usage_summary":{}}"#).unwrap();
        assert!((u.total_balance - 5657.82).abs() < 1e-9);
    }
}
