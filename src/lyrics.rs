//! Synced lyrics from LRCLIB (lrclib.net, free and open), cached on disk
//! so a song is only looked up once.
//!
//! One worker thread blocks on a channel (zero CPU) until a new track asks
//! for its lyrics; it answers with `Event::Lyrics`.

use crate::app::{Event, Sink};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use windows::core::{RuntimeType, HSTRING};
use windows::Foundation::{AsyncStatus, IAsyncOperationWithProgress, Uri};
use windows::Web::Http::{HttpClient, HttpStatusCode};
use windows::Win32::Foundation::E_ABORT;
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

const API: &str = "https://lrclib.net/api";
/// LRCLIB asks apps to say who they are.
const USER_AGENT: &str = concat!("Synapse/", env!("CARGO_PKG_VERSION"), " (desktop island for Windows)");
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Seconds into the track.
    pub t: f64,
    /// Empty for an instrumental break.
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lyrics {
    pub lines: Vec<Line>,
}

impl Lyrics {
    /// Parse LRC: `[mm:ss.xx] words`, several stamps per line allowed,
    /// tags such as `[ar: ...]` ignored. None when nothing is timed.
    pub fn parse(lrc: &str) -> Option<Lyrics> {
        let mut lines = Vec::new();
        for raw in lrc.lines() {
            let mut rest = raw.trim();
            let mut stamps = Vec::new();
            while let Some(after) = rest.strip_prefix('[') {
                let Some(end) = after.find(']') else { break };
                if let Some(t) = stamp(&after[..end]) {
                    stamps.push(t);
                }
                rest = &after[end + 1..];
            }
            let text = rest.trim().to_string();
            for t in stamps {
                lines.push(Line { t, text: text.clone() });
            }
        }
        lines.sort_by(|a, b| a.t.total_cmp(&b.t));
        (!lines.is_empty()).then_some(Lyrics { lines })
    }

    /// The line being sung at `t` seconds (None before the first one).
    pub fn index_at(&self, t: f64) -> Option<usize> {
        self.lines.partition_point(|l| l.t <= t).checked_sub(1)
    }

    /// When the line after the one at `t` starts.
    pub fn next_after(&self, t: f64) -> Option<f64> {
        self.lines.iter().map(|l| l.t).find(|&lt| lt > t)
    }
}

/// "mm:ss", "mm:ss.xx" or "mm:ss:xx" -> seconds.
fn stamp(s: &str) -> Option<f64> {
    let (m, rest) = s.split_once(':')?;
    let m: f64 = m.trim().parse().ok()?;
    let rest = rest.replacen(':', ".", 1);
    let sec: f64 = rest.trim().parse().ok()?;
    (m >= 0.0 && (0.0..60.0).contains(&sec)).then_some(m * 60.0 + sec)
}

#[derive(Clone, Debug)]
pub struct Request {
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: f64,
}

pub struct LyricsService {
    tx: Sender<Request>,
}

impl LyricsService {
    pub fn start(sink: Sink) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("synapse-lyrics".into())
            .spawn(move || worker(rx, sink))
            .expect("lyrics thread");
        Self { tx }
    }

    pub fn request(&self, r: Request) {
        let _ = self.tx.send(r);
    }
}

fn worker(rx: Receiver<Request>, sink: Sink) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut client: Option<HttpClient> = None;
    while let Ok(mut req) = rx.recv() {
        // Skipping through tracks: only the newest one matters.
        while let Ok(newer) = rx.try_recv() {
            req = newer;
        }
        let lyrics = match cached(&req) {
            Some(hit) => hit,
            None => {
                let c = client.get_or_insert_with(|| make_client().expect("http client"));
                match fetch(c, &req) {
                    Ok(found) => {
                        store(&req, found.as_deref());
                        found
                    }
                    Err(e) => {
                        // Offline or the server is down: try again next time the song plays.
                        crate::log(&format!("lyrics: lookup failed for \"{}\" ({e})", req.title));
                        None
                    }
                }
            }
        };
        let lyrics = lyrics.as_deref().and_then(Lyrics::parse);
        sink.send(Event::Lyrics { key: req.key, lyrics });
    }
}

fn make_client() -> windows::core::Result<HttpClient> {
    let c = HttpClient::new()?;
    c.DefaultRequestHeaders()?.UserAgent()?.TryParseAdd(&HSTRING::from(USER_AGENT))?;
    Ok(c)
}

