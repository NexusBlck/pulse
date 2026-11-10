//! Bluetooth devices connecting and disconnecting (earbuds, headphones,
//! mice...), from Windows' device watcher: it calls us on changes, nothing
//! polls. Paired devices only, classic and Low Energy.
//!
//! A device's battery, when Windows knows it (most headphones report it),
//! is looked up just after it connects and follows as a second event.

use crate::app::{Event, Sink};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use windows::core::{Interface, IInspectable, GUID, HSTRING};
use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceInformationKind, DeviceInformationUpdate, DeviceWatcher};
use windows::Foundation::Collections::{IIterable, IMapView};
use windows::Foundation::{IReference, TypedEventHandler};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

const CONNECTED: &str = "System.Devices.Aep.IsConnected";
const CONTAINER: &str = "System.Devices.Aep.ContainerId";
const MAJOR_CLASS: &str = "System.Devices.Aep.Bluetooth.Cod.Major";
/// Where Windows keeps a Bluetooth device's battery level (on its PnP node).
const BATTERY: &str = "{104EA319-6EE2-4701-BD47-8DDBF425BBE5} 2";
/// Classic and LE entries of one device often connect together: one pop-up.
const SAME_DEVICE_S: f64 = 4.0;

#[derive(Clone, Debug)]
pub struct Device {
    pub name: String,
    pub connected: bool,
    /// Headphones, speakers... (Bluetooth's audio/video class).
    pub audio: bool,
    container: Option<GUID>,
}

enum Msg {
    Added { id: String, dev: Device },
    Updated { id: String, connected: Option<bool> },
    Removed(String),
    Ready,
}

pub fn start(sink: Sink) {
    std::thread::Builder::new()
        .name("synapse-bluetooth".into())
        .spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let (tx, rx) = mpsc::channel();
            let watchers: Vec<DeviceWatcher> = [BluetoothDevice::GetDeviceSelectorFromPairingState(true), BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)]
                .into_iter()
                .filter_map(|aqs| aqs.ok())
                .filter_map(|aqs| match watch(&aqs, tx.clone()) {
                    Ok(w) => Some(w),
                    Err(e) => {
                        crate::log(&format!("bluetooth: no watcher ({e})"));
                        None
                    }
                })
                .collect();
            if watchers.is_empty() {
                return;
            }
            run(rx, watchers.len(), &sink);
            for w in watchers {
                let _ = w.Stop();
            }
        })
        .expect("bluetooth thread");
}

fn props() -> windows::core::Result<IIterable<HSTRING>> {
    IIterable::try_from(vec![HSTRING::from(CONNECTED), HSTRING::from(CONTAINER), HSTRING::from(MAJOR_CLASS)])
}

fn watch(aqs: &HSTRING, tx: Sender<Msg>) -> windows::core::Result<DeviceWatcher> {
    let w = DeviceInformation::CreateWatcherWithKindAqsFilterAndAdditionalProperties(aqs, &props()?, DeviceInformationKind::AssociationEndpoint)?;
    let t = tx.clone();
    w.Added(&TypedEventHandler::<DeviceWatcher, DeviceInformation>::new(move |_, info| {
        if let Some(info) = info.as_ref() {
            if let (Ok(id), Ok(props)) = (info.Id(), info.Properties()) {
                let name = info.Name().map(|n| n.to_string()).unwrap_or_default();
                let dev = Device {
                    connected: get_bool(&props, CONNECTED).unwrap_or(false),
                    audio: get_u8(&props, MAJOR_CLASS) == Some(4) || sounds_like_audio(&name),
                    container: get_guid(&props, CONTAINER),
                    name,
                };
                let _ = t.send(Msg::Added { id: id.to_string(), dev });
            }
        }
        Ok(())
    }))?;
    let t = tx.clone();
    w.Updated(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, upd| {
        if let Some(upd) = upd.as_ref() {
            if let (Ok(id), Ok(props)) = (upd.Id(), upd.Properties()) {
                let _ = t.send(Msg::Updated { id: id.to_string(), connected: get_bool(&props, CONNECTED) });
            }
        }
        Ok(())
    }))?;
    let t = tx.clone();
    w.Removed(&TypedEventHandler::<DeviceWatcher, DeviceInformationUpdate>::new(move |_, upd| {
        if let Some(id) = upd.as_ref().and_then(|u| u.Id().ok()) {
            let _ = t.send(Msg::Removed(id.to_string()));
        }
        Ok(())
    }))?;
    w.EnumerationCompleted(&TypedEventHandler::<DeviceWatcher, IInspectable>::new(move |_, _| {
        let _ = tx.send(Msg::Ready);
        Ok(())
    }))?;
    w.Start()?;
    Ok(w)
}

