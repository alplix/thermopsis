//! Windows drives: temperature of every internal drive and, for NVMe drives, the full health log (wear, bytes written and read, power-on hours, unsafe shutdowns,
//! media errors, spare area). This uses the storage stack's own queries on the physical drive opened WITHOUT any access right: no driver and no administrator rights.
//! (SATA health attributes need administrator rights and are not read.) Serial numbers are never read.
use serde_json::{json, Value};

/// parsed NVMe SMART / Health log page (log page 2, 512 bytes)
#[derive(Debug, PartialEq)]
pub struct NvmeHealth {
    pub critical: u8,
    pub temp_c: i32,
    pub spare_pct: u8,
    pub used_pct: u8,
    pub read_bytes: u128,
    pub written_bytes: u128,
    pub power_on_hours: u128,
    pub unsafe_shutdowns: u128,
    pub media_errors: u128,
}

fn le128(b: &[u8], o: usize) -> u128 {
    let mut a = [0u8; 16];
    a.copy_from_slice(&b[o..o + 16]);
    u128::from_le_bytes(a)
}

/// one data unit is 1000 blocks of 512 bytes (NVMe specification)
pub fn parse_nvme_health(b: &[u8]) -> Option<NvmeHealth> {
    if b.len() < 176 {
        return None;
    }
    let temp_k = u16::from_le_bytes([b[1], b[2]]) as i32;
    Some(NvmeHealth {
        critical: b[0],
        temp_c: temp_k - 273,
        spare_pct: b[3],
        used_pct: b[5],
        read_bytes: le128(b, 32) * 512_000,
        written_bytes: le128(b, 48) * 512_000,
        power_on_hours: le128(b, 128),
        unsafe_shutdowns: le128(b, 144),
        media_errors: le128(b, 160),
    })
}

/// STORAGE_TEMPERATURE_DATA_DESCRIPTOR: the first plausible sensor value (the entries after the first are often empty), and the drive's own warning and critical limits
pub fn parse_temperature(o: &[u8]) -> Option<(i32, i32, i32)> {
    if o.len() < 28 {
        return None;
    }
    let critical = i16::from_le_bytes([o[8], o[9]]) as i32;
    let warning = i16::from_le_bytes([o[10], o[11]]) as i32;
    let count = u16::from_le_bytes([o[12], o[13]]) as usize;
    for i in 0..count.min(4) {
        let b = 24 + i * 12;
        if b + 4 > o.len() {
            break;
        }
        let t = i16::from_le_bytes([o[b + 2], o[b + 3]]) as i32;
        if (1..=125).contains(&t) {
            return Some((t, warning, critical));
        }
    }
    None
}

/// (bus type, product text) from a STORAGE_DEVICE_DESCRIPTOR; bus 17 = NVMe, 11 = SATA, 7 = USB
pub fn parse_descriptor(o: &[u8]) -> Option<(u32, String)> {
    if o.len() < 32 {
        return None;
    }
    let u = |p: usize| u32::from_le_bytes([o[p], o[p + 1], o[p + 2], o[p + 3]]) as usize;
    let bus = u(28) as u32;
    let text = |off: usize| -> String {
        if off == 0 || off >= o.len() {
            return String::new();
        }
        let end = o[off..].iter().position(|&c| c == 0).map(|e| off + e).unwrap_or(o.len());
        String::from_utf8_lossy(&o[off..end]).trim().to_string()
    };
    let model = format!("{} {}", text(u(12)), text(u(16))).trim().to_string();
    Some((bus, model))
}

/// stable identifier of a drive for the project: a hash of model, size and slot (not of anything unique to the device)
pub fn drive_key(model: &str, gb: u64, slot: u32) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{}|{}|{}", model, gb, slot).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

pub fn health_word(h: &NvmeHealth) -> &'static str {
    if h.critical != 0 || h.media_errors > 0 || h.used_pct >= 100 || h.spare_pct < 10 {
        "fail"
    } else if h.used_pct >= 80 || h.unsafe_shutdowns > 1000 {
        "warn"
    } else {
        "ok"
    }
}

#[cfg(windows)]
mod sys {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateFileW(name: *const u16, access: u32, share: u32, sa: *mut c_void, disp: u32, flags: u32, template: *mut c_void) -> *mut c_void;
        pub fn DeviceIoControl(h: *mut c_void, code: u32, inb: *const c_void, inl: u32, outb: *mut c_void, outl: u32, ret: *mut u32, ov: *mut c_void) -> i32;
        pub fn CloseHandle(h: *mut c_void) -> i32;
        pub fn GetLastError() -> u32;
    }
}

