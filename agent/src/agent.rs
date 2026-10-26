//! The run loop: read the sensors every minute, send a report every ten minutes, ask the project for queued tests, run them one at a time and send their
//! results with the next report. Reports that cannot be sent wait in a queue (at most a day) and are sent later.
use crate::config::Config;
use crate::proto::{ApiError, Client};
use crate::report::{inventory_hash, Window};
use crate::selftest;
use crate::sensors::{Generic, Sensors};
use serde_json::{json, Value};
use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Options {
    pub sample_s: u64,
    pub report_s: u64,
    pub poll_s: u64,
}

impl Options {
    /// Defaults: a reading every 60 s, a report every 600 s, a look for queued tests every 60 s. The environment can shorten them (for tests; the server accepts a
    /// report at most every 50 s).
    pub fn from_env() -> Options {
        let get = |k: &str, d: u64, lo: u64| std::env::var(k).ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(d).max(lo);
        Options { sample_s: get("THERMOPSIS_SAMPLE_S", 60, 1), report_s: get("THERMOPSIS_REPORT_S", 600, 55), poll_s: get("THERMOPSIS_POLL_S", 60, 2) }
    }
}

#[derive(Debug, PartialEq)]
pub enum Stop {
    /// the project signed this device out (or the key was withdrawn): sign in again
    SignedOut,
    /// asked to stop
    Quit,
}

const QUEUE_MAX: usize = 144;

pub fn run(cfg: &Config, opt: &Options, quit: &Arc<AtomicBool>, log: &dyn Fn(&str)) -> Stop {
    let client = Client::new(&cfg.server);
    let token = cfg.token.clone();
    if let Err(e) = crate::driver::start() {
        log(&format!("driver mode: {}", e));
    }
    let mut sensors = Generic::new();
    let inventory = crate::inventory::collect();
    let inv_hash = inventory_hash(&inventory);
    let mut inventory_sent = false;
    let events: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
    let busy = Arc::new(AtomicBool::new(false));
    let mut queue: VecDeque<Value> = VecDeque::new();
    // the project hands a command out again at every poll until the result has been reported: remember what has been started, so each runs once
    let mut handled: HashSet<u64> = HashSet::new();
    let mut window = Window::new(crate::now());
    let (mut last_sample, mut last_poll) = (Instant::now() - Duration::from_secs(opt.sample_s), Instant::now() - Duration::from_secs(opt.poll_s));
    let mut last_report = Instant::now();
    // drive health changes slowly: once an hour is enough (and with the first report)
    let mut last_disks: Option<Instant> = None;
    log(&format!("running: a reading every {} s, a report every {} s", opt.sample_s, opt.report_s));
    loop {
        if quit.load(Ordering::Relaxed) {
            return Stop::Quit;
        }
        if last_sample.elapsed().as_secs() >= opt.sample_s {
            last_sample = Instant::now();
            window.add(&sensors.read());
        }
        if last_poll.elapsed().as_secs() >= opt.poll_s && !busy.load(Ordering::Relaxed) {
            last_poll = Instant::now();
            match client.poll(&token) {
                Ok(cmds) => {
                    if let Some(c) = cmds.into_iter().find(|c| !handled.contains(&c.id)) {
                        handled.insert(c.id);
                        if handled.len() > 500 {
                            handled.clear();
                            handled.insert(c.id);
                        }
                        busy.store(true, Ordering::Relaxed);
                        log(&format!("test queued by the project: {} (command {})", c.kind, c.id));
                        let (events, busy, quit_flag) = (events.clone(), busy.clone(), quit.clone());
                        std::thread::spawn(move || {
                            let cancel = AtomicBool::new(false);
                            let mut s = Generic::new();
                            // the test's own cancel flag follows the agent's quit flag through a short watcher loop
                            let result = std::thread::scope(|sc| {
                                let h = sc.spawn(|| selftest::run(&c.kind, &c.params, &cancel, &mut s));
                                while !h.is_finished() {
                                    if quit_flag.load(Ordering::Relaxed) {
                                        cancel.store(true, Ordering::Relaxed);
                                    }
                                    std::thread::sleep(Duration::from_millis(200));
                                }
                                h.join().ok().flatten()
                            });
                            let mut ev = result.unwrap_or_else(|| json!({"kind": "memtest", "t": crate::now(), "error": "unknown test"}));
                            if ev["kind"].as_str() != Some(c.kind.as_str()) {
                                ev["kind"] = json!(c.kind);
                            }
                            ev["cmd"] = json!(c.id);
                            if let Ok(mut l) = events.lock() {
                                l.push(ev);
                            }
                            busy.store(false, Ordering::Relaxed);
                        });
                    }
                }
                Err(ApiError::Http(401, _)) => return Stop::SignedOut,
                Err(e) => log(&format!("could not ask for tests: {}", e)),
            }
        }
        if last_report.elapsed().as_secs() >= opt.report_s {
            last_report = Instant::now();
            let now = crate::now();
            let evs: Vec<Value> = events.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default();
            if !window.is_empty() {
                let inv = if inventory_sent { None } else { Some(&inventory) };
                let disks = if last_disks.map_or(true, |t| t.elapsed().as_secs() >= 3500) {
                    last_disks = Some(Instant::now());
                    crate::windisk::read(true).1
                } else {
                    vec![]
                };
                queue.push_back(window.report(now, &inv_hash, inv, disks, evs));
                inventory_sent = true;
            } else if !evs.is_empty() {
                // results without a window of readings: send them in a report of their own (one reading of what there is)
                let mut w = Window::new(now.saturating_sub(60));
                w.add(&sensors.read());
                queue.push_back(w.report(now, &inv_hash, None, vec![], evs));
            }
            window = Window::new(now);
            while queue.len() > QUEUE_MAX {
                queue.pop_front();
            }
        }
        // send what waits, oldest first
        while let Some(front) = queue.front() {
            match client.report(&token, front) {
                Ok(()) => {
                    log("report sent");
                    queue.pop_front();
                }
                Err(ApiError::Http(401, _)) => return Stop::SignedOut,
                Err(ApiError::Http(429, _)) => break,                     // too fast: try again in a moment
                Err(ApiError::Http(code, err)) if code >= 400 && code < 500 => {
                    log(&format!("a report was refused ({}, {}) and is dropped", code, err));
                    queue.pop_front();
                }
                Err(e) => {
                    log(&format!("report not sent ({}), will try again", e));
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}
