//! Trae 登录（占位实现）。
//!
//! 目标流程（`provider-info/trae.md` §1）：PKCE S256 → 浏览器授权 → `GetPCAuthCode`
//! → `ExchangeToken` → 保存 `credential.toml`。
//!
//! 当前仅打印待实现步骤，**不做任何网络请求**，方便先联调 CLI 骨架。

use std::path::PathBuf;
use std::time::Duration;

use crate::cli::login;

/// 官方回调端口。
pub const CALLBACK_PORT: u16 = 40909;
pub const CALLBACK_TIMEOUT: Duration = Duration::from_secs(300);
pub const CLIENT_ID: &str = "ono9krqynydwx5";

pub async fn login(_root: PathBuf) -> anyhow::Result<()> {
    // TODO(协议): 生成 code_verifier / code_challenge(S256)、machine_id、device_id、
    // EC P-256 密钥；拼装授权 URL；GetPCAuthCode；ExchangeToken；写 credential.toml。
    let authorize = format!(
        "https://www.trae.cn/authorization?login_version=1&auth_from=trae&client_id={CLIENT_ID}\
         &auth_callback_url=http://127.0.0.1:{CALLBACK_PORT}/callback"
    );
    println!("[trae 占位] 登录尚未实现，将打开授权页以便验证浏览器链路：\n{authorize}");

    login::open_browser(&authorize)?;
    let query = login::wait_callback(CALLBACK_PORT, CALLBACK_TIMEOUT).await?;
    println!("[trae 占位] 收到回调参数: {query}");
    println!("[trae 占位] 未保存凭证（协议待实现）");
    Ok(())
}
