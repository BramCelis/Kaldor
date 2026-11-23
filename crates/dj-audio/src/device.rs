//! Real audio devices through `cpal`.
//!
//! Opening a device is done in two steps so the engine can be built at the
//! device's actual sample rate: [`plan_output`] picks the device and stream
//! configuration, then [`OutputPlan::start`] moves the engine into the
//! device callback.
//!
//! Tests never call into this module except through `#[ignore]` tests: they
//! use the offline renderer and [`crate::null_output::NullOutput`] instead.

use crate::engine::Engine;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use dj_core::AudioConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Duration;

/// Describes a running output.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputInfo {
    pub host: String,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
    /// Requested fixed buffer size, if the device accepted one.
    pub buffer_frames: Option<u32>,
    pub sample_format: String,
}

/// Anything the engine can run in: a real device or the null output.
pub trait AudioOutput {
    fn info(&self) -> &OutputInfo;
    /// Number of buffer underruns reported by the driver.
    fn xruns(&self) -> u64;
    /// Where the time went, for finding the cause of dropouts (outputs that
    /// measure it: WASAPI exclusive).
    fn timing(&self) -> Option<OutputTiming> {
        None
    }
    /// Starts the worst cases of [`Self::timing`] over (Reset in Setup).
    fn reset_timing(&self) {}
    /// Output latency: the time between the callback and the moment its
    /// first sample reaches the DAC, as reported by the driver.
    fn output_latency(&self) -> Option<Duration>;
    fn is_running(&self) -> bool;
}

/// Worst cases since the output started, in microseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OutputTiming {
    /// The longest the engine took to render one write.
    pub process_max_us: u32,
    /// The longest time between two checks of the device buffer (the
    /// thread asleep, or kept from running by Windows).
    pub gap_max_us: u32,
    /// How much audio was queued in the device at the tightest moment.
    pub queued_min_us: u32,
}

/// Errors from device discovery and stream setup.
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("audio host '{0}' is not available")]
    HostNotFound(String),
    #[error("no output device matches '{0}'")]
    DeviceNotFound(String),
    #[error("there is no default output device")]
    NoDefaultDevice,
    #[error(
        "device '{device}' has no configuration with {channels} or more output channels at {sample_rate} Hz"
    )]
    NoConfig {
        device: String,
        channels: u16,
        sample_rate: u32,
    },
    #[error("unsupported sample format {0}")]
    UnsupportedFormat(String),
    #[error("audio backend error: {0}")]
    Backend(String),
}

fn backend(e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Backend(e.to_string())
}

/// One output device as listed in settings.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub host: String,
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub max_output_channels: u16,
    pub min_sample_rate: u32,
    pub max_sample_rate: u32,
}

/// Names of the audio hosts available on this machine.
pub fn host_names() -> Vec<String> {
    #[allow(unused_mut)]
    let mut names: Vec<String> = cpal::available_hosts()
        .into_iter()
        .map(|h| h.name().to_string())
        .collect();
    #[cfg(windows)]
    names.push(EXCLUSIVE_HOST.to_string());
    names
}

fn host_by_name(name: Option<&str>) -> Result<cpal::Host, DeviceError> {
    match name {
        None => Ok(cpal::default_host()),
        Some(n) => {
            let id = cpal::available_hosts()
                .into_iter()
                .find(|h| h.name().eq_ignore_ascii_case(n))
                .ok_or_else(|| DeviceError::HostNotFound(n.to_string()))?;
            cpal::host_from_id(id).map_err(backend)
        }
    }
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "Unknown device".to_string())
}

fn device_id(device: &cpal::Device) -> String {
    device.id().map(|i| i.to_string()).unwrap_or_default()
}

