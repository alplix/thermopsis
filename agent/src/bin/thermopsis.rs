//! Thermopsis window: sign in with the BitBOINC key, see this computer's readings live, switch driver mode and theme, run a test by hand.
//! Two themes: "paper" (light) and "arcade" (dark, 8-bit). The agent loop runs in a background thread while the window is open.
#![cfg_attr(windows, windows_subsystem = "windows")]
use eframe::egui::{self, Color32, FontId, RichText, Rounding, Stroke};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use thermopsis_agent::{agent, config, driver, proto, selftest, sensors::Generic};

/// 32x32 pixel-art thermometer for the tray: dark rim, light tube with a red column, red bulb
fn tray_icon_rgba() -> Vec<u8> {
    let mut px = vec![0u8; 32 * 32 * 4];
    let mut put = |x: usize, y: usize, c: [u8; 4]| px[(y * 32 + x) * 4..(y * 32 + x) * 4 + 4].copy_from_slice(&c);
    let (rim, tube, red) = ([0x17, 0x14, 0x33, 255], [0xf6, 0xf1, 0xe7, 255], [0xff, 0x3e, 0xa5, 255]);
    for y in 3..22 {
        for x in 12..20 {
            put(x, y, if x == 12 || x == 19 || y == 3 { rim } else { tube });
        }
    }
    for y in 9..22 {
        for x in 14..18 {
            put(x, y, red);
        }
    }
    for y in 18..30 {
        for x in 8..24 {
            let (dx, dy) = (x as f32 - 15.5, y as f32 - 23.5);
            let d = (dx * dx + dy * dy).sqrt();
            if d <= 6.5 {
                put(x, y, if d > 5.2 { rim } else { red });
            }
        }
    }
    px
}

struct Palette {
    bg: Color32,
    panel: Color32,
    ink: Color32,
    dim: Color32,
    accent: Color32,
    good: Color32,
    bad: Color32,
    line: Color32,
    round: f32,
}

fn palette(arcade: bool) -> Palette {
    if arcade {
        Palette { bg: Color32::from_rgb(0x0d, 0x0b, 0x1e), panel: Color32::from_rgb(0x17, 0x14, 0x33), ink: Color32::from_rgb(0xe8, 0xe6, 0xff), dim: Color32::from_rgb(0x8a, 0x86, 0xb8), accent: Color32::from_rgb(0xff, 0x3e, 0xa5), good: Color32::from_rgb(0x39, 0xff, 0x88), bad: Color32::from_rgb(0xff, 0x6b, 0x4a), line: Color32::from_rgb(0x29, 0xd8, 0xff), round: 0.0 }
    } else {
        Palette { bg: Color32::from_rgb(0xf6, 0xf1, 0xe7), panel: Color32::from_rgb(0xfd, 0xfa, 0xf3), ink: Color32::from_rgb(0x2b, 0x2a, 0x28), dim: Color32::from_rgb(0x7a, 0x74, 0x69), accent: Color32::from_rgb(0xb8, 0x4a, 0x1f), good: Color32::from_rgb(0x2f, 0x7d, 0x4f), bad: Color32::from_rgb(0xb3, 0x26, 0x1e), line: Color32::from_rgb(0xd9, 0xd0, 0xbd), round: 6.0 }
    }
}

fn apply(ctx: &egui::Context, p: &Palette, arcade: bool) {
    let mut v = if arcade { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.panel;
    v.override_text_color = Some(p.ink);
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent;
    let r = Rounding::same(p.round);
    for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.rounding = r;
        w.bg_stroke = Stroke::new(if arcade { 2.0_f32 } else { 1.0_f32 }, p.line);
        w.fg_stroke.color = p.ink;
    }
    v.widgets.inactive.weak_bg_fill = p.panel;
    v.widgets.hovered.weak_bg_fill = if arcade { Color32::from_rgb(0x2a, 0x24, 0x5c) } else { Color32::from_rgb(0xee, 0xe5, 0xd2) };
    v.widgets.active.weak_bg_fill = p.accent;
    ctx.set_visuals(v);
    let mut style = (*ctx.style()).clone();
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    if arcade {
        for (_, f) in style.text_styles.iter_mut() {
            f.family = egui::FontFamily::Monospace;
        }
    }
    ctx.set_style(style);
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Live,
    Tests,
    Settings,
}

