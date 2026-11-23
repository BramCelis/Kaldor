//! Sampler: 32 slots in 4 banks of 8, mixed into the master bus.
//!
//! Each slot holds a decoded sample and plays it with one voice. Modes:
//! one-shot (plays to the end, a new press restarts it), hold (plays while
//! held), toggle (press starts, press again stops) and loop (like toggle,
//! looping). A slot with sync on and a known BPM plays at the master tempo
//! (varispeed, so its pitch follows). Recording copies the master output
//! into a buffer the UI handed over; the full buffer goes back to the UI,
//! which turns it into a sample.
//!
//! Real-time: nothing here allocates or frees. Replaced samples and finished
//! recordings go back to the UI thread through the engine's queues.

use dj_decode::TrackAudio;
use dj_dsp::LinearSmoother;
use dj_dsp::interp::hermite;
use std::sync::Arc;

/// Number of sampler slots.
pub const SAMPLER_SLOTS: usize = 32;
/// Slots per bank (one bank is shown on the pads).
pub const SAMPLER_BANK: usize = 8;

/// How a slot reacts to its pad.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SampleMode {
    /// Plays to the end; pressing again restarts it.
    #[default]
    OneShot,
    /// Plays while the pad is held.
    Hold,
    /// Press starts, press again stops.
    Toggle,
    /// Loops until pressed again.
    Loop,
}

impl SampleMode {
    pub const ALL: [SampleMode; 4] = [
        SampleMode::OneShot,
        SampleMode::Hold,
        SampleMode::Toggle,
        SampleMode::Loop,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SampleMode::OneShot => "One-shot",
            SampleMode::Hold => "Hold",
            SampleMode::Toggle => "Toggle",
            SampleMode::Loop => "Loop",
        }
    }
}

/// A buffer for recording the master output into a slot. Allocated by the
/// UI (`frames` stereo frames), filled on the audio thread, handed back.
#[derive(Debug)]
pub struct RecordBuffer {
    pub slot: u8,
    /// Interleaved stereo, `2 * frames` samples, pre-allocated.
    pub samples: Vec<f32>,
    /// Frames written so far.
    pub written: usize,
    /// Master tempo when the recording started (for sync), if known.
    pub bpm: Option<f64>,
}

impl RecordBuffer {
    pub fn new(slot: u8, frames: usize) -> Self {
        Self {
            slot,
            samples: dj_dsp::prefaulted_zeros(frames * 2),
            written: 0,
            bpm: None,
        }
    }

    pub fn capacity_frames(&self) -> usize {
        self.samples.len() / 2
    }

    /// The recorded part, interleaved.
    pub fn recorded(&self) -> &[f32] {
        let n = (self.written * 2).min(self.samples.len());
        &self.samples[..n]
    }
}

/// Sampler commands.
#[derive(Debug)]
pub enum SamplerCommand {
    /// Puts a sample in a slot (`bpm` for sync, if known).
    Load {
        slot: u8,
        audio: Arc<TrackAudio>,
        bpm: Option<f64>,
    },
    Eject(u8),
    /// Pad pressed or released.
    Trigger {
        slot: u8,
        pressed: bool,
    },
    Stop(u8),
    StopAll,
    SetMode(u8, SampleMode),
    /// Slot volume, 0 to 1.
    SetVolume(u8, f32),
    SetSync(u8, bool),
    /// Sampler output volume, 0 to 1.
    Volume(f32),
    /// Starts recording the master output into `buffer.slot`.
    Record(RecordBuffer),
    /// Stops a recording early (the part recorded so far is kept).
    StopRecord,
}

/// One slot as the UI sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SlotSnapshot {
    pub loaded: bool,
    pub playing: bool,
    pub mode: SampleMode,
    pub volume: f32,
    pub sync: bool,
    pub bpm: Option<f64>,
    /// Play position as a fraction of the sample.
    pub progress: f32,
    pub length_seconds: f32,
}

/// Sampler state for the UI.
#[derive(Clone, Copy, Debug)]
pub struct SamplerSnapshot {
    pub slots: [SlotSnapshot; SAMPLER_SLOTS],
    pub volume: f32,
    /// Slot being recorded into and how full the buffer is (0 to 1).
    pub recording: Option<(u8, f32)>,
}

impl Default for SamplerSnapshot {
    fn default() -> Self {
        Self {
            slots: [SlotSnapshot {
                volume: 1.0,
                ..SlotSnapshot::default()
            }; SAMPLER_SLOTS],
            volume: 0.8,
            recording: None,
        }
    }
}

const FADE_MS: f32 = 3.0;

