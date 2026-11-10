//! Ambient light: a palette of two colours (left and right) taken from the
//! album art or from what's on screen, which the island, the glow along the
//! screen's top edge and RGB keyboards all wear.
//!
//! The screen is sampled as a 64×32 snapshot once a second, from below the
//! top strip (where the island and its glow live, so they never feed back),
//! on a thread that sleeps whenever nothing wants it.

use crate::app::{Event, Sink};
use crate::gfx::{Color, Image};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    /// The left and right colours (the same when the picture has one).
    pub a: Color,
    pub b: Color,
}

impl Palette {
    pub fn mix(&self) -> Color {
        lerp(self.a, self.b, 0.5)
    }

    /// Ease towards `to` by `k` (0..1).
    pub fn toward(&self, to: &Palette, k: f32) -> Palette {
        Palette { a: lerp(self.a, to.a, k), b: lerp(self.b, to.b, k) }
    }

    pub fn distance(&self, o: &Palette) -> f32 {
        let d = |x: Color, y: Color| (x.r - y.r).abs() + (x.g - y.g).abs() + (x.b - y.b).abs();
        d(self.a, o.a).max(d(self.b, o.b))
    }
}

pub fn lerp(x: Color, y: Color, t: f32) -> Color {
    Color::rgba(x.r + (y.r - x.r) * t, x.g + (y.g - x.g) * t, x.b + (y.b - x.b) * t, x.a + (y.a - x.a) * t)
}

