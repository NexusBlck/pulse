//! Small pieces of Windows: the tray icon and its menu, running commands,
//! start with Windows, the global hotkey, the clock and the battery.

use crate::app::WM_APP_TRAY;
use windows::core::{w, HSTRING, PCWSTR};
use windows::Win32::Foundation::{BOOL, HWND, POINT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::System::Registry::{RegDeleteKeyValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTimeAsFileTime};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::{ShellExecuteW, Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const MENU_CONFIG: usize = 1;
pub const MENU_RELOAD: usize = 2;
pub const MENU_AUTOSTART: usize = 3;
pub const MENU_QUIT: usize = 4;
pub const MENU_SETTINGS: usize = 5;

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Self {
        let icon = make_icon().unwrap_or_else(|| unsafe { LoadIconW(None, IDI_APPLICATION).unwrap_or_default() });
        let t = Self { hwnd, icon };
        t.add();
        t
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut d = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: 1,
            uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.icon,
            ..Default::default()
        };
        for (i, c) in "Synapse".encode_utf16().enumerate() {
            d.szTip[i] = c;
        }
        d
    }

    /// Add the icon (again, after Explorer restarts).
    pub fn add(&self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &self.data());
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &self.data());
            let _ = DestroyIcon(self.icon);
        }
    }
}

/// Show the menu at the pointer; returns the picked item.
pub fn menu(hwnd: HWND, autostart: bool) -> usize {
    unsafe {
        let Ok(m) = CreatePopupMenu() else { return 0 };
        let _ = AppendMenuW(m, MF_STRING, MENU_SETTINGS, w!("Settings..."));
        let _ = AppendMenuW(m, MF_STRING, MENU_CONFIG, w!("Open config file"));
        let _ = AppendMenuW(m, MF_STRING, MENU_RELOAD, w!("Reload config"));
        let _ = AppendMenuW(m, MF_STRING | if autostart { MF_CHECKED } else { MF_UNCHECKED }, MENU_AUTOSTART, w!("Start with Windows"));
        let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(m, MF_STRING, MENU_QUIT, w!("Quit Synapse"));
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(hwnd);
        let picked = TrackPopupMenu(m, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, p.x, p.y, 0, hwnd, None);
        let _ = DestroyMenu(m);
        picked.0 as usize
    }
}

/// Synapse's icon (the tray's), for windows.
pub fn app_icon() -> HICON {
    make_icon().unwrap_or_else(|| unsafe { LoadIconW(None, IDI_APPLICATION).unwrap_or_default() })
}