fn lookup(props: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<IInspectable> {
    props.Lookup(&HSTRING::from(key)).ok()
}

fn get_bool(props: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<bool> {
    lookup(props, key)?.cast::<IReference<bool>>().ok()?.Value().ok()
}

/// A small number, whichever integer type Windows stored it as.
fn get_u8(props: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<u8> {
    let v = lookup(props, key)?;
    if let Some(n) = v.cast::<IReference<u8>>().ok().and_then(|r| r.Value().ok()) {
        return Some(n);
    }
    if let Some(n) = v.cast::<IReference<u16>>().ok().and_then(|r| r.Value().ok()) {
        return u8::try_from(n).ok();
    }
    v.cast::<IReference<u32>>().ok().and_then(|r| r.Value().ok()).and_then(|n| u8::try_from(n).ok())
}

/// Headphones by name, for devices whose class Windows doesn't report.
fn sounds_like_audio(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    ["airpods", "buds", "headphone", "headset", "earphone", "speaker", "beats", "wh-", "wf-", "jbl", "soundcore", "audio"].iter().any(|k| n.contains(k))
}

fn get_guid(props: &IMapView<HSTRING, IInspectable>, key: &str) -> Option<GUID> {
    lookup(props, key)?.cast::<IReference<GUID>>().ok()?.Value().ok()
}

fn run(rx: Receiver<Msg>, watchers: usize, sink: &Sink) {
    let mut devices: HashMap<String, Device> = HashMap::new();
    let mut ready = 0;
    // The last pop-up per device name, to merge a device's classic and LE entries.
    let mut told: HashMap<String, (bool, Instant)> = HashMap::new();
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Added { id, dev } => {
                devices.insert(id, dev);
            }
            Msg::Removed(id) => {
                devices.remove(&id);
            }
            Msg::Ready => {
                ready += 1;
                if ready == watchers {
                    let on: Vec<&str> = devices.values().filter(|d| d.connected).map(|d| d.name.as_str()).collect();
                    crate::log(&format!("bluetooth: {} paired, connected now: {on:?}", devices.len()));
                }
            }
            Msg::Updated { id, connected: Some(now) } => {
                let Some(dev) = devices.get_mut(&id) else { continue };
                if dev.connected == now {
                    continue;
                }
                dev.connected = now;
                // Before the first listing is done, changes are just the starting state.
                if ready < watchers || dev.name.is_empty() {
                    continue;
                }
                let dup = told.get(&dev.name).is_some_and(|(c, at)| *c == now && at.elapsed().as_secs_f64() < SAME_DEVICE_S);
                told.insert(dev.name.clone(), (now, Instant::now()));
                if dup {
                    continue;
                }
                let dev = dev.clone();
                sink.send(Event::Bluetooth { name: dev.name.clone(), connected: now, audio: dev.audio, battery: None });
                if now {
                    // Windows fills in the battery a moment after the connection.
                    if let Some(level) = dev.container.and_then(battery_soon) {
                        sink.send(Event::BluetoothBattery { name: dev.name, battery: level });
                    }
                }
            }
            Msg::Updated { .. } => {}
        }
    }
}

/// The battery level of the device in `container`, waiting up to a few seconds for it.
fn battery_soon(container: GUID) -> Option<u8> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(4) {
        std::thread::sleep(Duration::from_millis(700));
        if let Some(b) = battery(container) {
            return Some(b);
        }
    }
    None
}

fn battery(container: GUID) -> Option<u8> {
    let aqs = HSTRING::from(format!("System.Devices.ContainerId:=\"{{{container:?}}}\""));
    let props = IIterable::try_from(vec![HSTRING::from(BATTERY)]).ok()?;
    let found = DeviceInformation::FindAllAsyncWithKindAqsFilterAndAdditionalProperties(&aqs, &props, DeviceInformationKind::Device).ok()?.get().ok()?;
    let n = found.Size().ok()?;
    (0..n).filter_map(|i| found.GetAt(i).ok()).find_map(|d| get_u8(&d.Properties().ok()?, BATTERY)).filter(|b| *b <= 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_names() {
        assert!(sounds_like_audio("AirPods Pro"));
        assert!(sounds_like_audio("Galaxy Buds2"));
        assert!(sounds_like_audio("WH-1000XM5"));
        assert!(!sounds_like_audio("BT5.2 Mouse"));
    }

    /// Lists this PC's paired Bluetooth devices: `cargo test bluetooth -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lists_paired_devices() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let (tx, rx) = mpsc::channel();
        let aqs = [BluetoothDevice::GetDeviceSelectorFromPairingState(true).unwrap(), BluetoothLEDevice::GetDeviceSelectorFromPairingState(true).unwrap()];
        let ws: Vec<_> = aqs.iter().map(|a| watch(a, tx.clone()).unwrap()).collect();
        let mut ready = 0;
        while let Ok(m) = rx.recv_timeout(Duration::from_secs(5)) {
            match m {
                Msg::Added { dev, .. } => {
                    let bat = if dev.connected { dev.container.and_then(battery) } else { None };
                    println!("{:30} connected={} audio={} battery={:?}", dev.name, dev.connected, dev.audio, bat);
                }
                Msg::Ready => {
                    ready += 1;
                    if ready == ws.len() {
                        break;
                    }
                }
                _ => {}
            }
        }
        for w in ws {
            let _ = w.Stop();
        }
    }
}
