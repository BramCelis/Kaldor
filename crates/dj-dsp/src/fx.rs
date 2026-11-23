//! The effects of the FX units.
//!
//! An [`Effect`] runs one [`EffectKind`] at a time on a stereo signal, in
//! place, synced to the beat clock in [`FxContext`]. All memory is allocated
//! in [`Effect::new`]; [`Effect::process`] never allocates and never panics.
//!
//! Two families:
//! - **Tail effects** (echo, ping-pong, reverb, wash-out) add a wet signal
//!   to the input. With [`FxContext::feed`] off no new input goes in, but
//!   what is already in the delay or reverb keeps sounding and dies away.
//! - **Insert effects** (everything else) replace the signal. The FX unit
//!   fades them in and out; [`FxContext::feed`] does not matter to them.
//!
//! All algorithms are textbook ones written for this project (delay lines,
//! LFOs, the TPT state-variable filter, first-order allpasses, an 8-line
//! feedback-delay-network reverb, a two-tap rotating-delay pitch shifter),
//! tuned for DJ use: filtered and
//! saturated echo feedback, a sub-free reverb tail, raised-cosine gate
//! edges and crossfaded roll seams.

use crate::biquad::{Biquad, Coeffs};
use crate::interp::hermite;
use crate::svf::Svf;
use std::f32::consts::PI;

/// Longest delay, loop or captured window an effect can hold, in seconds.
pub const MAX_SECONDS: f32 = 3.2;

/// Wet level of the reverb at full depth (the hall's output is quiet: it
/// spreads the energy over seconds, and the 120 Hz high-pass takes the
/// bass out).
pub(crate) const REVERB_WET_GAIN: f32 = 4.5;

/// Effect types. The order is the order in menus.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum EffectKind {
    #[default]
    Echo,
    PingPong,
    Reverb,
    WashOut,
    Flanger,
    Phaser,
    Chorus,
    Filter,
    BandPassSweep,
    NoiseSweep,
    Gate,
    AutoPan,
    BeatRoll,
    Slicer,
    Reverse,
    VinylBrake,
    TapeStop,
    BitCrusher,
    Overdrive,
    PitchShifter,
}

impl EffectKind {
    pub const ALL: [EffectKind; 20] = [
        EffectKind::Echo,
        EffectKind::PingPong,
        EffectKind::Reverb,
        EffectKind::WashOut,
        EffectKind::Flanger,
        EffectKind::Phaser,
        EffectKind::Chorus,
        EffectKind::Filter,
        EffectKind::BandPassSweep,
        EffectKind::NoiseSweep,
        EffectKind::Gate,
        EffectKind::AutoPan,
        EffectKind::BeatRoll,
        EffectKind::Slicer,
        EffectKind::Reverse,
        EffectKind::VinylBrake,
        EffectKind::TapeStop,
        EffectKind::BitCrusher,
        EffectKind::Overdrive,
        EffectKind::PitchShifter,
    ];

    /// Name for the screen.
    pub fn name(self) -> &'static str {
        match self {
            EffectKind::Echo => "Echo",
            EffectKind::PingPong => "Ping-pong",
            EffectKind::Reverb => "Reverb",
            EffectKind::WashOut => "Wash-out",
            EffectKind::Flanger => "Flanger",
            EffectKind::Phaser => "Phaser",
            EffectKind::Chorus => "Chorus",
            EffectKind::Filter => "Filter",
            EffectKind::BandPassSweep => "Band sweep",
            EffectKind::NoiseSweep => "Noise sweep",
            EffectKind::Gate => "Gate",
            EffectKind::AutoPan => "Auto-pan",
            EffectKind::BeatRoll => "Beat roll",
            EffectKind::Slicer => "Slicer",
            EffectKind::Reverse => "Reverse",
            EffectKind::VinylBrake => "Vinyl brake",
            EffectKind::TapeStop => "Tape stop",
            EffectKind::BitCrusher => "Bit crusher",
            EffectKind::Overdrive => "Overdrive",
            EffectKind::PitchShifter => "Pitch shift",
        }
    }

    /// Stable name for config and preset files.
    pub fn id(self) -> &'static str {
        match self {
            EffectKind::Echo => "echo",
            EffectKind::PingPong => "ping_pong",
            EffectKind::Reverb => "reverb",
            EffectKind::WashOut => "wash_out",
            EffectKind::Flanger => "flanger",
            EffectKind::Phaser => "phaser",
            EffectKind::Chorus => "chorus",
            EffectKind::Filter => "filter",
            EffectKind::BandPassSweep => "band_pass_sweep",
            EffectKind::NoiseSweep => "noise_sweep",
            EffectKind::Gate => "gate",
            EffectKind::AutoPan => "auto_pan",
            EffectKind::BeatRoll => "beat_roll",
            EffectKind::Slicer => "slicer",
            EffectKind::Reverse => "reverse",
            EffectKind::VinylBrake => "vinyl_brake",
            EffectKind::TapeStop => "tape_stop",
            EffectKind::BitCrusher => "bit_crusher",
            EffectKind::Overdrive => "overdrive",
            EffectKind::PitchShifter => "pitch_shifter",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Names of the three knobs ("" = not used). The first is always
    /// **Depth**: 0 = the effect is not heard, 1 = all of it. In multi mode
    /// it is the only knob.
    pub fn param_names(self) -> [&'static str; 3] {
        match self {
            EffectKind::Echo | EffectKind::PingPong => ["Depth", "Feedback", "Tone"],
            EffectKind::Reverb => ["Depth", "Size", "Damping"],
            EffectKind::WashOut => ["Depth", "Intensity", "Low cut"],
            EffectKind::Flanger => ["Depth", "Feedback", "Delay"],
            EffectKind::Phaser => ["Depth", "Feedback", "Spread"],
            EffectKind::Chorus => ["Depth", "Spread", "Width"],
            EffectKind::Filter => ["Depth", "LP / HP", "Resonance"],
            EffectKind::BandPassSweep => ["Depth", "Resonance", "Centre"],
            EffectKind::NoiseSweep => ["Depth", "Resonance", "Up / down"],
            EffectKind::Gate => ["Depth", "Length", "Smooth"],
            EffectKind::AutoPan => ["Depth", "Shape", ""],
            EffectKind::BeatRoll => ["Depth", "Fade", ""],
            EffectKind::Slicer => ["Depth", "Pattern", ""],
            EffectKind::Reverse => ["Depth", "Smooth", ""],
            EffectKind::VinylBrake => ["Depth", "Curve", ""],
            EffectKind::TapeStop => ["Depth", "Darken", ""],
            EffectKind::BitCrusher => ["Depth", "Downsample", "Level"],
            EffectKind::Overdrive => ["Depth", "Tone", "Level"],
            EffectKind::PitchShifter => ["Depth", "Up / down", "Grain"],
        }
    }

    /// Default knob values (Depth first).
    pub fn defaults(self) -> [f32; 3] {
        match self {
            EffectKind::Echo | EffectKind::PingPong => [0.75, 0.45, 0.65],
            EffectKind::Reverb => [0.6, 0.6, 0.4],
            EffectKind::WashOut => [0.75, 0.6, 0.5],
            // Feedback knob 0.8 = +0.42 (0.5 is none, 0 is -0.7).
            EffectKind::Flanger => [0.8, 0.8, 0.3],
            EffectKind::Phaser => [0.8, 0.4, 0.3],
            EffectKind::Chorus => [0.7, 0.5, 0.6],
            EffectKind::Filter => [0.5, 0.0, 0.3],
            EffectKind::BandPassSweep => [0.8, 0.5, 0.5],
            EffectKind::NoiseSweep => [0.5, 0.5, 0.0],
            EffectKind::Gate => [1.0, 0.5, 0.2],
            EffectKind::AutoPan => [0.8, 0.0, 0.0],
            EffectKind::BeatRoll => [1.0, 0.0, 0.0],
            EffectKind::Slicer => [1.0, 0.3, 0.0],
            EffectKind::Reverse => [1.0, 0.3, 0.0],
            EffectKind::VinylBrake => [1.0, 0.0, 0.0],
            EffectKind::TapeStop => [1.0, 0.5, 0.0],
            EffectKind::BitCrusher => [0.5, 0.3, 0.8],
            EffectKind::Overdrive => [0.5, 0.6, 0.7],
            // +7 semitones.
            EffectKind::PitchShifter => [7.0 / 12.0, 0.0, 0.4],
        }
    }

    /// The knobs (Depth first) as the effect's own settings for
    /// [`Effect::process`], and how much of the effect is heard (0 to 1:
    /// the FX unit blends the effect in by this much).
    pub fn internal(self, knobs: [f32; 3]) -> ([f32; 3], f32) {
        let [d, a, b] = knobs.map(|v| v.clamp(0.0, 1.0));
        // Effects whose sound grows with the depth itself still fade in from
        // zero, so depth 0 is always the dry signal.
        let near = |k: f32| (d * k).min(1.0);
        match self {
            EffectKind::Echo | EffectKind::PingPong => ([a, b, 1.0 / 1.2], d),
            // The reverb applies its depth itself (it also lowers the dry
            // signal), so the unit adds it at full.
            EffectKind::Reverb => ([a, b, d], 1.0),
            EffectKind::WashOut => ([a, b, 1.0], d),
            EffectKind::Flanger => ([0.75, a, b], d),
            EffectKind::Phaser => ([0.85, a, b], d),
            EffectKind::Chorus => ([b, a, 1.0], d),
            EffectKind::Filter => ([d, a, b], near(20.0)),
            EffectKind::BandPassSweep => ([0.8, a, b], d),
            EffectKind::NoiseSweep => ([d, a, b], near(20.0)),
            EffectKind::Gate | EffectKind::AutoPan => ([d, a, b], near(20.0)),
            EffectKind::BeatRoll
            | EffectKind::Slicer
            | EffectKind::Reverse
            | EffectKind::VinylBrake
            | EffectKind::TapeStop => ([a, 0.0, 0.0], d),
            EffectKind::BitCrusher => ([d, a * d, b], near(10.0)),
            EffectKind::Overdrive => ([d, a, b], near(4.0)),
            EffectKind::PitchShifter => {
                let pitch = if a < 0.5 {
                    0.5 + d * 0.5
                } else {
                    0.5 - d * 0.5
                };
                ([pitch, b, 0.0], near(20.0))
            }
        }
    }

    /// Adds a dying-away tail to the signal (see the module docs).
    pub fn has_tail(self) -> bool {
        matches!(
            self,
            EffectKind::Echo | EffectKind::PingPong | EffectKind::Reverb | EffectKind::WashOut
        )
    }

    /// Needs the audio from just before it was switched on (loops and
    /// captures are aligned to the beat), so the FX unit keeps feeding it
    /// while it is off.
    pub fn needs_history(self) -> bool {
        matches!(
            self,
            EffectKind::BeatRoll
                | EffectKind::Slicer
                | EffectKind::Reverse
                | EffectKind::VinylBrake
                | EffectKind::TapeStop
        )
    }
}