/// Synapse's tray icon, drawn here: a dark capsule with a light rim and a dot.
fn make_icon() -> Option<HICON> {
    const N: i32 = 32;
    let mut px = vec![0u8; (N * N * 4) as usize];
    let (cx, cy, half_w, rad) = (16.0f32, 16.0f32, 9.0f32, 6.5f32);
    for y in 0..N {
        for x in 0..N {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Distance to the capsule's spine, minus its radius.
            let dx = (fx - cx).abs() - half_w;
            let d = (dx.max(0.0).powi(2) + (fy - cy).powi(2)).sqrt() - rad;
            let cover = (0.5 - d).clamp(0.0, 1.0);
            let rim = (1.0 - (d + 1.2).abs()).clamp(0.0, 1.0);
            let dot = (0.5 - (((fx - 21.0).powi(2) + (fy - cy).powi(2)).sqrt() - 2.2)).clamp(0.0, 1.0);
            let a = cover.max(rim);
            // Straight colour: dark body, light rim, green dot; then premultiply.
            let mix = |base: f32, rim_c: f32, dot_c: f32| ((base * (1.0 - rim) + rim_c * rim) * (1.0 - dot) + dot_c * dot) * a;
            let i = ((y * N + x) * 4) as usize;
            px[i] = mix(18.0, 245.0, 88.0) as u8;
            px[i + 1] = mix(18.0, 245.0, 209.0) as u8;
            px[i + 2] = mix(18.0, 245.0, 48.0) as u8;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: N,
                biHeight: -N,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &bmi, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        let mask = CreateBitmap(N, N, 1, 1, None);
        let info = ICONINFO { fIcon: BOOL(1), xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color);
        let _ = DeleteObject(mask);
        icon
    }
}

/// Open a file, folder, program, URL or protocol, as Run would.
pub fn launch(target: &str, args: &str) {
    let target = crate::config::expand_env(target);
    let args = crate::config::expand_env(args);
    let args_w = HSTRING::from(args.as_str());
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(target.as_str()),
            if args.is_empty() { PCWSTR::null() } else { PCWSTR(args_w.as_ptr()) },
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if (r.0 as isize) <= 32 {
        crate::log(&format!("could not open {target} (error {})", r.0 as isize));
    }
}

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

pub fn set_autostart(on: bool) {
    unsafe {
        if on {
            let Ok(exe) = std::env::current_exe() else { return };
            let value: Vec<u16> = format!("\"{}\"", exe.display()).encode_utf16().chain(Some(0)).collect();
            let _ = RegSetKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Synapse"), REG_SZ.0, Some(value.as_ptr() as *const _), (value.len() * 2) as u32);
        } else {
            let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Synapse"));
        }
    }
}

/// "Ctrl+Alt+P" -> (modifiers, virtual key).
pub fn parse_hotkey(s: &str) -> Option<(HOT_KEY_MODIFIERS, u32)> {
    let mut mods = MOD_NOREPEAT;
    let mut key = None;
    for part in s.split('+').map(|p| p.trim().to_ascii_lowercase()) {
        match part.as_str() {
            "ctrl" | "control" => mods |= MOD_CONTROL,
            "alt" => mods |= MOD_ALT,
            "shift" => mods |= MOD_SHIFT,
            "win" | "super" => mods |= MOD_WIN,
            "space" => key = Some(0x20),
            "enter" => key = Some(0x0D),
            "tab" => key = Some(0x09),
            "`" | "backtick" => key = Some(0xC0),
            k if k.len() == 1 && k.chars().all(|c| c.is_ascii_alphanumeric()) => key = Some(k.to_ascii_uppercase().as_bytes()[0] as u32),
            k if k.starts_with('f') && k[1..].parse::<u32>().is_ok_and(|n| (1..=24).contains(&n)) => key = Some(0x6F + k[1..].parse::<u32>().unwrap()),
            "" => {}
            _ => return None,
        }
    }
    key.map(|k| (mods, k))
}

/// Hotkey ids: the island's, and the Script reader's.
pub const HOTKEY_ISLAND: i32 = 1;
pub const HOTKEY_SCRIPT: i32 = 2;

pub fn register_hotkey(hwnd: HWND, id: i32, spec: &str) -> Result<(), String> {
    unsafe {
        let _ = UnregisterHotKey(hwnd, id);
    }
    if spec.trim().is_empty() {
        return Ok(());
    }
    let (mods, vk) = parse_hotkey(spec).ok_or_else(|| format!("hotkey \"{spec}\" isn't understood"))?;
    unsafe { RegisterHotKey(hwnd, id, mods, vk) }.map_err(|_| format!("hotkey {spec} is taken by another app"))
}

/// The text on the clipboard, if any.
pub fn clipboard_text(hwnd: HWND) -> Option<String> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, OpenClipboard};
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        OpenClipboard(hwnd).ok()?;
        let text = GetClipboardData(CF_UNICODETEXT).ok().and_then(|h| {
            let g = HGLOBAL(h.0);
            let p = GlobalLock(g) as *const u16;
            if p.is_null() {
                return None;
            }
            let s = PCWSTR(p).to_string().ok();
            let _ = GlobalUnlock(g);
            s
        });
        let _ = CloseClipboard();
        text
    }
}

#[derive(Clone, Copy, Debug)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub weekday: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
    pub millis: u16,
}

pub fn local_time() -> LocalTime {
    let t = unsafe { GetLocalTime() };
    LocalTime {
        year: t.wYear,
        month: t.wMonth,
        day: t.wDay,
        weekday: t.wDayOfWeek,
        hour: t.wHour,
        minute: t.wMinute,
        second: t.wSecond,
        millis: t.wMilliseconds,
    }
}

/// Now as FILETIME ticks (100 ns since 1601, the same scale WinRT uses).
pub fn filetime_now() -> i64 {
    let f = unsafe { GetSystemTimeAsFileTime() };
    ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64
}

/// Battery percent and whether it's charging; None without a battery.
pub fn battery() -> Option<(u8, bool)> {
    let mut s = SYSTEM_POWER_STATUS::default();
    unsafe { GetSystemPowerStatus(&mut s).ok()? };
    if s.BatteryFlag & 128 != 0 || s.BatteryLifePercent > 100 {
        return None;
    }
    Some((s.BatteryLifePercent, s.ACLineStatus == 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkeys() {
        assert_eq!(parse_hotkey("Ctrl+Alt+P"), Some((MOD_NOREPEAT | MOD_CONTROL | MOD_ALT, 'P' as u32)));
        assert_eq!(parse_hotkey("Win+Shift+Space"), Some((MOD_NOREPEAT | MOD_WIN | MOD_SHIFT, 0x20)));
        assert_eq!(parse_hotkey("Alt+F2").map(|k| k.1), Some(0x71));
        assert_eq!(parse_hotkey("Ctrl+Banana"), None);
        assert_eq!(parse_hotkey("Ctrl+Alt"), None);
    }
}
