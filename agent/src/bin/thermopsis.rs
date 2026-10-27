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
    warn: Color32,
    line: Color32,
    round: f32,
}

fn palette(arcade: bool) -> Palette {
    if arcade {
        Palette { bg: Color32::from_rgb(0x0d, 0x0b, 0x1e), panel: Color32::from_rgb(0x17, 0x14, 0x33), ink: Color32::from_rgb(0xe8, 0xe6, 0xff), dim: Color32::from_rgb(0x8a, 0x86, 0xb8), accent: Color32::from_rgb(0xff, 0x3e, 0xa5), good: Color32::from_rgb(0x39, 0xff, 0x88), bad: Color32::from_rgb(0xff, 0x4d, 0x4d), warn: Color32::from_rgb(0xff, 0xd1, 0x3b), line: Color32::from_rgb(0x29, 0xd8, 0xff), round: 0.0 }
    } else {
        Palette { bg: Color32::from_rgb(0xf6, 0xf1, 0xe7), panel: Color32::from_rgb(0xfd, 0xfa, 0xf3), ink: Color32::from_rgb(0x2b, 0x2a, 0x28), dim: Color32::from_rgb(0x7a, 0x74, 0x69), accent: Color32::from_rgb(0xb8, 0x4a, 0x1f), good: Color32::from_rgb(0x2f, 0x7d, 0x4f), bad: Color32::from_rgb(0xb3, 0x26, 0x1e), warn: Color32::from_rgb(0xc7, 0x8a, 0x12), line: Color32::from_rgb(0xd9, 0xd0, 0xbd), round: 6.0 }
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
    style.spacing.item_spacing = egui::vec2(10.0, 10.0);
    style.spacing.button_padding = egui::vec2(14.0, 8.0);
    style.spacing.window_margin = egui::Margin::same(16.0);
    use egui::TextStyle::*;
    let pix = egui::FontFamily::Name("pixel".into());
    for (t, size_paper, size_arcade) in [(Heading, 22.0, 16.0), (Body, 15.0, 10.0), (Button, 15.0, 10.0), (Small, 12.0, 8.0), (Monospace, 13.0, 9.0)] {
        if let Some(f) = style.text_styles.get_mut(&t) {
            f.size = if arcade { size_arcade } else { size_paper };
            if arcade {
                f.family = pix.clone();
            } else if t != Monospace {
                f.family = egui::FontFamily::Proportional;
            } else {
                f.family = egui::FontFamily::Monospace;
            }
        }
    }
    ctx.set_style(style);
}

fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("pixel".into(), egui::FontData::from_static(include_bytes!("../../assets/PressStart2P.ttf")));
    fonts.families.insert(egui::FontFamily::Name("pixel".into()), vec!["pixel".into()]);
    ctx.set_fonts(fonts);
}

fn pix(arcade: bool, size: f32) -> FontId {
    if arcade { FontId::new(size * 0.7, egui::FontFamily::Name("pixel".into())) } else { FontId::proportional(size) }
}

/// temperature colour: calm below 60, warm below 80, hot above
fn heat(p: &Palette, t: f64) -> Color32 {
    if t < 60.0 { p.good } else if t < 80.0 { p.warn } else { p.bad }
}