/// Lists output devices of a host (the default host when `host` is `None`).
///
/// This queries the operating system's device list and each device's
/// supported formats. It does not open a stream.
pub fn list_output_devices(host: Option<&str>) -> Result<Vec<DeviceInfo>, DeviceError> {
    if is_exclusive_host(host) {
        return list_exclusive();
    }
    let host = host_by_name(host)?;
    let default_id = host.default_output_device().map(|d| device_id(&d));
    let mut out = Vec::new();
    for device in host.output_devices().map_err(backend)? {
        let mut max_ch = 0u16;
        let mut min_sr = u32::MAX;
        let mut max_sr = 0u32;
        if let Ok(configs) = device.supported_output_configs() {
            for c in configs {
                max_ch = max_ch.max(c.channels());
                min_sr = min_sr.min(c.min_sample_rate());
                max_sr = max_sr.max(c.max_sample_rate());
            }
        }
        let id = device_id(&device);
        out.push(DeviceInfo {
            host: host.id().name().to_string(),
            is_default: default_id.as_deref() == Some(id.as_str()),
            name: device_name(&device),
            id,
            max_output_channels: max_ch,
            min_sample_rate: if min_sr == u32::MAX { 0 } else { min_sr },
            max_sample_rate: max_sr,
        });
    }
    Ok(out)
}

/// Output devices as the WASAPI exclusive backend sees them. The sample
/// rate range is left at 0: exclusive formats are probed when opening.
#[cfg(windows)]
fn list_exclusive() -> Result<Vec<DeviceInfo>, DeviceError> {
    Ok(crate::wasapi_exclusive::report()?
        .into_iter()
        .map(|d| DeviceInfo {
            host: EXCLUSIVE_HOST.to_string(),
            id: d.id,
            name: d.name,
            is_default: d.is_default,
            max_output_channels: d.channels,
            min_sample_rate: 0,
            max_sample_rate: 0,
        })
        .collect())
}

#[cfg(not(windows))]
fn list_exclusive() -> Result<Vec<DeviceInfo>, DeviceError> {
    Err(DeviceError::HostNotFound(EXCLUSIVE_HOST.to_string()))
}

/// Opens an output for `cfg` and starts a new engine in it, built at the
/// rate the device runs at.
pub fn open_output(
    cfg: &AudioConfig,
) -> Result<(crate::engine::EngineHandle, DeviceOutput), DeviceError> {
    let plan = plan_output(cfg)?;
    let (engine, handle) = Engine::new(crate::engine::EngineConfig {
        sample_rate: plan.info().sample_rate,
        max_block_frames: 512,
        routing: cfg.routing,
    });
    Ok((handle, plan.start(engine)?))
}

/// One registered ASIO driver and whether it could be loaded.
#[derive(Clone, Debug)]
pub struct AsioDriverStatus {
    pub name: String,
    /// Output and input channel counts, or why the driver did not load.
    pub result: Result<(i32, i32), String>,
}

/// Every ASIO driver registered on this machine, loaded one at a time.
///
/// cpal's device list silently skips drivers that fail to load; this shows
/// them with the reason. Returns `None` when Kaldor was built without the
/// `asio` feature (or not on Windows). Do not call while an ASIO stream is
/// open: ASIO allows only one loaded driver at a time.
pub fn asio_driver_report() -> Option<Vec<AsioDriverStatus>> {
    #[cfg(all(windows, feature = "asio"))]
    {
        let asio = asio_sys::Asio::new();
        let report = asio
            .driver_names()
            .into_iter()
            .map(|name| {
                let result = match asio.load_driver(&name) {
                    Ok(driver) => driver
                        .channels()
                        .map(|c| (c.outs, c.ins))
                        .map_err(|e| format!("loaded, but channel query failed: {e}")),
                    Err(e) => Err(e.to_string()),
                };
                AsioDriverStatus { name, result }
            })
            .collect();
        Some(report)
    }
    #[cfg(not(all(windows, feature = "asio")))]
    None
}

/// A chosen device and stream configuration, not yet running.
pub struct OutputPlan {
    kind: PlanKind,
    info: OutputInfo,
}