/// What an effect needs to know for one block.
#[derive(Clone, Copy, Debug)]
pub struct FxContext {
    /// Frames per beat at the current tempo.
    pub beat_frames: f64,
    /// The FX unit's beats setting (1/16 to 32).
    pub beats: f64,
    /// Bar-aligned beat count at the first frame of the block (a multiple
    /// of 4 is a downbeat).
    pub bar_beat: f64,
    /// The three parameters, 0 to 1.
    pub params: [f32; 3],
    /// New input goes into tail effects (off: the tail dies away).
    pub feed: bool,
    /// How much of their input tail effects (echo, ping-pong, reverb,
    /// wash-out) pass through, at the start and end of the block (the FX
    /// unit's WET/DRY turned past the middle takes the track out; 1 = all).
    pub pass_from: f32,
    pub pass_to: f32,
}

impl FxContext {
    /// Length of the beats setting in frames.
    pub fn period(&self) -> f64 {
        (self.beat_frames * self.beats).max(1.0)
    }

    /// The pass-through ramp for a block of `n` frames: the value before
    /// the first frame and the step per frame.
    #[inline]
    pub fn pass_ramp(&self, n: usize) -> (f32, f32) {
        (
            self.pass_from,
            (self.pass_to - self.pass_from) / n.max(1) as f32,
        )
    }

    /// LFO phase (0 to 1) at frame `i` of the block: one cycle per beats
    /// setting, aligned to the bar.
    #[inline]
    fn lfo(&self, i: usize) -> f32 {
        let b = self.bar_beat + i as f64 / self.beat_frames.max(1.0);
        (b / self.beats.max(1e-3)).rem_euclid(1.0) as f32
    }

    /// Position (0 to 1) within the current beats period at the block start.
    fn phase(&self) -> f64 {
        (self.bar_beat / self.beats.max(1e-3)).rem_euclid(1.0)
    }
}

/// Stereo history buffer with absolute frame numbers.
struct Ring {
    l: Vec<f32>,
    r: Vec<f32>,
    /// Frames written since creation.
    written: u64,
    /// Frames of valid history (reset to 0 instead of clearing memory).
    filled: u64,
}

impl Ring {
    fn new(cap: usize) -> Self {
        let cap = cap.max(1024);
        Self {
            l: crate::prefaulted_zeros(cap),
            r: crate::prefaulted_zeros(cap),
            written: 0,
            filled: 0,
        }
    }

    fn cap(&self) -> u64 {
        self.l.len() as u64
    }

    fn reset(&mut self) {
        self.filled = 0;
    }

    #[inline]
    fn push(&mut self, l: f32, r: f32) {
        let i = (self.written % self.cap()) as usize;
        if let (Some(a), Some(b)) = (self.l.get_mut(i), self.r.get_mut(i)) {
            *a = l;
            *b = r;
        }
        self.written += 1;
        if self.filled < self.cap() {
            self.filled += 1;
        }
    }

    /// Frame number `abs`, or silence if it is not held.
    #[inline]
    fn at(&self, abs: i64) -> (f32, f32) {
        if abs < 0 {
            return (0.0, 0.0);
        }
        let abs = abs as u64;
        if abs >= self.written || self.written - abs > self.filled {
            return (0.0, 0.0);
        }
        let i = (abs % self.cap()) as usize;
        (
            self.l.get(i).copied().unwrap_or(0.0),
            self.r.get(i).copied().unwrap_or(0.0),
        )
    }

