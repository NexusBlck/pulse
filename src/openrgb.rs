//! RGB keyboards (and any device OpenRGB drives) in the island's palette,
//! through OpenRGB's SDK server (OpenRGB > SDK Server > Start Server; port
//! 6742 by default, this computer only).
//!
//! A worker thread holds the connection: it lists the devices, puts each in
//! its direct ("custom") mode and sets every LED whenever the colour changes.
//! Nothing runs while the colour stays put; without a server it tries again
//! at most every 20 seconds, and only when there's a colour to send.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const REQUEST_CONTROLLER_COUNT: u32 = 0;
const REQUEST_CONTROLLER_DATA: u32 = 1;
const SET_CLIENT_NAME: u32 = 50;
const UPDATE_LEDS: u32 = 1050;
const SET_CUSTOM_MODE: u32 = 1100;
const RETRY: Duration = Duration::from_secs(20);

pub struct OpenRgb {
    tx: Sender<[u8; 3]>,
}

impl OpenRgb {
    pub fn start(port: u16) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("synapse-openrgb".into())
            .spawn(move || worker(rx, port))
            .expect("openrgb thread");
        Self { tx }
    }

    pub fn set(&self, rgb: [u8; 3]) {
        let _ = self.tx.send(rgb);
    }
}

struct Conn {
    s: TcpStream,
    /// Each device's LED count, by index.
    leds: Vec<u16>,
}

fn worker(rx: Receiver<[u8; 3]>, port: u16) {
    let mut conn: Option<Conn> = None;
    let mut tried: Option<Instant> = None;
    let mut want: Option<[u8; 3]> = None;
    loop {
        // Wait for a colour; while one is waiting for a server, wake to retry.
        let got = match (want, &conn) {
            (Some(_), None) => rx.recv_timeout(RETRY),
            _ => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(c) => want = Some(c),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
        // Only the newest colour matters.
        while let Ok(c) = rx.try_recv() {
            want = Some(c);
        }
        let Some(color) = want else { continue };
        if conn.is_none() && tried.map_or(true, |t| t.elapsed() >= RETRY) {
            tried = Some(Instant::now());
            match connect(port) {
                Ok(c) => {
                    crate::log(&format!("openrgb: connected, {} device(s)", c.leds.len()));
                    conn = Some(c);
                }
                Err(e) => crate::log(&format!("openrgb: no server on port {port} ({e})")),
            }
        }
        if let Some(c) = conn.as_mut() {
            if let Err(e) = paint(c, color) {
                crate::log(&format!("openrgb: lost the server ({e})"));
                conn = None;
            } else {
                want = None;
            }
        }
        // Don't flood the devices: keyboards update at a few frames a second.
        std::thread::sleep(Duration::from_millis(120));
    }
}

fn packet(dev: u32, id: u32, data: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(16 + data.len());
    p.extend_from_slice(b"ORGB");
    p.extend_from_slice(&dev.to_le_bytes());
    p.extend_from_slice(&id.to_le_bytes());
    p.extend_from_slice(&(data.len() as u32).to_le_bytes());
    p.extend_from_slice(data);
    p
}

fn read_packet(s: &mut TcpStream) -> std::io::Result<(u32, Vec<u8>)> {
    let mut head = [0u8; 16];
    s.read_exact(&mut head)?;
    if &head[..4] != b"ORGB" {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not an OpenRGB server"));
    }
    let id = u32::from_le_bytes(head[8..12].try_into().unwrap());
    let len = u32::from_le_bytes(head[12..16].try_into().unwrap()) as usize;
    if len > 4 * 1024 * 1024 {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "reply too big"));
    }
    let mut data = vec![0u8; len];
    s.read_exact(&mut data)?;
    Ok((id, data))
}

fn connect(port: u16) -> std::io::Result<Conn> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(500))?;
    s.set_read_timeout(Some(Duration::from_secs(3)))?;
    s.set_nodelay(true)?;
    // Protocol 0 (no version request): every OpenRGB server speaks it.
    s.write_all(&packet(0, SET_CLIENT_NAME, b"Synapse\0"))?;
    s.write_all(&packet(0, REQUEST_CONTROLLER_COUNT, &[]))?;
    let (_, d) = read_packet(&mut s)?;
    let n = d.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
    let mut leds = Vec::new();
    for i in 0..n.min(64) {
        s.write_all(&packet(i, REQUEST_CONTROLLER_DATA, &[]))?;
        let (_, d) = read_packet(&mut s)?;
        let count = led_count(&d).unwrap_or(0);
        leds.push(count);
        if count > 0 {
            s.write_all(&packet(i, SET_CUSTOM_MODE, &[]))?;
        }
    }
    Ok(Conn { s, leds })
}

