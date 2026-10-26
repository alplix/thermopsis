//! The agent's settings: server, a random device id (made once), the revocable token of this device. No account key and no password is ever stored.
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

pub const DEFAULT_SERVER: &str = "https://bitboinc.athena.org.tr";

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Config {
    #[serde(default)]
    pub server: String,
    /// 32 hex characters, random, made on first start; identifies this installation to the project (not the hardware)
    #[serde(default)]
    pub device: String,
    /// the name shown on the website (default: the operating system; never the computer's own name)
    #[serde(default)]
    pub name: String,
    /// empty when signed out
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub user: String,
    /// "paper" (light) or "arcade" (dark 8-bit), used by the window
    #[serde(default)]
    pub theme: String,
}

pub fn dir() -> PathBuf {
    if let Ok(p) = std::env::var("THERMOPSIS_HOME") {
        return PathBuf::from(p);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("Thermopsis")
}

fn file() -> PathBuf {
    dir().join("config.json")
}

fn random_hex(bytes: usize) -> String {
    let mut b = vec![0u8; bytes];
    if getrandom::getrandom(&mut b).is_err() {
        // never happens on supported systems; fall back to the clock so that the id is at least not constant
        let t = crate::now().to_le_bytes();
        for (i, x) in b.iter_mut().enumerate() {
            *x = t[i % 8] ^ (i as u8).wrapping_mul(31);
        }
    }
    b.iter().map(|x| format!("{:02x}", x)).collect()
}

pub fn load() -> Config {
    let mut c: Config = std::fs::read_to_string(file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let mut changed = false;
    if c.server.is_empty() {
        c.server = std::env::var("THERMOPSIS_SERVER").unwrap_or_else(|_| DEFAULT_SERVER.to_string());
        changed = true;
    }
    if c.device.len() != 32 {
        c.device = random_hex(16);
        changed = true;
    }
    if c.name.is_empty() {
        c.name = default_name();
        changed = true;
    }
    if c.theme.is_empty() {
        c.theme = "paper".to_string();
        changed = true;
    }
    if changed {
        let _ = save(&c);
    }
    c
}

pub fn default_name() -> String {
    match crate::os_name() {
        "windows" => "Windows computer".to_string(),
        "macos" => "Mac".to_string(),
        _ => "Linux computer".to_string(),
    }
}

pub fn save(c: &Config) -> std::io::Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = dir().join("config.json.tmp");
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(serde_json::to_string_pretty(c).unwrap_or_default().as_bytes())?;
        f.sync_all()?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, file())
}
