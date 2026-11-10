//! Reload the config the moment it's saved: a thread blocks on a folder
//! change notification (zero CPU while nothing changes).

use crate::app::{Event, Sink};
use std::time::{Duration, SystemTime};
use windows::core::HSTRING;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Storage::FileSystem::{FindFirstChangeNotificationW, FindNextChangeNotification, FILE_NOTIFY_CHANGE_LAST_WRITE};
use windows::Win32::System::Threading::{WaitForSingleObject, INFINITE};

pub fn start(sink: Sink) {
    std::thread::Builder::new()
        .name("synapse-config".into())
        .spawn(move || run(sink))
        .expect("config watcher thread");
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(crate::config::path()).and_then(|m| m.modified()).ok()
}

fn run(sink: Sink) {
    let dir = crate::config::dir();
    let _ = std::fs::create_dir_all(&dir);
    let handle = match unsafe { FindFirstChangeNotificationW(&HSTRING::from(dir.as_os_str()), false, FILE_NOTIFY_CHANGE_LAST_WRITE) } {
        Ok(h) => h,
        Err(e) => {
            crate::log(&format!("config watcher: {e}"));
            return;
        }
    };
    let mut seen = modified();
    loop {
        unsafe {
            WaitForSingleObject(HANDLE(handle.0), INFINITE);
        }
        // Editors save in bursts (truncate, write, rename): let it land.
        std::thread::sleep(Duration::from_millis(150));
        let now = modified();
        if now != seen {
            seen = now;
            sink.send(Event::ConfigChanged);
        }
        if unsafe { FindNextChangeNotification(handle) }.is_err() {
            return;
        }
    }
}
