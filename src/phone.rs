//! Phone drop: send files from your phone to the Shelf over Wi-Fi.
//!
//! While the Shelf's phone panel is open, a small web server listens on the
//! local network. The QR code holds its address plus a secret, random path;
//! only a phone that scanned it can open the page or send anything. Files
//! land in `Downloads\Synapse` and join the Shelf. Closing the panel (or 10
//! idle minutes) stops the server: nothing listens otherwise.

use crate::app::{Event, Sink};
use std::io::{BufWriter, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows::Win32::Security::Cryptography::{BCryptGenRandom, BCRYPT_USE_SYSTEM_PREFERRED_RNG};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT};

/// Largest file accepted (a long phone video fits).
const MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const HEAD_MAX: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub enum PhoneEvent {
    /// A phone opened the page.
    Connected,
    Receiving { name: String, done: u64, total: u64 },
    Saved(String),
    Failed(String),
}

pub struct PhoneDrop {
    pub url: String,
    port: u16,
    stop: Arc<AtomicBool>,
}

impl PhoneDrop {
    pub fn start(sink: Sink) -> Result<Self, String> {
        Self::start_into(sink, inbox(), Ipv4Addr::UNSPECIFIED)
    }

    /// Receive into `dest`, listening on `bind`. The tests use a folder of
    /// their own and this PC only (so Windows' firewall never asks about them).
    fn start_into(sink: Sink, dest: PathBuf, bind: Ipv4Addr) -> Result<Self, String> {
        let ip = lan_ip().ok_or("This PC isn't on a network")?;
        let listener = TcpListener::bind((bind, 0)).map_err(|e| format!("can't listen ({e})"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let token = token();
        let url = format!("http://{ip}:{port}/{token}/");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::Builder::new()
            .name("synapse-phone".into())
            .spawn(move || {
                // Blocks in accept (no CPU); Drop wakes it with a connection to itself.
                for conn in listener.incoming() {
                    if flag.load(Ordering::Relaxed) {
                        break;
                    }
                    let Ok(stream) = conn else { continue };
                    let (sink, token, dest) = (sink.clone(), token.clone(), dest.clone());
                    std::thread::spawn(move || {
                        if let Err(e) = serve(stream, &token, &dest, &sink) {
                            crate::log(&format!("phone drop: {e}"));
                        }
                    });
                }
            })
            .map_err(|e| e.to_string())?;
        crate::log(&format!("phone drop: listening on {ip}:{port}"));
        Ok(Self { url, port, stop })
    }
}

impl Drop for PhoneDrop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = TcpStream::connect_timeout(&(Ipv4Addr::LOCALHOST, self.port).into(), Duration::from_millis(300));
        crate::log("phone drop: stopped");
    }
}

