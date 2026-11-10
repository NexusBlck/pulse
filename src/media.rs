//! Now playing, from Windows' media controls (GSMTC): any player that shows
//! in the volume flyout (Spotify, browsers, Media Player...).
//!
//! One worker thread owns the session manager. WinRT events only nudge it
//! through a channel; it coalesces bursts, reads the current session and
//! tells the UI. Between changes it blocks on the channel (zero CPU).

use crate::app::{Event, Sink};
use crate::gfx::Image;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;
use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status, MediaPropertiesChangedEventArgs, PlaybackInfoChangedEventArgs,
    TimelinePropertiesChangedEventArgs,
};
use windows::Storage::Streams::DataReader;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

#[derive(Clone, Debug, PartialEq)]
pub struct MediaInfo {
    /// Changes with the track (app + title + artist).
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub app: String,
    pub playing: bool,
    pub can_prev: bool,
    pub can_next: bool,
    /// Seconds into the track when `updated` was taken.
    pub position: f64,
    pub duration: f64,
    /// FILETIME ticks (100 ns since 1601) of that position.
    pub updated: i64,
}

#[derive(Clone, Copy, Debug)]
pub enum MediaCmd {
    Toggle,
    Next,
    Prev,
    Seek(f64),
}

enum Msg {
    Changed,
    SessionChanged,
    Cmd(MediaCmd),
}

pub struct Media {
    tx: Sender<Msg>,
}

impl Media {
    pub fn start(sink: Sink) -> Self {
        let (tx, rx) = mpsc::channel();
        let me = tx.clone();
        std::thread::Builder::new()
            .name("synapse-media".into())
            .spawn(move || worker(rx, me, sink))
            .expect("media thread");
        Self { tx }
    }

    pub fn send(&self, c: MediaCmd) {
        let _ = self.tx.send(Msg::Cmd(c));
    }
}

struct Watched {
    session: Session,
    tokens: [EventRegistrationToken; 3],
}

impl Watched {
    fn new(session: Session, tx: &Sender<Msg>) -> Option<Self> {
        let (a, b, c) = (tx.clone(), tx.clone(), tx.clone());
        let t1 = session
            .MediaPropertiesChanged(&TypedEventHandler::<Session, MediaPropertiesChangedEventArgs>::new(move |_, _| {
                let _ = a.send(Msg::Changed);
                Ok(())
            }))
            .ok()?;
        let t2 = session
            .PlaybackInfoChanged(&TypedEventHandler::<Session, PlaybackInfoChangedEventArgs>::new(move |_, _| {
                let _ = b.send(Msg::Changed);
                Ok(())
            }))
            .ok()?;
        let t3 = session
            .TimelinePropertiesChanged(&TypedEventHandler::<Session, TimelinePropertiesChangedEventArgs>::new(move |_, _| {
                let _ = c.send(Msg::Changed);
                Ok(())
            }))
            .ok()?;
        Some(Self { session, tokens: [t1, t2, t3] })
    }
}

impl Drop for Watched {
    fn drop(&mut self) {
        let _ = self.session.RemoveMediaPropertiesChanged(self.tokens[0]);
        let _ = self.session.RemovePlaybackInfoChanged(self.tokens[1]);
        let _ = self.session.RemoveTimelinePropertiesChanged(self.tokens[2]);
    }
}

fn worker(rx: Receiver<Msg>, tx: Sender<Msg>, sink: Sink) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mgr = match Manager::RequestAsync().and_then(|op| op.get()) {
        Ok(m) => m,
        Err(e) => {
            crate::log(&format!("media: no session manager ({e})"));
            return;
        }
    };
    let t = tx.clone();
    let _token = mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Msg::SessionChanged);
        Ok(())
    }));
    let mut watched: Option<Watched> = None;
    let mut art_key = String::new();
    let mut last: Option<MediaInfo> = None;
    let mut resubscribe = true;
    loop {
        if resubscribe {
            watched = mgr.GetCurrentSession().ok().and_then(|s| Watched::new(s, &tx));
            resubscribe = false;
        }
        let session = watched.as_ref().map(|w| w.session.clone());
        let info = session.as_ref().and_then(read);
        if info != last {
            // The track first: the app keeps art only for the track it knows.
            sink.send(Event::Media(info.clone()));
            if let (Some(i), Some(s)) = (&info, &session) {
                if i.key != art_key {
                    art_key = i.key.clone();
                    let (img, accent) = match art(s) {
                        Some((img, accent)) => (Some(img), accent),
                        None => (None, None),
                    };
                    sink.send(Event::Art { key: i.key.clone(), img, accent });
                }
            }
            last = info;
        }
        // Block until something happens, then gather the burst.
        let Ok(first) = rx.recv() else { return };
        let mut msgs = vec![first];
        std::thread::sleep(Duration::from_millis(60));
        msgs.extend(rx.try_iter());
        for m in msgs {
            match m {
                Msg::Changed => {}
                Msg::SessionChanged => resubscribe = true,
                Msg::Cmd(c) => {
                    if let Some(s) = &session {
                        run(s, c);
                    }
                }
            }
        }
    }
}

fn run(s: &Session, c: MediaCmd) {
    let r = match c {
        MediaCmd::Toggle => s.TryTogglePlayPauseAsync().and_then(|o| o.get()),
        MediaCmd::Next => s.TrySkipNextAsync().and_then(|o| o.get()),
        MediaCmd::Prev => s.TrySkipPreviousAsync().and_then(|o| o.get()),
        MediaCmd::Seek(secs) => s.TryChangePlaybackPositionAsync((secs * 1e7) as i64).and_then(|o| o.get()),
    };
    if let Err(e) = r {
        crate::log(&format!("media: {c:?} failed ({e})"));
    }
}

