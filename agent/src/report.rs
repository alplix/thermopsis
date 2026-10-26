//! One reporting window: the lowest, mean and highest value of every sensor, and the JSON report (v1) the server reads.
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

const MAX_KEYS: usize = 512;

struct Agg {
    min: f64,
    max: f64,
    sum: f64,
    n: u32,
}

pub struct Window {
    pub start: u64,
    pub samples: u32,
    agg: BTreeMap<String, Agg>,
}

fn round(x: f64) -> f64 {
    if x.abs() >= 1000.0 {
        (x * 10.0).round() / 10.0
    } else {
        (x * 1000.0).round() / 1000.0
    }
}

impl Window {
    pub fn new(now: u64) -> Window {
        Window { start: now, samples: 0, agg: BTreeMap::new() }
    }

    pub fn add(&mut self, readings: &[(String, f64)]) {
        self.samples += 1;
        for (k, v) in readings {
            if !v.is_finite() {
                continue;
            }
            if !self.agg.contains_key(k) && self.agg.len() >= MAX_KEYS {
                continue;
            }
            let a = self.agg.entry(k.clone()).or_insert(Agg { min: *v, max: *v, sum: 0.0, n: 0 });
            a.min = a.min.min(*v);
            a.max = a.max.max(*v);
            a.sum += *v;
            a.n += 1;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.samples == 0 || self.agg.is_empty()
    }

    pub fn has_temperature(&self) -> bool {
        self.agg.keys().any(|k| k.starts_with("t."))
    }

    /// The report document: {"v":1,"t":..,"w":..,"s":..,"inv":"<hash>"[,"inventory":{..}],"m":{"key":[min,mean,max],..}[,"disks":[..]][,"events":[..]]}
    pub fn report(&self, now: u64, inv_hash: &str, inventory: Option<&Value>, disks: Vec<Value>, events: Vec<Value>) -> Value {
        let mut m = Map::new();
        for (k, a) in &self.agg {
            m.insert(k.clone(), json!([round(a.min), round(a.sum / a.n.max(1) as f64), round(a.max)]));
        }
        let mut r = json!({"v": 1, "t": now, "w": now.saturating_sub(self.start).max(30), "s": self.samples, "inv": inv_hash, "m": Value::Object(m)});
        if let Some(i) = inventory {
            r["inventory"] = i.clone();
        }
        if !disks.is_empty() {
            r["disks"] = Value::Array(disks);
        }
        if !events.is_empty() {
            r["events"] = Value::Array(events);
        }
        r
    }
}

/// FNV-1a of the inventory text, 8 hex characters: the same value for the same hardware, so the inventory is sent again only when it changes.
pub fn inventory_hash(inv: &Value) -> String {
    let text = inv.to_string();
    let mut h: u32 = 2166136261;
    for b in text.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(16777619);
    }
    format!("{:08x}", h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_keeps_min_mean_max() {
        let mut w = Window::new(1000);
        w.add(&[("t.win.cpu.package".to_string(), 40.0), ("u.cpu.percent".to_string(), 10.0)]);
        w.add(&[("t.win.cpu.package".to_string(), 60.0)]);
        w.add(&[("t.win.cpu.package".to_string(), 50.0), ("u.cpu.percent".to_string(), 30.0)]);
        let r = w.report(1600, "abcd1234", None, vec![], vec![]);
        assert_eq!(r["m"]["t.win.cpu.package"], json!([40.0, 50.0, 60.0]));
        assert_eq!(r["m"]["u.cpu.percent"], json!([10.0, 20.0, 30.0]));
        assert_eq!(r["s"], 3);
        assert_eq!(r["w"], 600);
        assert!(r.get("inventory").is_none() && r.get("events").is_none());
        assert!(w.has_temperature());
    }

    #[test]
    fn non_finite_values_are_dropped_and_keys_are_capped() {
        let mut w = Window::new(0);
        w.add(&[("t.x.nan".to_string(), f64::NAN), ("t.x.inf".to_string(), f64::INFINITY)]);
        assert!(w.is_empty() || !w.has_temperature());
        let many: Vec<(String, f64)> = (0..700).map(|i| (format!("x.k.{}", i), i as f64)).collect();
        w.add(&many);
        assert_eq!(w.report(100, "h", None, vec![], vec![])["m"].as_object().unwrap().len(), MAX_KEYS);
    }

    #[test]
    fn inventory_hash_is_stable_and_sensitive() {
        let a = json!({"cpu": {"model": "A"}});
        let b = json!({"cpu": {"model": "B"}});
        assert_eq!(inventory_hash(&a), inventory_hash(&a));
        assert_ne!(inventory_hash(&a), inventory_hash(&b));
        assert_eq!(inventory_hash(&a).len(), 8);
    }
}