struct App {
    cfg: config::Config,
    tab: Tab,
    key: String,
    login_msg: String,
    quit: Arc<AtomicBool>,
    agent_running: bool,
    stopped_note: Arc<Mutex<String>>,
    test_out: Arc<Mutex<String>>,
    test_busy: Arc<AtomicBool>,
    shown_theme: String,
    quitting: Arc<AtomicBool>,
    _tray: Option<tray_icon::TrayIcon>,
}

impl App {
    fn arcade(&self) -> bool {
        self.cfg.theme == "arcade"
    }

    fn start_agent(&mut self) {
        if self.agent_running || self.cfg.token.is_empty() {
            return;
        }
        self.agent_running = true;
        self.quit.store(false, Ordering::Relaxed);
        let (cfg, quit, note) = (self.cfg.clone(), self.quit.clone(), self.stopped_note.clone());
        std::thread::spawn(move || {
            let opt = agent::Options::from_env();
            if let agent::Stop::SignedOut = agent::run(&cfg, &opt, &quit, &|_| {}) {
                if let Ok(mut n) = note.lock() {
                    *n = "The project signed this device out. Sign in again.".into();
                }
            }
        });
    }

    fn stop_agent(&mut self) {
        self.quit.store(true, Ordering::Relaxed);
        self.agent_running = false;
    }

    fn login_view(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.add_space(30.0);
        ui.label(RichText::new("THERMOPSIS").font(FontId::monospace(30.0)).color(p.accent));
        ui.label(RichText::new("Temperatures, power and drive health of this computer, reported to your BitBOINC account.").color(p.dim));
        ui.add_space(14.0);
        ui.label("Weak account key (BitBOINC account page):");
        ui.add(egui::TextEdit::singleline(&mut self.key).password(true).desired_width(360.0));
        ui.label("Name of this computer on the website:");
        ui.add(egui::TextEdit::singleline(&mut self.cfg.name).desired_width(360.0));
        if ui.button("Sign in").clicked() {
            match proto::Client::new(&self.cfg.server).login(self.key.trim(), &self.cfg.device, &self.cfg.name) {
                Ok((token, user)) => {
                    self.cfg.token = token;
                    self.cfg.user = user;
                    let _ = config::save(&self.cfg);
                    self.key.clear();
                    self.login_msg.clear();
                    self.start_agent();
                }
                Err(proto::ApiError::Http(401, _)) => self.login_msg = "The key was not accepted.".into(),
                Err(proto::ApiError::Http(429, _)) => self.login_msg = "Too many wrong attempts: wait an hour.".into(),
                Err(proto::ApiError::Http(409, _)) => self.login_msg = "This account already has 10 devices: sign one out on the website.".into(),
                Err(e) => self.login_msg = format!("Sign-in failed: {}", e),
            }
        }
        if !self.login_msg.is_empty() {
            ui.label(RichText::new(&self.login_msg).color(p.bad));
        }
    }

