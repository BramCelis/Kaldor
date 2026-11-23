//! Color FX on the channel filter knob, as on a DJM mixer: one
//! effect chosen for all channels; each channel's FILTER
//! knob plays it. Knob in the middle: off (the dry track); turned either
//! way: the effect comes in, more the further it is turned.
//!
//! What each does follows the DJM-900NXS2 operating instructions (the
//! behaviour only; nothing else is taken from Pioneer):
//!
//! - **Space**: reverb added to the track. Left: on the mid and low range;
//!   right: on the mid and high range. The track itself stays as it is;
//!   the tail rings out when the knob returns.
//! - **Dub echo**: an echo (3/4 beat) whose repeats fade away. Left: the
//!   repeats are mid-range only; right: high range only. They ring out
//!   when the knob returns.
//! - **Sweep**: left makes the sound tighter: each hit opens it, then the
//!   decay fades out sooner the further left (smooth, no chatter); right a
//!   band-pass whose band narrows.
//! - **Noise**: a riser behind the track: white noise
//!   that gets louder the further the knob is turned; right a resonant
//!   high-pass sweeps up through it, left a low-pass sweeps down. The
//!   track itself is not filtered.
//! - **Crush**: left more and more distortion (bit and sample-rate
//!   reduction); right crushed, then through a rising high-pass.
//!
//! Everything is allocated in [`ColorProcessor::new`]; processing never
//! allocates.

use crate::fx::{REVERB_WET_GAIN, Reverb};
use crate::svf::{DjFilter, FILTER_DEAD_ZONE, Svf};

/// What the filter knob does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColorFx {
    /// The plain filter (no color FX).
    #[default]
    Filter,
    Space,
    DubEcho,
    Sweep,
    Noise,
    Crush,
}

impl ColorFx {
    pub const ALL: [ColorFx; 6] = [
        ColorFx::Filter,
        ColorFx::Space,
        ColorFx::DubEcho,
        ColorFx::Sweep,
        ColorFx::Noise,
        ColorFx::Crush,
    ];

    /// Name for the screen.
    pub fn name(self) -> &'static str {
        match self {
            ColorFx::Filter => "FILTER",
            ColorFx::Space => "SPACE",
            ColorFx::DubEcho => "DUB ECHO",
            ColorFx::Sweep => "SWEEP",
            ColorFx::Noise => "NOISE",
            ColorFx::Crush => "CRUSH",
        }
    }
}

/// Amount glide (seconds), so stepped MIDI values sweep smoothly.
const GLIDE_SECONDS: f32 = 0.012;
/// A tail counts as gone after this long below [`SILENT`].
const TAIL_SILENT_SECONDS: f32 = 0.5;
const SILENT: f32 = 1e-5;

/// Space: decay (seconds to -60 dB), damping, and the reverb's level (at
/// 0.8 of the turn its tail is about 0.6 of the track's level: a room
/// around the track, not over it).
const SPACE_RT60: f32 = 2.6;
const SPACE_DAMP_HZ: f32 = 6500.0;
const SPACE_LEVEL: f32 = 0.2;
/// Space: the band the reverb works on (left: below, right: above).
const SPACE_LOW_HZ: f64 = 2500.0;
const SPACE_HIGH_HZ: f64 = 600.0;

/// Dub echo: delay in beats, the longest delay held, feedback and the
/// level of the repeats.
const DUB_ECHO_BEATS: f64 = 0.75;
const ECHO_MAX_SECONDS: f32 = 3.0;
const ECHO_FEEDBACK: f32 = 0.62;
const ECHO_LEVEL: f32 = 0.8;
/// Dub echo bands: left mid-range only, right high range only.
const ECHO_MID_HZ: (f64, f64) = (350.0, 2600.0);
const ECHO_HIGH_HZ: f64 = 2000.0;

/// Noise riser: the noise's peak level at the end of the turn (about
/// -16 dBFS RMS before its filter: a strong build-up, still under a
/// mastered track). It rises as the turn to the power 1.5, so a third of
/// the way it is about 14 dB lower.
const NOISE_LEVEL: f32 = 0.28;

