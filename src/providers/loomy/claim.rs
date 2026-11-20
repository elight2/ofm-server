//! Loomy 的「签到」：宠物打工奖励（`/pet-work` + `/pet-work/rewards`）。
//!
//! Loomy 本身**没有**每日签到接口（`provider-info/loomy.md` §7「无」），
//! 但它的每日福利落在「宠物打工」这套接口上，语义与签到等价：
//!
//! ```text
//! GET  /pet-work          → 业务日 businessDate、配置 config{tiers}、
//!                           今日状态 status{earnedPointsToday, rewardedTierIds,
//!                                          dailyRewardCapped}
//! POST /pet-work/rewards  → {businessDate, configVersion, reachedTierId}
//!                           返回 {granted, duplicate, grantedPoints,
//!                                 grantedTierIds, status{...}}
//! ```
//!
//! # 实测结论（2026-11-20）
//!
//! * 上游**不校验**本地算出的打工分数：直接申请任意 `tierId` 都会发分；
//! * 每个档位**每天只能领一次**，重复申请回 `duplicate: true` + `grantedPoints: 0`；
//! * 档位奖励 `tier_1/2/3 → 200/300/500`，累加受 `config.dailyMaxPoints`（当前 1000）封顶，
//!   `status.dailyRewardCapped` 为真时不再发分。
//!
//! 故本实现按档位升序逐个申请未领的档位，直到被拒（已领 / 已封顶）。
//! **不要**只申请最高档：服务端按「本次增量」发分，只领 `tier_3` 只拿 500，
//! 依次领三档才能拿满 1000。

use serde::Deserialize;

use super::api::{self, MODEL_BASE_URL};
use crate::core::claim::{ClaimDetail, ClaimOutcome, SkipReason};

/// 快照路径。
pub const PATH_SNAPSHOT: &str = "/pet-work";
/// 领取路径。
pub const PATH_REWARD: &str = "/pet-work/rewards";

/// `GET /pet-work` 的 `data`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// 活动是否开启。
    #[serde(default)]
    pub enabled: bool,
    /// 服务端业务日（`YYYY-MM-DD`），申请奖励时要原样回传。
    #[serde(default)]
    pub business_date: String,
    /// 配置。
    #[serde(default)]
    pub config: Config,
    /// 今日状态。
    #[serde(default)]
    pub status: Status,
}

/// 活动配置。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// 配置版本号，申请奖励时要原样回传。
    #[serde(default)]
    pub version: String,
    /// 档位表（按 `score` 升序，但上游不校验，只用于确定申请顺序）。
    #[serde(default)]
    pub tiers: Vec<Tier>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tier {
    #[serde(default)]
    pub tier_id: String,
    #[serde(default)]
    pub score: f64,
}

/// 今日状态。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// 今日已发积分数。
    #[serde(default)]
    pub earned_points_today: f64,
    /// 今日已领档位 id。
    #[serde(default)]
    pub rewarded_tier_ids: Vec<String>,
    /// 今日是否已封顶。
    #[serde(default)]
    pub daily_reward_capped: bool,
}

/// `POST /pet-work/rewards` 的 `data`。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    /// 本次是否真的发分。
    #[serde(default)]
    pub granted: bool,
    /// 是否为重复申请（已领过）。
    #[serde(default)]
    pub duplicate: bool,
    /// 本次发放积分。
    #[serde(default)]
    pub granted_points: f64,
    /// 本次发放的档位。
    #[serde(default)]
    pub granted_tier_ids: Vec<String>,
    /// 发放后的今日状态。
    #[serde(default)]
    pub status: Status,
}

impl Snapshot {
    /// 已领取的档位里，按配置顺序挑下一个要申请的档位。
    ///
    /// 返回 `None` 表示今日档位都已领完或已封顶。
    fn next_tier(&self) -> Option<&Tier> {
        self.sorted_tiers()
            .into_iter()
            .find(|t| !self.status.rewarded_tier_ids.contains(&t.tier_id))
    }

    /// 按 `score` 升序的档位（同分保持目录顺序）。
    fn sorted_tiers(&self) -> Vec<&Tier> {
        let mut tiers: Vec<&Tier> = self.config.tiers.iter().collect();
        tiers.sort_by(|a, b| a.score.total_cmp(&b.score));
        tiers
    }
}

/// 签到结果。
pub struct Claimed {
    /// 结果。
    pub outcome: ClaimOutcome,
    /// 本次发放积分（可观测时）。
    pub credits_delta: Option<f64>,
}