enum PlanKind {
    Cpal {
        device: cpal::Device,
        config: cpal::StreamConfig,
        format: cpal::SampleFormat,
    },
    #[cfg(windows)]
    Exclusive(crate::wasapi_exclusive::ExclusivePlan),
}

/// True when `host` names the WASAPI exclusive-mode backend.
fn is_exclusive_host(host: Option<&str>) -> bool {
    host.is_some_and(|h| h.eq_ignore_ascii_case(EXCLUSIVE_HOST))
}

/// Host name of Kaldor's own WASAPI exclusive-mode backend (Windows only).
pub const EXCLUSIVE_HOST: &str = "WASAPI-Exclusive";

impl std::fmt::Debug for OutputPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputPlan")
            .field("info", &self.info)
            .finish()
    }
}

/// Order of preference for sample formats.
fn format_rank(f: cpal::SampleFormat) -> Option<u8> {
    match f {
        cpal::SampleFormat::F32 => Some(0),
        cpal::SampleFormat::I32 => Some(1),
        cpal::SampleFormat::I16 => Some(2),
        cpal::SampleFormat::U16 => Some(3),
        _ => None,
    }
}

/// Picks the output device and a stream configuration for `cfg`.
pub fn plan_output(cfg: &AudioConfig) -> Result<OutputPlan, DeviceError> {
    let cfg = cfg.sanitized();
    if is_exclusive_host(cfg.host.as_deref()) {
        #[cfg(windows)]
        {
            let (plan, info) = crate::wasapi_exclusive::plan(&cfg)?;
            return Ok(OutputPlan {
                kind: PlanKind::Exclusive(plan),
                info,
            });
        }
        #[cfg(not(windows))]
        return Err(DeviceError::HostNotFound(EXCLUSIVE_HOST.to_string()));
    }
    let host = host_by_name(cfg.host.as_deref())?;
    let device = match cfg.device.as_deref() {
        None => host
            .default_output_device()
            .ok_or(DeviceError::NoDefaultDevice)?,
        Some(wanted) => {
            let wanted_lc = wanted.to_lowercase();
            host.output_devices()
                .map_err(backend)?
                .find(|d| {
                    device_id(d) == wanted || device_name(d).to_lowercase().contains(&wanted_lc)
                })
                .ok_or_else(|| DeviceError::DeviceNotFound(wanted.to_string()))?
        }
    };
    let name = device_name(&device);
    let needed = cfg.routing.channels_needed();

    let ranges: Vec<cpal::SupportedStreamConfigRange> = device
        .supported_output_configs()
        .map_err(backend)?
        .filter(|c| c.channels() >= needed && format_rank(c.sample_format()).is_some())
        .collect();

    let pick = |rate: u32| {
        ranges
            .iter()
            .filter(|c| c.contains_rate(rate))
            .min_by_key(|c| (format_rank(c.sample_format()), c.channels()))
            .and_then(|c| c.try_with_sample_rate(rate))
    };
    let fallback_rate = device
        .default_output_config()
        .map(|c| c.sample_rate())
        .unwrap_or(cfg.sample_rate);
    let chosen = pick(cfg.sample_rate)
        .or_else(|| pick(fallback_rate))
        .ok_or_else(|| DeviceError::NoConfig {
            device: name.clone(),
            channels: needed,
            sample_rate: cfg.sample_rate,
        })?;

    let mut config = chosen.config();
    let buffer_frames = match chosen.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max }
            if (*min..=*max).contains(&cfg.buffer_frames) =>
        {
            config.buffer_size = cpal::BufferSize::Fixed(cfg.buffer_frames);
            Some(cfg.buffer_frames)
        }
        _ => {
            config.buffer_size = cpal::BufferSize::Default;
            None
        }
    };
    let info = OutputInfo {
        host: host.id().name().to_string(),
        device: name,
        sample_rate: config.sample_rate,
        channels: config.channels,
        buffer_frames,
        sample_format: format!("{}", chosen.sample_format()),
    };
    Ok(OutputPlan {
        kind: PlanKind::Cpal {
            device,
            config,
            format: chosen.sample_format(),
        },
        info,
    })
}