/// One channel's color FX.
pub struct ColorProcessor {
    sr: f32,
    kind: ColorFx,
    /// How far the knob is out of the middle (0 to 1), gliding.
    amount: f32,
    /// The side the knob was last turned to (true: right).
    right: bool,
    /// The plain filter.
    filter: DjFilter,
    reverb: Reverb,
    /// Dub echo delay line.
    echo_l: Vec<f32>,
    echo_r: Vec<f32>,
    echo_pos: usize,
    echo_dirty: bool,
    /// Smoothed delay in frames (0: not set yet).
    delay: f32,
    /// Two filter stages per channel (send / band / noise filters).
    f1: [Svf; 2],
    f2: [Svf; 2],
    /// Space and dub echo: a tail is ringing; frames it has been silent.
    tail: bool,
    silent: u32,
    /// Sweep gate: fast envelope, recent peak, gain and hold.
    env: f32,
    peak: f32,
    gate: f32,
    /// Sweep gate: ready for the next hit; what is left of the last one.
    armed: bool,
    sustain: f32,
    /// Crush: the held sample and frames until the next one.
    crush: [f32; 2],
    crush_left: u32,
    noise: u32,
}

impl ColorProcessor {
    /// `_max_block` is kept for the mixer's call; nothing here needs a
    /// block buffer.
    pub fn new(sample_rate: u32, _max_block: usize) -> Self {
        let sr = sample_rate.max(1) as f32;
        let echo = (ECHO_MAX_SECONDS * sr) as usize;
        Self {
            sr,
            kind: ColorFx::Filter,
            amount: 0.0,
            right: false,
            filter: DjFilter::new(sample_rate),
            reverb: Reverb::new(sample_rate),
            echo_l: crate::prefaulted_zeros(echo),
            echo_r: crate::prefaulted_zeros(echo),
            echo_pos: 0,
            echo_dirty: false,
            delay: 0.0,
            f1: [Svf::default(); 2],
            f2: [Svf::default(); 2],
            tail: false,
            silent: 0,
            env: 0.0,
            peak: 0.0,
            gate: 1.0,
            armed: true,
            sustain: 1.0,
            crush: [0.0; 2],
            crush_left: 0,
            noise: 0x2545_F491,
        }
    }

    /// The color FX in use.
    pub fn kind(&self) -> ColorFx {
        self.kind
    }

    /// Starts `kind` clean: nothing ringing, the knob gliding in from 0.
    fn switch_to(&mut self, kind: ColorFx) {
        self.kind = kind;
        self.amount = 0.0;
        self.filter = DjFilter::new(self.sr as u32);
        self.reverb.clear();
        if self.echo_dirty {
            self.echo_l.fill(0.0);
            self.echo_r.fill(0.0);
            self.echo_dirty = false;
        }
        self.delay = 0.0;
        for s in self.f1.iter_mut().chain(self.f2.iter_mut()) {
            s.reset();
        }
        self.tail = false;
        self.silent = 0;
        self.env = 0.0;
        self.peak = 0.0;
        self.gate = 1.0;
        self.armed = true;
        self.sustain = 1.0;
        self.crush_left = 0;
    }

    /// Processes one block of a channel in place. `knob` is the filter
    /// knob (-1 to 1); `beat` the deck's (BPM, bar-aligned beat) when it
    /// has a beatgrid, else `fallback_bpm` is used (dub echo time).
    pub fn process(
        &mut self,
        kind: ColorFx,
        knob: f32,
        beat: Option<(f64, f64)>,
        fallback_bpm: f64,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        if kind != self.kind {
            self.switch_to(kind);
        }
        let n = l.len().min(r.len());
        let (Some(l), Some(r)) = (l.get_mut(..n), r.get_mut(..n)) else {
            return;
        };
        if n == 0 {
            return;
        }
        let knob = knob.clamp(-1.0, 1.0);
        if self.kind == ColorFx::Filter {
            self.filter.set(knob);
            self.filter.process(l, r);
            return;
        }
        let target = ((knob.abs() - FILTER_DEAD_ZONE) / (1.0 - FILTER_DEAD_ZONE)).clamp(0.0, 1.0);
        if target > 0.0 {
            self.right = knob > 0.0;
        }
        // The amount glides from `a0` to `a1` over the block.
        let a0 = self.amount;
        let k = 1.0 - (-(n as f32) / (GLIDE_SECONDS * self.sr)).exp();
        let mut a1 = a0 + (target - a0) * k;
        if (a1 - target).abs() < 1e-4 {
            a1 = target;
        }
        self.amount = a1;
        let bpm = beat.map_or(fallback_bpm, |b| b.0).clamp(20.0, 400.0);
        let beat_frames = 60.0 * f64::from(self.sr) / bpm;
        match self.kind {
            ColorFx::Filter => {}
            ColorFx::Space => self.space(a0, a1, l, r),
            ColorFx::DubEcho => self.dub_echo(a0, a1, beat_frames, l, r),
            ColorFx::Sweep => self.sweep(a0, a1, l, r),
            ColorFx::Noise => self.noise_fx(a0, a1, l, r),
            ColorFx::Crush => self.crush_fx(a0, a1, l, r),
        }
    }

