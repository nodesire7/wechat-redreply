use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub fn now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}
pub fn contact_id(name: &str) -> u64 {
    let hash = Sha256::digest(name.as_bytes());
    1 + hash[..6].iter().fold(0, |n, b| (n << 8) | u64::from(*b))
}
pub fn group_id(name: &str) -> u64 {
    contact_id(&format!("wechat-group:{name}"))
}
pub fn member_id(group: &str, name: &str) -> u64 {
    contact_id(&format!("wechat-member:{group}:{name}"))
}
pub fn base() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
pub fn load(path: &Path) -> Result<Value> {
    let s = fs::read_to_string(path)?;
    Ok(serde_json::from_str(s.trim_start_matches('\u{feff}'))?)
}
pub fn atomic(path: &Path, value: &Value) -> Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        rand::random::<u64>()
    ));
    fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    let mut result = Ok(());
    for i in 0..4 {
        result = replace(&tmp, path);
        if result.is_ok() {
            break;
        }
        if i < 3 {
            std::thread::sleep(std::time::Duration::from_millis(10 << i));
        }
    }
    let _ = fs::remove_file(tmp);
    result
}
fn replace(src: &Path, dst: &Path) -> Result<()> {
    use windows::{
        Win32::Storage::FileSystem::{
            MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        },
        core::PCWSTR,
    };
    let s = wide(src.as_os_str());
    let d = wide(dst.as_os_str());
    unsafe {
        MoveFileExW(
            PCWSTR(s.as_ptr()),
            PCWSTR(d.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )?;
    }
    Ok(())
}
pub fn wide(s: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    s.as_ref().encode_wide().chain(Some(0)).collect()
}
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct Binding {
    pub pid: u32,
    pub created_at: f64,
    pub hwnd: isize,
    #[serde(default)]
    pub bound_at: f64,
}
#[derive(Clone)]
pub struct Config {
    pub raw: Value,
    pub path: PathBuf,
}
impl Config {
    pub fn read(path: PathBuf) -> Result<Self> {
        let c = Self {
            raw: load(&path)?,
            path,
        };
        c.validate()?;
        Ok(c)
    }
    pub fn r(&self) -> &Value {
        &self.raw["redreply"]
    }
    pub fn token(&self) -> &str {
        self.r()["access_token"].as_str().unwrap_or("")
    }
    pub fn bot(&self) -> u64 {
        self.r()["self_id"].as_u64().unwrap_or(900000000001)
    }
    pub fn port(&self) -> u16 {
        self.r()["port"].as_u64().unwrap_or(8765) as u16
    }
    pub fn host(&self) -> &str {
        self.r()["listen_host"].as_str().unwrap_or("127.0.0.1")
    }
    pub fn scope(&self) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        for (field, g) in [("whitelist", false), ("group_whitelist", true)] {
            if let Some(a) = self.raw[field].as_array() {
                for n in a.iter().filter_map(Value::as_str) {
                    if !self.raw["blacklist"]
                        .as_array()
                        .is_some_and(|b| b.iter().any(|x| x.as_str() == Some(n)))
                        && !out.contains(&(n.to_owned(), g))
                    {
                        out.push((n.to_owned(), g));
                    }
                }
            }
        }
        out
    }
    pub fn binding(&self) -> Result<Binding> {
        serde_json::from_value(self.raw["wechat_binding"].clone())
            .context("请在账号页绑定已登录的微信窗口")
    }
    pub fn interval(&self) -> f64 {
        self.raw["database_poll_interval_seconds"]
            .as_f64()
            .unwrap_or(0.2)
            .clamp(0.05, 5.)
    }
    pub fn timeout(&self) -> f64 {
        self.r()["delivery_timeout_seconds"]
            .as_f64()
            .unwrap_or(20.)
            .clamp(1., 25.)
    }
    pub fn allows(&self, name: &str, g: bool) -> bool {
        self.scope()
            .iter()
            .any(|(n, group)| n == name && *group == g)
    }
    pub fn validate(&self) -> Result<()> {
        if self.token().len() < 24 {
            bail!("接入令牌至少需要 24 个字符");
        }
        if self.raw["whitelist_mode"] != true {
            bail!("必须启用白名单模式");
        }
        for f in ["whitelist", "group_whitelist"] {
            if !self.raw[f].is_array()
                || self.raw[f]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|x| x.as_str().is_none_or(|s| s.trim().is_empty()))
            {
                bail!("{f} 必须是非空名称数组（可不添加任何名称）");
            }
        }
        self.host()
            .parse::<std::net::IpAddr>()
            .context("监听地址必须为 IP 地址")?;
        if !(1..=65535).contains(&self.r()["port"].as_u64().unwrap_or(0))
            || self.bot() == 0
            || self.bot() >= 1 << 53
        {
            bail!("端口或机器人 ID 无效");
        }
        let cert = self.r()["tls_certfile"].as_str().unwrap_or("");
        let key = self.r()["tls_keyfile"].as_str().unwrap_or("");
        if cert.is_empty() != key.is_empty() {
            bail!("WSS 需同时设置证书及私钥");
        }
        Ok(())
    }
    pub fn url(&self) -> String {
        let h = self.r()["connect_host"]
            .as_str()
            .filter(|s| !s.is_empty())
            .unwrap_or(self.host());
        let h = if h.contains(':') {
            format!("[{h}]")
        } else {
            h.to_owned()
        };
        let token: String = url::form_urlencoded::byte_serialize(self.token().as_bytes()).collect();
        format!(
            "{}://{}:{}/onebot/v11?access_token={}",
            if self.r()["tls_certfile"].as_str().unwrap_or("").is_empty() {
                "ws"
            } else {
                "wss"
            },
            h,
            self.port(),
            token
        )
    }
    pub fn default(path: PathBuf) -> Self {
        let token: String = (0..48)
            .map(|_| format!("{:x}", rand::random::<u8>() % 16))
            .collect();
        Self {
            path,
            raw: json!({"whitelist_mode":true,"whitelist":[],"group_whitelist":[],"blacklist":[],"offscreen_mode":true,"require_fully_hidden":true,"receive_backend":"database","database_poll_interval_seconds":0.2,"redreply":{"listen_host":"0.0.0.0","connect_host":"127.0.0.1","port":8765,"self_id":900000000001u64,"access_token":token,"delivery_timeout_seconds":20},"redreply_management_url":"http://127.0.0.1:1207"}),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_matches_python() {
        assert_eq!(contact_id("测试好友"), 279515997009546);
        assert_ne!(member_id("群", "测试好友"), contact_id("测试好友"));
    }
    #[test]
    fn atomic_replaces() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("配置.json");
        atomic(&p, &json!({"a":1})).unwrap();
        atomic(&p, &json!({"a":2})).unwrap();
        assert_eq!(load(&p).unwrap()["a"], 2);
    }
}
