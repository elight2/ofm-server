//! 设备标识与设备密钥（服务端按 device 绑定登录，必须持久化且稳定）。
//!
//! * `machine_id` = `sha256(本机稳定标识)` 的小写十六进制（64 字符），官方客户端同算法；
//! * `device_id` = 纯数字串；
//! * `device_private_key_pem` = EC prime256v1 私钥（PKCS#8 PEM），其公钥随登录上报并被绑定，
//!   续期时需要配对私钥，**不能每次重新生成**。
//!
//! 首次登录时生成，存入 `credential.toml`；续期/再次登录沿用旧值。

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use p256::SecretKey;
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, spki::EncodePublicKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub machine_id: String,
    pub device_id: String,
    /// PKCS#8 PEM 私钥（`-----BEGIN PRIVATE KEY-----`）。
    pub device_private_key_pem: String,
    /// 设备名（登录上报用）。
    pub device_name: String,
    pub os_version: String,
    pub device_cpu: String,
}

impl Device {
    /// 复用已有值，否则生成新的。
    pub fn load_or_create(
        machine_id: Option<String>,
        device_id: Option<String>,
        pem: Option<String>,
    ) -> Result<Self, DeviceError> {
        let (machine_id, device_id, device_private_key_pem) = match (machine_id, device_id, pem) {
            (Some(m), Some(d), Some(p)) => (m, d, p),
            _ => {
                let (m, d, p) = Self::generate()?;
                (m, d, p)
            }
        };
        Ok(Self {
            machine_id,
            device_id,
            device_private_key_pem,
            device_name: device_name(),
            os_version: os_version(),
            device_cpu: cpu_model(),
        })
    }

    fn generate() -> Result<(String, String, String), DeviceError> {
        let machine_id = sha256_hex(&machine_seed());
        let device_id = random_digits(16);
        let secret = SecretKey::random(&mut OsRng);
        let pem = secret
            .to_pkcs8_pem(Default::default())
            .map_err(|e| DeviceError::Key(e.to_string()))?
            .to_string();
        Ok((machine_id, device_id, pem))
    }

    /// 上报用的 SPKI 公钥 PEM（`-----BEGIN PUBLIC KEY-----`）。
    pub fn public_key_pem(&self) -> Result<String, DeviceError> {
        let secret = SecretKey::from_pkcs8_pem(&self.device_private_key_pem)
            .map_err(|e| DeviceError::Key(e.to_string()))?;
        let public = secret.public_key();
        Ok(public
            .to_public_key_pem(Default::default())
            .map_err(|e| DeviceError::Key(e.to_string()))?
            .trim_end()
            .to_string()
            + "\n")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("设备密钥处理失败: {0}")]
    Key(String),
    /// 随机源不可用时的专用错误。当前回退到主机名派生，暂无构造点。
    #[allow(dead_code)]
    #[error("系统随机数不可用: {0}")]
    Rng(String),
}

