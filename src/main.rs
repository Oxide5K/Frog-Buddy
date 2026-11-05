#![windows_subsystem = "windows"]

use eframe::egui::{self, Color32, Pos2, Sense, Stroke, Vec2};
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const RELEASES: &str = "https://github.com/Oxide5K/Frog-Buddy/releases/latest";

const W: f32 = 240.0;
const H: f32 = 230.0;
const MARGIN: f32 = 12.0;
// shortcut: fixed taskbar allowance instead of querying the real work area; upgrade if taskbar is moved/resized
const TASKBAR: f32 = 48.0;
const SHOW_FOR: Duration = Duration::from_secs(20);

const REMINDERS: &[&str] = &[
    "Ribbit! Time for a sip of water.",
    "Stand up and stretch those legs!",
    "Look at something 20 feet away for 20 seconds.",
    "Check your posture, friend.",
    "Take three deep breaths.",
    "Blink a few times, your eyes will thank you.",
    "Hydration check! Drink some water.",
];

const CORNERS: [&str; 4] = ["Top left", "Top right", "Bottom left", "Bottom right"];

#[derive(Clone, Copy, PartialEq, Debug)]
struct Config {
    corner: usize,
    minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self { corner: 3, minutes: 30 }
    }
}

impl Config {
    fn parse(s: &str) -> Self {
        let mut c = Self::default();
        for line in s.lines() {
            match line.split_once('=') {
                Some(("corner", v)) => c.corner = v.trim().parse().unwrap_or(c.corner).min(3),
                Some(("minutes", v)) => c.minutes = v.trim().parse().unwrap_or(c.minutes).clamp(1, 240),
                _ => {}
            }
        }
        c
    }

    fn path() -> Option<std::path::PathBuf> {
        std::env::var_os("APPDATA").map(|d| std::path::Path::new(&d).join("FrogBuddy").join("config.txt"))
    }

    fn load() -> Self {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|s| Self::parse(&s)).unwrap_or_default()
    }

    fn save(&self) {
        if let Some(p) = Self::path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(p, format!("corner={}\nminutes={}\n", self.corner, self.minutes));
        }
    }
}

