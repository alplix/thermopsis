//! Thermal test: loads every processor core for a few minutes and records temperature (and power where the platform offers it) every few seconds. Stops by
//! itself when the processor gets too hot or when asked. From the curve it takes the heating time constant and the temperature rise: how well the cooling copes.
use crate::sensors::{package_key, Sensors};
use serde_json::{json, Value};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct Params {
    pub secs: u64,
    pub limit_c: f64,
}

/// time until the curve has covered 63.2 % of its rise (the time constant of an exponential approach), and the rise itself
pub fn time_constant(curve: &[(f64, f64)]) -> Option<(f64, f64)> {
    let (t0, y0) = *curve.first()?;
    let peak = curve.iter().map(|c| c.1).fold(f64::MIN, f64::max);
    let rise = peak - y0;
    if rise < 3.0 {
        return None;
    }
    let target = y0 + 0.632 * rise;
    let hit = curve.iter().find(|c| c.1 >= target)?;
    Some((hit.0 - t0, rise))
}

fn worker(stop: &AtomicBool) {
    let mut x = 1.0f64;
    let mut h = 0u64;
    while !stop.load(Ordering::Relaxed) {
        for _ in 0..20_000 {
            x = x * 1.000_000_1 + 0.1;
            h = h.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            h ^= h >> 29;
        }
        black_box((x, h));
    }
}

pub fn run(p: &Params, cancel: &AtomicBool, sensors: &mut dyn Sensors) -> Value {
    let started = Instant::now();
    let stop = AtomicBool::new(false);
    let threads = super::threads();
    let pkg = package_key();
    let mut curve: Vec<(f64, f64)> = Vec::new();
    let mut peak_c = f64::MIN;
    let mut peak_w = 0.0f64;
    let mut stopped = false;
    // temperature before the load: a short idle reading as the starting point of the curve
    for (k, v) in sensors.read() {
        if k == pkg {
            curve.push((0.0, v));
            peak_c = peak_c.max(v);
        }
    }
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| worker(&stop));
        }
        let mut next = 0.0f64;
        while started.elapsed().as_secs() < p.secs && !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(250));
            let el = started.elapsed().as_secs_f64();
            if el < next {
                continue;
            }
            next = el + 4.0;
            let mut hot = false;
            for (k, v) in sensors.read() {
                if k == pkg {
                    curve.push((el, v));
                    peak_c = peak_c.max(v);
                    if v >= p.limit_c {
                        hot = true;
                    }
                } else if k.starts_with("p.") && !k.contains(".limit") {
                    peak_w = peak_w.max(v);
                }
            }
            if hot {
                stopped = true;
                break;
            }
        }
        stop.store(true, Ordering::Relaxed);
    });
    let secs = started.elapsed().as_secs_f64();
    let mut r = json!({"secs": secs.round(), "threads": threads, "stopped": stopped, "cancelled": cancel.load(Ordering::Relaxed)});
    if peak_c > f64::MIN {
        r["peak_c"] = json!((peak_c * 10.0).round() / 10.0);
    }
    if peak_w > 0.0 {
        r["peak_w"] = json!((peak_w * 10.0).round() / 10.0);
    }
    if let Some((tau, rise)) = time_constant(&curve) {
        r["tau_s"] = json!(tau.round());
        r["dt_c"] = json!((rise * 10.0).round() / 10.0);
    }
    // the curve itself, at most 40 points, for the website's chart
    let step = (curve.len() / 40).max(1);
    r["curve"] = Value::Array(curve.iter().step_by(step).map(|(t, v)| json!([t.round(), (v * 10.0).round() / 10.0])).collect());
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(f64, f64);
    impl Sensors for Fake {
        fn read(&mut self) -> Vec<(String, f64)> {
            self.0 += self.1;
            vec![(package_key().to_string(), self.0)]
        }
    }

    #[test]
    fn time_constant_of_an_exponential_curve() {
        // T(t) = 80 - 40 * exp(-t / 30): rise 40 -> 63.2 % at t = 30 s
        let curve: Vec<(f64, f64)> = (0..30).map(|i| (i as f64 * 4.0, 80.0 - 40.0 * (-(i as f64 * 4.0) / 30.0).exp())).collect();
        let (tau, rise) = time_constant(&curve).unwrap();
        assert!((tau - 32.0).abs() <= 4.0, "tau {}", tau);
        assert!(rise > 35.0 && rise < 41.0);
        assert!(time_constant(&[(0.0, 40.0), (4.0, 41.0)]).is_none());
    }

    #[test]
    fn stops_when_too_hot() {
        let mut s = Fake(50.0, 30.0);
        let r = run(&Params { secs: 60, limit_c: 95.0 }, &AtomicBool::new(false), &mut s);
        assert_eq!(r["stopped"], true);
        assert!(r["secs"].as_f64().unwrap() < 20.0);
    }

    #[test]
    fn short_run_with_cancel() {
        let mut s = Fake(40.0, 0.0);
        let r = run(&Params { secs: 60, limit_c: 95.0 }, &AtomicBool::new(true), &mut s);
        assert_eq!(r["cancelled"], true);
        assert_eq!(r["stopped"], false);
    }
}