struct Slot {
    audio: Option<Arc<TrackAudio>>,
    bpm: Option<f64>,
    mode: SampleMode,
    volume: LinearSmoother,
    sync: bool,
    playing: bool,
    position: f64,
    /// Fade gain (fades out before stopping or restarting).
    gain: LinearSmoother,
    /// Restart from the beginning once the fade-out ends.
    restart: bool,
}

impl Slot {
    fn new(sample_rate: u32) -> Self {
        Self {
            audio: None,
            bpm: None,
            mode: SampleMode::default(),
            volume: LinearSmoother::with_time(1.0, sample_rate, 10.0),
            sync: false,
            playing: false,
            position: 0.0,
            gain: LinearSmoother::with_time(0.0, sample_rate, FADE_MS),
            restart: false,
        }
    }

    fn start(&mut self) {
        if self.audio.is_none() {
            return;
        }
        if self.playing && self.gain.current() > 0.0 {
            // Fade out, then start again from the top.
            self.restart = true;
            self.gain.set_target(0.0);
        } else {
            self.position = 0.0;
            self.playing = true;
            self.restart = false;
            self.gain.set_target(1.0);
        }
    }

    fn stop(&mut self) {
        self.restart = false;
        self.gain.set_target(0.0);
    }

    /// Fading out to stop (not to restart).
    fn stopping(&self) -> bool {
        self.gain.target() == 0.0 && !self.restart
    }

    fn trigger(&mut self, pressed: bool) {
        match (self.mode, pressed) {
            (SampleMode::OneShot | SampleMode::Hold, true) => self.start(),
            (SampleMode::Hold, false) => self.stop(),
            (SampleMode::Toggle | SampleMode::Loop, true) => {
                if self.playing && !self.stopping() {
                    self.stop();
                } else {
                    self.start();
                }
            }
            _ => {}
        }
    }
}

pub(crate) struct Sampler {
    slots: Vec<Slot>,
    volume: LinearSmoother,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
    recording: Option<RecordBuffer>,
    sample_rate: u32,
}

impl Sampler {
    pub(crate) fn new(sample_rate: u32, max_block: usize) -> Self {
        Self {
            slots: (0..SAMPLER_SLOTS).map(|_| Slot::new(sample_rate)).collect(),
            volume: LinearSmoother::with_time(0.8, sample_rate, 10.0),
            out_l: vec![0.0; max_block],
            out_r: vec![0.0; max_block],
            recording: None,
            sample_rate,
        }
    }

    /// Applies a command. Returns a sample or a recording buffer the caller
    /// must hand back to the UI thread.
    pub(crate) fn command(&mut self, cmd: SamplerCommand) -> Returned {
        let slot = |s: u8| usize::from(s);
        match cmd {
            SamplerCommand::Load {
                slot: s,
                audio,
                bpm,
            } => {
                if let Some(sl) = self.slots.get_mut(slot(s)) {
                    sl.playing = false;
                    sl.gain.set_immediate(0.0);
                    sl.position = 0.0;
                    sl.bpm = bpm.filter(|b| *b > 0.0);
                    return Returned::Track(sl.audio.replace(audio));
                }
                return Returned::Track(Some(audio));
            }
            SamplerCommand::Eject(s) => {
                if let Some(sl) = self.slots.get_mut(slot(s)) {
                    sl.playing = false;
                    sl.gain.set_immediate(0.0);
                    sl.bpm = None;
                    return Returned::Track(sl.audio.take());
                }
            }
            SamplerCommand::Trigger { slot: s, pressed } => {
                if let Some(sl) = self.slots.get_mut(slot(s)) {
                    sl.trigger(pressed);
                }
            }
            SamplerCommand::Stop(s) => {
                if let Some(sl) = self.slots.get_mut(slot(s)) {
                    sl.stop();
                }
            }
            SamplerCommand::StopAll => self.slots.iter_mut().for_each(Slot::stop),
            SamplerCommand::SetMode(s, m) => {
                if let Some(sl) = self.slots.get_mut(slot(s)) {
                    sl.mode = m;
                }
            }
            SamplerCommand::SetVolume(s, v) => {
                if let Some(sl) = self.slots.get_mut(slot(s)) {
                    sl.volume.set_target(v.clamp(0.0, 1.0));
                }
            }
            SamplerCommand::SetSync(s, on) => {
                if let Some(sl) = self.slots.get_mut(slot(s)) {
                    sl.sync = on;
                }
            }
            SamplerCommand::Volume(v) => self.volume.set_target(v.clamp(0.0, 1.5)),
            SamplerCommand::Record(buf) => {
                // A recording in progress is handed back first.
                let old = self.recording.replace(buf);
                return Returned::Recording(old);
            }
            SamplerCommand::StopRecord => return Returned::Recording(self.recording.take()),
        }
        Returned::Nothing
    }