/// Every LED of every device in `rgb`.
fn paint(c: &mut Conn, rgb: [u8; 3]) -> std::io::Result<()> {
    for (i, &n) in c.leds.iter().enumerate() {
        if n == 0 {
            continue;
        }
        let mut d = Vec::with_capacity(6 + 4 * n as usize);
        d.extend_from_slice(&((6 + 4 * n as u32).to_le_bytes()));
        d.extend_from_slice(&n.to_le_bytes());
        for _ in 0..n {
            d.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 0]);
        }
        c.s.write_all(&packet(i as u32, UPDATE_LEDS, &d))?;
    }
    Ok(())
}

/// A little reader over a controller description.
struct Cursor<'a> {
    d: &'a [u8],
    at: usize,
}

impl Cursor<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.d.get(self.at..self.at + n)?;
        self.at += n;
        Some(s)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn string(&mut self) -> Option<()> {
        let n = self.u16()? as usize;
        self.take(n).map(|_| ())
    }
}

/// How many LED colours a device has, from its protocol-0 description:
/// size, type, five strings, its modes, its zones, its LEDs, then the colours.
fn led_count(d: &[u8]) -> Option<u16> {
    let mut c = Cursor { d, at: 0 };
    c.u32()?; // size
    c.u32()?; // type
    for _ in 0..5 {
        c.string()?; // name, description, version, serial, location
    }
    let modes = c.u16()?;
    c.u32()?; // active mode
    for _ in 0..modes {
        c.string()?;
        c.take(4 * 9)?; // value, flags, speed min/max, colours min/max, speed, direction, colour mode
        let n = c.u16()? as usize;
        c.take(4 * n)?;
    }
    let zones = c.u16()?;
    for _ in 0..zones {
        c.string()?;
        c.take(4 * 4)?; // type, LEDs min/max/count
        let matrix = c.u16()? as usize;
        c.take(matrix)?;
    }
    let leds = c.u16()?;
    for _ in 0..leds {
        c.string()?;
        c.u32()?;
    }
    c.u16()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &mut Vec<u8>, t: &str) {
        v.extend_from_slice(&((t.len() + 1) as u16).to_le_bytes());
        v.extend_from_slice(t.as_bytes());
        v.push(0);
    }

    /// A keyboard as an OpenRGB server describes it: one mode, one zone, 3 LEDs.
    fn keyboard() -> Vec<u8> {
        let mut v = vec![0; 4];
        v.extend_from_slice(&2u32.to_le_bytes());
        for t in ["Laptop Keyboard", "desc", "1.0", "", "HID"] {
            s(&mut v, t);
        }
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&0u32.to_le_bytes());
        s(&mut v, "Direct");
        v.extend_from_slice(&[0u8; 36]);
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&[1, 2, 3, 0]);
        v.extend_from_slice(&1u16.to_le_bytes());
        s(&mut v, "Keys");
        v.extend_from_slice(&[0u8; 16]);
        v.extend_from_slice(&12u16.to_le_bytes());
        v.extend_from_slice(&[0u8; 12]);
        v.extend_from_slice(&3u16.to_le_bytes());
        for k in ["A", "B", "C"] {
            s(&mut v, k);
            v.extend_from_slice(&0u32.to_le_bytes());
        }
        v.extend_from_slice(&3u16.to_le_bytes());
        v.extend_from_slice(&[0u8; 12]);
        v
    }

    #[test]
    fn counts_leds() {
        assert_eq!(led_count(&keyboard()), Some(3));
        assert_eq!(led_count(&keyboard()[..20]), None);
    }

    #[test]
    fn packets_have_the_header() {
        let p = packet(2, UPDATE_LEDS, &[9, 9]);
        assert_eq!(&p[..4], b"ORGB");
        assert_eq!(u32::from_le_bytes(p[4..8].try_into().unwrap()), 2);
        assert_eq!(u32::from_le_bytes(p[8..12].try_into().unwrap()), UPDATE_LEDS);
        assert_eq!(u32::from_le_bytes(p[12..16].try_into().unwrap()), 2);
    }
}
