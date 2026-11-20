//! Loomy 登录：讯飞账号手机号 + 短信验证码。
//!
//! 协议见 `provider-info/loomy.md` §1、实现见 [`super::account`]。
//! 登录态有效期 14 天，无续期令牌，过期只能重新登录。

use std::io::{self, Write};

use anyhow::{Context, Result, bail};

use super::account::AccountClient;
use super::credential::Credential;

/// 登录入口：手机号 → 验证码 → 换取 session。
pub async fn login(root: std::path::PathBuf) -> Result<()> {
    println!("[loomy] 使用讯飞账号登录（手机号 + 短信验证码）");
    println!();

    let phone = read_line("手机号: ")?;
    let phone = phone.trim();
    if !is_mobile(phone) {
        bail!("手机号格式不正确（需 11 位，1[3-9] 开头）");
    }

    let client = AccountClient::new().context("构建 HTTP 客户端失败")?;

    println!("[loomy] 正在发送验证码...");
    let msgid = client.send_sms_code(phone).await?;
    println!("[loomy] 验证码已发送，有效期 5 分钟");
    println!();

    let code = read_line("验证码: ")?;
    let code = code.trim();
    if code.is_empty() {
        bail!("验证码不能为空");
    }

    println!();
    println!("[loomy] 正在登录...");
    let session = client.login_by_sms_code(phone, code, &msgid).await?;

    // 用刚拿到的 session 回查一次账号信息，确认登录态真的可用。
    let client = AccountClient::new().context("构建 HTTP 客户端失败")?;
    if !verify(&client, &session.session).await? {
        bail!("登录后校验失败：账号网关不认可该 session，请重试");
    }

    let cred = Credential::from_login(session.session.clone(), session.userid.clone());
    cred.store(&root).map_err(|e| anyhow::anyhow!("{e}"))?;

    let expire = cred
        .expire_at
        .map(format_local)
        .unwrap_or_else(|| "未知".to_string());
    println!();
    println!("[loomy] 登录成功");
    println!("  userid: {}", session.userid);
    println!("  到期时间: {expire}（14 天，无自动续期）");
    println!(
        "  凭证已保存到: {}",
        crate::core::paths::credential_path(&root, super::ID).display()
    );
    Ok(())
}

/// 校验当前登录态；失效返回 `false`。
pub async fn verify(client: &AccountClient, session: &str) -> Result<bool> {
    match client.query_base_info(session).await {
        Ok(_) => Ok(true),
        Err(e) => {
            let text = e.to_string();
            // 网关用 020002 / 100002 表示登录态无效，其余错误照常抛出。
            if text.contains("020002") || text.contains("100002") {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
}

/// 中国大陆手机号：11 位，`1[3-9]` 开头。
pub fn is_mobile(phone: &str) -> bool {
    let bytes = phone.as_bytes();
    bytes.len() == 11
        && bytes[0] == b'1'
        && (b'3'..=b'9').contains(&bytes[1])
        && bytes.iter().all(u8::is_ascii_digit)
}

/// 毫秒时间戳转本地可读时间（`YYYY-MM-DD HH:MM`，UTC）。
fn format_local(millis: i64) -> String {
    let secs = millis.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// 由「1970-01-01 起的天数」还原公历年月日。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// 读一行输入（`prompt` 后等待回车）。
fn read_line(prompt: &str) -> Result<String> {
    print!("{prompt}");
    io::stdout().flush().context("刷新 stdout 失败")?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).context("读取输入失败")?;
    Ok(input.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 手机号校验() {
        assert!(is_mobile("13800138000"));
        assert!(is_mobile("19912345678"));
        assert!(!is_mobile("12800138000"));
        assert!(!is_mobile("1380013800"));
        assert!(!is_mobile("138001380001"));
        assert!(!is_mobile("1380013800a"));
        assert!(!is_mobile(""));
    }

    #[test]
    fn 到期时间可读() {
        // 2026-09-24 00:00:00 UTC
        assert_eq!(format_local(20_720 * 86_400_000), "2026-09-24 00:00 UTC");
    }
}
