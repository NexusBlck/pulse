//! The Shelf: files you drop on the island wait there until you drag them
//! out, open them or remove them. It keeps paths, never copies; the list
//! survives restarts in `%APPDATA%\Synapse\shelf.json`.

use crate::app::{Event, Sink};
use crate::gfx::Image;
use std::sync::mpsc::{self, Sender};
use windows::core::{implement, Interface, Result, HRESULT, HSTRING};
use windows::Win32::Foundation::{BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, HWND, POINTL, S_OK, SIZE};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoInitializeEx, CoTaskMemFree, IDataObject, COINIT_APARTMENTTHREADED, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, CF_HDROP, DROPEFFECT,
    DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    DragQueryFileW, IShellItem, IShellItemArray, IShellItemImageFactory, SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, SHParseDisplayName,
    BHID_DataObject, HDROP, SIIGBF_RESIZETOFIT,
};

/// The most files it holds (three rows with the Phone and Clear tiles); the oldest go first.
pub const MAX: usize = 16;

pub struct Shelf {
    pub items: Vec<String>,
}

fn file() -> std::path::PathBuf {
    crate::config::dir().join("shelf.json")
}

impl Shelf {
    pub fn load() -> Self {
        let items: Vec<String> = std::fs::read_to_string(file()).ok().and_then(|t| serde_json::from_str(t.trim_start_matches('\u{feff}')).ok()).unwrap_or_default();
        let mut s = Self { items };
        s.prune();
        s
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(crate::config::dir());
        if let Ok(t) = serde_json::to_string_pretty(&self.items) {
            let _ = std::fs::write(file(), t);
        }
    }

    /// Add files (newest last), skipping ones already there. Returns how many were added.
    pub fn add(&mut self, paths: Vec<String>) -> usize {
        let mut n = 0;
        for p in paths {
            if !self.items.iter().any(|q| q.eq_ignore_ascii_case(&p)) {
                self.items.push(p);
                n += 1;
            }
        }
        let over = self.items.len().saturating_sub(MAX);
        self.items.drain(..over);
        n
    }

    /// Forget files that were moved or deleted.
    pub fn prune(&mut self) -> bool {
        let before = self.items.len();
        self.items.retain(|p| std::path::Path::new(p).exists());
        self.items.len() != before
    }
}

pub fn name(path: &str) -> &str {
    path.trim_end_matches(['\\', '/']).rsplit(['\\', '/']).next().unwrap_or(path)
}

// ------------------------------------------------------------ dropping in

fn hdrop_format() -> FORMATETC {
    FORMATETC { cfFormat: CF_HDROP.0, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: -1, tymed: TYMED_HGLOBAL.0 as u32 }
}

fn has_files(data: Option<&IDataObject>) -> bool {
    data.is_some_and(|d| unsafe { d.QueryGetData(&hdrop_format()) } == S_OK)
}

fn files(data: &IDataObject) -> Vec<String> {
    let mut out = Vec::new();
    unsafe {
        let Ok(mut medium) = data.GetData(&hdrop_format()) else { return out };
        let drop = HDROP(medium.u.hGlobal.0);
        let n = DragQueryFileW(drop, u32::MAX, None);
        for i in 0..n {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            DragQueryFileW(drop, i, Some(&mut buf));
            out.push(String::from_utf16_lossy(&buf[..len]));
        }
        ReleaseStgMedium(&mut medium);
    }
    out
}

/// What a drag over the island did, for the app.
pub enum Drag {
    Enter,
    Leave,
    Drop(Vec<String>),
}

#[implement(IDropTarget)]
struct Target {
    on: fn(Drag),
}

impl IDropTarget_Impl for Target_Impl {
    fn DragEnter(&self, data: Option<&IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let ok = has_files(data);
        unsafe { *effect = if ok { DROPEFFECT_COPY } else { DROPEFFECT_NONE } };
        if ok {
            (self.on)(Drag::Enter);
        }
        Ok(())
    }

    fn DragOver(&self, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        unsafe {
            if *effect != DROPEFFECT_NONE {
                *effect = DROPEFFECT_COPY;
            }
        }
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        (self.on)(Drag::Leave);
        Ok(())
    }

    fn Drop(&self, data: Option<&IDataObject>, _keys: MODIFIERKEYS_FLAGS, _pt: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
        let got = data.map(files).unwrap_or_default();
        unsafe { *effect = if got.is_empty() { DROPEFFECT_NONE } else { DROPEFFECT_COPY } };
        (self.on)(Drag::Drop(got));
        Ok(())
    }
}

/// Make the island a drop target; `on` hears about drags (on the UI thread).
pub fn accept_drops(hwnd: HWND, on: fn(Drag)) -> Result<()> {
    let t: IDropTarget = Target { on }.into();
    unsafe { RegisterDragDrop(hwnd, &t) }
}

