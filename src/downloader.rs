//! The Downloader add-on: the video or sound of a link, with yt-dlp.
//!
//! yt-dlp isn't part of Synapse: the first time the add-on is switched on,
//! its official release is fetched from GitHub into
//! `%LOCALAPPDATA%\Synapse\tools` and checked against the SHA-256 the same
//! release publishes; switching the add-on off deletes it. Each download is
//! one yt-dlp, without a window, inside a job object (Stop or quitting ends
//! it and anything it started, such as FFmpeg); a thread reads its lines
//! and ends with it. Nothing runs between downloads.

use crate::app::{Event, Sink};
use std::io::{BufRead, BufReader};
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::Cryptography::{BCryptHash, BCRYPT_SHA256_ALG_HANDLE};
use windows::Win32::System::Com::Urlmon::URLDownloadToFileW;
use windows::Win32::System::JobObjects::*;

const YTDLP_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe";
const SUMS_URL: &str = "https://github.com/yt-dlp/yt-dlp/releases/latest/download/SHA2-256SUMS";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// At most this many at once; the rest wait their turn.
pub const PARALLEL: usize = 2;

pub fn tools_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(|| ".".into());
    base.join("Synapse").join("tools")
}

pub fn ytdlp() -> PathBuf {
    tools_dir().join("yt-dlp.exe")
}

pub fn have_ytdlp() -> bool {
    ytdlp().exists()
}

/// Fetch yt-dlp (once) on a thread, checked against its published checksum.
pub fn fetch_ytdlp(sink: Sink) {
    std::thread::Builder::new()
        .name("synapse-ytdlp-fetch".into())
        .spawn(move || {
            let r = fetch().map_err(|e| {
                crate::log(&format!("downloader: couldn't get yt-dlp ({e})"));
                e
            });
            sink.send(Event::ToolReady(r));
        })
        .expect("fetch thread");
}

fn download_to(url: &str, to: &Path) -> Result<(), String> {
    let hr = unsafe { URLDownloadToFileW(None, &HSTRING::from(url), &HSTRING::from(to.as_os_str()), 0, None) };
    hr.map_err(|e| format!("download failed ({e})"))
}

fn fetch() -> Result<(), String> {
    let dir = tools_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tmp = dir.join("yt-dlp.exe.part");
    let sums = dir.join("SHA2-256SUMS");
    download_to(SUMS_URL, &sums)?;
    download_to(YTDLP_URL, &tmp)?;
    let want = std::fs::read_to_string(&sums)
        .ok()
        .and_then(|t| t.lines().find(|l| l.trim_end().ends_with(" yt-dlp.exe")).and_then(|l| l.split_whitespace().next()).map(str::to_lowercase));
    let _ = std::fs::remove_file(&sums);
    let data = std::fs::read(&tmp).map_err(|e| e.to_string())?;
    let got = sha256_hex(&data).ok_or("couldn't check the file")?;
    match want {
        Some(w) if w == got => {}
        _ => {
            let _ = std::fs::remove_file(&tmp);
            return Err("the download didn't match yt-dlp's published checksum".into());
        }
    }
    std::fs::rename(&tmp, ytdlp()).map_err(|e| e.to_string())?;
    crate::log("downloader: yt-dlp ready");
    Ok(())
}

/// Delete the tools (the add-on was switched off).
pub fn remove_tools() {
    let _ = std::fs::remove_file(ytdlp());
}

pub fn sha256_hex(data: &[u8]) -> Option<String> {
    let mut out = [0u8; 32];
    unsafe { BCryptHash(BCRYPT_SHA256_ALG_HANDLE, None, data, &mut out).ok().ok()? };
    Some(out.iter().map(|b| format!("{b:02x}")).collect())
}

/// What a running download reports.
#[derive(Clone, Debug)]
pub enum DlEvent {
    Title { id: u64, title: String },
    Progress { id: u64, pct: f32, speed: String, eta: String },
    Done { id: u64, path: String },
    Failed { id: u64, why: String },
}

/// One of yt-dlp's lines, as Synapse asked it to print them.
pub fn parse(id: u64, line: &str) -> Option<DlEvent> {
    let line = line.trim();
    if let Some(t) = line.strip_prefix("SYNAPSETITLE ") {
        return Some(DlEvent::Title { id, title: t.to_string() });
    }
    if let Some(p) = line.strip_prefix("SYNAPSEFILE ") {
        return Some(DlEvent::Done { id, path: p.to_string() });
    }
    if let Some(rest) = line.strip_prefix("SYNAPSE ") {
        let mut parts = rest.split_whitespace();
        let pct = parts.next()?.trim_end_matches('%').parse::<f32>().ok()?;
        let speed = parts.next().unwrap_or("").replace("MiB/s", "MB/s").replace("KiB/s", "KB/s");
        let eta = parts.next().unwrap_or("").to_string();
        return Some(DlEvent::Progress { id, pct: (pct / 100.0).clamp(0.0, 1.0), speed, eta });
    }
    line.strip_prefix("ERROR: ").map(|e| DlEvent::Failed { id, why: e.chars().take(120).collect() })
}

