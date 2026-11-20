//! Loomy HTTP 客户端与共享常量。

use reqwest::Client;

/// 构建 HTTP 客户端。
pub fn client() -> Result<Client, reqwest::Error> {
    Ok(Client::new())
}

/// 客户端版本。
pub const VERSION: &str = "0.9.39";

/// 模型 API 基地址（对话、模型目录、积分）。
///
/// 注意不是 `api.loomy.ai`：那个域名只用于客户端升级检查
/// （`/api/v1/version/check`）。模型侧走讯飞网关，见 `provider-info/loomy.md`。
pub const MODEL_BASE_URL: &str = "https://loomyad.xunfei.cn/api/v1";