/// This PC's address on the local network. Connecting a UDP socket sends
/// nothing; it only picks the interface a packet would leave from.
fn lan_ip() -> Option<Ipv4Addr> {
    let s = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    s.connect((Ipv4Addr::new(8, 8, 8, 8), 80)).ok()?;
    match s.local_addr().ok()?.ip() {
        std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// 128 random bits as hex: the secret part of the address.
fn token() -> String {
    let mut b = [0u8; 16];
    unsafe {
        let _ = BCryptGenRandom(None, &mut b, BCRYPT_USE_SYSTEM_PREFERRED_RNG);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `Downloads\Synapse`.
pub fn inbox() -> PathBuf {
    let downloads = unsafe {
        SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None).ok().map(|p| {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            PathBuf::from(s)
        })
    };
    downloads
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Downloads"))
        .join("Synapse")
}

struct Request {
    method: String,
    path: String,
    query: String,
    length: u64,
    /// Body bytes read along with the head.
    rest: Vec<u8>,
}

fn read_head(s: &mut TcpStream) -> std::io::Result<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > HEAD_MAX {
            return Err(std::io::ErrorKind::InvalidData.into());
        }
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let mut lines = head.split("\r\n");
    let mut first = lines.next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("").to_string();
    let target = first.next().unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let length = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse().ok())
        .unwrap_or(0);
    Ok(Request { method, path: path.to_string(), query: query.to_string(), length, rest: buf[end + 4..].to_vec() })
}

fn reply(s: &mut TcpStream, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!("HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n", body.len());
    s.write_all(head.as_bytes())?;
    s.write_all(body)
}

fn serve(mut s: TcpStream, token: &str, dest: &Path, sink: &Sink) -> std::io::Result<()> {
    s.set_read_timeout(Some(Duration::from_secs(60)))?;
    let req = read_head(&mut s)?;
    let base = format!("/{token}");
    // Without the secret there's nothing here, not even a hint.
    if !(req.path == base || req.path.starts_with(&format!("{base}/"))) {
        return reply(&mut s, "404 Not Found", "text/plain", b"Not found");
    }
    let sub = &req.path[base.len()..];
    match (req.method.as_str(), sub) {
        ("GET", "" | "/") => {
            sink.send(Event::Phone(PhoneEvent::Connected));
            let page = PAGE.replace("__TOKEN__", token);
            reply(&mut s, "200 OK", "text/html; charset=utf-8", page.as_bytes())
        }
        ("POST", "/upload") => {
            let name = query_param(&req.query, "name").map(|n| safe_name(&n)).unwrap_or_else(|| "file".into());
            if req.length == 0 || req.length > MAX_BYTES {
                return reply(&mut s, "413 Payload Too Large", "text/plain", b"Too big");
            }
            match receive(&mut s, &req, &name, dest, sink) {
                Ok(path) => {
                    sink.send(Event::Phone(PhoneEvent::Saved(path.to_string_lossy().to_string())));
                    reply(&mut s, "200 OK", "application/json", b"{\"ok\":true}")
                }
                Err(e) => {
                    sink.send(Event::Phone(PhoneEvent::Failed(format!("{name}: {e}"))));
                    reply(&mut s, "500 Internal Server Error", "text/plain", b"Failed")
                }
            }
        }
        _ => reply(&mut s, "404 Not Found", "text/plain", b"Not found"),
    }
}

/// Stream the body into a hidden temp file, then give it its real name.
fn receive(s: &mut TcpStream, req: &Request, name: &str, dest: &Path, sink: &Sink) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dest)?;
    let tmp = dest.join(format!(".synapse-incoming-{}", token()));
    let result = (|| {
        let mut out = BufWriter::with_capacity(256 * 1024, std::fs::File::create(&tmp)?);
        let total = req.length;
        let first = req.rest.len().min(total as usize);
        out.write_all(&req.rest[..first])?;
        let mut done = first as u64;
        let mut buf = vec![0u8; 64 * 1024];
        let mut told = 0u64;
        while done < total {
            let want = buf.len().min((total - done) as usize);
            let n = s.read(&mut buf[..want])?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the phone stopped sending"));
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            // Progress about every 2 %: smooth enough, few messages.
            if done - told >= (total / 50).max(256 * 1024) || done == total {
                told = done;
                sink.send(Event::Phone(PhoneEvent::Receiving { name: name.to_string(), done, total }));
            }
        }
        out.flush()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let path = unique(dest, name);
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

fn query_param(q: &str, key: &str) -> Option<String> {
    q.split('&').filter_map(|kv| kv.split_once('=')).find(|(k, _)| *k == key).map(|(_, v)| url_decode(v))
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16) {
                    Ok(v) => {
                        out.push(v);
                        i += 2;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// A name Windows accepts: no folders, no reserved characters or names.
fn safe_name(raw: &str) -> String {
    let base = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let mut s: String = base.chars().map(|c| if c.is_control() || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
    s = s.trim().trim_end_matches(['.', ' ']).to_string();
    if s.chars().count() > 150 {
        let (stem, ext) = split_ext(&s);
        s = format!("{}{}", stem.chars().take(140).collect::<String>(), ext);
    }
    let stem = split_ext(&s).0.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str()) || ((stem.starts_with("COM") || stem.starts_with("LPT")) && stem.len() == 4);
    if reserved {
        s = format!("_{s}");
    }
    if s.is_empty() || s.starts_with('.') && s.len() == 1 {
        "file".into()
    } else {
        s
    }
}

fn split_ext(s: &str) -> (&str, &str) {
    match s.rfind('.') {
        Some(i) if i > 0 => (&s[..i], &s[i..]),
        _ => (s, ""),
    }
}

/// `name`, or `name (2)`, `name (3)`... if taken.
fn unique(dir: &Path, name: &str) -> PathBuf {
    let p = dir.join(name);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = split_ext(name);
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// The QR code for `text`: its width and dark modules, row by row.
pub fn qr(text: &str) -> Option<(usize, Vec<bool>)> {
    let q = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium).ok()?;
    let n = q.size();
    let mut cells = Vec::with_capacity((n * n) as usize);
    for y in 0..n {
        for x in 0..n {
            cells.push(q.get_module(x, y));
        }
    }
    Some((n as usize, cells))
}

/// The page the phone opens.
const PAGE: &str = r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Send to Synapse</title>
<style>
:root{color-scheme:dark}
body{margin:0;font:16px -apple-system,system-ui,"Segoe UI",Roboto,sans-serif;background:#000;color:#f5f5f7;min-height:100vh;display:flex;align-items:center;justify-content:center}
main{width:min(92vw,440px);padding:32px 0;text-align:center}
.dot{width:56px;height:20px;border-radius:10px;background:#1c1c1e;margin:0 auto 22px;position:relative}
.dot:after{content:"";position:absolute;right:8px;top:6px;width:8px;height:8px;border-radius:4px;background:#30d158}
h1{font-size:24px;margin:0 0 8px}
p{color:#f5f5f79e;margin:0 0 26px;line-height:1.4}
label{display:block;background:#30d158;color:#000;font-weight:700;font-size:17px;padding:17px;border-radius:999px;cursor:pointer;user-select:none}
label:active{transform:scale(.98)}
input{display:none}
ul{list-style:none;padding:0;margin:26px 0 0;text-align:left}
li{background:#1c1c1e;border-radius:16px;padding:13px 15px;margin-bottom:9px}
.n{display:flex;justify-content:space-between;gap:12px;font-size:14px}
.n span:first-child{overflow:hidden;text-overflow:ellipsis;white-space:nowrap}
.n span:last-child{color:#f5f5f79e;flex:none}
.b{height:4px;background:#ffffff1f;border-radius:2px;margin-top:9px;overflow:hidden}
.b i{display:block;height:100%;width:0;background:#30d158;transition:width .15s}
.err .b i{background:#ff453a}
</style></head><body><main>
<div class="dot"></div>
<h1>Send to Synapse</h1>
<p>Pick photos, PDFs or any files.<br>They land on your PC's Shelf.</p>
<label>Choose files<input id="f" type="file" multiple></label>
<ul id="l"></ul>
</main><script>
const T="__TOKEN__",f=document.getElementById("f"),l=document.getElementById("l");
f.onchange=async()=>{for(const file of [...f.files]){await send(file)}f.value=""};
function send(file){return new Promise(done=>{
  const li=document.createElement("li");
  li.innerHTML='<div class="n"><span></span><span>0%</span></div><div class="b"><i></i></div>';
  li.querySelector("span").textContent=file.name;l.prepend(li);
  const pct=li.querySelectorAll("span")[1],bar=li.querySelector("i");
  const x=new XMLHttpRequest();
  x.open("POST","/"+T+"/upload?name="+encodeURIComponent(file.name));
  x.upload.onprogress=e=>{if(e.lengthComputable){const p=Math.round(e.loaded/e.total*100);pct.textContent=p+"%";bar.style.width=p+"%"}};
  const fail=()=>{li.className="err";pct.textContent="Failed";bar.style.width="100%";done()};
  x.onload=()=>{if(x.status==200){pct.textContent="Sent \u2713";bar.style.width="100%";done()}else fail()};
  x.onerror=fail;x.send(file);
})}
</script></body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_made_safe() {
        assert_eq!(safe_name("photo.jpg"), "photo.jpg");
        assert_eq!(safe_name("../../evil.exe"), "evil.exe");
        assert_eq!(safe_name("C:\\x\\a:b?.pdf"), "a_b_.pdf");
        assert_eq!(safe_name("con.txt"), "_con.txt");
        assert_eq!(safe_name("  ..  "), "file");
        assert_eq!(safe_name("name. "), "name");
    }

    #[test]
    fn decodes_names() {
        assert_eq!(url_decode("My%20Scan%20%281%29.pdf"), "My Scan (1).pdf");
        assert_eq!(url_decode("caf%C3%A9.jpg"), "café.jpg");
        assert_eq!(url_decode("100%"), "100%");
        assert_eq!(query_param("a=1&name=x%2By.png", "name").as_deref(), Some("x+y.png"));
    }

    #[test]
    fn tokens_are_random_hex() {
        let (a, b) = (token(), token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    fn http(port: u16, req: &[u8]) -> String {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        s.write_all(req).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        out
    }

    #[test]
    fn a_phone_sends_a_file() {
        let dir = std::env::temp_dir().join(format!("synapse-phone-test-{}", token()));
        let (tx, rx) = std::sync::mpsc::channel();
        let Ok(drop) = PhoneDrop::start_into(Sink::for_test(tx), dir.clone(), Ipv4Addr::LOCALHOST) else {
            eprintln!("no network here; skipped");
            return;
        };
        let path = drop.url.splitn(4, '/').nth(3).unwrap().trim_end_matches('/').to_string();
        // The page needs the secret; a guess gets nothing.
        assert!(http(drop.port, b"GET / HTTP/1.1\r\n\r\n").starts_with("HTTP/1.1 404"));
        assert!(http(drop.port, format!("GET /{path}/ HTTP/1.1\r\n\r\n").as_bytes()).contains("Send to Synapse"));
        let body = b"%PDF-1.4 hello";
        let mut req = format!("POST /{path}/upload?name=My%20Scan.pdf HTTP/1.1\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        req.extend_from_slice(body);
        assert!(http(drop.port, &req).starts_with("HTTP/1.1 200"));
        assert_eq!(std::fs::read(dir.join("My Scan.pdf")).unwrap(), body);
        // The same name again doesn't overwrite.
        assert!(http(drop.port, &req).starts_with("HTTP/1.1 200"));
        assert!(dir.join("My Scan (2).pdf").exists());
        let saved = rx.try_iter().filter(|e| matches!(e, Event::Phone(PhoneEvent::Saved(_)))).count();
        assert_eq!(saved, 2);
        drop_and_clean(drop, &dir);
    }

    fn drop_and_clean(d: PhoneDrop, dir: &Path) {
        drop(d);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn qr_encodes() {
        let (n, cells) = qr("http://192.168.1.20:51234/0123456789abcdef0123456789abcdef/").unwrap();
        assert_eq!(cells.len(), n * n);
        assert!(n >= 21);
    }
}