    fn live_view(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let live = agent::LIVE.lock().map(|l| l.clone()).unwrap_or_default();
        let find = |pre: &str, suf: &str| live.readings.iter().filter(|(k, _)| k.starts_with(pre) && k.ends_with(suf)).map(|(_, v)| *v).fold(None, |a: Option<f64>, v| Some(a.map_or(v, |m| m.max(v))));
        let cpu = find("t.win.cpu.", "package").or_else(|| find("t.", "package")).or_else(|| find("t.cpu", ""));
        let gpu = find("t.nvml.", ".gpu");
        let nvme = find("t.win.nvme", "");
        let sata = find("t.win.sata", "");
        let cpu_w = find("p.win.cpu.", "package");
        let gpu_w = find("p.nvml.", ".board");
        let load = find("u.cpu", "");
        ui.horizontal_wrapped(|ui| {
            for (name, v, unit) in [("CPU", cpu, "°C"), ("GPU", gpu, "°C"), ("NVMe", nvme, "°C"), ("SATA", sata, "°C"), ("CPU power", cpu_w, "W"), ("GPU power", gpu_w, "W"), ("CPU load", load, "%")] {
                let Some(v) = v else { continue };
                egui::Frame::none().fill(p.panel).stroke(Stroke::new(if p.round == 0.0 { 2.0_f32 } else { 1.0_f32 }, p.line)).rounding(Rounding::same(p.round)).inner_margin(12.0).show(ui, |ui| {
                    ui.set_min_width(130.0);
                    ui.label(RichText::new(name).color(p.dim));
                    ui.label(RichText::new(format!("{:.0} {}", v, unit)).font(FontId::monospace(22.0)).color(p.accent));
                });
            }
        });
        if live.readings.is_empty() {
            ui.label(RichText::new("The first reading comes within a minute.").color(p.dim));
        }
        ui.add_space(6.0);
        ui.label(RichText::new(format!("{} readings   reports sent: {}   waiting: {}{}", live.readings.len(), live.sent, live.queued, if live.test_running { "   test running" } else { "" })).color(p.dim));
        if cpu.is_none() && cfg!(windows) && !driver::enabled() {
            ui.label(RichText::new("CPU temperature and power need the driver mode (Settings).").color(p.dim));
        }
        ui.separator();
        egui::ScrollArea::vertical().max_height(150.0).stick_to_bottom(true).show(ui, |ui| {
            for line in &live.log {
                ui.label(RichText::new(line).font(FontId::monospace(12.0)).color(p.dim));
            }
        });
    }

    fn tests_view(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.label(RichText::new("Run a test now on this computer. The result is shown here; tests queued on the website run by themselves.").color(p.dim));
        let busy = self.test_busy.load(Ordering::Relaxed);
        ui.horizontal_wrapped(|ui| {
            for (kind, label) in [("memtest", "Memory test"), ("membw", "Memory bandwidth"), ("stress", "CPU stress"), ("diskbench", "Disk speed")] {
                if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                    self.test_busy.store(true, Ordering::Relaxed);
                    let (out, flag) = (self.test_out.clone(), self.test_busy.clone());
                    let kind = kind.to_string();
                    if let Ok(mut o) = out.lock() {
                        *o = format!("{} running...", kind);
                    }
                    std::thread::spawn(move || {
                        let mut s = Generic::new();
                        let r = selftest::run(&kind, &serde_json::json!({"secs": 20}), &AtomicBool::new(false), &mut s);
                        if let Ok(mut o) = out.lock() {
                            *o = r.map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()).unwrap_or_else(|| "unknown test".into());
                        }
                        flag.store(false, Ordering::Relaxed);
                    });
                }
            }
        });
        egui::ScrollArea::vertical().show(ui, |ui| {
            let t = self.test_out.lock().map(|t| t.clone()).unwrap_or_default();
            ui.label(RichText::new(t).font(FontId::monospace(12.0)));
        });
    }

    fn settings_view(&mut self, ui: &mut egui::Ui, p: &Palette) {
        ui.label(RichText::new(format!("Signed in as {}", self.cfg.user)).color(p.good));
        ui.horizontal(|ui| {
            ui.label("Theme:");
            let was = self.cfg.theme.clone();
            ui.selectable_value(&mut self.cfg.theme, "paper".to_string(), "Paper (light)");
            ui.selectable_value(&mut self.cfg.theme, "arcade".to_string(), "Arcade (dark, 8-bit)");
            if was != self.cfg.theme {
                let _ = config::save(&self.cfg);
            }
        });
        if cfg!(windows) {
            let mut on = driver::enabled();
            if ui.checkbox(&mut on, "Driver mode: CPU and board sensors (administrator prompt at start, needs the PawnIO driver)").changed() {
                let _ = driver::set(on);
                ui.label(RichText::new("Takes effect the next time the app starts.").color(p.dim));
            }
        }
        ui.add_space(10.0);
        if ui.button("Sign out").clicked() {
            self.stop_agent();
            let _ = proto::Client::new(&self.cfg.server).logout(&self.cfg.token);
            self.cfg.token.clear();
            self.cfg.user.clear();
            let _ = config::save(&self.cfg);
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _f: &mut eframe::Frame) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.quitting.load(Ordering::Relaxed) && self._tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        let arcade = self.arcade();
        if self.shown_theme != self.cfg.theme {
            apply(ctx, &palette(arcade), arcade);
            self.shown_theme = self.cfg.theme.clone();
        }
        let p = palette(arcade);
        if let Ok(mut n) = self.stopped_note.lock() {
            if !n.is_empty() {
                self.login_msg = std::mem::take(&mut *n);
                self.cfg.token.clear();
                self.agent_running = false;
                let _ = config::save(&self.cfg);
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            if self.cfg.token.is_empty() {
                self.login_view(ui, &p);
                return;
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("THERMOPSIS").font(FontId::monospace(18.0)).color(p.accent));
                ui.add_space(12.0);
                ui.selectable_value(&mut self.tab, Tab::Live, "Live");
                ui.selectable_value(&mut self.tab, Tab::Tests, "Tests");
                ui.selectable_value(&mut self.tab, Tab::Settings, "Settings");
            });
            ui.separator();
            match self.tab {
                Tab::Live => self.live_view(ui, &p),
                Tab::Tests => self.tests_view(ui, &p),
                Tab::Settings => self.settings_view(ui, &p),
            }
        });
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.quit.store(true, Ordering::Relaxed);
    }
}

