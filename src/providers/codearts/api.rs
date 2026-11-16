//! CodeArts HTTP 层：域名常量与客户端构建。
//!
//! 域名取自扩展 `product.json`（`commercialVersionDomain.newFramework.productDomain`），
//! 本地包已核对（见 `provider-info/codearts.md`）。

/// 授权门户（`codeartsWebNewUrl`，即扩展里的 `portalHcAuthorizeUrl`）。
pub const PORTAL_AUTHORIZE: &str = "https://codearts.huaweicloud.com";
/// IAM STS Open：token 端点（`iamStsOpenDomain`）。
pub const IAM_STS_OPEN: &str = "https://sts.cn-north-4.myhuaweicloud.com";
/// SnapEngine 网关（`chatDomain`）：模型目录 / 对话 / 签到走这里。
pub const SNAP_GATEWAY: &str = "https://snap-access.cn-north-4.myhuaweicloud.com";
/// 国内备用域（`snapApigBackupDomain`）。
#[allow(dead_code)]
pub const SNAP_GATEWAY_BACKUP: &str = "https://snap-access.cn-north-4.myhuaweicloud.cn";

/// `client_id` = 扩展 package.json 的 `name`（`QH()` 的实际返回值）。
/// 注意 `provider-info/codearts.md` 早期写的是 `vscode-codebot`，那是 fallback。
pub const CLIENT_ID: &str = "codearts-snap";
/// 授权 URL 的 `plugin-name`（`${Lf()}_vscode`，非内部版为 `snap`）。
pub const PLUGIN_NAME: &str = "snap_vscode";
/// `plugin-version`（扩展 package.json `version`）。
pub const PLUGIN_VERSION: &str = "26.11.100";

pub const TOKEN_PATH: &str = "/v1/oauth2/tokens";

/// token 端点请求 UA（扩展用 axios 默认 UA，即 Node 的 axios 标识）。
pub const UA_TOKEN: &str = "axios/1.7.9";

/// 换 token 客户端：60s 超时、无 cookie、禁代理（官方 `proxy:!1`）。
pub fn token_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(UA_TOKEN)
        .timeout(std::time::Duration::from_secs(60))
        .no_proxy()
        .build()
}

/// 对话客户端：不能设总超时（SSE 可持续数分钟），只限制连接/空闲读取。
pub fn chat_client() -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent("codearts/1.0.0 (ofm-server)")
        .connect_timeout(std::time::Duration::from_secs(30))
        .read_timeout(std::time::Duration::from_secs(600))
        .no_proxy()
        .build()
}

/// HTTP 层错误（登录/续期共用）。
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("上游 HTTP {status}: {body}")]
    Status { status: u16, body: String },
    #[error("上游请求失败: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("上游响应异常: {0}")]
    Protocol(String),
}
