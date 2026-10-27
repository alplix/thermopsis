//! macOS (Apple silicon) without administrator rights: temperatures from the HID thermal sensors (IOHIDEventSystemClient) and power from IOReport's "Energy Model".
//! Both are system frameworks Apple does not document; the same calls are used by other open monitoring tools. Nothing here needs root and nothing is installed.
//! Keys: t.mac.soc.cpu / t.mac.soc.gpu (hottest sensor of each), t.mac.iohid.<sensor>, p.mac.cpu / gpu / ane / dram (watts, from the energy counters between two readings).
#![cfg(target_os = "macos")]
use std::ffi::{c_char, c_void, CString};
use std::time::Instant;

type CFRef = *const c_void;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
    fn CFRelease(r: CFRef);
    fn CFStringCreateWithCString(alloc: CFRef, s: *const c_char, enc: u32) -> CFRef;
    fn CFStringGetCString(s: CFRef, buf: *mut c_char, size: isize, enc: u32) -> u8;
    fn CFNumberCreate(alloc: CFRef, kind: i32, v: *const c_void) -> CFRef;
    fn CFDictionaryCreate(alloc: CFRef, keys: *const CFRef, vals: *const CFRef, n: isize, kc: *const c_void, vc: *const c_void) -> CFRef;
    fn CFDictionaryGetValue(d: CFRef, k: CFRef) -> CFRef;
    fn CFArrayGetCount(a: CFRef) -> isize;
    fn CFArrayGetValueAtIndex(a: CFRef, i: isize) -> CFRef;
}

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDEventSystemClientCreate(alloc: CFRef) -> CFRef;
    fn IOHIDEventSystemClientSetMatching(c: CFRef, m: CFRef) -> i32;
    fn IOHIDEventSystemClientCopyServices(c: CFRef) -> CFRef;
    fn IOHIDServiceClientCopyProperty(s: CFRef, key: CFRef) -> CFRef;
    fn IOHIDServiceClientCopyEvent(s: CFRef, t: i64, opt: i32, timeout: i64) -> CFRef;
    fn IOHIDEventGetFloatValue(e: CFRef, field: i32) -> f64;
}

#[link(name = "IOReport")]
extern "C" {
    fn IOReportCopyChannelsInGroup(group: CFRef, sub: CFRef, a: u64, b: u64, c: u64) -> CFRef;
    fn IOReportCreateSubscription(alloc: CFRef, chans: CFRef, subbed: *mut CFRef, a: u64, b: CFRef) -> CFRef;
    fn IOReportCreateSamples(sub: CFRef, subbed: CFRef, c: CFRef) -> CFRef;
    fn IOReportCreateSamplesDelta(a: CFRef, b: CFRef, c: CFRef) -> CFRef;
    fn IOReportChannelGetChannelName(ch: CFRef) -> CFRef;
    fn IOReportChannelGetUnitLabel(ch: CFRef) -> CFRef;
    fn IOReportSimpleGetIntegerValue(ch: CFRef, idx: i32) -> i64;
}

const UTF8: u32 = 0x0800_0100;

fn cfstr(s: &str) -> CFRef {
    let c = CString::new(s).unwrap_or_default();
    unsafe { CFStringCreateWithCString(std::ptr::null(), c.as_ptr(), UTF8) }
}

fn to_string(s: CFRef) -> String {
    if s.is_null() {
        return String::new();
    }
    let mut buf = [0 as c_char; 256];
    let ok = unsafe { CFStringGetCString(s, buf.as_mut_ptr(), 256, UTF8) };
    if ok == 0 {
        return String::new();
    }
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).to_string()
}

fn clean(s: &str) -> String {
    let t: String = s.to_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    t.trim_matches('_').to_string()
}

/// (sensor name, degrees C) of every HID temperature sensor
pub fn temperatures() -> Vec<(String, f64)> {
    let mut out = Vec::new();
    unsafe {
        let page: i32 = 0xff00;
        let usage: i32 = 5;
        let (k1, k2) = (cfstr("PrimaryUsagePage"), cfstr("PrimaryUsage"));
        let (v1, v2) = (CFNumberCreate(std::ptr::null(), 9, &page as *const i32 as *const c_void), CFNumberCreate(std::ptr::null(), 9, &usage as *const i32 as *const c_void));
        let keys = [k1, k2];
        let vals = [v1, v2];
        let matching = CFDictionaryCreate(std::ptr::null(), keys.as_ptr(), vals.as_ptr(), 2, &kCFTypeDictionaryKeyCallBacks as *const c_void, &kCFTypeDictionaryValueCallBacks as *const c_void);
        let client = IOHIDEventSystemClientCreate(std::ptr::null());
        if !client.is_null() && !matching.is_null() {
            IOHIDEventSystemClientSetMatching(client, matching);
            let services = IOHIDEventSystemClientCopyServices(client);
            if !services.is_null() {
                let product = cfstr("Product");
                for i in 0..CFArrayGetCount(services) {
                    let svc = CFArrayGetValueAtIndex(services, i);
                    let name_ref = IOHIDServiceClientCopyProperty(svc, product);
                    let name = to_string(name_ref);
                    if !name_ref.is_null() {
                        CFRelease(name_ref);
                    }
                    let ev = IOHIDServiceClientCopyEvent(svc, 15, 0, 0);
                    if ev.is_null() {
                        continue;
                    }
                    let t = IOHIDEventGetFloatValue(ev, 15 << 16);
                    CFRelease(ev);
                    if t.is_finite() && t > -20.0 && t < 150.0 && !name.is_empty() {
                        out.push((name, t));
                    }
                }
                CFRelease(product);
                CFRelease(services);
            }
        }
        if !client.is_null() {
            CFRelease(client);
        }
        for r in [matching, k1, k2, v1, v2] {
            if !r.is_null() {
                CFRelease(r);
            }
        }
    }
    out
}

