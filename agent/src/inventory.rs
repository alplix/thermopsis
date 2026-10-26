//! What kind of computer this is, for the project's product lists. Never sent: serial numbers, MAC or IP addresses, the computer's name, user names, UUIDs.
use serde_json::{json, Value};
use std::process::{Command, Stdio};
use sysinfo::{Disks, System};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// stdout of a child process, or "" on any failure; no window on Windows, no stdin
fn run(cmd: &str, args: &[&str]) -> String {
    let mut c = Command::new(cmd);
    c.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    c.creation_flags(0x08000000);
    match c.output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => String::new(),
    }
}

fn dmi() -> Value {
    let mut v = serde_json::Map::new();
    #[cfg(windows)]
    {
        for (key, name) in [("sys_vendor", "SystemManufacturer"), ("product", "SystemProductName"), ("board_vendor", "BaseBoardManufacturer"), ("board", "BaseBoardProduct"), ("bios_vendor", "BIOSVendor"), ("bios_version", "BIOSVersion")] {
            let out = run("reg", &["query", r"HKLM\HARDWARE\DESCRIPTION\System\BIOS", "/v", name]);
            // "    SystemProductName    REG_SZ    Victus by HP 15L ..."
            if let Some(line) = out.lines().find(|l| l.contains(name)) {
                if let Some(pos) = line.find("REG_SZ") {
                    let val = line[pos + 6..].trim();
                    if !val.is_empty() {
                        v.insert(key.to_string(), json!(val));
                    }
                }
            }
        }
    }
    #[cfg(target_os = "macos")]
    {
        let model = run("sysctl", &["-n", "hw.model"]);
        if !model.is_empty() {
            v.insert("sys_vendor".to_string(), json!("Apple"));
            v.insert("product".to_string(), json!(model));
        }
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        for (key, file) in [("sys_vendor", "sys_vendor"), ("product", "product_name"), ("board_vendor", "board_vendor"), ("board", "board_name"), ("bios_vendor", "bios_vendor"), ("bios_version", "bios_version")] {
            if let Ok(s) = std::fs::read_to_string(format!("/sys/class/dmi/id/{}", file)) {
                let s = s.trim();
                if !s.is_empty() {
                    v.insert(key.to_string(), json!(s));
                }
            }
        }
    }
    Value::Object(v)
}


/// Graphics cards: Windows lists them with ids and names (no administrator rights needed); the driver tool adds memory and PCIe maximum for NVIDIA cards.
fn gpus() -> Vec<Value> {
    let nv = crate::nvidia::inventory();
    let mut out: Vec<Value> = Vec::new();
    #[cfg(windows)]
    {
        let text = run("powershell", &["-NoProfile", "-NonInteractive", "-Command", "Get-CimInstance Win32_VideoController | Select-Object Name,PNPDeviceID,DriverVersion | ConvertTo-Json -Compress"]);
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            let items: Vec<Value> = match v {
                Value::Array(a) => a,
                o @ Value::Object(_) => vec![o],
                _ => vec![],
            };
            let mut k = 0usize;
            for it in items {
                let name = it["Name"].as_str().unwrap_or("").to_string();
                let pnp = it["PNPDeviceID"].as_str().unwrap_or("").to_uppercase();
                let low = name.to_lowercase();
                if name.is_empty() || low.contains("basic") || low.contains("remote") || low.contains("virtual") || low.contains("parsec") {
                    continue;
                }
                let hex = |tag: &str| pnp.split(tag).nth(1).map(|r| r.chars().take(4).collect::<String>().to_lowercase()).unwrap_or_default();
                let (ven, dev) = (hex("VEN_"), hex("DEV_"));
                let mut g = json!({"vendor": format!("0x{}", ven), "device": format!("0x{}", dev), "name": name, "driver_version": it["DriverVersion"].as_str().unwrap_or("")});
                if ven == "10de" {
                    if let Some((_, _, vram, gen, width)) = nv.get(k) {
                        g["vram_mb"] = json!(vram);
                        g["pcie_gen_max"] = json!(gen);
                        g["pcie_width_max"] = json!(width);
                    }
                    k += 1;
                }
                out.push(g);
            }
        }
    }
    #[cfg(not(windows))]
    {
        for (name, drv, vram, gen, width) in &nv {
            out.push(json!({"vendor": "0x10de", "device": "", "name": name, "driver_version": drv, "vram_mb": vram, "pcie_gen_max": gen, "pcie_width_max": width}));
        }
        #[cfg(target_os = "macos")]
        {
            let mut sys = System::new();
            sys.refresh_cpu_all();
            if let Some(c) = sys.cpus().first() {
                if c.brand().starts_with("Apple") {
                    out.push(json!({"vendor": "0x106b", "device": "", "name": format!("{} GPU", c.brand().trim())}));
                }
            }
        }
    }
    out
}