    fn set_pair(s: &mut [Svf; 2], hz: f64, q: f64, sr: f32) {
        for f in s {
            f.set(hz, q, f64::from(sr));
        }
    }

    /// Tail bookkeeping after a block of Space or dub echo: a tail runs
    /// while the knob is out, and stops once it has been silent a while.
    fn tail_after(&mut self, a1: f32, peak: f32, n: usize) {
        if a1 > 0.0 {
            self.tail = true;
            self.silent = 0;
        } else if peak < SILENT {
            self.silent = self.silent.saturating_add(n as u32);
            if self.silent as f32 > TAIL_SILENT_SECONDS * self.sr {
                self.tail = false;
                self.silent = 0;
            }
        } else {
            self.silent = 0;
        }
    }

    fn space(&mut self, a0: f32, a1: f32, l: &mut [f32], r: &mut [f32]) {
        if a0 <= 0.0 && a1 <= 0.0 && !self.tail {
            return;
        }
        let n = l.len();
        // The reverb hears (and gives back) only its side's range.
        let (hz, high) = if self.right {
            (SPACE_HIGH_HZ, true)
        } else {
            (SPACE_LOW_HZ, false)
        };
        Self::set_pair(&mut self.f1, hz, 0.707, self.sr);
        Self::set_pair(&mut self.f2, hz, 0.707, self.sr);
        self.reverb.configure(SPACE_RT60, SPACE_DAMP_HZ);
        let g = REVERB_WET_GAIN * SPACE_LEVEL;
        let step = (a1 - a0) / n as f32;
        let mut a = a0;
        let mut peak = 0.0f32;
        let band = |o: crate::svf::SvfOut| if high { o.high } else { o.low };
        for (x, y) in l.iter_mut().zip(r.iter_mut()) {
            a += step;
            let sl = band(self.f1[0].process(*x)) * a;
            let sr = band(self.f1[1].process(*y)) * a;
            let (wl, wr) = self.reverb.process(sl, sr);
            let wl = band(self.f2[0].process(wl)) * g;
            let wr = band(self.f2[1].process(wr)) * g;
            peak = peak.max(wl.abs()).max(wr.abs());
            *x += wl;
            *y += wr;
        }
        self.tail_after(a1, peak, n);
        if !self.tail {
            self.reverb.clear();
        }
    }

    fn dub_echo(&mut self, a0: f32, a1: f32, beat_frames: f64, l: &mut [f32], r: &mut [f32]) {
        if a0 <= 0.0 && a1 <= 0.0 && !self.tail {
            return;
        }
        let n = l.len();
        let cap = self.echo_l.len();
        if cap < 8 {
            return;
        }
        // Left: the repeats mid-range only (a band-pass); right: high
        // range only (a high-pass). In the loop, so each repeat keeps it.
        if self.right {
            Self::set_pair(&mut self.f1, ECHO_HIGH_HZ, 0.707, self.sr);
        } else {
            Self::set_pair(&mut self.f1, ECHO_MID_HZ.0, 0.707, self.sr);
            Self::set_pair(&mut self.f2, ECHO_MID_HZ.1, 0.707, self.sr);
        }
        let target = (beat_frames * DUB_ECHO_BEATS).clamp(16.0, (cap - 4) as f64) as f32;
        if self.delay <= 0.0 {
            self.delay = target;
        }
        let k_slow = 1.0 - (-1.0 / (0.06 * self.sr)).exp();
        let step = (a1 - a0) / n as f32;
        let mut a = a0;
        let mut peak = 0.0f32;
        self.echo_dirty = true;
        for (x, y) in l.iter_mut().zip(r.iter_mut()) {
            a += step;
            self.delay += (target - self.delay) * k_slow;
            // Read `delay` frames back (linear interpolation).
            let back = self.delay.clamp(2.0, (cap - 2) as f32);
            let pos = self.echo_pos as f32 - back;
            let pos = if pos < 0.0 { pos + cap as f32 } else { pos };
            let i = pos.floor() as usize % cap;
            let j = (i + 1) % cap;
            let t = pos - pos.floor();
            let el = self.echo_l[i] + t * (self.echo_l[j] - self.echo_l[i]);
            let er = self.echo_r[i] + t * (self.echo_r[j] - self.echo_r[i]);
            let (bl, br) = if self.right {
                (self.f1[0].process(el).high, self.f1[1].process(er).high)
            } else {
                let hl = self.f1[0].process(el).high;
                let hr = self.f1[1].process(er).high;
                (self.f2[0].process(hl).low, self.f2[1].process(hr).low)
            };
            self.echo_l[self.echo_pos] = soft_sat(*x * a + bl * ECHO_FEEDBACK);
            self.echo_r[self.echo_pos] = soft_sat(*y * a + br * ECHO_FEEDBACK);
            self.echo_pos = (self.echo_pos + 1) % cap;
            peak = peak.max(bl.abs()).max(br.abs());
            *x += bl * ECHO_LEVEL;
            *y += br * ECHO_LEVEL;
        }
        self.tail_after(a1, peak, n);
    }