#[cfg(windows)]
fn ioctl(h: *mut std::ffi::c_void, code: u32, input: &[u8], out_len: usize) -> Option<Vec<u8>> {
    let mut out = vec![0u8; out_len];
    let mut ret: u32 = 0;
    let ok = unsafe { sys::DeviceIoControl(h, code, input.as_ptr() as *const _, input.len() as u32, out.as_mut_ptr() as *mut _, out_len as u32, &mut ret, std::ptr::null_mut()) };
    if ok == 0 {
        if std::env::var("THERMOPSIS_DEBUG").is_ok() {
            eprintln!("  ioctl {:#x} failed (error {}, input {} bytes, output {} bytes)", code, unsafe { sys::GetLastError() }, input.len(), out_len);
        }
        return None;
    }
    out.truncate(ret as usize);
    Some(out)
}

#[cfg(windows)]
fn query(h: *mut std::ffi::c_void, property: u32, extra: &[u8], out_len: usize) -> Option<Vec<u8>> {
    let mut q = Vec::new();
    q.extend_from_slice(&property.to_le_bytes());
    q.extend_from_slice(&0u32.to_le_bytes()); // PropertyStandardQuery
    q.extend_from_slice(extra);
    if extra.is_empty() {
        q.extend_from_slice(&[0u8; 4]); // sizeof(STORAGE_PROPERTY_QUERY) is 12: AdditionalParameters[1] and padding
    }
    ioctl(h, 0x002D_1400, &q, out_len) // IOCTL_STORAGE_QUERY_PROPERTY
}

/// (temperature readings, drive entries for the report) of every internal drive
#[cfg(windows)]
pub fn read(full: bool) -> (Vec<(String, f64)>, Vec<Value>) {
    let sizes = if full { crate::inventory::disk_sizes() } else { std::collections::HashMap::new() };
    let (mut readings, mut drives) = (Vec::new(), Vec::new());
    for n in 0u32..16 {
        let name: Vec<u16> = format!(r"\\.\PhysicalDrive{}", n).encode_utf16().chain(std::iter::once(0)).collect();
        let h = unsafe { sys::CreateFileW(name.as_ptr(), 0, 3, std::ptr::null_mut(), 3, 0, std::ptr::null_mut()) };
        if h.is_null() || h as isize == -1 {
            if std::env::var("THERMOPSIS_DEBUG").is_ok() {
                eprintln!("PhysicalDrive{}: cannot open (error {})", n, unsafe { sys::GetLastError() });
            }
            continue;
        }
        let (bus, model) = query(h, 0, &[], 1024).and_then(|d| parse_descriptor(&d)).unwrap_or((0, String::new()));
        let gb = sizes.get(&n).copied().unwrap_or(0);       // the size query needs a read right on the raw drive: it comes from Get-PhysicalDisk (hourly)
        let temp = query(h, 52, &[], 512).and_then(|o| parse_temperature(&o));
        if std::env::var("THERMOPSIS_DEBUG").is_ok() {
            eprintln!("PhysicalDrive{}: bus {} model \"{}\" {} GB temperature {:?}", n, bus, model, gb, temp);
        }
        let kind = match bus {
            17 => "nvme",
            7 => "",
            _ => "",
        };
        if bus == 7 {
            unsafe { sys::CloseHandle(h) };
            continue; // USB drives are not part of the computer
        }
        let tag = if bus == 17 { "nvme" } else { "sata" };
        if let Some((t, _warn, _crit)) = temp {
            readings.push((format!("t.win.{}.{}", tag, n), t as f64));
        }
        let mut d = json!({"id": drive_key(&model, gb, n), "model": model, "kind": kind, "gb": gb});
        if let Some((t, _, _)) = temp {
            d["temp"] = json!(t);
        }
        if bus == 17 && full {
            // NVMe log page 2 (SMART / health): STORAGE_PROTOCOL_SPECIFIC_DATA after the 8-byte query header
            let mut extra = Vec::new();
            for v in [3u32, 2, 2, 0, 40, 512, 0, 0, 0, 0] {
                extra.extend_from_slice(&v.to_le_bytes()); // ProtocolType NVMe, DataType LogPage, page 2, sub 0, data offset 40, length 512, reserved
            }
            if let Some(o) = query(h, 50, &extra, 8 + 40 + 512 + 64) {
                let off = if o.len() > 28 { u32::from_le_bytes([o[24], o[25], o[26], o[27]]) as usize } else { 40 };
                let start = 8 + if (40..200).contains(&off) { off } else { 40 };
                if let Some(hl) = o.get(start..).and_then(parse_nvme_health) {
                    d["poh"] = json!(hl.power_on_hours as u64);
                    d["written"] = json!(hl.written_bytes as u64);
                    d["rd"] = json!(hl.read_bytes as u64);
                    d["used_pct"] = json!(hl.used_pct);
                    d["spare_pct"] = json!(hl.spare_pct);
                    d["media_err"] = json!(hl.media_errors as u64);
                    d["unsafe"] = json!(hl.unsafe_shutdowns as u64);
                    d["crit"] = json!(hl.critical);
                    d["health"] = json!(health_word(&hl));
                    if hl.temp_c > 0 && hl.temp_c < 125 {
                        d["temp"] = json!(hl.temp_c);
                    }
                }
            }
        }
        unsafe { sys::CloseHandle(h) };
        if full && (gb > 0 || !model.is_empty()) {
            drives.push(d);
        }
    }
    (readings, drives)
}

