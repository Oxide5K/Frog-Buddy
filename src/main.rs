#![windows_subsystem = "windows"]

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};
use std::cell::RefCell;
use std::os::windows::process::CommandExt;
use std::process::Command;
use std::rc::Rc;
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

const RELEASES: &str = "https://github.com/Oxide5K/Frog-Buddy/releases/latest";

const W: f32 = 280.0;
const H: f32 = 310.0;
const MARGIN: f32 = 12.0;
// shortcut: fixed taskbar allowance instead of querying the real work area; upgrade if taskbar is moved/resized
const TASKBAR: f32 = 48.0;

// (US wording, European wording)
const REMINDERS: &[(&str, &str)] = &[
    (
        "Ribbit! Time for a glass of water (8 oz).",
        "Ribbit! Time for a glass of water (250 ml).",
    ),
    (
        "Stand up and stretch those legs!",
        "Stand up and stretch those legs!",
    ),
    (
        "Look at something 20 feet away for 20 seconds.",
        "Look at something 6 metres away for 20 seconds.",
    ),
    ("Check your posture, friend.", "Check your posture, friend."),
    ("Take three deep breaths.", "Take three deep breaths."),
    (
        "Blink a few times, your eyes will thank you.",
        "Blink a few times, your eyes will thank you.",
    ),
    (
        "Go for a short walk, even 100 yards helps.",
        "Go for a short walk, even 100 metres helps.",
    ),
    (
        "Keep your screen about an arm's length (25 in) away.",
        "Keep your screen about an arm's length (65 cm) away.",
    ),
    (
        "Hydration check! Aim for about 2 quarts a day.",
        "Hydration check! Aim for about 2 litres a day.",
    ),
    (
        "Grab a snack? An apple beats a candy bar.",
        "Grab a snack? An apple beats a chocolate bar.",
    ),
];

const CORNERS: [&str; 4] = ["Top left", "Top right", "Bottom left", "Bottom right"];

#[derive(Clone, Copy, PartialEq, Debug)]
struct Config {
    corner: usize,
    minutes: u32,
    european: bool,
    show_secs: u32,
    volume: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            corner: 3,
            minutes: 30,
            european: false,
            show_secs: 20,
            volume: 0,
        }
    }
}

impl Config {
    fn parse(s: &str) -> Self {
        let mut c = Self::default();
        for line in s.lines() {
            match line.split_once('=') {
                Some(("corner", v)) => c.corner = v.trim().parse().unwrap_or(c.corner).min(3),
                Some(("minutes", v)) => {
                    c.minutes = v.trim().parse().unwrap_or(c.minutes).clamp(1, 240)
                }
                Some(("show_secs", v)) => {
                    c.show_secs = v.trim().parse().unwrap_or(c.show_secs).clamp(5, 300)
                }
                Some(("units", v)) => c.european = v.trim() == "european",
                Some(("volume", v)) => c.volume = v.trim().parse().unwrap_or(c.volume).min(100),
                _ => {}
            }
        }
        c
    }

    fn path() -> Option<std::path::PathBuf> {
        std::env::var_os("APPDATA").map(|d| {
            std::path::Path::new(&d)
                .join("FrogBuddy")
                .join("config.txt")
        })
    }

    fn load() -> Self {
        Self::path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| Self::parse(&s))
            .unwrap_or_default()
    }

    fn save(&self) {
        if let Some(p) = Self::path() {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            let _ = std::fs::write(
                p,
                format!(
                    "corner={}\nminutes={}\nshow_secs={}\nunits={}\nvolume={}\n",
                    self.corner,
                    self.minutes,
                    self.show_secs,
                    if self.european { "european" } else { "us" },
                    self.volume
                ),
            );
        }
    }
}

/// A tip shown every day at a set local time.
#[derive(Clone, PartialEq, Debug)]
struct Timed {
    hour: u32,
    minute: u32,
    text: String,
}

impl Timed {
    /// One line of schedule.txt: `HH:MM tip text`.
    fn parse(line: &str) -> Option<Self> {
        let (time, text) = line.trim().split_once(' ')?;
        let (h, m) = time.split_once(':')?;
        let (hour, minute) = (h.parse().ok()?, m.parse().ok()?);
        (hour < 24 && minute < 60).then(|| Timed {
            hour,
            minute,
            text: text.trim().to_string(),
        })
    }
}

fn schedule_path() -> Option<std::path::PathBuf> {
    Config::path().map(|p| p.with_file_name("schedule.txt"))
}

fn load_schedule() -> Vec<Timed> {
    let s = schedule_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default();
    s.lines().filter_map(Timed::parse).collect()
}

fn save_schedule(list: &[Timed]) {
    let Some(p) = schedule_path() else { return };
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let body: Vec<String> = list
        .iter()
        .map(|t| {
            format!(
                "{:02}:{:02} {}",
                t.hour,
                t.minute,
                t.text.replace(['\r', '\n'], " ")
            )
        })
        .collect();
    let _ = std::fs::write(p, body.join("\n"));
}

