//! NVIDIA graphics cards through the driver's own tool `nvidia-smi` (present wherever the driver is installed, no administrator rights needed). The keys are the same as
//! the Linux monitor's, so the server's analysis (GPU temperature under load, power, throttling) works for every platform.
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

static MISSING: AtomicBool = AtomicBool::new(false);

const FIELDS: &str = "index,temperature.gpu,power.draw,fan.speed,clocks.sm,utilization.gpu,\
clocks_throttle_reasons.hw_thermal_slowdown,clocks_throttle_reasons.sw_thermal_slowdown,\
memory.used,memory.total,clocks.mem,clocks.max.mem,clocks.max.sm,power.limit,pstate,\
clocks_throttle_reasons.sw_power_cap,clocks_throttle_reasons.hw_slowdown,clocks_throttle_reasons.hw_power_brake_slowdown,\
pcie.link.gen.current,pcie.link.gen.max,pcie.link.width.current,pcie.link.width.max";

fn smi(query: &str) -> Option<String> {
    if MISSING.load(Ordering::Relaxed) {
        return None;
    }
    let mut c = Command::new("nvidia-smi");
    c.arg(format!("--query-gpu={}", query)).arg("--format=csv,noheader,nounits").stdin(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    c.creation_flags(0x08000000);
    match c.output() {
        Ok(o) if o.status.success() => Some(String::from_utf8_lossy(&o.stdout).to_string()),
        Ok(_) => None, // the tool exists but failed this time (driver busy): try again next time
        Err(_) => {
            MISSING.store(true, Ordering::Relaxed); // no such program: never try again
            None
        }
    }
}

/// "[N/A]", "[Not Supported]" and empty fields have no value
fn num(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('[') {
        None
    } else {
        s.parse::<f64>().ok()
    }
}

fn active(s: &str) -> Option<bool> {
    let s = s.trim();
    if s.is_empty() || s.starts_with('[') {
        None
    } else {
        Some(s.contains("Active") && !s.contains("Not"))
    }
}

/// readings of every card, keys "<kind>.nvml.<index>.<name>"
pub fn read() -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let text = match smi(FIELDS) {
        Some(t) => t,
        None => return out,
    };
    for line in text.lines() {
        out.extend(parse_line(line));
    }
    out
}

pub fn parse_line(line: &str) -> Vec<(String, f64)> {
    let f: Vec<&str> = line.trim().split(", ").collect();
    let mut out: Vec<(String, f64)> = Vec::new();
    if f.len() < 22 {
        return out;
    }
    let i = match num(f[0]) {
        Some(v) => v as i64,
        None => return out,
    };
    let mut put = |key: &str, v: Option<f64>| {
        if let Some(v) = v {
            out.push((format!("{}", key.replace("{}", &i.to_string())), v));
        }
    };
    put("t.nvml.{}.gpu", num(f[1]));
    put("p.nvml.{}.board", num(f[2]));
    put("f.nvml.{}.fan_percent", num(f[3]));
    put("c.nvml.{}.sm_mhz", num(f[4]));
    put("u.nvml.{}.gpu_percent", num(f[5]));
    if let (Some(a), Some(b)) = (active(f[6]), active(f[7])) {
        put("x.nvml.{}.thermal_throttle", Some(if a || b { 1.0 } else { 0.0 }));
    }
    if let (Some(used), Some(total)) = (num(f[8]), num(f[9])) {
        if total > 0.0 {
            put("m.nvml.{}.vram_used_percent", Some(100.0 * used / total));
            put("x.nvml.{}.vram_used_mb", Some(used));
        }
    }
    put("c.nvml.{}.mem_mhz", num(f[10]));
    put("c.nvml.{}.mem_max_mhz", num(f[11]));
    put("c.nvml.{}.sm_max_mhz", num(f[12]));
    put("p.nvml.{}.limit", num(f[13]));
    if f[14].trim().starts_with('P') {
        put("x.nvml.{}.pstate", num(&f[14].trim()[1..]));
    }
    if let Some(b) = active(f[15]) {
        put("x.nvml.{}.thr_power", Some(if b { 1.0 } else { 0.0 }));
    }
    if let (Some(a), Some(b)) = (active(f[16]), active(f[17])) {
        put("x.nvml.{}.thr_hardware", Some(if a || b { 1.0 } else { 0.0 }));
    }
    put("c.nvml.{}.pcie_gen", num(f[18]));
    put("c.nvml.{}.pcie_gen_max", num(f[19]));
    put("c.nvml.{}.pcie_width", num(f[20]));
    put("c.nvml.{}.pcie_width_max", num(f[21]));
    out
}

/// For the inventory: (name, driver version, memory MB, PCIe generation maximum, PCIe width maximum) of every card
pub fn inventory() -> Vec<(String, String, f64, f64, f64)> {
    let mut v = Vec::new();
    if let Some(t) = smi("name,driver_version,memory.total,pcie.link.gen.max,pcie.link.width.max") {
        for line in t.lines() {
            let f: Vec<&str> = line.trim().split(", ").collect();
            if f.len() >= 5 {
                v.push((f[0].to_string(), f[1].to_string(), num(f[2]).unwrap_or(0.0), num(f[3]).unwrap_or(0.0), num(f[4]).unwrap_or(0.0)));
            }
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_line() {
        let l = "0, 55, 107.27, 38, 2452, 4, Not Active, Not Active, 3120, 16303, 13801, 13801, 3105, 300.00, P0, Not Active, Not Active, Not Active, 4, 5, 16, 16";
        let m: std::collections::HashMap<String, f64> = parse_line(l).into_iter().collect();
        assert_eq!(m["t.nvml.0.gpu"], 55.0);
        assert_eq!(m["p.nvml.0.board"], 107.27);
        assert_eq!(m["x.nvml.0.thermal_throttle"], 0.0);
        assert_eq!(m["x.nvml.0.pstate"], 0.0);
        assert!((m["m.nvml.0.vram_used_percent"] - 19.1).abs() < 0.2);
        assert_eq!(m["c.nvml.0.pcie_gen_max"], 5.0);
        assert_eq!(m["c.nvml.0.pcie_width"], 16.0);
    }

    #[test]
    fn missing_values_are_skipped_not_zero() {
        let l = "1, 60, [N/A], [Not Supported], 300, 0, Active, Not Active, 100, 8000, 400, 7000, 2100, 120.00, P8, Not Active, Active, Not Active, 1, 4, 8, 16";
        let m: std::collections::HashMap<String, f64> = parse_line(l).into_iter().collect();
        assert!(!m.contains_key("p.nvml.1.board") && !m.contains_key("f.nvml.1.fan_percent"));
        assert_eq!(m["x.nvml.1.thermal_throttle"], 1.0);
        assert_eq!(m["x.nvml.1.thr_hardware"], 1.0);
        assert_eq!(m["x.nvml.1.pstate"], 8.0);
        assert!(parse_line("garbage").is_empty());
    }
}
