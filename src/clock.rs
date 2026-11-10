//! The frame clock: a thread that waits for the display's vertical blank and
//! posts `WM_APP_FRAME`, only while something moves. Parked, it sleeps on a
//! condvar and costs nothing.

use crate::app::WM_APP_FRAME;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, LPARAM, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, IDXGIOutput};
use windows::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTOPRIMARY};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

struct Shared {
    active: Mutex<bool>,
    cv: Condvar,
    /// A frame is posted and not handled yet (never queue two).
    pending: AtomicBool,
    /// The clock woke from being parked (the next frame steps one refresh).
    resumed: AtomicBool,
    stop: AtomicBool,
}

pub struct FrameClock {
    s: Arc<Shared>,
}

impl FrameClock {
    pub fn start(hwnd: HWND) -> Self {
        let s = Arc::new(Shared {
            active: Mutex::new(false),
            cv: Condvar::new(),
            pending: AtomicBool::new(false),
            resumed: AtomicBool::new(true),
            stop: AtomicBool::new(false),
        });
        let t = s.clone();
        let h = hwnd.0 as isize;
        std::thread::Builder::new()
            .name("synapse-frames".into())
            .spawn(move || run(t, h))
            .expect("frame clock thread");
        Self { s }
    }

    pub fn set_active(&self, on: bool) {
        let mut a = self.s.active.lock().unwrap();
        if *a != on {
            *a = on;
            self.s.cv.notify_all();
        }
    }

    /// Call when a frame message arrives: whether the clock was parked before it.
    pub fn take_resumed(&self) -> bool {
        self.s.resumed.swap(false, Ordering::AcqRel)
    }

    pub fn frame_handled(&self) {
        self.s.pending.store(false, Ordering::Release);
    }
}

impl Drop for FrameClock {
    fn drop(&mut self) {
        self.s.stop.store(true, Ordering::Relaxed);
        self.s.cv.notify_all();
    }
}

/// The DXGI output of the primary display (its vblank paces the frames).
fn primary_output() -> Option<IDXGIOutput> {
    unsafe {
        let mon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let f: IDXGIFactory1 = CreateDXGIFactory1().ok()?;
        let mut a = 0;
        while let Ok(adapter) = f.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(out) = adapter.EnumOutputs(o) {
                if out.GetDesc().is_ok_and(|d| d.Monitor == mon) {
                    return Some(out);
                }
                o += 1;
            }
            a += 1;
        }
        None
    }
}

fn run(s: Arc<Shared>, hwnd: isize) {
    let output = primary_output();
    crate::log(if output.is_some() { "frame clock: display vblank" } else { "frame clock: DWM flush" });
    loop {
        {
            let mut a = s.active.lock().unwrap();
            if !*a {
                s.resumed.store(true, Ordering::Release);
            }
            while !*a && !s.stop.load(Ordering::Relaxed) {
                a = s.cv.wait(a).unwrap();
            }
        }
        if s.stop.load(Ordering::Relaxed) {
            break;
        }
        let t0 = Instant::now();
        let ok = match &output {
            Some(o) => unsafe { o.WaitForVBlank().is_ok() },
            None => unsafe { DwmFlush().is_ok() },
        };
        // A source that doesn't block would spin: pace it by hand.
        if !ok || t0.elapsed() < Duration::from_micros(500) {
            std::thread::sleep(Duration::from_millis(7));
        }
        if !*s.active.lock().unwrap() {
            continue;
        }
        if !s.pending.swap(true, Ordering::AcqRel) {
            unsafe {
                let _ = PostMessageW(HWND(hwnd as *mut _), WM_APP_FRAME, WPARAM(0), LPARAM(0));
            }
        }
    }
}
