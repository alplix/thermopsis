//! thermopsis-agent: command line of the Thermopsis desktop agent.
//!   thermopsis-agent login <KEY> [--server URL] [--name NAME]   sign in with the weak account key (or the account key) from your BitBOINC account page
//!   thermopsis-agent run                                         read, report and run the tests the website queues, until stopped
//!   thermopsis-agent status | logout | once | inventory | version
//!   thermopsis-agent test <memtest|membw|stress|diskbench> [--secs N] [--pct N]   run one test here and print the result
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use thermopsis_agent::{agent, config, inventory, proto, report, selftest, sensors};
use thermopsis_agent::sensors::Sensors;

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn help() {
    println!("Thermopsis agent {}\n", thermopsis_agent::VERSION);
    println!("  login <KEY> [--server URL] [--name NAME]   sign in (KEY: the weak account key from your BitBOINC account page)");
    println!("  run                                        read sensors, report, run queued tests");
    println!("  status                                     show the account this device reports to");
    println!("  logout                                     sign this device out");
    println!("  once                                       print one report (nothing is sent)");
    println!("  inventory                                  print the hardware description that is sent");
    println!("  disks                                      print drive temperatures and health (Windows)");
    println!("  test <memtest|membw|stress|diskbench> [--secs N] [--pct N]   run one test here");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("help");
    let mut cfg = config::load();
    let code = match cmd {
        "version" | "--version" => {
            println!("thermopsis-agent {}", thermopsis_agent::VERSION);
            0
        }
        "login" => {
            let key = match args.get(1) {
                Some(k) if !k.starts_with("--") => k.clone(),
                _ => {
                    eprintln!("usage: thermopsis-agent login <KEY>   (the weak account key is on your BitBOINC account page)");
                    std::process::exit(2);
                }
            };
            if let Some(s) = arg_value(&args, "--server") {
                cfg.server = s;
            }
            if let Some(n) = arg_value(&args, "--name") {
                cfg.name = n;
            }
            match proto::Client::new(&cfg.server).login(&key, &cfg.device, &cfg.name) {
                Ok((token, user)) => {
                    cfg.token = token;
                    cfg.user = user.clone();
                    if let Err(e) = config::save(&cfg) {
                        eprintln!("signed in, but the settings could not be saved: {}", e);
                        1
                    } else {
                        println!("signed in as {} (device \"{}\"); the key is not stored", user, cfg.name);
                        0
                    }
                }
                Err(proto::ApiError::Http(401, _)) => {
                    eprintln!("the key was not accepted. Use the weak account key from your BitBOINC account page.");
                    1
                }
                Err(proto::ApiError::Http(429, _)) => {
                    eprintln!("too many wrong attempts: wait an hour.");
                    1
                }
                Err(proto::ApiError::Http(409, _)) => {
                    eprintln!("this account already has 10 devices: sign one out on the Thermopsis page (My computers).");
                    1
                }
                Err(e) => {
                    eprintln!("sign-in failed: {}", e);
                    1
                }
            }
        }
        "logout" => {
            if cfg.token.is_empty() {
                println!("not signed in");
            } else {
                let _ = proto::Client::new(&cfg.server).logout(&cfg.token);
                cfg.token.clear();
                cfg.user.clear();
                let _ = config::save(&cfg);
                println!("signed out");
            }
            0
        }
        "status" => {
            if cfg.token.is_empty() {
                println!("not signed in");
                1
            } else {
                match proto::Client::new(&cfg.server).status(&cfg.token) {
                    Ok(s) => {
                        println!("{}", serde_json::to_string_pretty(&s).unwrap_or_default());
                        0
                    }
                    Err(e) => {
                        eprintln!("{}", e);
                        1
                    }
                }
            }
        }
        "disks" => {
            let (temps, drives) = thermopsis_agent::windisk::read(true);
            println!("{}", serde_json::to_string_pretty(&serde_json::json!({"temperatures": temps, "drives": drives})).unwrap_or_default());
            0
        }
        "inventory" => {
            println!("{}", serde_json::to_string_pretty(&inventory::collect()).unwrap_or_default());
            0
        }
        "once" => {
            let mut s = sensors::Generic::new();
            let _ = s.read();
            std::thread::sleep(std::time::Duration::from_millis(1200));
            let mut w = report::Window::new(thermopsis_agent::now().saturating_sub(60));
            w.add(&s.read());
            let inv = inventory::collect();
            let h = report::inventory_hash(&inv);
            println!("{}", serde_json::to_string_pretty(&w.report(thermopsis_agent::now(), &h, Some(&inv), vec![], vec![])).unwrap_or_default());
            0
        }
        "test" => {
            let kind = args.get(1).cloned().unwrap_or_default();
            let mut params = serde_json::json!({});
            if let Some(n) = arg_value(&args, "--secs").and_then(|v| v.parse::<u64>().ok()) {
                params["secs"] = n.into();
            }
            if let Some(n) = arg_value(&args, "--pct").and_then(|v| v.parse::<u64>().ok()) {
                params["pct"] = n.into();
            }
            let mut s = sensors::Generic::new();
            match selftest::run(&kind, &params, &AtomicBool::new(false), &mut s) {
                Some(r) => {
                    println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                    0
                }
                None => {
                    eprintln!("unknown test \"{}\" (memtest, membw, stress, diskbench)", kind);
                    2
                }
            }
        }
        "run" => {
            if cfg.token.is_empty() {
                eprintln!("not signed in: run  thermopsis-agent login <KEY>  first");
                std::process::exit(1);
            }
            let quit = Arc::new(AtomicBool::new(false));
            let opt = agent::Options::from_env();
            let log = |m: &str| eprintln!("[{}] {}", thermopsis_agent::now(), m);
            match agent::run(&cfg, &opt, &quit, &log) {
                agent::Stop::SignedOut => {
                    eprintln!("the project signed this device out: run  thermopsis-agent login <KEY>  again");
                    cfg.token.clear();
                    let _ = config::save(&cfg);
                    1
                }
                agent::Stop::Quit => 0,
            }
        }
        _ => {
            help();
            0
        }
    };
    std::process::exit(code);
}