/// 领取今日宠物打工奖励。
pub async fn claim(
    client: &reqwest::Client,
    headers: &[(&str, String)],
) -> Result<Claimed, api::EnvelopeErr> {
    let mut snap: Snapshot = api::get_envelope(with_headers(
        client.get(format!("{MODEL_BASE_URL}{PATH_SNAPSHOT}")),
        headers,
    ))
    .await?;

    if !snap.enabled {
        return Ok(Claimed {
            outcome: ClaimOutcome::skipped(SkipReason::NotEnabled, vec![]),
            credits_delta: None,
        });
    }

    let mut details: Vec<ClaimDetail> = Vec::new();
    let mut total = 0.0;

    while let Some(tier_id) = snap.next_tier().map(|t| t.tier_id.clone()) {
        // 先取出要回传的字段（`snap.status` 随后会被覆盖，不能同时借用）。
        let body = serde_json::json!({
            "businessDate": snap.business_date,
            "configVersion": snap.config.version,
            "reachedTierId": tier_id,
        });
        let grant: Grant = api::get_envelope(
            with_headers(
                client.post(format!("{MODEL_BASE_URL}{PATH_REWARD}")),
                headers,
            )
            .header("Content-Type", "application/json")
            .body(body.to_string()),
        )
        .await?;

        if !grant.granted {
            break;
        }
        total += grant.granted_points;
        // 一档一标签，用服务端回传的档位名（通常只含刚申请的这档）。
        for id in &grant.granted_tier_ids {
            details.push(ClaimDetail::Label {
                en: format!("{id} +{}", fmt_num(grant.granted_points)),
                zh: format!("{id} +{}", fmt_num(grant.granted_points)),
            });
        }
        // 用发放后的状态覆盖本地快照，避免重复申请同一档位。
        snap.status = grant.status;
        // 今日已封顶 / 已出现重复：再申请也只会被拒（实测 `dailyRewardCapped` 置位后不再发分）。
        if snap.status.daily_reward_capped || grant.duplicate {
            break;
        }
    }

    if total <= 0.0 {
        // 已领过时把今日已发分数带进明细，便于对照日志。
        let details = if snap.status.earned_points_today > 0.0 {
            vec![ClaimDetail::Label {
                en: format!("earned today {}", fmt_num(snap.status.earned_points_today)),
                zh: format!("今日已发 {}", fmt_num(snap.status.earned_points_today)),
            }]
        } else {
            vec![]
        };
        return Ok(Claimed {
            outcome: ClaimOutcome::skipped(SkipReason::AlreadyClaimed, details),
            credits_delta: None,
        });
    }

    details.push(ClaimDetail::Points(total));
    Ok(Claimed {
        outcome: ClaimOutcome::success(details),
        credits_delta: Some(total),
    })
}

fn with_headers(
    mut http: reqwest::RequestBuilder,
    headers: &[(&str, String)],
) -> reqwest::RequestBuilder {
    for (k, v) in headers {
        http = http.header(*k, v);
    }
    http
}

fn fmt_num(v: f64) -> String {
    if v.fract().abs() < f64::EPSILON {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实快照（2026-11-20 实测，已删减无关字段）。
    const SNAP: &str = r#"{
      "enabled": true, "businessDate": "2026-11-20",
      "config": {"version":"pet-work-v1","dailyMaxPoints":1000,
                 "tiers":[{"tierId":"tier_1","score":60},{"tierId":"tier_2","score":100},
                          {"tierId":"tier_3","score":150}]},
      "status": {"earnedPointsToday":0,"rewardedTierIds":[],"dailyRewardCapped":false,"lastGrant":null}
    }"#;

    fn snap() -> Snapshot {
        serde_json::from_str(SNAP).unwrap()
    }

    #[test]
    fn 解析真实快照() {
        let s = snap();
        assert!(s.enabled);
        assert_eq!(s.business_date, "2026-11-20");
        assert_eq!(s.config.version, "pet-work-v1");
        assert_eq!(s.config.tiers.len(), 3);
    }

    #[test]
    fn 下一个档位按分数升序() {
        assert_eq!(snap().next_tier().unwrap().tier_id, "tier_1");
    }

    #[test]
    fn 已领档位跳过() {
        let mut s = snap();
        s.status.rewarded_tier_ids = vec!["tier_1".into(), "tier_2".into()];
        assert_eq!(s.next_tier().unwrap().tier_id, "tier_3");
    }

    #[test]
    fn 全部领完无候选() {
        let mut s = snap();
        s.status.rewarded_tier_ids = vec!["tier_1".into(), "tier_2".into(), "tier_3".into()];
        assert!(s.next_tier().is_none());
    }

    /// 真实发放响应。
    #[test]
    fn 解析发放响应() {
        let g: Grant = serde_json::from_str(
            r#"{"businessDate":"2026-11-20","grantBatchId":"pwg_x","granted":true,
                "duplicate":false,"grantedPoints":200,"grantedTierIds":["tier_1"],
                "status":{"earnedPointsToday":200,"rewardedTierIds":["tier_1"],
                          "dailyRewardCapped":false,"lastGrant":null}}"#,
        )
        .unwrap();
        assert!(g.granted);
        assert!(!g.duplicate);
        assert_eq!(g.granted_points, 200.0);
        assert_eq!(g.status.rewarded_tier_ids, ["tier_1"]);
    }

    #[test]
    fn 解析重复申请() {
        let g: Grant = serde_json::from_str(
            r#"{"granted":false,"duplicate":true,"grantedPoints":0,"grantedTierIds":[],
                "status":{"earnedPointsToday":200,"rewardedTierIds":["tier_1"]}}"#,
        )
        .unwrap();
        assert!(g.duplicate);
        assert!(!g.granted);
    }
}