pub fn stop_drops(hwnd: HWND) {
    unsafe {
        let _ = RevokeDragDrop(hwnd);
    }
}

// ------------------------------------------------------------ dragging out

#[implement(IDropSource)]
struct Source;

impl IDropSource_Impl for Source_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() {
            DRAGDROP_S_CANCEL
        } else if keys.0 & MK_LBUTTON.0 == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Drag `paths` out of the island, as Explorer would. Blocks until dropped
/// (it runs its own message loop). Returns what the drop did.
pub fn drag_out(paths: &[String]) -> Option<DROPEFFECT> {
    unsafe {
        let mut pidls: Vec<*mut ITEMIDLIST> = Vec::new();
        for p in paths {
            let mut pidl = std::ptr::null_mut();
            if SHParseDisplayName(&HSTRING::from(p.as_str()), None, &mut pidl, 0, None).is_ok() {
                pidls.push(pidl);
            }
        }
        let consts: Vec<*const ITEMIDLIST> = pidls.iter().map(|p| *p as *const _).collect();
        let data = SHCreateShellItemArrayFromIDLists(&consts).and_then(|a: IShellItemArray| a.BindToHandler::<_, IDataObject>(None, &BHID_DataObject));
        for p in pidls {
            CoTaskMemFree(Some(p as *const _));
        }
        let data = match data {
            Ok(d) => d,
            Err(e) => {
                crate::log(&format!("shelf: can't drag those files ({e})"));
                return None;
            }
        };
        let src: IDropSource = Source.into();
        let mut effect = DROPEFFECT_NONE;
        let _ = DoDragDrop(&data, &src, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK, &mut effect);
        Some(effect)
    }
}

// ------------------------------------------------------------ thumbnails

/// Thumbnails (pictures) or icons (everything else), made off the UI thread.
pub struct Thumbs {
    tx: Sender<String>,
}

impl Thumbs {
    pub fn start(sink: Sink) -> Self {
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::Builder::new()
            .name("synapse-thumbs".into())
            .spawn(move || {
                unsafe {
                    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                }
                while let Ok(path) = rx.recv() {
                    let img = thumbnail(&path, 96);
                    sink.send(Event::Thumb { path, img });
                }
            })
            .expect("thumbnail thread");
        Self { tx }
    }

    pub fn request(&self, path: &str) {
        let _ = self.tx.send(path.to_string());
    }
}

fn thumbnail(path: &str, size: i32) -> Option<Image> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None).ok()?;
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let bmp = factory.GetImage(SIZE { cx: size, cy: size }, SIIGBF_RESIZETOFIT).ok()?;
        let img = pixels(bmp);
        let _ = DeleteObject(bmp);
        img
    }
}

/// An HBITMAP's pixels as premultiplied BGRA, top row first.
unsafe fn pixels(bmp: HBITMAP) -> Option<Image> {
    let mut info = BITMAP::default();
    if GetObjectW(bmp, std::mem::size_of::<BITMAP>() as i32, Some(&mut info as *mut _ as *mut _)) == 0 {
        return None;
    }
    let (w, h) = (info.bmWidth, info.bmHeight.abs());
    if w <= 0 || h <= 0 {
        return None;
    }
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -h, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() },
        ..Default::default()
    };
    let mut px = vec![0u8; (w * h * 4) as usize];
    let dc = GetDC(None);
    let rows = GetDIBits(dc, bmp, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
    ReleaseDC(None, dc);
    if rows == 0 {
        return None;
    }
    let chunks = px.chunks_exact(4);
    if chunks.clone().all(|p| p[3] == 0) {
        // No alpha at all (most photos): opaque.
        px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
    } else if chunks.clone().any(|p| p[0] > p[3] || p[1] > p[3] || p[2] > p[3]) {
        // Straight alpha: premultiply for Direct2D.
        for p in px.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in &mut p[..3] {
                *c = (*c as u32 * a / 255) as u8;
            }
        }
    }
    Some(Image { w: w as u32, h: h as u32, px })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(name("C:\\Users\\me\\photo.png"), "photo.png");
        assert_eq!(name("D:\\Projects\\"), "Projects");
    }

    #[test]
    fn adds_without_duplicates_and_caps() {
        let mut s = Shelf { items: vec![] };
        assert_eq!(s.add(vec!["C:\\a".into(), "C:\\A".into(), "C:\\b".into()]), 2);
        for i in 0..30 {
            s.add(vec![format!("C:\\f{i}")]);
        }
        assert_eq!(s.items.len(), MAX);
        assert_eq!(s.items.last().unwrap(), "C:\\f29");
    }
}
