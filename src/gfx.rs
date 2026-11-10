//! Drawing. Direct2D renders into a 32-bit DIB, which `UpdateLayeredWindow`
//! puts on screen with per-pixel alpha: fully transparent pixels let clicks
//! through to the windows below, so only the island takes the mouse.
//! All coordinates are DIPs; the render target's DPI turns them into pixels.

use std::collections::HashMap;
use std::mem::ManuallyDrop;
use windows::core::{w, Interface, Result, HSTRING};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, COLORREF, HWND, POINT, RECT, SIZE};
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::{UpdateLayeredWindow, ULW_ALPHA};

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + self.h / 2.0
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, (self.w - 2.0 * dx).max(0.0), (self.h - 2.0 * dy).max(0.0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
    pub const fn white(a: f32) -> Self {
        Self::rgba(1.0, 1.0, 1.0, a)
    }
    pub const fn black(a: f32) -> Self {
        Self::rgba(0.0, 0.0, 0.0, a)
    }
    pub fn fade(self, a: f32) -> Self {
        Self { a: self.a * a, ..self }
    }
    /// "#RRGGBB" or "#RRGGBBAA".
    pub fn parse(s: &str) -> Option<Color> {
        let h = s.trim().trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok().map(|v| v as f32 / 255.0);
        match h.len() {
            6 => Some(Color::rgba(byte(0)?, byte(2)?, byte(4)?, 1.0)),
            8 => Some(Color::rgba(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
            _ => None,
        }
    }
}

/// Premultiplied BGRA pixels.
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

struct Dib {
    hdc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
}

impl Drop for Dib {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.hdc, self.old);
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.hdc);
        }
    }
}

pub struct Gfx {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    rt: ID2D1DCRenderTarget,
    brush: ID2D1SolidColorBrush,
    /// Round line ends (progress rings).
    round: ID2D1StrokeStyle,
    formats: HashMap<(u32, u32, Align, bool), IDWriteTextFormat>,
    wrap_formats: HashMap<(u32, u32, Align, bool), IDWriteTextFormat>,
    font: String,
    icon_font: String,
    dib: Option<Dib>,
    pub scale: f32,
    /// Every colour drawn is multiplied by this (content fading in or out).
    pub alpha: f32,
    /// The render target was recreated: bitmaps made on the old one are gone.
    pub lost: bool,
}

fn rectf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F { left: r.x, top: r.y, right: r.right(), bottom: r.bottom() }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

pub fn identity() -> Matrix3x2 {
    Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 }
}

/// Scale by `s` about (`px`, `py`), then move by (`dx`, `dy`).
pub fn scale_about(px: f32, py: f32, s: f32, dx: f32, dy: f32) -> Matrix3x2 {
    Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: px - s * px + dx, M32: py - s * py + dy }
}