/// Local wall-clock (day, hour, minute).
fn local_time() -> (u16, u16, u16) {
    let mut t = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    (t.wDay, t.wHour, t.wMinute)
}

/// Two short 8-bit croaks as a 16-bit mono WAV; `volume` is 0..=100.
fn ribbit_wav(volume: u32) -> Vec<u8> {
    const RATE: u32 = 22050;
    let gain = (volume as f32 / 100.0).powi(2) * 0.6 * i16::MAX as f32;
    let mut samples: Vec<i16> = Vec::new();
    // (seconds, start Hz, end Hz); 0 Hz is a pause
    for (secs, f0, f1) in [(0.11, 190.0, 240.0), (0.06, 0.0, 0.0), (0.16, 170.0, 250.0)] {
        let n = (secs * RATE as f32) as usize;
        let mut phase = 0.0f32;
        for i in 0..n {
            if f0 == 0.0 {
                samples.push(0);
                continue;
            }
            let f = f0 + (f1 - f0) * i as f32 / n as f32;
            phase = (phase + f / RATE as f32) % 1.0;
            let square = if phase < 0.5 { 1.0 } else { -1.0 };
            // 35 Hz flutter gives the buzzy croak
            let flutter = if (i as f32 / RATE as f32 * 35.0) % 1.0 < 0.6 {
                1.0
            } else {
                0.3
            };
            let fade = (i.min(n - i) as f32 / (0.01 * RATE as f32)).min(1.0);
            samples.push((square * flutter * fade * gain) as i16);
        }
    }
    let data_len = samples.len() as u32 * 2;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    for v in [16u32] {
        wav.extend_from_slice(&v.to_le_bytes());
    }
    for v in [1u16, 1] {
        wav.extend_from_slice(&v.to_le_bytes()); // PCM, mono
    }
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    for v in [2u16, 16] {
        wav.extend_from_slice(&v.to_le_bytes()); // block align, bits
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    wav
}

/// 0..n in random order (Fisher-Yates driven by xorshift64; no need for a rand dependency).
fn shuffled(n: usize, rng: &mut u64) -> Vec<usize> {
    let mut v: Vec<usize> = (0..n).collect();
    for i in (1..n).rev() {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        v.swap(i, (*rng % (i as u64 + 1)) as usize);
    }
    v
}

fn default_tips(european: bool) -> Vec<String> {
    REMINDERS
        .iter()
        .map(|&(us, eu)| if european { eu } else { us }.to_string())
        .collect()
}

fn tips_path() -> Option<std::path::PathBuf> {
    Config::path().map(|p| p.with_file_name("tips.txt"))
}

/// Custom tips, one per line; `None` means the user hasn't edited them and the built-ins apply.
fn load_tips() -> Option<Vec<String>> {
    let s = std::fs::read_to_string(tips_path()?).ok()?;
    Some(
        s.lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
    )
}

fn save_tips(tips: Option<&[String]>) {
    let Some(p) = tips_path() else { return };
    match tips {
        Some(t) => {
            let _ = std::fs::create_dir_all(p.parent().unwrap());
            // one tip per line, so line breaks typed into a tip can't split it
            let body: Vec<String> = t.iter().map(|s| s.replace(['\r', '\n'], " ")).collect();
            let _ = std::fs::write(p, body.join("\n"));
        }
        None => {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn hidden(exe: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    cmd
}

fn ver(s: &str) -> Vec<u32> {
    s.trim()
        .trim_start_matches('v')
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// Latest release tag, if newer than this build.
fn check_latest() -> Option<String> {
    // /releases/latest redirects to /releases/tag/<tag>; reading the redirect skips the rate-limited JSON API
    let out = hidden("curl.exe")
        .args(["-sI", "-o", "NUL", "-w", "%{redirect_url}", RELEASES])
        .output()
        .ok()?;
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
        .raw_arg(format!(
            "start /wait msiexec /i \"{}\" /passive & start \"\" \"{}\"",
            msi.display(),
            exe.display()
        ))
        .spawn()
        .map_err(|e| format!("Could not start installer: {e}"))?;
    Ok(())
}

struct State {
    settings_id: tray_icon::menu::MenuId,
    quit_id: tray_icon::menu::MenuId,
    update_rx: Option<Receiver<Option<String>>>,
    update: Option<String>,
    cfg: Config,
    message: Option<String>,
    shown_at: Instant,
    next_at: Instant,
    settings: bool,
    // Shuffle bag of tip positions still to show; every tip appears once before any repeats.
    bag: Vec<usize>,
    last_tip: Option<usize>,
    rng: u64,
    quit: bool,
    monitor: Option<Vec2>,
    tips: Option<Vec<String>>,
    schedule: Vec<Timed>,
    last_minute: (u16, u16, u16),
    // PlaySound reads this buffer while the ribbit plays asynchronously, so it must outlive the call.
    sound: Vec<u8>,
}

impl State {
    fn frog_visible(&self) -> bool {
        self.message.is_some() || self.update.is_some()
    }

    fn hide(&mut self) {
        self.message = None;
        self.next_at = Instant::now() + Duration::from_secs(self.cfg.minutes as u64 * 60);
    }

    fn remind_now(&mut self) {
        // An emptied custom list falls back to the built-ins rather than showing a blank bubble.
        let list: Vec<String> = match &self.tips {
            Some(t) if t.iter().any(|s| !s.trim().is_empty()) => {
                t.iter().filter(|s| !s.trim().is_empty()).cloned().collect()
            }
            _ => default_tips(self.cfg.european),
        };
        // an edited list can leave stale positions in the bag, so refill whenever one is out of range
        if self.bag.is_empty() || self.bag.iter().any(|&i| i >= list.len()) {
            self.bag = shuffled(list.len(), &mut self.rng);
            // don't repeat the tip just shown across a reshuffle (bag pops from the end)
            if self.bag.len() > 1 && self.bag.last() == self.last_tip.as_ref() {
                self.bag.swap(0, list.len() - 1);
            }
        }
        let i = self.bag.pop().unwrap();
        self.last_tip = Some(i);
        self.show_tip(list[i].clone());
    }

    fn show_tip(&mut self, tip: String) {
        self.message = Some(tip);
        self.shown_at = Instant::now();
        self.ribbit();
    }

    fn ribbit(&mut self) {
        use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
        if self.cfg.volume == 0 {
            return;
        }
        let wav = ribbit_wav(self.cfg.volume);
        // a new PlaySound call stops the previous sound first, so the old buffer is free to drop afterwards
        unsafe {
            PlaySoundW(
                wav.as_ptr() as *const u16,
                std::ptr::null_mut(),
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            )
        };
        self.sound = wav;
    }

    /// Handles tray clicks, update results and due reminders; shared by both windows and the idle loop.
    fn poll(&mut self) {
        while let Ok(e) = tray_icon::menu::MenuEvent::receiver().try_recv() {
            if e.id == self.quit_id {
                self.quit = true;
            } else if e.id == self.settings_id {
                self.settings = true;
            }
        }
        if let Some(Ok(tag)) = self.update_rx.as_ref().map(|rx| rx.try_recv()) {
            self.update_rx = None;
            self.update = tag;
        }
        let clock = local_time();
        if clock != self.last_minute {
            self.last_minute = clock;
            let due = self.schedule.iter().rev().find(|t| {
                t.hour == clock.1 as u32 && t.minute == clock.2 as u32 && !t.text.trim().is_empty()
            });
            if let Some(t) = due.cloned() {
                self.show_tip(t.text);
            }
        }
        let now = Instant::now();
        if !self.frog_visible() && now >= self.next_at {
            self.remind_now();
        }
        if self.message.is_some()
            && self.update.is_none()
            && now - self.shown_at > Duration::from_secs(self.cfg.show_secs as u64)
        {
            self.hide();
        }
    }

    fn corner_pos(&self) -> Option<Pos2> {
        let m = self.monitor?;
        let x = if self.cfg.corner % 2 == 0 {
            MARGIN
        } else {
            m.x - W - MARGIN
        };
        let y = if self.cfg.corner < 2 {
            MARGIN
        } else {
            m.y - H - MARGIN - TASKBAR
        };
        Some(Pos2::new(x, y))
    }
}

/// One window session (the frog or the settings menu); destroyed as soon as it has nothing to show.
struct Frog {
    st: Rc<RefCell<State>>,
    settings_mode: bool,
    placed: Option<usize>,
    bubble: Vec2,
    frog: Option<[egui::TextureHandle; 2]>,
}

// 22x22 RGBA sprite cut from the frog art (assets/frog.png is the same image).
const FROG_PX: usize = 22;
// Screen pixels per art pixel. Not a whole number, so cell edges are snapped to whole pixels to stay crisp.
const CELL: f32 = 3.4;
const INK: Color32 = Color32::BLACK;
const PAPER: Color32 = Color32::from_rgb(252, 252, 240);
// Eye boxes in the sprite (columns) and the rows they span; used to draw the blink frame.
const EYES: [std::ops::RangeInclusive<usize>; 2] = [5..=8, 13..=16];
const EYE_ROWS: std::ops::RangeInclusive<usize> = 2..=5;

/// Open-eye and closed-eye frames of the frog sprite.
fn frog_frames() -> [egui::ColorImage; 2] {
    let open = include_bytes!("../assets/frog.rgba");
    let mut shut = open.to_vec();
    let px = |r: usize, c: usize| (r * FROG_PX + c) * 4;
    let lid: [u8; 4] = shut[px(6, 6)..px(6, 6) + 4].try_into().unwrap();
    for cols in EYES {
        for r in EYE_ROWS {
            for c in cols.clone() {
                let colour = if r == 4 { [0, 0, 0, 255] } else { lid };
                shut[px(r, c)..px(r, c) + 4].copy_from_slice(&colour);
            }
        }
    }
    [open.as_slice(), shut.as_slice()]
        .map(|b| egui::ColorImage::from_rgba_unmultiplied([FROG_PX, FROG_PX], b))
}

fn pixel_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "pixel".into(),
        egui::FontData::from_static(include_bytes!("../assets/Jersey10-Regular.ttf")),
    );
    for fam in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .get_mut(&fam)
            .unwrap()
            .insert(0, "pixel".into());
    }
    ctx.set_fonts(fonts);
    ctx.set_visuals(egui::Visuals::light());
    ctx.style_mut(|s| {
        s.text_styles.iter_mut().for_each(|(_, f)| f.size = 20.0);
        s.spacing.item_spacing = Vec2::new(6.0, 5.0);
        s.spacing.button_padding = Vec2::new(8.0, 1.0);
        let w = &mut s.visuals.widgets;
        for v in [
            &mut w.noninteractive,
            &mut w.inactive,
            &mut w.hovered,
            &mut w.active,
            &mut w.open,
        ] {
            v.rounding = egui::Rounding::ZERO;
            v.fg_stroke.color = INK;
        }
        w.inactive.bg_stroke = Stroke::new(2.0, INK);
        w.hovered.bg_stroke = Stroke::new(2.0, INK);
        w.active.bg_stroke = Stroke::new(2.0, INK);
        w.inactive.weak_bg_fill = Color32::from_rgb(200, 230, 120);
        w.inactive.bg_fill = Color32::from_rgb(200, 230, 120);
        w.hovered.weak_bg_fill = Color32::from_rgb(170, 215, 80);
        w.hovered.bg_fill = Color32::from_rgb(170, 215, 80);
        s.visuals.selection.bg_fill = Color32::from_rgb(110, 190, 60);
        s.visuals.selection.stroke = Stroke::new(1.0, INK);
        s.visuals.panel_fill = PAPER;
        s.visuals.override_text_color = Some(INK);
    });
}

/// Pixel-art speech bubble: notched corners and a stepped tail pointing down at `tail_x`.
fn speech_bubble(b: Rect, tail_x: f32) -> Vec<egui::Shape> {
    const P: f32 = 3.0;
    let mut out = Vec::new();
    let mut fill = |r: Rect, c: Color32| out.push(egui::Shape::rect_filled(r, 0.0, c));
    let px = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
    fill(b.shrink2(Vec2::new(P, 0.0)), PAPER);
    fill(b.shrink2(Vec2::new(0.0, P)), PAPER);
    let (l, t, r, bt) = (b.min.x, b.min.y, b.max.x, b.max.y);
    fill(px(l + P, t, r - l - 2.0 * P, P), INK);
    fill(px(l + P, bt - P, r - l - 2.0 * P, P), INK);
    fill(px(l, t + P, P, bt - t - 2.0 * P), INK);
    fill(px(r - P, t + P, P, bt - t - 2.0 * P), INK);
    for (x, y) in [
        (l + P, t + P),
        (r - 2.0 * P, t + P),
        (l + P, bt - 2.0 * P),
        (r - 2.0 * P, bt - 2.0 * P),
    ] {
        fill(px(x, y, P, P), INK);
    }
    // tail: vertical left edge, stepped right edge, overlapping the bottom border
    for i in 0..4 {
        let y = bt - P + i as f32 * P;
        let w = (4 - i) as f32 * P;
        fill(px(tail_x, y, w, P), PAPER);
        fill(px(tail_x, y, P, P), INK);
        fill(px(tail_x + w - P, y, P, P), INK);
    }
    out
}

/// Little pond with drifting wave highlights and a lily pad; returns where the frog's feet go.
fn pond(p: &egui::Painter, t: f64, bob: f32) -> Pos2 {
    let cell = |cx: f32, cy: f32, x: i32, y: i32, c: Color32| {
        let (x0, y0) = (cx + x as f32 * CELL, cy + y as f32 * CELL);
        p.rect_filled(
            Rect::from_min_max(
                Pos2::new(x0.round(), y0.round()),
                Pos2::new((x0 + CELL).round(), (y0 + CELL).round()),
            ),
            0.0,
            c,
        )
    };
    let inside =
        |x: i32, y: i32, rx: f32, ry: f32| (x as f32 / rx).powi(2) + (y as f32 / ry).powi(2) <= 1.0;
    let (cx, cy) = (W / 2.0 - CELL / 2.0, H - 26.0);
    let frame = (t * 4.0) as i32;
    let (rx, ry) = (31.0, 5.6);
    for y in -6..=6 {
        for x in -32..=32 {
            if !inside(x, y, rx, ry) {
                continue;
            }
            let edge = !inside(x - 1, y, rx, ry)
                || !inside(x + 1, y, rx, ry)
                || !inside(x, y - 1, rx, ry)
                || !inside(x, y + 1, rx, ry);
            // alternate rows drift in opposite directions
            let drift = if y % 2 == 0 { frame } else { -frame };
            let c = if edge {
                Color32::from_rgb(20, 60, 140)
            } else if y % 2 != 0 && (x + drift + y * 5).rem_euclid(9) < 2 {
                Color32::from_rgb(150, 210, 255)
            } else {
                Color32::from_rgb(40, 110, 210)
            };
            cell(cx, cy, x, y, c);
        }
    }
    let ly = cy - CELL + bob;
    let (lx, lyr) = (19.0, 3.6);
    for y in -4..=4 {
        for x in -20..=20 {
            // V-shaped notch in the front-right of the pad
            if !inside(x, y, lx, lyr) || (y > 0 && x > 1 && x < 2 + y * 3) {
                continue;
            }
            let edge = !inside(x, y + 1, lx, lyr)
                || !inside(x - 1, y, lx, lyr)
                || !inside(x + 1, y, lx, lyr)
                || !inside(x, y - 1, lx, lyr);
            let c = if edge {
                Color32::from_rgb(30, 90, 30)
            } else if (x + y) % 7 == 0 {
                Color32::from_rgb(120, 200, 80)
            } else {
                Color32::from_rgb(70, 160, 55)
            };
            cell(cx, ly, x, y, c);
        }
    }
    Pos2::new(W / 2.0, ly + CELL * 0.5)
}

/// Tip editor that wraps long text onto more lines instead of widening the window.
fn tip_box(ui: &mut egui::Ui, text: &mut String, width: f32) -> bool {
    let changed = ui
        .add(
            egui::TextEdit::multiline(text)
                .desired_rows(1)
                // desired_width is the text area only; the left/right margins below are added on top
                .desired_width(width - 16.0)
                .font(egui::FontId::proportional(20.0))
                // Jersey 10 draws high in its line box, so a little more room on top centers it
                .margin(egui::Margin {
                    left: 8.0,
                    right: 8.0,
                    top: 6.0,
                    bottom: 4.0,
                })
                .hint_text("Type a tip"),
        )
        .changed();
    if changed && text.contains('\n') {
        // Enter shouldn't split a tip; tips are saved one per line
        *text = text.replace(['\r', '\n'], " ");
    }
    changed
}

const X_W: f32 = 28.0;
// Height of a one-line tip box; buttons and time inputs in a tip row match it so the row lines up.
const ROW_H: f32 = 32.0;

fn remove_button(ui: &mut egui::Ui, hover: &str) -> bool {
    ui.add(egui::Button::new(txt("X")).min_size(Vec2::new(X_W, ROW_H)))
        .on_hover_text(hover)
        .clicked()
}

/// Pixel-font text with the extra 1px letter spacing used everywhere in the app.
fn txt(s: impl Into<String>) -> egui::RichText {
    egui::RichText::new(s).extra_letter_spacing(1.0)
}

impl Frog {
    fn settings_ui(&self, ui: &mut egui::Ui, st: &mut State) {
        ui.horizontal(|ui| {
            ui.label(txt("Frog Buddy Settings").size(30.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    txt(concat!("v", env!("CARGO_PKG_VERSION")))
                        .size(18.0)
                        .color(Color32::from_gray(110)),
                );
            });
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(ui.available_height() - 44.0)
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 14.0);
                self.settings_body(ui, st);
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(txt("Show a tip now")).clicked() {
                st.remind_now();
                st.settings = false;
            }
            if ui.button(txt("Close")).clicked() {
                st.settings = false;
            }
            if ui.button(txt("Quit")).clicked() {
                st.quit = true;
            }
        });
    }

    fn settings_body(&self, ui: &mut egui::Ui, st: &mut State) {
        let mut changed = false;
        ui.label(txt("Where should I sit?"));
        egui::Grid::new("corners").show(ui, |ui| {
            for (i, name) in CORNERS.iter().enumerate() {
                if ui
                    .selectable_label(st.cfg.corner == i, txt(*name))
                    .clicked()
                {
                    st.cfg.corner = i;
                    changed = true;
                }
                if i % 2 == 1 {
                    ui.end_row();
                }
            }
        });
        ui.add_space(6.0);
        let mut test_sound = false;
        egui::Grid::new("timing")
            .spacing(Vec2::new(8.0, 8.0))
            .show(ui, |ui| {
                ui.label(txt("Remind me every"));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut st.cfg.minutes)
                            .range(1..=240)
                            .suffix(" min"),
                    )
                    .changed();
                ui.end_row();
                ui.label(txt("Keep tips up for"));
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut st.cfg.show_secs)
                            .range(5..=300)
                            .suffix(" sec"),
                    )
                    .changed();
                ui.end_row();
                ui.label(txt("Ribbit volume"));
                ui.horizontal(|ui| {
                    changed |= ui
                        .add(egui::Slider::new(&mut st.cfg.volume, 0..=100).suffix("%"))
                        .changed();
                    test_sound = ui.button(txt("Test")).clicked();
                });
                ui.end_row();
            });
        ui.add_space(6.0);
        changed |= ui
            .checkbox(
                &mut st.cfg.european,
                txt("European units for built-in tips (ml, metres)"),
            )
            .changed();
        if changed {
            st.cfg.save();
        }
        if test_sound {
            st.ribbit();
        }

        ui.add_space(10.0);
        ui.label(txt("Tips"));
        // Built-in tips are shown until the first edit; from then on the list is saved as typed.
        let mut list = st
            .tips
            .clone()
            .unwrap_or_else(|| default_tips(st.cfg.european));
        let mut edited = false;
        let mut remove = None;
        // Row width is fixed up front; reading available_width per row let each row grow wider than the last.
        let row_w = ui.available_width() - 2.0 * 8.0;
        egui::Frame::none()
            .stroke(Stroke::new(2.0, INK))
            .inner_margin(6.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("tips")
                    .max_height(180.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.set_width(row_w);
                        for (i, tip) in list.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                edited |= tip_box(ui, tip, ui.available_width() - X_W - ui.spacing().item_spacing.x);
                                if remove_button(ui, "Remove this tip") {
                                    remove = Some(i);
                                }
                            });
                        }
                    });
            });
        if let Some(i) = remove {
            list.remove(i);
            edited = true;
        }
        ui.horizontal(|ui| {
            if ui.button(txt("Add tip")).clicked() {
                list.push(String::new());
                edited = true;
            }
            if st.tips.is_some() && ui.button(txt("Reset to built-in tips")).clicked() {
                st.tips = None;
                save_tips(None);
                edited = false;
            }
        });
        if edited {
            save_tips(Some(&list));
            st.tips = Some(list);
        }

        ui.add_space(10.0);
        ui.label(txt("Tips at a set time (your local time, every day)"));
        let mut edited = false;
        let mut remove = None;
        if !st.schedule.is_empty() {
            // same framed box and row width as the tips list above
            let row_w = ui.available_width() - 2.0 * 8.0;
            egui::Frame::none()
                .stroke(Stroke::new(2.0, INK))
                .inner_margin(6.0)
                .show(ui, |ui| {
                    // a scroll area like the tips list's, so both boxes reserve the same scrollbar gutter and match in width
                    egui::ScrollArea::vertical()
                        .id_salt("timed")
                        .max_height(180.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(row_w);
                            for (i, t) in st.schedule.iter_mut().enumerate() {
                                ui.horizontal(|ui| {
                                    let two = |n: f64, _| format!("{:02}", n as u32);
                                    edited |= ui
                                        .add_sized(
                                            [36.0, ROW_H],
                                            egui::DragValue::new(&mut t.hour)
                                                .range(0..=23)
                                                .custom_formatter(two),
                                        )
                                        .changed();
                                    ui.add_sized([6.0, ROW_H], egui::Label::new(txt(":")));
                                    edited |= ui
                                        .add_sized(
                                            [36.0, ROW_H],
                                            egui::DragValue::new(&mut t.minute)
                                                .range(0..=59)
                                                .custom_formatter(two),
                                        )
                                        .changed();
                                    edited |= tip_box(
                                        ui,
                                        &mut t.text,
                                        ui.available_width() - X_W - ui.spacing().item_spacing.x,
                                    );
                                    if remove_button(ui, "Remove this timed tip") {
                                        remove = Some(i);
                                    }
                                });
                            }
                        });
                });
        }
        if let Some(i) = remove {
            st.schedule.remove(i);
            edited = true;
        }
        if ui.button(txt("Add timed tip")).clicked() {
            st.schedule.push(Timed {
                hour: 12,
                minute: 0,
                text: String::new(),
            });
            edited = true;
        }
        if edited {
            save_schedule(&st.schedule);
        }
    }
    fn frog_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context, st: &mut State) {
        let t = ctx.input(|i| i.time);
        let bob = if (t * 0.8) as i64 % 2 == 0 { 0.0 } else { 2.0 };
        let feet = pond(ui.painter(), t, bob);

        let side = (FROG_PX as f32 * CELL).round();
        let frog_rect = Rect::from_min_size(
            Pos2::new((feet.x - side / 2.0).round(), (feet.y - side).round()),
            Vec2::splat(side),
        );
        let resp = ui.interact(frog_rect, ui.id().with("frog"), Sense::click());
        let tex = self.frog.get_or_insert_with(|| {
            let [open, shut] = frog_frames();
            [
                ctx.load_texture("frog", open, egui::TextureOptions::NEAREST),
                ctx.load_texture("frog-blink", shut, egui::TextureOptions::NEAREST),
            ]
        });
        // blink for ~150 ms every 4 s, with a double blink every third time
        let phase = t % 4.0;
        let blinking = phase < 0.15 || ((t / 4.0) as i64 % 3 == 0 && (0.3..0.45).contains(&phase));
        ui.painter().image(
            tex[blinking as usize].id(),
            frog_rect,
            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        let bottom = frog_rect.min.y - 16.0;
        if let (None, Some(m)) = (&st.update, &st.message) {
            // Plain tips: lay out once and pad around the glyphs' drawn bounds so padding is even on all sides.
            const PAD: f32 = 12.0;
            const BORDER: f32 = 3.0;
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = W - 8.0 - 2.0 * (PAD + BORDER);
            job.append(
                m,
                0.0,
                egui::TextFormat {
                    font_id: egui::FontId::proportional(22.0),
                    color: INK,
                    extra_letter_spacing: 1.0,
                    line_height: Some(17.0),
                    ..Default::default()
                },
            );
            let galley = ctx.fonts(|f| f.layout_job(job));
            let ink = galley.mesh_bounds;
            let size = ink.size() + Vec2::splat(2.0 * (PAD + BORDER));
            let bubble = Rect::from_min_size(
                Pos2::new(((W - size.x) / 2.0).round(), (bottom - size.y).round()),
                size.round(),
            );
            ui.painter().extend(speech_bubble(bubble, W / 2.0 - 18.0));
            ui.painter().galley(
                bubble.min + Vec2::splat(PAD + BORDER) - ink.min.to_vec2(),
                galley,
                INK,
            );
            if resp.clicked() {
                st.hide();
            }
            return;
        }

        // Update prompt has buttons, so it uses a normal ui sized from last frame's content (one-frame lag).
        let size = (self.bubble + Vec2::splat(24.0)).min(Vec2::new(W - 8.0, bottom));
        let bubble = Rect::from_min_size(Pos2::new((W - size.x) / 2.0, bottom - size.y), size);
        let bg = ui.painter().add(egui::Shape::Noop);
        let content =
            Rect::from_min_size(bubble.min + Vec2::splat(12.0), Vec2::new(W - 32.0, bottom));
        let used = ui
            .allocate_new_ui(egui::UiBuilder::new().max_rect(content), |ui| {
                if let Some(tag) = st.update.clone() {
                    ui.label(
                        txt(format!("Ribbit! Frog Buddy {tag} is out. Update now?"))
                            .size(22.0)
                            .line_height(Some(17.0)),
                    );
                    ui.horizontal(|ui| {
                        if ui.button(txt("Yes")).clicked() {
                            match start_installer() {
                                Ok(()) => st.quit = true,
                                Err(e) => {
                                    st.update = None;
                                    st.message = Some(e);
                                    st.shown_at = Instant::now();
                                }
                            }
                        }
                        if ui.button(txt("No")).clicked() {
                            st.update = None;
                            st.hide();
                        }
                    });
                } else if let Some(m) = &st.message {
                    ui.label(egui::RichText::new(m).size(22.0).line_height(Some(17.0)));
                }
                ui.min_rect().size()
            })
            .inner;
        ui.painter()
            .set(bg, egui::Shape::Vec(speech_bubble(bubble, W / 2.0 - 18.0)));
        if (used - self.bubble).length() > 0.5 {
            self.bubble = used;
            ctx.request_repaint();
        }

        if st.update.is_none() && resp.clicked() {
            st.hide();
        }
    }
}

