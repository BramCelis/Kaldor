//! WASAPI exclusive-mode output (Windows only), host name `WASAPI-Exclusive`.
//!
//! cpal opens WASAPI devices in shared mode only. There, Windows' audio
//! engine mixes every program and adds its own buffering (33 ms measured on
//! the NS4FX). In exclusive mode Kaldor owns the device: the engine's output
//! goes to the driver in the device's native format with no system mixer in
//! between. Other programs cannot play on that device meanwhile.
//!
//! Timing is polled rather than event-driven: in exclusive event mode,
//! devices on the standard Windows USB audio driver (the NS4FX is one) are
//! known to stutter (see the `wasapi` crate documentation). A dedicated
//! thread with the MMCSS "Pro Audio" priority keeps the device buffer filled
//! to a small target (`buffer_frames`, at least 3.25 device periods) and
//! checks it several times per period.
//!
//! COM objects never cross threads: planning runs on a short-lived probe
//! thread and playback on the audio thread, each with its own COM
//! initialisation, so nothing depends on how the UI thread set up COM.

use crate::device::{DeviceError, OutputInfo, Stats};
use crate::engine::Engine;
use dj_core::AudioConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;
use wasapi::{
    AudioClient, Device, DeviceEnumerator, Direction, SampleType, StreamMode, WasapiError,
    WaveFormat, calculate_period_100ns,
};
use windows::Win32::Media::Audio::{
    AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED, AUDCLNT_E_DEVICE_IN_USE,
    AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED, AUDCLNT_E_UNSUPPORTED_FORMAT,
};

/// Host name used with `--host` and in the config file.
pub const HOST_NAME: &str = crate::device::EXCLUSIVE_HOST;

/// Sample rates tried after the configured one and the device's own.
const FALLBACK_RATES: [u32; 2] = [48_000, 44_100];

/// Sample formats in order of preference.
const KIND_PREFERENCE: [SampleKind; 5] = [
    SampleKind::F32,
    SampleKind::I24In32,
    SampleKind::I32,
    SampleKind::I24Packed,
    SampleKind::I16,
];

/// A device sample format, little-endian.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SampleKind {
    F32,
    I32,
    /// 24 valid bits, left-aligned in a 32-bit container.
    I24In32,
    /// 24 bits in 3 bytes.
    I24Packed,
    I16,
}

impl SampleKind {
    fn store_bits(self) -> usize {
        match self {
            Self::F32 | Self::I32 | Self::I24In32 => 32,
            Self::I24Packed => 24,
            Self::I16 => 16,
        }
    }

    fn valid_bits(self) -> usize {
        match self {
            Self::F32 | Self::I32 => 32,
            Self::I24In32 | Self::I24Packed => 24,
            Self::I16 => 16,
        }
    }

    /// Bytes per sample.
    pub fn bytes(self) -> usize {
        self.store_bits() / 8
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::F32 => "f32",
            Self::I32 => "i32",
            Self::I24In32 => "i24 in 32",
            Self::I24Packed => "i24",
            Self::I16 => "i16",
        }
    }

    fn wave_format(self, rate: u32, channels: u16) -> WaveFormat {
        let sample_type = match self {
            Self::F32 => SampleType::Float,
            _ => SampleType::Int,
        };
        WaveFormat::new(
            self.store_bits(),
            self.valid_bits(),
            sample_type,
            rate as usize,
            usize::from(channels),
            None,
        )
    }
}