    fn sweep(&mut self, a0: f32, a1: f32, l: &mut [f32], r: &mut [f32]) {
        if a0 <= 0.0 && a1 <= 0.0 {
            self.gate = 1.0;
            self.armed = true;
            self.sustain = 1.0;
            for s in self.f1.iter_mut().chain(self.f2.iter_mut()) {
                s.reset();
            }
            return;
        }
        let n = l.len();
        let a = a1.max(a0);
        if !self.right {
            // Gate: only what stands out of the recent peak gets through,
            // (the level gate chattered on bass and sounded
            // glitchy). Now the decays are shortened smoothly: each hit
            // (the fast envelope jumping over the slow one) opens it over
            // about 2 ms, then the sound fades with a time constant that
            // gets shorter further left (0.42 s down to 20 ms), towards a
            // floor that gets lower. Driven by hits, not by the level of
            // every cycle, so it never chatters.
            let sr = self.sr;
            let coef = |s: f32| 1.0 - (-1.0 / (s * sr)).exp();
            let (fast_a, fast_r) = (coef(0.0005), coef(0.012));
            let (slow_a, slow_r) = (coef(0.04), coef(0.25));
            // Fast enough to keep a kick's punch, slow enough not to click.
            let open_k = coef(0.0005);
            let close_k = coef(0.001);
            let tau = 0.02 + 0.4 * (1.0 - a) * (1.0 - a);
            let fade = (-1.0 / (tau * sr)).exp();
            // The floor follows the knob sample by sample (no steps).
            let step = (a1 - a0) / n as f32;
            let mut m = a0;
            for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                m += step;
                let floor = 1.0 - 0.95 * (m * 1.5).min(1.0);
                let e = x.abs().max(y.abs());
                let kf = if e > self.env { fast_a } else { fast_r };
                self.env += (e - self.env) * kf;
                let ks = if e > self.peak { slow_a } else { slow_r };
                self.peak += (e - self.peak) * ks;
                // A hit: the fast envelope clearly over the slow one; armed
                // again once it has fallen back (no double triggers).
                if self.armed && self.env > self.peak * 1.5 + 1e-4 {
                    self.armed = false;
                    self.sustain = 1.0;
                } else if !self.armed && self.env < self.peak * 1.15 {
                    self.armed = true;
                }
                self.sustain *= fade;
                let target = floor + (1.0 - floor) * self.sustain;
                // Opens over ~2 ms; closes with the fade (smoothed 1 ms).
                let k = if target > self.gate { open_k } else { close_k };
                self.gate += (target - self.gate) * k;
                *x *= self.gate;
                *y *= self.gate;
            }
        } else {
            // Band-pass whose band narrows: the low edge rises from 20 Hz
            // to 700 Hz and the high edge falls from 20 kHz to 2.2 kHz
            // (around 1.2 kHz), a little resonant further in.
            let lo = 20.0 * (700.0f64 / 20.0).powf(f64::from(a));
            let hi = 20_000.0 * (2200.0f64 / 20_000.0).powf(f64::from(a));
            let q = 0.75 + 0.9 * f64::from(a);
            Self::set_pair(&mut self.f1, lo, q, self.sr);
            Self::set_pair(&mut self.f2, hi, q, self.sr);
            let step = (a1 - a0) / n as f32;
            let mut m = a0;
            for (x, y) in l.iter_mut().zip(r.iter_mut()) {
                m += step;
                let mix = (m * 6.0).min(1.0);
                let bl = self.f2[0].process(self.f1[0].process(*x).high).low;
                let br = self.f2[1].process(self.f1[1].process(*y).high).low;
                *x += mix * (bl - *x);
                *y += mix * (br - *y);
            }
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

    /// A riser behind the track: the further the knob is
    /// turned, the louder the noise. Right: a slightly resonant high-pass
    /// sweeps up through it (300 Hz to 9 kHz), the build-up "whoosh" before
    /// a drop. Left: a low-pass sweeps down (12 kHz to 500 Hz), a falling
    /// wash. The track itself is left alone.
    fn noise_fx(&mut self, a0: f32, a1: f32, l: &mut [f32], r: &mut [f32]) {
        if a0 <= 0.0 && a1 <= 0.0 {
            return;
        }
        let n = l.len();
        let step = (a1 - a0) / n as f32;
        let mut m = a0;
        let high = self.right;
        for (i, (x, y)) in l.iter_mut().zip(r.iter_mut()).enumerate() {
            m += step;
            if i % 16 == 0 {
                let a = f64::from(m);
                let hz = if high {
                    300.0 * (9000.0f64 / 300.0).powf(a)
                } else {
                    12_000.0 * (500.0f64 / 12_000.0).powf(a)
                };
                Self::set_pair(&mut self.f1, hz, 1.6, self.sr);
            }
            // Rises with the turn (soft at first, full at the end). Never
            // below 0: a power of a rounding error under 0 is NaN.
            let level = NOISE_LEVEL * m.max(0.0).powf(1.5);
            let (nl, nr) = (self.white() * level, self.white() * level);
            let (ol, or) = (self.f1[0].process(nl), self.f1[1].process(nr));
            *x += if high { ol.high } else { ol.low };
            *y += if high { or.high } else { or.low };
        }
    }

    fn crush_fx(&mut self, a0: f32, a1: f32, l: &mut [f32], r: &mut [f32]) {
        if a0 <= 0.0 && a1 <= 0.0 {
            self.crush_left = 0;
            return;
        }
        let n = l.len();
        let a = a1.max(a0);
        // Bits from 16 down to 5, sample hold from 1 to 16 frames, and a
        // little drive, all growing with the turn.
        let levels = 2f32.powf(15.0 - 11.0 * a.powf(0.6));
        let hold = 1 + (15.0 * a.powf(1.6)) as u32;
        let drive = 1.0 + 2.0 * a;
        let norm = drive.powf(-0.6);
        if self.right {
            // Crushed, then a high-pass rising from 20 Hz to 2.5 kHz.
            let hz = 20.0 * (2500.0f64 / 20.0).powf(f64::from(a));
            Self::set_pair(&mut self.f1, hz, 0.707, self.sr);
        }
        let step = (a1 - a0) / n as f32;
        let mut m = a0;
        for (x, y) in l.iter_mut().zip(r.iter_mut()) {
            m += step;
            if self.crush_left == 0 {
                let c = |v: f32| ((v * drive).tanh() * norm * levels).round() / levels;
                self.crush = [c(*x), c(*y)];
                self.crush_left = hold;
            }
            self.crush_left -= 1;
            let (mut cl, mut cr) = (self.crush[0], self.crush[1]);
            if self.right {
                cl = self.f1[0].process(cl).high;
                cr = self.f1[1].process(cr).high;
            }
            // Fully crushed a tenth of the way out.
            let mix = (m * 10.0).min(1.0);
            *x += mix * (cl - *x);
            *y += mix * (cr - *y);
        }
    }
}

/// Soft saturation for the echo loop: identity up to 0.6, then a `tanh`
/// knee towards 1.0.
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

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
    }

    /// A drum-like test signal: a low and a high tone (220 Hz and 3.5 kHz,
    /// so either filter side lets something through) struck every quarter
    /// of a beat at 120 BPM and decaying, so the gate has hits and tails.
    fn signal(t: usize) -> f32 {
        let w = |hz: f32| (t as f32 * hz * 2.0 * std::f32::consts::PI / SR as f32).sin();
        let since = (t % 6000) as f32 / SR as f32;
        (0.5 * w(220.0) + 0.3 * w(3520.0)) * (-since * 12.0).exp()
    }

    /// Runs `blocks` blocks of the test signal through `kind` with the
    /// knob at `knob`; returns the left output.
    fn run(p: &mut ColorProcessor, kind: ColorFx, knob: f32, blocks: usize) -> Vec<f32> {
        let mut out = Vec::new();
        for b in 0..blocks {
            let mut l: Vec<f32> = (0..256).map(|i| signal(b * 256 + i)).collect();
            let mut r = l.clone();
            p.process(kind, knob, Some((120.0, 0.0)), 120.0, &mut l, &mut r);
            out.extend(l);
        }
        out
    }

    fn dry(blocks: usize) -> Vec<f32> {
        (0..blocks * 256).map(signal).collect()
    }

    #[test]
    fn the_middle_of_the_knob_leaves_the_track_alone() {
        let d = dry(40);
        for kind in ColorFx::ALL {
            let out = run(&mut ColorProcessor::new(SR, 512), kind, 0.0, 40);
            let diff = out
                .iter()
                .zip(d.iter())
                .map(|(a, b)| (a - b).abs())
                .fold(0.0f32, f32::max);
            assert!(diff < 1e-5, "{kind:?}: {diff}");
        }
    }

    #[test]
    fn each_effect_changes_the_sound_both_ways_without_getting_loud() {
        let d = dry(120);
        let skip = 256 * 20;
        for kind in ColorFx::ALL.into_iter().skip(1) {
            for knob in [-0.8, 0.8] {
                let out = run(&mut ColorProcessor::new(SR, 512), kind, knob, 120);
                let diff: Vec<f32> = out[skip..]
                    .iter()
                    .zip(d[skip..].iter())
                    .map(|(a, b)| a - b)
                    .collect();
                // Noise turned far left is a quiet low rumble.
                let least = if kind == ColorFx::Noise { 0.005 } else { 0.01 };
                assert!(rms(&diff) > least, "{kind:?} {knob}: {}", rms(&diff));
                // Never much louder than the track (Space and Noise
                // were far too loud).
                let ratio = rms(&out[skip..]) / rms(&d[skip..]);
                assert!(ratio < 1.35, "{kind:?} {knob}: {ratio}x the track");
                assert!(
                    out.iter().all(|v| v.is_finite() && v.abs() < 2.0),
                    "{kind:?}"
                );
            }
        }
    }

    #[test]
    fn noise_is_a_riser_louder_and_brighter_further_right() {
        // Silence in: only the noise comes out.
        let noise = |knob: f32| {
            let mut p = ColorProcessor::new(SR, 512);
            let mut all = Vec::new();
            for _ in 0..100 {
                let mut l = vec![0.0f32; 256];
                let mut r = vec![0.0f32; 256];
                p.process(ColorFx::Noise, knob, None, 120.0, &mut l, &mut r);
                all.extend(l);
            }
            all.split_off(256 * 20)
        };
        // Brightness: how often the signal crosses zero.
        let crossings = |s: &[f32]| {
            s.windows(2)
                .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
                .count()
        };
        let (low, mid, high) = (noise(0.15), noise(0.5), noise(0.95));
        assert!(rms(&low) < rms(&mid) && rms(&mid) < rms(&high));
        assert!(rms(&low) < 0.01, "barely there at first: {}", rms(&low));
        assert!(
            rms(&high) > 0.06 && rms(&high) < 0.2,
            "riser at the end: {}",
            rms(&high)
        );
        assert!(crossings(&high) > crossings(&mid), "brighter further right");
        // Left: a falling wash, darker further out.
        assert!(crossings(&noise(-0.95)) < crossings(&noise(-0.5)));
    }

    /// Noise turned left silenced the whole track. Turned
    /// either way, back and forth, the track stays and every sample is a
    /// number.
    #[test]
    fn noise_never_takes_the_track_away() {
        for side in [-1.0f32, 1.0] {
            let mut p = ColorProcessor::new(SR, 512);
            let mut out = Vec::new();
            for b in 0..400 {
                // Out and back to the middle, again and again.
                let knob = side * (b as f32 * 0.05).sin().abs();
                let mut l: Vec<f32> = (0..256).map(|i| signal(b * 256 + i)).collect();
                let mut r = l.clone();
                p.process(ColorFx::Noise, knob, None, 120.0, &mut l, &mut r);
                assert!(
                    l.iter().chain(r.iter()).all(|v| v.is_finite()),
                    "side {side}, block {b}: not a number"
                );
                out.extend(l);
            }
            // The track is still there (at least as loud as the dry track).
            let d = dry(400);
            assert!(
                rms(&out) >= rms(&d) * 0.95,
                "side {side}: {} vs {}",
                rms(&out),
                rms(&d)
            );
        }
    }

    /// The knob gliding back to exactly the middle, many times, at odd
    /// block sizes: the amount must never dip below zero (the noise level
    /// is a power of it, and a NaN there silenced everything after it).
    #[test]
    fn gliding_back_to_the_middle_never_makes_a_nan() {
        for kind in ColorFx::ALL {
            for side in [-1.0f32, 1.0] {
                let mut p = ColorProcessor::new(SR, 512);
                let mut t = 0;
                for round in 0..300 {
                    let n = 37 + (round * 53) % 470;
                    let knob = if round % 3 == 0 {
                        side * (0.1 + (round % 7) as f32 * 0.13)
                    } else {
                        0.0
                    };
                    let mut l: Vec<f32> = (0..n).map(|i| signal(t + i)).collect();
                    let mut r = l.clone();
                    t += n;
                    p.process(kind, knob, None, 120.0, &mut l, &mut r);
                    assert!(
                        l.iter().chain(r.iter()).all(|v| v.is_finite()),
                        "{kind:?} side {side} round {round}"
                    );
                }
            }
        }
    }

    #[test]
    fn sweep_left_tightens_the_decays() {
        // The tails between hits get much quieter; the hits stay.
        // 200 blocks: hits every 6000 frames, 8 of them.
        let d = dry(200);
        let out = run(&mut ColorProcessor::new(SR, 512), ColorFx::Sweep, -0.9, 200);
        let tail = |s: &[f32]| {
            let mut v = Vec::new();
            for hit in 2..7 {
                let start = hit * 6000 + 2500;
                v.extend_from_slice(&s[start..start + 3000]);
            }
            rms(&v)
        };
        assert!(
            tail(&out) < tail(&d) * 0.2,
            "tails {} vs {}",
            tail(&out),
            tail(&d)
        );
        let hits = |s: &[f32]| {
            s[4 * 6000..4 * 6000 + 300]
                .iter()
                .fold(0.0f32, |m, v| m.max(v.abs()))
        };
        assert!(
            hits(&out) > hits(&d) * 0.7,
            "the hits stay: {} vs {}",
            hits(&out),
            hits(&d)
        );
        // No clicks or chatter (it sounded glitchy): the output never
        // jumps more from one sample to the next than the track does.
        let jump = |s: &[f32]| s.windows(2).fold(0.0f32, |m, w| m.max((w[1] - w[0]).abs()));
        assert!(
            jump(&out) <= jump(&d) * 1.05,
            "{} vs {}",
            jump(&out),
            jump(&d)
        );
        // Nor when the knob is swept quickly while it plays.
        let mut p = ColorProcessor::new(SR, 512);
        let mut swept = Vec::new();
        for b in 0..200 {
            let knob = -((b as f32 * 0.37).sin().abs());
            let mut l: Vec<f32> = (0..256).map(|i| signal(b * 256 + i)).collect();
            let mut r = l.clone();
            p.process(ColorFx::Sweep, knob, None, 120.0, &mut l, &mut r);
            swept.extend(l);
        }
        assert!(jump(&swept) <= jump(&d) * 1.05, "swept: {}", jump(&swept));
    }

    #[test]
    fn dub_echo_and_space_ring_out_after_the_knob_returns() {
        for kind in [ColorFx::DubEcho, ColorFx::Space] {
            let mut p = ColorProcessor::new(SR, 512);
            run(&mut p, kind, -0.9, 100);
            // Silence in, knob back in the middle: the tail is still heard.
            let mut tail = Vec::new();
            for _ in 0..40 {
                let mut l = vec![0.0f32; 256];
                let mut r = vec![0.0f32; 256];
                p.process(kind, 0.0, Some((120.0, 0.0)), 120.0, &mut l, &mut r);
                tail.extend_from_slice(&l);
            }
            assert!(rms(&tail) > 1e-3, "{kind:?}: no tail");
        }
    }
}
