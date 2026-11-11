//! Privacy indicators: which apps are using the camera, the microphone or
//! the location right now.
//!
//! Windows records every use under `CapabilityAccessManager\ConsentStore`
//! (a start and a stop time per app; stop is 0 while it's in use). A thread
//! asks Windows to signal when those keys change and reads them again only
//! then: nothing polls.

use crate::app::{Event, Sink};
use std::time::Duration;
use windows::core::{HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{ERROR_SUCCESS, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForMultipleObjects, INFINITE};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Sense {
    Camera,
    Microphone,
    Location,
}

impl Sense {
    pub const ALL: [Sense; 3] = [Sense::Camera, Sense::Microphone, Sense::Location];

    fn key(self) -> &'static str {
        match self {
            Sense::Camera => "webcam",
            Sense::Microphone => "microphone",
            Sense::Location => "location",
        }
    }

    pub fn noun(self) -> &'static str {
        match self {
            Sense::Camera => "camera",
            Sense::Microphone => "microphone",
            Sense::Location => "location",
        }
    }
}

/// An app using a sense right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InUse {
    pub sense: Sense,
    pub app: String,
}

const ROOT: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\";

pub fn start(sink: Sink) {
    std::thread::Builder::new()
        .name("synapse-privacy".into())
        .spawn(move || run(sink))
        .expect("privacy thread");
}

fn open(path: &str) -> Option<HKEY> {
    let mut k = HKEY::default();
    let r = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, &HSTRING::from(path), 0, KEY_READ | KEY_NOTIFY, &mut k) };
    (r == ERROR_SUCCESS).then_some(k)
}

fn run(sink: Sink) {
    let keys: Vec<HKEY> = Sense::ALL.iter().filter_map(|s| open(&format!("{ROOT}{}", s.key()))).collect();
    if keys.is_empty() {
        return;
    }
    let events: Vec<HANDLE> = keys.iter().filter_map(|_| unsafe { CreateEventW(None, false, false, PCWSTR::null()).ok() }).collect();
    let mut last = Vec::new();
    loop {
        // Re-arm before reading, so a change while reading isn't missed.
        for (k, e) in keys.iter().zip(&events) {
            unsafe {
                let _ = RegNotifyChangeKeyValue(*k, true, REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_CHANGE_NAME, *e, true);
            }
        }
        let now = scan();
        if now != last {
            sink.send(Event::Privacy(now.clone()));
            last = now;
        }
        let w = unsafe { WaitForMultipleObjects(&events, false, INFINITE) };
        if w.0 >= WAIT_OBJECT_0.0 + events.len() as u32 {
            return;
        }
        // A burst of writes (start, then the app's other keys): let it land.
        std::thread::sleep(Duration::from_millis(150));
    }
}

/// Every app using a sense right now (Synapse itself left out).
pub fn scan() -> Vec<InUse> {
    let mut out = Vec::new();
    for s in Sense::ALL {
        let base = format!("{ROOT}{}", s.key());
        for (name, path) in subkeys(&base).into_iter().flat_map(|n| {
            // Desktop apps live one level down, under NonPackaged.
            if n == "NonPackaged" {
                subkeys(&format!("{base}\\NonPackaged")).into_iter().map(|c| (c.clone(), format!("{base}\\NonPackaged\\{c}"))).collect::<Vec<_>>()
            } else {
                vec![(n.clone(), format!("{base}\\{n}"))]
            }
        }) {
            if in_use(&path) {
                let app = pretty(&name);
                if !app.eq_ignore_ascii_case("synapse") && !out.iter().any(|u: &InUse| u.sense == s && u.app == app) {
                    out.push(InUse { sense: s, app });
                }
            }
        }
    }
    out
}

fn subkeys(path: &str) -> Vec<String> {
    let Some(k) = open(path) else { return Vec::new() };
    let mut v = Vec::new();
    let mut i = 0;
    loop {
        let mut buf = [0u16; 512];
        let mut len = buf.len() as u32;
        let r = unsafe { RegEnumKeyExW(k, i, PWSTR(buf.as_mut_ptr()), &mut len, None, PWSTR::null(), None, None) };
        if r != ERROR_SUCCESS {
            break;
        }
        v.push(String::from_utf16_lossy(&buf[..len as usize]));
        i += 1;
    }
    unsafe {
        let _ = RegCloseKey(k);
    }
    v
}

fn qword(k: HKEY, name: &str) -> Option<u64> {
    let mut data = 0u64;
    let mut size = 8u32;
    let r = unsafe { RegQueryValueExW(k, &HSTRING::from(name), None, None, Some(&mut data as *mut u64 as *mut u8), Some(&mut size)) };
    (r == ERROR_SUCCESS).then_some(data)
}

/// In use: it has started and hasn't stopped.
fn in_use(path: &str) -> bool {
    let Some(k) = open(path) else { return false };
    let r = matches!((qword(k, "LastUsedTimeStart"), qword(k, "LastUsedTimeStop")), (Some(s), Some(0)) if s != 0);
    unsafe {
        let _ = RegCloseKey(k);
    }
    r
}

/// "C:#Program Files#Zoom#bin#Zoom.exe" -> "Zoom",
/// "Microsoft.WindowsCamera_8wekyb3d8bbwe" -> "Camera".
pub fn pretty(key: &str) -> String {
    if key.contains('#') {
        let file = key.rsplit('#').next().unwrap_or(key);
        let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(file);
        return match stem.to_ascii_lowercase().as_str() {
            "msedge" => "Edge".into(),
            "chrome" => "Chrome".into(),
            "firefox" => "Firefox".into(),
            "ms-teams" | "teams" => "Teams".into(),
            "discord" => "Discord".into(),
            _ => {
                let mut c = stem.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        };
    }
    let name = key.split('_').next().unwrap_or(key);
    let last = name.rsplit('.').next().unwrap_or(name);
    match last {
        "WindowsCamera" => "Camera".into(),
        "immersivecontrolpanel" => "Settings".into(),
        "WebExperience" => "Widgets".into(),
        "SoundRecorder" => "Sound Recorder".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(pretty("C:#Program Files (x86)#Microsoft#Edge#Application#msedge.exe"), "Edge");
        assert_eq!(pretty("C:#Users#me#AppData#Roaming#Zoom#bin#zoom.exe"), "Zoom");
        assert_eq!(pretty("Microsoft.WindowsCamera_8wekyb3d8bbwe"), "Camera");
        assert_eq!(pretty("windows.immersivecontrolpanel_cw5n1h2txyewy"), "Settings");
    }

    #[test]
    fn reads_without_crashing() {
        // Whatever is in use right now; just make sure the walk works.
        let _ = scan();
    }
}