    /// Interpolated at a fractional frame number.
    #[inline]
    fn read_at(&self, pos: f64) -> (f32, f32) {
        let i = pos.floor();
        let t = (pos - i) as f32;
        let i = i as i64;
        let (a0, b0) = self.at(i - 1);
        let (a1, b1) = self.at(i);
        let (a2, b2) = self.at(i + 1);
        let (a3, b3) = self.at(i + 2);
        (hermite(a0, a1, a2, a3, t), hermite(b0, b1, b2, b3, t))
    }

    /// Interpolated `delay` frames back (at least 3).
    #[inline]
    fn read_delay(&self, delay: f32) -> (f32, f32) {
        self.read_at(self.written as f64 - f64::from(delay.max(3.0)))
    }
}

/// Soft saturation for feedback loops: identity up to 0.6, then a `tanh`
/// knee towards 1.0, so runaway feedback is rounded off instead of
/// clipping.
#[inline]
fn soft_sat(x: f32) -> f32 {
    const K: f32 = 0.6;
    let a = x.abs();
    if a <= K {
        x
    } else {
        (K + (1.0 - K) * ((a - K) / (1.0 - K)).tanh()).copysign(x)
    }
}

/// One-pole low-pass coefficient for `hz`.
#[inline]
fn one_pole(hz: f32, sr: f32) -> f32 {
    1.0 - (-2.0 * PI * hz / sr).exp()
}

/// A plain delay line for the reverb.
struct Line {
    buf: Vec<f32>,
    i: usize,
}

impl Line {
    fn new(len: usize) -> Self {
        Self {
            buf: crate::prefaulted_zeros(len.max(1)),
            i: 0,
        }
    }

    /// The oldest value (`len` frames ago).
    #[inline]
    fn read(&self) -> f32 {
        self.buf.get(self.i).copied().unwrap_or(0.0)
    }

    #[inline]
    fn write(&mut self, x: f32) {
        if let Some(s) = self.buf.get_mut(self.i) {
            *s = x;
        }
        self.i += 1;
        if self.i >= self.buf.len() {
            self.i = 0;
        }
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
    }

    fn len(&self) -> usize {
        self.buf.len()
    }
}

/// Schroeder allpass diffuser.
struct Diffuser {
    line: Line,
    g: f32,
}

impl Diffuser {
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let d = self.line.read();
        let v = x + self.g * d;
        self.line.write(v);
        d - self.g * v
    }
}

/// FDN delay lengths at 44.1 kHz: mutually prime, so echoes never line up.
const FDN_LENGTHS: [f32; 8] = [
    1129.0, 1427.0, 1601.0, 1789.0, 1913.0, 2111.0, 2357.0, 2531.0,
];
/// Input diffusers at 44.1 kHz with their gains.
const DIFFUSERS: [(f32, f32); 4] = [(142.0, 0.75), (107.0, 0.75), (379.0, 0.625), (277.0, 0.625)];
const PRE_DELAY_MS: f32 = 25.0;

/// Stereo hall reverb: pre-delay, four input diffusers per channel, an
/// 8-line feedback delay network mixed by a Hadamard matrix, with a
/// low-pass (damping) and a 100 Hz high-pass in every feedback path, and a
/// 120 Hz 12 dB/oct high-pass on the wet output so the reverb never washes
/// the sub-bass. Also the color FX Space (wet only, see `color_fx`).
pub(crate) struct Reverb {
    sr: f32,
    pre: [Line; 2],
    diff: [[Diffuser; 4]; 2],
    lines: [Line; 8],
    gains: [f32; 8],
    lp: [f32; 8],
    hp_x: [f32; 8],
    hp_y: [f32; 8],
    lp_k: f32,
    hp_a: f32,
    wet_hp: [Biquad; 2],
    /// Settings the gains were computed for.
    rt60: f32,
    damp_hz: f32,
    dirty: bool,
}

impl Reverb {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f32;
        let k = sr / 44_100.0;
        let len = |n: f32| ((n * k).round() as usize).max(1);
        let pre = || Line::new(((PRE_DELAY_MS * 0.001 * sr) as usize).max(1));
        let diff = |ch: usize| {
            std::array::from_fn(|i| Diffuser {
                line: Line::new(len(DIFFUSERS[i].0 + 7.0 * ch as f32)),
                g: DIFFUSERS[i].1,
            })
        };
        let wet = Biquad::new(Coeffs::highpass(
            120.0,
            std::f64::consts::FRAC_1_SQRT_2,
            f64::from(sr),
        ));
        let mut r = Self {
            sr,
            pre: [pre(), pre()],
            diff: [diff(0), diff(1)],
            lines: std::array::from_fn(|i| Line::new(len(FDN_LENGTHS[i]))),
            gains: [0.0; 8],
            lp: [0.0; 8],
            hp_x: [0.0; 8],
            hp_y: [0.0; 8],
            lp_k: 0.0,
            hp_a: (-2.0 * PI * 100.0 / sr).exp(),
            wet_hp: [wet, wet],
            rt60: 0.0,
            damp_hz: 0.0,
            dirty: false,
        };
        r.configure(2.0, 5000.0);
        r
    }

    /// Decay time (seconds to -60 dB) and damping corner.
    pub(crate) fn configure(&mut self, rt60: f32, damp_hz: f32) {
        if (rt60 - self.rt60).abs() < 1e-3 && (damp_hz - self.damp_hz).abs() < 1.0 {
            return;
        }
        self.rt60 = rt60;
        self.damp_hz = damp_hz;
        for (g, l) in self.gains.iter_mut().zip(self.lines.iter()) {
            *g = 10f32.powf(-3.0 * l.len() as f32 / (rt60.max(0.05) * self.sr));
        }
        self.lp_k = one_pole(damp_hz.min(self.sr * 0.45), self.sr);
    }

    pub(crate) fn clear(&mut self) {
        if !self.dirty {
            return;
        }
        for l in self.pre.iter_mut().chain(self.lines.iter_mut()) {
            l.clear();
        }
        for d in self.diff.iter_mut().flatten() {
            d.line.clear();
        }
        self.lp = [0.0; 8];
        self.hp_x = [0.0; 8];
        self.hp_y = [0.0; 8];
        for b in &mut self.wet_hp {
            b.reset();
        }
        self.dirty = false;
    }

    /// The wet signal for one frame.
    #[inline]
    pub(crate) fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        self.dirty = true;
        // Pre-delay and diffusion.
        let mut inp = [0.0f32; 2];
        for (ch, x) in [l, r].into_iter().enumerate() {
            let mut v = self.pre[ch].read();
            self.pre[ch].write(x);
            for d in &mut self.diff[ch] {
                v = d.process(v);
            }
            inp[ch] = v;
        }
        // Read the lines, damp them.
        let mut v = [0.0f32; 8];
        for (i, o) in v.iter_mut().enumerate() {
            let y = self.lines[i].read();
            self.lp[i] += (y - self.lp[i]) * self.lp_k;
            // One-pole high-pass at 150 Hz.
            let hp = self.hp_a * (self.hp_y[i] + self.lp[i] - self.hp_x[i]);
            self.hp_x[i] = self.lp[i];
            self.hp_y[i] = hp;
            *o = hp;
        }
        let out_l = (v[0] - v[2] + v[4] - v[6]) * 0.5;
        let out_r = (v[1] - v[3] + v[5] - v[7]) * 0.5;
        // Hadamard mix (fast Walsh-Hadamard, normalised).
        let mut h = 1;
        while h < 8 {
            let mut i = 0;
            while i < 8 {
                for j in i..i + h {
                    let (a, b) = (v[j], v[j + h]);
                    v[j] = a + b;
                    v[j + h] = a - b;
                }
                i += 2 * h;
            }
            h *= 2;
        }
        let norm = 1.0 / 8f32.sqrt();
        for (i, x) in v.iter().enumerate() {
            let input = if i % 2 == 0 { inp[0] } else { inp[1] };
            self.lines[i].write(soft_sat(x * norm * self.gains[i] + input * 0.5));
        }
        (self.wet_hp[0].process(out_l), self.wet_hp[1].process(out_r))
    }
}

