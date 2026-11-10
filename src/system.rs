//! The System add-on's readings: CPU, GPU, memory, network, disk and the
//! busiest apps, once a second, on a thread that runs only while the
//! System page is on screen. The GPU counters (Windows' performance data
//! helper, a few MB once loaded) load the first time the page opens.

use crate::app::{Event, Sink};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::System::Performance::*;
use windows::Win32::System::ProcessStatus::EnumProcesses;
use windows::Win32::System::SystemInformation::{GetSystemInfo, GetTickCount64, GlobalMemoryStatusEx, MEMORYSTATUSEX, SYSTEM_INFO};
use windows::Win32::System::Threading::{GetProcessTimes, GetSystemTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

/// Seconds of history the sparklines show.
pub const HISTORY: usize = 60;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cpu: f32,
    pub gpu: Option<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    /// Bytes a second, down and up.
    pub net_down: f64,
    pub net_up: f64,
    pub disk_free: u64,
    pub disk_total: u64,
    pub uptime_s: u64,
    /// The busiest apps: name and share of all the CPU (0..1).
    pub top: Vec<(String, f32)>,
}

struct Shared {
    on: Mutex<bool>,
    cv: Condvar,
    stop: AtomicBool,
}

pub struct SystemMonitor {
    s: Arc<Shared>,
}

impl SystemMonitor {
    pub fn start(sink: Sink) -> Self {
        let s = Arc::new(Shared { on: Mutex::new(false), cv: Condvar::new(), stop: AtomicBool::new(false) });
        let t = s.clone();
        std::thread::Builder::new()
            .name("synapse-system".into())
            .spawn(move || run(t, sink))
            .expect("system thread");
        Self { s }
    }

    /// Read while `on` (the page is on screen); sleep otherwise.
    pub fn set(&self, on: bool) {
        let mut cur = self.s.on.lock().unwrap();
        if *cur != on {
            *cur = on;
            self.s.cv.notify_all();
        }
    }
}

impl Drop for SystemMonitor {
    fn drop(&mut self) {
        self.s.stop.store(true, Ordering::Relaxed);
        self.s.cv.notify_all();
    }
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

struct Reader {
    cpus: f64,
    last_sys: Option<(u64, u64, u64)>,
    last_net: Option<(u64, u64, Instant)>,
    last_procs: HashMap<u32, (String, u64)>,
    last_wall: Option<Instant>,
    gpu: Option<(isize, isize)>,
}

fn run(s: Arc<Shared>, sink: Sink) {
    let mut info = SYSTEM_INFO::default();
    unsafe { GetSystemInfo(&mut info) };
    let mut r = Reader { cpus: info.dwNumberOfProcessors.max(1) as f64, last_sys: None, last_net: None, last_procs: HashMap::new(), last_wall: None, gpu: None };
    loop {
        {
            let mut on = s.on.lock().unwrap();
            if !*on {
                // Coming back later starts fresh: no rate over the time asleep.
                r.last_sys = None;
                r.last_net = None;
                r.last_procs.clear();
                r.last_wall = None;
            }
            while !*on && !s.stop.load(Ordering::Relaxed) {
                on = s.cv.wait(on).unwrap();
            }
        }
        if s.stop.load(Ordering::Relaxed) {
            break;
        }
        if r.gpu.is_none() {
            r.gpu = open_gpu();
        }
        let snap = r.read();
        sink.send(Event::System(Box::new(snap)));
        std::thread::sleep(Duration::from_millis(1000));
    }
}

/// The 3D engines' utilisation counters, summed over every app using the GPU.
fn open_gpu() -> Option<(isize, isize)> {
    unsafe {
        let mut q = 0isize;
        if PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut q) != 0 {
            return None;
        }
        let mut c = 0isize;
        if PdhAddEnglishCounterW(q, w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage"), 0, &mut c) != 0 {
            let _ = PdhCloseQuery(q);
            return None;
        }
        let _ = PdhCollectQueryData(q);
        Some((q, c))
    }
}

fn read_gpu(q: isize, c: isize) -> Option<f32> {
    unsafe {
        if PdhCollectQueryData(q) != 0 {
            return None;
        }
        let (mut size, mut count) = (0u32, 0u32);
        let _ = PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, None);
        if size == 0 {
            return Some(0.0);
        }
        let mut buf = vec![0u8; size as usize];
        let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
        if PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) != 0 {
            return None;
        }
        let sum: f64 = (0..count as usize).map(|i| (*items.add(i)).FmtValue.Anonymous.doubleValue).sum();
        Some((sum as f32 / 100.0).clamp(0.0, 1.0))
    }
}

