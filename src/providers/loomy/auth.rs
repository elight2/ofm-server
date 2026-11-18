//! Loomy 登录。
//!
//! Loomy 使用 API Key 认证（Bearer token）。
//! 用户需从 Loomy 客户端设置中获取 API Key，直接配置即可。

use std::io::{self, Write};

use anyhow::{Context, Result};
use reqwest::Client;

use crate::providers::loomy::api::HttpError;
use crate::providers::loomy::credential::Credential;

/// 登录入口。
///
/// Loomy 使用 API Key 认证，无需 OAuth 流程。
/// 用户需提供 API Key（从 Loomy 客户端设置中获取）。
pub async fn login(root: std::path::PathBuf) -> Result<()> {
    println!("[loomy] Loomy 使用 API Key 认证");
    println!("  请从 Loomy 客户端设置中获取 API Key");
    println!("  （设置 → 模型服务 → API Key）");
    println!();

    // 读取 API Key
    let api_key = read_password("请输入 API Key: ")?;
    if api_key.trim().is_empty() {
        anyhow::bail!("API Key 不能为空");
    }

    // 读取 Base URL（可选，默认 https://api.loomy.ai）
    println!();
    println!("  默认 API 地址: https://api.loomy.ai");
    println!("  （如使用自定义地址请在此输入，否则直接回车）");
    let base_url_input = read_line("请输入 API 地址（回车使用默认）: ")?;
    let base_url = if base_url_input.trim().is_empty() {
        "https://api.loomy.ai".to_string()
    } else {
        base_url_input.trim().to_string()
    };

    // 验证 API Key 是否有效
    println!();
    println!("[loomy] 正在验证 API Key...");
    let client = Client::new();
    match verify_key(&client, &api_key, &base_url).await {
        Ok(true) => {
            println!("[loomy] API Key 验证成功");
        }
        Ok(false) => {
            println!("[loomy] 警告: API Key 验证失败（可能是余额不足或权限问题）");
            println!("  仍将继续保存，请稍后检查状态");
        }
        Err(e) => {
            println!("[loomy] 警告: 验证请求失败: {e}");
            println!("  仍将继续保存，请稍后检查状态");
        }
    }

    // 保存凭证
    let cred = Credential {
        token: api_key.trim().to_string(),
        base_url,
        token_expire_at: None,
        refresh_token: None,
        refresh_expire_at: None,
    };
    cred.store(&root)?;

    println!();
    println!("[loomy] 登录成功！");
    println!("  凭证已保存到: {}", crate::config::Config::credential_path(&root, "loomy").display());
    Ok(())
}

/// 验证 API Key 是否有效。
async fn verify_key(client: &Client, api_key: &str, base_url: &str) -> Result<bool> {
    let url = format!("{}/models", base_url.trim_end_matches('/'));

    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .context("验证请求失败")?;

    Ok(resp.status().is_success())
}

/// 读取密码（不回显）。
fn read_password(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush().context("刷新 stdout 失败")?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("读取输入失败")?;

    Ok(input.trim_end().to_string())
}

/// 读取一行文本。
fn read_line(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush().context("刷新 stdout 失败")?;

    let mut input = String::new();
    io::stdin()
        .read_line(&mut input)
        .context("读取输入失败")?;

    Ok(input.trim_end().to_string())
}

/// 刷新 token。
///
/// Loomy 的 API Key 通常长期有效，无需刷新。
/// 如 API Key 已过期，用户需重新登录。
pub async fn refresh(client: &Client, cred: &Credential) -> Result<Credential, HttpError> {
    // 验证当前 token 是否有效
    match verify_key(client, &cred.token, &cred.base_url).await {
        Ok(true) => {
            // Token 仍有效，返回原凭证
            return Ok(Credential {
                token: cred.token.clone(),
                base_url: cred.base_url.clone(),
                token_expire_at: None,
                refresh_token: None,
                refresh_expire_at: None,
            });
        }
        Ok(false) => {
            return Err(HttpError::Status {
                status: 401,
                body: "API Key 已失效，请重新登录".to_string(),
            });
        }
        Err(e) => {
            return Err(HttpError::Status {
                status: 500,
                body: format!("验证失败: {e}"),
            });
        }
    }
}
