//! Real glass: a plain (not layered) window just under the island, shaped
//! to its outline, that asks Windows to blur whatever is behind it. The
//! compositor does the blur on the GPU, so Synapse never copies the screen;
//! the island draws its tint, sheen and rim on top.

use windows::core::{s, w};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE};
use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, GetStockObject, SetWindowRgn, BLACK_BRUSH, HBRUSH};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::WindowsAndMessaging::*;

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    /// AABBGGRR tint over the blur.
    gradient: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionData {
    attribute: u32,
    data: *mut core::ffi::c_void,
    size: usize,
}

const WCA_ACCENT_POLICY: u32 = 19;
const ACCENT_ENABLE_ACRYLICBLURBEHIND: u32 = 4;

type SetCompositionFn = unsafe extern "system" fn(HWND, *mut CompositionData) -> i32;

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub struct Backdrop {
    hwnd: HWND,
    /// Where the window is (screen px: x, y, w, h, corner radius). Windows
    /// blurs the whole window whatever its region, so the window itself is
    /// the island's size, and follows it as it morphs.
    place: Option<(i32, i32, i32, i32, i32)>,
    shown: bool,
}

impl Backdrop {
    pub fn new() -> Option<Self> {
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(proc_),
                hInstance: hinst.into(),
                // Black reads as "nothing" to the compositor: only the blur shows.
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: w!("Synapse.Glass"),
                ..Default::default()
            };
            RegisterClassExW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TOPMOST | WS_EX_TRANSPARENT,
                w!("Synapse.Glass"),
                w!("Synapse glass"),
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
            .ok()?;
            let user32 = GetModuleHandleW(w!("user32.dll")).ok()?;
            let f = GetProcAddress(user32, s!("SetWindowCompositionAttribute"))?;
            let set: SetCompositionFn = std::mem::transmute(f);
            let mut accent = AccentPolicy { state: ACCENT_ENABLE_ACRYLICBLURBEHIND, flags: 0, gradient: 0x1A10_1010, animation: 0 };
            let mut data = CompositionData { attribute: WCA_ACCENT_POLICY, data: &mut accent as *mut _ as *mut _, size: std::mem::size_of::<AccentPolicy>() };
            if set(hwnd, &mut data) == 0 {
                crate::log("glass: Windows wouldn't blur behind the island");
            }
            // Rounded corners from Windows 11 itself (where the region can't help).
            let round: u32 = 2; // DWMWCP_ROUND
            let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as *const _, 4);
            Some(Self { hwnd, place: None, shown: false })
        }
    }

    /// Blur behind the island's body: `body` in screen pixels (x, y, w, h),
    /// its corners `radius` px; `hangs` squares the top against the edge.
    pub fn show(&mut self, body: (i32, i32, i32, i32), radius: i32, hangs: bool, island: HWND) {
        unsafe {
            let place = (body.0, body.1, body.2, body.3, radius);
            if self.place != Some(place) {
                self.place = Some(place);
                let _ = SetWindowPos(self.hwnd, island, body.0, body.1, body.2.max(1), body.3.max(1), SWP_NOACTIVATE);
                // The shape too, where Windows honours it.
                let top = if hangs { -radius } else { 0 };
                let rgn = CreateRoundRectRgn(0, top, body.2 + 1, body.3 + 1, radius * 2, radius * 2);
                SetWindowRgn(self.hwnd, rgn, true);
            }
            if !self.shown {
                self.shown = true;
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            let _ = SetWindowPos(self.hwnd, island, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn hide(&mut self) {
        if self.shown {
            self.shown = false;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
}

impl Drop for Backdrop {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