/// Wait for a WinRT operation, giving up (and cancelling it) after `TIMEOUT`.
fn wait<T: RuntimeType + 'static, P: RuntimeType + 'static>(op: IAsyncOperationWithProgress<T, P>) -> windows::core::Result<T> {
    let start = Instant::now();
    while op.Status()? == AsyncStatus::Started {
        if start.elapsed() > TIMEOUT {
            let _ = op.Cancel();
            return Err(E_ABORT.into());
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    op.GetResults()
}

/// GET `url`: Ok(None) on 404.
fn get(c: &HttpClient, url: &str) -> windows::core::Result<Option<String>> {
    let resp = wait(c.GetAsync(&Uri::CreateUri(&HSTRING::from(url))?)?)?;
    if resp.StatusCode()? == HttpStatusCode::NotFound {
        return Ok(None);
    }
    resp.EnsureSuccessStatusCode()?;
    Ok(Some(wait(resp.Content()?.ReadAsStringAsync()?)?.to_string()))
}

/// The track's synced lyrics as LRC text, or None when LRCLIB has none.
fn fetch(c: &HttpClient, r: &Request) -> windows::core::Result<Option<String>> {
    let synced = |v: &serde_json::Value| v.get("syncedLyrics").and_then(|s| s.as_str()).filter(|s| !s.trim().is_empty()).map(str::to_string);
    // The exact match first (it needs the duration), then a search.
    if r.duration > 0.0 {
        let url = format!(
            "{API}/get?track_name={}&artist_name={}&album_name={}&duration={}",
            enc(&r.title),
            enc(&r.artist),
            enc(&r.album),
            r.duration.round() as i64
        );
        if let Some(body) = get(c, &url)? {
            if let Some(s) = serde_json::from_str::<serde_json::Value>(&body).ok().as_ref().and_then(synced) {
                return Ok(Some(s));
            }
        }
    }
    let url = format!("{API}/search?track_name={}&artist_name={}", enc(&clean_title(&r.title)), enc(&r.artist));
    let Some(body) = get(c, &url)? else { return Ok(None) };
    let list: Vec<serde_json::Value> = serde_json::from_str(&body).unwrap_or_default();
    // The closest in length among those with synced lyrics.
    let best = list
        .iter()
        .filter_map(|v| synced(v).map(|s| (v.get("duration").and_then(|d| d.as_f64()).unwrap_or(0.0), s)))
        .filter(|(d, _)| r.duration <= 0.0 || (d - r.duration).abs() <= 8.0)
        .min_by(|a, b| (a.0 - r.duration).abs().total_cmp(&(b.0 - r.duration).abs()));
    Ok(best.map(|(_, s)| s))
}

/// "Song - Remastered 2011" / "Song (feat. X)" -> "Song", for the search.
fn clean_title(t: &str) -> String {
    let mut s = t.to_string();
    for cut in [" - ", " (feat", " (with", " [", " (Remaster", " (Live"] {
        if let Some(i) = s.find(cut).filter(|&i| i > 0) {
            s.truncate(i);
        }
    }
    s.trim().to_string()
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ------------------------------------------------------------ cache

fn cache_path(r: &Request) -> PathBuf {
    // FNV-1a over what identifies the song.
    let mut h: u64 = 0xcbf29ce484222325;
    for b in format!("{}\u{1}{}\u{1}{}", r.title, r.artist, r.duration.round()).bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into());
    base.join("Synapse").join("lyrics").join(format!("{h:016x}.lrc"))
}

/// Some(Some(lrc)) found earlier, Some(None) known to have none, None not looked up.
fn cached(r: &Request) -> Option<Option<String>> {
    let text = std::fs::read_to_string(cache_path(r)).ok()?;
    Some((!text.is_empty()).then_some(text))
}

/// Remember the answer; an empty file means "LRCLIB has none".
fn store(r: &Request, lrc: Option<&str>) {
    let p = cache_path(r);
    if let Some(dir) = p.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(p, lrc.unwrap_or(""));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc() {
        let l = Lyrics::parse("[ar: Someone]\n[00:12.50] first\n[00:15.00][01:02.00] chorus\n[00:20.25]\n").unwrap();
        let got: Vec<(f64, &str)> = l.lines.iter().map(|x| (x.t, x.text.as_str())).collect();
        assert_eq!(got, vec![(12.5, "first"), (15.0, "chorus"), (20.25, ""), (62.0, "chorus")]);
    }

    #[test]
    fn finds_the_current_line() {
        let l = Lyrics::parse("[00:10.00] a\n[00:20.00] b\n[00:30.00] c").unwrap();
        assert_eq!(l.index_at(5.0), None);
        assert_eq!(l.index_at(10.0), Some(0));
        assert_eq!(l.index_at(25.0), Some(1));
        assert_eq!(l.index_at(99.0), Some(2));
        assert_eq!(l.next_after(25.0), Some(30.0));
        assert_eq!(l.next_after(30.0), None);
    }

    #[test]
    fn rejects_untimed() {
        assert_eq!(Lyrics::parse("just words\nno times"), None);
    }

    #[test]
    fn cleans_titles_and_encodes() {
        assert_eq!(clean_title("Song - Remastered 2011"), "Song");
        assert_eq!(clean_title("Song (feat. X)"), "Song");
        assert_eq!(enc("a b&c/é"), "a%20b%26c%2F%C3%A9");
    }
}
