//! Memory speed in the manner of the STREAM benchmark: read, write, copy and triad over arrays far larger than any cache, with all threads at once, best of several
//! runs, in GB/s. A computer whose memory runs as single channel, or whose memory profile (XMP/DOCP/EXPO) is switched off, shows up as a low number.
use serde_json::{json, Value};
use std::hint::black_box;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const ARRAY_BYTES: usize = 192 << 20; // per array; three arrays are used for copy and triad
const RUNS: usize = 4;

fn best_of(cancel: &AtomicBool, mut f: impl FnMut()) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..RUNS {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let t = Instant::now();
        f();
        best = best.min(t.elapsed().as_secs_f64());
    }
    best
}

pub fn run(threads: usize, cancel: &AtomicBool) -> Value {
    let threads = threads.max(1);
    let per = (ARRAY_BYTES / 8 / threads).max(1 << 16);
    let n = per * threads;
    let mut a = vec![1.0f64; n];
    let mut b = vec![2.0f64; n];
    let mut c = vec![0.5f64; n];
    let bytes = |factor: f64| factor * 8.0 * n as f64 / 1e9;
    let s = 1.000_000_1f64;

    let read = best_of(cancel, || {
        let sum: f64 = std::thread::scope(|sc| {
            let hs: Vec<_> = a.chunks(per).map(|ch| sc.spawn(move || ch.iter().sum::<f64>())).collect();
            hs.into_iter().map(|h| h.join().unwrap_or(0.0)).sum()
        });
        black_box(sum);
    });
    let write = best_of(cancel, || {
        std::thread::scope(|sc| {
            for ch in a.chunks_mut(per) {
                sc.spawn(move || {
                    for x in ch.iter_mut() {
                        *x = s;
                    }
                    black_box(&ch[0]);
                });
            }
        });
    });
    let copy = best_of(cancel, || {
        std::thread::scope(|sc| {
            for (dst, src) in c.chunks_mut(per).zip(a.chunks(per)) {
                sc.spawn(move || dst.copy_from_slice(src));
            }
        });
        black_box(&c[0]);
    });
    let triad = best_of(cancel, || {
        std::thread::scope(|sc| {
            for ((da, sb), sc_) in a.chunks_mut(per).zip(b.chunks(per)).zip(c.chunks(per)) {
                sc.spawn(move || {
                    for i in 0..da.len() {
                        da[i] = sb[i] + s * sc_[i];
                    }
                });
            }
        });
        black_box(&a[0]);
    });
    black_box(&mut b);
    let gbs = |factor: f64, secs: f64| if secs.is_finite() && secs < f64::MAX && secs > 0.0 { (bytes(factor) / secs * 10.0).round() / 10.0 } else { 0.0 };
    json!({"read_gbs": gbs(1.0, read), "write_gbs": gbs(1.0, write), "copy_gbs": gbs(2.0, copy), "triad_gbs": gbs(3.0, triad), "threads": threads, "array_mb": ARRAY_BYTES >> 20, "cancelled": cancel.load(Ordering::Relaxed)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measures_plausible_speeds() {
        let r = run(2, &AtomicBool::new(false));
        for k in ["read_gbs", "write_gbs", "copy_gbs", "triad_gbs"] {
            let v = r[k].as_f64().unwrap();
            assert!(v > 0.2 && v < 2000.0, "{} = {}", k, v);
        }
    }
}
