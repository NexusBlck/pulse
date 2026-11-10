//! System volume: Windows calls us back when it changes (no polling), and
//! when the default output device changes (headphones plugged in) we follow
//! the new one.

use crate::app::{Event, Sink};
use windows::core::{implement, Result, PCWSTR};
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eConsole, eRender, EDataFlow, ERole, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl, MMDeviceEnumerator,
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCb {
    sink: Sink,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCb_Impl {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        if let Some(d) = unsafe { data.as_ref() } {
            self.sink.send(Event::Volume { level: d.fMasterVolume, muted: d.bMuted.as_bool() });
        }
        Ok(())
    }
}

#[implement(IMMNotificationClient)]
struct DeviceCb {
    sink: Sink,
}

impl IMMNotificationClient_Impl for DeviceCb_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> Result<()> {
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> Result<()> {
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> Result<()> {
        if flow == eRender && role == eConsole {
            self.sink.send(Event::AudioDevice);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        Ok(())
    }
}

pub struct Volume {
    enumerator: IMMDeviceEnumerator,
    device_cb: IMMNotificationClient,
    endpoint: Option<(IAudioEndpointVolume, IAudioEndpointVolumeCallback)>,
    /// How loud the output is right now (the ambient glow's breath).
    meter: Option<IAudioMeterInformation>,
    sink: Sink,
}

impl Volume {
    pub fn start(sink: Sink) -> Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device_cb: IMMNotificationClient = DeviceCb { sink: sink.clone() }.into();
            enumerator.RegisterEndpointNotificationCallback(&device_cb)?;
            let mut v = Self { enumerator, device_cb, endpoint: None, meter: None, sink };
            v.attach();
            Ok(v)
        }
    }

    /// Listen to the current default output device.
    pub fn attach(&mut self) {
        self.detach();
        let got = unsafe {
            self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole).and_then(|dev| {
                let ep: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None)?;
                let cb: IAudioEndpointVolumeCallback = VolumeCb { sink: self.sink.clone() }.into();
                ep.RegisterControlChangeNotify(&cb)?;
                let meter: Option<IAudioMeterInformation> = dev.Activate(CLSCTX_ALL, None).ok();
                Ok((ep, cb, meter))
            })
        };
        match got {
            Ok((ep, cb, meter)) => {
                self.endpoint = Some((ep, cb));
                self.meter = meter;
            }
            Err(e) => crate::log(&format!("volume: no output device ({e})")),
        }
    }

    fn detach(&mut self) {
        if let Some((ep, cb)) = self.endpoint.take() {
            unsafe {
                let _ = ep.UnregisterControlChangeNotify(&cb);
            }
        }
    }

    /// How loud the sound playing is right now, 0..1 (after the volume).
    pub fn peak(&self) -> f32 {
        self.meter.as_ref().and_then(|m| unsafe { m.GetPeakValue().ok() }).unwrap_or(0.0)
    }

    /// The level now (0..1) and whether it's muted.
    pub fn read(&self) -> Option<(f32, bool)> {
        let (ep, _) = self.endpoint.as_ref()?;
        unsafe { Some((ep.GetMasterVolumeLevelScalar().ok()?, ep.GetMute().ok()?.as_bool())) }
    }
}

impl Drop for Volume {
    fn drop(&mut self) {
        self.detach();
        unsafe {
            let _ = self.enumerator.UnregisterEndpointNotificationCallback(&self.device_cb);
        }
    }
}