#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) xruns: AtomicU64,
    pub(crate) latency_us: AtomicU32,
    pub(crate) failed: AtomicBool,
    /// See [`OutputTiming`]; `timed` is set by outputs that fill them.
    pub(crate) timed: AtomicBool,
    pub(crate) process_max_us: AtomicU32,
    pub(crate) gap_max_us: AtomicU32,
    pub(crate) queued_min_us: AtomicU32,
}

/// A running device output. Dropping it stops the stream.
pub struct DeviceOutput {
    _running: Running,
    info: OutputInfo,
    stats: Arc<Stats>,
}

/// Whatever keeps the audio running; dropping it stops playback.
enum Running {
    Cpal(#[allow(dead_code)] cpal::Stream),
    #[cfg(windows)]
    Exclusive(#[allow(dead_code)] crate::wasapi_exclusive::Runner),
}

impl OutputPlan {
    pub fn info(&self) -> &OutputInfo {
        &self.info
    }

    /// Opens the stream and starts the engine in its callback.
    ///
    /// If the device rejects the requested fixed buffer size (ASIO drivers
    /// can accept only sizes on a step from their minimum), the stream is
    /// opened again with the driver's own preferred size.
    pub fn start(self, engine: Engine) -> Result<DeviceOutput, DeviceError> {
        let OutputPlan { kind, mut info } = self;
        let stats = Arc::new(Stats::default());
        let (device, mut config, format) = match kind {
            PlanKind::Cpal {
                device,
                config,
                format,
            } => (device, config, format),
            #[cfg(windows)]
            PlanKind::Exclusive(plan) => {
                let runner = crate::wasapi_exclusive::start(plan, engine, Arc::clone(&stats))?;
                return Ok(DeviceOutput {
                    _running: Running::Exclusive(runner),
                    info,
                    stats,
                });
            }
        };
        // The engine moves into the callback, so it can be offered only once.
        let mut engine = Some(engine);
        let stream = match open_stream(&device, &config, format, &mut engine, &stats) {
            Err(e) if matches!(config.buffer_size, cpal::BufferSize::Fixed(_)) => {
                tracing::warn!(
                    "device rejected buffer size {:?} ({e}); using the driver's preferred size",
                    info.buffer_frames
                );
                config.buffer_size = cpal::BufferSize::Default;
                info.buffer_frames = None;
                open_stream(&device, &config, format, &mut engine, &stats)?
            }
            other => other?,
        };
        stream.play().map_err(backend)?;
        Ok(DeviceOutput {
            _running: Running::Cpal(stream),
            info,
            stats,
        })
    }
}

/// Builds the stream for `format`. The engine is handed to the callback
/// through a one-slot queue only after the stream was built, so a failed
/// attempt leaves it in `engine` for a retry.
fn open_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    engine: &mut Option<Engine>,
    stats: &Arc<Stats>,
) -> Result<cpal::Stream, DeviceError> {
    let (mut tx, rx) = rtrb::RingBuffer::<Box<Engine>>::new(1);
    let stream = match format {
        cpal::SampleFormat::F32 => build_stream::<f32>(device, config, rx, stats),
        cpal::SampleFormat::I32 => build_stream::<i32>(device, config, rx, stats),
        cpal::SampleFormat::I16 => build_stream::<i16>(device, config, rx, stats),
        cpal::SampleFormat::U16 => build_stream::<u16>(device, config, rx, stats),
        other => Err(DeviceError::UnsupportedFormat(other.to_string())),
    }?;
    if let Some(e) = engine.take()
        && tx.push(Box::new(e)).is_err()
    {
        // Unreachable: the slot is new and empty. The engine is dropped here,
        // on this thread, and the stream plays silence.
        tracing::error!("could not hand the engine to the audio callback");
    }
    Ok(stream)
}

/// Largest number of frames converted per engine call inside the callback.
const CONVERT_FRAMES: usize = 2048;

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut engine_rx: rtrb::Consumer<Box<Engine>>,
    stats: &Arc<Stats>,
) -> Result<cpal::Stream, DeviceError>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let mut scratch = vec![0.0f32; CONVERT_FRAMES * channels];
    let data_stats = Arc::clone(stats);
    let err_stats = Arc::clone(stats);
    // Filled from `engine_rx` on the first callback after the stream is
    // built. Moving the box is a pointer copy; it is never dropped in here.
    let mut engine: Option<Box<Engine>> = None;

    let data_cb = move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
        let ts = info.timestamp();
        if let Some(d) = ts.playback.checked_duration_since(ts.callback) {
            let us = u32::try_from(d.as_micros()).unwrap_or(u32::MAX);
            data_stats.latency_us.store(us, Ordering::Relaxed);
        }
        if engine.is_none() {
            engine = engine_rx.pop().ok();
        }
        let Some(engine) = engine.as_deref_mut() else {
            data.fill(T::EQUILIBRIUM);
            return;
        };
        // In debug builds, abort if anything in the callback allocates
        // (only effective when the binary installs `AllocDisabler`).
        #[cfg(debug_assertions)]
        assert_no_alloc::assert_no_alloc(|| render_converted(engine, &mut scratch, data, channels));
        #[cfg(not(debug_assertions))]
        render_converted(engine, &mut scratch, data, channels);
    };
    let err_cb = move |err: cpal::Error| match err.kind() {
        cpal::ErrorKind::Xrun => {
            err_stats.xruns.fetch_add(1, Ordering::Relaxed);
        }
        cpal::ErrorKind::DeviceNotAvailable | cpal::ErrorKind::StreamInvalidated => {
            err_stats.failed.store(true, Ordering::Relaxed);
        }
        _ => {}
    };
    device
        .build_output_stream::<T, _, _>(*config, data_cb, err_cb, None)
        .map_err(backend)
}