/// one reading as a card: name, big value, a gauge (blocks in arcade, a bar in paper); no value -> "--" and an empty gauge
fn card(ui: &mut egui::Ui, p: &Palette, arcade: bool, width: f32, name: &str, reading: Option<(String, f32, Color32)>) {
    let size = egui::vec2(width, 92.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let color = reading.as_ref().map(|r| r.2).unwrap_or(p.dim);
    painter.rect(rect, Rounding::same(p.round), p.panel, Stroke::new(if arcade { 2.0_f32 } else { 1.0_f32 }, if arcade && reading.is_some() { color } else { p.line }));
    if !arcade {
        painter.rect_filled(egui::Rect::from_min_size(rect.min, egui::vec2(5.0, rect.height())), Rounding { nw: p.round, sw: p.round, ne: 0.0, se: 0.0 }, color);
    }
    let pad = 14.0;
    painter.text(rect.min + egui::vec2(pad, 11.0), egui::Align2::LEFT_TOP, name, pix(arcade, 12.0), p.dim);
    let (text, frac) = reading.as_ref().map(|r| (r.0.clone(), r.1)).unwrap_or(("--".to_string(), 0.0));
    painter.text(rect.min + egui::vec2(pad, 31.0), egui::Align2::LEFT_TOP, text, pix(arcade, 24.0), if reading.is_some() { p.ink } else { p.dim });
    let bar = egui::Rect::from_min_size(rect.min + egui::vec2(pad, 68.0), egui::vec2(size.x - 2.0 * pad, 9.0));
    let frac = frac.clamp(0.0, 1.0);
    if arcade {
        let n = 10;
        let w = (bar.width() - 2.0 * (n as f32 - 1.0)) / n as f32;
        for i in 0..n {
            let r = egui::Rect::from_min_size(bar.min + egui::vec2(i as f32 * (w + 2.0), 0.0), egui::vec2(w, bar.height()));
            let on = (i as f32 + 0.5) / n as f32 <= frac;
            painter.rect_filled(r, Rounding::ZERO, if on { color } else { p.line.gamma_multiply(0.25) });
        }
    } else {
        painter.rect_filled(bar, Rounding::same(4.5), p.line.gamma_multiply(0.5));
        painter.rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * frac, bar.height())), Rounding::same(4.5), color);
    }
}

