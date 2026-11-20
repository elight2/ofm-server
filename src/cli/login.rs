//! `ofm-server login <供应商id>`：浏览器 OAuth + 本地回调，保存凭证。
//!
//! 流程见 `provider-info/<id>.md` §1。各家登录差异太大（回调 / 设备码 / 短信），
//! 无法泛化，故这里只做**按供应商分发**；与供应商无关的部件
//! （本地回调服务、打开浏览器、查询串解析）在 [`crate::login_util`]。
//!
//! * trae：PKCE S256 + `ExchangeToken`（`trae::auth`）
//! * raccoon：loopback 兼容登录，本地收货后 `login_with_authorization_code`
//! * codearts：PKCE(SHA-256) + DPoP(ES256)，门户授权 + STS 换 AKSK
//! * minimax：OAuth 2.0 设备码 + PKCE，**无本地回调**，打印 `user_code`
//!   后轮询令牌（需要用户在浏览器扫码 + 短信验证）
//! * loomy：讯飞账号手机号 + 短信验证码（无浏览器）

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