/// Renders the engine into `scratch` and converts into the device format.
#[inline]
fn render_converted<T>(engine: &mut Engine, scratch: &mut [f32], data: &mut [T], channels: usize)
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    for chunk in data.chunks_mut(scratch.len().max(1)) {
        if let Some(s) = scratch.get_mut(..chunk.len()) {
            engine.process(s, channels);
            for (d, v) in chunk.iter_mut().zip(s.iter()) {
                *d = T::from_sample(*v);
            }
        }
    }
}

impl AudioOutput for DeviceOutput {
    fn info(&self) -> &OutputInfo {
        &self.info
    }

    fn xruns(&self) -> u64 {
        self.stats.xruns.load(Ordering::Relaxed)
    }

    fn timing(&self) -> Option<OutputTiming> {
        let s = &self.stats;
        s.timed.load(Ordering::Relaxed).then(|| OutputTiming {
            process_max_us: s.process_max_us.load(Ordering::Relaxed),
            gap_max_us: s.gap_max_us.load(Ordering::Relaxed),
            queued_min_us: s.queued_min_us.load(Ordering::Relaxed),
        })
    }

    fn reset_timing(&self) {
        let s = &self.stats;
        s.process_max_us.store(0, Ordering::Relaxed);
        s.gap_max_us.store(0, Ordering::Relaxed);
        s.queued_min_us.store(u32::MAX, Ordering::Relaxed);
    }

    fn output_latency(&self) -> Option<Duration> {
        match self.stats.latency_us.load(Ordering::Relaxed) {
            0 => None,
            us => Some(Duration::from_micros(u64::from(us))),
        }
    }

    fn is_running(&self) -> bool {
        !self.stats.failed.load(Ordering::Relaxed)
    }
}
