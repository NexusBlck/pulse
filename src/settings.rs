//! The Settings window: every setting as a click (switches, pickers,
//! steppers, colour swatches) in a window of its own. Changes apply at once
//! and are written to config.json.
//!
//! It draws only when something in it changes (opening, a click, the pointer
//! moving onto another control), never on the frame clock, and closing it
//! drops the window, its renderer and its pixels.

use crate::config::{Config, Idle, PageKind, Style, ThemeName};
use crate::gfx::{Align, Color, Gfx, Rect};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, ScreenToClient, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const CLASS: windows::core::PCWSTR = w!("Synapse.Settings");

/// The card (DIPs), and the room around it for its shadow.
const W: f32 = 780.0;
const H: f32 = 560.0;
const M: f32 = 24.0;
const SIDEBAR: f32 = 200.0;
const ROW_H: f32 = 56.0;

const BG: Color = Color::rgba(0.11, 0.11, 0.12, 1.0);
const SIDE_BG: Color = Color::rgba(0.085, 0.085, 0.095, 1.0);
const TEXT: Color = Color::rgba(0.96, 0.96, 0.97, 1.0);
const DIM: Color = Color::rgba(0.96, 0.96, 0.97, 0.58);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    General,
    Appearance,
    Ambient,
    Island,
    Music,
    Popups,
    Focus,
    Tabs,
    Addons,
    Prayer,
    Caffeine,
    About,
}

impl Section {
    const ALL: [Section; 12] = [
        Section::General,
        Section::Appearance,
        Section::Ambient,
        Section::Island,
        Section::Music,
        Section::Popups,
        Section::Focus,
        Section::Tabs,
        Section::Addons,
        Section::Prayer,
        Section::Caffeine,
        Section::About,
    ];

    fn label(self) -> &'static str {
        match self {
            Section::General => "General",
            Section::Appearance => "Appearance",
            Section::Ambient => "Ambient light",
            Section::Island => "Island",
            Section::Music => "Music & lyrics",
            Section::Popups => "Pop-ups",
            Section::Focus => "Focus timer",
            Section::Tabs => "Tabs",
            Section::Addons => "Add-ons",
            Section::Prayer => "Prayer times",
            Section::Caffeine => "Caffeine",
            Section::About => "About",
        }
    }

    fn glyph(self) -> char {
        match self {
            Section::General => '\u{E713}',
            Section::Appearance => '\u{E790}',
            Section::Ambient => '\u{E706}',
            Section::Island => '\u{E7F4}',
            Section::Music => '\u{E8D6}',
            Section::Popups => '\u{E7E7}',
            Section::Focus => '\u{E916}',
            Section::Tabs => '\u{E8A9}',
            Section::Addons => '\u{E71D}',
            Section::Prayer => '\u{E708}',
            Section::Caffeine => '\u{E945}',
            Section::About => '\u{E946}',
        }
    }
}

/// Which setting a control changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Autostart,
    OpenOnHover,
    HoverDelay,
    CollapseDelay,
    Hotkey,
    HideFullscreen,
    Theme,
    Style,
    ArtAccent,
    Accent,
    Font,
    Speed,
    Bounce,
    Idle,
    Clock24,
    Seconds,
    Width,
    Offset,
    ShowMedia,
    Lyrics,
    LyricsPill,
    LyricsOffset,
    VolumeHud,
    PowerHud,
    BluetoothHud,
    HudSeconds,
    FocusMinutes,
    BreakMinutes,
    LongMinutes,
    LongEvery,
    AutoBreaks,
    AutoFocus,
    FocusSound,
    AmbientOn,
    AmbientSource,
    EdgeGlow,
    TintIsland,
    Breathe,
    Intensity,
    OpenRgb,
    AddonSystem,
    AddonCaffeine,
    AddonPrayer,
    AddonDownloader,
    AddonScript,
    ScriptHide,
    PrayerMethod,
    PrayerAsr,
    PrayerRemind,
    PrayerSound,
    Bedtime,
    HalfLife,
    BedLimit,
    /// Ask Windows where the PC is again.
    Relocate,
    Page(PageKind),
    /// Buttons: things to open rather than settings.
    OpenConfig,
    OpenLogs,
    OpenInbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Val {
    Toggle,
    Pick(usize),
    Step(i32),
}

