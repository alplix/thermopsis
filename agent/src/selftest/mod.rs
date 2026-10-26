//! The tests a volunteer can start with one click (or schedule): RAM test, memory speed, thermal test, drive speed. Each returns the result as JSON; the run loop
//! turns it into an event of the next report. They run at low priority work sizes, stop when asked and clean up after themselves.
pub mod diskbench;
pub mod membw;
pub mod memtest;
pub mod stress;

use crate::sensors::Sensors;
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use sysinfo::System;

pub fn threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2).max(1)
}

fn available_memory() -> u64 {
    let mut s = System::new();
    s.refresh_memory();
    s.available_memory()
}

/// Run a test by name; `params` are the parameters the website queued (for example {"pct":25} or {"secs":120}). None = unknown test.
pub fn run(kind: &str, params: &Value, cancel: &AtomicBool, sensors: &mut dyn Sensors) -> Option<Value> {
    let result = match kind {
        "memtest" => {
            let pct = params["pct"].as_u64().unwrap_or(25).clamp(5, 60);
            let budget = params["secs"].as_u64().unwrap_or(180).clamp(20, 900);
            memtest::run(&memtest::Params { pct, budget_secs: budget, avail_bytes: available_memory(), threads: (threads() / 2).clamp(1, 8) }, cancel)
        }
        "membw" => membw::run(threads().clamp(1, 16), cancel),
        "stress" => {
            let secs = params["secs"].as_u64().unwrap_or(120).clamp(20, 600);
            stress::run(&stress::Params { secs, limit_c: 95.0 }, cancel, sensors)
        }
        "diskbench" => diskbench::run(cancel),
        _ => return None,
    };
    let mut ev = json!({"kind": kind, "t": crate::now()});
    if let (Some(o), Some(r)) = (ev.as_object_mut(), result.as_object()) {
        for (k, v) in r {
            o.insert(k.clone(), v.clone());
        }
    }
    Some(ev)
}