impl Reader {
    fn read(&mut self) -> Snapshot {
        let mut snap = Snapshot::default();
        unsafe {
            // CPU: how much of the time since last look wasn't idle.
            let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
            if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_ok() {
                let now = (ft(idle), ft(kernel), ft(user));
                if let Some((i0, k0, u0)) = self.last_sys {
                    let total = (now.1 - k0) + (now.2 - u0);
                    if total > 0 {
                        snap.cpu = (1.0 - (now.0 - i0) as f64 / total as f64).clamp(0.0, 1.0) as f32;
                    }
                }
                self.last_sys = Some(now);
            }
            let mut mem = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            if GlobalMemoryStatusEx(&mut mem).is_ok() {
                snap.mem_total = mem.ullTotalPhys;
                snap.mem_used = mem.ullTotalPhys - mem.ullAvailPhys;
            }
            let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
            if GetDiskFreeSpaceExW(w!("C:\\"), Some(&mut avail), Some(&mut total), Some(&mut free)).is_ok() {
                snap.disk_free = avail;
                snap.disk_total = total;
            }
            snap.uptime_s = GetTickCount64() / 1000;
        }
        if let Some((q, c)) = self.gpu {
            snap.gpu = read_gpu(q, c);
        }
        let (down, up) = net_totals();
        let now = Instant::now();
        if let Some((d0, u0, t0)) = self.last_net {
            let dt = now.duration_since(t0).as_secs_f64().max(0.1);
            snap.net_down = down.saturating_sub(d0) as f64 / dt;
            snap.net_up = up.saturating_sub(u0) as f64 / dt;
        }
        self.last_net = Some((down, up, now));
        snap.top = self.top_apps(now);
        snap
    }

    /// Every app's CPU since last look, grouped by name (a browser's many processes as one).
    fn top_apps(&mut self, now: Instant) -> Vec<(String, f32)> {
        let wall = self.last_wall.map(|t| now.duration_since(t).as_secs_f64());
        self.last_wall = Some(now);
        let mut pids = vec![0u32; 2048];
        let mut needed = 0u32;
        unsafe {
            if EnumProcesses(pids.as_mut_ptr(), (pids.len() * 4) as u32, &mut needed).is_err() {
                return Vec::new();
            }
        }
        pids.truncate(needed as usize / 4);
        let mut seen = HashMap::new();
        let mut by_name: HashMap<String, f64> = HashMap::new();
        for pid in pids.into_iter().filter(|p| *p > 4) {
            let Some((name, cpu)) = proc_time(pid, self.last_procs.get(&pid).map(|(n, _)| n.clone())) else { continue };
            if let (Some(wall), Some((_, before))) = (wall, self.last_procs.get(&pid)) {
                let used = cpu.saturating_sub(*before) as f64 / 1e7;
                *by_name.entry(name.clone()).or_default() += used / (wall * self.cpus);
            }
            seen.insert(pid, (name, cpu));
        }
        self.last_procs = seen;
        let mut v: Vec<(String, f32)> = by_name.into_iter().filter(|(n, _)| n != "Idle").map(|(n, s)| (n, s.min(1.0) as f32)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1));
        v.truncate(4);
        v
    }
}

/// A process's name (looked up once) and the CPU time it has used (100 ns units).
fn proc_time(pid: u32, known: Option<String>) -> Option<(String, u64)> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let times = GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u).is_ok();
        let name = known.or_else(|| {
            let mut buf = [0u16; 520];
            let mut len = buf.len() as u32;
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).ok()?;
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            let file = path.rsplit('\\').next().unwrap_or(&path);
            Some(file.trim_end_matches(".exe").trim_end_matches(".EXE").to_string())
        });
        let _ = CloseHandle(h);
        if !times {
            return None;
        }
        Some((name?, ft(k) + ft(u)))
    }
}

/// Bytes received and sent so far over the PC's real network adapters.
fn net_totals() -> (u64, u64) {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return (0, 0);
        }
        let t = &*table;
        let rows = std::slice::from_raw_parts(t.Table.as_ptr(), t.NumEntries as usize);
        let (mut down, mut up) = (0u64, 0u64);
        for r in rows {
            // Hardware adapters only: virtual ones would count the same bytes twice.
            let hardware = r.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && r.OperStatus.0 == 1 {
                down += r.InOctets;
                up += r.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
        (down, up)
    }
}

/// "3.4 MB/s", "120 KB/s".
pub fn fmt_rate(b: f64) -> String {
    if b >= 1e6 {
        format!("{:.1} MB/s", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB/s", b / 1e3)
    } else {
        format!("{:.0} B/s", b)
    }
}

/// "12.3 GB".
pub fn fmt_gb(b: u64) -> String {
    format!("{:.1} GB", b as f64 / 1_073_741_824.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_rate(3_400_000.0), "3.4 MB/s");
        assert_eq!(fmt_rate(120_000.0), "120 KB/s");
        assert_eq!(fmt_gb(16 * 1_073_741_824), "16.0 GB");
    }

    #[test]
    fn reads_this_pc() {
        let mut info = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut info) };
        let mut r = Reader { cpus: info.dwNumberOfProcessors as f64, last_sys: None, last_net: None, last_procs: HashMap::new(), last_wall: None, gpu: None };
        r.read();
        std::thread::sleep(Duration::from_millis(300));
        let s = r.read();
        assert!(s.mem_total > 0 && s.mem_used <= s.mem_total);
        assert!((0.0..=1.0).contains(&s.cpu));
        assert!(s.disk_total > 0);
    }
}