#[cfg(not(windows))]
pub fn read(_full: bool) -> (Vec<(String, f64)>, Vec<Value>) {
    (Vec::new(), Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_nvme_health_log() {
        let mut b = vec![0u8; 512];
        b[1..3].copy_from_slice(&(46u16 + 273).to_le_bytes());
        b[3] = 100;
        b[5] = 18;
        b[32..48].copy_from_slice(&300_000_000u128.to_le_bytes());
        b[48..64].copy_from_slice(&622_000_000u128.to_le_bytes());
        b[128..144].copy_from_slice(&17889u128.to_le_bytes());
        b[144..160].copy_from_slice(&558u128.to_le_bytes());
        let h = parse_nvme_health(&b).unwrap();
        assert_eq!((h.temp_c, h.spare_pct, h.used_pct, h.power_on_hours, h.unsafe_shutdowns, h.media_errors), (46, 100, 18, 17889, 558, 0));
        assert_eq!(h.written_bytes, 622_000_000u128 * 512_000);
        assert_eq!(health_word(&h), "ok");
        assert!(parse_nvme_health(&b[..100]).is_none());
    }

    #[test]
    fn health_words() {
        let mut h = parse_nvme_health(&vec![0u8; 512]).unwrap();
        h.spare_pct = 100;
        assert_eq!(health_word(&h), "ok");
        h.used_pct = 85;
        assert_eq!(health_word(&h), "warn");
        h.media_errors = 3;
        assert_eq!(health_word(&h), "fail");
    }

    #[test]
    fn temperature_takes_the_first_plausible_sensor() {
        let mut o = vec![0u8; 80];
        o[8..10].copy_from_slice(&(85i16).to_le_bytes());
        o[10..12].copy_from_slice(&(84i16).to_le_bytes());
        o[12..14].copy_from_slice(&3u16.to_le_bytes());
        o[24 + 2..24 + 4].copy_from_slice(&0i16.to_le_bytes()); // empty first entry
        o[36 + 2..36 + 4].copy_from_slice(&46i16.to_le_bytes());
        assert_eq!(parse_temperature(&o), Some((46, 84, 85)));
        assert_eq!(parse_temperature(&vec![0u8; 80]), None);
    }

    #[test]
    fn descriptor_gives_bus_and_model() {
        let mut o = vec![0u8; 128];
        o[12..16].copy_from_slice(&40u32.to_le_bytes());
        o[16..20].copy_from_slice(&60u32.to_le_bytes());
        o[28..32].copy_from_slice(&17u32.to_le_bytes());
        o[40..44].copy_from_slice(b"WDC\0");
        o[60..72].copy_from_slice(b"WD10EZEX-60\0");
        assert_eq!(parse_descriptor(&o), Some((17, "WDC WD10EZEX-60".to_string())));
    }

    #[test]
    fn the_drive_key_is_stable_and_has_no_device_unique_part() {
        assert_eq!(drive_key("SK hynix BC711", 512, 1), drive_key("SK hynix BC711", 512, 1));
        assert_ne!(drive_key("SK hynix BC711", 512, 1), drive_key("SK hynix BC711", 512, 2));
        assert_eq!(drive_key("a", 1, 0).len(), 16);
    }
}