fn main() -> eframe::Result<()> {
    let cfg = config::load();
    let mut app = App { cfg, tab: Tab::Live, key: String::new(), login_msg: String::new(), quit: Arc::new(AtomicBool::new(false)), agent_running: false, stopped_note: Arc::new(Mutex::new(String::new())), test_out: Arc::new(Mutex::new(String::new())), test_busy: Arc::new(AtomicBool::new(false)), shown_theme: String::new(), quitting: Arc::new(AtomicBool::new(false)), _tray: None };
    if let Ok(t) = std::env::var("THERMOPSIS_THEME") {
        app.cfg.theme = t;
    }
    if driver::enabled() {
        let _ = driver::start();
    }
    app.start_agent();
    let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([620.0, 460.0]).with_title("Thermopsis"), ..Default::default() };
    eframe::run_native(
        "Thermopsis",
        opts,
        Box::new(|cc| {
            use tray_icon::menu::{Menu, MenuEvent, MenuItem};
            let (show, quit) = (MenuItem::with_id("show", "Open Thermopsis", true, None), MenuItem::with_id("quit", "Quit", true, None));
            let menu = Menu::new();
            let _ = menu.append_items(&[&show, &quit]);
            let icon = tray_icon::Icon::from_rgba(tray_icon_rgba(), 32, 32).ok();
            let tray = icon.and_then(|i| tray_icon::TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Thermopsis").with_icon(i).build().ok());
            if tray.is_some() {
                let (ctx, quitting, stop) = (cc.egui_ctx.clone(), app.quitting.clone(), app.quit.clone());
                MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                    if e.id.0 == "show" {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    } else if e.id.0 == "quit" {
                        quitting.store(true, Ordering::Relaxed);
                        stop.store(true, Ordering::Relaxed);
                        std::process::exit(0);
                    }
                    ctx.request_repaint();
                }));
            }
            let mut app = app;
            app._tray = tray;
            Ok(Box::new(app))
        }),
    )
}
