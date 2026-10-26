//! Thermopsis desktop agent (library): configuration, the BitBOINC protocol client, report windows, sensors, inventory, self tests and the run loop.
pub mod agent;
pub mod config;
pub mod inventory;
pub mod nvidia;
pub mod proto;
pub mod report;
pub mod selftest;
pub mod sensors;
pub mod windisk;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The operating system as the server knows it ("windows", "macos", "linux").
pub fn os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}
