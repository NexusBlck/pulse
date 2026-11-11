//! Synapse: a living island at the top of the screen.
//!
//! One UI thread owns the window, the renderer and all state. Services
//! (media, volume, the config watcher) run on their own threads or COM
//! callbacks and report through `app::Sink` (a channel plus a posted window
//! message). Frames are drawn only while something moves.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod ambient;
mod app;
mod backdrop;
mod bluetooth;
mod caffeine;
mod clock;
mod config;
mod downloader;
mod edge;
mod focus;
mod gfx;
mod island;
mod lyrics;
mod media;
mod openrgb;
mod phone;
mod prayer;
mod privacy;
mod settings;
mod shelf;
mod spring;
mod sys;
mod system;
mod tasks;
mod volume;
mod watch;

use std::io::Write;

/// Append a line to `%LOCALAPPDATA%\Synapse\synapse.log`.
pub fn log(line: &str) {
    let dir = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(|| ".".into()).join("Synapse");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("synapse.log");
    // Start over once it gets big; it's for troubleshooting, not history.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 512 * 1024) {
        let _ = std::fs::remove_file(&path);
    }
    let t = sys::local_time();
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {line}", t.year, t.month, t.day, t.hour, t.minute, t.second);
    }
    #[cfg(debug_assertions)]
    eprintln!("{line}");
}

fn main() {
    std::panic::set_hook(Box::new(|info| log(&format!("panic: {info}"))));
    std::process::exit(app::run());
}