/// Slice orders of the slicer (8 slices per window).
const SLICER_PATTERNS: [[u8; 8]; 5] = [
    [0, 1, 2, 3, 4, 5, 6, 7],
    [0, 1, 0, 1, 4, 5, 4, 5],
    [0, 0, 2, 2, 4, 4, 6, 7],
    [0, 1, 2, 3, 3, 2, 1, 0],
    [7, 6, 5, 4, 3, 2, 1, 0],
];

/// Coefficients of swept filters are recomputed every this many frames.
const UPDATE: u32 = 16;

/// One effect instance.
pub struct Effect {
    kind: EffectKind,
    sr: f32,
    ring: Ring,
    reverb: Reverb,
    /// One-pole coefficients: ~60 ms (delay times) and ~10 ms (gains).
    k_slow: f32,
    k_fast: f32,
    /// Smoothed delay in frames (0 = not set yet).
    delay: f32,
    feedback: f32,
    lp: [f32; 2],
    hp: [f32; 2],
    hp_x: [f32; 2],
    svf: [Svf; 2],
    /// Phaser: allpass states (x1, y1) per stage and channel.
    ap: [[(f32, f32); 6]; 2],
    ap_out: [f32; 2],
    coef: [f32; 2],
    hold: [f32; 2],
    hold_left: u32,
    gain: f32,
    pan: f32,
    /// Loop / capture start (frame number in the ring).
    start: u64,
    /// Frames since `start`.
    t: u64,
    /// Brake / tape stop read position and speed.
    read: f64,
    rate: f32,
    noise: u32,
    grain: f32,
    tick: u32,
    /// Echo: 2-pole 200 Hz high-pass in the feedback loop.
    echo_hp: [Biquad; 2],
    /// Beat roll: the audio just before the loop is held, so the loop seam
    /// can be crossfaded.
    seam_ok: bool,
    /// Reverb: smoothed depth (wet amount) and dry level.
    rv_wet: f32,
    rv_dry: f32,
}