fn machine_seed() -> String {
    // Windows：优先读 MachineGuid（系统级稳定标识，不随重装/改名变化）。
    #[cfg(windows)]
    {
        if let Some(guid) = reg_query(r"HKLM\SOFTWARE\Microsoft\Cryptography", "MachineGuid") {
            return guid;
        }
        // 退化为计算机名（改名后会变，但好过随机值）。
        if let Ok(h) = std::env::var("COMPUTERNAME")
            && !h.is_empty()
        {
            return h;
        }
        return random_seed();
    }
    #[cfg(not(windows))]
    {
        // 优先级：本机 machine-id → 主机名。二者缺失时退化为随机串。
        for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
            if let Ok(s) = std::fs::read_to_string(path) {
                let s = s.trim();
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        if let Ok(h) = std::env::var("HOSTNAME")
            && !h.is_empty()
        {
            return h;
        }
        random_seed()
    }
}

/// 随机退路（仅在拿不到任何稳定标识时使用）。
fn random_seed() -> String {
    let mut buf = [0u8; 16];
    if getrandom::fill(&mut buf).is_ok() {
        return URL_SAFE_NO_PAD.encode(buf);
    }
    "ofm-server".to_string()
}

/// 读 Windows 注册表的值（仅 Windows 可用）。
///
/// 用系统自带的 `reg.exe` 查询，避免为此引入额外 crate。
#[cfg(windows)]
fn reg_query(key: &str, value: &str) -> Option<String> {
    let out = std::process::Command::new("reg")
        .args(["query", key, "/v", value])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    // 输出形如： `    MachineGuid    REG_SZ    <值>`
    // 值本身可能含空格（如 `ProductName = Windows 10 Pro`），
    // 故按空白切分后取类型名之后的部分重新拼接。
    for line in text.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(_type)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !name.eq_ignore_ascii_case(value) {
            continue;
        }
        let v = parts.collect::<Vec<_>>().join(" ");
        if !v.is_empty() {
            return Some(v);
        }
    }
    None
}

fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn random_digits(n: usize) -> String {
    let mut buf = vec![0u8; n];
    // getrandom 失败概率极低；退化为 0 序列也能完成登录。
    let _ = getrandom::fill(&mut buf);
    buf.iter()
        .map(|b| {
            let d = b % 10;
            char::from(b'0' + d)
        })
        .collect()
}

fn device_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .map(|u| format!("{u}的电脑"))
        .unwrap_or_else(|_| "ofm-server".to_string())
}

pub fn os_version() -> String {
    #[cfg(windows)]
    {
        // 注册表里的 ProductName / DisplayVersion 比 `std::env::consts::OS` 信息量大。
        if let Some(v) = reg_query(
            r"HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion",
            "ProductName",
        ) {
            return v;
        }
        return std::env::consts::OS.to_string();
    }
    #[cfg(not(windows))]
    {
        // 优先 PRETTY_NAME，退化为内核版本。
        if let Ok(s) = std::fs::read_to_string("/etc/os-release") {
            for line in s.lines() {
                if let Some(v) = line.strip_prefix("PRETTY_NAME=") {
                    let v = v.trim().trim_matches('"');
                    if !v.is_empty() {
                        return v.to_string();
                    }
                }
            }
        }
        std::env::consts::OS.to_string()
    }
}

pub fn cpu_model() -> String {
    #[cfg(windows)]
    {
        // CPU 型号在注册表 HARDWARE 描述里，WMI 在新系统上可能不可用。
        if let Some(v) = reg_query(
            r"HKLM\HARDWARE\DESCRIPTION\System\CentralProcessor\0",
            "ProcessorNameString",
        ) {
            return v;
        }
        return String::new();
    }
    #[cfg(not(windows))]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/cpuinfo") {
            for line in s.lines() {
                if let Some(v) = line.strip_prefix("model name")
                    && let Some(v) = v.split_once(':').map(|(_, v)| v.trim())
                    && !v.is_empty()
                {
                    return v.to_string();
                }
            }
        }
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_roundtrip() {
        let d = Device::load_or_create(None, None, None).expect("生成设备");
        assert_eq!(d.machine_id.len(), 64);
        assert!(d.machine_id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(d.device_id.len(), 16);
        assert!(d.device_id.chars().all(|c| c.is_ascii_digit()));
        let pem = d.public_key_pem().expect("公钥");
        assert!(pem.starts_with("-----BEGIN PUBLIC KEY-----"));

        // 复用：再次 load 应返回相同值。
        let again = Device::load_or_create(
            Some(d.machine_id.clone()),
            Some(d.device_id.clone()),
            Some(d.device_private_key_pem.clone()),
        )
        .expect("复用设备");
        assert_eq!(again.machine_id, d.machine_id);
        assert_eq!(again.public_key_pem().unwrap(), pem);
    }
}
