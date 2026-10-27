//! Reading the computer: the cross-platform baseline (processor load and clocks, memory, the temperatures the system offers). Platform-specific readers
//! (processor package temperature and power on Windows, Apple Silicon sensors, disk SMART) plug in through the same trait.
use sysinfo::{Components, System};

pub trait Sensors {
    /// every reading as (key, value); keys follow "<kind>.<source>.<name>" (kinds: t temperature, f fan, p power, c clock, u load, m memory, b battery, x other)
    fn read(&mut self) -> Vec<(String, f64)>;
}

/// Keys that mean "the processor package temperature" for the analysis on the server (see thermopsis_hw.py PKG_KEYS).
pub fn package_key() -> &'static str {
    match crate::os_name() {
        "windows" => "t.win.cpu.package",
        "macos" => "t.mac.soc.cpu",
        _ => "t.hwmon.coretemp.Package id 0",
    }
}

/// no dots (the dot separates the parts of a key), no control characters, short
pub fn clean(label: &str) -> String {
    let s: String = label.chars().map(|c| if c.is_control() || c == '.' || c == '"' || c == '\\' { '_' } else { c }).collect();
    s.trim().chars().take(60).collect()
}

/// Which of the system's temperature labels is the processor? The hottest label that names the processor, package, die or SoC.
pub fn pick_cpu(labels: &[(String, f64)]) -> Option<f64> {
    let mut best: Option<f64> = None;
    for (label, t) in labels {
        let l = label.to_lowercase();
        let cpuish = l.contains("package") || l.contains("tdie") || l.contains("tctl") || l.contains("cpu") || l.contains("soc") || l.contains("core") || l.contains("proc");
        let notcpu = l.contains("gpu") || l.contains("nvme") || l.contains("ssd") || l.contains("battery") || l.contains("wifi") || l.contains("nand");
        if cpuish && !notcpu {
            best = Some(best.map_or(*t, |b: f64| b.max(*t)));
        }
    }
    best
}

pub struct Generic {
    sys: System,
    comps: Components,
    #[cfg(target_os = "macos")]
    power: Option<crate::macsensors::Power>,
}

impl Generic {
    pub fn new() -> Generic {
        let mut sys = System::new();
        sys.refresh_cpu_all();
        sys.refresh_memory();
        Generic {
            sys,
            comps: Components::new_with_refreshed_list(),
            #[cfg(target_os = "macos")]
            power: crate::macsensors::Power::new(),
        }
    }
}

impl Default for Generic {
    fn default() -> Self {
        Self::new()
    }
}

impl Sensors for Generic {
    fn read(&mut self) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = Vec::new();
        self.sys.refresh_cpu_all();
        self.sys.refresh_memory();
        self.comps.refresh(false);
        out.push(("u.cpu.percent".to_string(), self.sys.global_cpu_usage() as f64));
        let mhz: Vec<f64> = self.sys.cpus().iter().map(|c| c.frequency() as f64).filter(|f| *f > 0.0).collect();
        if !mhz.is_empty() {
            out.push(("c.cpu.mhz_mean".to_string(), mhz.iter().sum::<f64>() / mhz.len() as f64));
            if mhz.len() > 1 {
                out.push(("c.cpu.mhz_min".to_string(), mhz.iter().cloned().fold(f64::MAX, f64::min)));
                out.push(("c.cpu.mhz_max".to_string(), mhz.iter().cloned().fold(0.0, f64::max)));
            }
        }
        let total = self.sys.total_memory() as f64;
        if total > 0.0 {
            out.push(("m.mem.used_percent".to_string(), 100.0 * (1.0 - self.sys.available_memory() as f64 / total)));
        }
        let st = self.sys.total_swap() as f64;
        if st > 0.0 {
            out.push(("m.swap.used_percent".to_string(), 100.0 * self.sys.used_swap() as f64 / st));
        }
        let mut temps: Vec<(String, f64)> = Vec::new();
        for c in self.comps.list() {
            if let Some(t) = c.temperature() {
                let t = t as f64;
                if t > -40.0 && t < 150.0 {
                    temps.push((c.label().to_string(), t));
                }
            }
        }
        if let Some(p) = pick_cpu(&temps) {
            out.push((package_key().to_string(), p));
        }
        let src = match crate::os_name() {
            "windows" => "win",
            "macos" => "mac",
            _ => "sys",
        };
        for (label, t) in &temps {
            out.push((format!("t.{}.{}", src, clean(label)), *t));
        }
        out.extend(crate::nvidia::read());
        out.extend(crate::windisk::read(false).0);
        out.extend(crate::driver::read());
        #[cfg(target_os = "macos")]
        {
            out.extend(crate::macsensors::temperature_keys(&crate::macsensors::temperatures()));
            if let Some(p) = self.power.as_mut() {
                out.extend(p.read());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_the_hottest_processor_label() {
        let v = vec![("CPU Package".to_string(), 61.0), ("Core 0".to_string(), 64.5), ("nvme Composite".to_string(), 80.0), ("GPU hotspot".to_string(), 90.0)];
        assert_eq!(pick_cpu(&v), Some(64.5));
        assert_eq!(pick_cpu(&[("nvme".to_string(), 50.0)]), None);
    }

    #[test]
    fn keys_have_no_dots_or_controls() {
        assert_eq!(clean("a.b\"c\\d\n"), "a_b_c_d_");
        assert!(clean(&"x".repeat(200)).len() <= 60);
    }
}
