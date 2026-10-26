//! RAM test in user space: writes patterns into a part of the free memory and reads them back, counting every mismatch. It cannot test memory the operating system
//! keeps for itself (a boot-time tester such as MemTest86 is more thorough), but it finds bad cells, bad speed settings and unstable overclocks in the part it covers.
use serde_json::{json, Value};
use std::ptr::{read_volatile, write_volatile};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub struct Params {
    pub pct: u64,
    pub budget_secs: u64,
    pub avail_bytes: u64,
    pub threads: usize,
}

const GIB: u64 = 1 << 30;
const MIN_BYTES: u64 = 16 << 20;

fn xorshift(s: &mut u64) -> u64 {
    let mut x = *s;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *s = x;
    x
}

/// Writes `f(i)` into every slot, then reads every slot back; returns the number of slots that differ.
fn fill_verify(buf: &mut [u64], f: &dyn Fn(usize) -> u64) -> u64 {
    let p = buf.as_mut_ptr();
    for i in 0..buf.len() {
        unsafe { write_volatile(p.add(i), f(i)) };
    }
    std::sync::atomic::fence(Ordering::SeqCst);
    let mut errors = 0u64;
    for i in 0..buf.len() {
        let v = unsafe { read_volatile(p.add(i)) };
        if v != f(i) {
            errors += 1;
        }
    }
    errors
}

/// Same with a random sequence: the sequence is generated again for the check, so nothing is kept besides the memory under test.
fn random_verify(buf: &mut [u64], seed: u64) -> u64 {
    let p = buf.as_mut_ptr();
    let mut s = seed | 1;
    for i in 0..buf.len() {
        unsafe { write_volatile(p.add(i), xorshift(&mut s)) };
    }
    std::sync::atomic::fence(Ordering::SeqCst);
    let mut s = seed | 1;
    let mut errors = 0u64;
    for i in 0..buf.len() {
        if unsafe { read_volatile(p.add(i)) } != xorshift(&mut s) {
            errors += 1;
        }
    }
    errors
}

/// How many bytes to test: the share asked for, never more than 16 GiB, always leaving about 1 GiB (or half of a small memory) to the system.
pub fn plan_bytes(avail: u64, pct: u64) -> u64 {
    let want = ((avail as u128 * pct as u128 / 100) as u64).min(16 * GIB);
    let cap = if avail > 2 * GIB { avail - GIB } else { avail / 2 };
    want.min(cap).max(MIN_BYTES.min(avail / 2))
}

fn alloc(words: usize) -> Option<Vec<u64>> {
    let mut v: Vec<u64> = Vec::new();
    v.try_reserve_exact(words).ok()?;
    v.resize(words, 0);
    Some(v)
}

