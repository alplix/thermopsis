//! Optional driver mode (Windows): CPU and board sensors through the helper `thermopsis-sensors.exe` (LibreHardwareMonitorLib + the PawnIO driver).
//! The helper needs administrator rights, so it is started once with a single UAC prompt; it writes its readings to a file that the agent reads.
//! Switched on with `thermopsis-agent advanced on`; without it nothing here runs.
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

static STARTED: AtomicBool = AtomicBool::new(false);

fn marker() -> PathBuf {
    crate::config::dir().join("advanced")
}

fn out_file() -> PathBuf {
    crate::config::dir().join("driver-sensors.json")
}

pub fn enabled() -> bool {
    cfg!(windows) && marker().exists()
}

pub fn set(on: bool) -> std::io::Result<()> {
    if on {
        std::fs::create_dir_all(crate::config::dir())?;
        std::fs::write(marker(), b"1")
    } else {
        let _ = std::fs::remove_file(out_file());
        match std::fs::remove_file(marker()) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

fn helper_path() -> Option<PathBuf> {
    let p = std::env::var("THERMOPSIS_HELPER").map(PathBuf::from).unwrap_or_else(|_| std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("thermopsis-sensors.exe"))).unwrap_or_default());
    p.exists().then_some(p)
}

/// Starts the helper (once per run) when driver mode is on; the UAC prompt appears here.
pub fn start() -> Result<(), String> {
    if !enabled() || STARTED.swap(true, Ordering::Relaxed) {
        return Ok(());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let helper = helper_path().ok_or("thermopsis-sensors.exe not found next to the agent")?;
        let _ = std::fs::remove_file(out_file());
        let script = format!("Start-Process -Verb RunAs -WindowStyle Hidden -FilePath '{}' -ArgumentList '\"{}\" {} 5'", helper.display(), out_file().display(), std::process::id());
        let st = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", &script]).creation_flags(0x08000000).status().map_err(|e| e.to_string())?;
        if !st.success() {
            STARTED.store(false, Ordering::Relaxed);
            return Err("the administrator prompt was declined".into());
        }
    }
    Ok(())
}

/// Latest helper readings; empty when driver mode is off or the file is missing/old (older than 60 s)
pub fn read() -> Vec<(String, f64)> {
    if !enabled() {
        return Vec::new();
    }
    let fresh = std::fs::metadata(out_file()).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs() < 60).unwrap_or(false);
    if !fresh {
        return Vec::new();
    }
    let text = std::fs::read_to_string(out_file()).unwrap_or_default();
    parse(&text)
}

pub fn parse(text: &str) -> Vec<(String, f64)> {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(m)) => m.into_iter().filter_map(|(k, v)| v.as_f64().map(|n| (k, n))).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_helper_output() {
        let m: std::collections::HashMap<String, f64> = parse(r#"{"t.win.cpu.package":50,"p.win.cpu.package":43.91,"x":"no"}"#).into_iter().collect();
        assert_eq!(m["t.win.cpu.package"], 50.0);
        assert_eq!(m.len(), 2);
        assert!(parse("garbage").is_empty());
    }
}