fn hidden(exe: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

fn ver(s: &str) -> Vec<u32> {
    s.trim().trim_start_matches('v').split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Latest release tag, if newer than this build.
fn check_latest() -> Option<String> {
    // /releases/latest redirects to /releases/tag/<tag>; reading the redirect skips the rate-limited JSON API
    let out = hidden("curl.exe").args(["-sI", "-o", "NUL", "-w", "%{redirect_url}", RELEASES]).output().ok()?;
    let url = String::from_utf8_lossy(&out.stdout);
    let tag = url.rsplit_once("/tag/")?.1.trim().to_string();
    (ver(&tag) > ver(env!("CARGO_PKG_VERSION"))).then_some(tag)
}

/// Downloads the latest MSI and starts a detached installer that relaunches the app; the caller must exit.
fn start_installer() -> Result<(), String> {
    let msi = std::env::temp_dir().join("frog-buddy-update.msi");
    let ok = hidden("curl.exe")
        .args(["-sfL", "-o"])
        .arg(&msi)
        .arg(format!("{RELEASES}/download/frog-buddy.msi"))
        .status()
        .map_err(|e| format!("Could not run curl: {e}"))?
        .success();
    if !ok {
        return Err("Downloading the update failed.".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // cmd outlives us: waits for msiexec to replace the files, then reopens the app
    hidden("cmd")
        .arg("/c")
        .raw_arg(format!("start /wait msiexec /i \"{}\" /passive & start \"\" \"{}\"", msi.display(), exe.display()))
        .spawn()
        .map_err(|e| format!("Could not start installer: {e}"))?;
    Ok(())
}

struct Frog {
    _tray: tray_icon::TrayIcon,
    show_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
    update_rx: Option<Receiver<Option<String>>>,
    update: Option<String>,
    cfg: Config,
    message: Option<String>,
    shown_at: Instant,
    next_at: Instant,
    settings: bool,
    placed: Option<usize>,
    passthrough: Option<bool>,
    idx: usize,
}

impl Frog {
    fn new(cc: &eframe::CreationContext) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        cc.egui_ctx.style_mut(|s| s.text_styles.iter_mut().for_each(|(_, f)| f.size = f.size.max(15.0)));
        use tray_icon::menu::{Menu, MenuEvent, MenuItem};
        let show = MenuItem::new("Show", true, None);
        let quit = MenuItem::new("Quit", true, None);
        let menu = Menu::new();
        let _ = menu.append_items(&[&show, &quit]);
        let tray = tray_icon::TrayIconBuilder::new()
            .with_tooltip("Frog Buddy")
            .with_menu(Box::new(menu))
            // id 1 is the exe icon embedded by winresource in build.rs
            .with_icon(tray_icon::Icon::from_resource(1, None).expect("embedded icon"))
            .build()
            .expect("tray icon");
        let ctx = cc.egui_ctx.clone();
        MenuEvent::set_event_handler(Some(move |_| ctx.request_repaint()));
        let cfg = Config::load();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(check_latest());
        });
        Self {
            _tray: tray,
            show_id: show.id().clone(),
            quit_id: quit.id().clone(),
            update_rx: Some(rx),
            update: None,
            cfg,
            message: Some("Hi! I'm your frog buddy. Right-click me for settings.".into()),
            shown_at: Instant::now(),
            next_at: Instant::now() + Duration::from_secs(cfg.minutes as u64 * 60),
            settings: false,
            placed: None,
            passthrough: None,
            idx: 0,
        }
    }

    fn visible(&self) -> bool {
        self.message.is_some() || self.settings || self.update.is_some()
    }

    fn hide(&mut self) {
        self.message = None;
        self.settings = false;
        self.next_at = Instant::now() + Duration::from_secs(self.cfg.minutes as u64 * 60);
    }

    fn place(&mut self, ctx: &egui::Context) {
        if self.placed == Some(self.cfg.corner) {
            return;
        }
        let Some(m) = ctx.input(|i| i.viewport().monitor_size) else { return };
        let x = if self.cfg.corner % 2 == 0 { MARGIN } else { m.x - W - MARGIN };
        let y = if self.cfg.corner < 2 { MARGIN } else { m.y - H - MARGIN - TASKBAR };
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(Pos2::new(x, y)));
        self.placed = Some(self.cfg.corner);
    }
}

fn draw_frog(p: &egui::Painter, c: Pos2) {
    let green = Color32::from_rgb(88, 170, 70);
    let dark = Color32::from_rgb(50, 110, 40);
    p.circle(c, 42.0, green, Stroke::new(2.0, dark));
    for dx in [-22.0, 22.0] {
        let e = c + Vec2::new(dx, -34.0);
        p.circle(e, 15.0, green, Stroke::new(2.0, dark));
        p.circle_filled(e, 9.0, Color32::WHITE);
        p.circle_filled(e + Vec2::new(0.0, 1.5), 5.0, Color32::BLACK);
        p.circle_filled(c + Vec2::new(dx * 1.2, 8.0), 6.0, Color32::from_rgb(240, 140, 150));
    }
    let mouth: Vec<Pos2> = (0..=12)
        .map(|i| {
            let t = std::f32::consts::PI * (0.15 + 0.7 * i as f32 / 12.0);
            c + Vec2::new(-22.0 * t.cos(), 2.0 + 14.0 * t.sin())
        })
        .collect();
    p.add(egui::Shape::line(mouth, Stroke::new(3.0, dark)));
}