fn read(s: &Session) -> Option<MediaInfo> {
    let props = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let title = props.Title().map(|h| h.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|h| h.to_string()).unwrap_or_default();
    let album = props.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
    if title.is_empty() && artist.is_empty() {
        return None;
    }
    let app = pretty_app(&s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());
    let (mut playing, mut can_prev, mut can_next) = (false, false, false);
    if let Ok(pb) = s.GetPlaybackInfo() {
        playing = pb.PlaybackStatus().is_ok_and(|st| st == Status::Playing);
        if let Ok(c) = pb.Controls() {
            can_prev = c.IsPreviousEnabled().unwrap_or(false);
            can_next = c.IsNextEnabled().unwrap_or(false);
        }
    }
    let (mut position, mut duration, mut updated) = (0.0, 0.0, 0);
    if let Ok(tl) = s.GetTimelineProperties() {
        position = tl.Position().map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0);
        let start = tl.StartTime().map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0);
        duration = tl.EndTime().map(|t| t.Duration as f64 / 1e7).unwrap_or(0.0) - start;
        updated = tl.LastUpdatedTime().map(|d| d.UniversalTime).unwrap_or(0);
    }
    Some(MediaInfo { key: format!("{app}\u{1}{title}\u{1}{artist}"), title, artist, album, app, playing, can_prev, can_next, position, duration, updated })
}

/// "Spotify.exe" -> "Spotify", "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic" -> "ZuneMusic".
fn pretty_app(id: &str) -> String {
    let tail = id.rsplit('!').next().unwrap_or(id);
    let tail = tail.trim_end_matches(".exe").trim_end_matches(".EXE");
    let name = tail.rsplit(['.', '\\']).next().unwrap_or(tail);
    match name.to_ascii_lowercase().as_str() {
        "msedge" => "Edge".into(),
        "chrome" => "Chrome".into(),
        "firefox" | "308046b0af4a39cb" => "Firefox".into(),
        "zunemusic" => "Media Player".into(),
        _ => name.to_string(),
    }
}

/// The track's thumbnail, decoded small, and a colour taken from it.
fn art(s: &Session) -> Option<(Image, Option<[f32; 3]>)> {
    match read_art(s) {
        Ok(img) => {
            let accent = accent_of(&img);
            Some((img, accent))
        }
        Err(why) => {
            crate::log(&format!("media: no album art ({why})"));
            None
        }
    }
}

fn read_art(s: &Session) -> Result<Image, String> {
    let props = s.TryGetMediaPropertiesAsync().and_then(|o| o.get()).map_err(|e| format!("properties: {e}"))?;
    let thumb = props.Thumbnail().map_err(|e| format!("thumbnail: {e}"))?;
    let stream = thumb.OpenReadAsync().and_then(|o| o.get()).map_err(|e| format!("open: {e}"))?;
    let size = stream.Size().map_err(|e| format!("size: {e}"))? as u32;
    if size == 0 || size > 16 * 1024 * 1024 {
        return Err(format!("size {size}"));
    }
    let reader = DataReader::CreateDataReader(&stream).map_err(|e| format!("reader: {e}"))?;
    reader.LoadAsync(size).and_then(|o| o.get()).map_err(|e| format!("load: {e}"))?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).map_err(|e| format!("read: {e}"))?;
    decode(&bytes, 160).ok_or_else(|| format!("decode of {size} bytes failed"))
}

fn decode(bytes: &[u8], max: u32) -> Option<Image> {
    unsafe {
        let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = wic.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let decoder = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let k = (max as f32 / w.max(h) as f32).min(1.0);
        let (tw, th) = (((w as f32 * k) as u32).max(1), ((h as f32 * k) as u32).max(1));
        let scaler = wic.CreateBitmapScaler().ok()?;
        scaler.Initialize(&frame, tw, th, WICBitmapInterpolationModeFant).ok()?;
        let conv = wic.CreateFormatConverter().ok()?;
        conv.Initialize(&scaler, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let mut px = vec![0u8; (tw * th * 4) as usize];
        conv.CopyPixels(std::ptr::null(), tw * 4, &mut px).ok()?;
        Some(Image { w: tw, h: th, px })
    }
}

/// The most colourful average of the art: skips greys, then lifts it so it
/// reads on black.
fn accent_of(img: &Image) -> Option<[f32; 3]> {
    let (mut r, mut g, mut b, mut n) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for p in img.px.chunks_exact(4).step_by(3) {
        let (pb, pg, pr) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
        let (hi, lo) = (pr.max(pg).max(pb), pr.min(pg).min(pb));
        let sat = if hi > 0.0 { (hi - lo) / hi } else { 0.0 };
        let wgt = sat * sat * hi;
        if wgt > 0.02 {
            r += pr * wgt;
            g += pg * wgt;
            b += pb * wgt;
            n += wgt;
        }
    }
    if n < 1.0 {
        return None;
    }
    let (r, g, b) = (r / n, g / n, b / n);
    let hi = r.max(g).max(b).max(0.01);
    let lift = (0.85 / hi).max(1.0);
    Some([(r * lift).min(1.0), (g * lift).min(1.0), (b * lift).min(1.0)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_names() {
        assert_eq!(pretty_app("Spotify.exe"), "Spotify");
        assert_eq!(pretty_app("MSEdge"), "Edge");
        assert_eq!(pretty_app("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Media Player");
    }
}
