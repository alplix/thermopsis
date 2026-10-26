//! Drive speed: writes a temporary file of 256 MB to the system's temporary folder, flushes it to the drive, reads it back and deletes it. The write speed is real
//! (the flush waits for the drive); the read speed may come partly from the operating system's cache, which the result says (`cached`).
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

const TOTAL: usize = 256 << 20;
const CHUNK: usize = 4 << 20;

pub fn run(cancel: &AtomicBool) -> Value {
    let path = std::env::temp_dir().join(format!("thermopsis-speedtest-{}.tmp", std::process::id()));
    let result = run_in(&path, TOTAL, cancel);
    let _ = std::fs::remove_file(&path);
    result
}

pub fn run_in(path: &std::path::Path, total: usize, cancel: &AtomicBool) -> Value {
    // incompressible content, so a drive with transparent compression does not look faster than it is
    let mut chunk = vec![0u8; CHUNK];
    let mut s = 0x9E37_79B9_7F4A_7C15u64 ^ crate::now();
    for x in chunk.iter_mut() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        *x = (s >> 24) as u8;
    }
    let t = Instant::now();
    let mut f = match std::fs::File::create(path) {
        Ok(f) => f,
        Err(e) => return json!({"error": format!("cannot write the temporary file: {}", e)}),
    };
    let mut written = 0usize;
    while written < total {
        if cancel.load(Ordering::Relaxed) {
            return json!({"cancelled": true});
        }
        if f.write_all(&chunk).is_err() {
            return json!({"error": "write failed (disk full?)"});
        }
        written += CHUNK;
    }
    let _ = f.sync_all();
    let write_s = t.elapsed().as_secs_f64();
    drop(f);
    let t = Instant::now();
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) => return json!({"error": format!("cannot read the temporary file: {}", e)}),
    };
    let mut buf = vec![0u8; CHUNK];
    let mut read = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return json!({"cancelled": true});
        }
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(_) => return json!({"error": "read failed"}),
        }
    }
    let read_s = t.elapsed().as_secs_f64();
    let mbs = |bytes: usize, secs: f64| if secs > 0.0 { ((bytes as f64 / 1e6) / secs).round() } else { 0.0 };
    json!({"mb": total >> 20, "write_mbs": mbs(written, write_s), "read_mbs": mbs(read, read_s), "cached": true})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_reads_and_leaves_nothing_behind() {
        let p = std::env::temp_dir().join("thermopsis-selftest-unit.tmp");
        let r = run_in(&p, 16 << 20, &AtomicBool::new(false));
        assert!(r["write_mbs"].as_f64().unwrap() > 0.0, "{}", r);
        assert!(r["read_mbs"].as_f64().unwrap() > 0.0);
        let _ = std::fs::remove_file(&p);
        // the public entry removes its file by itself
        let before: Vec<_> = std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("thermopsis-speedtest-")).collect();
        let _ = run(&AtomicBool::new(true));
        let after: Vec<_> = std::fs::read_dir(std::env::temp_dir()).unwrap().filter_map(|e| e.ok()).filter(|e| e.file_name().to_string_lossy().starts_with("thermopsis-speedtest-")).collect();
        assert_eq!(before.len(), after.len());
    }
}