/// Drives with their model names where the system tells them without administrator rights (Windows: Get-PhysicalDisk); None = use the generic list.
fn physical_disks() -> Option<Vec<Value>> {
    #[cfg(windows)]
    {
        let text = run("powershell", &["-NoProfile", "-NonInteractive", "-Command", "Get-PhysicalDisk | Select-Object FriendlyName,MediaType,BusType,Size | ConvertTo-Json -Compress"]);
        let v: Value = serde_json::from_str(&text).ok()?;
        let items: Vec<Value> = match v {
            Value::Array(a) => a,
            o @ Value::Object(_) => vec![o],
            _ => return None,
        };
        let mut out = Vec::new();
        for it in items {
            let bus = it["BusType"].as_str().map(|s| s.to_string()).or_else(|| it["BusType"].as_u64().map(|n| if n == 17 { "NVMe".to_string() } else { n.to_string() })).unwrap_or_default();
            let media = it["MediaType"].as_str().unwrap_or("").to_string();
            let kind = if bus.eq_ignore_ascii_case("nvme") { "nvme" } else if media.eq_ignore_ascii_case("ssd") { "ssd" } else if media.eq_ignore_ascii_case("hdd") { "hdd" } else { "" };
            let gb = it["Size"].as_u64().unwrap_or(0) / 1_000_000_000;
            let model = it["FriendlyName"].as_str().unwrap_or("").trim().to_string();
            if gb > 0 && !model.is_empty() && !bus.eq_ignore_ascii_case("usb") {
                out.push(json!({"kind": kind, "model": model, "gb": gb}));
            }
        }
        if out.is_empty() { None } else { Some(out) }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub fn collect() -> Value {
    let mut sys = System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();
    let cpu_model = sys.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default();
    let mut storage: Vec<Value> = Vec::new();
    let mut seen: Vec<u64> = Vec::new();
    for d in Disks::new_with_refreshed_list().list() {
        let gb = d.total_space() / 1_000_000_000;
        if gb == 0 || d.is_removable() || seen.contains(&d.total_space()) {
            continue;
        }
        seen.push(d.total_space());
        let kind = match d.kind() {
            sysinfo::DiskKind::SSD => "ssd",
            sysinfo::DiskKind::HDD => "hdd",
            _ => "",
        };
        storage.push(json!({"kind": kind, "gb": gb}));
    }
    if let Some(disks) = physical_disks() {
        storage = disks;
    }
    json!({
        "cpu": {"model": cpu_model, "threads": sys.cpus().len()},
        "dmi": dmi(),
        "mem_kb": sys.total_memory() / 1024,
        "storage": storage,
        "gpus": gpus(),
        "kernel": System::kernel_version().unwrap_or_default(),
        "machine": std::env::consts::ARCH,
        "os": format!("{} {}", System::name().unwrap_or_default(), System::os_version().unwrap_or_default()).trim().to_string(),
        "virtual_machine_hint": false,
        "container_hint": false,
        "app": {"name": "thermopsis-agent", "version": crate::VERSION},
    })
}
