//! Everything you can change about Synapse lives in one JSON file:
//! `%APPDATA%\Synapse\config.json`. It is written with defaults on first run
//! and reloaded live whenever you save it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    /// Hangs from the top edge with concave shoulders.
    Notch,
    /// Floats a little below the top edge as a capsule.
    Pill,
}

/// A whole look: shape, colours and finish. Picking one on the island's
/// Themes page writes its `style` and `colors` here, so you can still tune
/// them; the finish (shadow, glow, glass, gradient) comes with the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeName {
    Midnight,
    Capsule,
    Glass,
    Neon,
    Light,
    Aurora,
    Pastel,
    Coffee,
}

impl ThemeName {
    pub const ALL: [ThemeName; 8] = [ThemeName::Midnight, ThemeName::Capsule, ThemeName::Glass, ThemeName::Neon, ThemeName::Light, ThemeName::Aurora, ThemeName::Pastel, ThemeName::Coffee];

    pub fn label(self) -> &'static str {
        match self {
            ThemeName::Midnight => "Midnight",
            ThemeName::Capsule => "Capsule",
            ThemeName::Glass => "Glass",
            ThemeName::Neon => "Neon",
            ThemeName::Light => "Light",
            ThemeName::Aurora => "Aurora",
            ThemeName::Pastel => "Pastel",
            ThemeName::Coffee => "Coffee",
        }
    }

    /// The shape and colours this theme starts from.
    pub fn preset(self) -> (Style, Colors) {
        let c = |island: &str, text: &str, dim: &str, accent: &str, border: &str, art: bool| Colors {
            island: island.into(),
            text: text.into(),
            dim: dim.into(),
            accent: accent.into(),
            border: border.into(),
            accent_from_art: art,
        };
        match self {
            ThemeName::Midnight => (Style::Notch, Colors::default()),
            ThemeName::Capsule => (Style::Pill, c("#000000", "#F5F5F7", "#F5F5F79E", "#30D158", "#FFFFFF1A", true)),
            // Light tint: Windows blurs what's behind, and it shows through.
            ThemeName::Glass => (Style::Pill, c("#16161C59", "#FFFFFF", "#FFFFFFCC", "#64D2FF", "#FFFFFF66", true)),
            ThemeName::Neon => (Style::Notch, c("#000000", "#F5F5F7", "#F5F5F79E", "#FF2D95", "#00000000", true)),
            ThemeName::Light => (Style::Pill, c("#F5F5F7", "#1C1C1E", "#1C1C1E99", "#007AFF", "#00000014", false)),
            ThemeName::Aurora => (Style::Pill, c("#2B1055", "#FFFFFF", "#FFFFFFB8", "#9D7BFF", "#FFFFFF33", false)),
            // Candy pink into baby blue, with plum ink and a lavender accent.
            ThemeName::Pastel => (Style::Pill, c("#FAD4E6", "#3E3352", "#3E3352A6", "#9B7BE0", "#FFFFFF99", false)),
            // Espresso into mocha, cream ink, caramel accent.
            ThemeName::Coffee => (Style::Pill, c("#2E1F17", "#F4E7D4", "#F4E7D4A8", "#D4A373", "#D4A37338", false)),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Idle {
    /// The time.
    Clock,
    /// Two eyes that blink, glance about and follow the pointer.
    Eyes,
    /// A small empty pill.
    Dot,
    /// Invisible until the pointer reaches the top edge.
    Hidden,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PageKind {
    Media,
    Shelf,
    Tasks,
    Focus,
    Clock,
    Commands,
    /// The theme picker (the palette button opens it; it has no tab).
    Themes,
    /// Add-ons: the last tab, and a tab for each add-on switched on.
    Addons,
    System,
    Caffeine,
    Prayer,
    Downloads,
    Script,
    /// Quick settings (the gear opens it; it has no tab).
    Settings,
}

impl PageKind {
    pub fn label(self) -> &'static str {
        match self {
            PageKind::Settings => "Settings",
            PageKind::Downloads => "Downloads",
            PageKind::Script => "Script",
            PageKind::Themes => "Themes",
            PageKind::Addons => "Add-ons",
            PageKind::System => "System",
            PageKind::Caffeine => "Caffeine",
            PageKind::Prayer => "Prayer",
            PageKind::Media => "Now Playing",
            PageKind::Shelf => "Shelf",
            PageKind::Tasks => "Tasks",
            PageKind::Focus => "Focus",
            PageKind::Clock => "Clock",
            PageKind::Commands => "Commands",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FocusConfig {
    pub focus_minutes: f32,
    pub break_minutes: f32,
    pub long_break_minutes: f32,
    /// A long break after this many focus sessions.
    pub long_break_every: u32,
    /// A break starts by itself when focus ends.
    pub auto_start_breaks: bool,
    /// Focus starts by itself when a break ends.
    pub auto_start_focus: bool,
    /// A chime when a phase ends.
    pub sound: bool,
}

impl Default for FocusConfig {
    fn default() -> Self {
        Self { focus_minutes: 25.0, break_minutes: 5.0, long_break_minutes: 15.0, long_break_every: 4, auto_start_breaks: true, auto_start_focus: false, sound: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AmbientSource {
    /// The album art while music plays, the screen otherwise.
    Auto,
    Art,
    Screen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AmbientConfig {
    /// The island and the screen edge glow in the colours of what you're looking at.
    pub enabled: bool,
    pub source: AmbientSource,
    /// Light spilling onto the screen's top edge.
    pub edge_glow: bool,
    /// The island's body tinted, with a halo around it.
    pub tint_island: bool,
    /// The glow breathes with the music's loudness.
    pub breathe: bool,
    /// How bright the glow is (0..1).
    pub intensity: f32,
    /// Send the colour to OpenRGB's SDK server (RGB keyboards, mice, strips).
    pub openrgb: bool,
    pub openrgb_port: u16,
}

impl Default for AmbientConfig {
    fn default() -> Self {
        Self { enabled: true, source: AmbientSource::Auto, edge_glow: true, tint_island: true, breathe: true, intensity: 0.7, openrgb: false, openrgb_port: 6742 }
    }
}

/// Add-ons: each stays off until you switch it on.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Addons {
    pub system: bool,
    pub caffeine: bool,
    pub prayer: bool,
    pub downloader: bool,
    pub script: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScriptConfig {
    /// How fast the script scrolls (DIPs a second).
    pub speed: f32,
    /// Text size.
    pub size: f32,
    /// Starts and pauses reading from anywhere ("" for none).
    pub hotkey: String,
    /// Screen shares and recordings leave the island out while the script shows.
    pub hide_from_capture: bool,
}

impl Default for ScriptConfig {
    fn default() -> Self {
        Self { speed: 32.0, size: 22.0, hotkey: "Ctrl+Alt+S".into(), hide_from_capture: true }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CaffeineConfig {
    /// When you go to bed (minutes after midnight; past midnight is fine, e.g. 1500 = 01:00).
    pub bedtime_min: u32,
    /// How long your body takes to clear half of it (hours; 5 is typical).
    pub half_life_h: f32,
    /// What's still fine at bedtime (mg).
    pub bedtime_limit_mg: f32,
    /// The quick-add buttons: name and mg.
    pub drinks: Vec<(String, f32)>,
}

impl Default for CaffeineConfig {
    fn default() -> Self {
        let d = |n: &str, mg: f32| (n.to_string(), mg);
        Self {
            bedtime_min: 23 * 60,
            half_life_h: 5.0,
            bedtime_limit_mg: 35.0,
            drinks: vec![d("Espresso", 63.0), d("Coffee", 95.0), d("Latte", 75.0), d("Tea", 47.0), d("Energy drink", 80.0), d("Cola", 34.0)],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrayerConfig {
    /// "auto" (by country), "mwl", "isna", "egypt", "makkah", "karachi", "tehran", "gulf", "kemenag", "diyanet", "jakim", "singapore".
    pub method: String,
    /// "auto" (by country), "standard" or "hanafi".
    pub asr: String,
    /// Where you are: found with Windows' location the first time, or set by hand.
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub place: String,
    /// The closed island counts down this many minutes before each prayer (0 = off).
    pub remind_min: u32,
    /// A sound at each prayer's time.
    pub sound: bool,
}

impl Default for PrayerConfig {
    fn default() -> Self {
        Self { method: "auto".into(), asr: "auto".into(), lat: None, lon: None, place: String::new(), remind_min: 10, sound: true }
    }
}

/// Bumped when a new version should change an existing config (see `migrate`).
pub const VERSION: u32 = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Colors {
    /// Colours are "#RRGGBB" or "#RRGGBBAA".
    pub island: String,
    pub text: String,
    pub dim: String,
    pub accent: String,
    /// A thin rim around the island ("#00000000" for none).
    pub border: String,
    /// Take the accent from the album art while music plays.
    pub accent_from_art: bool,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            island: "#000000".into(),
            text: "#F5F5F7".into(),
            dim: "#F5F5F79E".into(),
            accent: "#30D158".into(),
            border: "#FFFFFF14".into(),
            accent_from_art: true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Motion {
    /// 1.0 is the default pace; 2.0 is twice as fast.
    pub speed: f32,
    /// 0 = no overshoot, 0.6 = very springy.
    pub bounce: f32,
}

impl Default for Motion {
    fn default() -> Self {
        Self { speed: 1.0, bounce: 0.27 }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Command {
    pub label: String,
    /// A program, file, folder, URL or protocol (`ms-settings:`).
    pub target: String,
    #[serde(default)]
    pub args: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Which Synapse wrote this file's layout (missing = before pages could be added).
    #[serde(default)]
    pub version: u32,
    pub theme: ThemeName,
    pub style: Style,
    pub idle: Idle,
    pub clock_24h: bool,
    pub show_seconds: bool,
    pub font: String,
    /// Width of the open island (DIPs).
    pub width: f32,
    /// Move the island left (negative) or right of the screen's centre (DIPs).
    pub offset_x: f32,
    pub open_on_hover: bool,
    pub hover_delay_ms: u32,
    pub collapse_delay_ms: u32,
    pub hide_in_fullscreen: bool,
    pub show_media: bool,
    /// Synced lyrics from lrclib.net on the Now Playing page.
    pub lyrics: bool,
    /// ...and the line being sung in the closed music pill.
    pub lyrics_in_pill: bool,
    /// Shift the lyrics' timing: positive shows lines later, negative earlier.
    pub lyrics_offset_ms: i32,
    pub volume_hud: bool,
    /// "Charging" when you plug in, and low-battery warnings at 20 % and 10 %.
    pub power_hud: bool,
    /// Bluetooth devices connecting and disconnecting.
    pub bluetooth_hud: bool,
    /// A lock springing open when you unlock Windows.
    pub unlock_hud: bool,
    /// Muting and unmuting get their own pop-up.
    pub mute_hud: bool,
    /// Windows' Do Not Disturb turning on or off.
    pub dnd_hud: bool,
    /// Coloured dots beside the island while an app uses the camera
    /// (green), microphone (orange) or location (blue), and a pop-up naming it.
    pub privacy: bool,
    pub hud_seconds: f32,
    /// Opens and pins the island, e.g. "Ctrl+Alt+P" ("" for none).
    pub hotkey: String,
    /// Program that opens config.json.
    pub editor: String,
    pub start_with_windows: bool,
    pub pages: Vec<PageKind>,
    pub colors: Colors,
    pub motion: Motion,
    pub focus: FocusConfig,
    pub ambient: AmbientConfig,
    pub addons: Addons,
    pub caffeine: CaffeineConfig,
    pub prayer: PrayerConfig,
    pub script: ScriptConfig,
    pub commands: Vec<Command>,
}

impl Default for Config {
    fn default() -> Self {
        let cmd = |label: &str, target: &str| Command { label: label.into(), target: target.into(), args: String::new() };
        Self {
            version: VERSION,
            theme: ThemeName::Midnight,
            style: Style::Notch,
            idle: Idle::Clock,
            clock_24h: false,
            show_seconds: false,
            font: "Segoe UI Variable Text".into(),
            width: 560.0,
            offset_x: 0.0,
            open_on_hover: true,
            hover_delay_ms: 220,
            collapse_delay_ms: 0,
            hide_in_fullscreen: true,
            show_media: true,
            lyrics: true,
            lyrics_in_pill: true,
            lyrics_offset_ms: 0,
            volume_hud: true,
            power_hud: true,
            bluetooth_hud: true,
            unlock_hud: true,
            mute_hud: true,
            dnd_hud: true,
            privacy: true,
            hud_seconds: 1.8,
            hotkey: "Ctrl+Alt+P".into(),
            editor: "notepad.exe".into(),
            start_with_windows: false,
            pages: vec![PageKind::Media, PageKind::Shelf, PageKind::Tasks, PageKind::Focus, PageKind::Clock, PageKind::Commands],
            colors: Colors::default(),
            motion: Motion::default(),
            focus: FocusConfig::default(),
            ambient: AmbientConfig::default(),
            addons: Addons::default(),
            caffeine: CaffeineConfig::default(),
            prayer: PrayerConfig::default(),
            script: ScriptConfig::default(),
            commands: vec![
                cmd("Terminal", "cmd.exe"),
                cmd("Explorer", "explorer.exe"),
                cmd("Task Manager", "taskmgr.exe"),
                cmd("Settings", "ms-settings:"),
                cmd("GitHub", "https://github.com"),
                cmd("Synapse folder", "%APPDATA%\\Synapse"),
            ],
        }
    }
}

impl Config {
    /// Keep values in a range the island can draw.
    pub fn sanitized(mut self) -> Self {
        self.width = self.width.clamp(360.0, 900.0);
        self.hover_delay_ms = self.hover_delay_ms.min(3000);
        self.collapse_delay_ms = self.collapse_delay_ms.min(5000);
        self.hud_seconds = self.hud_seconds.clamp(0.5, 10.0);
        self.lyrics_offset_ms = self.lyrics_offset_ms.clamp(-10_000, 10_000);
        self.ambient.intensity = self.ambient.intensity.clamp(0.1, 1.0);
        self.caffeine.half_life_h = self.caffeine.half_life_h.clamp(2.0, 10.0);
        self.caffeine.bedtime_min = self.caffeine.bedtime_min.clamp(18 * 60, 28 * 60);
        self.caffeine.bedtime_limit_mg = self.caffeine.bedtime_limit_mg.clamp(0.0, 300.0);
        self.prayer.remind_min = self.prayer.remind_min.min(60);
        self.script.speed = self.script.speed.clamp(5.0, 200.0);
        self.script.size = self.script.size.clamp(14.0, 40.0);
        // Add-on pages come and go with their switches, never from `pages`.
        self.pages.retain(|p| matches!(p, PageKind::Media | PageKind::Shelf | PageKind::Tasks | PageKind::Focus | PageKind::Clock | PageKind::Commands));
        let f = &mut self.focus;
        f.focus_minutes = f.focus_minutes.clamp(1.0, 240.0);
        f.break_minutes = f.break_minutes.clamp(1.0, 120.0);
        f.long_break_minutes = f.long_break_minutes.clamp(1.0, 120.0);
        f.long_break_every = f.long_break_every.clamp(1, 12);
        self.motion.speed = self.motion.speed.clamp(0.3, 3.0);
        self.motion.bounce = self.motion.bounce.clamp(0.0, 0.6);
        let mut seen = Vec::new();
        self.pages.retain(|p| {
            let fresh = !seen.contains(p);
            seen.push(*p);
            fresh
        });
        if self.pages.is_empty() {
            self.pages.push(PageKind::Clock);
        }
        if self.font.trim().is_empty() {
            self.font = "Segoe UI".into();
        }
        self
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("Synapse")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
}

/// Read the config. A missing file is created with defaults; a broken one
/// returns the parse error (with its line) so the island can say so.
pub fn load() -> Result<Config, String> {
    let p = path();
    match std::fs::read_to_string(&p) {
        // Some editors save UTF-8 with a byte-order mark; JSON parsers refuse it.
        Ok(text) => serde_json::from_str::<Config>(text.trim_start_matches('\u{feff}'))
            .map(|c| migrate(c).sanitized())
            .map_err(|e| format!("config.json line {}: {}", e.line(), short(&e.to_string()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let c = Config::default();
            let _ = std::fs::create_dir_all(dir());
            if let Ok(s) = serde_json::to_string_pretty(&c) {
                let _ = std::fs::write(&p, s);
            }
            Ok(c)
        }
        Err(e) => Err(format!("config.json: {e}")),
    }
}

/// Write the whole config (the Settings window's changes).
pub fn save(c: &Config) -> Result<(), String> {
    let _ = std::fs::create_dir_all(dir());
    let s = serde_json::to_string_pretty(c).map_err(|e| e.to_string())?;
    std::fs::write(path(), s).map_err(|e| e.to_string())
}

/// Bring a config written by an older Synapse up to date, once: new pages join
/// the tab row (the file lists pages, so they wouldn't show otherwise).
/// Written back, so the file says what the island shows.
fn migrate(mut c: Config) -> Config {
    if c.version >= VERSION {
        return c;
    }
    if c.version < 2 {
        let at = c.pages.iter().position(|p| *p == PageKind::Media).map_or(0, |i| i + 1);
        for (k, p) in [PageKind::Shelf, PageKind::Focus].into_iter().enumerate() {
            if !c.pages.contains(&p) {
                c.pages.insert((at + k).min(c.pages.len()), p);
            }
        }
    }
    if c.version < 3 && !c.pages.contains(&PageKind::Tasks) {
        // Tasks joins after the Shelf (or at the front).
        let at = c.pages.iter().position(|p| *p == PageKind::Shelf).map_or(0, |i| i + 1);
        c.pages.insert(at.min(c.pages.len()), PageKind::Tasks);
    }
    c.version = VERSION;
    let pages = serde_json::to_value(&c.pages).unwrap_or_default();
    if set_key("pages", pages).and_then(|_| set_key("version", VERSION.into())).is_err() {
        crate::log("config: couldn't write the migrated pages back");
    }
    c
}

/// Set one top-level key in the file, keeping everything else as written.
pub fn set_key(key: &str, value: serde_json::Value) -> Result<(), String> {
    set_keys(vec![(key, value)])
}

/// Set several top-level keys in one write.
pub fn set_keys(pairs: Vec<(&str, serde_json::Value)>) -> Result<(), String> {
    let p = path();
    let text = std::fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let mut v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())?;
    if let Some(obj) = v.as_object_mut() {
        for (key, value) in pairs {
            obj.insert(key.into(), value);
        }
    }
    let s = serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?;
    std::fs::write(&p, s).map_err(|e| e.to_string())
}

fn short(s: &str) -> String {
    let s = s.split(" at line").next().unwrap_or(s);
    if s.chars().count() > 60 {
        s.chars().take(57).collect::<String>() + "..."
    } else {
        s.to_string()
    }
}

/// Expand `%VAR%` in a command target.
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        match after.find('%') {
            Some(b) => {
                let name = &after[..b];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[b + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let c = Config::default();
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&s).unwrap(), c);
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let c: Config = serde_json::from_str(r#"{ "style": "pill", "motion": { "bounce": 0.5 } }"#).unwrap();
        assert_eq!(c.style, Style::Pill);
        assert_eq!(c.motion.bounce, 0.5);
        assert_eq!(c.motion.speed, 1.0);
        assert_eq!(c.width, 560.0);
    }

    #[test]
    fn theme_presets_parse() {
        for t in ThemeName::ALL {
            let (_, c) = t.preset();
            for s in [&c.island, &c.text, &c.dim, &c.accent, &c.border] {
                assert!(s.starts_with('#') && (s.len() == 7 || s.len() == 9), "{t:?}: {s}");
            }
        }
        // Midnight is what Synapse always looked like.
        assert_eq!(ThemeName::Midnight.preset(), (Config::default().style, Config::default().colors));
    }

    #[test]
    fn sanitize_clamps() {
        let c = Config { width: 5000.0, pages: vec![], ..Config::default() }.sanitized();
        assert_eq!(c.width, 900.0);
        assert_eq!(c.pages, vec![PageKind::Clock]);
    }
}
