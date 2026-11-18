//! Loomy 登录。
//!
//! 流程：
//! 1. 启动本地 HTTP 回调服务（随机端口）
//! 2. 打开浏览器访问 Loomy 登录页
//! 3. 等待 OAuth 回调，换取 token

use std::time::Duration;

use anyhow::Context;
use reqwest::Client;

use crate::cli::login::{open_browser, wait_callback_on, query_param};
use crate::providers::loomy::api::HttpError;
use crate::providers::loomy::credential::Credential;

/// 登录入口。
pub async fn login(root: std::path::PathBuf) -> anyhow::Result<()> {
    let client = Client::new();
    let port = find_free_port().await?;

    // 1. 获取授权 URL
    let _auth_url = format!("http://127.0.0.1:{port}/auth");
    println!("[loomy] 正在启动登录流程...");
    println!("[loomy] 请在浏览器中完成登录");

    // 2. 打开浏览器
    open_browser(&format!("http://127.0.0.1:{port}/login")).context("打开浏览器失败")?;

    // 3. 等待回调
    let query = wait_callback_on(port, "/callback", Duration::from_secs(300)).await?;
    let code = query_param(&query, "code").context("回调中缺少 code 参数")?;
    let state = query_param(&query, "state").unwrap_or_default();

    // 4. 用 code 换取 token
    let cred = exchange_code(&client, &code, &state).await?;
    cred.store(&root)?;

    println!("[loomy] 登录成功！");
    println!("  凭证已保存到: {}", crate::config::Config::credential_path(&root, "loomy").display());
    Ok(())
}

/// 查找可用端口。
async fn find_free_port() -> Result<u16, anyhow::Error> {
    use tokio::net::TcpListener;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    Ok(addr.port())
}

/// 用授权码换取凭证。
async fn exchange_code(client: &Client, code: &str, state: &str) -> Result<Credential, anyhow::Error> {
    let url = "https://api.loomy.ai/oauth/token";
    let body = serde_json::json!({
        "code": code,
        "state": state,
        "grant_type": "authorization_code"
    });

    let resp = client.post(url)
        .json(&body)
        .send()
        .await
        .context("请求 token 失败")?;

    if !resp.status().is_success() {
        let text = resp.text().await.unwrap_or_default();
        anyhow::bail!("token 请求失败: {}", text);
    }

    let data: serde_json::Value = resp.json().await.context("解析 token 响应失败")?;

    let token = data.get("access_token")
        .and_then(|v| v.as_str())
        .context("响应中缺少 access_token")?
        .to_string();
    let base_url = data.get("base_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://api.loomy.ai")
        .to_string();
    let token_expire_at = data.get("expires_in")
        .and_then(|v| v.as_i64())
        .map(|exp| {
            use std::time::SystemTime;
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            now + exp * 1000
        });
    let refresh_token = data.get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Ok(Credential {
        token,
        base_url,
        token_expire_at,
        refresh_token,
        refresh_expire_at: None,
    })
}

/// 刷新 token。
pub async fn refresh(client: &Client, cred: &Credential) -> Result<Credential, HttpError> {
    let refresh_token = cred.refresh_token.as_ref().ok_or_else(|| {
        HttpError::Status {
            status: 400,
            body: "缺少 refresh_token".to_string(),
        }
    })?;

    let url = "https://api.loomy.ai/oauth/token";
    let body = serde_json::json!({
        "refresh_token": refresh_token,
        "grant_type": "refresh_token"
    });

    let resp = client.post(url)
        .json(&body)
        .send()
        .await
        .map_err(HttpError::Transport)?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(HttpError::Status {
            status: status.as_u16(),
            body,
        });
    }

    let data: serde_json::Value = resp.json().await.map_err(HttpError::Transport)?;

    let token = data.get("access_token")
        .and_then(|v| v.as_str())
        .unwrap_or(&cred.token)
        .to_string();
    let token_expire_at = data.get("expires_in")
        .and_then(|v| v.as_i64())
        .map(|exp| {
            use std::time::SystemTime;
            let now = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64;
            now + exp * 1000
        });
    let refresh_token = data.get("refresh_token")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| cred.refresh_token.clone().unwrap());

    Ok(Credential {
        token,
        base_url: cred.base_url.clone(),
        token_expire_at,
        refresh_token: Some(refresh_token),
        refresh_expire_at: None,
    })
}
