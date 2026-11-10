//! The glow spilling onto the screen's top edge: light in the palette's
//! colours blooming out from the island and running along the edge. A
//! click-through window across the top of the monitor, just under the
//! island; it draws only when the light changes.

use crate::ambient::{lerp, Palette};
use crate::gfx::{Color, Gfx, Rect};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

/// How far down the screen the light reaches (DIPs).
const H: f32 = 110.0;

unsafe extern "system" fn proc_(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Never in the way: hit tests go straight through.
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

pub struct EdgeGlow {
    hwnd: HWND,
    gfx: Gfx,
    x: i32,
    y: i32,
    w_dip: f32,
    shown: bool,
    /// What was drawn last (palette, island centre, strength), to skip repeats.
    last: Option<(Palette, f32, f32)>,
}

impl EdgeGlow {
    /// Across the top of the monitor at (`x`, `y`), `w` pixels wide.
    pub fn new(x: i32, y: i32, w: i32, scale: f32, font: &str) -> Option<Self> {
        unsafe {
            let hinst = GetModuleHandleW(None).ok()?;
            let wc = WNDCLASSEXW { cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(proc_), hInstance: hinst.into(), lpszClassName: w!("Synapse.Glow"), ..Default::default() };
            RegisterClassExW(&wc);
            let h = (H * scale).ceil() as i32;
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("Synapse.Glow"),
                w!("Synapse glow"),
                WS_POPUP,
                x,
                y,
                w,
                h,
                None,
                None,
                hinst,
                None,
            )
            .ok()?;
            let mut gfx = Gfx::new(font).ok()?;
            gfx.resize(w, h, scale).ok()?;
            Some(Self { hwnd, gfx, x, y, w_dip: w as f32 / scale, shown: false, last: None })
        }
    }

    /// Light the edge in `p`, brightest around the island at `cx` (DIPs from
    /// this window's left), at `strength` (0..1). Keeps it just under `island`.
    pub fn draw(&mut self, p: Palette, cx: f32, strength: f32, island: HWND) {
        if let Some((lp, lcx, ls)) = self.last {
            if lp.distance(&p) < 0.004 && (lcx - cx).abs() < 0.5 && (ls - strength).abs() < 0.01 {
                return;
            }
        }
        self.last = Some((p, cx, strength));
        if self.gfx.begin().is_err() {
            return;
        }
        let k = strength.clamp(0.0, 1.0);
        let full = Rect::new(0.0, 0.0, self.w_dip, H);
        let mid = p.mix();
        // Each side's colour washing along its half of the edge.
        for (c, at) in [(p.a, 0.22), (p.b, 0.78)] {
            self.gfx.fill_rect_radial(full, (self.w_dip * at, 0.0), self.w_dip * 0.42, H * 0.55, c.fade(0.26 * k), c.fade(0.0));
        }
        // The bloom around the island.
        self.gfx.fill_rect_radial(full, (cx, 0.0), 420.0, H, mid.fade(0.5 * k), mid.fade(0.0));
        // A thin line of light right on the edge, left colour to right.
        let a = lerp(p.a, Color::white(1.0), 0.25);
        let b = lerp(p.b, Color::white(1.0), 0.25);
        self.gfx.fill_rect_gradient(Rect::new(0.0, 0.0, self.w_dip, 2.0), (0.0, 0.0), (self.w_dip, 0.0), a.fade(0.55 * k), b.fade(0.55 * k));
        self.gfx.end();
        self.gfx.present(self.hwnd, self.x, self.y);
        unsafe {
            if !self.shown {
                self.shown = true;
                let _ = ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            }
            // Under the island, over everything else.
            let _ = SetWindowPos(self.hwnd, island, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn hide(&mut self) {
        if self.shown {
            self.shown = false;
            self.last = None;
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
        }
    }
}

impl Drop for EdgeGlow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