/// Converts interleaved `f32` samples into the device's bytes.
///
/// Writes `min(src.len(), dst.len() / kind.bytes())` samples. Values are
/// clamped to [-1, 1]; NaN and infinity become silence. Real-time safe.
pub fn convert(src: &[f32], dst: &mut [u8], kind: SampleKind) {
    for (s, d) in src.iter().zip(dst.chunks_exact_mut(kind.bytes())) {
        let v = if s.is_finite() {
            s.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        match kind {
            SampleKind::F32 => d.copy_from_slice(&v.to_le_bytes()),
            SampleKind::I32 => {
                let x = (f64::from(v) * 2_147_483_647.0).round() as i32;
                d.copy_from_slice(&x.to_le_bytes());
            }
            SampleKind::I24In32 => {
                let x = ((v * 8_388_607.0).round() as i32) << 8;
                d.copy_from_slice(&x.to_le_bytes());
            }
            SampleKind::I24Packed => {
                let x = (v * 8_388_607.0).round() as i32;
                let b = x.to_le_bytes();
                d.copy_from_slice(&[b[0], b[1], b[2]]);
            }
            SampleKind::I16 => {
                let x = (v * 32_767.0).round() as i16;
                d.copy_from_slice(&x.to_le_bytes());
            }
        }
    }
}

/// Frames in a period given in 100 ns units (rounded up).
fn hns_to_frames(hns: i64, rate: u32) -> u32 {
    let frames = (hns.max(0) as u128 * u128::from(rate)).div_ceil(10_000_000);
    u32::try_from(frames).unwrap_or(u32::MAX)
}

/// Target fill of the device buffer: the configured size, but at least three
/// and a quarter device periods. The device takes a period at a time; the
/// audio thread must survive Windows holding it up for a while (measured on
/// an NS4FX: up to 3.4 ms between checks, 1.3 ms for an engine write) plus
/// one more period being taken meanwhile. At 1.25 periods (the old floor,
/// 3.8 ms on the NS4FX) as little as 0.8 ms was left at times, which gave
/// about five clicks in 30 minutes; 3.25 periods is 9.8 ms.
pub fn target_frames(configured: u32, period_frames: u32) -> u32 {
    let floor = period_frames
        .saturating_mul(3)
        .saturating_add(period_frames.div_ceil(4));
    configured.max(floor).max(1)
}

/// Time between two checks of the device buffer: a quarter of the target
/// fill, between 0.25 ms and 2 ms.
pub fn poll_interval(target_frames: u32, rate: u32) -> Duration {
    let target = Duration::from_secs_f64(f64::from(target_frames) / f64::from(rate.max(1)));
    (target / 4).clamp(Duration::from_micros(250), Duration::from_millis(2))
}

/// Did the device run dry? `queued` frames were in its buffer at a moment
/// `elapsed` ago; since then it played `elapsed × rate` frames. More than
/// was queued (with half a device period of slack, as the device takes
/// whole periods) means it ran out before more arrived: a dropout, even if
/// the buffer is filled again by the time it is looked at (heard as
/// clicks while the underrun count stayed 0).
pub fn ran_dry(queued: u32, elapsed: Duration, rate: u32, period_frames: u32) -> bool {
    let played = elapsed.as_secs_f64() * f64::from(rate);
    played > f64::from(queued) + f64::from(period_frames) / 2.0
}

/// Candidate sample rates: configured, device mix rate, then the fallbacks,
/// without duplicates.
fn candidate_rates(configured: u32, mix: u32) -> Vec<u32> {
    let mut rates = Vec::new();
    for r in [configured, mix].into_iter().chain(FALLBACK_RATES) {
        if r > 0 && !rates.contains(&r) {
            rates.push(r);
        }
    }
    rates
}

fn backend(e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Backend(e.to_string())
}

/// Explains the common reasons exclusive mode is refused.
fn init_error(e: WasapiError, device: &str) -> DeviceError {
    if let WasapiError::Windows(w) = &e {
        let code = w.code();
        if code == AUDCLNT_E_DEVICE_IN_USE {
            return DeviceError::Backend(format!(
                "'{device}' is in use by another program. Exclusive mode needs the device to \
                 itself: close Serato, browsers and other audio apps that use it, or make \
                 another device the Windows default"
            ));
        }
        if code == AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED {
            return DeviceError::Backend(format!(
                "Windows does not allow exclusive mode on '{device}'. Turn on Sound settings > \
                 {device} > Properties > Advanced > 'Allow applications to take exclusive \
                 control of this device'"
            ));
        }
        if code == AUDCLNT_E_UNSUPPORTED_FORMAT {
            return DeviceError::Backend(format!("'{device}' rejected the exclusive-mode format"));
        }
    }
    DeviceError::Backend(format!("opening '{device}' in exclusive mode: {e}"))
}

/// Initialises COM (multithreaded) for the current thread and undoes it on drop.
struct ComGuard {
    initialised: bool,
}

impl ComGuard {
    fn mta() -> Self {
        Self {
            initialised: wasapi::initialize_mta().is_ok(),
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.initialised {
            wasapi::deinitialize();
        }
    }
}

/// Runs `f` on a new thread with its own COM apartment and waits for it.
fn on_com_thread<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, DeviceError> + Send + 'static,
) -> Result<T, DeviceError> {
    std::thread::Builder::new()
        .name("kaldor-wasapi-probe".into())
        .spawn(move || {
            let _com = ComGuard::mta();
            f()
        })
        .map_err(backend)?
        .join()
        .map_err(|_| DeviceError::Backend("WASAPI probe thread panicked".into()))?
}

/// Finds an active output device by id or friendly-name substring
/// (case-insensitive); `None` means the Windows default output device.
fn find_device(enumerator: &DeviceEnumerator, wanted: Option<&str>) -> Result<Device, DeviceError> {
    let Some(wanted) = wanted else {
        return enumerator
            .get_default_device(Direction::Render)
            .map_err(|_| DeviceError::NoDefaultDevice);
    };
    let wanted_lc = wanted.to_lowercase();
    let collection = enumerator
        .get_device_collection(Direction::Render)
        .map_err(backend)?;
    for i in 0..collection.get_nbr_devices().map_err(backend)? {
        let device = collection.get_device_at_index(i).map_err(backend)?;
        let id = device.get_id().unwrap_or_default();
        let name = device.get_friendlyname().unwrap_or_default();
        if id == wanted || name.to_lowercase().contains(&wanted_lc) {
            return Ok(device);
        }
    }
    Err(DeviceError::DeviceNotFound(wanted.to_string()))
}

/// Initialises an exclusive polling client. If the driver asks for an
/// aligned buffer, retries once with the size it proposes. Returns the
/// client and the buffer duration that was accepted.
fn open_client(
    device: &Device,
    name: &str,
    format: &WaveFormat,
    period_hns: i64,
    buffer_hns: i64,
) -> Result<(AudioClient, i64), DeviceError> {
    let mut client = device.get_iaudioclient().map_err(backend)?;
    let mode = StreamMode::PollingExclusive {
        buffer_duration_hns: buffer_hns,
        period_hns,
    };
    match client.initialize_client(format, Direction::Render, mode, None) {
        Ok(()) => Ok((client, buffer_hns)),
        Err(WasapiError::Windows(w)) if w.code() == AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED => {
            let frames = client.get_buffer_size().map_err(backend)?;
            let aligned =
                calculate_period_100ns(i64::from(frames), i64::from(format.get_samplespersec()))
                    .max(period_hns);
            let mut client = device.get_iaudioclient().map_err(backend)?;
            let mode = StreamMode::PollingExclusive {
                buffer_duration_hns: aligned,
                period_hns,
            };
            client
                .initialize_client(format, Direction::Render, mode, None)
                .map_err(|e| init_error(e, name))?;
            Ok((client, aligned))
        }
        Err(e) => Err(init_error(e, name)),
    }
}

/// A device and format chosen for exclusive mode, not yet running.
#[derive(Clone, Debug)]
pub(crate) struct ExclusivePlan {
    device_id: String,
    device_name: String,
    format: WaveFormat,
    kind: SampleKind,
    channels: u16,
    rate: u32,
    period_hns: i64,
    buffer_hns: i64,
    target_frames: u32,
}

/// Picks the device, an exclusive-mode format and the buffer sizes, and
/// checks once that the device can actually be opened that way.
pub(crate) fn plan(cfg: &AudioConfig) -> Result<(ExclusivePlan, OutputInfo), DeviceError> {
    let cfg = cfg.clone();
    on_com_thread(move || {
        let enumerator = DeviceEnumerator::new().map_err(backend)?;
        let device = find_device(&enumerator, cfg.device.as_deref())?;
        let name = device
            .get_friendlyname()
            .unwrap_or_else(|_| "Unknown device".into());
        let device_id = device.get_id().map_err(backend)?;
        let client = device.get_iaudioclient().map_err(backend)?;
        let mix = client.get_mixformat().map_err(backend)?;
        let needed = cfg.routing.channels_needed();
        let channels = mix.get_nchannels().max(needed);

        let mut chosen = None;
        'search: for rate in candidate_rates(cfg.sample_rate, mix.get_samplespersec()) {
            for kind in KIND_PREFERENCE {
                if let Ok(f) =
                    client.is_supported_exclusive_with_quirks(&kind.wave_format(rate, channels))
                {
                    chosen = Some((f, kind, rate));
                    break 'search;
                }
            }
        }
        let Some((format, kind, rate)) = chosen else {
            return Err(DeviceError::Backend(format!(
                "'{name}' accepts no exclusive-mode format with {channels} channels \
                 (tried f32, i32, i24 and i16 at {:?} Hz)",
                candidate_rates(cfg.sample_rate, mix.get_samplespersec())
            )));
        };

        let (_default_period, min_period) = client.get_device_period().map_err(backend)?;
        let period_hns = client
            .calculate_aligned_period_near(min_period, None, &format)
            .map_err(backend)?;
        let period_frames = hns_to_frames(period_hns, rate);
        let target = target_frames(cfg.buffer_frames, period_frames);
        // Twice the target, so a late wake-up has room before the buffer runs dry.
        let buffer_hns = calculate_period_100ns(i64::from(target) * 2, i64::from(rate));

        // Trial run: catches "in use", "not allowed" and alignment now,
        // before the engine is built. The client is released on return.
        let (_client, buffer_hns) = open_client(&device, &name, &format, period_hns, buffer_hns)?;

        let info = OutputInfo {
            host: HOST_NAME.to_string(),
            device: name.clone(),
            sample_rate: rate,
            channels,
            buffer_frames: Some(target),
            sample_format: kind.name().to_string(),
        };
        let plan = ExclusivePlan {
            device_id,
            device_name: name,
            format,
            kind,
            channels,
            rate,
            period_hns,
            buffer_hns,
            target_frames: target,
        };
        Ok((plan, info))
    })
}

