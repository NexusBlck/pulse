//! The island: its window, what it shows, how it reacts to the pointer,
//! and the frame loop that animates it.

use crate::clock::FrameClock;
use crate::config::{self, Config, Idle, PageKind, Style, ThemeName};
use crate::focus::{self, Phase, Pomodoro};
use crate::phone::{PhoneDrop, PhoneEvent};
use crate::shelf::{self, Drag, Shelf, Thumbs};
use std::collections::HashMap;
use crate::gfx::{identity, scale_about, Align, Color, Gfx, Image, Rect};
use crate::island::{Geom, Island};
use crate::lyrics::{Lyrics, LyricsService, Request};
use crate::media::{Media, MediaCmd, MediaInfo};
use crate::spring::Spring;
use crate::sys;
use crate::volume::Volume;
use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;
use windows::core::w;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::RemoteDesktop::{WTSRegisterSessionNotification, NOTIFY_FOR_THIS_SESSION};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT};
use windows::Win32::UI::Shell::{SHAppBarMessage, ABM_NEW, ABM_REMOVE, ABN_FULLSCREENAPP, APPBARDATA};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const WM_APP_EVENT: u32 = WM_APP + 1;
pub const WM_APP_FRAME: u32 = WM_APP + 2;
pub const WM_APP_TRAY: u32 = WM_APP + 3;
pub const WM_APP_APPBAR: u32 = WM_APP + 4;
const WM_MOUSELEAVE: u32 = 0x02A3;
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;

const TIMER_HOVER: usize = 1;
const TIMER_COLLAPSE: usize = 2;
const TIMER_WAKE: usize = 3;
/// Windows doesn't announce Do Not Disturb, so it's asked every few seconds
/// (one shell call, microseconds; skipped while locked).
const TIMER_DND: usize = 4;
const DND_CHECK_MS: u32 = 5000;

/// The hanging island's concave flare into the top edge.
const SHOULDER: f32 = 8.0;
/// Room around the island inside the window (the open island's forgiving edge).
const MARGIN: f32 = 12.0;
/// The pill style floats this far below the top edge.
const GAP: f32 = 8.0;
const HEADER: f32 = 48.0;
const PAD: f32 = 18.0;
/// How long the leaving content takes to fade.
const OUT_FADE: f64 = 0.14;
const PAGE_SWITCH: f64 = 0.24;
/// Back on the island this soon after it started closing: reopen at once.
const REOPEN_S: f64 = 0.3;
/// A paused player keeps its pill this long before the island goes idle.
const MEDIA_GRACE: f64 = 20.0;
/// A new track's title shows on the closed island this long.
const PEEK_S: f64 = 4.0;
/// Pointer travel that restarts the hover wait (brushing past doesn't open it).
const DWELL_SLOP: f32 = 4.0;
/// The Now Playing page's base height, and the lyrics under it.
const MEDIA_H: f32 = 148.0;
/// Now Playing's columns: the art, the player, the lyrics' share of the width.
const ART: f32 = 104.0;
const GAP_ART: f32 = 16.0;
const GAP_LYRICS: f32 = 22.0;
const LYRICS_SHARE: f32 = 0.42;
/// Space between lyric lines, and how many show around the current one.
const LYRIC_GAP: f32 = 9.0;
const ROWS_ABOVE: usize = 3;
const ROWS_BELOW: usize = 5;

const SYSTEM_H: f32 = 176.0;
const CAFFEINE_H: f32 = 176.0;
const PRAYER_H: f32 = 150.0;
const DOWNLOADS_H: f32 = 190.0;
const SCRIPT_H: f32 = 200.0;
const QUICK_H: f32 = 292.0;
const TASKS_H: f32 = 222.0;
const TASK_ROW: f32 = 30.0;
/// Task rows the page shows at once (the wheel scrolls the rest).
const TASK_ROWS: usize = 5;
/// The Script reader's reading line, this far down its text area.
const SCRIPT_LINE: f32 = 0.3;
const ADDON_ROW: f32 = 30.0;

/// The add-ons, in the order the Add-ons page lists them: name, what it
/// does, its icon, and whether it's here yet (the rest are on their way).
const ADDONS: [(&str, &str, char, bool); 8] = [
    ("System", "CPU, GPU, memory, network and the busiest apps", '\u{E9D9}', true),
    ("Caffeine", "Log your drinks and see what's still in you at bedtime", '\u{E945}', true),
    ("Prayer times", "Today's times where you are, and a countdown on the island", '\u{E708}', true),
    ("Downloader", "Save the video or sound of a link you paste, with yt-dlp", '\u{E896}', true),
    ("Script reader", "A teleprompter under your webcam, hidden from screen shares", '\u{E8A5}', true),
    ("Stocks", "Your watchlist's prices and charts", '\u{E9D2}', false),
    ("AI usage", "How much of your AI coding plans is left", '\u{E99A}', false),
    ("Japanese readings", "Romaji under the kanji in Japanese lyrics", '\u{E8C1}', false),
];

/// The Eyes resting look's life: when it last blinked and will next, where
/// it's looking (a spring, -1 left to 1 right) and when it next glances.
struct Eyes {
    blink_at: f64,
    next_blink: f64,
    look: Spring,
    glance_to: f32,
    next_glance: f64,
    seed: u64,
}

impl Eyes {
    fn new() -> Self {
        let seed = sys::filetime_now() as u64 | 1;
        Self { blink_at: -10.0, next_blink: 2.5, look: Spring::new(0.0, 260.0, Spring::damping_for(260.0, 0.85)), glance_to: 0.0, next_glance: 6.0, seed }
    }

    /// 0..1, a little random (xorshift).
    fn rand(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed % 10_000) as f32 / 10_000.0
    }
}

/// How open an eye is `t` seconds after a blink began (a blink is 0.16 s).
fn blink_open(t: f64) -> f32 {
    const BLINK: f64 = 0.16;
    if (0.0..BLINK).contains(&t) {
        let x = (t / BLINK) as f32;
        (1.0 - (x * std::f32::consts::PI).sin()).max(0.0)
    } else {
        1.0
    }
}

/// The tabs in the header: your pages, a tab for each add-on switched on,
/// and Add-ons last.
fn tabs_of(c: &Config) -> Vec<PageKind> {
    let mut t = c.pages.clone();
    let a = &c.addons;
    for (on, p) in [(a.system, PageKind::System), (a.caffeine, PageKind::Caffeine), (a.prayer, PageKind::Prayer), (a.downloader, PageKind::Downloads), (a.script, PageKind::Script)] {
        if on {
            t.push(p);
        }
    }
    t.push(PageKind::Addons);
    t
}

fn page_glyph(p: PageKind) -> char {
    match p {
        PageKind::Media => '\u{E8D6}',
        PageKind::Shelf => '\u{E7B8}',
        PageKind::Tasks => '\u{E762}',
        PageKind::Focus => '\u{E916}',
        PageKind::Clock => '\u{E823}',
        PageKind::Commands => '\u{E756}',
        PageKind::Themes => '\u{E790}',
        PageKind::Addons => '\u{E71D}',
        PageKind::System => '\u{E9D9}',
        PageKind::Caffeine => '\u{E945}',
        PageKind::Prayer => '\u{E708}',
        PageKind::Downloads => '\u{E896}',
        PageKind::Script => '\u{E8A5}',
        PageKind::Settings => '\u{E713}',
    }
}

/// Where the Script reader's script lives (edit it in any editor).
fn script_file() -> std::path::PathBuf {
    config::dir().join("script.txt")
}

/// A lyric line as shown: an instrumental break as dots (they render in any font).
fn lyric_text(s: &str) -> &str {
    if s.is_empty() {
        "\u{2022}  \u{2022}  \u{2022}"
    } else {
        s
    }
}

/// The current line's size and weight, and the others'.
fn lyric_font(current: bool) -> (f32, u32) {
    if current {
        (15.5, 700)
    } else {
        (13.0, 600)
    }
}
/// Shelf tiles, and the Focus page's height.
const TILE_W: f32 = 84.0;
const TILE_H: f32 = 88.0;
const TILE_GAP: f32 = 6.0;
const FOCUS_H: f32 = 136.0;
/// Pointer travel on a shelf file that starts dragging it out.
const DRAG_SLOP: f32 = 6.0;
/// The Shelf's phone panel, and how long it waits with nothing sent.
const PHONE_H: f32 = 150.0;
const PHONE_IDLE_S: f64 = 600.0;

// Segoe Fluent Icons / MDL2 Assets glyphs.
const G_PLAY: char = '\u{E768}';
const G_PAUSE: char = '\u{E769}';
const G_PREV: char = '\u{E892}';
const G_NEXT: char = '\u{E893}';
const G_NOTE: char = '\u{E8D6}';
const G_GEAR: char = '\u{E713}';
const G_PIN: char = '\u{E718}';
const G_PINNED: char = '\u{E840}';
const G_INFO: char = '\u{E946}';
const G_MUTE: char = '\u{E74F}';
const G_VOL: [char; 4] = ['\u{E992}', '\u{E993}', '\u{E994}', '\u{E995}'];
const G_CLOSE: char = '\u{E711}';
const G_DROP: char = '\u{E896}';
const G_DELETE: char = '\u{E74D}';
const G_RESET: char = '\u{E72C}';
const G_SKIP: char = '\u{E893}';
const G_PHONE: char = '\u{E8EA}';
const G_CHECK: char = '\u{E73E}';
const G_WARN: char = '\u{E7BA}';

/// The Focus timer's colours: amber for focus, green and blue for breaks.
fn phase_color(p: Phase) -> Color {
    match p {
        Phase::Focus => Color::rgba(1.0, 0.62, 0.04, 1.0),
        Phase::Break => Color::rgba(0.19, 0.82, 0.35, 1.0),
        Phase::LongBreak => Color::rgba(0.39, 0.82, 1.0, 1.0),
    }
}

/// Wall-clock seconds (the Focus timer counts through sleep).
fn wall() -> f64 {
    sys::filetime_now() as f64 / 1e7
}

pub enum Event {
    Media(Option<MediaInfo>),
    Art { key: String, img: Option<Image>, accent: Option<[f32; 3]> },
    Lyrics { key: String, lyrics: Option<Lyrics> },
    Thumb { path: String, img: Option<Image> },
    Phone(PhoneEvent),
    Bluetooth { name: String, connected: bool, audio: bool, battery: Option<u8> },
    BluetoothBattery { name: String, battery: u8 },
    ScreenPalette(crate::ambient::Palette),
    System(Box<crate::system::Snapshot>),
    Located(Result<(f64, f64), String>),
    Download(crate::downloader::DlEvent),
    ToolReady(Result<(), String>),
    Privacy(Vec<crate::privacy::InUse>),
    Volume { level: f32, muted: bool },
    AudioDevice,
    ConfigChanged,
}

/// How services talk to the UI thread.
#[derive(Clone)]
pub struct Sink {
    tx: Sender<Event>,
    hwnd: isize,
}

impl Sink {
    /// A sink with no window behind it: events only go down the channel.
    #[cfg(test)]
    pub fn for_test(tx: Sender<Event>) -> Self {
        Self { tx, hwnd: 0 }
    }