/// The most colourful average of some BGRA pixels, lifted so it glows on
/// black. Greys barely count; None when there's no colour to speak of.
pub fn vivid<'a>(pixels: impl Iterator<Item = &'a [u8]>) -> Option<[f32; 3]> {
    let (mut r, mut g, mut b, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for p in pixels {
        let (pb, pg, pr) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
        let (hi, lo) = (pr.max(pg).max(pb), pr.min(pg).min(pb));
        let sat = if hi > 0.0 { (hi - lo) / hi } else { 0.0 };
        let w = sat * sat * hi;
        if w > 0.02 {
            r += pr * w;
            g += pg * w;
            b += pb * w;
            n += w;
        }
    }
    if n < 0.5 {
        return None;
    }
    let (r, g, b) = (r / n, g / n, b / n);
    let lift = (0.9 / r.max(g).max(b).max(0.01)).max(1.0);
    Some([(r * lift).min(1.0), (g * lift).min(1.0), (b * lift).min(1.0)])
}

/// Left and right halves' colours of a picture (BGRA, `w` wide).
pub fn from_pixels(px: &[u8], w: usize) -> Option<Palette> {
    if w == 0 {
        return None;
    }
    let half = |left: bool| px.chunks_exact(4).enumerate().filter(move |(i, _)| ((i % w) < w / 2) == left).map(|(_, p)| p);
    let (l, r) = (vivid(half(true)), vivid(half(false)));
    let col = |c: [f32; 3]| Color::rgba(c[0], c[1], c[2], 1.0);
    match (l, r) {
        (Some(l), Some(r)) => Some(Palette { a: col(l), b: col(r) }),
        (Some(c), None) | (None, Some(c)) => Some(Palette { a: col(c), b: col(c) }),
        _ => None,
    }
}

pub fn from_image(img: &Image) -> Option<Palette> {
    from_pixels(&img.px, img.w as usize)
}

// ------------------------------------------------------------ screen sampler

struct Shared {
    on: Mutex<bool>,
    cv: Condvar,
    stop: AtomicBool,
    area: Mutex<RECT>,
}

pub struct ScreenSampler {
    s: Arc<Shared>,
}

/// How often the screen is looked at while it's the palette's source.
const EVERY: Duration = Duration::from_millis(1000);

impl ScreenSampler {
    pub fn start(sink: Sink) -> Self {
        let s = Arc::new(Shared { on: Mutex::new(false), cv: Condvar::new(), stop: AtomicBool::new(false), area: Mutex::new(RECT::default()) });
        let t = s.clone();
        std::thread::Builder::new()
            .name("synapse-ambient".into())
            .spawn(move || loop {
                {
                    let mut on = t.on.lock().unwrap();
                    while !*on && !t.stop.load(Ordering::Relaxed) {
                        on = t.cv.wait(on).unwrap();
                    }
                }
                if t.stop.load(Ordering::Relaxed) {
                    break;
                }
                let area = *t.area.lock().unwrap();
                if let Some(p) = sample(area) {
                    sink.send(Event::ScreenPalette(p));
                }
                std::thread::sleep(EVERY);
            })
            .expect("ambient thread");
        Self { s }
    }

    /// Sample `area` (screen pixels) while `on`; sleep otherwise.
    pub fn set(&self, on: bool, area: RECT) {
        *self.s.area.lock().unwrap() = area;
        let mut cur = self.s.on.lock().unwrap();
        if *cur != on {
            *cur = on;
            self.s.cv.notify_all();
        }
    }
}

impl Drop for ScreenSampler {
    fn drop(&mut self) {
        self.s.stop.store(true, Ordering::Relaxed);
        self.s.cv.notify_all();
    }
}

/// `area` shrunk to a 64×32 picture (Windows averages it as it shrinks).
fn sample(area: RECT) -> Option<Palette> {
    const W: i32 = 64;
    const H: i32 = 32;
    let (w, h) = (area.right - area.left, area.bottom - area.top);
    if w <= 0 || h <= 0 {
        return None;
    }
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(screen);
        let bmp = CreateCompatibleBitmap(screen, W, H);
        let old = SelectObject(mem, bmp);
        SetStretchBltMode(mem, HALFTONE);
        let ok = StretchBlt(mem, 0, 0, W, H, screen, area.left, area.top, w, h, SRCCOPY).as_bool();
        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: W, biHeight: -H, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
            ..Default::default()
        };
        let mut px = vec![0u8; (W * H * 4) as usize];
        SelectObject(mem, old);
        let rows = if ok { GetDIBits(mem, bmp, 0, H as u32, Some(px.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS) } else { 0 };
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        if rows == 0 {
            return None;
        }
        from_pixels(&px, W as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(colors: &[[u8; 3]], w: usize, h: usize) -> Vec<u8> {
        // Left half the first colour, right half the second.
        let mut v = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                let c = colors[if x < w / 2 { 0 } else { 1 }];
                v.extend_from_slice(&[c[2], c[1], c[0], 255]);
            }
        }
        v
    }

    #[test]
    fn halves_give_two_colours() {
        let p = from_pixels(&px(&[[200, 20, 20], [20, 20, 200]], 8, 4), 8).unwrap();
        assert!(p.a.r > 0.8 && p.a.b < 0.2, "{:?}", p.a);
        assert!(p.b.b > 0.8 && p.b.r < 0.2, "{:?}", p.b);
    }

    #[test]
    fn grey_has_no_colour() {
        assert!(from_pixels(&px(&[[120, 120, 120], [30, 30, 30]], 8, 4), 8).is_none());
        // One colourful half lends its colour to both sides.
        let p = from_pixels(&px(&[[128, 128, 128], [0, 180, 90]], 8, 4), 8).unwrap();
        assert_eq!(p.a, p.b);
    }

    #[test]
    fn eases() {
        let red = Palette { a: Color::rgba(1.0, 0.0, 0.0, 1.0), b: Color::rgba(1.0, 0.0, 0.0, 1.0) };
        let blue = Palette { a: Color::rgba(0.0, 0.0, 1.0, 1.0), b: Color::rgba(0.0, 0.0, 1.0, 1.0) };
        let mid = red.toward(&blue, 0.5);
        assert!((mid.a.r - 0.5).abs() < 1e-6 && (mid.a.b - 0.5).abs() < 1e-6);
        assert!(red.distance(&blue) > 1.9);
    }
}