/// The running audio thread. Dropping it stops playback and joins the thread.
pub(crate) struct Runner {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Opens the device and starts the audio thread with `engine`.
pub(crate) fn start(
    plan: ExclusivePlan,
    engine: Engine,
    stats: Arc<Stats>,
) -> Result<Runner, DeviceError> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
    let thread_stop = Arc::clone(&stop);
    let thread = std::thread::Builder::new()
        .name("kaldor-audio".into())
        .spawn(move || run(plan, engine, &stats, &thread_stop, &ready_tx))
        .map_err(backend)?;
    let mut runner = Runner {
        stop,
        thread: Some(thread),
    };
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(runner),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            runner.thread.take().map(JoinHandle::join);
            Err(DeviceError::Backend(
                "the audio thread ended while starting".into(),
            ))
        }
    }
}

/// Raises the thread to the MMCSS "Pro Audio" class, and back on drop.
struct Mmcss(windows::Win32::Foundation::HANDLE);

impl Mmcss {
    fn pro_audio() -> Option<Self> {
        let mut task_index = 0u32;
        // SAFETY: the task name is a static NUL-terminated wide string and
        // `task_index` outlives the call. The returned handle is released in
        // `Drop` on the same thread.
        let handle = unsafe {
            windows::Win32::System::Threading::AvSetMmThreadCharacteristicsW(
                windows::core::w!("Pro Audio"),
                &mut task_index,
            )
        };
        handle.ok().map(Self)
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        // SAFETY: the handle came from AvSetMmThreadCharacteristicsW on this thread.
        let _ =
            unsafe { windows::Win32::System::Threading::AvRevertMmThreadCharacteristics(self.0) };
    }
}