    pub fn send(&self, e: Event) {
        if self.tx.send(e).is_ok() {
            unsafe {
                let _ = PostMessageW(HWND(self.hwnd as *mut _), WM_APP_EVENT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    Idle,
    Media,
    Hud(HudTag),
    Notice,
    /// The Focus timer counting down.
    Timer,
    /// The minutes before a prayer.
    Prayer,
    /// Downloads under way.
    Download,
    Expanded(PageKind),
}

impl Mode {
    /// Modes with the same key share content (no cross-fade between them).
    fn key(self) -> u8 {
        match self {
            Mode::Idle => 0,
            Mode::Media => 1,
            Mode::Notice => 3,
            Mode::Expanded(_) => 4,
            Mode::Timer => 5,
            Mode::Prayer => 6,
            Mode::Download => 7,
            Mode::Hud(t) => 10 + t as u8,
        }
    }
}

/// What a pop-up is about (each kind cross-fades into the next).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HudTag {
    Volume,
    Power,
    Bluetooth,
    /// Short status changes: unlocked, mute, Do Not Disturb.
    Status,
    Privacy,
    /// Files arriving from the phone.
    Transfer,
}

/// The pop-up dropping down from the closed island.
#[derive(Clone, Debug, PartialEq)]
enum Hud {
    Volume,
    Charging(u8),
    LowBattery(u8),
    Bluetooth { name: String, connected: bool, audio: bool, battery: Option<u8> },
    /// Windows was just unlocked.
    Unlocked,
    /// Muted (true) or sound back on.
    Mute(bool),
    /// Do Not Disturb on (true) or off.
    Dnd(bool),
    /// An app started using the camera, microphone or location.
    Privacy(crate::privacy::Sense, String),
    /// A file from the phone: its name, and how far (1.0 = arrived).
    Transfer(String, f32),
}

impl Hud {
    fn tag(&self) -> HudTag {
        match self {
            Hud::Volume => HudTag::Volume,
            Hud::Charging(_) | Hud::LowBattery(_) => HudTag::Power,
            Hud::Bluetooth { .. } => HudTag::Bluetooth,
            Hud::Unlocked | Hud::Mute(_) | Hud::Dnd(_) => HudTag::Status,
            Hud::Privacy(..) => HudTag::Privacy,
            Hud::Transfer(..) => HudTag::Transfer,
        }
    }
}

/// The privacy colours, as on a phone: camera green, microphone orange, location blue.
fn sense_color(s: crate::privacy::Sense) -> Color {
    match s {
        crate::privacy::Sense::Camera => Color::rgba(0.19, 0.82, 0.35, 1.0),
        crate::privacy::Sense::Microphone => Color::rgba(1.0, 0.62, 0.04, 1.0),
        crate::privacy::Sense::Location => Color::rgba(0.04, 0.52, 1.0, 1.0),
    }
}

/// Windows' Do Not Disturb, as the shell reports it.
fn dnd_on() -> bool {
    use windows::Win32::UI::Shell::{SHQueryUserNotificationState, QUNS_QUIET_TIME};
    unsafe { SHQueryUserNotificationState().is_ok_and(|s| s == QUNS_QUIET_TIME) }
}

/// Low-battery warnings, once each as the battery drains past them.
const LOW_BATTERY: [u8; 2] = [20, 10];

/// Work that runs its own message loop, done outside the app's borrow.
enum Deferred {
    Menu,
    DragOut(Vec<String>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Action {
    Tab(PageKind),
    Toggle,
    Next,
    Prev,
    Seek,
    Command(usize),
    OpenConfig,
    Pin,
    ShelfOpen(usize),
    ShelfRemove(usize),
    ShelfClear,
    PhoneOpen,
    PhoneClose,
    OpenThemes,
    Theme(ThemeName),
    OpenSettings,
    /// The gear: the island's own Settings page (and back).
    QuickSettings,
    /// Tasks: type in the field, add what's typed, tick, delete, clear done.
    TaskFocus,
    TaskAdd,
    TaskToggle(u64),
    TaskDelete(u64),
    TaskClearDone,
    /// A setting changed on that page.
    Set(crate::settings::Key, crate::settings::Val),
    /// Switch add-on `i` (in `ADDONS`) on or off.
    Addon(usize),
    DrinkAdd(usize),
    DrinkUndo,
    Locate,
    LocationSettings,
    /// Downloader: the link on the clipboard, as sound (true) or video.
    Paste(bool),
    DlStop(u64),
    DlOpen(u64),
    DlFolder,
    ToolRetry,
    /// Script reader.
    ScriptPaste,
    ScriptEdit,
    ScriptPlay,
    ScriptRestart,
    ScriptSpeed(i32),
    ScriptSize(i32),
    FocusToggle,
    FocusReset,
    FocusSkip,
}

#[derive(Clone, Debug, PartialEq)]
enum JobState {
    Queued,
    Running,
    Done(String),
    Failed(String),
}

/// A download, as the Downloads page shows it.
#[derive(Clone, Debug)]
struct Job {
    id: u64,
    url: String,
    audio: bool,
    title: String,
    pct: f32,
    speed: String,
    eta: String,
    state: JobState,
}

#[derive(Clone, Debug, PartialEq)]
enum Tool {
    Missing,
    Fetching,
    Ready,
    Failed(String),
}

struct Notice {
    text: String,
    until: f64,
    w: f32,
}

struct Theme {
    name: ThemeName,
    island: Color,
    text: Color,
    dim: Color,
    accent: Color,
    border: Color,
    /// Ambient light: the halo around the island and how strong it is.
    glow: Option<(Color, f32)>,
}

impl Theme {
    /// The colours a theme draws with: `colors` as written, the accent from
    /// the album art when it should and there is one.
    fn of(name: ThemeName, c: &config::Colors, art: Option<[f32; 3]>) -> Theme {
        let pick = |s: &str, d: Color| Color::parse(s).unwrap_or(d);
        let mut accent = pick(&c.accent, Color::rgba(0.19, 0.82, 0.35, 1.0));
        if let (true, Some([r, g, b])) = (c.accent_from_art, art) {
            accent = Color::rgba(r, g, b, 1.0);
        }
        Theme {
            name,
            island: pick(&c.island, Color::black(1.0)),
            text: pick(&c.text, Color::white(0.96)),
            dim: pick(&c.dim, Color::white(0.62)),
            accent,
            border: pick(&c.border, Color::white(0.08)),
            glow: None,
        }
    }

    /// The text colour at `a`: hover fills, tracks and buttons, on any island.
    fn ink(&self, a: f32) -> Color {
        Color::rgba(self.text.r, self.text.g, self.text.b, a)
    }
}

/// Where a gradient theme's body runs to, from the island colour.
fn gradient_end(t: ThemeName) -> Option<Color> {
    match t {
        ThemeName::Aurora => Some(Color::rgba(0.23, 0.48, 0.84, 1.0)),
        ThemeName::Pastel => Some(Color::rgba(0.80, 0.88, 1.0, 1.0)),
        ThemeName::Coffee => Some(Color::rgba(0.42, 0.28, 0.20, 1.0)),
        _ => None,
    }
}
const THEMES_H: f32 = 178.0;

pub struct App {
    hwnd: HWND,
    cfg: Config,
    gfx: Gfx,
    island: Island,
    mode: Mode,
    clock: FrameClock,
    rx: Receiver<Event>,
    media: Option<Media>,
    volume_svc: Option<Volume>,
    tray: Option<sys::Tray>,
    taskbar_created: u32,
    t0: Instant,
    last_t: f64,
    // Window placement (screen px) and the island's centre in it (DIPs).
    win_x: i32,
    win_y: i32,
    cx: f32,
    shown: bool,
    // Pointer and holds.
    mouse: Option<(f32, f32)>,
    hovering: bool,
    hover_arm: Option<(f32, f32)>,
    expanded: bool,
    pinned: bool,
    /// The right-click menu is showing (it holds the open island open).
    menu_open: bool,
    down: bool,
    pressed: Option<(Rect, Action)>,
    collapse_due: bool,
    left_at: f64,
    page: PageKind,
    // Transitions.
    outgoing: Option<(Mode, f64, Geom, f32)>,
    page_anim: Option<(PageKind, f64, f32)>,
    // What's shown.
    now_playing: Option<MediaInfo>,
    paused_at: Option<f64>,
    peek_until: f64,
    art: Option<Image>,
    art_bmp: Option<ID2D1Bitmap>,
    art_accent: Option<[f32; 3]>,
    lyrics_svc: Option<LyricsService>,
    /// The current track's synced lyrics, once found.
    lyrics: Option<Lyrics>,
    /// The line being sung, when it started showing, and its width in the pill.
    lyric_idx: Option<usize>,
    lyric_at: f64,
    lyric_w: f32,
    /// The Now Playing page's lyrics scroll (in lines).
    lyric_scroll: Spring,
    /// Wrapped lyric lines' heights (line, current) for the column width.
    lyric_heights: HashMap<(usize, bool), f32>,
    lyric_cache_w: f32,
    shelf: Shelf,
    thumbs: Option<Thumbs>,
    thumb_imgs: HashMap<String, Image>,
    thumb_bmps: HashMap<String, ID2D1Bitmap>,
    /// Files are being dragged over the island (it stays open).
    drag_hover: bool,
    /// A press on a shelf file: where, and which (moving turns it into a drag out).
    drag_from: Option<((f32, f32), usize)>,
    sink: Sink,
    /// Ambient light: the palettes of the art and the screen, the one shown
    /// now (easing towards its source), and the music's loudness.
    art_palette: Option<crate::ambient::Palette>,
    screen_palette: Option<crate::ambient::Palette>,
    pal_now: Option<crate::ambient::Palette>,
    level: f32,
    sampler: Option<crate::ambient::ScreenSampler>,
    edge: Option<crate::edge::EdgeGlow>,
    openrgb: Option<crate::openrgb::OpenRgb>,
    rgb_sent: Option<[u8; 3]>,
    /// The island's monitor (screen pixels).
    mon_rect: RECT,
    /// System add-on: its reader (while the page is open), the last
    /// reading, and a minute of CPU, GPU, memory and download history.
    sysmon: Option<crate::system::SystemMonitor>,
    sys_snap: Option<crate::system::Snapshot>,
    sys_hist: std::collections::VecDeque<[f32; 4]>,
    /// Caffeine add-on: the drinks logged.
    caf: crate::caffeine::Log,
    /// Prayer add-on: the last prayer announced (day, prayer), and
    /// whether Windows is being asked where the PC is, or said no.
    prayer_told: Option<(i64, crate::prayer::Prayer)>,
    locating: bool,
    locate_error: Option<String>,
    /// Downloader: the downloads (newest last), the yt-dlps running, yt-dlp itself.
    jobs: Vec<Job>,
    running: HashMap<u64, crate::downloader::Running>,
    next_job: u64,
    tool: Tool,
    /// Script reader: the script, how far it's read (DIPs), whether it scrolls,
    /// its paragraphs' heights (for the width and size they were measured at),
    /// and whether the island is out of screen capture.
    script: String,
    script_pos: f32,
    script_playing: bool,
    script_heights: Vec<f32>,
    script_measured: (f32, f32),
    capture_hidden: bool,
    eyes: Eyes,
    /// The Glass theme's blur window (made the first time Glass shows).
    backdrop: Option<crate::backdrop::Backdrop>,
    /// Tasks: the list, what's being typed (and where the caret is), whether
    /// the field has the keyboard (and who had it before), the list's scroll.
    tasks: crate::tasks::Tasks,
    draft: String,
    caret: usize,
    typing: bool,
    prev_focus: HWND,
    high_surrogate: Option<u16>,
    tasks_scroll: usize,
    /// Apps using the camera, microphone or location right now.
    in_use: Vec<crate::privacy::InUse>,
    /// Windows' Do Not Disturb as last seen.
    dnd: bool,
    /// The Settings window, while it's open.
    settings_win: Option<crate::settings::SettingsWindow>,
    /// The Shelf's phone panel shows (holding the island open).
    phone_panel: bool,
    /// Phone drop's server, while the panel is open.
    phone: Option<PhoneDrop>,
    phone_qr: Option<(usize, Vec<bool>)>,
    /// The panel's status line, and whether it reports trouble.
    phone_status: String,
    phone_bad: bool,
    phone_progress: Option<f32>,
    phone_got: u32,
    phone_seen: bool,
    phone_last: f64,
    pomo: Pomodoro,
    volume: (f32, bool),
    hud_until: f64,
    hud: Hud,
    /// A config write of our own (a theme pick) reloads quietly until then.
    quiet_until: f64,
    /// The battery as last seen (percent, charging), to notice changes.
    power: Option<(u8, bool)>,
    notice: Option<Notice>,
    fullscreen: bool,
    locked: bool,
    // Per frame: clickable regions, and whether the content being drawn takes input.
    hits: Vec<(Rect, Action)>,
    live: bool,
    base_xf: Matrix3x2,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub fn run() -> i32 {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _mutex = CreateMutexW(None, true, w!("Local\\Synapse.Island"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return 0;
        }
        // OLE, not just COM: dragging files in and out needs it.
        let _ = OleInitialize(None);
        let hwnd = match create_window() {
            Ok(h) => h,
            Err(e) => {
                crate::log(&format!("window: {e}"));
                return 1;
            }
        };
        let (tx, rx) = mpsc::channel();
        let sink = Sink { tx, hwnd: hwnd.0 as isize };
        let (cfg, err) = match config::load() {
            Ok(c) => (c, None),
            Err(e) => (Config::default(), Some(e)),
        };
        let app = match App::new(hwnd, cfg, sink, rx) {
            Ok(a) => a,
            Err(e) => {
                crate::log(&format!("renderer: {e}"));
                return 2;
            }
        };
        APP.with(|c| *c.borrow_mut() = Some(app));
        with_app(|a| a.start(err));
        crate::log("Synapse started");
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|c| c.borrow_mut().take());
        crate::log("Synapse quit");
        0
    }
}

fn create_window() -> windows::core::Result<HWND> {
    unsafe {
        let hinst = GetModuleHandleW(None)?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: w!("Synapse.Island"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        crate::settings::register(Some(settings_wndproc));
        CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            w!("Synapse.Island"),
            w!("Synapse"),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            hinst,
            None,
        )
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let mut deferred = None;
    let r = with_app(|a| a.handle(msg, wp, lp, &mut deferred)).flatten();
    // The menu and a drag out run their own message loops: outside the app's borrow.
    match deferred {
        Some(Deferred::Menu) => {
            let auto = with_app(|a| {
                a.menu_open = true;
                a.cfg.start_with_windows
            })
            .unwrap_or(false);
            let pick = sys::menu(hwnd, auto);
            with_app(|a| {
                a.menu_closed();
                a.on_menu(pick);
            });
        }
        Some(Deferred::DragOut(paths)) => {
            with_app(|a| a.drag_hover = true);
            let _ = shelf::drag_out(&paths);
            with_app(|a| a.dragged_out());
        }
        None => {}
    }
    match r {
        Some(l) => l,
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

unsafe extern "system" fn settings_wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match with_app(|a| a.handle_settings(hwnd, msg, wp, lp)).flatten() {
        Some(r) => LRESULT(r),
        None => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Files dragged over the island (OLE calls this on the UI thread).
fn on_drag(d: Drag) {
    with_app(|a| a.on_drag(d));
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How much of the leaving content still shows `age` seconds into its fade.
fn out_left(age: f64) -> f32 {
    1.0 - smoothstep((age / OUT_FADE) as f32)
}

fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

impl App {
    fn new(hwnd: HWND, cfg: Config, sink: Sink, rx: Receiver<Event>) -> windows::core::Result<Self> {
        let gfx = Gfx::new(&cfg.font)?;
        let first = page_or_first(&cfg, PageKind::Media);
        let mut app = Self {
            hwnd,
            gfx,
            island: Island::new(Geom::new(88.0, 30.0, 15.0), cfg.motion),
            mode: Mode::Idle,
            clock: FrameClock::start(hwnd),
            rx,
            media: Some(Media::start(sink.clone())),
            volume_svc: None,
            tray: None,
            taskbar_created: unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) },
            t0: Instant::now(),
            last_t: 0.0,
            win_x: 0,
            win_y: 0,
            cx: 0.0,
            shown: false,
            mouse: None,
            hovering: false,
            hover_arm: None,
            expanded: false,
            pinned: false,
            menu_open: false,
            down: false,
            pressed: None,
            collapse_due: false,
            left_at: -10.0,
            page: first,
            outgoing: None,
            page_anim: None,
            now_playing: None,
            paused_at: None,
            peek_until: 0.0,
            art: None,
            art_bmp: None,
            art_accent: None,
            lyrics_svc: Some(LyricsService::start(sink.clone())),
            lyrics: None,
            lyric_idx: None,
            lyric_at: 0.0,
            lyric_w: 0.0,
            lyric_scroll: Spring::new(-1.0, 170.0, Spring::damping_for(170.0, 0.9)),
            lyric_heights: HashMap::new(),
            lyric_cache_w: 0.0,
            shelf: Shelf::load(),
            thumbs: Some(Thumbs::start(sink.clone())),
            thumb_imgs: HashMap::new(),
            thumb_bmps: HashMap::new(),
            drag_hover: false,
            drag_from: None,
            sink: sink.clone(),
            art_palette: None,
            screen_palette: None,
            pal_now: None,
            level: 0.0,
            sampler: None,
            edge: None,
            openrgb: None,
            rgb_sent: None,
            mon_rect: RECT::default(),
            sysmon: None,
            sys_snap: None,
            sys_hist: std::collections::VecDeque::new(),
            caf: crate::caffeine::Log::load(),
            prayer_told: None,
            locating: false,
            locate_error: None,
            jobs: Vec::new(),
            running: HashMap::new(),
            next_job: 1,
            tool: if crate::downloader::have_ytdlp() { Tool::Ready } else { Tool::Missing },
            script: std::fs::read_to_string(script_file()).unwrap_or_default(),
            script_pos: 0.0,
            script_playing: false,
            script_heights: Vec::new(),
            script_measured: (0.0, 0.0),
            capture_hidden: false,
            eyes: Eyes::new(),
            backdrop: None,
            tasks: crate::tasks::Tasks::load(),
            draft: String::new(),
            caret: 0,
            typing: false,
            prev_focus: HWND::default(),
            high_surrogate: None,
            tasks_scroll: 0,
            in_use: Vec::new(),
            dnd: dnd_on(),
            settings_win: None,
            phone_panel: false,
            phone: None,
            phone_qr: None,
            phone_status: String::new(),
            phone_bad: false,
            phone_progress: None,
            phone_got: 0,
            phone_seen: false,
            phone_last: 0.0,
            pomo: Pomodoro::default(),
            volume: (0.0, false),
            hud_until: 0.0,
            hud: Hud::Volume,
            quiet_until: 0.0,
            power: sys::battery(),
            notice: None,
            fullscreen: false,
            locked: false,
            hits: Vec::new(),
            live: true,
            base_xf: identity(),
            cfg,
        };
        match Volume::start(sink.clone()) {
            Ok(v) => {
                if let Some(r) = v.read() {
                    app.volume = r;
                }
                app.volume_svc = Some(v);
            }
            Err(e) => crate::log(&format!("volume: {e}")),
        }
        crate::bluetooth::start(sink.clone());
        crate::privacy::start(sink.clone());
        crate::watch::start(sink);
        Ok(app)
    }

    fn start(&mut self, config_error: Option<String>) {
        self.tray = Some(sys::Tray::new(self.hwnd));
        unsafe {
            let mut abd = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, uCallbackMessage: WM_APP_APPBAR, ..Default::default() };
            SHAppBarMessage(ABM_NEW, &mut abd);
            let _ = WTSRegisterSessionNotification(self.hwnd, NOTIFY_FOR_THIS_SESSION);
            SetTimer(self.hwnd, TIMER_DND, DND_CHECK_MS, None);
        }
        if let Err(e) = shelf::accept_drops(self.hwnd, on_drag) {
            crate::log(&format!("shelf: can't accept dropped files ({e})"));
        }
        for p in self.shelf.items.clone() {
            self.want_thumb(&p);
        }
        let cfg = self.cfg.clone();
        self.apply(cfg, true);
        let g = self.geom_for(self.mode, 0.0);
        self.island.snap(g);
        match config_error {
            Some(e) => self.notify(&e, 8.0),
            None => self.notify("Synapse is running", 2.5),
        }
        self.request_frame();
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    fn request_frame(&self) {
        self.clock.set_active(true);
    }

    // ------------------------------------------------------------ config

    fn apply(&mut self, c: Config, first: bool) {
        let before = self.cfg.clone();
        self.gfx.set_font(&c.font);
        self.island.set_motion(c.motion);
        if first || c.hotkey != before.hotkey {
            if let Err(e) = sys::register_hotkey(self.hwnd, sys::HOTKEY_ISLAND, &c.hotkey) {
                self.notify(&e, 5.0);
            }
        }
        // The Downloader: on fetches yt-dlp; off stops downloads and deletes it.
        if !first && c.addons.downloader != before.addons.downloader {
            if c.addons.downloader {
                self.ensure_tool();
            } else {
                for r in self.running.values() {
                    r.stop();
                }
                self.running.clear();
                self.jobs.clear();
                crate::downloader::remove_tools();
                self.tool = Tool::Missing;
            }
        }
        if !c.addons.script {
            self.script_playing = false;
        }
        // The Script reader's hotkey, only while the add-on is on.
        let script_key = if c.addons.script { c.script.hotkey.clone() } else { String::new() };
        let was = if before.addons.script { before.script.hotkey.clone() } else { String::new() };
        if first || script_key != was {
            if let Err(e) = sys::register_hotkey(self.hwnd, sys::HOTKEY_SCRIPT, &script_key) {
                self.notify(&e, 5.0);
            }
        }
        if first || c.start_with_windows != before.start_with_windows {
            sys::set_autostart(c.start_with_windows);
        }
        if !matches!(self.page, PageKind::Themes | PageKind::Settings) && !tabs_of(&c).contains(&self.page) {
            self.page = c.pages[0];
        }
        self.cfg = c;
        self.layout();
    }

    fn reload(&mut self) {
        match config::load() {
            Ok(c) => {
                self.apply(c, false);
                if self.now() >= self.quiet_until {
                    self.notify("Config reloaded", 1.8);
                }
                // An edit by hand shows in the open Settings window too.
                self.repaint_settings();
            }
            Err(e) => self.notify(&e, 8.0),
        }
    }

    fn notify(&mut self, text: &str, secs: f64) {
        let w = (self.gfx.measure(text, 13.0, 500) + 70.0).clamp(220.0, 600.0);
        self.notice = Some(Notice { text: text.to_string(), until: self.now() + secs, w });
        self.request_frame();
    }

    fn open_config(&self) {
        let path = config::path();
        if self.cfg.editor.trim().is_empty() {
            sys::launch(&path.to_string_lossy(), "");
        } else {
            sys::launch(&self.cfg.editor, &format!("\"{}\"", path.display()));
        }
    }

    fn on_menu(&mut self, pick: usize) {
        match pick {
            sys::MENU_SETTINGS => self.open_settings(),
            sys::MENU_CONFIG => self.open_config(),
            sys::MENU_RELOAD => self.reload(),
            sys::MENU_AUTOSTART => {
                let on = !self.cfg.start_with_windows;
                // Written to the file; the watcher reloads it and applies it.
                if let Err(e) = config::set_key("start_with_windows", serde_json::Value::Bool(on)) {
                    self.notify(&format!("Couldn't save: {e}"), 4.0);
                }
            }
            sys::MENU_QUIT => unsafe {
                let _ = DestroyWindow(self.hwnd);
            },
            _ => {}
        }
    }

    // ------------------------------------------------------------ layout

    fn shelf_cols(&self) -> usize {
        (((self.cfg.width - 2.0 * PAD + TILE_GAP) / (TILE_W + TILE_GAP)) as usize).max(1)
    }

    /// Rows for `n` files plus the Phone and Clear tiles (one row when empty).
    fn shelf_rows(&self, n: usize) -> usize {
        if n == 0 {
            1
        } else {
            (n + 2).div_ceil(self.shelf_cols())
        }
    }

    fn lyrics_shown(&self) -> bool {
        self.cfg.lyrics && self.lyrics.is_some()
    }

    fn page_h(&self, p: PageKind) -> f32 {
        match p {
            PageKind::Media => MEDIA_H,
            PageKind::Shelf if self.phone_panel => PHONE_H,
            PageKind::Shelf => {
                let rows = self.shelf_rows(self.shelf.items.len()) as f32;
                rows * TILE_H + (rows - 1.0) * TILE_GAP
            }
            PageKind::Focus => FOCUS_H,
            PageKind::Themes => THEMES_H,
            PageKind::Addons => 26.0 + ADDONS.len() as f32 * ADDON_ROW,
            PageKind::System => SYSTEM_H,
            PageKind::Caffeine => CAFFEINE_H,
            PageKind::Prayer => PRAYER_H,
            PageKind::Downloads => DOWNLOADS_H,
            PageKind::Script => SCRIPT_H,
            PageKind::Settings => QUICK_H,
            PageKind::Tasks => TASKS_H,
            PageKind::Clock => 112.0,
            PageKind::Commands => {
                let rows = self.cfg.commands.len().div_ceil(3).max(1) as f32;
                rows * 38.0 + (rows - 1.0) * 10.0
            }
        }
    }

    /// Size the window for the tallest open island and put it at the top
    /// of the primary monitor. Morphs never move or resize it.
    fn layout(&mut self) {
        unsafe {
            let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(mon, &mut mi);
            let (mut dx, mut dy) = (96u32, 96u32);
            let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
            let scale = dx as f32 / 96.0;
            // Room for the Now Playing page with lyrics, found or not yet.
            // ...and for a full Shelf.
            let page_max = |p: PageKind| match p {
                PageKind::Shelf => {
                    let rows = self.shelf_rows(shelf::MAX) as f32;
                    (rows * TILE_H + (rows - 1.0) * TILE_GAP).max(PHONE_H)
                }
                _ => self.page_h(p),
            };
            // The Themes page opens from any page.
            let tallest = tabs_of(&self.cfg).iter().chain([PageKind::Addons, PageKind::System, PageKind::Caffeine, PageKind::Prayer, PageKind::Downloads, PageKind::Script, PageKind::Settings].iter()).map(|p| HEADER + page_max(*p) + PAD).fold(HEADER + THEMES_H + PAD, f32::max);
            let w_dip = self.cfg.width * 1.06 + 2.0 * (SHOULDER + MARGIN);
            let h_dip = GAP + tallest * 1.06 + MARGIN;
            let (w, h) = ((w_dip * scale).ceil() as i32, (h_dip * scale).ceil() as i32);
            let rc = mi.rcMonitor;
            // A new monitor or scale: the edge glow is made again for it.
            if rc != self.mon_rect {
                self.edge = None;
            }
            self.mon_rect = rc;
            let mon_w = rc.right - rc.left;
            let offset = self.cfg.offset_x;
            // Keep the open island on the monitor however far it's moved.
            let room = ((mon_w as f32 / scale - self.cfg.width) / 2.0 - MARGIN).max(0.0);
            let centre = rc.left + mon_w / 2 + (offset.clamp(-room, room) * scale).round() as i32;
            self.win_x = (centre - w / 2).clamp(rc.left, (rc.right - w).max(rc.left));
            self.win_y = rc.top;
            self.cx = (centre - self.win_x) as f32 / scale;
            if let Err(e) = self.gfx.resize(w, h, scale) {
                crate::log(&format!("resize {w}x{h}: {e}"));
            }
        }
        self.request_frame();
    }

    fn hidden_rest(&self, m: Mode) -> bool {
        m == Mode::Idle && self.cfg.idle == Idle::Hidden
    }

    fn top_for(&self, m: Mode) -> f32 {
        if self.cfg.style == Style::Pill && !self.hidden_rest(m) {
            GAP
        } else {
            0.0
        }
    }

    fn peeking(&self, now: f64) -> bool {
        now < self.peek_until
    }

    fn media_shown(&self, now: f64) -> bool {
        self.cfg.show_media && self.now_playing.as_ref().is_some_and(|m| m.playing || self.paused_at.is_some_and(|t| now < t + MEDIA_GRACE))
    }

    fn target_mode(&self, now: f64) -> Mode {
        if self.expanded {
            Mode::Expanded(self.page)
        } else if self.notice.is_some() {
            Mode::Notice
        } else if now < self.hud_until {
            Mode::Hud(self.hud.tag())
        } else if self.prayer_pill().is_some() {
            // The minutes before a prayer don't come back: they go first.
            Mode::Prayer
        } else if self.pomo.active() {
            Mode::Timer
        } else if self.media_shown(now) {
            Mode::Media
        } else if self.download_progress().is_some() {
            // Downloads take minutes and have a page: they wait behind music.
            Mode::Download
        } else {
            Mode::Idle
        }
    }

    fn geom_for(&self, m: Mode, now: f64) -> Geom {
        let g = match m {
            Mode::Idle => match self.cfg.idle {
                Idle::Clock => Geom::new(if self.cfg.show_seconds { 112.0 } else { 88.0 }, 30.0, 15.0),
                Idle::Eyes => Geom::new(96.0, 30.0, 15.0),
                Idle::Dot => Geom::new(72.0, 24.0, 12.0),
                Idle::Hidden => Geom::new(180.0, 5.0, 2.5),
            },
            // The pill grows and shrinks with the line being sung.
            Mode::Media => {
                let w = if self.peeking(now) {
                    340.0
                } else if self.sings() {
                    (self.lyric_w + 112.0).clamp(220.0, 620.0)
                } else {
                    196.0
                };
                Geom::new(w, 34.0, 17.0)
            }
            Mode::Prayer => Geom::new(196.0, 32.0, 16.0),
            Mode::Download => Geom::new(108.0, 32.0, 16.0),
            Mode::Hud(HudTag::Bluetooth) => Geom::new(340.0, 46.0, 23.0),
            Mode::Hud(HudTag::Status) => Geom::new(if self.hud == Hud::Unlocked { 172.0 } else { 236.0 }, 36.0, 18.0),
            Mode::Hud(HudTag::Privacy) => Geom::new(320.0, 36.0, 18.0),
            Mode::Hud(HudTag::Transfer) => Geom::new(330.0, 44.0, 22.0),
            Mode::Hud(_) => Geom::new(300.0, 40.0, 20.0),
            Mode::Timer => Geom::new(if focus::fmt_left(self.pomo.left(wall(), &self.cfg.focus)).len() > 5 { 136.0 } else { 120.0 }, 32.0, 16.0),
            Mode::Notice => Geom::new(self.notice.as_ref().map_or(300.0, |n| n.w), 46.0, 23.0),
            Mode::Expanded(p) => Geom::new(self.cfg.width, HEADER + self.page_h(p) + PAD, 30.0),
        };
        // The closed island swells a little under the pointer: instant feedback.
        let liftable = !matches!(m, Mode::Expanded(_)) && !self.hidden_rest(m);
        if liftable && self.hovering && !self.expanded {
            Geom::new(g.w + 10.0, g.h + 4.0, g.r + 2.0)
        } else {
            g
        }
    }

    fn theme(&self) -> Theme {
        let mut th = Theme::of(self.cfg.theme, &self.cfg.colors, self.art_accent.filter(|_| self.now_playing.is_some()));
        let amb = &self.cfg.ambient;
        if let Some(p) = self.pal_now.filter(|_| amb.enabled) {
            // The island wears the light: accent, a tint in its body, a halo.
            let m = p.mix();
            th.accent = m;
            if amb.tint_island {
                let tint = crate::ambient::lerp(th.island, Color::rgba(m.r, m.g, m.b, th.island.a), 0.16);
                th.island = tint;
                th.glow = Some((m, amb.intensity * self.breath_gain()));
            }
        }
        th
    }

    /// The glow's brightness from the music (1 without breathing).
    fn breath_gain(&self) -> f32 {
        if self.cfg.ambient.breathe {
            0.65 + 0.7 * self.level
        } else {
            1.0
        }
    }

    /// The island's body in theme `th`: hanging from the edge (`hangs`) or
    /// floating, with the theme's finish. Returns its outline, for clipping.
    fn paint_island(&mut self, body: Rect, r: f32, hangs: bool, th: &Theme) -> Option<windows::Win32::Graphics::Direct2D::ID2D1Geometry> {
        let (shape, rim) = if hangs {
            (self.gfx.notch(body, r, SHOULDER, true)?, self.gfx.notch(body, r, SHOULDER, false)?)
        } else {
            let s = self.gfx.rrect_geo(body, r)?;
            (s.clone(), s)
        };
        let floating_finish = matches!(th.name, ThemeName::Capsule | ThemeName::Glass | ThemeName::Light | ThemeName::Aurora | ThemeName::Pastel | ThemeName::Coffee);
        if floating_finish && !hangs {
            // A soft shadow: a few widening, fading outlines below the body.
            for i in 1..=5 {
                let g = i as f32 * 1.7;
                let s = Rect::new(body.x - g, body.y - g + 2.5, body.w + 2.0 * g, body.h + 2.0 * g);
                self.gfx.fill_rrect(s, r.min(body.h / 2.0) + g, Color::black(if matches!(th.name, ThemeName::Light | ThemeName::Pastel) { 0.035 } else { 0.05 }));
            }
        }
        if let Some((c, k)) = th.glow.filter(|_| th.name != ThemeName::Neon) {
            // Ambient halo: soft strokes of the light around the outline.
            for (w, a) in [(14.0, 0.05), (8.0, 0.09), (3.5, 0.16)] {
                self.gfx.stroke_geo(&rim, c.fade(a * k.min(1.4)), w);
            }
        }
        if th.name == ThemeName::Neon {
            // Glow: wide, faint strokes of the accent under a crisp one.
            for (w, a) in [(12.0, 0.05), (7.0, 0.09), (3.5, 0.18)] {
                self.gfx.stroke_geo(&rim, th.accent.fade(a), w);
            }
        }
        match gradient_end(th.name) {
            Some(end) => self.gfx.fill_geo_gradient(&shape, (body.x, body.y), (body.right(), body.bottom()), th.island, end),
            None => self.gfx.fill_geo(&shape, th.island),
        }
        match th.name {
            ThemeName::Glass => {
                // Sheen across the top, and a rim that catches light from above.
                self.gfx.fill_geo_gradient(&shape, (body.x, body.y), (body.x, body.y + body.h * 0.6), Color::white(0.16), Color::white(0.0));
                self.gfx.stroke_geo_gradient(&rim, (body.x, body.y), (body.x, body.bottom()), th.border, Color::white(0.06), 1.0);
            }
            ThemeName::Neon => self.gfx.stroke_geo(&rim, th.accent.fade(0.9), 1.2),
            _ if th.border.a > 0.0 => self.gfx.stroke_geo(&rim, th.border, 1.0),
            _ => {}
        }
        Some(shape)
    }

    // ------------------------------------------------------------ open / close

    fn expand(&mut self) {
        self.expanded = true;
        self.cancel_hover();
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
            // Opening comes to the front once, over other always-on-top
            // windows (another island); it doesn't keep fighting for it.
            let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
        self.request_frame();
    }

    fn collapse(&mut self) {
        self.blur_text();
        self.expanded = false;
        self.pinned = false;
        self.left_at = self.now();
        self.request_frame();
    }

    fn free_to_collapse(&self) -> bool {
        // (Phone drop doesn't hold it open: files arriving show on the closed island.)
        !self.hovering && !self.pinned && !self.down && !self.menu_open && !self.drag_hover && !self.script_playing && !self.typing
    }

    // ------------------------------------------------------------ phone drop

    /// The phone panel shows (it holds the island open while you send).
    fn open_phone(&mut self) {
        self.close_phone();
        self.phone_panel = true;
        let now = self.now();
        self.phone_last = now;
        self.phone_got = 0;
        self.phone_seen = false;
        self.phone_progress = None;
        let sink = self.sink.clone();
        match PhoneDrop::start(sink) {
            Ok(p) => {
                self.phone_qr = crate::phone::qr(&p.url);
                self.phone_status = "Waiting for your phone".into();
                self.phone_bad = false;
                self.phone = Some(p);
            }
            Err(e) => {
                self.phone_status = e;
                self.phone_bad = true;
            }
        }
        self.request_frame();
    }

    fn close_phone(&mut self) {
        self.phone_panel = false;
        self.phone = None;
        self.phone_qr = None;
        self.phone_bad = false;
        self.phone_status.clear();
        self.phone_last = 0.0;
    }

    fn on_phone(&mut self, e: PhoneEvent, now: f64) {
        if self.phone.is_none() {
            return;
        }
        self.phone_last = now;
        match e {
            PhoneEvent::Connected => {
                self.phone_seen = true;
                if self.phone_got == 0 {
                    self.phone_status = "Phone connected. Pick some files".into();
                }
            }
            PhoneEvent::Receiving { name, done, total } => {
                self.phone_seen = true;
                self.phone_status = format!("Receiving {name}");
                let p = (done as f64 / total.max(1) as f64) as f32;
                self.phone_progress = Some(p);
                // Closed island: the transfer shows as a pop-up, like AirDrop.
                if !self.expanded {
                    self.show_hud(Hud::Transfer(name, p), 3.0);
                }
            }
            PhoneEvent::Saved(path) => {
                self.phone_got += 1;
                self.phone_progress = None;
                if !self.expanded {
                    self.show_hud(Hud::Transfer(crate::shelf::name(&path).to_string(), 1.0), 2.5);
                }
                self.phone_status = format!("Got {} file{}", self.phone_got, if self.phone_got == 1 { "" } else { "s" });
                self.shelf.add(vec![path.clone()]);
                self.shelf.save();
                self.want_thumb(&path);
            }
            PhoneEvent::Failed(why) => {
                self.phone_progress = None;
                self.phone_status = format!("Didn't arrive: {why}");
            }
        }
        self.phone_bad = false;
    }

    // ------------------------------------------------------------ shelf

    fn want_thumb(&mut self, path: &str) {
        if !self.thumb_imgs.contains_key(path) {
            if let Some(t) = &self.thumbs {
                t.request(path);
            }
        }
    }

    fn on_drag(&mut self, d: Drag) {
        match d {
            Drag::Enter => {
                // Files coming: open on the Shelf and stay open while they hover.
                self.drag_hover = true;
                if self.cfg.pages.contains(&PageKind::Shelf) {
                    self.page = PageKind::Shelf;
                }
                self.expand();
            }
            Drag::Leave => {
                self.drag_hover = false;
                self.hold_ended();
            }
            Drag::Drop(paths) => {
                self.drag_hover = false;
                let n = self.shelf.add(paths);
                self.shelf.save();
                for p in self.shelf.items.clone() {
                    self.want_thumb(&p);
                }
                if n == 0 {
                    self.notify("Already on the shelf", 1.5);
                }
                self.hold_ended();
            }
        }
        self.request_frame();
    }

    /// A file was dragged out: one that was moved away leaves the shelf.
    fn dragged_out(&mut self) {
        self.drag_hover = false;
        self.down = false;
        self.pressed = None;
        if self.shelf.prune() {
            self.shelf.save();
        }
        self.hold_ended();
        self.request_frame();
    }

    /// Something that held the island open ended: close soon unless the pointer is on it.
    fn hold_ended(&mut self) {
        if self.expanded && self.free_to_collapse() {
            unsafe {
                SetTimer(self.hwnd, TIMER_COLLAPSE, self.cfg.collapse_delay_ms.max(400), None);
            }
        }
    }

    /// The right-click menu took the pointer off the island, which must not
    /// close it. Once the menu is gone, close it shortly unless the pointer
    /// comes back (a move onto the island cancels the timer).
    fn menu_closed(&mut self) {
        self.menu_open = false;
        if self.expanded && !self.hovering && !self.pinned {
            unsafe {
                SetTimer(self.hwnd, TIMER_COLLAPSE, self.cfg.collapse_delay_ms.max(400), None);
            }
        }
    }

    fn arm_hover(&mut self, p: (f32, f32)) {
        self.hover_arm = Some(p);
        unsafe {
            SetTimer(self.hwnd, TIMER_HOVER, self.cfg.hover_delay_ms.max(1), None);
        }
    }

    fn cancel_hover(&mut self) {
        self.hover_arm = None;
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_HOVER);
        }
    }

    fn pointer_left(&mut self) {
        if !self.expanded || !self.free_to_collapse() {
            return;
        }
        match self.cfg.collapse_delay_ms {
            0 => {
                self.collapse_due = true;
                self.request_frame();
            }
            ms => unsafe {
                SetTimer(self.hwnd, TIMER_COLLAPSE, ms, None);
            },
        }
    }

    /// The hotkey and a tray click: open and pin, or close.
    fn toggle_pinned(&mut self) {
        if self.expanded && self.pinned {
            self.collapse();
        } else {
            self.pinned = true;
            self.expand();
        }
    }

    // ------------------------------------------------------------ events

    fn on_event(&mut self, e: Event) {
        let now = self.now();
        match e {
            Event::Media(info) => {
                let was = self.now_playing.take();
                if let Some(i) = &info {
                    let new_track = was.as_ref().map_or(true, |w| w.key != i.key);
                    if new_track && i.playing && !self.expanded {
                        self.peek_until = now + PEEK_S;
                    }
                    if new_track {
                        self.art_bmp = None;
                        self.art = None;
                        self.art_accent = None;
                        self.art_palette = None;
                        self.lyrics = None;
                        self.set_lyric(None, now);
                        if let Some(svc) = self.lyrics_svc.as_ref().filter(|_| self.cfg.lyrics) {
                            svc.request(Request { key: i.key.clone(), title: i.title.clone(), artist: i.artist.clone(), album: i.album.clone(), duration: i.duration });
                        }
                    }
                    if i.playing {
                        self.paused_at = None;
                    } else if self.paused_at.is_none() {
                        self.paused_at = Some(now);
                    }
                }
                self.now_playing = info;
            }
            Event::Art { key, img, accent } => {
                if self.now_playing.as_ref().is_some_and(|m| m.key == key) {
                    self.art_palette = img.as_ref().and_then(crate::ambient::from_image);
                    self.art = img;
                    self.art_bmp = None;
                    self.art_accent = accent;
                }
            }
            Event::Lyrics { key, lyrics } => {
                if self.now_playing.as_ref().is_some_and(|m| m.key == key) {
                    self.lyrics = lyrics;
                    self.lyric_heights.clear();
                    self.lyric_idx = None;
                    self.update_lyric(now);
                    self.lyric_scroll.snap(self.lyric_idx.map_or(0.0, |i| i as f32));
                }
            }
            Event::Phone(e) => self.on_phone(e, now),
            Event::ScreenPalette(p) => self.screen_palette = Some(p),
            Event::System(s) => {
                let mem = s.mem_used as f32 / s.mem_total.max(1) as f32;
                self.sys_hist.push_back([s.cpu, s.gpu.unwrap_or(0.0), mem, s.net_down as f32]);
                while self.sys_hist.len() > crate::system::HISTORY {
                    self.sys_hist.pop_front();
                }
                self.sys_snap = Some(*s);
            }
            Event::Located(r) => self.on_located(r),
            Event::ToolReady(r) => {
                self.tool = match r {
                    Ok(()) => Tool::Ready,
                    Err(e) => Tool::Failed(e),
                };
                self.pump_jobs();
            }
            Event::Download(ev) => self.on_download(ev),
            Event::Privacy(now_in_use) => {
                // Announce apps that just started; the dots show the rest.
                let fresh = now_in_use.iter().find(|u| !self.in_use.contains(u)).cloned();
                self.in_use = now_in_use;
                if let Some(u) = fresh.filter(|_| self.cfg.privacy && !self.expanded) {
                    self.show_hud(Hud::Privacy(u.sense, u.app), 3.0);
                }
            }
            Event::Thumb { path, img } => {
                // No picture: an empty entry, so it isn't asked for again.
                self.thumb_bmps.remove(&path);
                self.thumb_imgs.insert(path, img.unwrap_or(Image { w: 0, h: 0, px: Vec::new() }));
            }
            Event::Volume { level, muted } => {
                let mute_flip = muted != self.volume.1;
                let changed = (level - self.volume.0).abs() > 0.001 || mute_flip;
                self.volume = (level, muted);
                if mute_flip && !self.expanded && self.cfg.mute_hud {
                    // Muting is its own thing, like a phone's silent switch.
                    self.show_hud(Hud::Mute(muted), self.cfg.hud_seconds as f64);
                } else if changed && !self.expanded && self.cfg.volume_hud {
                    self.show_hud(Hud::Volume, self.cfg.hud_seconds as f64);
                }
            }
            Event::Bluetooth { name, connected, audio, battery } => {
                if self.cfg.bluetooth_hud && !self.expanded {
                    self.show_hud(Hud::Bluetooth { name, connected, audio, battery }, (self.cfg.hud_seconds as f64).max(3.0));
                }
            }
            Event::BluetoothBattery { name: who, battery: level } => {
                // The battery arrives a moment later: fill it into the pop-up still showing.
                if let Hud::Bluetooth { name, battery, .. } = &mut self.hud {
                    if *name == who && now < self.hud_until {
                        *battery = Some(level);
                        self.hud_until = self.hud_until.max(now + 2.0);
                    }
                }
            }
            Event::AudioDevice => {
                if let Some(v) = self.volume_svc.as_mut() {
                    v.attach();
                    if let Some(r) = v.read() {
                        self.volume = r;
                    }
                }
            }
            Event::ConfigChanged => self.reload(),
        }
    }

    // ------------------------------------------------------------ settings window

    fn open_settings(&mut self) {
        if let Some(w) = &self.settings_win {
            unsafe {
                let _ = ShowWindow(w.hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(w.hwnd);
            }
            return;
        }
        let accent = self.theme().accent;
        match crate::settings::SettingsWindow::open(self.gfx.scale, &self.cfg.font, sys::app_icon()) {
            Some(mut w) => {
                w.show(&self.cfg, accent);
                self.settings_win = Some(w);
            }
            None => self.notify("Couldn't open settings", 3.0),
        }
    }

    fn close_settings(&mut self) {
        if let Some(w) = self.settings_win.take() {
            w.close();
        }
    }

    fn repaint_settings(&mut self) {
        let accent = self.theme().accent;
        if let Some(w) = self.settings_win.as_mut() {
            w.paint(&self.cfg, accent);
        }
    }

    /// A setting changed (in the Settings window or on the island's
    /// Settings page): apply it, save it, and show it in both.
    fn change_setting(&mut self, k: crate::settings::Key, v: crate::settings::Val) {
        let mut c = self.cfg.clone();
        if crate::settings::change(&mut c, k, v) {
            self.apply(c, false);
            // Our own write comes back through the watcher: no notice for it.
            self.quiet_until = self.now() + 2.0;
            if let Err(e) = config::save(&self.cfg) {
                self.notify(&format!("Couldn't save settings: {e}"), 4.0);
            }
            self.repaint_settings();
            self.request_frame();
        }
    }

    fn handle_settings(&mut self, hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<isize> {
        use crate::settings::{Key, Outcome};
        if self.settings_win.as_ref().map(|w| w.hwnd) != Some(hwnd) {
            return None;
        }
        let accent = self.theme().accent;
        let (ret, out) = self.settings_win.as_mut()?.handle(msg, wp, lp, &self.cfg, accent)?;
        match out {
            Outcome::Nothing => {}
            Outcome::Close => self.close_settings(),
            Outcome::Open(k) => match k {
                Key::OpenConfig => self.open_config(),
                Key::OpenInbox => {
                    let dir = crate::phone::inbox();
                    let _ = std::fs::create_dir_all(&dir);
                    sys::launch(&dir.to_string_lossy(), "");
                }
                Key::Relocate => {
                    self.locate_error = None;
                    self.start_locate();
                }
                _ => sys::launch("%LOCALAPPDATA%\\Synapse", ""),
            },
            Outcome::Change(k, v) => self.change_setting(k, v),
        }
        Some(ret)
    }

    /// Switch to theme `t` now, and write its shape and colours to the config.
    fn pick_theme(&mut self, t: ThemeName) {
        let (style, colors) = t.preset();
        let mut c = self.cfg.clone();
        c.theme = t;
        c.style = style;
        c.colors = colors.clone();
        self.apply(c, false);
        // Our own write comes back through the watcher: no "Config reloaded" for it.
        self.quiet_until = self.now() + 2.0;
        let json = |v: serde_json::Result<serde_json::Value>| v.unwrap_or_default();
        let pairs = vec![("theme", json(serde_json::to_value(t))), ("style", json(serde_json::to_value(style))), ("colors", json(serde_json::to_value(colors)))];
        if let Err(e) = config::set_keys(pairs) {
            self.notify(&format!("Couldn't save the theme: {e}"), 4.0);
        }
    }

    fn show_hud(&mut self, h: Hud, secs: f64) {
        self.hud = h;
        self.hud_until = self.now() + secs;
        self.request_frame();
    }

    /// Windows says the power changed: plugged in, unplugged, or the battery moved.
    fn power_changed(&mut self) {
        let Some((pct, charging)) = sys::battery() else { return };
        let before = self.power.replace((pct, charging));
        if !self.cfg.power_hud || self.expanded {
            return;
        }
        let secs = (self.cfg.hud_seconds as f64).max(2.5);
        match before {
            Some((_, false)) if charging => self.show_hud(Hud::Charging(pct), secs),
            Some((was, _)) if !charging => {
                // Draining past a warning level (each one once on the way down).
                if LOW_BATTERY.iter().any(|&l| pct <= l && was > l) {
                    self.show_hud(Hud::LowBattery(pct), secs.max(4.0));
                }
            }
            _ => {}
        }
    }

    fn on_action(&mut self, a: Action, r: Rect, x: f32) {
        let media = |s: &Self, c: MediaCmd| {
            if let Some(m) = &s.media {
                m.send(c);
            }
        };
        match a {
            Action::Tab(p) => self.page = p,
            Action::Toggle => media(self, MediaCmd::Toggle),
            Action::Next => media(self, MediaCmd::Next),
            Action::Prev => media(self, MediaCmd::Prev),
            Action::Seek => {
                if let Some(m) = self.now_playing.as_mut() {
                    let at = ((x - r.x) / r.w).clamp(0.0, 1.0) as f64 * m.duration;
                    // Move the bar now; the player confirms a moment later.
                    m.position = at;
                    m.updated = sys::filetime_now();
                    media(self, MediaCmd::Seek(at));
                }
            }
            Action::Command(i) => {
                if let Some(c) = self.cfg.commands.get(i).cloned() {
                    sys::launch(&c.target, &c.args);
                    self.collapse();
                }
            }
            Action::OpenConfig => {
                self.open_config();
                self.collapse();
            }
            Action::Pin => self.pinned = !self.pinned,
            Action::ShelfOpen(i) => {
                if let Some(p) = self.shelf.items.get(i).cloned() {
                    sys::launch(&p, "");
                }
            }
            Action::ShelfRemove(i) => {
                if i < self.shelf.items.len() {
                    self.shelf.items.remove(i);
                    self.shelf.save();
                }
            }
            Action::ShelfClear => {
                self.shelf.items.clear();
                self.shelf.save();
            }
            Action::OpenThemes => self.page = if self.page == PageKind::Themes { self.cfg.pages[0] } else { PageKind::Themes },
            Action::Theme(t) => {
                self.pick_theme(t);
                self.repaint_settings();
            }
            Action::Addon(i) => {
                let a = &mut self.cfg.addons;
                let (flag, page) = match i {
                    0 => (&mut a.system, PageKind::System),
                    1 => (&mut a.caffeine, PageKind::Caffeine),
                    2 => (&mut a.prayer, PageKind::Prayer),
                    3 => (&mut a.downloader, PageKind::Downloads),
                    4 => (&mut a.script, PageKind::Script),
                    _ => return,
                };
                *flag = !*flag;
                let on = *flag;
                if !on && page == PageKind::System {
                    self.sysmon = None;
                    self.sys_hist.clear();
                }
                let c = self.cfg.clone();
                // The add-on's own start-up and clean-up happen in `apply`.
                let mut before = c.clone();
                match page {
                    PageKind::Downloads => before.addons.downloader = !on,
                    PageKind::Script => before.addons.script = !on,
                    _ => {}
                }
                self.cfg = before;
                self.apply(c, false);
                self.save_config_quietly();
                if on {
                    // Its tab appears; go and have a look.
                    self.page = page;
                }
            }
            Action::DrinkAdd(i) => {
                if let Some((name, mg)) = self.cfg.caffeine.drinks.get(i).cloned() {
                    self.caf.add(&name, mg, crate::caffeine::now_min());
                    self.caf.save();
                }
            }
            Action::DrinkUndo => {
                self.caf.undo();
                self.caf.save();
            }
            Action::Paste(audio) => self.paste_link(audio),
            Action::DlStop(id) => {
                if let Some(r) = self.running.get(&id) {
                    r.stop();
                }
                self.jobs.retain(|j| !(j.id == id && j.state == JobState::Queued));
            }
            Action::DlOpen(id) => {
                if let Some(JobState::Done(p)) = self.jobs.iter().find(|j| j.id == id).map(|j| j.state.clone()) {
                    sys::launch(&p, "");
                }
            }
            Action::DlFolder => sys::launch(&crate::phone::inbox().parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(), ""),
            Action::ToolRetry => self.ensure_tool(),
            Action::ScriptPaste => {
                if let Some(t) = sys::clipboard_text(self.hwnd).filter(|t| !t.trim().is_empty()) {
                    self.set_script(t);
                } else {
                    self.notify("Copy your script first, then Paste", 2.5);
                }
            }
            Action::ScriptEdit => {
                if !script_file().exists() {
                    let _ = std::fs::create_dir_all(config::dir());
                    let _ = std::fs::write(script_file(), &self.script);
                }
                let editor = if self.cfg.editor.trim().is_empty() { "notepad.exe".to_string() } else { self.cfg.editor.clone() };
                sys::launch(&editor, &format!("\"{}\"", script_file().display()));
            }
            Action::ScriptPlay => self.script_playing = !self.script_playing && !self.script.trim().is_empty(),
            Action::ScriptRestart => {
                self.script_pos = 0.0;
                self.script_playing = false;
            }
            Action::ScriptSpeed(d) => {
                self.cfg.script.speed = (self.cfg.script.speed + d as f32 * 4.0).clamp(5.0, 200.0);
                self.save_config_quietly();
            }
            Action::ScriptSize(d) => {
                self.cfg.script.size = (self.cfg.script.size + d as f32 * 2.0).clamp(14.0, 40.0);
                self.save_config_quietly();
            }
            Action::Locate => self.start_locate(),
            Action::LocationSettings => sys::launch("ms-settings:privacy-location", ""),
            Action::QuickSettings => {
                // The gear again goes back to the first tab.
                self.page = if self.page == PageKind::Settings { tabs_of(&self.cfg)[0] } else { PageKind::Settings };
            }
            Action::Set(k, v) => self.change_setting(k, v),
            Action::TaskFocus => self.focus_text(),
            Action::TaskAdd => {
                if self.draft.trim().is_empty() {
                    self.focus_text();
                } else {
                    self.add_task();
                }
            }
            Action::TaskToggle(id) => {
                self.tasks.toggle(id);
                self.tasks.save();
            }
            Action::TaskDelete(id) => {
                self.tasks.remove(id);
                self.tasks.save();
            }
            Action::TaskClearDone => {
                self.tasks.clear_done();
                self.tasks.save();
            }
            Action::OpenSettings => {
                // The open island would sit over the window: it closes as it opens.
                self.collapse();
                self.open_settings();
            }
            Action::PhoneOpen => self.open_phone(),
            Action::PhoneClose => {
                self.close_phone();
                self.hold_ended();
            }
            Action::FocusToggle => self.pomo.toggle(wall(), &self.cfg.focus),
            Action::FocusReset => self.pomo.reset(),
            Action::FocusSkip => {
                self.pomo.skip(wall(), &self.cfg.focus);
            }
        }
        self.request_frame();
    }

    // ------------------------------------------------------------ messages

    fn mouse_dip(&self, lp: LPARAM) -> (f32, f32) {
        let x = (lp.0 & 0xffff) as u16 as i16 as f32;
        let y = ((lp.0 >> 16) & 0xffff) as u16 as i16 as f32;
        (x / self.gfx.scale, y / self.gfx.scale)
    }

    fn hit_at(&self, p: (f32, f32)) -> Option<(Rect, Action)> {
        self.hits.iter().rev().find(|(r, _)| r.contains(p.0, p.1)).copied()
    }

    fn handle(&mut self, msg: u32, wp: WPARAM, lp: LPARAM, deferred: &mut Option<Deferred>) -> Option<LRESULT> {
        let now = self.now();
        match msg {
            WM_APP_FRAME => {
                self.frame();
                self.clock.frame_handled();
            }
            WM_APP_EVENT => {
                while let Ok(e) = self.rx.try_recv() {
                    self.on_event(e);
                }
                self.request_frame();
            }
            WM_TIMER if wp.0 == TIMER_DND => {
                if self.cfg.dnd_hud && !self.locked {
                    let on = dnd_on();
                    if on != self.dnd {
                        self.dnd = on;
                        if !self.expanded {
                            self.show_hud(Hud::Dnd(on), 2.0);
                        }
                    }
                }
            }
            WM_TIMER => {
                unsafe {
                    let _ = KillTimer(self.hwnd, wp.0);
                }
                match wp.0 {
                    TIMER_HOVER => {
                        if self.hovering && !self.expanded && self.cfg.open_on_hover {
                            self.expand();
                        }
                    }
                    TIMER_COLLAPSE => {
                        if self.expanded && self.free_to_collapse() {
                            self.collapse();
                        }
                    }
                    TIMER_WAKE => self.frame(),
                    _ => {}
                }
            }
            WM_MOUSEMOVE => {
                let p = self.mouse_dip(lp);
                self.mouse = Some(p);
                // A shelf file pulled away from its tile: drag it out.
                if let Some((p0, i)) = self.drag_from.filter(|_| self.down) {
                    if (p.0 - p0.0).hypot(p.1 - p0.1) > DRAG_SLOP {
                        self.drag_from = None;
                        self.pressed = None;
                        unsafe {
                            let _ = ReleaseCapture();
                        }
                        if let Some(path) = self.shelf.items.get(i).cloned() {
                            *deferred = Some(Deferred::DragOut(vec![path]));
                        }
                        return Some(LRESULT(0));
                    }
                }
                if !self.hovering {
                    self.hovering = true;
                    self.collapse_due = false;
                    unsafe {
                        let mut tme = TRACKMOUSEEVENT { cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32, dwFlags: TME_LEAVE, hwndTrack: self.hwnd, dwHoverTime: 0 };
                        let _ = TrackMouseEvent(&mut tme);
                        let _ = KillTimer(self.hwnd, TIMER_COLLAPSE);
                    }
                    if !self.expanded && self.cfg.open_on_hover {
                        if now - self.left_at < REOPEN_S {
                            self.expand();
                        } else {
                            self.arm_hover(p);
                        }
                    }
                } else if let Some(p0) = self.hover_arm.filter(|_| !self.expanded) {
                    if (p.0 - p0.0).hypot(p.1 - p0.1) > DWELL_SLOP {
                        self.arm_hover(p);
                    }
                }
                self.request_frame();
            }
            WM_MOUSELEAVE => {
                self.hovering = false;
                self.mouse = None;
                self.cancel_hover();
                self.pointer_left();
                self.request_frame();
            }
            WM_LBUTTONDOWN => {
                let p = self.mouse_dip(lp);
                self.down = true;
                self.pressed = self.hit_at(p);
                // A click anywhere but the field (or Add) gives the keyboard back.
                if self.typing && !matches!(self.pressed, Some((_, Action::TaskFocus | Action::TaskAdd))) {
                    self.blur_text();
                }
                self.drag_from = match self.pressed {
                    Some((_, Action::ShelfOpen(i))) => Some((p, i)),
                    _ => None,
                };
                unsafe { SetCapture(self.hwnd) };
                self.request_frame();
            }
            WM_LBUTTONUP => {
                let p = self.mouse_dip(lp);
                unsafe {
                    let _ = ReleaseCapture();
                }
                let was_down = std::mem::take(&mut self.down);
                self.drag_from = None;
                if !self.expanded {
                    if was_down {
                        // Open on what the pill was showing.
                        let page = match self.mode {
                            Mode::Media => Some(PageKind::Media),
                            Mode::Timer => Some(PageKind::Focus),
                            Mode::Prayer => Some(PageKind::Prayer),
                            Mode::Download => Some(PageKind::Downloads),
                            _ => None,
                        };
                        if let Some(p) = page.filter(|p| tabs_of(&self.cfg).contains(p)) {
                            self.page = p;
                        }
                        self.notice = None;
                        self.expand();
                    }
                } else if let (Some((r, a)), Some((_, b))) = (self.pressed, self.hit_at(p)) {
                    if a == b {
                        self.on_action(a, r, p.0);
                    }
                }
                self.pressed = None;
                if !self.hovering {
                    self.pointer_left();
                }
                self.request_frame();
            }
            WM_RBUTTONUP => *deferred = Some(Deferred::Menu),
            WM_SETCURSOR => {
                let hand = self.mouse.and_then(|p| self.hit_at(p)).is_some();
                unsafe {
                    if let Ok(c) = LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }) {
                        SetCursor(c);
                    }
                }
                return Some(LRESULT(1));
            }
            WM_MOUSEACTIVATE => return Some(LRESULT(if self.typing { MA_ACTIVATE } else { MA_NOACTIVATE } as isize)),
            WM_KEYDOWN if self.typing => {
                if !self.on_key(wp.0 as u32) {
                    return None;
                }
            }
            WM_CHAR if self.typing => {
                let u = wp.0 as u16;
                match u {
                    // Ctrl+V: paste (text only, one line).
                    0x16 => {
                        if let Some(t) = sys::clipboard_text(self.hwnd) {
                            self.insert_text(&t.replace(['\r', '\n'], " "));
                        }
                    }
                    0xD800..=0xDBFF => self.high_surrogate = Some(u),
                    0xDC00..=0xDFFF => {
                        if let Some(hi) = self.high_surrogate.take() {
                            let s = String::from_utf16_lossy(&[hi, u]);
                            self.insert_text(&s);
                        }
                    }
                    c if c >= 0x20 => {
                        if let Some(ch) = char::from_u32(c as u32) {
                            self.insert_text(&ch.to_string());
                        }
                    }
                    _ => {}
                }
                self.request_frame();
            }
            WM_KILLFOCUS if self.typing => {
                // Another window took the keyboard: stop typing (keep what's typed).
                self.typing = false;
                unsafe {
                    let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
                    SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
                }
                self.hold_ended();
                self.request_frame();
            }
            WM_MOUSEWHEEL => {
                // The Tasks list scrolls a row a notch.
                if self.mode == Mode::Expanded(PageKind::Tasks) {
                    let delta = ((wp.0 >> 16) & 0xffff) as u16 as i16;
                    let rows = (delta / 120).signum();
                    self.tasks_scroll = if rows > 0 { self.tasks_scroll.saturating_sub(1) } else { self.tasks_scroll + 1 };
                    self.request_frame();
                }
            }
            WM_HOTKEY if wp.0 as i32 == sys::HOTKEY_SCRIPT => {
                // Open on the script (pinned, so it stays while you read) and start or pause.
                if self.page != PageKind::Script || !self.expanded {
                    self.page = PageKind::Script;
                    self.pinned = true;
                    self.expand();
                }
                self.script_playing = !self.script_playing && !self.script.trim().is_empty();
                self.request_frame();
            }
            WM_HOTKEY => self.toggle_pinned(),
            WM_APP_TRAY => match (lp.0 & 0xffff) as u32 {
                WM_LBUTTONUP => self.toggle_pinned(),
                WM_RBUTTONUP | WM_CONTEXTMENU => *deferred = Some(Deferred::Menu),
                _ => {}
            },
            WM_APP_APPBAR => {
                if wp.0 as u32 == ABN_FULLSCREENAPP {
                    self.fullscreen = lp.0 != 0;
                    self.request_frame();
                }
            }
            WM_WTSSESSION_CHANGE => {
                match wp.0 {
                    7 => self.locked = true,
                    8 => {
                        self.locked = false;
                        if self.cfg.unlock_hud && !self.expanded {
                            self.show_hud(Hud::Unlocked, 1.8);
                        }
                    }
                    _ => {}
                }
                self.request_frame();
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE => self.layout(),
            WM_POWERBROADCAST => {
                if wp.0 as u32 == PBT_APMPOWERSTATUSCHANGE {
                    self.power_changed();
                }
                return Some(LRESULT(1));
            }
            WM_DESTROY => {
                self.close_settings();
                shelf::stop_drops(self.hwnd);
                unsafe {
                    let mut abd = APPBARDATA { cbSize: std::mem::size_of::<APPBARDATA>() as u32, hWnd: self.hwnd, ..Default::default() };
                    SHAppBarMessage(ABM_REMOVE, &mut abd);
                }
                self.clock.set_active(false);
                self.tray = None;
                unsafe { PostQuitMessage(0) };
            }
            m if m == self.taskbar_created && m != 0 => {
                if let Some(t) = &self.tray {
                    t.add();
                }
            }
            _ => return None,
        }
        Some(LRESULT(0))
    }

    // ------------------------------------------------------------ frame

    fn frame(&mut self) {
        let now = self.now();
        let resumed = self.clock.take_resumed();
        let mut dt = if resumed || self.last_t == 0.0 { 1.0 / 60.0 } else { now - self.last_t };
        self.last_t = now;
        if self.notice.as_ref().is_some_and(|n| now >= n.until) {
            self.notice = None;
        }
        if std::mem::take(&mut self.collapse_due) && self.expanded && self.free_to_collapse() {
            self.collapse();
        }
        if let Some(ended) = self.pomo.tick(wall(), &self.cfg.focus) {
            self.phase_ended(ended);
        }
        // Phone drop forgotten open: stop listening and let the island close.
        if self.phone_panel && now - self.phone_last >= PHONE_IDLE_S {
            self.close_phone();
            self.hold_ended();
        }
        if self.cfg.hide_in_fullscreen && self.fullscreen && !self.expanded {
            // Out of the way over a game or video: an empty, click-through frame.
            let _ = self.gfx.begin();
            self.gfx.end();
            self.gfx.present(self.hwnd, self.win_x, self.win_y);
            self.hits.clear();
            self.update_ambient(0.0, now);
            self.draw_edge();
            self.schedule(now);
            return;
        }

        self.update_lyric(now);
        self.update_addons();
        let target = self.target_mode(now);
        let mut retarget = false;
        if target != self.mode {
            let new_content = target.key() != self.mode.key();
            // Turned around mid-fade: the content coming back picks up where it is.
            let back = self.outgoing.filter(|o| new_content && o.0.key() == target.key()).map(|o| o.3 * out_left(now - o.1));
            if new_content {
                self.outgoing = Some((self.mode, now, self.island.target(), self.island.content_alpha()));
            }
            if let (Mode::Expanded(a), Mode::Expanded(b)) = (self.mode, target) {
                let idx = |p| tabs_of(&self.cfg).iter().position(|q| *q == p).unwrap_or(0);
                self.page_anim = Some((a, now, if idx(b) > idx(a) { 1.0 } else { -1.0 }));
            }
            let g = self.geom_for(target, now);
            self.island.set_target(g, now, new_content);
            if let Some(a) = back {
                self.island.resume_content(a);
            }
            self.mode = target;
            retarget = true;
        } else {
            let g = self.geom_for(self.mode, now);
            if g != self.island.target() {
                self.island.set_target(g, now, false);
                retarget = true;
            }
        }
        if retarget {
            dt = dt.min(1.0 / 60.0);
        }
        self.island.step(dt.clamp(0.0, 0.05) as f32, now);
        self.lyric_scroll.step(dt.clamp(0.0, 0.05) as f32);
        self.step_script(dt.clamp(0.0, 0.05) as f32);
        self.step_eyes(dt.clamp(0.0, 0.05) as f32, now);
        self.update_ambient(dt.clamp(0.0, 0.05) as f32, now);
        self.draw(now);
        self.draw_edge();
        self.schedule(now);
    }

    // ------------------------------------------------------------ add-ons

    /// Each frame: the System reader runs only while its page shows; the
    /// Prayer add-on finds where you are once and announces each prayer.
    fn update_addons(&mut self) {
        let want_sys = self.cfg.addons.system && self.mode == Mode::Expanded(PageKind::System) && !self.locked;
        if want_sys && self.sysmon.is_none() {
            self.sysmon = Some(crate::system::SystemMonitor::start(self.sink.clone()));
        }
        if let Some(m) = &self.sysmon {
            m.set(want_sys);
        }
        if !self.cfg.addons.prayer {
            return;
        }
        if self.cfg.prayer.lat.is_none() && !self.locating && self.locate_error.is_none() {
            self.start_locate();
        }
        self.prayer_tick();
    }

    // ------------------------------------------------------------ downloader

    /// Get yt-dlp if it isn't here yet.
    fn ensure_tool(&mut self) {
        if crate::downloader::have_ytdlp() {
            self.tool = Tool::Ready;
        } else if self.tool != Tool::Fetching {
            self.tool = Tool::Fetching;
            crate::downloader::fetch_ytdlp(self.sink.clone());
        }
    }

    /// Queue the link on the clipboard.
    fn paste_link(&mut self, audio: bool) {
        let link = sys::clipboard_text(self.hwnd).map(|t| t.trim().to_string()).unwrap_or_default();
        if !(link.starts_with("https://") || link.starts_with("http://")) || link.contains(char::is_whitespace) {
            self.notify("Copy a link first (it starts with https://)", 3.0);
            return;
        }
        let id = self.next_job;
        self.next_job += 1;
        self.jobs.push(Job { id, url: link.clone(), audio, title: link, pct: 0.0, speed: String::new(), eta: String::new(), state: JobState::Queued });
        // Only the last few stay listed.
        while self.jobs.len() > 6 {
            if let Some(i) = self.jobs.iter().position(|j| !matches!(j.state, JobState::Running | JobState::Queued)) {
                self.jobs.remove(i);
            } else {
                break;
            }
        }
        self.ensure_tool();
        self.pump_jobs();
    }

    /// Start queued downloads while there's room.
    fn pump_jobs(&mut self) {
        if self.tool != Tool::Ready {
            return;
        }
        let out = crate::phone::inbox().parent().map(|p| p.to_path_buf()).unwrap_or_default();
        while self.running.len() < crate::downloader::PARALLEL {
            let Some(j) = self.jobs.iter_mut().find(|j| j.state == JobState::Queued) else { break };
            match crate::downloader::start(j.id, &j.url, j.audio, &out, self.sink.clone()) {
                Ok(r) => {
                    j.state = JobState::Running;
                    self.running.insert(j.id, r);
                }
                Err(e) => j.state = JobState::Failed(e),
            }
        }
    }

    fn on_download(&mut self, ev: crate::downloader::DlEvent) {
        use crate::downloader::DlEvent;
        let id = match &ev {
            DlEvent::Title { id, .. } | DlEvent::Progress { id, .. } | DlEvent::Done { id, .. } | DlEvent::Failed { id, .. } => *id,
        };
        let Some(j) = self.jobs.iter_mut().find(|j| j.id == id) else { return };
        let mut finished = None;
        match ev {
            DlEvent::Title { title, .. } => j.title = title,
            DlEvent::Progress { pct, speed, eta, .. } => {
                j.pct = pct;
                j.speed = speed;
                j.eta = eta;
            }
            DlEvent::Done { path, .. } => {
                j.pct = 1.0;
                j.state = JobState::Done(path.clone());
                finished = Some(Some(path));
            }
            DlEvent::Failed { why, .. } => {
                if !matches!(j.state, JobState::Done(_)) {
                    j.state = JobState::Failed(why);
                }
                finished = Some(None);
            }
        }
        if let Some(saved) = finished {
            self.running.remove(&id);
            if let Some(path) = saved {
                // A finished download waits on the Shelf too.
                self.shelf.add(vec![path.clone()]);
                self.shelf.save();
                self.want_thumb(&path);
                if !self.expanded {
                    self.notify("Download finished: it's on the Shelf", 3.0);
                }
            }
            self.pump_jobs();
        }
    }

    /// The downloads' overall progress, while any run.
    fn download_progress(&self) -> Option<f32> {
        let active: Vec<&Job> = self.jobs.iter().filter(|j| matches!(j.state, JobState::Running | JobState::Queued)).collect();
        (!active.is_empty()).then(|| active.iter().map(|j| j.pct).sum::<f32>() / active.len() as f32)
    }

    // ------------------------------------------------------------ script reader

    fn set_script(&mut self, text: String) {
        let _ = std::fs::create_dir_all(config::dir());
        let _ = std::fs::write(script_file(), &text);
        self.script = text;
        self.script_pos = 0.0;
        self.script_playing = false;
        self.script_heights.clear();
    }

    /// The eyes: blink every few seconds (sometimes twice), glance about,
    /// and while the pointer is on the island, watch it.
    fn step_eyes(&mut self, dt: f32, now: f64) {
        if !(self.mode == Mode::Idle && self.cfg.idle == Idle::Eyes) {
            return;
        }
        if now >= self.eyes.next_blink {
            self.eyes.blink_at = now;
            let double = self.eyes.rand() < 0.2;
            self.eyes.next_blink = now + if double { 0.3 } else { 3.0 + 4.0 * self.eyes.rand() as f64 };
        }
        if now >= self.eyes.next_glance {
            let r = self.eyes.rand();
            self.eyes.glance_to = if r < 0.45 { 0.0 } else { (self.eyes.rand() * 2.0 - 1.0).signum() * (0.5 + 0.5 * self.eyes.rand()) };
            self.eyes.next_glance = now + 3.5 + 6.0 * self.eyes.rand() as f64;
        }
        let target = match self.mouse.filter(|_| self.hovering) {
            Some((mx, _)) => ((mx - self.cx) / 40.0).clamp(-1.0, 1.0),
            None => self.eyes.glance_to,
        };
        self.eyes.look.target = target;
        self.eyes.look.step(dt);
    }

    /// A pop-up that moves by itself: the lock springing open (its first
    /// half second), the low battery pulsing (while it shows).
    fn hud_animating(&self, now: f64) -> bool {
        if !matches!(self.mode, Mode::Hud(_)) || now >= self.hud_until {
            return false;
        }
        match self.hud {
            Hud::Unlocked => now > self.hud_until - 1.8 && now < self.hud_until - 1.2,
            Hud::LowBattery(_) => true,
            _ => false,
        }
    }

    /// The eyes are mid-blink or turning (frames needed).
    fn eyes_moving(&self) -> bool {
        self.mode == Mode::Idle && self.cfg.idle == Idle::Eyes && (self.now() - self.eyes.blink_at < 0.18 || !self.eyes.look.settled())
    }

    /// Scroll while reading (only while its page shows), stopping at the end;
    /// keep the island out of screen capture while the Script page is open.
    fn step_script(&mut self, dt: f32) {
        let shown = self.cfg.addons.script && self.mode == Mode::Expanded(PageKind::Script);
        if self.script_playing {
            if shown {
                self.script_pos += self.cfg.script.speed * dt;
                let total: f32 = self.script_heights.iter().sum();
                if !self.script_heights.is_empty() && self.script_pos >= total {
                    self.script_pos = total;
                    self.script_playing = false;
                }
            } else {
                self.script_playing = false;
            }
        }
        let hide = shown && self.cfg.script.hide_from_capture;
        if hide != self.capture_hidden {
            self.capture_hidden = hide;
            let affinity = if hide { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE };
            unsafe {
                if SetWindowDisplayAffinity(self.hwnd, affinity).is_err() {
                    crate::log("script: Windows wouldn't keep the island out of screen capture");
                }
            }
        }
    }

    /// Each paragraph's height at the reader's width and size (measured once).
    fn script_layout(&mut self, w: f32) {
        let size = self.cfg.script.size;
        if self.script_measured == (w, size) && !self.script_heights.is_empty() {
            return;
        }
        self.script_measured = (w, size);
        let paras: Vec<String> = self.script.lines().map(str::to_string).collect();
        self.script_heights = paras.iter().map(|p| if p.trim().is_empty() { size * 0.7 } else { self.gfx.measure_wrapped(p, size, 650, w) + size * 0.35 }).collect();
    }

    fn start_locate(&mut self) {
        if self.locating {
            return;
        }
        self.locating = true;
        self.locate_error = None;
        let sink = self.sink.clone();
        std::thread::spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            sink.send(Event::Located(crate::prayer::locate()));
        });
    }

    fn on_located(&mut self, r: Result<(f64, f64), String>) {
        self.locating = false;
        match r {
            Ok((lat, lon)) => {
                let (name, _, km) = crate::prayer::nearest(lat, lon);
                self.cfg.prayer.lat = Some(lat);
                self.cfg.prayer.lon = Some(lon);
                self.cfg.prayer.place = if km < 80.0 { name.to_string() } else { format!("{lat:.2}, {lon:.2}") };
                self.save_config_quietly();
                self.prayer_told = None;
            }
            Err(e) => self.locate_error = Some(e),
        }
    }

    fn save_config_quietly(&mut self) {
        self.quiet_until = self.now() + 2.0;
        if let Err(e) = config::save(&self.cfg) {
            self.notify(&format!("Couldn't save: {e}"), 4.0);
        }
        self.repaint_settings();
    }

    /// Where the prayers are worked out for, and that place's country.
    fn prayer_site(&self) -> Option<(f64, f64, &'static str)> {
        let (lat, lon) = (self.cfg.prayer.lat?, self.cfg.prayer.lon?);
        Some((lat, lon, crate::prayer::nearest(lat, lon).1))
    }

    /// The method and Asr rule: yours, or your country's.
    fn prayer_rules(&self, country: &str) -> (crate::prayer::Method, bool) {
        let p = &self.cfg.prayer;
        let m = if p.method == "auto" { crate::prayer::method_for(country) } else { p.method.as_str() };
        let hanafi = match p.asr.as_str() {
            "hanafi" => true,
            "standard" => false,
            _ => crate::prayer::hanafi_for(country),
        };
        (crate::prayer::method(m), hanafi)
    }

    fn prayer_times(&self) -> Option<crate::prayer::Times> {
        let (lat, lon, cc) = self.prayer_site()?;
        let (m, hanafi) = self.prayer_rules(cc);
        let t = sys::local_time();
        Some(crate::prayer::compute(t.year as i32, t.month as i32, t.day as i32, lat, lon, crate::prayer::utc_offset_h(), m, hanafi))
    }

    fn now_hours() -> f64 {
        let t = sys::local_time();
        t.hour as f64 + t.minute as f64 / 60.0 + (t.second as f64 + t.millis as f64 / 1000.0) / 3600.0
    }

    /// The next prayer, its time (hours after today's midnight; tomorrow's
    /// Fajr past 24) and the minutes until it.
    fn next_prayer(&self) -> Option<(crate::prayer::Prayer, f64, f64)> {
        use crate::prayer::Prayer;
        let t = self.prayer_times()?;
        let now = Self::now_hours();
        let next = Prayer::ALL.into_iter().filter(|p| p.is_prayer()).map(|p| (p, t.get(p))).find(|(_, h)| *h > now);
        let (p, h) = next.unwrap_or((Prayer::Fajr, t.get(Prayer::Fajr) + 24.0));
        Some((p, h, (h - now) * 60.0))
    }

    /// The closed island counts down to the next prayer in its last minutes.
    fn prayer_pill(&self) -> Option<(crate::prayer::Prayer, f64)> {
        if !self.cfg.addons.prayer || self.cfg.prayer.remind_min == 0 {
            return None;
        }
        let (p, _, mins) = self.next_prayer()?;
        (mins <= self.cfg.prayer.remind_min as f64).then_some((p, mins))
    }

    /// Announce a prayer as its time comes (not ones that passed before Synapse started).
    fn prayer_tick(&mut self) {
        let Some(t) = self.prayer_times() else { return };
        let now = Self::now_hours();
        let day = crate::caffeine::now_min().div_euclid(1440);
        let passed = crate::prayer::Prayer::ALL.into_iter().filter(|p| p.is_prayer() && t.get(*p) <= now).last();
        let Some(p) = passed else { return };
        let key = (day, p);
        match self.prayer_told {
            None => self.prayer_told = Some(key),
            Some(k) if k != key => {
                self.prayer_told = Some(key);
                if now - t.get(p) < 3.0 / 60.0 {
                    self.notify(&format!("Time for {}  \u{00B7}  {}", p.label(), crate::prayer::fmt(t.get(p))), 10.0);
                    if self.cfg.prayer.sound {
                        unsafe {
                            let _ = PlaySoundW(w!("SystemNotification"), None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------ ambient light

    fn hidden_fs(&self) -> bool {
        self.cfg.hide_in_fullscreen && self.fullscreen && !self.expanded
    }

    /// Music with album art is what the light comes from (in Auto).
    fn art_lights(&self, now: f64) -> bool {
        self.media_shown(now) && self.art_palette.is_some()
    }

    /// The palette the light is heading for.
    fn ambient_target(&self, now: f64) -> Option<crate::ambient::Palette> {
        use crate::config::AmbientSource;
        let a = &self.cfg.ambient;
        if !a.enabled {
            return None;
        }
        match a.source {
            AmbientSource::Art => self.art_palette.filter(|_| self.media_shown(now)),
            AmbientSource::Screen => self.screen_palette,
            AmbientSource::Auto if self.art_lights(now) => self.art_palette,
            AmbientSource::Auto => self.screen_palette,
        }
    }

    /// Ease the light towards its source, follow the music's loudness, keep
    /// the screen sampler running only while it's the source and someone's
    /// looking, and hand the colour to OpenRGB.
    fn update_ambient(&mut self, dt: f32, now: f64) {
        use crate::config::AmbientSource;
        let a = self.cfg.ambient.clone();
        let screen_wanted = a.enabled
            && !self.locked
            && !self.hidden_fs()
            && match a.source {
                AmbientSource::Screen => true,
                AmbientSource::Auto => !self.art_lights(now),
                AmbientSource::Art => false,
            };
        if screen_wanted && self.sampler.is_none() {
            self.sampler = Some(crate::ambient::ScreenSampler::start(self.sink.clone()));
        }
        if let Some(s) = &self.sampler {
            // Below the top strip, where the island and its glow live.
            let mut area = self.mon_rect;
            area.top += (140.0 * self.gfx.scale) as i32;
            s.set(screen_wanted, area);
        }
        if !screen_wanted && self.screen_palette.is_some() && !matches!(a.source, AmbientSource::Screen | AmbientSource::Auto) {
            self.screen_palette = None;
        }

        let target = self.ambient_target(now);
        self.pal_now = match (self.pal_now, target) {
            (Some(cur), Some(t)) => Some(cur.toward(&t, 1.0 - (-dt / 0.45).exp())),
            (None, t) => t,
            (Some(_), None) => None,
        };

        let playing = self.now_playing.as_ref().is_some_and(|m| m.playing);
        let raw = if a.enabled && a.breathe && playing && !self.locked { self.volume_svc.as_ref().map_or(0.0, |v| v.peak()) } else { 0.0 };
        // Quick to rise, slow to fall: it breathes rather than flickers.
        let k = if raw > self.level { 0.55 } else { 0.12 };
        self.level += (raw - self.level) * k;
        if self.level < 0.002 {
            self.level = 0.0;
        }

        if a.enabled && a.openrgb {
            if self.openrgb.is_none() {
                self.openrgb = Some(crate::openrgb::OpenRgb::start(a.openrgb_port));
                self.rgb_sent = None;
            }
            if let (Some(p), Some(o)) = (target, &self.openrgb) {
                // Full brightness for the keys: the colour, not its shade.
                let m = p.mix();
                let hi = m.r.max(m.g).max(m.b).max(0.01);
                let rgb = [(m.r / hi * 255.0) as u8, (m.g / hi * 255.0) as u8, (m.b / hi * 255.0) as u8];
                let changed = self.rgb_sent.map_or(true, |s| s.iter().zip(rgb).any(|(x, y)| x.abs_diff(y) > 6));
                if changed {
                    o.set(rgb);
                    self.rgb_sent = Some(rgb);
                }
            }
        } else if self.openrgb.is_some() {
            self.openrgb = None;
        }
    }

    /// The light on the screen's top edge, or none.
    fn draw_edge(&mut self) {
        let a = &self.cfg.ambient;
        let pal = self.pal_now.filter(|_| a.enabled && a.edge_glow && !self.hidden_fs() && !self.locked);
        let Some(p) = pal else {
            if let Some(e) = self.edge.as_mut() {
                e.hide();
            }
            return;
        };
        let s = self.gfx.scale;
        if self.edge.is_none() {
            let rc = self.mon_rect;
            self.edge = crate::edge::EdgeGlow::new(rc.left, rc.top, rc.right - rc.left, s, &self.cfg.font);
        }
        let cx = (self.win_x as f32 + self.cx * s - self.mon_rect.left as f32) / s;
        let strength = a.intensity * self.breath_gain();
        if let Some(e) = self.edge.as_mut() {
            e.draw(p, cx, strength, self.hwnd);
        }
    }

    /// A Focus phase ran out: say what's next, with a chime.
    fn phase_ended(&mut self, ended: Phase) {
        let c = self.cfg.focus;
        let mins = |p: Phase| (focus::duration(p, &c) / 60.0).round() as i64;
        let next = self.pomo.phase;
        let text = match (ended, self.pomo.running()) {
            (Phase::Focus, true) => format!("Focus done. {} for {} min", next.label(), mins(next)),
            (Phase::Focus, false) => format!("Focus done. Time for a {} min {}", mins(next), next.label().to_lowercase()),
            (_, true) => format!("Break over. Focus for {} min", mins(Phase::Focus)),
            (_, false) => "Break over. Ready to focus?".to_string(),
        };
        self.notify(&text, 6.0);
        if c.sound {
            unsafe {
                let _ = PlaySoundW(w!("SystemNotification"), None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT);
            }
        }
    }

    /// Park the frame clock unless something moves, and wake once for the
    /// next change you'd see (a minute, a HUD going away, the equalizer).
    fn schedule(&mut self, now: f64) {
        let hidden = self.cfg.hide_in_fullscreen && self.fullscreen && !self.expanded;
        let media_page = self.mode == Mode::Expanded(PageKind::Media);
        let scrolling = media_page && self.lyrics_shown() && !self.lyric_scroll.settled();
        let fading_line = self.mode == Mode::Media && self.sings() && now - self.lyric_at < 0.3;
        // The light easing to a new colour.
        let shifting = match (self.pal_now, self.ambient_target(now)) {
            (Some(c), Some(t)) => c.distance(&t) > 0.004,
            _ => false,
        };
        let moving = !hidden && (!self.island.settled() || self.outgoing.is_some() || self.page_anim.is_some() || scrolling || fading_line || shifting || self.script_playing || self.eyes_moving() || self.hud_animating(now));
        self.clock.set_active(moving);
        unsafe {
            let _ = KillTimer(self.hwnd, TIMER_WAKE);
        }
        if moving {
            return;
        }
        let mut next = f64::INFINITY;
        let mut at = |t: f64| {
            if t > now {
                next = next.min(t);
            }
        };
        let playing = self.now_playing.as_ref().is_some_and(|m| m.playing);
        let watched = !self.locked && !hidden;
        if watched && playing && self.mode == Mode::Media {
            at(now + 1.0 / 30.0);
        }
        // The glow breathing with the music, while there's a glow to see.
        let a = &self.cfg.ambient;
        if watched && playing && a.enabled && a.breathe && self.pal_now.is_some() {
            at(now + 1.0 / 30.0);
        }
        // Settling after the music stops.
        if self.level > 0.0 {
            at(now + 1.0 / 30.0);
        }
        if watched && playing && self.mode == Mode::Expanded(PageKind::Media) {
            at(now + 0.25);
        }
        // The next sung line, exactly when it starts.
        if watched && (self.mode == Mode::Media || media_page) {
            if let (Some(l), Some(m)) = (&self.lyrics, self.now_playing.as_ref().filter(|m| m.playing)) {
                let pos = self.lyric_pos(m);
                if let Some(t) = l.next_after(pos) {
                    at(now + (t - pos) + 0.005);
                }
            }
        }
        // The Focus timer: its end always (even unseen), each second while you can see it.
        if let Some(end) = self.pomo.ends_at() {
            let w = wall();
            at(now + (end - w).max(0.0) + 0.01);
            let seen = self.mode == Mode::Timer || self.mode == Mode::Expanded(PageKind::Focus);
            if watched && seen {
                let left = end - w;
                at(now + (left - left.floor()).max(0.0) + 0.005);
            }
        }
        if self.phone_panel {
            at(self.phone_last + PHONE_IDLE_S);
        }
        // The eyes wake for their next blink and glance (one-shot, nothing in between).
        if watched && self.mode == Mode::Idle && self.cfg.idle == Idle::Eyes {
            at(self.eyes.next_blink);
            at(self.eyes.next_glance);
        }
        // Prayer: when the countdown starts, each minute of it, and the time itself.
        if let Some((_, _, mins)) = self.next_prayer().filter(|_| self.cfg.addons.prayer) {
            let remind = self.cfg.prayer.remind_min as f64;
            if mins > remind && remind > 0.0 {
                at(now + (mins - remind) * 60.0 + 0.2);
            } else if watched {
                at(now + (mins - mins.floor()) * 60.0 + 0.05);
            }
            at(now + mins * 60.0 + 0.5);
        }
        // The Caffeine page's "now" moves on; once a minute is plenty.
        if watched && self.mode == Mode::Expanded(PageKind::Caffeine) {
            at(now + 60.0);
        }
        if let Some(n) = &self.notice {
            at(n.until);
        }
        at(self.hud_until);
        at(self.peek_until);
        if let Some(p) = self.paused_at {
            at(p + MEDIA_GRACE);
        }
        let clock_page = self.mode == Mode::Expanded(PageKind::Clock);
        let clock_idle = self.mode == Mode::Idle && self.cfg.idle == Idle::Clock;
        if watched && (clock_page || clock_idle) {
            let t = sys::local_time();
            let to_second = 1.0 - t.millis as f64 / 1000.0;
            if clock_page || self.cfg.show_seconds {
                at(now + to_second + 0.01);
            } else {
                at(now + (59 - t.second.min(59)) as f64 + to_second + 0.02);
            }
        }
        if next.is_finite() {
            let ms = (((next - now) * 1000.0).ceil() as u32).clamp(10, 3_600_000);
            unsafe {
                SetTimer(self.hwnd, TIMER_WAKE, ms, None);
            }
        }
    }

    fn draw(&mut self, now: f64) {
        if self.gfx.begin().is_err() {
            return;
        }
        self.hits.clear();
        let th = self.theme();
        let g = self.island.current();
        let top = self.top_for(self.mode);
        let body = Rect::new(self.cx - g.w / 2.0, top, g.w, g.h);
        let hidden = self.hidden_rest(self.mode) && self.island.settled();

        if self.expanded {
            // Nearly invisible, but it takes the mouse: a forgiving edge.
            let r = Rect::new(body.x - SHOULDER - MARGIN, 0.0, body.w + 2.0 * (SHOULDER + MARGIN), body.bottom() + MARGIN);
            self.gfx.fill_rect(r, Color::black(1.0 / 255.0));
        }
        let hangs = self.cfg.style == Style::Notch && top == 0.0;
        let shape = if hidden {
            // Invisible, but it catches the pointer at the top edge.
            let s = self.gfx.rrect_geo(body, g.r);
            if let Some(s) = &s {
                self.gfx.fill_geo(s, Color::black(1.0 / 255.0));
            }
            s
        } else {
            self.paint_island(body, g.r, hangs, &th)
        };
        let Some(shape) = shape else {
            self.gfx.end();
            return;
        };
        // Glass: Windows blurs what's behind the island's outline.
        if self.cfg.theme == ThemeName::Glass && !hidden && !self.hidden_fs() {
            if self.backdrop.is_none() {
                self.backdrop = crate::backdrop::Backdrop::new();
            }
            let s = self.gfx.scale;
            let px = |v: f32| (v * s).round() as i32;
            let radius = if hangs { g.r } else { g.r.min(body.h / 2.0) };
            let screen = (self.win_x + px(body.x), self.win_y + px(body.y), px(body.w), px(body.h));
            if let Some(b) = self.backdrop.as_mut() {
                b.show(screen, px(radius), hangs, self.hwnd);
            }
        } else if let Some(b) = self.backdrop.as_mut() {
            b.hide();
        }

        if !hidden {
            if self.art_bmp.is_none() {
                self.art_bmp = self.art.as_ref().and_then(|i| self.gfx.bitmap(i));
            }
            let morphing = !self.island.settled() || self.outgoing.is_some() || self.page_anim.is_some();
            if morphing {
                self.gfx.push_mask(&shape);
            }
            let tgt = self.island.target();
            let pivot = match self.cfg.style {
                Style::Notch => (body.cx(), body.y),
                Style::Pill => (body.cx(), body.y + body.h.min(tgt.h) / 2.0),
            };
            // The leaving content shrinks with the outline as it fades.
            if let Some((m, t0, og, from)) = self.outgoing {
                let age = now - t0;
                if age >= OUT_FADE {
                    self.outgoing = None;
                } else {
                    let s = (g.w / og.w.max(1.0)).clamp(0.6, 1.0);
                    self.base_xf = scale_about(pivot.0, pivot.1, s, 0.0, 0.0);
                    self.gfx.set_transform(&self.base_xf);
                    let ob = Rect::new(body.cx() - og.w / 2.0, self.top_for(m), og.w, og.h);
                    self.draw_mode(m, ob, from * out_left(age), false, now, &th);
                }
            }
            // The arriving content grows and settles as it fades in.
            let a = self.island.content_alpha();
            let drop = if self.cfg.style == Style::Pill { 2.0 } else { 6.0 };
            self.base_xf = scale_about(pivot.0, pivot.1, 0.94 + 0.06 * a, 0.0, -drop * (1.0 - a));
            self.gfx.set_transform(&self.base_xf);
            // Open pages lay out at their final size; the outline reveals them.
            let lay = match self.mode {
                Mode::Expanded(_) => Rect::new(body.cx() - tgt.w / 2.0, top, tgt.w, tgt.h),
                _ => body,
            };
            self.draw_mode(self.mode, lay, a, true, now, &th);
            self.gfx.set_transform(&identity());
            if morphing {
                self.gfx.pop();
            }
        }
        // Privacy: a small bubble beside the closed island with a dot per
        // sense in use (camera green, microphone orange, location blue).
        if self.cfg.privacy && !self.expanded && !self.in_use.is_empty() {
            let senses: Vec<crate::privacy::Sense> = crate::privacy::Sense::ALL.into_iter().filter(|s| self.in_use.iter().any(|u| u.sense == *s)).collect();
            let h = 18.0f32.min(body.h);
            let w = h + (senses.len() as f32 - 1.0) * 10.0;
            let bubble = Rect::new(body.right() + 6.0, body.cy() - h / 2.0, w, h);
            let mut fill = th.island;
            fill.a = fill.a.max(0.85);
            self.gfx.fill_rrect(bubble, h / 2.0, fill);
            for (i, s) in senses.into_iter().enumerate() {
                self.gfx.fill_circle(bubble.x + h / 2.0 + i as f32 * 10.0, bubble.cy(), 3.6, sense_color(s));
            }
        }
        self.gfx.end();
        if std::mem::take(&mut self.gfx.lost) {
            self.art_bmp = None;
            self.thumb_bmps.clear();
        }
        self.gfx.present(self.hwnd, self.win_x, self.win_y);
        if !self.shown {
            self.shown = true;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
                let _ = SetWindowPos(self.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    fn draw_mode(&mut self, m: Mode, r: Rect, alpha: f32, live: bool, now: f64, th: &Theme) {
        self.gfx.alpha = alpha;
        self.live = live;
        match m {
            Mode::Idle => self.draw_idle(r, th),
            Mode::Media => self.draw_media_pill(r, now, th),
            Mode::Hud(_) => match self.hud.clone() {
                Hud::Volume => self.draw_hud(r, th),
                Hud::Charging(p) => self.draw_power_hud(r, p, true, th),
                Hud::LowBattery(p) => self.draw_power_hud(r, p, false, th),
                Hud::Bluetooth { name, connected, audio, battery } => self.draw_bt_hud(r, &name, connected, audio, battery, th),
                Hud::Unlocked => self.draw_unlock_hud(r, th),
                Hud::Mute(m) => {
                    let (g, col, t) = if m { (G_MUTE, Color::rgba(1.0, 0.27, 0.23, 1.0), "Muted") } else { ('\u{E767}', th.text, "Sound on") };
                    self.status_hud(r, g, col, t, th);
                }
                Hud::Dnd(on) => {
                    let col = Color::rgba(0.62, 0.48, 1.0, 1.0);
                    self.status_hud(r, '\u{E708}', if on { col } else { th.dim }, if on { "Do Not Disturb on" } else { "Do Not Disturb off" }, th);
                }
                Hud::Privacy(sense, app) => {
                    let (cx, cy) = (r.x + 22.0, r.cy());
                    self.gfx.fill_circle(cx, cy, 5.5, sense_color(sense));
                    let t = format!("{app} is using your {}", sense.noun());
                    self.gfx.text(&t, Rect::new(r.x + 38.0, r.y, r.w - 52.0, r.h), 13.0, 600, th.text, Align::Left);
                }
                Hud::Transfer(name, p) => self.draw_transfer_hud(r, &name, p, th),
            },
            Mode::Notice => self.draw_notice(r, th),
            Mode::Timer => self.draw_timer_pill(r, th),
            Mode::Download => {
                let p = self.download_progress().unwrap_or(1.0);
                let (cx, cy) = (r.x + 19.0, r.cy());
                self.gfx.ring(cx, cy, 8.0, 2.4, th.ink(0.18));
                self.gfx.arc(cx, cy, 8.0, p, 2.4, th.accent);
                self.gfx.icon('\u{E896}', Rect::new(cx - 8.0, cy - 8.0, 16.0, 16.0), 8.0, th.text);
                self.gfx.text(&format!("{:.0}%", p * 100.0), Rect::new(r.x + 34.0, r.y, r.w - 46.0, r.h), 13.5, 650, th.text, Align::Right);
            }
            Mode::Prayer => {
                if let Some((p, mins)) = self.prayer_pill() {
                    self.gfx.icon('\u{E708}', Rect::new(r.x + 10.0, r.y, 28.0, r.h), 14.0, th.accent);
                    let left = if mins < 1.0 { "now".to_string() } else { format!("in {} min", mins.ceil() as i64) };
                    self.gfx.text(&format!("{} {left}", p.label()), Rect::new(r.x + 40.0, r.y, r.w - 54.0, r.h), 13.5, 650, th.text, Align::Left);
                }
            }
            Mode::Expanded(p) => self.draw_expanded(p, r, alpha, now, th),
        }
        self.gfx.alpha = 1.0;
    }

    fn hit(&mut self, r: Rect, a: Action) {
        if self.live {
            self.hits.push((r, a));
        }
    }

    fn hovered(&self, r: Rect) -> bool {
        self.live && self.mouse.is_some_and(|(x, y)| r.contains(x, y))
    }

    fn clock_text(&self, seconds: bool) -> String {
        let t = sys::local_time();
        let h = if self.cfg.clock_24h { t.hour } else { (t.hour + 11) % 12 + 1 };
        let mut s = if self.cfg.clock_24h { format!("{:02}:{:02}", h, t.minute) } else { format!("{}:{:02}", h, t.minute) };
        if seconds {
            s += &format!(":{:02}", t.second);
        }
        s
    }

    fn draw_idle(&mut self, r: Rect, th: &Theme) {
        match self.cfg.idle {
            Idle::Clock => {
                let s = self.clock_text(self.cfg.show_seconds);
                self.gfx.text(&s, r, 14.0, 600, th.text, Align::Center);
            }
            Idle::Eyes => self.draw_eyes(r, th),
            _ => {}
        }
    }

    /// Two eyes: they blink, glance about now and then, and watch the
    /// pointer while it's on the island.
    fn draw_eyes(&mut self, r: Rect, th: &Theme) {
        let now = self.now();
        let open = blink_open(now - self.eyes.blink_at);
        let look = self.eyes.look.value;
        let (w, h) = (8.5, 14.0 * open.max(0.12));
        for side in [-1.0f32, 1.0] {
            let cx = r.cx() + side * 13.0 + look * 3.5;
            let e = Rect::new(cx - w / 2.0, r.cy() - h / 2.0, w, h);
            self.gfx.fill_rrect(e, w / 2.0, th.text);
        }
    }

    fn position_now(m: &MediaInfo) -> f64 {
        let mut p = m.position;
        if m.playing && m.updated > 0 {
            p += (sys::filetime_now() - m.updated) as f64 / 1e7;
        }
        if m.duration > 0.0 {
            p.clamp(0.0, m.duration)
        } else {
            p.max(0.0)
        }
    }

    /// Where the lyrics are in the track: the player's position with your offset.
    fn lyric_pos(&self, m: &MediaInfo) -> f64 {
        Self::position_now(m) - self.cfg.lyrics_offset_ms as f64 / 1000.0
    }

    fn lyric_line(&self) -> Option<&str> {
        self.lyrics.as_ref()?.lines.get(self.lyric_idx?).map(|l| l.text.as_str())
    }

    /// The closed pill sings: a line with words is being sung right now.
    fn sings(&self) -> bool {
        self.cfg.lyrics && self.cfg.lyrics_in_pill && self.lyric_w > 0.0 && self.now_playing.as_ref().is_some_and(|m| m.playing)
    }

    fn set_lyric(&mut self, idx: Option<usize>, now: f64) {
        self.lyric_idx = idx;
        self.lyric_at = now;
        let text = self.lyric_line().unwrap_or("").to_string();
        self.lyric_w = if text.is_empty() { 0.0 } else { self.gfx.measure(&text, 13.0, 600) };
        // Before the singing starts, the first line waits in the middle.
        let to = idx.map_or(0.0, |i| i as f32);
        // A seek far away jumps; the next line scrolls.
        if (to - self.lyric_scroll.target).abs() > 3.0 {
            self.lyric_scroll.snap(to);
        } else {
            self.lyric_scroll.target = to;
        }
    }

    /// Follow the song: move to the line being sung now.
    fn update_lyric(&mut self, now: f64) {
        let idx = match (&self.lyrics, &self.now_playing) {
            (Some(l), Some(m)) => l.index_at(self.lyric_pos(m)),
            _ => None,
        };
        if idx != self.lyric_idx {
            self.set_lyric(idx, now);
        }
    }

    fn draw_art(&mut self, r: Rect, radius: f32, glyph_size: f32, th: &Theme) {
        if let Some(b) = self.art_bmp.clone() {
            self.gfx.image(&b, r, radius);
        } else {
            self.gfx.fill_rrect(r, radius, th.ink(0.08));
            self.gfx.icon(G_NOTE, r, glyph_size, th.dim);
        }
    }

    fn draw_media_pill(&mut self, r: Rect, now: f64, th: &Theme) {
        let Some(m) = self.now_playing.clone() else { return };
        let size = (r.h - 12.0).max(10.0);
        let art = Rect::new(r.x + 7.0, r.cy() - size / 2.0, size, size);
        self.draw_art(art, size * 0.28, 12.0, th);
        // Four bars dancing while it plays, resting low while paused.
        let (bw, gap) = (3.0, 2.5);
        let x0 = r.right() - 14.0 - (4.0 * bw + 3.0 * gap);
        for i in 0..4 {
            let f = [5.3, 7.1, 6.2, 8.4][i];
            let ph = [0.0, 1.7, 3.1, 4.4][i];
            let hgt = if m.playing { 4.0 + 10.0 * (0.5 + 0.5 * (now * f + ph).sin() as f32) * (0.6 + 0.4 * (now * f * 0.37 + ph).cos().abs() as f32) } else { 3.0 };
            let bar = Rect::new(x0 + i as f32 * (bw + gap), r.cy() - hgt / 2.0, bw, hgt);
            self.gfx.fill_rrect(bar, bw / 2.0, th.accent);
        }
        let tr = Rect::new(art.right() + 10.0, r.y, x0 - art.right() - 20.0, r.h);
        let saved = self.gfx.alpha;
        if self.peeking(now) {
            // A new track's title, once the pill is wide enough to hold it.
            self.gfx.alpha *= ((r.w - 230.0) / 90.0).clamp(0.0, 1.0);
            let title = if m.artist.is_empty() { m.title.clone() } else { format!("{}  ·  {}", m.title, m.artist) };
            self.gfx.text(&title, tr, 13.0, 600, th.text, Align::Left);
        } else if self.sings() {
            // The line being sung, fading in as the pill reaches its width.
            let line = self.lyric_line().unwrap_or("").to_string();
            self.gfx.alpha *= smoothstep(((now - self.lyric_at) / 0.28) as f32);
            self.gfx.text(&line, tr, 13.0, 600, th.text, Align::Center);
        }
        self.gfx.alpha = saved;
    }

    fn draw_hud(&mut self, r: Rect, th: &Theme) {
        let (level, muted) = self.volume;
        let glyph = if muted || level <= 0.005 { G_MUTE } else { G_VOL[1 + ((level * 3.0).ceil() as usize).clamp(1, 3) - 1] };
        self.gfx.icon(glyph, Rect::new(r.x + 12.0, r.y, 30.0, r.h), 16.0, th.text);
        let bar = Rect::new(r.x + 50.0, r.cy() - 3.0, r.w - 50.0 - 62.0, 6.0);
        self.gfx.fill_rrect(bar, 3.0, th.ink(0.16));
        let fill = if muted { 0.0 } else { level.clamp(0.0, 1.0) };
        if fill > 0.0 {
            self.gfx.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * fill).max(6.0), bar.h), 3.0, th.accent);
        }
        let pct = if muted { "Muted".to_string() } else { format!("{}%", (level * 100.0).round() as i32) };
        self.gfx.text(&pct, Rect::new(r.right() - 60.0, r.y, 46.0, r.h), 13.0, 600, th.text, Align::Right);
    }

    /// A small battery, `pct` full, its right end at `right`. Returns where it starts.
    fn draw_battery(&mut self, right: f32, cy: f32, pct: u8, fill: Color, th: &Theme) -> f32 {
        let body = Rect::new(right - 25.0, cy - 6.0, 22.0, 12.0);
        if let Some(g) = self.gfx.rrect_geo(body, 3.5) {
            self.gfx.stroke_geo(&g, th.ink(0.45), 1.2);
        }
        self.gfx.fill_rrect(Rect::new(body.right() + 1.2, cy - 2.5, 2.0, 5.0), 1.0, th.ink(0.45));
        let inner = body.inset(2.2, 2.2);
        let w = (inner.w * pct.min(100) as f32 / 100.0).max(1.5);
        self.gfx.fill_rrect(Rect::new(inner.x, inner.y, w, inner.h), 1.8, fill);
        body.x
    }

    /// A status pop-up: an icon and a few words, centred.
    fn status_hud(&mut self, r: Rect, glyph: char, col: Color, text: &str, th: &Theme) {
        let tw = self.gfx.measure(text, 13.5, 650);
        let x = r.cx() - (tw + 30.0) / 2.0;
        self.gfx.icon(glyph, Rect::new(x, r.y, 22.0, r.h), 14.0, col);
        self.gfx.text(text, Rect::new(x + 30.0, r.y, tw + 4.0, r.h), 13.5, 650, th.text, Align::Left);
    }

    /// Unlocked: a padlock whose shackle springs open.
    fn draw_unlock_hud(&mut self, r: Rect, th: &Theme) {
        let t = (self.now() - (self.hud_until - 1.8)).clamp(0.0, 1.0) as f32;
        // Lift with a little overshoot, settling by ~0.45 s.
        let e = 1.0 - (-9.0 * t).exp() * (10.0 * t).cos();
        let (cx, cy) = (r.x + 30.0, r.cy() + 3.0);
        let body = Rect::new(cx - 8.0, cy - 4.0, 16.0, 12.0);
        let lift = 5.0 * e;
        // The shackle: an arch over the body, its right leg lifting free.
        let arch: Vec<(f32, f32)> = (0..=12)
            .map(|i| {
                let a = std::f32::consts::PI * (1.0 - i as f32 / 12.0);
                (cx + 5.0 * a.cos(), cy - 6.0 - lift - 5.0 * a.sin())
            })
            .collect();
        let mut pts = vec![(cx - 5.0, cy - 4.0)];
        pts.extend(arch);
        pts.push((cx + 5.0, cy - 6.0 - lift + 2.0));
        self.gfx.polyline(&pts, th.text, 2.2);
        self.gfx.fill_rrect(body, 3.0, th.accent);
        self.gfx.text("Unlocked", Rect::new(r.x + 52.0, r.y, r.w - 64.0, r.h), 13.5, 650, th.text, Align::Left);
    }

    /// A file arriving from the phone: a progress ring, its name, then a tick.
    fn draw_transfer_hud(&mut self, r: Rect, name: &str, p: f32, th: &Theme) {
        let (cx, cy) = (r.x + 24.0, r.cy());
        let blue = Color::rgba(0.04, 0.52, 1.0, 1.0);
        if p >= 1.0 {
            self.gfx.fill_circle(cx, cy, 12.0, blue);
            self.gfx.icon('\u{E73E}', Rect::new(cx - 12.0, cy - 12.0, 24.0, 24.0), 11.0, Color::rgba(1.0, 1.0, 1.0, 1.0));
        } else {
            self.gfx.ring(cx, cy, 11.0, 2.6, th.ink(0.18));
            self.gfx.arc(cx, cy, 11.0, p, 2.6, blue);
            self.gfx.icon(G_PHONE, Rect::new(cx - 11.0, cy - 11.0, 22.0, 22.0), 10.0, th.text);
        }
        let x = r.x + 46.0;
        self.gfx.text(if p >= 1.0 { "Received from your phone" } else { "Receiving from your phone" }, Rect::new(x, r.y + 5.0, r.w - 60.0, 17.0), 11.0, 650, th.dim, Align::Left);
        self.gfx.text(name, Rect::new(x, r.y + 21.0, r.w - 60.0, 18.0), 13.0, 650, th.text, Align::Left);
    }

    fn draw_power_hud(&mut self, r: Rect, pct: u8, charging: bool, th: &Theme) {
        let red = Color::rgba(1.0, 0.27, 0.23, 1.0);
        let (mut col, glyph, label) = if charging {
            // Green while there's charge to spare, red when it's low (as on a phone).
            (if pct > 20 { Color::rgba(0.19, 0.82, 0.35, 1.0) } else { red }, '\u{E945}', "Charging")
        } else {
            (red, '\u{E7BA}', "Low battery")
        };
        if !charging {
            // Low battery throbs.
            let s = (self.now() * 4.0).sin() as f32;
            col = col.fade(0.6 + 0.4 * s.abs());
        }
        self.gfx.icon(glyph, Rect::new(r.x + 12.0, r.y, 30.0, r.h), 15.0, col);
        self.gfx.text(label, Rect::new(r.x + 46.0, r.y, 140.0, r.h), 13.0, 600, th.text, Align::Left);
        let x = self.draw_battery(r.right() - 16.0, r.cy(), pct, col, th);
        self.gfx.text(&format!("{pct}%"), Rect::new(x - 56.0, r.y, 48.0, r.h), 13.0, 600, if charging { th.text } else { col }, Align::Right);
    }

    fn draw_bt_hud(&mut self, r: Rect, name: &str, connected: bool, audio: bool, battery: Option<u8>, th: &Theme) {
        let c = Rect::new(r.x + 9.0, r.cy() - 15.0, 30.0, 30.0);
        self.gfx.fill_circle(c.cx(), c.cy(), 15.0, if connected { th.accent } else { th.ink(0.14) });
        let glyph = if audio { '\u{E7F6}' } else { '\u{E702}' };
        self.gfx.icon(glyph, c, 14.0, if connected { Color::black(0.9) } else { th.dim });
        let right = if battery.is_some() { r.right() - 96.0 } else { r.right() - 16.0 };
        let tx = c.right() + 11.0;
        self.gfx.text(name, Rect::new(tx, r.y + 5.0, right - tx, 19.0), 13.0, 600, th.text, Align::Left);
        self.gfx.text(if connected { "Connected" } else { "Disconnected" }, Rect::new(tx, r.y + 23.0, right - tx, 16.0), 11.0, 500, th.dim, Align::Left);
        if let Some(b) = battery {
            let col = if b <= 20 { Color::rgba(1.0, 0.27, 0.23, 1.0) } else { th.ink(0.9) };
            let x = self.draw_battery(r.right() - 18.0, r.cy(), b, col, th);
            self.gfx.text(&format!("{b}%"), Rect::new(x - 54.0, r.y, 46.0, r.h), 13.0, 600, th.text, Align::Right);
        }
    }

    fn draw_notice(&mut self, r: Rect, th: &Theme) {
        let Some(text) = self.notice.as_ref().map(|n| n.text.clone()) else { return };
        self.gfx.icon(G_INFO, Rect::new(r.x + 12.0, r.y, 28.0, r.h), 15.0, th.accent);
        self.gfx.text(&text, Rect::new(r.x + 44.0, r.y, r.w - 60.0, r.h), 13.0, 500, th.text, Align::Left);
    }

    fn draw_expanded(&mut self, page: PageKind, r: Rect, alpha: f32, now: f64, th: &Theme) {
        // Header: a tab per page as an icon, the open one with its name too;
        // then the theme, settings and pin buttons on the right.
        let (mut x, hy, th_h) = (r.x + PAD - 6.0, r.y + 12.0, 28.0);
        for p in tabs_of(&self.cfg) {
            let open = p == page;
            let w = if open { self.gfx.measure(p.label(), 13.0, 650) + 44.0 } else { 34.0 };
            let tab = Rect::new(x, hy, w, th_h);
            if open {
                self.gfx.fill_rrect(tab, th_h / 2.0, th.ink(0.14));
                self.gfx.icon(page_glyph(p), Rect::new(tab.x + 6.0, tab.y, 22.0, th_h), 14.0, th.text);
                self.gfx.text(p.label(), Rect::new(tab.x + 30.0, tab.y, w - 36.0, th_h), 13.0, 650, th.text, Align::Left);
            } else {
                if self.hovered(tab) {
                    self.gfx.fill_rrect(tab, th_h / 2.0, th.ink(0.07));
                }
                self.gfx.icon(page_glyph(p), tab, 14.0, th.dim);
            }
            self.hit(tab, Action::Tab(p));
            x += w + 3.0;
        }
        let pin = Rect::new(r.right() - PAD - 28.0, hy, 28.0, 28.0);
        let gear = Rect::new(pin.x - 32.0, hy, 28.0, 28.0);
        let palette = Rect::new(gear.x - 32.0, hy, 28.0, 28.0);
        let on_themes = page == PageKind::Themes;
        for (b, glyph, act, on) in [
            (palette, '\u{E790}', Action::OpenThemes, on_themes),
            (gear, G_GEAR, Action::QuickSettings, page == PageKind::Settings),
            (pin, if self.pinned { G_PINNED } else { G_PIN }, Action::Pin, self.pinned),
        ] {
            if self.hovered(b) {
                self.gfx.fill_circle(b.cx(), b.cy(), 14.0, th.ink(0.1));
            }
            self.gfx.icon(glyph, b, 14.0, if on { th.accent } else { th.dim });
            self.hit(b, act);
        }

        let area = Rect::new(r.x + PAD, r.y + HEADER, r.w - 2.0 * PAD, self.page_h(page));
        let anim = self.page_anim.and_then(|(from, t0, dir)| {
            let t = ((now - t0) / PAGE_SWITCH) as f32;
            (t < 1.0).then_some((from, smoothstep(t), dir))
        });
        if anim.is_none() && self.live {
            self.page_anim = None;
        }
        match anim {
            Some((from, e, dir)) if from != page => {
                let live = self.live;
                let from_area = Rect::new(area.x, area.y, area.w, self.page_h(from));
                self.live = false;
                self.page_at(from, from_area, alpha * (1.0 - e), -dir * 24.0 * e, now, th);
                self.live = live;
                self.page_at(page, area, alpha * e, dir * 24.0 * (1.0 - e), now, th);
            }
            _ => self.page_at(page, area, alpha, 0.0, now, th),
        }
    }

    fn page_at(&mut self, p: PageKind, area: Rect, alpha: f32, dx: f32, now: f64, th: &Theme) {
        let mut xf = self.base_xf;
        xf.M31 += dx * xf.M11;
        self.gfx.set_transform(&xf);
        self.gfx.alpha = alpha;
        match p {
            PageKind::Media => self.page_media(area, th),
            PageKind::Shelf => self.page_shelf(area, th),
            PageKind::Focus => self.page_focus(area, th),
            PageKind::Themes => self.page_themes(area, th),
            PageKind::Addons => self.page_addons(area, th),
            PageKind::System => self.page_system(area, th),
            PageKind::Caffeine => self.page_caffeine(area, th),
            PageKind::Prayer => self.page_prayer(area, th),
            PageKind::Downloads => self.page_downloads(area, th),
            PageKind::Script => self.page_script(area, now, th),
            PageKind::Settings => self.page_quick_settings(area, th),
            PageKind::Tasks => self.page_tasks(area, th),
            PageKind::Clock => self.page_clock(area, now, th),
            PageKind::Commands => self.page_commands(area, th),
        }
        self.gfx.set_transform(&self.base_xf);
    }

    /// Now Playing: the art, the track and its controls, and the lyrics in a
    /// column beside them (when the song has synced lyrics).
    fn page_media(&mut self, r: Rect, th: &Theme) {
        let Some(m) = self.now_playing.clone() else {
            self.gfx.icon(G_NOTE, Rect::new(r.x, r.y + 24.0, r.w, 36.0), 26.0, th.dim);
            self.gfx.text("Nothing playing", Rect::new(r.x, r.y + 64.0, r.w, 22.0), 15.0, 600, th.text, Align::Center);
            self.gfx.text("Play something in any app and it shows up here", Rect::new(r.x, r.y + 86.0, r.w, 18.0), 12.0, 400, th.dim, Align::Center);
            return;
        };
        let art = Rect::new(r.x, r.y, ART, ART);
        self.draw_art(art, 14.0, 34.0, th);

        // The lyrics take the right of the page; the player keeps the middle.
        let lyrics = self.lyrics_shown();
        let lyr_w = if lyrics { (r.w * LYRICS_SHARE).round() } else { 0.0 };
        let lyr_x = r.right() - lyr_w;
        let x = art.right() + GAP_ART;
        let right = if lyrics { lyr_x - GAP_LYRICS } else { r.right() };
        let cw = right - x;

        self.gfx.text(&m.title, Rect::new(x, r.y - 3.0, cw, 24.0), 16.5, 700, th.text, Align::Left);
        self.gfx.text(&m.artist, Rect::new(x, r.y + 20.0, cw, 19.0), 13.0, 500, th.dim, Align::Left);
        if !m.app.is_empty() {
            self.gfx.text(&m.app.to_uppercase(), Rect::new(x, r.y + 40.0, cw, 15.0), 10.5, 650, th.dim.fade(0.7), Align::Left);
        }
        if m.duration > 0.0 {
            let pos = Self::position_now(&m);
            let frac = (pos / m.duration).clamp(0.0, 1.0) as f32;
            let zone = Rect::new(x, r.y + 60.0, cw, 18.0);
            let hot = self.hovered(zone);
            let bh = if hot { 6.0 } else { 4.0 };
            let bar = Rect::new(x, zone.cy() - bh / 2.0, cw, bh);
            self.gfx.fill_rrect(bar, bh / 2.0, th.ink(0.16));
            self.gfx.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * frac).max(bh), bh), bh / 2.0, th.accent);
            if hot {
                self.gfx.fill_circle(bar.x + bar.w * frac, bar.cy(), 6.0, th.text);
            }
            self.hit(zone, Action::Seek);
            self.gfx.text(&fmt_time(pos), Rect::new(x, r.y + 77.0, 60.0, 14.0), 11.0, 550, th.dim, Align::Left);
            self.gfx.text(&format!("-{}", fmt_time(m.duration - pos)), Rect::new(right - 60.0, r.y + 77.0, 60.0, 14.0), 11.0, 550, th.dim, Align::Right);
        }
        // Back, play / pause (on its own disc), next.
        let (cy, mid) = (r.y + 121.0, x + cw / 2.0);
        let play = Rect::new(mid - 22.0, cy - 22.0, 44.0, 44.0);
        let buttons = [
            (Rect::new(play.x - 52.0, cy - 19.0, 38.0, 38.0), G_PREV, 17.0, Action::Prev, m.can_prev),
            (play, if m.playing { G_PAUSE } else { G_PLAY }, 20.0, Action::Toggle, true),
            (Rect::new(play.right() + 14.0, cy - 19.0, 38.0, 38.0), G_NEXT, 17.0, Action::Next, m.can_next),
        ];
        for (b, glyph, size, act, enabled) in buttons {
            let hot = enabled && self.hovered(b);
            if act == Action::Toggle {
                self.gfx.fill_circle(b.cx(), b.cy(), b.h / 2.0, th.ink(if hot { 0.2 } else { 0.12 }));
            } else if hot {
                self.gfx.fill_circle(b.cx(), b.cy(), b.h / 2.0, th.ink(0.1));
            }
            self.gfx.icon(glyph, b, size, if enabled { th.text } else { th.text.fade(0.3) });
            if enabled {
                self.hit(b, act);
            }
        }
        if lyrics {
            self.gfx.fill_rect(Rect::new(lyr_x - GAP_LYRICS / 2.0, r.y + 6.0, 1.0, r.h - 12.0), th.ink(0.08));
            self.draw_lyrics(Rect::new(lyr_x, r.y - 4.0, lyr_w, r.h + 8.0), th);
        }
    }

    /// How tall lyric line `i` is in a column `w` wide (wrapped), cached.
    fn lyric_height(&mut self, i: usize, current: bool, w: f32) -> f32 {
        if (self.lyric_cache_w - w).abs() > 0.5 {
            self.lyric_heights.clear();
            self.lyric_cache_w = w;
        }
        if let Some(h) = self.lyric_heights.get(&(i, current)) {
            return *h;
        }
        let text = self.lyrics.as_ref().and_then(|l| l.lines.get(i)).map(|l| lyric_text(&l.text).to_string()).unwrap_or_default();
        let (size, weight) = lyric_font(current);
        let h = self.gfx.measure_wrapped(&text, size, weight, w) + LYRIC_GAP;
        self.lyric_heights.insert((i, current), h);
        h
    }

    /// The lyrics around the line being sung, wrapped to the column and
    /// scrolling as the song goes: the current line larger and bright, the
    /// others fading towards the column's ends.
    fn draw_lyrics(&mut self, area: Rect, th: &Theme) {
        let Some(n) = self.lyrics.as_ref().map(|l| l.lines.len()) else { return };
        if n == 0 {
            return;
        }
        let Some(mask) = self.gfx.rrect_geo(area, 0.0) else { return };
        self.gfx.push_mask(&mask);
        let s = self.lyric_scroll.value.clamp(0.0, (n - 1) as f32);
        let base = s.floor() as usize;
        let frac = s - base as f32;
        let w = area.w;
        let cur = self.lyric_idx;
        // Each line's top relative to the line at `base`.
        let first = base.saturating_sub(ROWS_ABOVE);
        let last = (base + ROWS_BELOW).min(n - 1);
        let mut tops = vec![0.0f32; last - first + 1];
        for i in (first..base).rev() {
            let h = self.lyric_height(i, cur == Some(i), w);
            tops[i - first] = tops[i + 1 - first] - h;
        }
        for i in base + 1..=last {
            let h = self.lyric_height(i - 1, cur == Some(i - 1), w);
            tops[i - first] = tops[i - 1 - first] + h;
        }
        let shift = frac * self.lyric_height(base, cur == Some(base), w);
        let anchor = area.y + area.h * 0.36;
        let saved = self.gfx.alpha;
        for i in first..=last {
            let current = cur == Some(i);
            let top = anchor + tops[i - first] - shift;
            let h = self.lyric_height(i, current, w);
            // Fade with distance from the anchor, and softly at the column's ends.
            let dist = (top + h / 2.0 - anchor).abs();
            let a = if current { 1.0 } else { (1.0 - dist / (area.h * 0.75)).clamp(0.0, 1.0) };
            let edge = ((top + h - area.y) / 22.0).min((area.bottom() - top) / 22.0).clamp(0.0, 1.0);
            self.gfx.alpha = saved * a * edge;
            let text = self.lyrics.as_ref().map(|l| lyric_text(&l.lines[i].text).to_string()).unwrap_or_default();
            let (size, weight) = lyric_font(current);
            self.gfx.text_wrapped(&text, Rect::new(area.x, top, w, h + 4.0), size, weight, if current { th.text } else { th.dim });
        }
        self.gfx.alpha = saved;
        self.gfx.pop();
    }

    fn draw_timer_pill(&mut self, r: Rect, th: &Theme) {
        let c = self.cfg.focus;
        let w = wall();
        let col = phase_color(self.pomo.phase);
        let (cx, cy) = (r.x + 19.0, r.cy());
        self.gfx.ring(cx, cy, 7.5, 2.5, th.ink(0.18));
        self.gfx.arc(cx, cy, 7.5, self.pomo.progress(w, &c), 2.5, col);
        let paused = !self.pomo.running();
        let text = focus::fmt_left(self.pomo.left(w, &c));
        let tr = Rect::new(r.x + 34.0, r.y, r.w - 48.0, r.h);
        self.gfx.text(&text, tr, 14.0, 650, if paused { th.dim } else { th.text }, Align::Right);
    }

    fn page_focus(&mut self, r: Rect, th: &Theme) {
        let c = self.cfg.focus;
        let w = wall();
        let phase = self.pomo.phase;
        let col = phase_color(phase);
        // The ring: how much of this phase has gone.
        let (cx, cy, rad) = (r.x + 62.0, r.y + 64.0, 54.0);
        self.gfx.ring(cx, cy, rad, 8.0, th.ink(0.1));
        self.gfx.arc(cx, cy, rad, self.pomo.progress(w, &c), 8.0, col);
        let left = focus::fmt_left(self.pomo.left(w, &c));
        self.gfx.text(&left, Rect::new(cx - 50.0, cy - 16.0, 100.0, 28.0), 24.0, 700, th.text, Align::Center);
        let state = if self.pomo.running() {
            "running"
        } else if self.pomo.active() {
            "paused"
        } else {
            "ready"
        };
        self.gfx.text(state, Rect::new(cx - 50.0, cy + 12.0, 100.0, 16.0), 11.0, 600, th.dim, Align::Center);

        let x = r.x + 150.0;
        let cw = r.right() - x;
        self.gfx.text(phase.label(), Rect::new(x, r.y + 6.0, cw, 30.0), 22.0, 700, col, Align::Left);
        let every = c.long_break_every.max(1);
        let sub = match phase {
            Phase::Focus => format!("Session {} of {}  ·  {} min", self.pomo.done % every + 1, every, c.focus_minutes.round()),
            Phase::Break => format!("Stretch, breathe, look away  ·  {} min", c.break_minutes.round()),
            Phase::LongBreak => format!("You earned it  ·  {} min", c.long_break_minutes.round()),
        };
        self.gfx.text(&sub, Rect::new(x, r.y + 36.0, cw, 20.0), 13.0, 500, th.dim, Align::Left);
        if self.pomo.done > 0 {
            let done = format!("{} done", self.pomo.done);
            self.gfx.text(&done, Rect::new(r.right() - 120.0, r.y + 10.0, 120.0, 20.0), 12.0, 600, th.dim, Align::Right);
        }

        // Start / Pause, then Reset and Skip.
        let by = r.y + 78.0;
        let main = Rect::new(x, by, 128.0, 40.0);
        let label = if self.pomo.running() {
            "Pause"
        } else if self.pomo.active() {
            "Resume"
        } else {
            "Start"
        };
        let hot = self.hovered(main);
        self.gfx.fill_rrect(main, 20.0, if hot { col } else { col.fade(0.9) });
        self.gfx.text(label, main, 14.0, 700, Color::black(0.9), Align::Center);
        self.hit(main, Action::FocusToggle);
        for (k, (glyph, act)) in [(G_RESET, Action::FocusReset), (G_SKIP, Action::FocusSkip)].into_iter().enumerate() {
            let b = Rect::new(main.right() + 10.0 + k as f32 * 50.0, by, 40.0, 40.0);
            self.gfx.fill_circle(b.cx(), b.cy(), 20.0, th.ink(if self.hovered(b) { 0.18 } else { 0.1 }));
            self.gfx.icon(glyph, b, 14.0, th.text);
            self.hit(b, act);
        }
    }

    /// The tile that opens Phone drop.
    fn phone_tile(&mut self, t: Rect, th: &Theme) {
        let hot = self.hovered(t);
        self.gfx.fill_rrect(t, 12.0, if hot { th.accent.fade(0.28) } else { th.accent.fade(0.16) });
        self.gfx.icon(G_PHONE, Rect::new(t.x, t.y + 16.0, t.w, 28.0), 20.0, th.accent);
        self.gfx.text("From phone", Rect::new(t.x, t.y + 52.0, t.w, 18.0), 11.0, 600, th.text, Align::Center);
        self.hit(t, Action::PhoneOpen);
    }

    fn page_shelf(&mut self, r: Rect, th: &Theme) {
        if self.phone_panel {
            self.phone_panel_draw(r, th);
            return;
        }
        let n = self.shelf.items.len();
        if n == 0 {
            let drop = Rect::new(r.x, r.y, r.w - TILE_W - TILE_GAP, TILE_H);
            let fill = if self.drag_hover { th.accent.fade(0.18) } else { th.ink(0.05) };
            self.gfx.fill_rrect(drop, 12.0, fill);
            self.gfx.icon(G_DROP, Rect::new(drop.x, drop.y + 12.0, drop.w, 26.0), 18.0, if self.drag_hover { th.accent } else { th.dim });
            self.gfx.text("Drop files here, or send them from your phone", Rect::new(drop.x, drop.y + 42.0, drop.w, 20.0), 14.0, 600, th.text, Align::Center);
            self.gfx.text("They wait on the shelf until you drag them out", Rect::new(drop.x, drop.y + 61.0, drop.w, 16.0), 12.0, 400, th.dim, Align::Center);
            self.phone_tile(Rect::new(drop.right() + TILE_GAP, r.y, TILE_W, TILE_H), th);
            return;
        }
        if self.drag_hover {
            self.gfx.fill_rrect(r.inset(-6.0, -6.0), 18.0, th.accent.fade(0.14));
        }
        let cols = self.shelf_cols();
        let tile_at = |i: usize| Rect::new(r.x + (i % cols) as f32 * (TILE_W + TILE_GAP), r.y + (i / cols) as f32 * (TILE_H + TILE_GAP), TILE_W, TILE_H);
        for (i, path) in self.shelf.items.clone().iter().enumerate() {
            let t = tile_at(i);
            let hot = self.hovered(t);
            if hot {
                self.gfx.fill_rrect(t, 12.0, th.ink(0.08));
            }
            let pic = Rect::new(t.cx() - 26.0, t.y + 8.0, 52.0, 52.0);
            if !self.thumb_bmps.contains_key(path) {
                if let Some(b) = self.thumb_imgs.get(path).filter(|i| i.w > 0).and_then(|i| self.gfx.bitmap(i)) {
                    self.thumb_bmps.insert(path.clone(), b);
                }
            }
            match (self.thumb_bmps.get(path).cloned(), self.thumb_imgs.get(path)) {
                (Some(b), Some(img)) => {
                    // Fit the picture in its square, keeping its shape.
                    let k = (pic.w / img.w as f32).min(pic.h / img.h as f32);
                    let (iw, ih) = (img.w as f32 * k, img.h as f32 * k);
                    self.gfx.image(&b, Rect::new(pic.cx() - iw / 2.0, pic.cy() - ih / 2.0, iw, ih), 6.0);
                }
                _ => self.gfx.icon('\u{E7C3}', pic, 30.0, th.dim),
            }
            self.gfx.text(shelf::name(path), Rect::new(t.x + 4.0, t.y + 64.0, t.w - 8.0, 18.0), 11.0, 500, th.text, Align::Center);
            self.hit(t, Action::ShelfOpen(i));
            if hot {
                let x = Rect::new(t.right() - 22.0, t.y + 4.0, 18.0, 18.0);
                self.gfx.fill_circle(x.cx(), x.cy(), 9.0, th.ink(if self.hovered(x) { 0.35 } else { 0.2 }));
                self.gfx.icon(G_CLOSE, x, 8.0, th.text);
                self.hit(x, Action::ShelfRemove(i));
            }
        }
        self.phone_tile(tile_at(n), th);
        let clear = tile_at(n + 1);
        let hot = self.hovered(clear);
        self.gfx.fill_rrect(clear, 12.0, th.ink(if hot { 0.1 } else { 0.04 }));
        self.gfx.icon(G_DELETE, Rect::new(clear.x, clear.y + 16.0, clear.w, 28.0), 18.0, th.dim);
        self.gfx.text("Clear", Rect::new(clear.x, clear.y + 52.0, clear.w, 18.0), 11.0, 600, th.dim, Align::Center);
        self.hit(clear, Action::ShelfClear);
    }

    /// Phone drop: the QR code to scan, what's arriving, and Done.
    fn phone_panel_draw(&mut self, r: Rect, th: &Theme) {
        let size = 140.0;
        let qr_box = Rect::new(r.x, r.y + (r.h - size) / 2.0, size, size);
        if let Some((n, cells)) = self.phone_qr.clone() {
            // Dark on white with a quiet edge, each module on whole pixels so it scans crisply.
            let total = n as f32 + 4.0;
            let s = self.gfx.scale;
            let cell = ((size * s / total).floor() / s).max(1.0 / s);
            let side = cell * total;
            let ox = ((qr_box.x + (size - side) / 2.0) * s).round() / s;
            let oy = ((qr_box.y + (size - side) / 2.0) * s).round() / s;
            self.gfx.fill_rrect(Rect::new(ox - 4.0, oy - 4.0, side + 8.0, side + 8.0), 14.0, Color::rgba(1.0, 1.0, 1.0, 1.0));
            for y in 0..n {
                let mut x = 0;
                while x < n {
                    if !cells[y * n + x] {
                        x += 1;
                        continue;
                    }
                    let start = x;
                    while x < n && cells[y * n + x] {
                        x += 1;
                    }
                    let run = Rect::new(ox + (start as f32 + 2.0) * cell, oy + (y as f32 + 2.0) * cell, (x - start) as f32 * cell, cell);
                    self.gfx.fill_rect(run, Color::black(1.0));
                }
            }
        } else {
            self.gfx.fill_rrect(qr_box, 14.0, th.ink(0.06));
            self.gfx.icon(G_WARN, qr_box, 28.0, th.dim);
        }

        let x = qr_box.right() + 22.0;
        let cw = r.right() - x;
        self.gfx.text("Send from your phone", Rect::new(x, r.y + 4.0, cw, 26.0), 18.0, 700, th.text, Align::Left);
        let how = if self.phone_qr.is_some() { "Scan it with your phone's camera (same Wi-Fi)" } else { "Phone drop needs this PC on Wi-Fi or a network." };
        self.gfx.text(how, Rect::new(x, r.y + 30.0, cw, 18.0), 12.5, 500, th.dim, Align::Left);
        if let Some(url) = self.phone.as_ref().map(|p| p.url.clone()) {
            self.gfx.text(&url, Rect::new(x, r.y + 49.0, cw, 16.0), 11.0, 500, th.dim.fade(0.7), Align::Left);
        }

        // Status: waiting, receiving (with progress) or what arrived.
        let sy = r.y + 78.0;
        let (glyph, col) = if self.phone_bad {
            (G_WARN, Color::rgba(1.0, 0.45, 0.4, 1.0))
        } else if self.phone_got > 0 && self.phone_progress.is_none() {
            (G_CHECK, th.accent)
        } else {
            (G_PHONE, th.dim)
        };
        self.gfx.icon(glyph, Rect::new(x, sy, 18.0, 20.0), 13.0, col);
        let status = self.phone_status.clone();
        self.gfx.text(&status, Rect::new(x + 24.0, sy, cw - 24.0, 20.0), 13.0, 600, th.text, Align::Left);
        if let Some(p) = self.phone_progress {
            let bar = Rect::new(x + 24.0, sy + 24.0, cw - 24.0 - 100.0, 4.0);
            self.gfx.fill_rrect(bar, 2.0, th.ink(0.14));
            self.gfx.fill_rrect(Rect::new(bar.x, bar.y, (bar.w * p).max(4.0), 4.0), 2.0, th.accent);
        } else if !self.phone_seen && self.phone_qr.is_some() {
            self.gfx.text("Windows may ask: choose Allow", Rect::new(x + 24.0, sy + 20.0, cw - 124.0, 16.0), 11.0, 500, th.dim.fade(0.8), Align::Left);
        }

        let done = Rect::new(r.right() - 92.0, r.bottom() - 40.0, 92.0, 36.0);
        self.gfx.fill_rrect(done, 18.0, th.ink(if self.hovered(done) { 0.2 } else { 0.12 }));
        self.gfx.text("Done", done, 13.0, 700, th.text, Align::Center);
        self.hit(done, Action::PhoneClose);
    }

    /// Every theme as a little preview: its island hanging over a bit of desktop.
    fn page_themes(&mut self, r: Rect, th: &Theme) {
        let (cols, gap) = (4usize, 10.0);
        let cw = (r.w - gap * (cols as f32 - 1.0)) / cols as f32;
        let ch = (r.h - gap) / 2.0;
        let saved = self.gfx.alpha;
        for (i, t) in ThemeName::ALL.into_iter().enumerate() {
            let card = Rect::new(r.x + (i % cols) as f32 * (cw + gap), r.y + (i / cols) as f32 * (ch + gap), cw, ch);
            let hot = self.hovered(card);
            self.gfx.fill_rrect(card, 14.0, th.ink(if hot { 0.1 } else { 0.05 }));
            // A strip of desktop for the island to sit on.
            let desk = Rect::new(card.x + 8.0, card.y + 8.0, card.w - 16.0, 46.0);
            let Some(g) = self.gfx.rrect_geo(desk, 9.0) else { continue };
            self.gfx.fill_geo_gradient(&g, (desk.x, desk.y), (desk.right(), desk.bottom()), Color::rgba(0.16, 0.22, 0.38, 1.0), Color::rgba(0.45, 0.27, 0.42, 1.0));
            self.gfx.push_mask(&g);
            let (style, colors) = t.preset();
            let look = Theme::of(t, &colors, None);
            let hangs = style == Style::Notch;
            let bw = (desk.w - 14.0).min(88.0);
            let body = Rect::new(desk.cx() - bw / 2.0, desk.y + if hangs { 0.0 } else { 7.0 }, bw, 24.0);
            self.paint_island(body, 12.0, hangs, &look);
            self.gfx.text("9:41", body, 11.0, 650, look.text, Align::Center);
            self.gfx.pop();
            self.gfx.alpha = saved;
            let chosen = t == self.cfg.theme;
            self.gfx.text(t.label(), Rect::new(card.x, desk.bottom() + 6.0, card.w, 18.0), 12.0, 650, if chosen { th.text } else { th.dim }, Align::Center);
            if chosen {
                if let Some(g) = self.gfx.rrect_geo(card.inset(1.0, 1.0), 13.0) {
                    self.gfx.stroke_geo(&g, th.accent, 2.0);
                }
            }
            self.hit(card, Action::Theme(t));
        }
    }

    /// A small switch (Add-ons page).
    fn switch(&mut self, s: Rect, on: bool, act: Action, th: &Theme) {
        let hot = self.hovered(s);
        self.gfx.fill_rrect(s, s.h / 2.0, if on { th.accent } else { th.ink(if hot { 0.24 } else { 0.16 }) });
        let kx = if on { s.right() - s.h / 2.0 } else { s.x + s.h / 2.0 };
        self.gfx.fill_circle(kx, s.cy(), s.h / 2.0 - 3.0, Color::rgba(1.0, 1.0, 1.0, 1.0));
        self.hit(s, act);
    }

    fn page_addons(&mut self, r: Rect, th: &Theme) {
        self.gfx.text("Each one stays off until you switch it on.", Rect::new(r.x, r.y - 2.0, r.w, 20.0), 12.0, 500, th.dim, Align::Left);
        let a = &self.cfg.addons;
        let on = [a.system, a.caffeine, a.prayer, a.downloader, a.script, false, false, false];
        for (i, (name, desc, glyph, ready)) in ADDONS.iter().enumerate() {
            let row = Rect::new(r.x, r.y + 26.0 + i as f32 * ADDON_ROW, r.w, ADDON_ROW);
            let col = if *ready { th.text } else { th.dim.fade(0.7) };
            self.gfx.icon(*glyph, Rect::new(row.x, row.y, 22.0, row.h), 13.0, if *ready { th.accent } else { th.dim.fade(0.6) });
            let nw = self.gfx.measure(name, 13.5, 700) + 12.0;
            self.gfx.text(name, Rect::new(row.x + 30.0, row.y, nw, row.h), 13.5, 700, col, Align::Left);
            self.gfx.text(desc, Rect::new(row.x + 30.0 + nw, row.y, row.w - nw - 30.0 - 112.0, row.h), 12.0, 500, th.dim.fade(if *ready { 1.0 } else { 0.6 }), Align::Left);
            if *ready {
                self.gfx.text("Built in", Rect::new(row.right() - 112.0, row.y, 60.0, row.h), 11.5, 600, th.dim, Align::Right);
                self.switch(Rect::new(row.right() - 40.0, row.cy() - 10.0, 38.0, 20.0), on[i], Action::Addon(i), th);
            } else {
                self.gfx.text("Coming soon", Rect::new(row.right() - 112.0, row.y, 110.0, row.h), 11.5, 600, th.dim.fade(0.6), Align::Right);
            }
        }
    }

    /// A tiny chart of `values` (0..1) filling `r`.
    fn sparkline(&mut self, r: Rect, values: &[f32], c: Color) {
        if values.len() < 2 {
            return;
        }
        let n = crate::system::HISTORY.max(values.len()) as f32 - 1.0;
        let off = crate::system::HISTORY.saturating_sub(values.len()) as f32;
        let pts: Vec<(f32, f32)> = values.iter().enumerate().map(|(i, v)| (r.x + (off + i as f32) / n * r.w, r.bottom() - v.clamp(0.0, 1.0) * r.h)).collect();
        self.gfx.area_under(&pts, r.bottom(), c.fade(0.32), c.fade(0.0));
        self.gfx.polyline(&pts, c, 1.6);
    }

    fn page_system(&mut self, r: Rect, th: &Theme) {
        let Some(s) = self.sys_snap.clone() else {
            self.gfx.text("Reading…", Rect::new(r.x, r.y, r.w, 40.0), 13.0, 500, th.dim, Align::Center);
            return;
        };
        let hist: Vec<[f32; 4]> = self.sys_hist.iter().copied().collect();
        let col = |k: usize| hist.iter().map(|h| h[k]).collect::<Vec<f32>>();
        let net = col(3);
        let net_max = net.iter().copied().fold(64_000.0f32, f32::max);
        let net_n: Vec<f32> = net.iter().map(|v| v / net_max).collect();
        let mem_pct = s.mem_used as f32 / s.mem_total.max(1) as f32;
        let tiles = [
            ("CPU", format!("{:.0}%", s.cpu * 100.0), String::new(), col(0), Color::rgba(0.39, 0.82, 1.0, 1.0)),
            ("GPU", s.gpu.map_or("—".into(), |g| format!("{:.0}%", g * 100.0)), String::new(), col(1), Color::rgba(0.75, 0.35, 0.95, 1.0)),
            ("Memory", format!("{:.0}%", mem_pct * 100.0), format!("{} of {}", crate::system::fmt_gb(s.mem_used), crate::system::fmt_gb(s.mem_total)), col(2), Color::rgba(0.19, 0.82, 0.35, 1.0)),
            ("Network", crate::system::fmt_rate(s.net_down), format!("\u{2191} {}", crate::system::fmt_rate(s.net_up)), net_n, Color::rgba(1.0, 0.62, 0.04, 1.0)),
        ];
        let gap = 8.0;
        let tw = (r.w - 3.0 * gap) / 4.0;
        for (i, (label, value, sub, series, c)) in tiles.into_iter().enumerate() {
            let t = Rect::new(r.x + i as f32 * (tw + gap), r.y, tw, 98.0);
            self.gfx.fill_rrect(t, 14.0, th.ink(0.05));
            self.gfx.text(label, Rect::new(t.x + 12.0, t.y + 8.0, t.w - 24.0, 16.0), 11.0, 650, th.dim, Align::Left);
            self.gfx.text(&value, Rect::new(t.x + 12.0, t.y + 24.0, t.w - 24.0, 26.0), 19.0, 700, th.text, Align::Left);
            if !sub.is_empty() {
                self.gfx.text(&sub, Rect::new(t.x + 12.0, t.y + 49.0, t.w - 24.0, 14.0), 10.5, 550, th.dim, Align::Left);
            }
            if let Some(g) = self.gfx.rrect_geo(t, 14.0) {
                self.gfx.push_mask(&g);
                self.sparkline(Rect::new(t.x, t.bottom() - 30.0, t.w, 28.0), &series, c);
                self.gfx.pop();
            }
        }
        // The busiest apps, and the disk.
        let y = r.y + 108.0;
        self.gfx.text("Busiest apps", Rect::new(r.x, y, 200.0, 16.0), 11.0, 650, th.dim, Align::Left);
        let half = (r.w - 16.0) / 2.0;
        for (i, (name, share)) in s.top.iter().take(3).enumerate() {
            let ry = y + 20.0 + i as f32 * 17.0;
            self.gfx.text(name, Rect::new(r.x, ry, half - 60.0, 16.0), 12.0, 600, th.text, Align::Left);
            self.gfx.text(&format!("{:.0}%", share * 100.0), Rect::new(r.x + half - 56.0, ry, 50.0, 16.0), 12.0, 600, th.dim, Align::Right);
        }
        let x2 = r.x + half + 16.0;
        let used = 1.0 - s.disk_free as f32 / s.disk_total.max(1) as f32;
        self.gfx.text("Disk C:", Rect::new(x2, y, half, 16.0), 11.0, 650, th.dim, Align::Left);
        self.gfx.text(&format!("{} free of {}", crate::system::fmt_gb(s.disk_free), crate::system::fmt_gb(s.disk_total)), Rect::new(x2, y + 18.0, half, 18.0), 12.5, 600, th.text, Align::Left);
        let bar = Rect::new(x2, y + 40.0, half, 5.0);
        self.gfx.fill_rrect(bar, 2.5, th.ink(0.12));
        self.gfx.fill_rrect(Rect::new(bar.x, bar.y, bar.w * used, bar.h), 2.5, if used > 0.9 { Color::rgba(1.0, 0.27, 0.23, 1.0) } else { th.accent });
        let up = s.uptime_s;
        self.gfx.text(&format!("Up {}d {}h {}m", up / 86400, up / 3600 % 24, up / 60 % 60), Rect::new(x2, y + 50.0, half, 16.0), 11.0, 550, th.dim, Align::Left);
    }

    fn page_caffeine(&mut self, r: Rect, th: &Theme) {
        use crate::caffeine as cf;
        let c = self.cfg.caffeine.clone();
        let now = cf::now_min();
        let in_you = self.caf.in_body(now, c.half_life_h);
        let bed = cf::bedtime(now, &c);
        let at_bed = self.caf.in_body(bed, c.half_life_h);
        let usual = c.drinks.get(1).or(c.drinks.first()).map_or(95.0, |d| d.1);
        let last = cf::last_call(&self.caf, now, usual, &c);
        let warn = Color::rgba(1.0, 0.62, 0.04, 1.0);

        // Left: how much now, today, at bedtime, and the last call.
        let lw = 176.0;
        let big = format!("{:.0}", in_you);
        let bw = self.gfx.measure(&big, 34.0, 700);
        self.gfx.text(&big, Rect::new(r.x, r.y - 6.0, bw + 4.0, 42.0), 34.0, 700, th.text, Align::Left);
        self.gfx.text("mg in you now", Rect::new(r.x + bw + 8.0, r.y + 12.0, lw, 18.0), 12.0, 600, th.dim, Align::Left);
        let today = self.caf.total_since(cf::day_start(now));
        self.gfx.text(&format!("{:.0} mg today", today), Rect::new(r.x, r.y + 40.0, lw, 18.0), 12.5, 600, th.text, Align::Left);
        let bed_ok = at_bed <= c.bedtime_limit_mg;
        self.gfx.text(&format!("~{:.0} mg at bedtime ({})", at_bed, cf::fmt_hm(bed)), Rect::new(r.x, r.y + 59.0, lw + 20.0, 18.0), 12.5, 600, if bed_ok { th.dim } else { warn }, Align::Left);
        let lc = match last {
            Some(t) if t <= now => "Last call has passed".to_string(),
            Some(t) if t >= bed => "Fine for one more anytime".to_string(),
            Some(t) => format!("Last call {}", cf::fmt_hm(t)),
            None => "No more today".to_string(),
        };
        self.gfx.text(&lc, Rect::new(r.x, r.y + 78.0, lw + 20.0, 18.0), 12.5, 700, if last.is_some_and(|t| t > now) { th.accent } else { warn }, Align::Left);

        // Right: the day's curve, 06:00 to 06:00, with now and bedtime marked.
        let chart = Rect::new(r.x + lw + 24.0, r.y, r.w - lw - 24.0, 96.0);
        self.gfx.fill_rrect(chart, 12.0, th.ink(0.04));
        let start = cf::day_start(now);
        let n = 96;
        let vals: Vec<f32> = (0..=n).map(|i| self.caf.in_body(start + i as i64 * 15, c.half_life_h)).collect();
        let top = vals.iter().copied().fold(c.bedtime_limit_mg * 3.0, f32::max).max(60.0);
        let px = |t: i64| chart.x + 8.0 + ((t - start) as f32 / 1440.0).clamp(0.0, 1.0) * (chart.w - 16.0);
        let py = |mg: f32| chart.bottom() - 8.0 - (mg / top).clamp(0.0, 1.0) * (chart.h - 16.0);
        let pts: Vec<(f32, f32)> = vals.iter().enumerate().map(|(i, v)| (px(start + i as i64 * 15), py(*v))).collect();
        // The limit line, bedtime and now.
        self.gfx.fill_rect(Rect::new(chart.x + 8.0, py(c.bedtime_limit_mg), chart.w - 16.0, 1.0), th.ink(0.14));
        self.gfx.fill_rect(Rect::new(px(bed), chart.y + 6.0, 1.0, chart.h - 12.0), th.ink(0.18));
        self.gfx.text("bed", Rect::new(px(bed) - 30.0, chart.y + 4.0, 26.0, 12.0), 9.5, 600, th.dim, Align::Right);
        self.gfx.area_under(&pts, chart.bottom() - 8.0, th.accent.fade(0.35), th.accent.fade(0.02));
        self.gfx.polyline(&pts, th.accent, 1.8);
        self.gfx.fill_circle(px(now), py(in_you), 3.5, th.text);

        // Bottom: a button per drink, and undo.
        let by = r.y + 110.0;
        let mut x = r.x;
        let drinks = c.drinks.clone();
        for (i, (name, mg)) in drinks.iter().enumerate() {
            let label = format!("{name}  {mg:.0}");
            let w = self.gfx.measure(&label, 12.0, 650) + 24.0;
            if x + w > r.right() - 70.0 {
                break;
            }
            let b = Rect::new(x, by, w, 30.0);
            self.gfx.fill_rrect(b, 15.0, th.ink(if self.hovered(b) { 0.16 } else { 0.08 }));
            self.gfx.text(&label, b, 12.0, 650, th.text, Align::Center);
            self.hit(b, Action::DrinkAdd(i));
            x += w + 6.0;
        }
        if !self.caf.drinks.is_empty() {
            let u = Rect::new(r.right() - 64.0, by, 64.0, 30.0);
            self.gfx.fill_rrect(u, 15.0, th.ink(if self.hovered(u) { 0.14 } else { 0.05 }));
            self.gfx.text("Undo", u, 12.0, 650, th.dim, Align::Center);
            self.hit(u, Action::DrinkUndo);
        }
        let last_drink = self.caf.drinks.last().map(|d| format!("Last: {} at {}", d.name, cf::fmt_hm(d.at))).unwrap_or_else(|| "Tap a drink when you have one".into());
        self.gfx.text(&last_drink, Rect::new(r.x, by + 36.0, r.w, 16.0), 11.0, 550, th.dim, Align::Left);
    }

    fn page_prayer(&mut self, r: Rect, th: &Theme) {
        use crate::prayer::{self as pr, Prayer};
        let Some((lat, lon, cc)) = self.prayer_site() else {
            // No location yet.
            self.gfx.icon('\u{E707}', Rect::new(r.x, r.y + 6.0, r.w, 30.0), 22.0, th.dim);
            let (title, sub) = if self.locating {
                ("Finding where you are…", "Windows may ask if Synapse can use your location".to_string())
            } else {
                ("Synapse needs to know where you are", self.locate_error.clone().map_or("".into(), |e| format!("{e}. Turn on location for desktop apps, or set prayer.lat / lon in the config.")))
            };
            self.gfx.text(title, Rect::new(r.x, r.y + 40.0, r.w, 22.0), 14.0, 650, th.text, Align::Center);
            self.gfx.text(&sub, Rect::new(r.x, r.y + 62.0, r.w, 18.0), 11.5, 500, th.dim, Align::Center);
            if !self.locating {
                let b1 = Rect::new(r.cx() - 158.0, r.y + 92.0, 150.0, 32.0);
                let b2 = Rect::new(r.cx() + 8.0, r.y + 92.0, 150.0, 32.0);
                for (b, label, act) in [(b1, "Use my location", Action::Locate), (b2, "Location settings", Action::LocationSettings)] {
                    self.gfx.fill_rrect(b, 16.0, th.ink(if self.hovered(b) { 0.18 } else { 0.1 }));
                    self.gfx.text(label, b, 12.5, 650, th.text, Align::Center);
                    self.hit(b, act);
                }
            }
            return;
        };
        let Some(t) = self.prayer_times() else { return };
        let (m, hanafi) = self.prayer_rules(cc);
        let next = self.next_prayer();

        // Left: the next prayer and how long until it; the place and the qibla.
        let lw = 190.0;
        if let Some((p, at, mins)) = next {
            self.gfx.text("Next", Rect::new(r.x, r.y - 2.0, lw, 16.0), 11.0, 650, th.dim, Align::Left);
            self.gfx.text(p.label(), Rect::new(r.x, r.y + 12.0, lw, 32.0), 26.0, 700, th.text, Align::Left);
            let left = if mins >= 60.0 { format!("in {} h {} min", (mins / 60.0) as i64, (mins % 60.0) as i64) } else { format!("in {} min", mins.ceil() as i64) };
            self.gfx.text(&format!("{}  \u{00B7}  {left}", pr::fmt(at)), Rect::new(r.x, r.y + 44.0, lw, 18.0), 13.0, 600, th.accent, Align::Left);
        }
        let place = if self.cfg.prayer.place.is_empty() { format!("{lat:.2}, {lon:.2}") } else { self.cfg.prayer.place.clone() };
        self.gfx.text(&place, Rect::new(r.x, r.y + 72.0, lw, 17.0), 12.0, 650, th.text, Align::Left);
        let rule = format!("{}{}", m.name, if hanafi { ", Hanafi Asr" } else { "" });
        self.gfx.text(&rule, Rect::new(r.x, r.y + 89.0, lw, 15.0), 10.5, 550, th.dim, Align::Left);
        // The qibla: a little compass needle.
        let q = pr::qibla(lat, lon);
        let (cx, cy, rad) = (r.x + 15.0, r.y + 126.0, 13.0);
        self.gfx.ring(cx, cy, rad, 1.2, th.ink(0.25));
        let a = (q as f32).to_radians();
        self.gfx.polyline(&[(cx, cy), (cx + rad * 0.85 * a.sin(), cy - rad * 0.85 * a.cos())], th.accent, 2.2);
        self.gfx.fill_circle(cx, cy, 2.0, th.text);
        self.gfx.text(&format!("Qibla {:.0}\u{00B0} {}", q, pr::compass(q)), Rect::new(cx + 20.0, cy - 9.0, lw, 18.0), 12.0, 600, th.dim, Align::Left);

        // Right: today's six times, the next one lit.
        let x = r.x + lw + 10.0;
        let (cols, gap) = (3usize, 8.0);
        let cw = (r.right() - x - gap * 2.0) / cols as f32;
        for (i, p) in Prayer::ALL.into_iter().enumerate() {
            let cell = Rect::new(x + (i % cols) as f32 * (cw + gap), r.y + (i / cols) as f32 * 72.0, cw, 64.0);
            let lit = next.is_some_and(|(n, at, _)| n == p && at < 24.0);
            self.gfx.fill_rrect(cell, 14.0, if lit { th.accent.fade(0.22) } else { th.ink(0.05) });
            self.gfx.text(p.label(), Rect::new(cell.x + 12.0, cell.y + 8.0, cell.w - 24.0, 16.0), 11.5, 650, if p.is_prayer() { th.dim } else { th.dim.fade(0.7) }, Align::Left);
            self.gfx.text(&pr::fmt(t.get(p)), Rect::new(cell.x + 12.0, cell.y + 26.0, cell.w - 24.0, 28.0), 21.0, 700, if lit { th.text } else if p.is_prayer() { th.text.fade(0.9) } else { th.dim }, Align::Left);
        }
    }

    /// A rounded text button.
    fn button(&mut self, b: Rect, label: &str, act: Action, strong: bool, th: &Theme) {
        let hot = self.hovered(b);
        if strong {
            self.gfx.fill_rrect(b, b.h / 2.0, if hot { th.accent } else { th.accent.fade(0.9) });
            self.gfx.text(label, b, 12.5, 700, Color::black(0.9), Align::Center);
        } else {
            self.gfx.fill_rrect(b, b.h / 2.0, th.ink(if hot { 0.18 } else { 0.1 }));
            self.gfx.text(label, b, 12.5, 650, th.text, Align::Center);
        }
        self.hit(b, act);
    }

    fn page_downloads(&mut self, r: Rect, th: &Theme) {
        // The way in: copy a link, then paste it as video or sound.
        self.gfx.text("Copy a link, then", Rect::new(r.x, r.y, 130.0, 32.0), 13.0, 600, th.dim, Align::Left);
        self.button(Rect::new(r.x + 124.0, r.y, 132.0, 32.0), "Paste \u{2192} Video", Action::Paste(false), true, th);
        self.button(Rect::new(r.x + 264.0, r.y, 132.0, 32.0), "Paste \u{2192} Audio", Action::Paste(true), false, th);
        self.button(Rect::new(r.right() - 84.0, r.y, 84.0, 32.0), "Folder", Action::DlFolder, false, th);
        let tool = self.tool.clone();
        let status = match &tool {
            Tool::Fetching => Some(("Getting yt-dlp from GitHub (about 18 MB, once)…".to_string(), false)),
            Tool::Failed(e) => Some((format!("Couldn't get yt-dlp: {e}"), true)),
            Tool::Missing => Some(("yt-dlp is fetched when you start your first download".to_string(), false)),
            Tool::Ready => None,
        };
        let mut y = r.y + 44.0;
        if let Some((s, bad)) = status {
            self.gfx.text(&s, Rect::new(r.x, y, r.w - 90.0, 18.0), 11.5, 600, if bad { Color::rgba(1.0, 0.45, 0.4, 1.0) } else { th.dim }, Align::Left);
            if bad {
                self.button(Rect::new(r.right() - 84.0, y - 4.0, 84.0, 26.0), "Try again", Action::ToolRetry, false, th);
            }
            y += 26.0;
        }
        let jobs: Vec<Job> = self.jobs.iter().rev().take(3).cloned().collect();
        if jobs.is_empty() {
            self.gfx.text("Videos and sound land in your Downloads folder, and on the Shelf.", Rect::new(r.x, y + 6.0, r.w, 18.0), 12.0, 500, th.dim.fade(0.8), Align::Left);
            return;
        }
        for j in jobs {
            let row = Rect::new(r.x, y, r.w, 40.0);
            if row.bottom() > r.bottom() + 2.0 {
                break;
            }
            self.gfx.fill_rrect(row, 12.0, th.ink(0.05));
            let kind = if j.audio { '\u{E8D6}' } else { '\u{E714}' };
            self.gfx.icon(kind, Rect::new(row.x + 6.0, row.y, 24.0, row.h), 12.0, th.dim);
            let tx = row.x + 34.0;
            let right_w = 150.0;
            self.gfx.text(&j.title, Rect::new(tx, row.y + 3.0, row.w - 34.0 - right_w - 8.0, 18.0), 12.5, 650, th.text, Align::Left);
            let (line, color) = match &j.state {
                JobState::Queued => ("Waiting…".to_string(), th.dim),
                JobState::Running => (format!("{:.0}%  {}  {}", j.pct * 100.0, j.speed, if j.eta.is_empty() { String::new() } else { format!("\u{00B7} {} left", j.eta) }), th.dim),
                JobState::Done(_) => ("Saved".to_string(), th.accent),
                JobState::Failed(e) => (e.clone(), Color::rgba(1.0, 0.45, 0.4, 1.0)),
            };
            self.gfx.text(&line, Rect::new(tx, row.y + 21.0, row.w - 34.0 - right_w - 8.0, 15.0), 10.5, 600, color, Align::Left);
            // Progress along the row's foot while it runs.
            if j.state == JobState::Running {
                let bar = Rect::new(row.x + 12.0, row.bottom() - 3.0, (row.w - 24.0) * j.pct.max(0.01), 2.0);
                self.gfx.fill_rrect(bar, 1.0, th.accent);
            }
            match j.state {
                JobState::Running | JobState::Queued => self.button(Rect::new(row.right() - 70.0, row.y + 7.0, 62.0, 26.0), "Stop", Action::DlStop(j.id), false, th),
                JobState::Done(_) => self.button(Rect::new(row.right() - 70.0, row.y + 7.0, 62.0, 26.0), "Open", Action::DlOpen(j.id), true, th),
                JobState::Failed(_) => {}
            }
            y += 46.0;
        }
    }

    fn page_script(&mut self, r: Rect, now: f64, th: &Theme) {
        let _ = now;
        let area = Rect::new(r.x, r.y, r.w, r.h - 44.0);
        let size = self.cfg.script.size;
        if self.script.trim().is_empty() {
            self.gfx.icon('\u{E8A5}', Rect::new(area.x, area.y + 22.0, area.w, 30.0), 22.0, th.dim);
            self.gfx.text("Paste your script, or write it in script.txt", Rect::new(area.x, area.y + 58.0, area.w, 22.0), 14.0, 650, th.text, Align::Center);
            self.gfx.text("It scrolls right under your webcam, and screen shares leave it out", Rect::new(area.x, area.y + 80.0, area.w, 18.0), 12.0, 500, th.dim, Align::Center);
        } else {
            self.script_layout(area.w);
            let Some(mask) = self.gfx.rrect_geo(area, 10.0) else { return };
            self.gfx.push_mask(&mask);
            let line_y = area.y + area.h * SCRIPT_LINE;
            let mut y = line_y - self.script_pos;
            let paras: Vec<String> = self.script.lines().map(str::to_string).collect();
            let saved = self.gfx.alpha;
            for (i, p) in paras.iter().enumerate() {
                let h = self.script_heights.get(i).copied().unwrap_or(size);
                if y + h > area.y && y < area.bottom() {
                    // Lines already read dim; what's ahead is bright.
                    let read = y + h < line_y;
                    self.gfx.alpha = saved * if read { 0.4 } else { 1.0 };
                    self.gfx.text_wrapped(p, Rect::new(area.x, y, area.w, h + 4.0), size, 650, th.text);
                }
                y += h;
            }
            self.gfx.alpha = saved;
            // Soft edges, top and bottom, in the island's own colour.
            self.gfx.fill_rect_gradient(Rect::new(area.x, area.y, area.w, 22.0), (0.0, area.y), (0.0, area.y + 22.0), th.island, th.island.fade(0.0));
            self.gfx.fill_rect_gradient(Rect::new(area.x, area.bottom() - 28.0, area.w, 28.0), (0.0, area.bottom() - 28.0), (0.0, area.bottom()), th.island.fade(0.0), th.island);
            self.gfx.pop();
            // The reading line, just left of the text.
            self.gfx.fill_rrect(Rect::new(area.x - 11.0, line_y + 3.0, 3.0, size * 1.05), 1.5, th.accent);
        }
        // Controls.
        let by = r.bottom() - 34.0;
        let mut x = r.x;
        for (label, act, w, strong) in [
            ("Paste", Action::ScriptPaste, 60.0, false),
            ("Edit", Action::ScriptEdit, 52.0, false),
            (if self.script_playing { "Pause" } else { "Read" }, Action::ScriptPlay, 72.0, true),
        ] {
            self.button(Rect::new(x, by, w, 30.0), label, act, strong, th);
            x += w + 6.0;
        }
        // Back to the top, as a round icon (room for the steppers).
        let back = Rect::new(x, by, 30.0, 30.0);
        self.gfx.fill_circle(back.cx(), back.cy(), 15.0, th.ink(if self.hovered(back) { 0.18 } else { 0.1 }));
        self.gfx.icon(G_RESET, back, 12.0, th.text);
        self.hit(back, Action::ScriptRestart);
        // Speed and size steppers on the right.
        let mut rx = r.right();
        for (label, value, minus, plus) in [
            ("Size", format!("{:.0}", size), Action::ScriptSize(-1), Action::ScriptSize(1)),
            ("Speed", format!("{:.0}", self.cfg.script.speed), Action::ScriptSpeed(-1), Action::ScriptSpeed(1)),
        ] {
            let p = Rect::new(rx - 28.0, by + 1.0, 28.0, 28.0);
            let v = Rect::new(p.x - 34.0, by, 34.0, 30.0);
            let m = Rect::new(v.x - 28.0, by + 1.0, 28.0, 28.0);
            for (b, g, act) in [(m, '\u{E738}', minus), (p, '\u{E710}', plus)] {
                self.gfx.fill_circle(b.cx(), b.cy(), 14.0, th.ink(if self.hovered(b) { 0.18 } else { 0.08 }));
                self.gfx.icon(g, b, 10.0, th.text);
                self.hit(b, act);
            }
            self.gfx.text(&value, v, 12.5, 700, th.text, Align::Center);
            let lw = self.gfx.measure(label, 11.0, 600) + 6.0;
            self.gfx.text(label, Rect::new(m.x - lw, by, lw, 30.0), 11.0, 600, th.dim, Align::Left);
            rx = m.x - lw - 12.0;
        }
    }

    /// A segmented picker across `r`: one option per `opts`, `sel` lit.
    fn segmented(&mut self, r: Rect, opts: &[&str], sel: usize, key: crate::settings::Key, th: &Theme) {
        self.gfx.fill_rrect(r, r.h / 2.0, th.ink(0.07));
        let w = (r.w - 6.0) / opts.len() as f32;
        // Many options in a narrow row: the labels shrink to fit.
        let widest = opts.iter().map(|o| self.gfx.measure(o, 12.5, 650)).fold(0.0, f32::max);
        let size = if widest > w - 8.0 { (12.5 * (w - 8.0) / widest).max(9.5) } else { 12.5 };
        for (i, o) in opts.iter().enumerate() {
            let b = Rect::new(r.x + 3.0 + i as f32 * w, r.y + 3.0, w, r.h - 6.0);
            if i == sel {
                self.gfx.fill_rrect(b, b.h / 2.0, th.ink(0.2));
            } else if self.hovered(b) {
                self.gfx.fill_rrect(b, b.h / 2.0, th.ink(0.08));
            }
            self.gfx.text(o, b, size, 650, if i == sel { th.text } else { th.dim }, Align::Center);
            self.hit(b, Action::Set(key, crate::settings::Val::Pick(i)));
        }
    }

    fn page_tasks(&mut self, r: Rect, th: &Theme) {
        // The field, and Add.
        let field = Rect::new(r.x, r.y, r.w - 72.0, 34.0);
        self.gfx.fill_rrect(field, 12.0, th.ink(if self.typing { 0.1 } else if self.hovered(field) { 0.08 } else { 0.06 }));
        if self.typing {
            if let Some(g) = self.gfx.rrect_geo(field, 12.0) {
                self.gfx.stroke_geo(&g, th.accent.fade(0.8), 1.5);
            }
        }
        let tx = Rect::new(field.x + 12.0, field.y, field.w - 24.0, field.h);
        if self.draft.is_empty() {
            self.gfx.text("Add a task…", tx, 13.5, 500, th.dim.fade(0.7), Align::Left);
        } else {
            self.gfx.text(&self.draft.clone(), tx, 13.5, 600, th.text, Align::Left);
        }
        if self.typing {
            let before: String = self.draft.chars().take(self.caret).collect();
            let cx = (tx.x + self.gfx.measure(&before, 13.5, 600)).min(tx.right());
            self.gfx.fill_rect(Rect::new(cx, field.y + 8.0, 1.5, field.h - 16.0), th.accent);
        }
        self.hit(field, Action::TaskFocus);
        self.button(Rect::new(r.right() - 64.0, r.y, 64.0, 34.0), "Add", Action::TaskAdd, true, th);

        // The list: open tasks first, then the done ones.
        let all = self.tasks.ordered();
        let max_scroll = all.len().saturating_sub(TASK_ROWS);
        self.tasks_scroll = self.tasks_scroll.min(max_scroll);
        let list_y = r.y + 46.0;
        if all.is_empty() {
            self.gfx.text("Nothing to do. Add something above.", Rect::new(r.x, list_y + 20.0, r.w, 22.0), 13.0, 500, th.dim, Align::Center);
        }
        for (i, t) in all.iter().skip(self.tasks_scroll).take(TASK_ROWS).enumerate() {
            let row = Rect::new(r.x, list_y + i as f32 * TASK_ROW, r.w, TASK_ROW - 2.0);
            let hot = self.hovered(row);
            if hot {
                self.gfx.fill_rrect(row, 10.0, th.ink(0.05));
            }
            // The tick.
            let (cx, cy) = (row.x + 16.0, row.cy());
            let tick = Rect::new(cx - 11.0, cy - 11.0, 22.0, 22.0);
            if t.done {
                self.gfx.fill_circle(cx, cy, 9.0, th.accent);
                self.gfx.icon('\u{E73E}', tick, 9.0, Color::black(0.85));
            } else {
                self.gfx.ring(cx, cy, 8.5, 1.6, th.ink(if self.hovered(tick) { 0.7 } else { 0.4 }));
            }
            self.hit(tick, Action::TaskToggle(t.id));
            // The words (struck through once done).
            let text_r = Rect::new(row.x + 36.0, row.y, row.w - 36.0 - 34.0, row.h);
            let col = if t.done { th.dim.fade(0.7) } else { th.text };
            self.gfx.text(&t.text, text_r, 13.5, 600, col, Align::Left);
            if t.done {
                let w = self.gfx.measure(&t.text, 13.5, 600).min(text_r.w);
                self.gfx.fill_rect(Rect::new(text_r.x, row.cy(), w, 1.2), th.dim.fade(0.8));
            }
            if hot {
                let x = Rect::new(row.right() - 28.0, row.cy() - 11.0, 22.0, 22.0);
                self.gfx.fill_circle(x.cx(), x.cy(), 10.0, th.ink(if self.hovered(x) { 0.2 } else { 0.08 }));
                self.gfx.icon(G_CLOSE, x, 8.0, th.text);
                self.hit(x, Action::TaskDelete(t.id));
            }
        }
        // Footer: what's left, more below, and clearing the done ones.
        let fy = r.bottom() - 22.0;
        let left = self.tasks.left();
        let mut info = if left == 1 { "1 left".to_string() } else { format!("{left} left") };
        if max_scroll > 0 {
            info += &format!("  \u{00B7}  scroll for {} more", all.len() - TASK_ROWS - self.tasks_scroll.min(all.len() - TASK_ROWS));
        }
        self.gfx.text(&info, Rect::new(r.x, fy, 300.0, 20.0), 11.5, 600, th.dim, Align::Left);
        if self.tasks.items.iter().any(|t| t.done) {
            let b = Rect::new(r.right() - 90.0, fy - 2.0, 90.0, 24.0);
            if self.hovered(b) {
                self.gfx.fill_rrect(b, 12.0, th.ink(0.08));
            }
            self.gfx.text("Clear done", b, 11.5, 650, th.dim, Align::Center);
            self.hit(b, Action::TaskClearDone);
        }
    }

    // ------------------------------------------------------------ typing

    /// The text field takes the keyboard: the island may now be the active
    /// window (it never is otherwise), until Esc, a click elsewhere, or it closes.
    fn focus_text(&mut self) {
        if self.typing {
            return;
        }
        self.typing = true;
        self.caret = self.draft.chars().count();
        unsafe {
            self.prev_focus = GetForegroundWindow();
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex & !(WS_EX_NOACTIVATE.0 as isize));
            let _ = SetForegroundWindow(self.hwnd);
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(self.hwnd);
        }
    }

    /// Give the keyboard back to whoever had it.
    fn blur_text(&mut self) {
        if !self.typing {
            return;
        }
        self.typing = false;
        self.high_surrogate = None;
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
            if !self.prev_focus.is_invalid() && IsWindow(self.prev_focus).as_bool() {
                let _ = SetForegroundWindow(self.prev_focus);
            }
        }
        self.hold_ended();
        self.request_frame();
    }

    fn add_task(&mut self) {
        let text = std::mem::take(&mut self.draft);
        self.caret = 0;
        if self.tasks.add(&text) {
            self.tasks.save();
            self.tasks_scroll = 0;
        }
    }

    fn insert_text(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        let at = self.draft.char_indices().nth(self.caret).map_or(self.draft.len(), |(i, _)| i);
        self.draft.insert_str(at, &s);
        self.caret += s.chars().count();
    }

    /// A key while typing. True when it was handled.
    fn on_key(&mut self, vk: u32) -> bool {
        let n = self.draft.chars().count();
        let byte = |s: &str, c: usize| s.char_indices().nth(c).map_or(s.len(), |(i, _)| i);
        match vk {
            0x08 if self.caret > 0 => {
                let (a, b) = (byte(&self.draft, self.caret - 1), byte(&self.draft, self.caret));
                self.draft.replace_range(a..b, "");
                self.caret -= 1;
            }
            0x2E if self.caret < n => {
                let (a, b) = (byte(&self.draft, self.caret), byte(&self.draft, self.caret + 1));
                self.draft.replace_range(a..b, "");
            }
            0x25 => self.caret = self.caret.saturating_sub(1),
            0x27 => self.caret = (self.caret + 1).min(n),
            0x24 => self.caret = 0,
            0x23 => self.caret = n,
            0x0D => self.add_task(),
            0x1B => self.blur_text(),
            0x08 | 0x2E => {}
            _ => return false,
        }
        self.request_frame();
        true
    }

    /// The island's own Settings page: the everyday settings at a glance;
    /// Advanced settings opens the window with everything.
    fn page_quick_settings(&mut self, r: Rect, th: &Theme) {
        use crate::settings::{idle_index, Key, Val, IDLE_NAMES};
        let label = |s: &mut Self, t: &str, x: f32, y: f32, w: f32| s.gfx.text(t, Rect::new(x, y, w, 14.0), 10.5, 750, th.dim, Align::Left);
        // Theme across the top.
        label(self, "THEME", r.x, r.y - 2.0, 200.0);
        let names: Vec<&str> = ThemeName::ALL.iter().map(|t| t.label()).collect();
        let sel = ThemeName::ALL.iter().position(|t| *t == self.cfg.theme).unwrap_or(0);
        self.segmented(Rect::new(r.x, r.y + 14.0, r.w, 34.0), &names, sel, Key::Theme, th);
        // At rest, and shape.
        let half = (r.w - 16.0) / 2.0;
        let y = r.y + 58.0;
        label(self, "AT REST", r.x, y, half);
        label(self, "SHAPE", r.x + half + 16.0, y, half);
        self.segmented(Rect::new(r.x, y + 16.0, half, 34.0), &IDLE_NAMES, idle_index(self.cfg.idle), Key::Idle, th);
        let pill = (self.cfg.style == Style::Pill) as usize;
        self.segmented(Rect::new(r.x + half + 16.0, y + 16.0, half, 34.0), &["Notch", "Pill"], pill, Key::Style, th);
        // Switches, two columns.
        let c = &self.cfg;
        let rows = [
            [("Synced lyrics", Key::Lyrics, c.lyrics), ("Start with Windows", Key::Autostart, c.start_with_windows)],
            [("Sing in the pill", Key::LyricsPill, c.lyrics_in_pill), ("Hide over full-screen apps", Key::HideFullscreen, c.hide_in_fullscreen)],
            [("Ambient light", Key::AmbientOn, c.ambient.enabled), ("Music pill", Key::ShowMedia, c.show_media)],
        ];
        let mut y = r.y + 120.0;
        for row in rows {
            for (col, (name, key, on)) in row.into_iter().enumerate() {
                let x = r.x + col as f32 * (half + 16.0);
                self.gfx.text(name, Rect::new(x, y, half - 56.0, 30.0), 13.0, 600, th.text, Align::Left);
                self.switch(Rect::new(x + half - 44.0, y + 5.0, 40.0, 20.0), on, Action::Set(key, Val::Toggle), th);
            }
            y += 34.0;
        }
        // Tabs as chips: lit while they're on the row.
        let y = r.y + 230.0;
        self.gfx.text("Tabs", Rect::new(r.x, y, 60.0, 30.0), 13.0, 600, th.text, Align::Left);
        let mut x = r.x + 42.0;
        for p in [PageKind::Media, PageKind::Shelf, PageKind::Tasks, PageKind::Focus, PageKind::Clock, PageKind::Commands] {
            let on = self.cfg.pages.contains(&p);
            let name = if p == PageKind::Media { "Music" } else { p.label() };
            let w = self.gfx.measure(name, 12.0, 650) + 32.0;
            let b = Rect::new(x, y, w, 30.0);
            self.gfx.fill_rrect(b, 15.0, if on { th.ink(if self.hovered(b) { 0.22 } else { 0.16 }) } else { th.ink(if self.hovered(b) { 0.08 } else { 0.03 }) });
            self.gfx.icon(page_glyph(p), Rect::new(b.x + 6.0, b.y, 22.0, b.h), 12.0, if on { th.text } else { th.dim.fade(0.6) });
            self.gfx.text(name, Rect::new(b.x + 28.0, b.y, w - 32.0, b.h), 12.0, 650, if on { th.text } else { th.dim.fade(0.6) }, Align::Left);
            self.hit(b, Action::Set(Key::Page(p), Val::Toggle));
            x += w + 4.0;
        }
        // Everything else, in the window.
        let y = r.y + 270.0;
        self.gfx.fill_rect(Rect::new(r.x, y - 4.0, r.w, 1.0), th.ink(0.08));
        let adv = Rect::new(r.x, y, r.w, 26.0);
        if self.hovered(adv) {
            self.gfx.fill_rrect(adv.inset(-6.0, 0.0), 8.0, th.ink(0.05));
        }
        self.gfx.text("Advanced settings", Rect::new(adv.x, adv.y, 300.0, adv.h), 13.0, 600, th.text, Align::Left);
        self.gfx.icon('\u{E8A7}', Rect::new(adv.right() - 24.0, adv.y, 24.0, adv.h), 12.0, th.dim);
        self.hit(adv, Action::OpenSettings);
    }

    fn page_clock(&mut self, r: Rect, _now: f64, th: &Theme) {
        let t = sys::local_time();
        let big = self.clock_text(false);
        self.gfx.text(&big, Rect::new(r.x, r.y - 2.0, r.w * 0.62, 66.0), 54.0, 600, th.text, Align::Left);
        if !self.cfg.clock_24h {
            let w = self.gfx.measure(&big, 54.0, 600);
            let ampm = if t.hour < 12 { "AM" } else { "PM" };
            self.gfx.text(ampm, Rect::new(r.x + w + 8.0, r.y + 30.0, 40.0, 24.0), 15.0, 600, th.dim, Align::Left);
        }
        let date = format!("{}, {} {}", DAYS[t.weekday as usize % 7], t.day, MONTHS[(t.month as usize).clamp(1, 12) - 1]);
        self.gfx.text(&date, Rect::new(r.x, r.y + 64.0, r.w * 0.62, 22.0), 15.0, 500, th.dim, Align::Left);
        if let Some((pct, charging)) = sys::battery() {
            let s = if charging { format!("{pct}%  ·  Charging") } else { format!("{pct}%  battery") };
            self.gfx.text(&s, Rect::new(r.right() - 200.0, r.y + 8.0, 200.0, 22.0), 13.0, 500, th.dim, Align::Right);
        }
        self.gfx.text(&format!(":{:02}", t.second), Rect::new(r.right() - 80.0, r.y + 30.0, 80.0, 30.0), 22.0, 600, th.text.fade(0.8), Align::Right);
        // The minute going by.
        let line = Rect::new(r.x, r.bottom() - 6.0, r.w, 3.0);
        self.gfx.fill_rrect(line, 1.5, th.ink(0.1));
        let frac = (t.second as f32 + t.millis as f32 / 1000.0) / 60.0;
        self.gfx.fill_rrect(Rect::new(line.x, line.y, (line.w * frac).max(3.0), line.h), 1.5, th.accent);
    }

    fn page_commands(&mut self, r: Rect, th: &Theme) {
        if self.cfg.commands.is_empty() {
            self.gfx.text("No commands yet. Add them to config.json", Rect::new(r.x, r.y, r.w * 0.7, 38.0), 13.0, 500, th.dim, Align::Left);
            let b = Rect::new(r.right() - 130.0, r.y, 130.0, 38.0);
            self.gfx.fill_rrect(b, 19.0, th.ink(if self.hovered(b) { 0.16 } else { 0.08 }));
            self.gfx.text("Open config", b, 13.0, 600, th.text, Align::Center);
            self.hit(b, Action::OpenConfig);
            return;
        }
        let (cols, gap, bh) = (3usize, 10.0, 38.0);
        let bw = (r.w - gap * (cols as f32 - 1.0)) / cols as f32;
        for (i, c) in self.cfg.commands.clone().iter().enumerate() {
            let (row, col) = (i / cols, i % cols);
            let b = Rect::new(r.x + col as f32 * (bw + gap), r.y + row as f32 * (bh + gap), bw, bh);
            self.gfx.fill_rrect(b, bh / 2.0, th.ink(if self.hovered(b) { 0.17 } else { 0.08 }));
            self.gfx.text(&c.label, b.inset(12.0, 0.0), 13.0, 600, th.text, Align::Center);
            self.hit(b, Action::Command(i));
        }
    }
}

fn page_or_first(cfg: &Config, want: PageKind) -> PageKind {
    if cfg.pages.contains(&want) {
        want
    } else {
        cfg.pages.first().copied().unwrap_or(PageKind::Clock)
    }
}