impl eframe::App for Frog {
    fn clear_color(&self, v: &egui::Visuals) -> [f32; 4] {
        if self.settings_mode {
            v.panel_fill.to_normalized_gamma_f32()
        } else {
            [0.0; 4]
        }
    }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let rc = self.st.clone();
        let mut st = rc.borrow_mut();
        st.poll();
        // Each window only lives while its mode is wanted; main() opens the next one.
        let wanted = if self.settings_mode {
            st.settings
        } else {
            st.frog_visible() && !st.settings
        };
        if st.quit || !wanted {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if self.settings_mode {
            ctx.request_repaint_after(Duration::from_millis(500));
            egui::CentralPanel::default()
                .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(16.0))
                .show(ctx, |ui| self.settings_ui(ui, &mut st));
            return;
        }

        if let Some(m) = ctx.input(|i| i.viewport().monitor_size) {
            st.monitor = Some(m);
        }
        if self.placed != Some(st.cfg.corner) {
            if let Some(p) = st.corner_pos() {
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(p));
                self.placed = Some(st.cfg.corner);
            }
        }
        ctx.request_repaint_after(Duration::from_millis(50));
        egui::CentralPanel::default()
            .frame(egui::Frame::none())
            .show(ctx, |ui| self.frog_ui(ui, ctx, &mut st));
    }
}

