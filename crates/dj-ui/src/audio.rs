//! Choosing and switching the audio output while the app runs.
//!
//! The UI never talks to audio devices directly: the host application hands
//! it an [`AudioBackend`]. Kaldor's backend opens real devices; tests use one
//! built on [`open_null`], so no test touches hardware.

use dj_audio::null_output::NullOutput;
use dj_audio::{AudioOutput, Engine, EngineConfig, EngineHandle, OutputInfo};
use dj_core::{AudioConfig, OutputRouting};

/// One output device the user can pick.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceChoice {
    pub name: String,
    pub is_default: bool,
    /// Output channels (0 if unknown).
    pub channels: u16,
}

/// A running output and the handle of the engine inside it.
pub struct OpenedAudio {
    pub engine: EngineHandle,
    pub info: OutputInfo,
    pub output: Box<dyn AudioOutput>,
}

/// Lists and opens audio outputs.
pub trait AudioBackend {
    /// Host names, for example "WASAPI", "WASAPI-Exclusive", "ASIO".
    fn hosts(&self) -> Vec<String>;
    /// Output devices of `host` (`None` = the platform default host).
    fn devices(&self, host: Option<&str>) -> Result<Vec<DeviceChoice>, String>;
    /// Opens an output for `cfg` with a new engine running in it.
    fn open(&mut self, cfg: &AudioConfig) -> Result<OpenedAudio, String>;
}

/// Starts the null output (runs the engine at real-time pace, no sound).
pub fn open_null(cfg: &AudioConfig) -> Result<OpenedAudio, String> {
    let cfg = cfg.sanitized();
    let (engine, handle) = Engine::new(EngineConfig {
        sample_rate: cfg.sample_rate,
        max_block_frames: 512,
        routing: cfg.routing,
    });
    let channels = cfg.routing.channels_needed().max(2);
    let out = NullOutput::start(engine, channels, cfg.buffer_frames)
        .map_err(|e| format!("cannot start the null output: {e}"))?;
    Ok(OpenedAudio {
        engine: handle,
        info: out.info().clone(),
        output: Box::new(out),
    })
}

/// `wanted` if a device with `channels` outputs can carry it (or the count
/// is unknown), otherwise the stereo master only.
pub fn fit_routing(wanted: OutputRouting, channels: u16) -> OutputRouting {
    if channels == 0 || wanted.channels_needed() <= channels {
        wanted
    } else {
        OutputRouting::laptop()
    }
}

/// What [`open_with_fallback`] ended up with.
pub struct Fallback {
    pub audio: OpenedAudio,
    /// The settings the output actually runs with.
    pub running: AudioConfig,
    /// Set when that is not the requested output: what happened, for the
    /// user.
    pub notice: Option<String>,
}

/// Opens `cfg`. If that fails (device unplugged, in use, too few
/// channels), the platform's default output with the stereo master only;
/// as a last resort the null output, so the app always starts.
pub fn open_with_fallback(
    backend: &mut dyn AudioBackend,
    cfg: &AudioConfig,
) -> Result<Fallback, String> {
    let first = match backend.open(cfg) {
        Ok(audio) => {
            return Ok(Fallback {
                audio,
                running: cfg.clone(),
                notice: None,
            });
        }
        Err(e) => e,
    };
    let wanted = cfg.device.as_deref().unwrap_or("the default output");
    let default = AudioConfig {
        host: None,
        device: None,
        routing: OutputRouting::laptop(),
        ..cfg.clone()
    };
    if default != *cfg {
        match backend.open(&default) {
            Ok(audio) => {
                let notice = format!(
                    "{wanted} is not available ({first}). Playing on {} instead; \
                     choose another output in Setup.",
                    audio.info.device
                );
                return Ok(Fallback {
                    audio,
                    running: default,
                    notice: Some(notice),
                });
            }
            Err(e) => tracing::warn!("default output failed too: {e}"),
        }
    }
    let audio = open_null(cfg)?;
    Ok(Fallback {
        audio,
        running: cfg.clone(),
        notice: Some(format!(
            "No audio output could be opened ({first}). Running without sound; \
             choose an output in Setup."
        )),
    })
}

/// Routing presets offered in Setup.
pub(crate) const ROUTINGS: [(OutputRouting, &str); 3] = [
    (OutputRouting::laptop(), "Master only (stereo, outputs 1-2)"),
    (
        OutputRouting::laptop_split_cue(),
        "Split cue (headphones left, master right)",
    ),
    (OutputRouting::ns4fx(), "Master 1-2, headphones 3-4 (NS4FX)"),
];

/// Name of a routing for the Setup panel.
pub(crate) fn routing_name(r: OutputRouting) -> String {
    // The booth output is a setting of its own.
    let r = OutputRouting { booth: None, ..r };
    ROUTINGS.iter().find(|(p, _)| *p == r).map_or_else(
        || "Custom (from the config file)".to_string(),
        |(_, n)| (*n).to_string(),
    )
}

/// A device list, or why there is none.
pub(crate) type DeviceList = Result<Vec<DeviceChoice>, String>;

/// State of the output picker in Setup.
pub(crate) struct AudioPicker {
    /// Filled from the config the first time Setup is shown.
    pub initialized: bool,
    pub hosts: Vec<String>,
    /// `None` = platform default host.
    pub host: Option<String>,
    /// `None` = default device of the host.
    pub device: Option<String>,
    pub routing: OutputRouting,
    pub buffer_frames: u32,
    /// Requested sample rate in Hz.
    pub sample_rate: u32,
    /// Device list and the host it was listed for.
    pub devices: Option<(Option<String>, DeviceList)>,
    /// Result of the last change, or why the app is on a fallback output.
    pub message: Option<String>,
}

impl AudioPicker {
    pub fn new() -> Self {
        Self {
            initialized: false,
            hosts: Vec::new(),
            host: None,
            device: None,
            routing: OutputRouting::laptop(),
            buffer_frames: 256,
            sample_rate: 48_000,
            devices: None,
            message: None,
        }
    }

    /// Copies the saved settings into the picker.
    pub fn reset_from(&mut self, cfg: &AudioConfig) {
        self.host = cfg.host.clone();
        self.device = cfg.device.clone();
        self.routing = cfg.routing;
        self.buffer_frames = cfg.sanitized().buffer_frames;
        self.sample_rate = cfg.sample_rate;
    }

    /// The settings the picker describes.
    pub fn to_config(&self) -> AudioConfig {
        AudioConfig {
            host: self.host.clone(),
            device: self.device.clone(),
            routing: self.routing,
            buffer_frames: self.buffer_frames,
            sample_rate: self.sample_rate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_falls_back_to_stereo_on_two_channel_devices() {
        assert_eq!(
            fit_routing(OutputRouting::ns4fx(), 2),
            OutputRouting::laptop()
        );
        assert_eq!(
            fit_routing(OutputRouting::ns4fx(), 4),
            OutputRouting::ns4fx()
        );
        assert_eq!(
            fit_routing(OutputRouting::ns4fx(), 0),
            OutputRouting::ns4fx()
        );
        assert_eq!(
            fit_routing(OutputRouting::laptop_split_cue(), 2),
            OutputRouting::laptop_split_cue()
        );
    }
}