/// The audio thread: opens the device, prefills, then keeps the buffer at
/// the target until `stop` is set or the device fails.
fn run(
    plan: ExclusivePlan,
    mut engine: Engine,
    stats: &Stats,
    stop: &AtomicBool,
    ready: &std::sync::mpsc::SyncSender<Result<(), DeviceError>>,
) {
    let _com = ComGuard::mta();
    let setup = || -> Result<_, DeviceError> {
        let enumerator = DeviceEnumerator::new().map_err(backend)?;
        let device = enumerator.get_device(&plan.device_id).map_err(backend)?;
        let (client, _) = open_client(
            &device,
            &plan.device_name,
            &plan.format,
            plan.period_hns,
            plan.buffer_hns,
        )?;
        let render = client.get_audiorenderclient().map_err(backend)?;
        let buffer_frames = client.get_buffer_size().map_err(backend)?;
        // Driver-side delay after the buffer (hardware and USB), 100 ns units.
        let stream_latency_hns = client.get_stream_latency().unwrap_or(0).max(0);
        Ok((client, render, buffer_frames, stream_latency_hns))
    };
    let (client, render, buffer_frames, stream_latency_hns) = match setup() {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    let channels = usize::from(plan.channels);
    let frame_bytes = channels * plan.kind.bytes();
    let target = plan.target_frames.min(buffer_frames);
    // Everything the loop touches is allocated here, before playback starts.
    let mut scratch = vec![0.0f32; buffer_frames as usize * channels];
    let mut bytes = vec![0u8; buffer_frames as usize * frame_bytes];
    let interval = poll_interval(target, plan.rate);
    let stream_latency_us = u64::try_from(stream_latency_hns / 10).unwrap_or(0);
    let _mmcss = Mmcss::pro_audio();

    let mut write = |frames: u32, engine: &mut Engine| -> bool {
        let samples = frames as usize * channels;
        let n_bytes = frames as usize * frame_bytes;
        let (Some(s), Some(b)) = (scratch.get_mut(..samples), bytes.get_mut(..n_bytes)) else {
            return false;
        };
        // In debug builds, abort if the engine or the conversion allocates.
        #[cfg(debug_assertions)]
        assert_no_alloc::assert_no_alloc(|| {
            engine.process(s, channels);
            convert(s, b, plan.kind);
        });
        #[cfg(not(debug_assertions))]
        {
            engine.process(s, channels);
            convert(s, b, plan.kind);
        }
        render.write_to_device(b, None).is_ok()
    };

    if !write(target, &mut engine) || client.start_stream().is_err() {
        let _ = ready.send(Err(DeviceError::Backend(format!(
            "could not start exclusive playback on '{}'",
            plan.device_name
        ))));
        return;
    }
    let _ = ready.send(Ok(()));

    let rate = plan.rate.max(1);
    let period_frames = hns_to_frames(plan.period_hns, rate);
    let us = |frames: u32| {
        u32::try_from(u64::from(frames) * 1_000_000 / u64::from(rate)).unwrap_or(u32::MAX)
    };
    let micros = |d: Duration| u32::try_from(d.as_micros()).unwrap_or(u32::MAX);
    stats.timed.store(true, Ordering::Relaxed);
    stats.queued_min_us.store(u32::MAX, Ordering::Relaxed);
    // The last check: when, and how many frames were queued from then on.
    let mut last: (std::time::Instant, u32) = (std::time::Instant::now(), target);

    while !stop.load(Ordering::Relaxed) {
        let now = std::time::Instant::now();
        let Ok(padding) = client.get_current_padding() else {
            stats.failed.store(true, Ordering::Relaxed);
            break;
        };
        // A dropout: the buffer is empty, or more time passed since the
        // last check than what was queued then lasts (the thread was kept
        // from running).
        let gap = now.duration_since(last.0);
        let mut dry = padding == 0 || ran_dry(last.1, gap, rate, period_frames);
        stats.gap_max_us.fetch_max(micros(gap), Ordering::Relaxed);
        stats
            .queued_min_us
            .fetch_min(us(padding), Ordering::Relaxed);
        let mut queued = padding;
        if padding < target {
            if !write(target - padding, &mut engine) {
                stats.failed.store(true, Ordering::Relaxed);
                break;
            }
            let took = now.elapsed();
            stats
                .process_max_us
                .fetch_max(micros(took), Ordering::Relaxed);
            // The new audio arrived `took` after the check: what was queued
            // then must have lasted that long (the engine was too slow).
            dry |= ran_dry(padding, took, rate, period_frames);
            queued = target;
        }
        if dry {
            stats.xruns.fetch_add(1, Ordering::Relaxed);
        }
        last = (now, queued);
        let queued = u64::from(padding.max(target));
        let latency_us = queued * 1_000_000 / u64::from(plan.rate.max(1)) + stream_latency_us;
        stats.latency_us.store(
            u32::try_from(latency_us).unwrap_or(u32::MAX),
            Ordering::Relaxed,
        );
        std::thread::sleep(interval);
    }
    let _ = client.stop_stream();
}

/// What one output device offers in exclusive mode, for `--list-devices`.
#[derive(Clone, Debug)]
pub struct ExclusiveDeviceReport {
    pub name: String,
    pub id: String,
    pub is_default: bool,
    /// Channel count of Windows' shared-mode format (0 if unknown).
    pub channels: u16,
    /// Windows' shared-mode format: channels, rate, bits.
    pub mix_format: String,
    /// Default and minimum device period in milliseconds.
    pub periods_ms: Option<(f64, f64)>,
    /// Accepted exclusive formats, e.g. "44100 Hz i16".
    pub formats: Vec<String>,
}

/// Lists every active output device with the formats it accepts in
/// exclusive mode. Opens no stream.
pub fn report() -> Result<Vec<ExclusiveDeviceReport>, DeviceError> {
    on_com_thread(|| {
        let enumerator = DeviceEnumerator::new().map_err(backend)?;
        let default_id = enumerator
            .get_default_device(Direction::Render)
            .and_then(|d| d.get_id())
            .ok();
        let collection = enumerator
            .get_device_collection(Direction::Render)
            .map_err(backend)?;
        let mut out = Vec::new();
        for i in 0..collection.get_nbr_devices().map_err(backend)? {
            let device = collection.get_device_at_index(i).map_err(backend)?;
            let id = device.get_id().unwrap_or_default();
            let mut r = ExclusiveDeviceReport {
                name: device.get_friendlyname().unwrap_or_default(),
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
                channels: 0,
                mix_format: String::new(),
                periods_ms: None,
                formats: Vec::new(),
            };
            if let Ok(client) = device.get_iaudioclient() {
                if let Ok(mix) = client.get_mixformat() {
                    r.channels = mix.get_nchannels();
                    r.mix_format = format!(
                        "{} ch, {} Hz, {} bit",
                        mix.get_nchannels(),
                        mix.get_samplespersec(),
                        mix.get_validbitspersample()
                    );
                    for rate in [44_100, 48_000, 88_200, 96_000] {
                        for kind in KIND_PREFERENCE {
                            let f = kind.wave_format(rate, mix.get_nchannels());
                            if client.is_supported_exclusive_with_quirks(&f).is_ok() {
                                r.formats.push(format!("{rate} Hz {}", kind.name()));
                            }
                        }
                    }
                }
                if let Ok((def, min)) = client.get_device_period() {
                    r.periods_ms = Some((def as f64 / 10_000.0, min as f64 / 10_000.0));
                }
            }
            out.push(r);
        }
        Ok(out)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i16s(bytes: &[u8]) -> Vec<i16> {
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i16::from_le_bytes(*c))
            .collect()
    }

    #[test]
    fn convert_i16() {
        let mut out = [0u8; 8];
        convert(&[0.0, 1.0, -1.0, 0.5], &mut out, SampleKind::I16);
        let v = i16s(&out);
        assert_eq!(v, [0, 32_767, -32_767, 16_384]);
    }

    #[test]
    fn convert_i24_packed_and_in_32() {
        let mut packed = [0u8; 6];
        convert(&[1.0, -1.0], &mut packed, SampleKind::I24Packed);
        assert_eq!(&packed[..3], &[0xff, 0xff, 0x7f]);
        assert_eq!(&packed[3..], &[0x01, 0x00, 0x80]);

        let mut in32 = [0u8; 4];
        convert(&[1.0], &mut in32, SampleKind::I24In32);
        // Left-aligned: the low byte is zero, the top three hold the sample.
        assert_eq!(i32::from_le_bytes(in32), 8_388_607 << 8);
    }

    #[test]
    fn convert_i32_and_f32() {
        let mut out = [0u8; 8];
        convert(&[1.0, -0.25], &mut out, SampleKind::I32);
        assert_eq!(
            i32::from_le_bytes([out[0], out[1], out[2], out[3]]),
            i32::MAX
        );
        assert_eq!(
            i32::from_le_bytes([out[4], out[5], out[6], out[7]]),
            -536_870_912
        );

        let mut f = [0u8; 4];
        convert(&[0.75], &mut f, SampleKind::F32);
        assert_eq!(f32::from_le_bytes(f), 0.75);
    }

    #[test]
    fn convert_clamps_and_silences_bad_values() {
        let mut out = [0u8; 8];
        convert(
            &[2.0, -3.0, f32::NAN, f32::INFINITY],
            &mut out,
            SampleKind::I16,
        );
        let v = i16s(&out);
        assert_eq!(v, [32_767, -32_767, 0, 0]);
    }

    #[test]
    fn convert_stops_at_the_shorter_buffer() {
        let mut out = [0xAAu8; 6];
        convert(&[0.0, 0.0, 0.0, 0.0], &mut out, SampleKind::I16);
        assert_eq!(out, [0; 6]);
        let mut out = [0xAAu8; 6];
        convert(&[0.0], &mut out, SampleKind::I16);
        assert_eq!(out, [0, 0, 0xAA, 0xAA, 0xAA, 0xAA]);
    }

    #[test]
    fn sizes_and_timing() {
        // 3 ms at 44.1 kHz is 132.3 frames: rounded up.
        assert_eq!(hns_to_frames(30_000, 44_100), 133);
        assert_eq!(hns_to_frames(-5, 48_000), 0);
        assert_eq!(target_frames(256, 48), 256);
        assert_eq!(target_frames(64, 144), 468);
        // NS4FX: 3 ms period at 44.1 kHz is 133 frames; floor 3.25 periods,
        // 433 frames (9.8 ms). A bigger configured buffer wins.
        assert_eq!(target_frames(64, 133), 433);
        assert_eq!(target_frames(256, 133), 433);
        assert_eq!(target_frames(512, 133), 512);
        assert_eq!(target_frames(0, 0), 1);
        assert_eq!(poll_interval(256, 48_000).as_micros(), 1333);
        assert_eq!(poll_interval(16, 48_000), Duration::from_micros(250));
        assert_eq!(poll_interval(1024, 44_100), Duration::from_millis(2));
    }

    #[test]
    fn a_dropout_is_seen_even_when_the_buffer_is_full_again() {
        // NS4FX: 44.1 kHz, 133-frame period, 266 frames kept queued (6 ms).
        let ms = |m: u64| Duration::from_micros(m * 1000);
        // On time: 266 frames last 6 ms; 1.5 ms between checks is fine.
        assert!(!ran_dry(266, ms(1) + ms(1) / 2, 44_100, 133));
        // The thread kept from running for 8 ms: the 266 frames were gone
        // 2 ms before it looked (more than half a period of slack).
        assert!(ran_dry(266, ms(8), 44_100, 133));
        // The engine took 3 ms while 60 frames (1.4 ms) were queued.
        assert!(ran_dry(60, ms(3), 44_100, 133));
        // Just inside the slack: not counted.
        assert!(!ran_dry(266, Duration::from_micros(7_500), 44_100, 133));
    }

    #[test]
    fn rates_are_unique_and_ordered() {
        assert_eq!(candidate_rates(48_000, 44_100), [48_000, 44_100]);
        assert_eq!(candidate_rates(96_000, 44_100), [96_000, 44_100, 48_000]);
        assert_eq!(candidate_rates(44_100, 0), [44_100, 48_000]);
    }

    #[test]
    fn wave_formats_match_the_kinds() {
        let f = SampleKind::I24In32.wave_format(44_100, 4);
        assert_eq!(f.get_bitspersample(), 32);
        assert_eq!(f.get_validbitspersample(), 24);
        assert_eq!(f.get_blockalign(), 16);
        let f = SampleKind::I16.wave_format(48_000, 2);
        assert_eq!(f.get_blockalign(), 4);
        assert_eq!(f.get_samplespersec(), 48_000);
    }

    /// Needs real hardware: lists what each device accepts in exclusive mode.
    #[test]
    #[ignore = "queries real audio devices"]
    fn list_exclusive_capabilities() {
        for d in report().unwrap() {
            println!(
                "{} [{}] {:?} {:?}",
                d.name, d.mix_format, d.periods_ms, d.formats
            );
        }
    }
}