    /// Renders all playing slots, mixed and scaled by the sampler volume, into
    /// the internal buffers. `master_bpm` is the tempo synced slots follow.
    pub(crate) fn render(&mut self, n: usize, master_bpm: Option<f64>) -> (&[f32], &[f32]) {
        let n = n.min(self.out_l.len());
        let (out_l, out_r) = (&mut self.out_l[..n], &mut self.out_r[..n]);
        out_l.fill(0.0);
        out_r.fill(0.0);
        for sl in &mut self.slots {
            let Some(audio) = sl.audio.as_deref() else {
                continue;
            };
            if !sl.playing {
                continue;
            }
            let len = audio.len_frames() as f64;
            let rate = match (sl.sync, sl.bpm, master_bpm) {
                (true, Some(own), Some(m)) if own > 0.0 && m > 0.0 => {
                    let r = m / own;
                    (r * 2f64.powf((-r.log2()).round().clamp(-2.0, 2.0))).clamp(0.25, 4.0)
                }
                _ => 1.0,
            };
            let looping = sl.mode == SampleMode::Loop;
            for (l, r) in out_l.iter_mut().zip(out_r.iter_mut()) {
                if !sl.playing {
                    break;
                }
                let g = sl.gain.next_value() * sl.volume.next_value();
                let i = sl.position.floor();
                let t = (sl.position - i) as f32;
                let i = i as u64;
                let f = |k: u64| audio.frame(k).unwrap_or([0.0; 2]);
                let (a, b, c, d) = (f(i.saturating_sub(1)), f(i), f(i + 1), f(i + 2));
                *l += g * hermite(a[0], b[0], c[0], d[0], t);
                *r += g * hermite(a[1], b[1], c[1], d[1], t);
                sl.position += rate;
                if sl.position >= len && audio.is_complete() {
                    if looping && len > 1.0 {
                        sl.position -= len;
                    } else {
                        sl.playing = false;
                        sl.gain.set_immediate(0.0);
                    }
                }
                if sl.gain.is_settled() && sl.gain.current() == 0.0 {
                    if sl.restart {
                        sl.restart = false;
                        sl.position = 0.0;
                        sl.gain.set_target(1.0);
                    } else {
                        sl.playing = false;
                    }
                }
            }
        }
        for (l, r) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let v = self.volume.next_value();
            *l *= v;
            *r *= v;
        }
        (&self.out_l[..n], &self.out_r[..n])
    }

    /// Copies the master output into the recording buffer. Returns the
    /// buffer when it is full.
    pub(crate) fn record(&mut self, l: &[f32], r: &[f32]) -> Option<RecordBuffer> {
        let rec = self.recording.as_mut()?;
        let cap = rec.capacity_frames();
        for (a, b) in l.iter().zip(r.iter()) {
            if rec.written >= cap {
                break;
            }
            let k = rec.written * 2;
            if let Some(s) = rec.samples.get_mut(k..k + 2) {
                s[0] = *a;
                s[1] = *b;
            }
            rec.written += 1;
        }
        if rec.written >= cap {
            self.recording.take()
        } else {
            None
        }
    }

    pub(crate) fn snapshot(&self) -> SamplerSnapshot {
        let mut s = SamplerSnapshot {
            volume: self.volume.target(),
            recording: self
                .recording
                .as_ref()
                .map(|r| (r.slot, r.written as f32 / r.capacity_frames().max(1) as f32)),
            ..SamplerSnapshot::default()
        };
        for (out, sl) in s.slots.iter_mut().zip(self.slots.iter()) {
            let len = sl.audio.as_ref().map_or(0, |a| a.len_frames()) as f64;
            *out = SlotSnapshot {
                loaded: sl.audio.is_some(),
                playing: sl.playing && !sl.stopping(),
                mode: sl.mode,
                volume: sl.volume.target(),
                sync: sl.sync,
                bpm: sl.bpm,
                progress: if len > 0.0 {
                    (sl.position / len) as f32
                } else {
                    0.0
                },
                length_seconds: (len / f64::from(self.sample_rate.max(1))) as f32,
            };
        }
        s
    }
}

/// Something the audio thread must not free, on its way back to the UI.
pub(crate) enum Returned {
    Nothing,
    Track(Option<Arc<TrackAudio>>),
    Recording(Option<RecordBuffer>),
}