enum Control {
    Switch(bool),
    Pick(Vec<String>, usize),
    Step(String),
    Swatches(usize),
    Button(&'static str),
}

struct Row {
    label: &'static str,
    desc: String,
    key: Key,
    control: Control,
}

fn row(label: &'static str, desc: &str, key: Key, control: Control) -> Row {
    Row { label, desc: desc.to_string(), key, control }
}

pub const IDLES: [Idle; 4] = [Idle::Clock, Idle::Eyes, Idle::Dot, Idle::Hidden];
pub const IDLE_NAMES: [&str; 4] = ["Clock", "Eyes", "Dot", "Hidden"];

pub fn idle_index(i: Idle) -> usize {
    IDLES.iter().position(|x| *x == i).unwrap_or(0)
}

const HOTKEYS: [&str; 4] = ["Off", "Ctrl+Alt+P", "Win+Shift+Space", "Ctrl+Shift+Space"];
const FONTS: [&str; 4] = ["Segoe UI Variable Text", "Segoe UI", "Bahnschrift", "Cascadia Code"];
pub const ACCENTS: [&str; 8] = ["#30D158", "#0A84FF", "#64D2FF", "#BF5AF2", "#FF375F", "#FF9F0A", "#FFD60A", "#F5F5F7"];
const PAGES: [PageKind; 6] = [PageKind::Media, PageKind::Shelf, PageKind::Tasks, PageKind::Focus, PageKind::Clock, PageKind::Commands];

/// The options of a picker whose current value may not be one of them:
/// it joins the list, so it shows as picked.
fn options_with(list: &[&str], current: &str) -> (Vec<String>, usize) {
    let mut v: Vec<String> = list.iter().map(|s| s.to_string()).collect();
    let cur = if current.trim().is_empty() { "Off" } else { current };
    let i = v.iter().position(|o| o.eq_ignore_ascii_case(cur)).unwrap_or_else(|| {
        v.push(cur.to_string());
        v.len() - 1
    });
    (v, i)
}

fn rows(s: Section, c: &Config) -> Vec<Row> {
    let pick = |opts: &[&str], i: usize| Control::Pick(opts.iter().map(|s| s.to_string()).collect(), i);
    match s {
        Section::General => vec![
            row("Start with Windows", "Synapse opens when you sign in", Key::Autostart, Control::Switch(c.start_with_windows)),
            row("Open on hover", "Otherwise click the island to open it", Key::OpenOnHover, Control::Switch(c.open_on_hover)),
            row("Hover wait", "How long the pointer rests before it opens", Key::HoverDelay, Control::Step(format!("{} ms", c.hover_delay_ms))),
            row("Close delay", "Grace after the pointer leaves", Key::CollapseDelay, Control::Step(format!("{} ms", c.collapse_delay_ms))),
            {
                let (o, i) = options_with(&HOTKEYS, &c.hotkey);
                row("Hotkey", "Opens and pins the island", Key::Hotkey, Control::Pick(o, i))
            },
            row("Hide over full-screen apps", "Games and videos stay clear", Key::HideFullscreen, Control::Switch(c.hide_in_fullscreen)),
        ],
        Section::Appearance => vec![
            row("Theme", "Shape, colours and finish", Key::Theme, Control::Pick(ThemeName::ALL.iter().map(|t| t.label().to_string()).collect(), ThemeName::ALL.iter().position(|t| *t == c.theme).unwrap_or(0))),
            row("Shape", "Hang from the top edge, or float below it", Key::Style, pick(&["Notch", "Pill"], (c.style == Style::Pill) as usize)),
            row("Accent colour", "Progress bars, the equalizer, highlights", Key::Accent, Control::Swatches(ACCENTS.iter().position(|a| a.eq_ignore_ascii_case(&c.colors.accent)).unwrap_or(usize::MAX))),
            row("Accent from album art", "While music plays", Key::ArtAccent, Control::Switch(c.colors.accent_from_art)),
            {
                let (o, i) = options_with(&FONTS, &c.font);
                row("Font", "", Key::Font, Control::Pick(o, i))
            },
            row("Motion speed", "How fast the island moves", Key::Speed, Control::Step(format!("{:.1}\u{00D7}", c.motion.speed))),
            row("Bounce", "0 is no overshoot", Key::Bounce, Control::Step(format!("{:.2}", c.motion.bounce))),
        ],
        Section::Ambient => vec![
            row("Ambient light", "Colour and light from what you're seeing", Key::AmbientOn, Control::Switch(c.ambient.enabled)),
            row("Colours from", "Auto: the album art while music plays, else the screen", Key::AmbientSource, pick(&["Auto", "Album art", "Screen"], match c.ambient.source {
                crate::config::AmbientSource::Auto => 0,
                crate::config::AmbientSource::Art => 1,
                crate::config::AmbientSource::Screen => 2,
            })),
            row("Glow on the screen edge", "Light spilling out along the top", Key::EdgeGlow, Control::Switch(c.ambient.edge_glow)),
            row("Tint the island", "A wash of colour and a halo", Key::TintIsland, Control::Switch(c.ambient.tint_island)),
            row("Breathe with music", "The glow breathes with the sound", Key::Breathe, Control::Switch(c.ambient.breathe)),
            row("Brightness", "", Key::Intensity, Control::Step(format!("{:.0}%", c.ambient.intensity * 100.0))),
            row("RGB keyboard (OpenRGB)", "Needs OpenRGB with its SDK server started", Key::OpenRgb, Control::Switch(c.ambient.openrgb)),
        ],
        Section::Island => vec![
            row("At rest", "What the closed island shows", Key::Idle, pick(&IDLE_NAMES, idle_index(c.idle))),
            row("24-hour clock", "", Key::Clock24, Control::Switch(c.clock_24h)),
            row("Show seconds", "", Key::Seconds, Control::Switch(c.show_seconds)),
            row("Open width", "", Key::Width, Control::Step(format!("{:.0}", c.width))),
            row("Position", "Left or right of the screen's centre", Key::Offset, Control::Step(format!("{:+.0}", c.offset_x))),
        ],
        Section::Music => vec![
            row("Music pill", "Album art and equalizer while music plays", Key::ShowMedia, Control::Switch(c.show_media)),
            row("Synced lyrics", "From lrclib.net, on the Now Playing page", Key::Lyrics, Control::Switch(c.lyrics)),
            row("Sing in the pill", "The current line on the closed island", Key::LyricsPill, Control::Switch(c.lyrics_in_pill)),
            row("Lyrics timing", "Positive shows lines later", Key::LyricsOffset, Control::Step(format!("{:+} ms", c.lyrics_offset_ms))),
        ],
        Section::Popups => vec![
            row("Volume", "When you change the volume", Key::VolumeHud, Control::Switch(c.volume_hud)),
            row("Battery", "Charging, and warnings at 20% and 10%", Key::PowerHud, Control::Switch(c.power_hud)),
            row("Bluetooth", "Devices connecting and disconnecting", Key::BluetoothHud, Control::Switch(c.bluetooth_hud)),
            row("How long they show", "", Key::HudSeconds, Control::Step(format!("{:.1} s", c.hud_seconds))),
        ],
        Section::Focus => vec![
            row("Focus", "", Key::FocusMinutes, Control::Step(format!("{:.0} min", c.focus.focus_minutes))),
            row("Break", "", Key::BreakMinutes, Control::Step(format!("{:.0} min", c.focus.break_minutes))),
            row("Long break", "", Key::LongMinutes, Control::Step(format!("{:.0} min", c.focus.long_break_minutes))),
            row("Long break after", "Focus sessions", Key::LongEvery, Control::Step(format!("{}", c.focus.long_break_every))),
            row("Start breaks by themselves", "", Key::AutoBreaks, Control::Switch(c.focus.auto_start_breaks)),
            row("Start focus by itself", "", Key::AutoFocus, Control::Switch(c.focus.auto_start_focus)),
            row("Chime", "When a phase ends", Key::FocusSound, Control::Switch(c.focus.sound)),
        ],
        Section::Tabs => PAGES.iter().map(|&p| row(p.label(), "", Key::Page(p), Control::Switch(c.pages.contains(&p)))).collect(),
        Section::Addons => vec![
            row("System", "CPU, GPU, memory, network and the busiest apps", Key::AddonSystem, Control::Switch(c.addons.system)),
            row("Caffeine", "Log drinks; see what's still in you at bedtime", Key::AddonCaffeine, Control::Switch(c.addons.caffeine)),
            row("Prayer times", "Today's times where you are, with a countdown", Key::AddonPrayer, Control::Switch(c.addons.prayer)),
            row("Downloader", "Video or sound from a link (fetches yt-dlp, ~18 MB)", Key::AddonDownloader, Control::Switch(c.addons.downloader)),
            row("Script reader", "A teleprompter under your webcam", Key::AddonScript, Control::Switch(c.addons.script)),
            row("Script: hide from screen shares", "Calls and recordings leave the island out", Key::ScriptHide, Control::Switch(c.script.hide_from_capture)),
        ],
        Section::Prayer => {
            let method = if c.prayer.method == "auto" { "Auto (by country)".to_string() } else { crate::prayer::method(&c.prayer.method).name.to_string() };
            let place = if c.prayer.place.is_empty() { "Not found yet".to_string() } else { c.prayer.place.clone() };
            vec![
                row("Calculation method", "Auto follows your country's custom", Key::PrayerMethod, Control::Step(method)),
                row("Asr", "Hanafi: when a shadow is twice its object", Key::PrayerAsr, pick(&["Auto", "Standard", "Hanafi"], match c.prayer.asr.as_str() {
                    "standard" => 1,
                    "hanafi" => 2,
                    _ => 0,
                })),
                row("Countdown on the island", "Minutes before each prayer (0 = off)", Key::PrayerRemind, Control::Step(format!("{} min", c.prayer.remind_min))),
                row("Sound at prayer time", "", Key::PrayerSound, Control::Switch(c.prayer.sound)),
                row("Location", &place, Key::Relocate, Control::Button("Find again")),
            ]
        }
        Section::Caffeine => vec![
            row("Bedtime", "Last call keeps bedtime under your limit", Key::Bedtime, Control::Step({
                let m = c.caffeine.bedtime_min % 1440;
                format!("{:02}:{:02}", m / 60, m % 60)
            })),
            row("Half-life", "How fast your body clears it (5 h is typical)", Key::HalfLife, Control::Step(format!("{:.1} h", c.caffeine.half_life_h))),
            row("Fine at bedtime", "", Key::BedLimit, Control::Step(format!("{:.0} mg", c.caffeine.bedtime_limit_mg))),
        ],
        Section::About => vec![
            row("Config file", "Everything here, and your commands, as JSON", Key::OpenConfig, Control::Button("Open")),
            row("Phone drop folder", "Downloads\\Synapse", Key::OpenInbox, Control::Button("Open")),
            row("Logs", "For when something misbehaves", Key::OpenLogs, Control::Button("Open")),
        ],
    }
}

/// Change `c` as the control for `k` asked. Returns whether anything changed.
pub fn change(c: &mut Config, k: Key, v: Val) -> bool {
    let before = c.clone();
    let step = |v: Val| if let Val::Step(d) = v { d } else { 0 };
    fn add_f(x: &mut f32, d: i32, by: f32, lo: f32, hi: f32) {
        *x = (((*x + d as f32 * by) / by).round() * by).clamp(lo, hi);
    }
    fn add_u(x: &mut u32, d: i32, by: i32, lo: i32, hi: i32) {
        *x = (*x as i32 + d * by).clamp(lo, hi) as u32;
    }
    let d = step(v);
    let picked = if let Val::Pick(i) = v { Some(i) } else { None };
    match k {
        Key::Autostart => c.start_with_windows ^= true,
        Key::OpenOnHover => c.open_on_hover ^= true,
        Key::HoverDelay => add_u(&mut c.hover_delay_ms, d, 50, 0, 1500),
        Key::CollapseDelay => add_u(&mut c.collapse_delay_ms, d, 100, 0, 3000),
        Key::Hotkey => {
            if let Some(i) = picked {
                let (opts, _) = options_with(&HOTKEYS, &c.hotkey);
                if let Some(o) = opts.get(i) {
                    c.hotkey = if o == "Off" { String::new() } else { o.clone() };
                }
            }
        }
        Key::HideFullscreen => c.hide_in_fullscreen ^= true,
        Key::Theme => {
            if let Some(t) = picked.and_then(|i| ThemeName::ALL.get(i).copied()) {
                c.theme = t;
                (c.style, c.colors) = t.preset();
            }
        }
        Key::Style => {
            if let Some(i) = picked {
                c.style = if i == 1 { Style::Pill } else { Style::Notch };
            }
        }
        Key::Accent => {
            if let Some(a) = picked.and_then(|i| ACCENTS.get(i)) {
                c.colors.accent = a.to_string();
            }
        }
        Key::ArtAccent => c.colors.accent_from_art ^= true,
        Key::Font => {
            if let Some(i) = picked {
                let (opts, _) = options_with(&FONTS, &c.font);
                if let Some(o) = opts.get(i) {
                    c.font = o.clone();
                }
            }
        }
        Key::Speed => add_f(&mut c.motion.speed, d, 0.1, 0.3, 3.0),
        Key::Bounce => add_f(&mut c.motion.bounce, d, 0.05, 0.0, 0.6),
        Key::Idle => {
            if let Some(i) = picked {
                c.idle = IDLES[i.min(IDLES.len() - 1)];
            }
        }
        Key::Clock24 => c.clock_24h ^= true,
        Key::Seconds => c.show_seconds ^= true,
        Key::Width => add_f(&mut c.width, d, 20.0, 400.0, 900.0),
        Key::Offset => add_f(&mut c.offset_x, d, 20.0, -1200.0, 1200.0),
        Key::ShowMedia => c.show_media ^= true,
        Key::Lyrics => c.lyrics ^= true,
        Key::LyricsPill => c.lyrics_in_pill ^= true,
        Key::LyricsOffset => c.lyrics_offset_ms = (c.lyrics_offset_ms + d * 100).clamp(-10_000, 10_000),
        Key::VolumeHud => c.volume_hud ^= true,
        Key::PowerHud => c.power_hud ^= true,
        Key::BluetoothHud => c.bluetooth_hud ^= true,
        Key::HudSeconds => add_f(&mut c.hud_seconds, d, 0.5, 0.5, 10.0),
        Key::FocusMinutes => add_f(&mut c.focus.focus_minutes, d, 5.0, 5.0, 240.0),
        Key::BreakMinutes => add_f(&mut c.focus.break_minutes, d, 1.0, 1.0, 120.0),
        Key::LongMinutes => add_f(&mut c.focus.long_break_minutes, d, 5.0, 5.0, 120.0),
        Key::LongEvery => add_u(&mut c.focus.long_break_every, d, 1, 1, 12),
        Key::AutoBreaks => c.focus.auto_start_breaks ^= true,
        Key::AutoFocus => c.focus.auto_start_focus ^= true,
        Key::FocusSound => c.focus.sound ^= true,
        Key::AmbientOn => c.ambient.enabled ^= true,
        Key::AmbientSource => {
            use crate::config::AmbientSource as S;
            if let Some(i) = picked {
                c.ambient.source = [S::Auto, S::Art, S::Screen][i.min(2)];
            }
        }
        Key::EdgeGlow => c.ambient.edge_glow ^= true,
        Key::TintIsland => c.ambient.tint_island ^= true,
        Key::Breathe => c.ambient.breathe ^= true,
        Key::Intensity => add_f(&mut c.ambient.intensity, d, 0.1, 0.1, 1.0),
        Key::OpenRgb => c.ambient.openrgb ^= true,
        Key::AddonSystem => c.addons.system ^= true,
        Key::AddonCaffeine => c.addons.caffeine ^= true,
        Key::AddonPrayer => c.addons.prayer ^= true,
        Key::AddonDownloader => c.addons.downloader ^= true,
        Key::AddonScript => c.addons.script ^= true,
        Key::ScriptHide => c.script.hide_from_capture ^= true,
        Key::PrayerMethod => {
            // Auto, then every method, round and round.
            let keys: Vec<&str> = std::iter::once("auto").chain(crate::prayer::METHODS.iter().map(|m| m.key)).collect();
            let i = keys.iter().position(|k| *k == c.prayer.method).unwrap_or(0) as i32;
            let n = keys.len() as i32;
            c.prayer.method = keys[(i + d).rem_euclid(n) as usize].to_string();
        }
        Key::PrayerAsr => {
            if let Some(i) = picked {
                c.prayer.asr = ["auto", "standard", "hanafi"][i.min(2)].to_string();
            }
        }
        Key::PrayerRemind => add_u(&mut c.prayer.remind_min, d, 5, 0, 60),
        Key::PrayerSound => c.prayer.sound ^= true,
        Key::Bedtime => add_u(&mut c.caffeine.bedtime_min, d, 30, 18 * 60, 28 * 60),
        Key::HalfLife => add_f(&mut c.caffeine.half_life_h, d, 0.5, 2.0, 10.0),
        Key::BedLimit => add_f(&mut c.caffeine.bedtime_limit_mg, d, 5.0, 0.0, 300.0),
        Key::Relocate => {}
        Key::Page(p) => {
            let on = !c.pages.contains(&p);
            let mut kept: Vec<PageKind> = c.pages.iter().copied().filter(|q| *q != p).collect();
            if on {
                // Back in its usual place among the others.
                let rank = |q: &PageKind| PAGES.iter().position(|x| x == q).unwrap_or(99);
                kept.push(p);
                kept.sort_by_key(rank);
            }
            // At least one tab stays.
            if !kept.is_empty() {
                c.pages = kept;
            }
        }
        Key::OpenConfig | Key::OpenLogs | Key::OpenInbox => {}
    }
    *c != before
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Hit {
    Section(Section),
    Close,
    Set(Key, Val),
}

/// What a message to the window asks the app to do.
pub enum Outcome {
    Nothing,
    Change(Key, Val),
    Open(Key),
    Close,
}

pub struct SettingsWindow {
    pub hwnd: HWND,
    gfx: Gfx,
    section: Section,
    hits: Vec<(Rect, Hit)>,
    mouse: Option<(f32, f32)>,
    hovered: Option<usize>,
    pressed: Option<Hit>,
}

pub fn register(proc_: WNDPROC) {
    unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else { return };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: proc_,
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
    }
}

impl SettingsWindow {
    pub fn open(scale: f32, font: &str, icon: HICON) -> Option<Self> {
        let (w, h) = (((W + 2.0 * M) * scale).ceil() as i32, ((H + 2.0 * M) * scale).ceil() as i32);
        unsafe {
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let wa = mi.rcWork;
            let (x, y) = (wa.left + (wa.right - wa.left - w) / 2, wa.top + (wa.bottom - wa.top - h) / 2);
            let hinst = GetModuleHandleW(None).ok()?;
            let hwnd = CreateWindowExW(WS_EX_LAYERED | WS_EX_APPWINDOW, CLASS, w!("Synapse settings"), WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX, x, y, w, h, None, None, hinst, None).ok()?;
            let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(icon.0 as isize));
            let _ = SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(icon.0 as isize));
            let mut gfx = Gfx::new(font).ok()?;
            gfx.resize(w, h, scale).ok()?;
            let s = Self { hwnd, gfx, section: Section::General, hits: Vec::new(), mouse: None, hovered: None, pressed: None };
            Some(s)
        }
    }

    pub fn show(&mut self, cfg: &Config, accent: Color) {
        self.paint(cfg, accent);
        unsafe {
            let _ = ShowWindow(self.hwnd, SW_SHOW);
            let _ = SetForegroundWindow(self.hwnd);
        }
    }

    pub fn close(self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }

    fn dip(&self, lp: LPARAM) -> (f32, f32) {
        let x = (lp.0 & 0xffff) as u16 as i16 as f32;
        let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as f32;
        (x / self.gfx.scale, y / self.gfx.scale)
    }

    fn hit_index(&self, p: (f32, f32)) -> Option<usize> {
        self.hits.iter().rposition(|(r, _)| r.contains(p.0, p.1))
    }

    pub fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM, cfg: &Config, accent: Color) -> Option<(isize, Outcome)> {
        let done = |o| Some((0, o));
        match msg {
            WM_NCHITTEST => {
                // Empty parts of the card move the window; controls are clicks.
                let mut pt = POINT { x: (lp.0 & 0xffff) as u16 as i16 as i32, y: ((lp.0 >> 16) & 0xffff) as u16 as i16 as i32 };
                unsafe {
                    let _ = ScreenToClient(self.hwnd, &mut pt);
                }
                let p = (pt.x as f32 / self.gfx.scale, pt.y as f32 / self.gfx.scale);
                let over = self.hit_index(p).is_some();
                Some((if over { HTCLIENT as isize } else { HTCAPTION as isize }, Outcome::Nothing))
            }
            WM_MOUSEMOVE => {
                let p = self.dip(lp);
                if self.mouse.is_none() {
                    unsafe {
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut tme);
                    }
                }
                self.mouse = Some(p);
                // Repaint only when the pointer moves onto another control.
                let h = self.hit_index(p);
                if h != self.hovered {
                    self.hovered = h;
                    self.paint(cfg, accent);
                }
                done(Outcome::Nothing)
            }
            0x02A3 /* WM_MOUSELEAVE */ => {
                self.mouse = None;
                if self.hovered.take().is_some() {
                    self.paint(cfg, accent);
                }
                done(Outcome::Nothing)
            }
            WM_LBUTTONDOWN => {
                self.pressed = self.hit_index(self.dip(lp)).map(|i| self.hits[i].1);
                done(Outcome::Nothing)
            }
            WM_LBUTTONUP => {
                let at = self.hit_index(self.dip(lp)).map(|i| self.hits[i].1);
                let pressed = self.pressed.take();
                if at.is_none() || at != pressed {
                    return done(Outcome::Nothing);
                }
                match at.unwrap() {
                    Hit::Section(s) => {
                        self.section = s;
                        self.paint(cfg, accent);
                        done(Outcome::Nothing)
                    }
                    Hit::Close => done(Outcome::Close),
                    Hit::Set(k @ (Key::OpenConfig | Key::OpenLogs | Key::OpenInbox | Key::Relocate), _) => done(Outcome::Open(k)),
                    Hit::Set(k, v) => done(Outcome::Change(k, v)),
                }
            }
            WM_KEYDOWN if wp.0 == 0x1B => done(Outcome::Close),
            WM_CLOSE => done(Outcome::Close),
            WM_MOUSEACTIVATE => Some((MA_ACTIVATE as isize, Outcome::Nothing)),
            _ => None,
        }
    }

    /// Draw the whole window and put it on screen where it is.
    pub fn paint(&mut self, cfg: &Config, accent: Color) {
        if self.gfx.begin().is_err() {
            return;
        }
        self.hits.clear();
        let card = Rect::new(M, M, W, H);
        let g = &mut self.gfx;
        for i in 1..=6 {
            let s = i as f32 * 3.0;
            g.fill_rrect(Rect::new(card.x - s, card.y - s + 6.0, card.w + 2.0 * s, card.h + 2.0 * s), 18.0 + s, Color::black(0.035));
        }
        g.fill_rrect(card, 18.0, BG);
        // Sidebar.
        let side = Rect::new(card.x, card.y, SIDEBAR, card.h);
        if let Some(geo) = g.rrect_geo(card, 18.0) {
            g.push_mask(&geo);
            g.fill_rect(side, SIDE_BG);
            g.pop();
        }
        if let Some(geo) = g.rrect_geo(card, 18.0) {
            g.stroke_geo(&geo, Color::white(0.08), 1.0);
        }
        let logo = Rect::new(side.x + 22.0, side.y + 26.0, 34.0, 14.0);
        g.fill_rrect(logo, 7.0, Color::black(1.0));
        g.fill_circle(logo.right() - 6.0, logo.cy(), 2.5, accent);
        g.text("Synapse", Rect::new(logo.right() + 10.0, side.y + 18.0, 120.0, 30.0), 18.0, 700, TEXT, Align::Left);
        let mut y = side.y + 66.0;
        for s in Section::ALL {
            let r = Rect::new(side.x + 10.0, y, SIDEBAR - 20.0, 34.0);
            let on = s == self.section;
            let hot = self.mouse.is_some_and(|(mx, my)| r.contains(mx, my));
            if on {
                self.gfx.fill_rrect(r, 9.0, Color::white(0.09));
            } else if hot {
                self.gfx.fill_rrect(r, 9.0, Color::white(0.05));
            }
            self.gfx.icon(s.glyph(), Rect::new(r.x + 8.0, r.y, 24.0, r.h), 13.0, if on { accent } else { DIM });
            self.gfx.text(s.label(), Rect::new(r.x + 38.0, r.y, r.w - 44.0, r.h), 13.0, 600, if on { TEXT } else { DIM }, Align::Left);
            self.hits.push((r, Hit::Section(s)));
            y += 38.0;
        }

        // Content.
        let cx = card.x + SIDEBAR + 34.0;
        let cw = card.right() - cx - 30.0;
        self.gfx.text(self.section.label(), Rect::new(cx, card.y + 22.0, cw, 34.0), 22.0, 700, TEXT, Align::Left);
        let close = Rect::new(card.right() - 46.0, card.y + 18.0, 30.0, 30.0);
        let hot = self.mouse.is_some_and(|(mx, my)| close.contains(mx, my));
        self.gfx.fill_circle(close.cx(), close.cy(), 15.0, Color::white(if hot { 0.14 } else { 0.06 }));
        self.gfx.icon('\u{E711}', close, 11.0, TEXT);
        self.hits.push((close, Hit::Close));

        let mut y = card.y + 76.0;
        for (i, row) in rows(self.section, cfg).into_iter().enumerate() {
            let r = Rect::new(cx, y, cw, ROW_H);
            if i > 0 {
                self.gfx.fill_rect(Rect::new(cx, y, cw, 1.0), Color::white(0.06));
            }
            // The control first: the words stop short of wherever it starts.
            let room = (self.control(&row, r, accent) - cx - 16.0).max(60.0);
            let (ly, lh) = if row.desc.is_empty() { (r.y, r.h) } else { (r.y + 9.0, 20.0) };
            self.gfx.text(row.label, Rect::new(cx, ly, room, lh), 14.0, 600, TEXT, Align::Left);
            if !row.desc.is_empty() {
                self.gfx.text(&row.desc, Rect::new(cx, r.y + 29.0, room, 17.0), 11.5, 500, DIM, Align::Left);
            }
            y += ROW_H;
        }
        if self.section == Section::About {
            let v = concat!("Synapse ", env!("CARGO_PKG_VERSION"), "  \u{00B7}  made for you, by you");
            self.gfx.text(v, Rect::new(cx, card.bottom() - 46.0, cw, 20.0), 12.0, 500, DIM, Align::Left);
        }
        self.gfx.end();
        self.gfx.present_in_place(self.hwnd);
    }

    fn hot(&self, r: Rect) -> bool {
        self.mouse.is_some_and(|(x, y)| r.contains(x, y))
    }

    /// The row's control, right-aligned in `r`. Returns where it starts.
    fn control(&mut self, row: &Row, r: Rect, accent: Color) -> f32 {
        let right = r.right();
        let cy = r.cy();
        let start = self.hits.len();
        match &row.control {
            Control::Switch(on) => {
                let s = Rect::new(right - 46.0, cy - 13.0, 46.0, 26.0);
                let hot = self.hot(s);
                self.gfx.fill_rrect(s, 13.0, if *on { accent } else { Color::white(if hot { 0.24 } else { 0.18 }) });
                let kx = if *on { s.right() - 13.0 } else { s.x + 13.0 };
                self.gfx.fill_circle(kx, cy, 10.0, Color::white(1.0));
                self.hits.push((s, Hit::Set(row.key, Val::Toggle)));
            }
            Control::Pick(opts, sel) => {
                let widths: Vec<f32> = opts.iter().map(|o| self.gfx.measure(o, 12.5, 600) + 22.0).collect();
                let total: f32 = widths.iter().sum::<f32>() + 6.0;
                let group = Rect::new(right - total, cy - 16.0, total, 32.0);
                self.gfx.fill_rrect(group, 16.0, Color::white(0.07));
                let mut x = group.x + 3.0;
                for (i, (o, w)) in opts.iter().zip(widths).enumerate() {
                    let b = Rect::new(x, group.y + 3.0, w, 26.0);
                    let on = i == *sel;
                    if on {
                        self.gfx.fill_rrect(b, 13.0, Color::white(0.18));
                    } else if self.hot(b) {
                        self.gfx.fill_rrect(b, 13.0, Color::white(0.07));
                    }
                    self.gfx.text(o, b, 12.5, 600, if on { TEXT } else { DIM }, Align::Center);
                    self.hits.push((b, Hit::Set(row.key, Val::Pick(i))));
                    x += w;
                }
            }
            Control::Step(value) => {
                let plus = Rect::new(right - 30.0, cy - 15.0, 30.0, 30.0);
                let val = Rect::new(plus.x - 84.0, cy - 15.0, 84.0, 30.0);
                let minus = Rect::new(val.x - 30.0, cy - 15.0, 30.0, 30.0);
                for (b, glyph, d) in [(minus, '\u{E738}', -1), (plus, '\u{E710}', 1)] {
                    let hot = self.hot(b);
                    self.gfx.fill_circle(b.cx(), b.cy(), 15.0, Color::white(if hot { 0.16 } else { 0.08 }));
                    self.gfx.icon(glyph, b, 11.0, TEXT);
                    self.hits.push((b, Hit::Set(row.key, Val::Step(d))));
                }
                self.gfx.text(value, val, 13.5, 650, TEXT, Align::Center);
            }
            Control::Swatches(sel) => {
                let n = ACCENTS.len();
                for (i, hex) in ACCENTS.iter().enumerate() {
                    let c = Color::parse(hex).unwrap_or(accent);
                    let x = right - 13.0 - (n - 1 - i) as f32 * 32.0;
                    let b = Rect::new(x - 14.0, cy - 14.0, 28.0, 28.0);
                    if i == *sel {
                        self.gfx.ring(x, cy, 13.0, 2.0, Color::white(0.9));
                    } else if self.hot(b) {
                        self.gfx.ring(x, cy, 13.0, 2.0, Color::white(0.3));
                    }
                    self.gfx.fill_circle(x, cy, 9.5, c);
                    self.hits.push((b, Hit::Set(row.key, Val::Pick(i))));
                }
            }
            Control::Button(label) => {
                let b = Rect::new(right - 96.0, cy - 16.0, 96.0, 32.0);
                let hot = self.hot(b);
                self.gfx.fill_rrect(b, 16.0, Color::white(if hot { 0.18 } else { 0.1 }));
                self.gfx.text(label, b, 13.0, 650, TEXT, Align::Center);
                self.hits.push((b, Hit::Set(row.key, Val::Toggle)));
            }
        }
        // Pickers sit in a group a few DIPs wider than their buttons.
        self.hits[start..].iter().map(|(h, _)| h.x).fold(right, f32::min) - 4.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steppers_round_and_clamp() {
        let mut c = Config::default();
        assert!(change(&mut c, Key::HoverDelay, Val::Step(1)));
        assert_eq!(c.hover_delay_ms, 270);
        for _ in 0..50 {
            change(&mut c, Key::Bounce, Val::Step(1));
        }
        assert_eq!(c.motion.bounce, 0.6);
        change(&mut c, Key::Speed, Val::Step(1));
        assert!((c.motion.speed - 1.1).abs() < 1e-5);
    }

    #[test]
    fn tabs_keep_their_order_and_one_stays() {
        let mut c = Config::default();
        change(&mut c, Key::Page(PageKind::Shelf), Val::Toggle);
        assert!(!c.pages.contains(&PageKind::Shelf));
        change(&mut c, Key::Page(PageKind::Shelf), Val::Toggle);
        assert_eq!(c.pages, Config::default().pages);
        c.pages = vec![PageKind::Clock];
        assert!(!change(&mut c, Key::Page(PageKind::Clock), Val::Toggle));
    }

    #[test]
    fn pickers() {
        let mut c = Config::default();
        change(&mut c, Key::Theme, Val::Pick(4));
        assert_eq!((c.theme, c.style), (ThemeName::Light, Style::Pill));
        change(&mut c, Key::Hotkey, Val::Pick(0));
        assert_eq!(c.hotkey, "");
        c.hotkey = "Alt+F9".into();
        let (opts, i) = options_with(&HOTKEYS, &c.hotkey);
        assert_eq!(opts[i], "Alt+F9");
        change(&mut c, Key::Accent, Val::Pick(3));
        assert_eq!(c.colors.accent, "#BF5AF2");
    }
}