impl Effect {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f32;
        let k = |ms: f32| 1.0 - (-1.0 / (ms * 0.001 * sr)).exp();
        Self {
            kind: EffectKind::default(),
            sr,
            ring: Ring::new((MAX_SECONDS * sr) as usize),
            reverb: Reverb::new(sample_rate.max(1)),
            k_slow: k(60.0),
            k_fast: k(10.0),
            delay: 0.0,
            feedback: 0.0,
            lp: [0.0; 2],
            hp: [0.0; 2],
            hp_x: [0.0; 2],
            svf: [Svf::default(); 2],
            ap: [[(0.0, 0.0); 6]; 2],
            ap_out: [0.0; 2],
            coef: [0.0; 2],
            hold: [0.0; 2],
            hold_left: 0,
            gain: 1.0,
            pan: 0.0,
            start: 0,
            t: 0,
            read: 0.0,
            rate: 1.0,
            noise: 0x1234_5678,
            grain: 0.0,
            tick: 0,
            echo_hp: [Biquad::new(Coeffs::highpass(
                200.0,
                std::f64::consts::FRAC_1_SQRT_2,
                f64::from(sr),
            )); 2],
            seam_ok: false,
            rv_wet: 0.0,
            rv_dry: 1.0,
        }
    }

    pub fn kind(&self) -> EffectKind {
        self.kind
    }

    /// Changes the effect type (forgets everything, history included).
    pub fn set_kind(&mut self, kind: EffectKind) {
        self.kind = kind;
        self.ring.reset();
        self.clear_state();
    }

    fn clear_state(&mut self) {
        self.reverb.clear();
        self.delay = 0.0;
        self.feedback = 0.0;
        self.lp = [0.0; 2];
        self.hp = [0.0; 2];
        self.hp_x = [0.0; 2];
        for s in &mut self.svf {
            s.reset();
        }
        self.ap = [[(0.0, 0.0); 6]; 2];
        self.ap_out = [0.0; 2];
        self.hold = [0.0; 2];
        self.hold_left = 0;
        self.gain = 1.0;
        self.pan = 0.0;
        self.t = 0;
        self.rate = 1.0;
        self.grain = 0.0;
        self.tick = 0;
    }

    /// Feeds the history of an effect that is off (see
    /// [`EffectKind::needs_history`]).
    pub fn warm(&mut self, l: &[f32], r: &[f32]) {
        for (a, b) in l.iter().zip(r.iter()) {
            self.ring.push(*a, *b);
        }
    }

    /// The effect is switched on (it was silent before).
    pub fn start(&mut self, c: &FxContext) {
        if !self.kind.needs_history() {
            self.ring.reset();
        }
        self.clear_state();
        let cap = self.ring.cap();
        let written = self.ring.written;
        match self.kind {
            EffectKind::BeatRoll | EffectKind::Slicer => {
                // The loop starts at the last beats boundary; what came
                // since then is already in the history.
                let len = self.loop_len(c);
                let offset = ((c.phase() * len as f64) as u64)
                    .min(self.ring.filled)
                    .min(cap / 2);
                self.start = written - offset;
                self.t = offset;
                // Enough history before the loop start for the seam crossfade.
                let seam = (0.002 * self.sr) as u64 + 1;
                self.seam_ok = offset + seam < self.ring.filled;
            }
            EffectKind::VinylBrake | EffectKind::TapeStop => {
                self.read = written as f64 - 4.0;
                self.rate = 1.0;
            }
            EffectKind::Reverse => self.start = written,
            EffectKind::Reverb => {
                self.rv_wet = c.params[2];
                self.rv_dry = Self::reverb_dry(c.params[2]);
            }
            _ => {}
        }
    }

    /// Loop / slice length in frames.
    fn loop_len(&self, c: &FxContext) -> u64 {
        let cap = self.ring.cap();
        match self.kind {
            EffectKind::Slicer => (c.period() as u64).clamp(64, cap / 8 - 8),
            EffectKind::Reverse => (c.period() as u64).clamp(256, cap / 2 - 8),
            _ => (c.period() as u64).clamp(64, cap - 64),
        }
    }

    /// Processes one block in place (`l` and `r` the same length).
    pub fn process(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        match self.kind {
            EffectKind::Echo => self.echo(c, l, r, false),
            EffectKind::PingPong => self.echo(c, l, r, true),
            EffectKind::Reverb => self.reverb(c, l, r),
            EffectKind::WashOut => self.wash_out(c, l, r),
            EffectKind::Flanger => self.flanger(c, l, r),
            EffectKind::Phaser => self.phaser(c, l, r),
            EffectKind::Chorus => self.chorus(c, l, r),
            EffectKind::Filter => self.filter(c, l, r),
            EffectKind::BandPassSweep => self.band_sweep(c, l, r),
            EffectKind::NoiseSweep => self.noise_sweep(c, l, r),
            EffectKind::Gate => self.gate(c, l, r),
            EffectKind::AutoPan => self.auto_pan(c, l, r),
            EffectKind::BeatRoll => self.beat_roll(c, l, r),
            EffectKind::Slicer => self.slicer(c, l, r),
            EffectKind::Reverse => self.reverse(c, l, r),
            EffectKind::VinylBrake => self.brake(c, l, r, false),
            EffectKind::TapeStop => self.brake(c, l, r, true),
            EffectKind::BitCrusher => self.bit_crusher(c, l, r),
            EffectKind::Overdrive => self.overdrive(c, l, r),
            EffectKind::PitchShifter => self.pitch_shift(c, l, r),
        }
    }

    fn max_delay(&self) -> f32 {
        self.ring.cap() as f32 - 8.0
    }

    fn echo(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32], ping: bool) {
        let target = (c.period() as f32).clamp(16.0, self.max_delay());
        if self.delay <= 0.0 {
            self.delay = target;
        }
        let fb_target = c.params[0] * 0.95;
        // Tone: the feedback loop's low-pass, 1.5 kHz to 12 kHz (the
        // default sits near 5 kHz, like a tape echo darkening each repeat).
        let tone = one_pole(1500.0 * 8f32.powf(c.params[1]), self.sr);
        let level = c.params[2] * 1.2;
        let feed = if c.feed { 1.0 } else { 0.0 };
        let (mut pass, dp) = c.pass_ramp(l.len());
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            self.delay += (target - self.delay) * self.k_slow;
            self.feedback += (fb_target - self.feedback) * self.k_fast;
            let (yl, yr) = self.ring.read_delay(self.delay);
            // Each repeat: high-pass 200 Hz (no bass build-up), low-pass
            // (darker), soft saturation (no runaway at high feedback).
            let hl = self.echo_hp[0].process(yl);
            let hr = self.echo_hp[1].process(yr);
            self.lp[0] += (hl - self.lp[0]) * tone;
            self.lp[1] += (hr - self.lp[1]) * tone;
            let (el, er) = (self.lp[0], self.lp[1]);
            let fb = self.feedback;
            let (wl, wr) = if ping {
                let m = 0.5 * (*xl + *xr) * feed;
                (m + er * fb, el * fb)
            } else {
                (*xl * feed + el * fb, *xr * feed + er * fb)
            };
            self.ring.push(soft_sat(wl), soft_sat(wr));
            pass += dp;
            *xl = *xl * pass + el * level;
            *xr = *xr * pass + er * level;
        }
    }

    fn reverb(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        // Size: decay 0.5 s to 8 s. Damping: 10 kHz (bright) to 2 kHz.
        let rt60 = 0.5 * 16f32.powf(c.params[0]);
        let damp = 10_000.0 * 0.2f32.powf(c.params[1]);
        self.reverb.configure(rt60, damp);
        // Depth raises the wet signal and pulls the dry one back (to -12 dB
        // at full depth), like a send reverb pushed into the mix; switched
        // off, the dry signal returns while the tail rings out ("even
        // fully wet it is not very audible").
        let depth = c.params[2];
        let wet_target = depth;
        let dry_target = if c.feed { Self::reverb_dry(depth) } else { 1.0 };
        let feed = if c.feed { 1.0 } else { 0.0 };
        let k = self.k_fast;
        let (mut pass, dp) = c.pass_ramp(l.len());
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            self.rv_wet += (wet_target - self.rv_wet) * k;
            self.rv_dry += (dry_target - self.rv_dry) * k;
            if (self.rv_wet - wet_target).abs() < 1e-5 {
                self.rv_wet = wet_target;
            }
            if (self.rv_dry - dry_target).abs() < 1e-5 {
                self.rv_dry = dry_target;
            }
            let (wl, wr) = self.reverb.process(*xl * feed, *xr * feed);
            let g = self.rv_wet * REVERB_WET_GAIN;
            pass += dp;
            let dry = self.rv_dry * pass;
            *xl = *xl * dry + wl * g;
            *xr = *xr * dry + wr * g;
        }
    }

    /// Dry level of the reverb at `depth`: -12 dB at full depth.
    fn reverb_dry(depth: f32) -> f32 {
        1.0 - 0.75 * depth * depth
    }

    fn wash_out(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let target = (c.period() as f32).clamp(16.0, self.max_delay());
        if self.delay <= 0.0 {
            self.delay = target;
        }
        let fb = 0.55 + 0.4 * c.params[0];
        self.reverb.configure(4.0, 4000.0);
        let cut = 50.0 * 40f32.powf(c.params[1]);
        let a = 1.0 / (1.0 + 2.0 * PI * cut / self.sr);
        let level = c.params[2];
        let feed = if c.feed { 1.0 } else { 0.0 };
        let (mut pass, dp) = c.pass_ramp(l.len());
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            self.delay += (target - self.delay) * self.k_slow;
            let (yl, yr) = self.ring.read_delay(self.delay);
            let mut h = [0.0f32; 2];
            for (ch, y) in [yl, yr].into_iter().enumerate() {
                self.hp[ch] = a * (self.hp[ch] + y - self.hp_x[ch]);
                self.hp_x[ch] = y;
                h[ch] = self.hp[ch];
            }
            self.ring.push(
                soft_sat(*xl * feed + h[0] * fb),
                soft_sat(*xr * feed + h[1] * fb),
            );
            let (vl, vr) = self.reverb.process(h[0], h[1]);
            pass += dp;
            *xl = *xl * pass + (yl + vl) * level * 0.8;
            *xr = *xr * pass + (yr + vr) * level * 0.8;
        }
    }

    fn flanger(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        // Delay sweeps exponentially between `lo` (raised by the Delay knob)
        // and 8 ms; the Feedback knob runs from -0.7 (hollow "jet") through
        // 0 to +0.7; the right channel's LFO is 90 degrees ahead.
        let depth = c.params[0];
        let fb = (c.params[1] * 2.0 - 1.0) * 0.7;
        const HI_MS: f32 = 8.0;
        let lo_ms = 0.5 * (HI_MS / 0.5f32).powf(0.6 * c.params[2]);
        let span = (HI_MS / lo_ms).ln() * depth;
        let lo = lo_ms * 0.001 * self.sr;
        let delay = |ph: f32| {
            // Raised cosine: smoother turnarounds than a triangle.
            let s = 0.5 - 0.5 * (2.0 * PI * ph).cos();
            3.0 + lo * (span * s).exp()
        };
        for (i, (xl, xr)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let ph = c.lfo(i);
            let (yl, _) = self.ring.read_delay(delay(ph));
            let (_, yr) = self.ring.read_delay(delay((ph + 0.25).rem_euclid(1.0)));
            self.ring
                .push(soft_sat(*xl + yl * fb), soft_sat(*xr + yr * fb));
            *xl = 0.5 * (*xl + yl);
            *xr = 0.5 * (*xr + yr);
        }
    }

    fn phaser(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let depth = c.params[0];
        let fb = c.params[1] * 0.7;
        let spread = c.params[2] * 0.5;
        let sr = self.sr;
        let coef = |ph: f32| {
            // Six first-order allpasses swept exponentially 200 Hz - 4 kHz.
            let s = 0.5 - 0.5 * (2.0 * PI * ph).cos();
            let f = 200.0 * 20f32.powf(s * depth);
            let t = (PI * f.min(sr * 0.45) / sr).tan();
            (t - 1.0) / (t + 1.0)
        };
        for (i, (xl, xr)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            if self.tick.is_multiple_of(UPDATE) {
                let ph = c.lfo(i);
                self.coef = [coef(ph), coef((ph + spread).rem_euclid(1.0))];
            }
            self.tick = self.tick.wrapping_add(1);
            for (ch, x) in [xl, xr].into_iter().enumerate() {
                let a = self.coef[ch];
                let mut v = *x + self.ap_out[ch] * fb;
                for st in &mut self.ap[ch] {
                    let y = a * v + st.0 - a * st.1;
                    st.0 = v;
                    st.1 = y;
                    v = y;
                }
                self.ap_out[ch] = soft_sat(v);
                *x = 0.6 * (*x + v);
            }
        }
    }

    fn chorus(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let depth = c.params[0] * 0.006 * self.sr;
        let spread = c.params[1];
        let level = c.params[2];
        let base = 0.012 * self.sr;
        for (i, (xl, xr)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let ph = c.lfo(i);
            let s1 = (2.0 * PI * ph).sin();
            let s2 = (2.0 * PI * (ph + 0.33)).sin();
            let (al, ar) = self.ring.read_delay(base + depth * (0.5 + 0.5 * s1));
            let (bl, br) = self.ring.read_delay(base * 1.3 + depth * (0.5 + 0.5 * s2));
            self.ring.push(*xl, *xr);
            let near = 1.0 - 0.5 * spread;
            let far = 0.5 * spread;
            let vl = al * near + bl * far;
            let vr = br * near + ar * far;
            *xl = *xl * 0.8 + vl * level * 0.7;
            *xr = *xr * 0.8 + vr * level * 0.7;
        }
    }

    fn filter(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let high = c.params[1] >= 0.5;
        let cut = if high {
            20.0 * 500f32.powf(c.params[0])
        } else {
            20_000.0 * (100.0f32 / 20_000.0).powf(c.params[0])
        };
        let q = 0.7 + c.params[2] * 8.0;
        for s in &mut self.svf {
            s.set(f64::from(cut), f64::from(q), f64::from(self.sr));
        }
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            let ol = self.svf[0].process(*xl);
            let or = self.svf[1].process(*xr);
            if high {
                *xl = ol.high;
                *xr = or.high;
            } else {
                *xl = ol.low;
                *xr = or.low;
            }
        }
    }

    fn band_sweep(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let depth = c.params[0];
        let q = 0.7 + c.params[1] * 6.0;
        let centre = 150.0 * 53f32.powf(c.params[2]);
        let norm = 1.4 / q;
        for (i, (xl, xr)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            if self.tick.is_multiple_of(UPDATE) {
                let s = 0.5 - 0.5 * (2.0 * PI * c.lfo(i)).cos();
                let f = centre * 2f32.powf((s - 0.5) * 5.0 * depth);
                for sv in &mut self.svf {
                    sv.set(f64::from(f), f64::from(q), f64::from(self.sr));
                }
            }
            self.tick = self.tick.wrapping_add(1);
            *xl = self.svf[0].process(*xl).band * norm;
            *xr = self.svf[1].process(*xr).band * norm;
        }
    }

    #[inline]
    fn white(&mut self) -> f32 {
        // xorshift32
        let mut x = self.noise;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.noise = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }

    fn noise_sweep(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let level = c.params[0] * 0.5;
        let q = 0.8 + c.params[1] * 6.0;
        let down = c.params[2] >= 0.5;
        let norm = 1.0 / q;
        for (i, (xl, xr)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let ramp = c.lfo(i);
            let pos = if down { 1.0 - ramp } else { ramp };
            if self.tick.is_multiple_of(UPDATE) {
                let f = 200.0 * 60f32.powf(pos);
                for sv in &mut self.svf {
                    sv.set(f64::from(f), f64::from(q), f64::from(self.sr));
                }
            }
            self.tick = self.tick.wrapping_add(1);
            let swell = level * pos.max(0.05);
            let nl = self.white();
            let nr = self.white();
            *xl += self.svf[0].process(nl).band * norm * swell;
            *xr += self.svf[1].process(nr).band * norm * swell;
        }
    }

    fn gate(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        // Open for the first `duty` of every step, closed after; each edge
        // is a raised-cosine ramp (3 ms by default, the Smooth knob runs
        // 1 to 12 ms), so the chops never click.
        let depth = c.params[0];
        let duty = 0.1 + 0.8 * c.params[1];
        let period = c.period() as f32;
        let open_len = duty * period;
        let w = ((1.0 + c.params[2] * 11.0) * 0.001 * self.sr)
            .min(open_len * 0.5)
            .min((period - open_len) * 0.5)
            .max(1.0);
        for (i, (xl, xr)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let pos = c.lfo(i) * period;
            let open = if pos < w {
                0.5 - 0.5 * (PI * pos / w).cos()
            } else if pos < open_len - w {
                1.0
            } else if pos < open_len {
                0.5 - 0.5 * (PI * (open_len - pos) / w).cos()
            } else {
                0.0
            };
            self.gain = 1.0 - depth * (1.0 - open);
            *xl *= self.gain;
            *xr *= self.gain;
        }
    }

    fn auto_pan(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let depth = c.params[0];
        let shape = c.params[1];
        let k = 1.0 - (-1.0 / (0.003 * self.sr)).exp();
        for (i, (xl, xr)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            let s = (2.0 * PI * c.lfo(i)).sin();
            let s = s * (1.0 - shape) + s.signum() * shape;
            self.pan += (s * depth - self.pan) * k;
            let a = (self.pan + 1.0) * PI * 0.25;
            *xl *= a.cos() * std::f32::consts::SQRT_2;
            *xr *= a.sin() * std::f32::consts::SQRT_2;
        }
    }

    /// Short fade at the edges of a slice of `len` frames.
    #[inline]
    fn edge(pos: u64, len: u64, w: f32) -> f32 {
        let a = (pos as f32 + 1.0) / w;
        let b = (len.saturating_sub(pos)) as f32 / w;
        a.min(b).min(1.0)
    }

    fn beat_roll(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let len = self.loop_len(c);
        let fade = 1.0 - c.params[0] * 0.4;
        // The last 2 ms of every pass crossfade into the audio that led up
        // to the loop start, so the seam is continuous (no dip, no click).
        let x = ((0.002 * self.sr) as u64).clamp(1, len / 4);
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            if self.ring.written - self.start < len {
                self.ring.push(*xl, *xr);
            }
            if self.t >= len {
                let k = self.t % len;
                let reps = (self.t / len).min(64) as i32;
                let (mut yl, mut yr) = self.ring.at((self.start + k) as i64);
                let mut g = fade.powi(reps);
                if k + x >= len {
                    let w = (k + x + 1 - len) as f32 / (x + 1) as f32;
                    if self.seam_ok {
                        let (pl, pr) = self.ring.at(self.start as i64 + k as i64 - len as i64);
                        yl += w * (pl - yl);
                        yr += w * (pr - yr);
                    } else {
                        g *= 1.0 - w;
                    }
                } else if !self.seam_ok && k < x {
                    g *= (k + 1) as f32 / (x + 1) as f32;
                }
                *xl = yl * g;
                *xr = yr * g;
            }
            self.t += 1;
        }
    }

    fn slicer(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let s = self.loop_len(c);
        let window = s * 8;
        let pattern =
            SLICER_PATTERNS[((c.params[0] * SLICER_PATTERNS.len() as f32) as usize).min(4)];
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            if self.ring.written - self.start < window {
                self.ring.push(*xl, *xr);
            }
            if self.t >= window {
                let k = ((self.t / s) % 8) as usize;
                let w = self.t % s;
                let src = u64::from(pattern[k]) * s + w;
                let g = Self::edge(w, s, 48.0);
                let (yl, yr) = self.ring.at((self.start + src) as i64);
                *xl = yl * g;
                *xr = yr * g;
            }
            self.t += 1;
        }
    }

    fn reverse(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let len = self.loop_len(c);
        let w = ((0.002 + c.params[0] * 0.04) * self.sr).min(len as f32 * 0.5);
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            self.ring.push(*xl, *xr);
            if self.t >= len {
                self.start = self.ring.written;
                self.t = 0;
            }
            let (yl, yr) = self.ring.at(self.start as i64 - 1 - self.t as i64);
            let g = Self::edge(self.t, len, w);
            *xl = yl * g;
            *xr = yr * g;
            self.t += 1;
        }
    }

    fn brake(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32], tape: bool) {
        let dur = (c.period() as f32).clamp(64.0, self.max_delay());
        let curve = 1.0 + 2.0 * c.params[0];
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            self.ring.push(*xl, *xr);
            if self.rate <= 0.0 {
                *xl = 0.0;
                *xr = 0.0;
                continue;
            }
            let (mut yl, mut yr) = self.ring.read_at(self.read);
            self.t += 1;
            let tt = (self.t as f32 / dur).min(1.0);
            self.rate = if tape {
                1.0 - tt * tt
            } else {
                (1.0 - tt).powf(curve)
            };
            self.read += f64::from(self.rate);
            if tape {
                // Darker as it slows down.
                let k = 0.02 + 0.98 * self.rate.powf(1.0 + 3.0 * c.params[0]);
                self.lp[0] += (yl - self.lp[0]) * k;
                self.lp[1] += (yr - self.lp[1]) * k;
                yl = self.lp[0];
                yr = self.lp[1];
            }
            *xl = yl;
            *xr = yr;
        }
    }

    fn bit_crusher(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let bits = 16.0 - c.params[0] * 13.0;
        let levels = 2f32.powf(bits - 1.0);
        let hold = 1 + (c.params[1] * c.params[1] * 31.0) as u32;
        let level = c.params[2] * 1.25;
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            if self.hold_left == 0 {
                self.hold = [
                    (*xl * levels).round() / levels,
                    (*xr * levels).round() / levels,
                ];
                self.hold_left = hold;
            }
            self.hold_left -= 1;
            *xl = self.hold[0] * level;
            *xr = self.hold[1] * level;
        }
    }

    fn overdrive(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let drive = 1.0 + c.params[0] * 24.0;
        let tone = 0.05 + 0.95 * c.params[1];
        let level = c.params[2] * 1.2 / drive.sqrt();
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            let yl = (*xl * drive).tanh();
            let yr = (*xr * drive).tanh();
            self.lp[0] += (yl - self.lp[0]) * tone;
            self.lp[1] += (yr - self.lp[1]) * tone;
            *xl = self.lp[0] * level;
            *xr = self.lp[1] * level;
        }
    }

    fn pitch_shift(&mut self, c: &FxContext, l: &mut [f32], r: &mut [f32]) {
        let semis = (c.params[0] - 0.5) * 24.0;
        let ratio = 2f32.powf(semis / 12.0);
        let w = ((0.02 + c.params[1] * 0.08) * self.sr).min(self.max_delay() - 8.0);
        let step = (1.0 - ratio) / w;
        for (xl, xr) in l.iter_mut().zip(r.iter_mut()) {
            self.ring.push(*xl, *xr);
            self.grain = (self.grain + step).rem_euclid(1.0);
            let p2 = (self.grain + 0.5).rem_euclid(1.0);
            let (al, ar) = self.ring.read_delay(4.0 + self.grain * w);
            let (bl, br) = self.ring.read_delay(4.0 + p2 * w);
            let wa = (PI * self.grain).sin().powi(2);
            let wb = 1.0 - wa;
            *xl = al * wa + bl * wb;
            *xr = ar * wa + br * wb;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn ctx(params: [f32; 3]) -> FxContext {
        FxContext {
            beat_frames: 24_000.0, // 120 BPM
            beats: 0.5,
            bar_beat: 0.0,
            params,
            feed: true,
            pass_from: 1.0,
            pass_to: 1.0,
        }
    }

    fn sine(n: usize, f: f32) -> Vec<f32> {
        (0..n)
            .map(|i| 0.5 * (2.0 * PI * f * i as f32 / SR as f32).sin())
            .collect()
    }

    /// Runs `blocks` blocks of 256 frames of `input` (looped) through an
    /// effect and returns the output.
    fn run(e: &mut Effect, c: FxContext, input: &[f32], frames: usize) -> (Vec<f32>, Vec<f32>) {
        let mut out_l = Vec::with_capacity(frames);
        let mut out_r = Vec::with_capacity(frames);
        let mut done = 0;
        let mut c = c;
        while done < frames {
            let n = 256.min(frames - done);
            let mut l: Vec<f32> = (0..n).map(|i| input[(done + i) % input.len()]).collect();
            let mut r = l.clone();
            e.process(&c, &mut l, &mut r);
            out_l.extend_from_slice(&l);
            out_r.extend_from_slice(&r);
            c.bar_beat += n as f64 / c.beat_frames;
            done += n;
        }
        (out_l, out_r)
    }

    #[test]
    fn every_effect_stays_finite_and_bounded() {
        let input = sine(SR as usize, 220.0);
        for kind in EffectKind::ALL {
            for params in [[0.0; 3], [1.0; 3], kind.defaults()] {
                let mut e = Effect::new(SR);
                e.set_kind(kind);
                let c = ctx(params);
                e.warm(&input[..4096], &input[..4096]);
                e.start(&c);
                let (l, r) = run(&mut e, c, &input, SR as usize);
                let peak = l.iter().chain(r.iter()).fold(0.0f32, |m, x| {
                    assert!(x.is_finite(), "{kind:?} {params:?}");
                    m.max(x.abs())
                });
                assert!(peak < 4.0, "{kind:?} {params:?}: peak {peak}");
            }
        }
    }

    #[test]
    fn ids_round_trip() {
        for k in EffectKind::ALL {
            assert_eq!(EffectKind::from_id(k.id()), Some(k));
        }
        assert_eq!(EffectKind::ALL.len(), 20);
    }

    #[test]
    fn echo_repeats_after_the_beats_setting() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::Echo);
        // One click, then silence: echoes every half beat at 120 BPM.
        let mut input = vec![0.0f32; SR as usize];
        input[0] = 1.0;
        let c = ctx([0.5, 1.0, 1.0]);
        e.start(&c);
        let (l, _) = run(&mut e, c, &input, SR as usize);
        let first = l[1..].iter().position(|x| x.abs() > 0.3).map(|p| p + 1);
        assert_eq!(
            first.map(|p| (p as i64 - 12_000).abs() <= 2),
            Some(true),
            "{first:?}"
        );
    }

    #[test]
    fn echo_tail_dies_away_without_feed() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::Echo);
        let input = sine(SR as usize, 440.0);
        let mut c = ctx([0.6, 0.7, 1.0]);
        e.start(&c);
        run(&mut e, c, &input, SR as usize / 2);
        c.feed = false;
        // Silence in, echoes out, getting quieter.
        let silence = vec![0.0f32; 1024];
        let (l, _) = run(&mut e, c, &silence, 12_000);
        assert!(l.iter().any(|x| x.abs() > 0.05), "a tail");
        let (l, _) = run(&mut e, c, &silence, SR as usize * 20);
        let end = l[l.len() - 4800..]
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(end < 1e-3, "dies away: {end}");
    }

    #[test]
    fn gate_follows_the_beat() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::Gate);
        let input = vec![0.5f32; 1024];
        let c = ctx([1.0, 0.5, 0.0]);
        e.start(&c);
        // Half beat at 120 BPM = 12 000 frames: open for the first half.
        let (l, _) = run(&mut e, c, &input, 24_000);
        assert!(l[3_000] > 0.45, "open: {}", l[3_000]);
        assert!(l[9_000] < 0.05, "closed: {}", l[9_000]);
        assert!(l[15_000] > 0.45, "open again: {}", l[15_000]);
    }

    #[test]
    fn beat_roll_repeats_the_captured_audio() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::BeatRoll);
        let ramp: Vec<f32> = (0..SR as usize).map(|i| i as f32 / SR as f32).collect();
        let c = ctx([0.0; 3]);
        e.start(&c);
        let (l, _) = run(&mut e, c, &ramp, 36_000);
        // The first loop (12 000 frames) passes the input; then it repeats.
        assert!((l[6_000] - ramp[6_000]).abs() < 1e-6);
        assert!((l[18_000] - ramp[6_000]).abs() < 1e-6, "{}", l[18_000]);
        assert!((l[30_000] - ramp[6_000]).abs() < 1e-6);
    }

    #[test]
    fn filter_low_pass_removes_highs() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::Filter);
        let input = sine(SR as usize, 8_000.0);
        let c = ctx([0.9, 0.0, 0.0]);
        e.start(&c);
        let (l, _) = run(&mut e, c, &input, SR as usize / 4);
        let peak = l[4_000..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak < 0.01, "{peak}");
    }

    #[test]
    fn pitch_shift_up_an_octave_doubles_the_frequency() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::PitchShifter);
        let input = sine(SR as usize, 300.0);
        let c = ctx([1.0, 0.4, 0.0]);
        e.start(&c);
        let (l, _) = run(&mut e, c, &input, SR as usize);
        // Count zero crossings over the last half second.
        let tail = &l[SR as usize / 2..];
        let crossings = tail
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!(
            (280..=320).contains(&crossings),
            "{crossings} (600 Hz expected)"
        );
    }

    #[test]
    fn brake_ends_in_silence() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::VinylBrake);
        let input = sine(SR as usize, 440.0);
        e.warm(&input[..8192], &input[..8192]);
        let c = ctx([0.0; 3]);
        e.start(&c);
        let (l, _) = run(&mut e, c, &input, 30_000);
        assert!(
            l[1_000].abs() > 0.0 || l[1_001].abs() > 0.0,
            "plays at first"
        );
        assert!(
            l[20_000..].iter().all(|x| *x == 0.0),
            "stopped after the half beat"
        );
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    #[test]
    fn echo_repeats_lose_their_bass() {
        // A 40 Hz tone through the echo: the repeats are high-passed, so
        // after the input stops the tail is far quieter than for 1 kHz.
        let tail_level = |freq: f32| {
            let mut e = Effect::new(SR);
            e.set_kind(EffectKind::Echo);
            let mut c = ctx([0.7, 1.0, 1.0]);
            e.start(&c);
            run(&mut e, c, &sine(SR as usize, freq), SR as usize / 2);
            c.feed = false;
            let (l, _) = run(&mut e, c, &vec![0.0; 1024], SR as usize);
            rms(&l)
        };
        let bass = tail_level(40.0);
        let mid = tail_level(1000.0);
        assert!(bass < mid * 0.2, "bass {bass} vs mid {mid}");
    }

    #[test]
    fn reverb_has_no_sub_bass_and_dies_away() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::Reverb);
        let mut c = ctx([0.5, 0.5, 1.0]);
        e.start(&c);
        // 30 Hz in: the wet output (output minus dry) stays small.
        let input = sine(SR as usize, 30.0);
        let (l, _) = run(&mut e, c, &input, SR as usize);
        let wet: Vec<f32> = l
            .iter()
            .zip(input.iter())
            .map(|(y, x)| y - x * Effect::reverb_dry(1.0))
            .collect();
        assert!(rms(&wet[SR as usize / 2..]) < 0.05, "{}", rms(&wet));
        // Noise in: a clear wet signal, then silence once fed nothing.
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::Reverb);
        e.start(&c);
        let noise: Vec<f32> = (0..SR as usize)
            .map(|i| ((i as f32 * 12.9898).sin() * 43_758.547).fract() - 0.5)
            .collect();
        let (l, _) = run(&mut e, c, &noise, SR as usize);
        let wet: Vec<f32> = l
            .iter()
            .zip(noise.iter())
            .map(|(y, x)| y - x * Effect::reverb_dry(1.0))
            .collect();
        let level = rms(&wet[SR as usize / 2..]) / rms(&noise);
        assert!(level > 0.45 && level < 1.5, "wet level {level}");
        c.feed = false;
        let (l, _) = run(&mut e, c, &vec![0.0; 1024], SR as usize * 12);
        assert!(rms(&l[l.len() - 4800..]) < 1e-3);
    }

    #[test]
    fn gate_edges_do_not_click() {
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::Gate);
        let input = vec![0.5f32; 1024];
        let c = ctx([1.0, 0.5, 0.2]);
        e.start(&c);
        let (l, _) = run(&mut e, c, &input, 48_000);
        let step = l.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        // A 3 ms raised cosine on 0.5: at most about 0.5 * pi / 2 / 150.
        assert!(step < 0.006, "{step}");
    }

    #[test]
    fn beat_roll_seam_is_continuous() {
        // A slow sine captured with history: across every seam the output
        // moves no more than the sine itself does.
        let mut e = Effect::new(SR);
        e.set_kind(EffectKind::BeatRoll);
        let input = sine(SR as usize * 2, 110.0);
        e.warm(&input[..24_000], &input[..24_000]);
        let mut c = ctx([0.0; 3]);
        c.bar_beat = 0.0;
        e.start(&c);
        let (l, _) = run(&mut e, c, &input[24_000..], 60_000);
        let max_in = input
            .windows(2)
            .fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        let max_out = l.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(max_out < max_in * 3.0, "out {max_out} vs in {max_in}");
    }

    #[test]
    fn flanger_feedback_is_bipolar() {
        for fb_knob in [0.0, 0.5, 1.0] {
            let mut e = Effect::new(SR);
            e.set_kind(EffectKind::Flanger);
            let c = ctx(EffectKind::Flanger.internal([1.0, fb_knob, 0.3]).0);
            e.start(&c);
            let (l, r) = run(&mut e, c, &sine(SR as usize, 500.0), SR as usize);
            assert!(
                l.iter()
                    .chain(r.iter())
                    .all(|v| v.is_finite() && v.abs() < 2.0)
            );
            // Stereo: the two sides sweep differently.
            assert!(l.iter().zip(r.iter()).any(|(a, b)| (a - b).abs() > 0.01));
        }
    }
}