/// Sleeps until there is something to show, pumping Win32 messages so the tray keeps working.
fn idle(st: &RefCell<State>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    loop {
        unsafe {
            let mut msg = std::mem::zeroed();
            while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let mut s = st.borrow_mut();
        s.poll();
        if s.quit || s.settings || s.frog_visible() {
            return;
        }
        drop(s);
        unsafe { MsgWaitForMultipleObjects(0, std::ptr::null(), 0, 1000, QS_ALLINPUT) };
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

    use tray_icon::menu::{Menu, MenuItem};
    let settings = MenuItem::new("Settings", true, None);
    let quit = MenuItem::new("Quit", true, None);
    let menu = Menu::new();
    let _ = menu.append_items(&[&settings, &quit]);
    let _tray = tray_icon::TrayIconBuilder::new()
        .with_tooltip("Frog Buddy")
        .with_menu(Box::new(menu))
        // id 1 is the exe icon embedded by winresource in build.rs
        .with_icon(tray_icon::Icon::from_resource(1, None).expect("embedded icon"))
        .build()
        .expect("tray icon");

    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let _ = tx.send(check_latest());
    });
    let cfg = Config::load();
    let st = Rc::new(RefCell::new(State {
        settings_id: settings.id().clone(),
        quit_id: quit.id().clone(),
        update_rx: Some(rx),
        update: None,
        cfg,
        message: Some("Hi! I'm your frog buddy. Right-click my tray icon for settings.".into()),
        shown_at: Instant::now(),
        next_at: Instant::now() + Duration::from_secs(cfg.minutes as u64 * 60),
        settings: false,
        bag: Vec::new(),
        last_tip: None,
        // seed from the clock so each launch starts somewhere different; `| 1` keeps xorshift off zero
        rng: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64) | 1,        quit: false,
        monitor: None,
        tips: load_tips(),
        schedule: load_schedule(),
        // start on the current minute so a launch doesn't replay a tip already due this minute
        last_minute: local_time(),
        sound: Vec::new(),
    }));

    loop {
        let settings_mode = st.borrow().settings;
        let vp = if settings_mode {
            egui::ViewportBuilder::default()
                .with_title("Frog Buddy Settings")
                .with_inner_size([520.0, 640.0])
                .with_icon(egui::IconData {
                    rgba: include_bytes!("../assets/frog.rgba").to_vec(),
                    width: FROG_PX as u32,
                    height: FROG_PX as u32,
                })
                .with_resizable(false)
                .with_maximize_button(false)
        } else {
            let mut vp = egui::ViewportBuilder::default()
                .with_title("Frog Buddy")
                .with_inner_size([W, H])
                .with_decorations(false)
                .with_transparent(true)
                .with_always_on_top()
                .with_taskbar(false)
                .with_resizable(false);
            if let Some(p) = st.borrow().corner_pos() {
                vp = vp.with_position(p);
            }
            vp
        };
        let opts = eframe::NativeOptions {
            viewport: vp,
            centered: settings_mode,
            ..Default::default()
        };
        let app_st = st.clone();
        eframe::run_native(
            "Frog Buddy",
            opts,
            Box::new(move |cc| {
                pixel_style(&cc.egui_ctx);
                let placed = app_st.borrow().monitor.map(|_| app_st.borrow().cfg.corner);
                Ok(Box::new(Frog {
                    st: app_st,
                    settings_mode,
                    placed,
                    bubble: Vec2::new(W - 32.0, 40.0),
                    frog: None,
                }))
            }),
        )?;
        let mut s = st.borrow_mut();
        if s.quit {
            return Ok(());
        }
        if settings_mode {
            // closed with the title-bar X
            s.settings = false;
        }
        drop(s);
        // Window and GL context are gone; hand the freed pages back to Windows.
        unsafe {
            use windows_sys::Win32::System::Threading::{
                GetCurrentProcess, SetProcessWorkingSetSize,
            };
            SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
        }
        idle(&st);
        if st.borrow().quit {
            return Ok(());
        }
    }
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
    fn shuffle_covers_every_tip_once() {
        let mut rng = 12345;
        for n in [0, 1, 2, 10] {
            let mut v = shuffled(n, &mut rng);
            v.sort();
            assert_eq!(v, (0..n).collect::<Vec<_>>());
        }
        assert_ne!(shuffled(10, &mut rng), shuffled(10, &mut rng));
    }

    #[test]
    fn timed_parse() {
        assert_eq!(
            Timed::parse("09:30 Drink water"),
            Some(Timed {
                hour: 9,
                minute: 30,
                text: "Drink water".into()
            })
        );
        assert_eq!(Timed::parse("24:00 nope"), None);
        assert_eq!(Timed::parse("garbage"), None);
    }

    #[test]
    fn ribbit_is_valid_wav() {
        let w = ribbit_wav(50);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(
            u32::from_le_bytes(w[4..8].try_into().unwrap()) as usize,
            w.len() - 8
        );
        assert_eq!(
            u32::from_le_bytes(w[40..44].try_into().unwrap()) as usize,
            w.len() - 44
        );
        assert!(ribbit_wav(0)[44..].iter().all(|&b| b == 0));
    }

    #[test]
    fn config_parse() {
        assert_eq!(
            Config::parse("corner=1\nminutes=15\nunits=european\n"),
            Config {
                corner: 1,
                minutes: 15,
                european: true,
                show_secs: 20,
                volume: 0
            }
        );
        assert_eq!(
            Config::parse("corner=9\nminutes=0\njunk"),
            Config {
                corner: 3,
                minutes: 1,
                european: false,
                show_secs: 20,
                volume: 0
            }
        );
        assert_eq!(Config::parse(""), Config::default());
    }
}