/// A running yt-dlp: its job object (ending it ends the whole tree).
pub struct Running {
    job: HANDLE,
}

unsafe impl Send for Running {}

impl Running {
    pub fn stop(&self) {
        unsafe {
            let _ = TerminateJobObject(self.job, 1);
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        unsafe {
            // Closing the job kills what's left (KILL_ON_JOB_CLOSE).
            let _ = CloseHandle(self.job);
        }
    }
}

fn on_path(exe: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|p| std::env::split_paths(&p).map(|d| d.join(exe)).find(|f| f.exists()))
}

/// Start downloading `url` (`audio` only, or video) into `out`.
pub fn start(id: u64, url: &str, audio: bool, out: &Path, sink: Sink) -> Result<Running, String> {
    let ffmpeg = on_path("ffmpeg.exe").or_else(|| Some(tools_dir().join("ffmpeg.exe")).filter(|p| p.exists()));
    let mut args: Vec<String> = vec![
        url.into(),
        "--newline".into(),
        "--no-playlist".into(),
        "--no-colors".into(),
        "--no-simulate".into(),
        "--progress".into(),
        "--progress-template".into(),
        "download:SYNAPSE %(progress._percent_str)s %(progress._speed_str)s %(progress._eta_str)s".into(),
        "--print".into(),
        "before_dl:SYNAPSETITLE %(title)s".into(),
        "--print".into(),
        "after_move:SYNAPSEFILE %(filepath)s".into(),
        "-P".into(),
        out.to_string_lossy().into(),
        "-o".into(),
        "%(title).90B [%(id)s].%(ext)s".into(),
    ];
    // YouTube needs a JavaScript runtime now; Node does the job if it's here.
    if on_path("node.exe").is_some() {
        args.extend(["--js-runtimes".into(), "node".into()]);
    }
    match (audio, &ffmpeg) {
        (true, _) => args.extend(["-f".into(), "ba[ext=m4a]/ba".into()]),
        (false, Some(f)) => args.extend([
            "-f".into(),
            "bv*[height<=1080]+ba/b".into(),
            "--merge-output-format".into(),
            "mp4".into(),
            "--ffmpeg-location".into(),
            f.to_string_lossy().into(),
        ]),
        // Without FFmpeg nothing can join picture and sound: the best single file.
        (false, None) => args.extend(["-f".into(), "b".into()]),
    }
    let mut child = Command::new(ytdlp())
        .args(&args)
        .env("PYTHONIOENCODING", "utf-8")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("couldn't start yt-dlp ({e})"))?;
    let job = unsafe {
        let job = CreateJobObjectW(None, PCWSTR::null()).map_err(|e| e.to_string())?;
        let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let _ = SetInformationJobObject(job, JobObjectExtendedLimitInformation, &info as *const _ as *const _, std::mem::size_of_val(&info) as u32);
        let _ = AssignProcessToJobObject(job, HANDLE(child.as_raw_handle()));
        job
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let err_sink = sink.clone();
    std::thread::spawn(move || {
        // Errors come on stderr.
        if let Some(e) = stderr {
            for line in BufReader::new(e).lines().map_while(Result::ok) {
                if let Some(ev) = parse(id, &line) {
                    err_sink.send(Event::Download(ev));
                }
            }
        }
    });
    std::thread::spawn(move || {
        let mut finished = false;
        if let Some(o) = stdout {
            for line in BufReader::new(o).lines().map_while(Result::ok) {
                if let Some(ev) = parse(id, &line) {
                    finished |= matches!(ev, DlEvent::Done { .. });
                    sink.send(Event::Download(ev));
                }
            }
        }
        let ok = child.wait().map(|s| s.success()).unwrap_or(false);
        if !finished {
            sink.send(Event::Download(DlEvent::Failed { id, why: if ok { "nothing was saved".into() } else { "stopped".into() } }));
        }
    });
    Ok(Running { job })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_lines() {
        match parse(3, "SYNAPSE  42.5%  1.20MiB/s 00:31") {
            Some(DlEvent::Progress { id: 3, pct, speed, eta }) => {
                assert!((pct - 0.425).abs() < 1e-6);
                assert_eq!((speed.as_str(), eta.as_str()), ("1.20MB/s", "00:31"));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(parse(1, "SYNAPSETITLE Some Video"), Some(DlEvent::Title { title, .. }) if title == "Some Video"));
        assert!(matches!(parse(1, "SYNAPSEFILE C:\\x\\a.mp4"), Some(DlEvent::Done { path, .. }) if path == "C:\\x\\a.mp4"));
        assert!(matches!(parse(1, "ERROR: Unsupported URL"), Some(DlEvent::Failed { .. })));
        assert!(parse(1, "[youtube] Extracting URL").is_none());
        assert!(parse(1, "SYNAPSE NA% NA NA").is_none());
    }

    #[test]
    fn sha256() {
        assert_eq!(sha256_hex(b"abc").unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
