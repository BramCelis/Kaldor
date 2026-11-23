//! The UI's audio backend on real devices (cpal hosts and, on Windows, the
//! WASAPI exclusive-mode backend).

use dj_audio::AudioOutput;
use dj_core::AudioConfig;
use dj_ui::audio::{AudioBackend, DeviceChoice, OpenedAudio};

pub struct RealAudio;

impl AudioBackend for RealAudio {
    fn hosts(&self) -> Vec<String> {
        dj_audio::device::host_names()
    }

    fn devices(&self, host: Option<&str>) -> Result<Vec<DeviceChoice>, String> {
        let list = dj_audio::device::list_output_devices(host).map_err(|e| e.to_string())?;
        Ok(list
            .into_iter()
            .map(|d| DeviceChoice {
                name: d.name,
                is_default: d.is_default,
                channels: d.max_output_channels,
            })
            .collect())
    }

    fn open(&mut self, cfg: &AudioConfig) -> Result<OpenedAudio, String> {
        let (engine, out) = dj_audio::device::open_output(cfg).map_err(|e| e.to_string())?;
        Ok(OpenedAudio {
            engine,
            info: out.info().clone(),
            output: Box::new(out),
        })
    }
}