fn chip(ui: &mut egui::Ui, p: &Palette, arcade: bool, text: &str, color: Color32) {
    egui::Frame::none().fill(color.gamma_multiply(0.18)).stroke(Stroke::new(1.0_f32, color)).rounding(Rounding::same(if arcade { 0.0 } else { 20.0 })).inner_margin(egui::Margin::symmetric(10.0, 4.0)).show(ui, |ui| {
        ui.label(RichText::new(text).font(pix(arcade, 12.0)).color(color));
    });
    let _ = p;
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
        let arcade = self.arcade();
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(RichText::new("THERMOPSIS").font(pix(arcade, 40.0)).color(p.accent));
            ui.label(RichText::new("Temperatures, power and drive health of this computer,").color(p.dim));
            ui.label(RichText::new("reported to your BitBOINC account.").color(p.dim));
            ui.add_space(18.0);
            egui::Frame::none().fill(p.panel).stroke(Stroke::new(if arcade { 2.0_f32 } else { 1.0_f32 }, p.line)).rounding(Rounding::same(p.round)).inner_margin(20.0).show(ui, |ui| {
                ui.set_width(380.0);
                ui.label(RichText::new("Weak account key").color(p.dim));
                ui.add(egui::TextEdit::singleline(&mut self.key).password(true).hint_text("on your BitBOINC account page").desired_width(f32::INFINITY));
                ui.label(RichText::new("Name of this computer").color(p.dim));
                ui.add(egui::TextEdit::singleline(&mut self.cfg.name).desired_width(f32::INFINITY));
                ui.add_space(6.0);
                if ui.add_sized([ui.available_width(), 36.0], egui::Button::new(RichText::new("Sign in").color(if arcade { p.bg } else { Color32::WHITE })).fill(p.accent)).clicked() {
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
            });
        });
    }

    fn live_view(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let arcade = self.arcade();
        let live = agent::LIVE.lock().map(|l| l.clone()).unwrap_or_default();
        let find = |pre: &str, suf: &str| live.readings.iter().filter(|(k, _)| k.starts_with(pre) && k.ends_with(suf)).map(|(_, v)| *v).fold(None, |a: Option<f64>, v| Some(a.map_or(v, |m| m.max(v))));
        let cpu = find("t.win.cpu.", "package").or_else(|| find("t.", "package")).or_else(|| find("t.cpu", ""));
        let gpu = find("t.nvml.", ".gpu");
        let nvme = find("t.win.nvme", "");
        let sata = find("t.win.sata", "");
        let cpu_w = find("p.win.cpu.", "package");
        let gpu_w = find("p.nvml.", ".board");
        let load = find("u.cpu", "");
        let mem = find("m.", "used_percent");
        let gap = 12.0;
        let w = ((ui.available_width() - 3.0 * gap) / 4.0).floor();
        let temp = |v: Option<f64>| v.map(|v| (format!("{:.0}\u{b0}C", v), (v / 100.0) as f32, heat(p, v)));
        let watt = |v: Option<f64>, max: f64| v.map(|v| (format!("{:.0} W", v), (v / max) as f32, p.accent));
        let pct = |v: Option<f64>| v.map(|v| (format!("{:.0}%", v), (v / 100.0) as f32, p.line));
        ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
        ui.horizontal(|ui| {
            card(ui, p, arcade, w, "CPU", temp(cpu));
            card(ui, p, arcade, w, "GPU", temp(gpu));
            card(ui, p, arcade, w, "NVMe", temp(nvme));
            card(ui, p, arcade, w, "SATA", temp(sata));
        });
        ui.horizontal(|ui| {
            card(ui, p, arcade, w, "CPU power", watt(cpu_w, 150.0));
            card(ui, p, arcade, w, "GPU power", watt(gpu_w, 350.0));
            card(ui, p, arcade, w, "CPU load", pct(load));
            card(ui, p, arcade, w, "Memory", pct(mem));
        });
        if live.readings.is_empty() {
            ui.label(RichText::new("Waiting for the first reading...").color(p.dim));
        } else if cpu.is_none() && cfg!(windows) && !driver::enabled() {
            ui.label(RichText::new("CPU temperature and power need driver mode (Settings).").color(p.dim));
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            chip(ui, p, arcade, &format!("{} readings", live.readings.len()), p.dim);
            chip(ui, p, arcade, &format!("sent {}", live.sent), p.good);
            if live.queued > 0 {
                chip(ui, p, arcade, &format!("waiting {}", live.queued), p.warn);
            }
            if live.test_running {
                chip(ui, p, arcade, "test running", p.accent);
            }
        });
        ui.add_space(4.0);
        egui::Frame::none().fill(p.panel).stroke(Stroke::new(1.0_f32, p.line)).rounding(Rounding::same(p.round)).inner_margin(8.0).show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            ui.set_min_height((ui.available_height() - 12.0).max(40.0));
            egui::ScrollArea::vertical().max_height((ui.available_height() - 4.0).max(40.0)).auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                for line in live.log.iter().rev().take(40).rev() {
                    ui.label(RichText::new(line).font(FontId::monospace(11.0)).color(p.dim));
                }
            });
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
                ui.label(RichText::new("THERMOPSIS").font(pix(arcade, 22.0)).color(p.accent));
                ui.add_space(8.0);
                let on = self.agent_running;
                chip(ui, &p, arcade, if on { "connected" } else { "stopped" }, if on { p.good } else { p.bad });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(&self.cfg.user).color(p.dim));
                });
            });
            ui.horizontal(|ui| {
                for (t, label) in [(Tab::Live, "Live"), (Tab::Tests, "Tests"), (Tab::Settings, "Settings")] {
                    let sel = self.tab == t;
                    let b = egui::Button::new(RichText::new(label).color(if sel { if arcade { p.bg } else { Color32::WHITE } } else { p.ink })).fill(if sel { p.accent } else { p.panel });
                    if ui.add(b).clicked() {
                        self.tab = t;
                    }
                }
            });
            ui.add_space(2.0);
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
    let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([760.0, 500.0]).with_min_inner_size([620.0, 420.0]).with_title("Thermopsis"), ..Default::default() };
    eframe::run_native(
        "Thermopsis",
        opts,
        Box::new(|cc| {
            install_fonts(&cc.egui_ctx);
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