impl Gfx {
    pub fn new(font: &str) -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let rt = Self::make_target(&d2d)?;
            let brush = rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
            let icon_font = if Self::has_family(&dw, "Segoe Fluent Icons") { "Segoe Fluent Icons" } else { "Segoe MDL2 Assets" };
            let round = d2d.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;
            let mut g = Self {
                d2d,
                dw,
                rt,
                brush,
                round,
                formats: HashMap::new(),
                wrap_formats: HashMap::new(),
                font: String::new(),
                icon_font: icon_font.into(),
                dib: None,
                scale: 1.0,
                alpha: 1.0,
                lost: false,
            };
            g.set_font(font);
            Ok(g)
        }
    }

    fn make_target(d2d: &ID2D1Factory) -> Result<ID2D1DCRenderTarget> {
        let props = D2D1_RENDER_TARGET_PROPERTIES {
            r#type: D2D1_RENDER_TARGET_TYPE_DEFAULT,
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
            usage: D2D1_RENDER_TARGET_USAGE_NONE,
            minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
        };
        unsafe {
            let rt = d2d.CreateDCRenderTarget(&props)?;
            // ClearType needs an opaque background; the island sits on transparency.
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            Ok(rt)
        }
    }

    fn has_family(dw: &IDWriteFactory, name: &str) -> bool {
        unsafe {
            let mut coll = None;
            if dw.GetSystemFontCollection(&mut coll, false).is_err() {
                return false;
            }
            let Some(coll) = coll else { return false };
            let (mut index, mut exists) = (0u32, BOOL(0));
            coll.FindFamilyName(&HSTRING::from(name), &mut index, &mut exists).is_ok() && exists.as_bool()
        }
    }

    pub fn set_font(&mut self, font: &str) {
        let font = if Self::has_family(&self.dw, font) { font.to_string() } else { "Segoe UI".to_string() };
        if font != self.font {
            self.font = font;
            self.formats.clear();
            self.wrap_formats.clear();
        }
    }

    /// Size the backing bitmap to `w`×`h` pixels at `scale`.
    pub fn resize(&mut self, w: i32, h: i32, scale: f32) -> Result<()> {
        self.scale = scale;
        unsafe { self.rt.SetDpi(96.0 * scale, 96.0 * scale) };
        if self.dib.as_ref().is_some_and(|d| d.w == w && d.h == h) {
            return Ok(());
        }
        self.dib = None;
        unsafe {
            let hdc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(hdc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old = SelectObject(hdc, bmp);
            self.dib = Some(Dib { hdc, bmp, old, w, h });
        }
        Ok(())
    }

    pub fn begin(&mut self) -> Result<()> {
        let Some(d) = &self.dib else { return Ok(()) };
        unsafe {
            self.rt.BindDC(d.hdc, &RECT { left: 0, top: 0, right: d.w, bottom: d.h })?;
            self.rt.BeginDraw();
            self.rt.SetTransform(&identity());
            self.rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        }
        self.alpha = 1.0;
        Ok(())
    }

    pub fn end(&mut self) {
        if let Err(e) = unsafe { self.rt.EndDraw(None, None) } {
            crate::log(&format!("EndDraw failed ({e}); recreating the render target"));
            if let Ok(rt) = Self::make_target(&self.d2d) {
                unsafe { rt.SetDpi(96.0 * self.scale, 96.0 * self.scale) };
                if let Ok(b) = unsafe { rt.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None) } {
                    self.rt = rt;
                    self.brush = b;
                    self.lost = true;
                }
            }
        }
    }

    /// Put the frame on screen with its top-left at (`x`, `y`) screen pixels.
    pub fn present(&self, hwnd: HWND, x: i32, y: i32) {
        let Some(d) = &self.dib else { return };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe {
            let _ = UpdateLayeredWindow(
                hwnd,
                None,
                Some(&POINT { x, y }),
                Some(&SIZE { cx: d.w, cy: d.h }),
                d.hdc,
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    /// Put the frame on screen wherever the window is now (it may have been dragged).
    pub fn present_in_place(&self, hwnd: HWND) {
        let Some(d) = &self.dib else { return };
        let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
        unsafe {
            let _ = UpdateLayeredWindow(hwnd, None, None, Some(&SIZE { cx: d.w, cy: d.h }), d.hdc, Some(&POINT { x: 0, y: 0 }), COLORREF(0), Some(&blend), ULW_ALPHA);
        }
    }

    pub fn set_transform(&mut self, m: &Matrix3x2) {
        unsafe { self.rt.SetTransform(m) };
    }

    fn color(&self, c: Color) {
        unsafe { self.brush.SetColor(&D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a * self.alpha }) };
    }

    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        self.color(c);
        unsafe { self.rt.FillRectangle(&rectf(r), &self.brush) };
    }

    pub fn fill_rrect(&mut self, r: Rect, radius: f32, c: Color) {
        self.color(c);
        let rad = radius.min(r.h / 2.0).min(r.w / 2.0).max(0.0);
        unsafe { self.rt.FillRoundedRectangle(&D2D1_ROUNDED_RECT { rect: rectf(r), radiusX: rad, radiusY: rad }, &self.brush) };
    }

    pub fn fill_circle(&mut self, x: f32, y: f32, radius: f32, c: Color) {
        self.color(c);
        unsafe { self.rt.FillEllipse(&D2D1_ELLIPSE { point: pt(x, y), radiusX: radius, radiusY: radius }, &self.brush) };
    }

    pub fn fill_geo(&mut self, g: &ID2D1Geometry, c: Color) {
        self.color(c);
        unsafe { self.rt.FillGeometry(g, &self.brush, None) };
    }

    pub fn stroke_geo(&mut self, g: &ID2D1Geometry, c: Color, width: f32) {
        self.color(c);
        unsafe { self.rt.DrawGeometry(g, &self.brush, width, None) };
    }

    fn gradient(&self, from: (f32, f32), to: (f32, f32), c0: Color, c1: Color) -> Option<ID2D1LinearGradientBrush> {
        let col = |c: Color| D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a * self.alpha };
        let stops = [D2D1_GRADIENT_STOP { position: 0.0, color: col(c0) }, D2D1_GRADIENT_STOP { position: 1.0, color: col(c1) }];
        unsafe {
            let coll = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(from.0, from.1), endPoint: pt(to.0, to.1) };
            self.rt.CreateLinearGradientBrush(&props, None, &coll).ok()
        }
    }

    /// Fill `r` blending from `c0` at `from` to `c1` at `to`.
    pub fn fill_rect_gradient(&mut self, r: Rect, from: (f32, f32), to: (f32, f32), c0: Color, c1: Color) {
        if let Some(b) = self.gradient(from, to, c0, c1) {
            unsafe { self.rt.FillRectangle(&rectf(r), &b) };
        }
    }

    /// Fill `r` with light that fades from `c0` at `centre` to `c1` at the ellipse's edge.
    pub fn fill_rect_radial(&mut self, r: Rect, centre: (f32, f32), rx: f32, ry: f32, c0: Color, c1: Color) {
        let col = |c: Color| D2D1_COLOR_F { r: c.r, g: c.g, b: c.b, a: c.a * self.alpha };
        let stops = [D2D1_GRADIENT_STOP { position: 0.0, color: col(c0) }, D2D1_GRADIENT_STOP { position: 1.0, color: col(c1) }];
        unsafe {
            let Ok(coll) = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
            let props = D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES { center: pt(centre.0, centre.1), gradientOriginOffset: pt(0.0, 0.0), radiusX: rx, radiusY: ry };
            if let Ok(b) = self.rt.CreateRadialGradientBrush(&props, None, &coll) {
                self.rt.FillRectangle(&rectf(r), &b);
            }
        }
    }

    /// Fill `g` blending from `c0` at `from` to `c1` at `to`.
    pub fn fill_geo_gradient(&mut self, g: &ID2D1Geometry, from: (f32, f32), to: (f32, f32), c0: Color, c1: Color) {
        if let Some(b) = self.gradient(from, to, c0, c1) {
            unsafe { self.rt.FillGeometry(g, &b, None) };
        }
    }

    pub fn stroke_geo_gradient(&mut self, g: &ID2D1Geometry, from: (f32, f32), to: (f32, f32), c0: Color, c1: Color, width: f32) {
        if let Some(b) = self.gradient(from, to, c0, c1) {
            unsafe { self.rt.DrawGeometry(g, &b, width, None) };
        }
    }

    fn path(&self, pts: &[(f32, f32)], close_to: Option<f32>) -> Option<ID2D1Geometry> {
        let (first, rest) = pts.split_first()?;
        unsafe {
            let geo = self.d2d.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            sink.BeginFigure(pt(first.0, first.1), if close_to.is_some() { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW });
            for p in rest {
                sink.AddLine(pt(p.0, p.1));
            }
            if let Some(base) = close_to {
                sink.AddLine(pt(pts[pts.len() - 1].0, base));
                sink.AddLine(pt(first.0, base));
            }
            sink.EndFigure(if close_to.is_some() { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            sink.Close().ok()?;
            geo.cast().ok()
        }
    }

    /// A line through `pts` (charts), with round joins and ends.
    pub fn polyline(&mut self, pts: &[(f32, f32)], c: Color, width: f32) {
        if let Some(g) = self.path(pts, None) {
            self.color(c);
            unsafe { self.rt.DrawGeometry(&g, &self.brush, width, &self.round) };
        }
    }

    /// The area between `pts` and `base`, fading from `top` down to `bottom`.
    pub fn area_under(&mut self, pts: &[(f32, f32)], base: f32, top: Color, bottom: Color) {
        let min_y = pts.iter().map(|p| p.1).fold(base, f32::min);
        if let Some(g) = self.path(pts, Some(base)) {
            self.fill_geo_gradient(&g, (0.0, min_y), (0.0, base), top, bottom);
        }
    }

    pub fn ring(&mut self, x: f32, y: f32, radius: f32, width: f32, c: Color) {
        self.color(c);
        unsafe { self.rt.DrawEllipse(&D2D1_ELLIPSE { point: pt(x, y), radiusX: radius, radiusY: radius }, &self.brush, width, None) };
    }

    /// Part of a ring, clockwise from 12 o'clock, `frac` of the way round, round ends.
    pub fn arc(&mut self, x: f32, y: f32, radius: f32, frac: f32, width: f32, c: Color) {
        let frac = frac.clamp(0.0, 1.0);
        if frac <= 0.001 {
            return;
        }
        if frac >= 0.999 {
            self.ring(x, y, radius, width, c);
            return;
        }
        let a = frac * std::f32::consts::TAU;
        let end = pt(x + radius * a.sin(), y - radius * a.cos());
        unsafe {
            let Ok(geo) = self.d2d.CreatePathGeometry() else { return };
            let Ok(sink) = geo.Open() else { return };
            sink.BeginFigure(pt(x, y - radius), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: end,
                size: D2D_SIZE_F { width: radius, height: radius },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            if sink.Close().is_err() {
                return;
            }
            self.color(c);
            self.rt.DrawGeometry(&geo, &self.brush, width, &self.round);
        }
    }

    /// The hanging island: flat top on the screen edge, concave shoulders
    /// flaring into it, rounded bottom corners. With `closed` false only the
    /// sides and bottom are traced (for the rim, so no seam shows at the edge).
    pub fn notch(&self, body: Rect, r: f32, shoulder: f32, closed: bool) -> Option<ID2D1Geometry> {
        let s = shoulder.min(body.h).max(0.0);
        let r = r.min(body.h).min(body.w / 2.0).max(0.0);
        let (x0, x1, y0, y1) = (body.x, body.right(), body.y, body.bottom());
        let arc = |x: f32, y: f32, rad: f32, cw: bool| D2D1_ARC_SEGMENT {
            point: pt(x, y),
            size: D2D_SIZE_F { width: rad, height: rad },
            rotationAngle: 0.0,
            sweepDirection: if cw { D2D1_SWEEP_DIRECTION_CLOCKWISE } else { D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE },
            arcSize: D2D1_ARC_SIZE_SMALL,
        };
        unsafe {
            let geo = self.d2d.CreatePathGeometry().ok()?;
            let sink = geo.Open().ok()?;
            if closed {
                sink.BeginFigure(pt(x0 - s, y0), D2D1_FIGURE_BEGIN_FILLED);
                if s > 0.0 {
                    sink.AddArc(&arc(x0, y0 + s, s, true));
                }
            } else {
                sink.BeginFigure(pt(x0, y0 + s), D2D1_FIGURE_BEGIN_HOLLOW);
            }
            sink.AddLine(pt(x0, (y1 - r).max(y0 + s)));
            sink.AddArc(&arc(x0 + r, y1, r, false));
            sink.AddLine(pt(x1 - r, y1));
            sink.AddArc(&arc(x1, (y1 - r).max(y0 + s), r, false));
            sink.AddLine(pt(x1, y0 + s));
            if closed && s > 0.0 {
                sink.AddArc(&arc(x1 + s, y0, s, true));
            }
            sink.EndFigure(if closed { D2D1_FIGURE_END_CLOSED } else { D2D1_FIGURE_END_OPEN });
            sink.Close().ok()?;
            geo.cast().ok()
        }
    }

    pub fn rrect_geo(&self, r: Rect, radius: f32) -> Option<ID2D1Geometry> {
        let rad = radius.min(r.h / 2.0).min(r.w / 2.0).max(0.0);
        unsafe { self.d2d.CreateRoundedRectangleGeometry(&D2D1_ROUNDED_RECT { rect: rectf(r), radiusX: rad, radiusY: rad }).ok()?.cast().ok() }
    }

    /// Clip what follows to `g` (anti-aliased), until [`Gfx::pop`].
    pub fn push_mask(&mut self, g: &ID2D1Geometry) {
        let params = D2D1_LAYER_PARAMETERS {
            contentBounds: D2D_RECT_F { left: -1e6, top: -1e6, right: 1e6, bottom: 1e6 },
            geometricMask: ManuallyDrop::new(Some(g.clone())),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: identity(),
            opacity: 1.0,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS_NONE,
        };
        unsafe { self.rt.PushLayer(&params, None) };
        let _ = ManuallyDrop::into_inner(params.geometricMask);
    }

    pub fn pop(&mut self) {
        unsafe { self.rt.PopLayer() };
    }

    fn format(&mut self, size: f32, weight: u32, align: Align, icon: bool) -> Option<IDWriteTextFormat> {
        let key = ((size * 10.0) as u32, weight, align, icon);
        if let Some(f) = self.formats.get(&key) {
            return Some(f.clone());
        }
        let family = HSTRING::from(if icon { self.icon_font.as_str() } else { self.font.as_str() });
        unsafe {
            let f = self
                .dw
                .CreateTextFormat(&family, None, DWRITE_FONT_WEIGHT(weight as i32), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))
                .ok()?;
            let _ = f.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&f) {
                let trim = DWRITE_TRIMMING { granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER, delimiter: 0, delimiterCount: 0 };
                let _ = f.SetTrimming(&trim, &sign);
            }
            self.formats.insert(key, f.clone());
            Some(f)
        }
    }

    /// One line of text in `r`, vertically centred, ellipsized if too long.
    pub fn text(&mut self, s: &str, r: Rect, size: f32, weight: u32, c: Color, align: Align) {
        self.draw_text(s, r, size, weight, c, align, false);
    }

    /// A glyph from Segoe Fluent Icons (or MDL2 Assets on Windows 10).
    pub fn icon(&mut self, glyph: char, r: Rect, size: f32, c: Color) {
        self.draw_text(&glyph.to_string(), r, size, 400, c, Align::Center, true);
    }

    fn draw_text(&mut self, s: &str, r: Rect, size: f32, weight: u32, c: Color, align: Align, icon: bool) {
        let Some(f) = self.format(size, weight, align, icon) else { return };
        let w: Vec<u16> = s.encode_utf16().collect();
        self.color(c);
        unsafe { self.rt.DrawText(&w, &f, &rectf(r), &self.brush, D2D1_DRAW_TEXT_OPTIONS_CLIP, DWRITE_MEASURING_MODE_NATURAL) };
    }

    /// A format that wraps at word boundaries, top-aligned, for paragraphs.
    fn wrap_format(&mut self, size: f32, weight: u32) -> Option<IDWriteTextFormat> {
        let key = ((size * 10.0) as u32, weight, Align::Left, true);
        if let Some(f) = self.wrap_formats.get(&key) {
            return Some(f.clone());
        }
        unsafe {
            let f = self
                .dw
                .CreateTextFormat(&HSTRING::from(self.font.as_str()), None, DWRITE_FONT_WEIGHT(weight as i32), DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_STRETCH_NORMAL, size, w!("en-us"))
                .ok()?;
            let _ = f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_LEADING);
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP);
            // Even rows, so a wrapped line reads as one block.
            let _ = f.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_UNIFORM, size * 1.22, size * 0.95);
            self.wrap_formats.insert(key, f.clone());
            Some(f)
        }
    }

    /// Text wrapped to `r`'s width, from its top.
    pub fn text_wrapped(&mut self, s: &str, r: Rect, size: f32, weight: u32, c: Color) {
        let Some(f) = self.wrap_format(size, weight) else { return };
        let w: Vec<u16> = s.encode_utf16().collect();
        self.color(c);
        unsafe { self.rt.DrawText(&w, &f, &rectf(r), &self.brush, D2D1_DRAW_TEXT_OPTIONS_NONE, DWRITE_MEASURING_MODE_NATURAL) };
    }

    /// How tall `s` is wrapped to `width`.
    pub fn measure_wrapped(&mut self, s: &str, size: f32, weight: u32, width: f32) -> f32 {
        let Some(f) = self.wrap_format(size, weight) else { return size * 1.22 };
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&w, &f, width.max(1.0), 10_000.0) else { return size * 1.22 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                m.height.max(size * 1.22)
            } else {
                size * 1.22
            }
        }
    }

    pub fn measure(&mut self, s: &str, size: f32, weight: u32) -> f32 {
        let Some(f) = self.format(size, weight, Align::Left, false) else { return 0.0 };
        let w: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(layout) = self.dw.CreateTextLayout(&w, &f, 10_000.0, 100.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if layout.GetMetrics(&mut m).is_ok() {
                m.widthIncludingTrailingWhitespace
            } else {
                0.0
            }
        }
    }

    pub fn bitmap(&self, img: &Image) -> Option<ID2D1Bitmap> {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        unsafe { self.rt.CreateBitmap(D2D_SIZE_U { width: img.w, height: img.h }, Some(img.px.as_ptr() as *const _), img.w * 4, &props).ok() }
    }

    /// `bmp` filling `r`, its corners rounded by `radius`.
    pub fn image(&mut self, bmp: &ID2D1Bitmap, r: Rect, radius: f32) {
        let mask = self.rrect_geo(r, radius);
        if let Some(m) = &mask {
            self.push_mask(m);
        }
        unsafe { self.rt.DrawBitmap(bmp, Some(&rectf(r)), self.alpha, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None) };
        if mask.is_some() {
            self.pop();
        }
    }
}