/// readings in the agent's key scheme
pub fn temperature_keys(sensors: &[(String, f64)]) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let (mut cpu, mut gpu): (Option<f64>, Option<f64>) = (None, None);
    let max = |a: Option<f64>, v: f64| Some(a.map_or(v, |m| m.max(v)));
    for (name, t) in sensors {
        let l = name.to_lowercase();
        out.push((format!("t.mac.iohid.{}", clean(name)), *t));
        if l.contains("gpu") {
            gpu = max(gpu, *t);
        } else if l.contains("acc") || l.contains("tdie") || l.contains("cpu") {
            cpu = max(cpu, *t);
        }
    }
    if let Some(v) = cpu {
        out.push(("t.mac.soc.cpu".to_string(), v));
    }
    if let Some(v) = gpu {
        out.push(("t.mac.soc.gpu".to_string(), v));
    }
    out
}

/// Energy counters: each reading is the power since the previous reading
pub struct Power {
    sub: CFRef,
    subbed: CFRef,
    prev: Option<(CFRef, Instant)>,
}

unsafe impl Send for Power {}

impl Power {
    pub fn new() -> Option<Power> {
        unsafe {
            let group = cfstr("Energy Model");
            let chans = IOReportCopyChannelsInGroup(group, std::ptr::null(), 0, 0, 0);
            CFRelease(group);
            if chans.is_null() {
                return None;
            }
            let mut subbed: CFRef = std::ptr::null();
            let sub = IOReportCreateSubscription(std::ptr::null(), chans, &mut subbed, 0, std::ptr::null());
            if sub.is_null() || subbed.is_null() {
                return None;
            }
            Some(Power { sub, subbed, prev: None })
        }
    }

    pub fn read(&mut self) -> Vec<(String, f64)> {
        let mut out = Vec::new();
        unsafe {
            let now = IOReportCreateSamples(self.sub, self.subbed, std::ptr::null());
            if now.is_null() {
                return out;
            }
            let t = Instant::now();
            if let Some((prev, t0)) = self.prev.take() {
                let secs = t.duration_since(t0).as_secs_f64();
                let delta = IOReportCreateSamplesDelta(prev, now, std::ptr::null());
                CFRelease(prev);
                if !delta.is_null() && secs > 0.2 {
                    let chans_key = cfstr("IOReportChannels");
                    let arr = CFDictionaryGetValue(delta, chans_key);
                    CFRelease(chans_key);
                    if !arr.is_null() {
                        for i in 0..CFArrayGetCount(arr) {
                            let ch = CFArrayGetValueAtIndex(arr, i);
                            let name = to_string(IOReportChannelGetChannelName(ch));
                            let unit = to_string(IOReportChannelGetUnitLabel(ch));
                            let v = IOReportSimpleGetIntegerValue(ch, 0) as f64;
                            let joules = match unit.trim() {
                                "mJ" => v / 1e3,
                                "uJ" => v / 1e6,
                                "nJ" => v / 1e9,
                                _ => continue,
                            };
                            let w = joules / secs;
                            let l = name.to_lowercase();
                            // "CPU Energy", "GPU Energy", "ANE", "DRAM": the totals; the per-core channels ("ECPU0", "PCPU1"...) are summed into the CPU total by the system
                            let key = if l == "cpu energy" { "p.mac.cpu" } else if l == "gpu energy" { "p.mac.gpu" } else if l.starts_with("ane") { "p.mac.ane" } else if l.starts_with("dram") { "p.mac.dram" } else { continue };
                            if w.is_finite() && w >= 0.0 && w < 400.0 {
                                out.push((key.to_string(), w));
                            }
                        }
                    }
                    CFRelease(delta);
                }
            }
            self.prev = Some((now, t));
        }
        out
    }
}