pub fn run(p: &Params, cancel: &AtomicBool) -> Value {
    let started = Instant::now();
    let threads = p.threads.max(1);
    let mut bytes = plan_bytes(p.avail_bytes, p.pct);
    let mut bufs: Vec<Vec<u64>> = Vec::new();
    // the memory may be taken by someone else between the measurement and now: halve until it fits
    loop {
        bufs.clear();
        let words = (bytes / threads as u64 / 8) as usize;
        let ok = (0..threads).all(|_| match alloc(words) {
            Some(v) => {
                bufs.push(v);
                true
            }
            None => false,
        });
        if ok && words > 0 {
            break;
        }
        bufs.clear();
        bytes /= 2;
        if bytes < MIN_BYTES {
            return json!({"mb": 0, "passes": 0, "errors": 0, "secs": started.elapsed().as_secs_f64().round(), "threads": threads, "error": "not enough free memory"});
        }
    }
    let tested_mb: u64 = bufs.iter().map(|b| b.len() as u64 * 8).sum::<u64>() / (1 << 20);
    let budget = p.budget_secs;
    let results: Vec<(u64, u32)> = std::thread::scope(|s| {
        let handles: Vec<_> = bufs
            .iter_mut()
            .enumerate()
            .map(|(t, buf)| {
                s.spawn(move || {
                    let mut errors = 0u64;
                    let mut passes = 0u32;
                    let mut pass = 0u64;
                    'outer: loop {
                        // one pass = every pattern once; a pass that the time budget or a cancel interrupts is not counted
                        let patterns: [Box<dyn Fn(&mut [u64]) -> u64>; 7] = [
                            Box::new(|b| fill_verify(b, &|_| 0x5555_5555_5555_5555)),
                            Box::new(|b| fill_verify(b, &|_| 0xAAAA_AAAA_AAAA_AAAA)),
                            Box::new(|b| fill_verify(b, &|_| 0)),
                            Box::new(|b| fill_verify(b, &|_| u64::MAX)),
                            Box::new(|b| fill_verify(b, &|i| (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))),
                            Box::new(move |b| fill_verify(b, &|i| (1u64 << ((pass + i as u64) % 64)) | (i as u64 & 1) << 63)),
                            Box::new(move |b| random_verify(b, 0xDEADBEEF ^ (pass * 7919 + t as u64 * 104729))),
                        ];
                        for pat in patterns.iter() {
                            if cancel.load(Ordering::Relaxed) || (passes >= 1 && started.elapsed().as_secs() >= budget) {
                                break 'outer;
                            }
                            errors += pat(buf);
                        }
                        passes += 1;
                        pass += 1;
                        if started.elapsed().as_secs() >= budget {
                            break;
                        }
                    }
                    (errors, passes)
                })
            })
            .collect();
        handles.into_iter().map(|h| h.join().unwrap_or((0, 0))).collect()
    });
    let errors: u64 = results.iter().map(|r| r.0).sum();
    let passes = results.iter().map(|r| r.1).min().unwrap_or(0);
    json!({"mb": tested_mb, "passes": passes, "errors": errors, "secs": started.elapsed().as_secs_f64().round(), "threads": threads, "cancelled": cancel.load(Ordering::Relaxed)})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_leaves_memory_to_the_system() {
        assert_eq!(plan_bytes(8 * GIB, 25), 2 * GIB);
        assert_eq!(plan_bytes(64 * GIB, 60), 16 * GIB);               // never more than 16 GiB
        assert!(plan_bytes(3 * GIB, 60) <= 2 * GIB);                   // at least 1 GiB stays free
        assert!(plan_bytes(GIB, 60) <= GIB / 2);                       // half of a very small memory
    }

    #[test]
    fn a_good_buffer_has_no_errors_and_a_corrupted_one_is_caught() {
        let mut b = vec![0u64; 1 << 16];
        assert_eq!(fill_verify(&mut b, &|i| i as u64 * 3), 0);
        assert_eq!(random_verify(&mut b, 42), 0);
        // a pattern function that changes between write and read simulates a flipped bit
        let flip = std::sync::atomic::AtomicU64::new(0);
        let n = fill_verify(&mut b, &|i| (i as u64) ^ if flip.fetch_add(1, Ordering::Relaxed) >= (1 << 16) && i == 10 { 1 } else { 0 });
        assert_eq!(n, 1);
    }

    #[test]
    fn a_short_run_finishes_and_counts_passes() {
        let r = run(&Params { pct: 5, budget_secs: 1, avail_bytes: 512 << 20, threads: 2 }, &AtomicBool::new(false));
        assert_eq!(r["errors"], 0);
        assert!(r["passes"].as_u64().unwrap() >= 1);
        assert!(r["mb"].as_u64().unwrap() >= 16);
    }

    #[test]
    fn cancel_stops_early() {
        let t = Instant::now();
        let r = run(&Params { pct: 5, budget_secs: 600, avail_bytes: 512 << 20, threads: 1 }, &AtomicBool::new(true));
        assert!(t.elapsed().as_secs() < 10);
        assert_eq!(r["cancelled"], true);
    }
}