impl eframe::App for Frog {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        self.place(ctx);
        while let Ok(e) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if e.id == self.quit_id {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            } else if e.id == self.show_id {
                self.settings = true;
            }
        }
        if let Some(Ok(tag)) = self.update_rx.as_ref().map(|rx| rx.try_recv()) {
            self.update_rx = None;
            self.update = tag;
            if self.update.is_some() {
                self.settings = false;
            }
        }
        let now = Instant::now();
        if !self.visible() && now >= self.next_at {
            self.message = Some(REMINDERS[self.idx % REMINDERS.len()].into());
            self.idx += 1;
            self.shown_at = now;
        }
        if self.message.is_some() && !self.settings && self.update.is_none() && now - self.shown_at > SHOW_FOR {
            self.hide();
        }
        // Idle: invisible window lets clicks fall through to whatever is underneath.
        let pass = !self.visible();
        if self.passthrough != Some(pass) {
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(pass));
            self.passthrough = Some(pass);
        }
        ctx.request_repaint_after(Duration::from_secs(1));

        egui::CentralPanel::default().frame(egui::Frame::none()).show(ctx, |ui| {
            if !self.visible() {
                return;
            }
            let frog_c = Pos2::new(W / 2.0, H - 50.0);
            let resp = ui.interact(
                egui::Rect::from_center_size(frog_c, Vec2::splat(100.0)),
                ui.id().with("frog"),
                Sense::click(),
            );
            draw_frog(ui.painter(), frog_c);

            let bubble = egui::Rect::from_min_size(Pos2::new(6.0, 6.0), Vec2::new(W - 12.0, 114.0));
            ui.painter().rect(bubble, 12.0, Color32::from_rgb(252, 252, 240), Stroke::new(2.0, Color32::from_rgb(50, 110, 40)));
            ui.allocate_ui_at_rect(bubble.shrink(10.0), |ui| {
                ui.visuals_mut().override_text_color = Some(Color32::from_rgb(30, 40, 30));
                if let Some(tag) = self.update.clone() {
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new(format!("Ribbit! Frog Buddy {tag} is out. Update now?")).size(15.0));
                        ui.horizontal(|ui| {
                            if ui.button("Yes").clicked() {
                                match start_installer() {
                                    Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                                    Err(e) => {
                                        self.update = None;
                                        self.message = Some(e);
                                        self.shown_at = Instant::now();
                                    }
                                }
                            }
                            if ui.button("No").clicked() {
                                self.update = None;
                                self.hide();
                            }
                        });
                    });
                } else if self.settings {
                    ui.label("Where should I sit?");
                    egui::Grid::new("corners").show(ui, |ui| {
                        for (i, name) in CORNERS.iter().enumerate() {
                            if ui.selectable_label(self.cfg.corner == i, *name).clicked() {
                                self.cfg.corner = i;
                                self.cfg.save();
                            }
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label("Every");
                        if ui.add(egui::DragValue::new(&mut self.cfg.minutes).range(1..=240).suffix(" min")).changed() {
                            self.cfg.save();
                        }
                        if ui.button("Done").clicked() {
                            self.hide();
                        }
                        if ui.button("Quit").clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                } else if let Some(m) = &self.message {
                    ui.vertical_centered(|ui| ui.label(egui::RichText::new(m).size(16.0)));
                }
            });

            if self.update.is_some() {
            } else if resp.secondary_clicked() {
                self.settings = true;
            } else if resp.clicked() && !self.settings {
                self.hide();
            }
        });
    }
}

fn main() -> eframe::Result {
    // Named mutex lives until the process exits; a second launch sees it and quits.
    let name: Vec<u16> = "Local\\FrogBuddySingleInstance\0".encode_utf16().collect();
    unsafe {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        windows_sys::Win32::System::Threading::CreateMutexW(std::ptr::null(), 0, name.as_ptr());
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
    }
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Frog Buddy")
            .with_inner_size([W, H])
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native("Frog Buddy", opts, Box::new(|cc| Ok(Box::new(Frog::new(cc)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(ver("v1.0.1") > ver("1.0.0"));
        assert!(ver("v1.10.0") > ver("1.9.9"));
        assert!(ver("v1.0.0") <= ver("1.0.0"));
    }

    #[test]
    fn config_parse() {
        assert_eq!(Config::parse("corner=1\nminutes=15\n"), Config { corner: 1, minutes: 15 });
        assert_eq!(Config::parse("corner=9\nminutes=0\njunk"), Config { corner: 3, minutes: 1 });
        assert_eq!(Config::parse(""), Config::default());
    }
}
