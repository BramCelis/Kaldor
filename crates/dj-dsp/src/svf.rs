//! State-variable filter (topology-preserving transform, after Zavalishin's
//! "The Art of VA Filter Design") and the DJ filter knob built on it.
//! The TPT structure stays stable and smooth when the cutoff is swept fast,
//! which a biquad does not.

use crate::smoothing::LinearSmoother;
use std::f64::consts::PI;

/// One TPT state-variable filter channel.
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    ic1: f64,
    ic2: f64,
    a1: f64,
    a2: f64,
    a3: f64,
    k: f64,
}

/// The three simultaneous SVF outputs.
#[derive(Clone, Copy, Debug)]
pub struct SvfOut {
    pub low: f32,
    pub band: f32,
    pub high: f32,
}

impl Svf {
    /// Sets cutoff (Hz) and resonance `q`.
    pub fn set(&mut self, cutoff: f64, q: f64, sample_rate: f64) {
        let fc = cutoff.clamp(10.0, sample_rate * 0.45);
        let g = (PI * fc / sample_rate).tan();
        self.k = 1.0 / q.max(0.1);
        self.a1 = 1.0 / (1.0 + g * (g + self.k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> SvfOut {
        let x = f64::from(x);
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut {
            low: v2 as f32,
            band: v1 as f32,
            high: (x - self.k * v1 - v2) as f32,
        }
    }
}

// The knob's behaviour follows a measurement of a reference DJ mixer's
// filter, knob position by knob position. Positions are counted in 7-bit controller
// steps from the centre (64 = 0; the controller maps 0..127 so that 64 is
// exactly 0).

/// Controller steps from the centre to either end.
const STEPS: f64 = 63.5;
/// Knob values closer to the centre than this are "off": MIDI 63 to 65.
pub const FILTER_DEAD_ZONE: f32 = 0.024;
/// Low-pass cutoff (each stage's natural frequency) at the centre and
/// fully left; exponential in between (measured: within 2 %).
pub const LP_RANGE_HZ: (f64, f64) = (17_400.0, 80.0);
/// High-pass cutoff at the centre and fully right.
pub const HP_RANGE_HZ: (f64, f64) = (61.0, 17_150.0);
/// The filter comes in as a wet/dry mix just past the dead zone: wet is
/// 1 - WET_STEP_FALL^(steps - 1), about 0.6, 0.84 and 0.94 one, two and
/// three steps out (measured).
const WET_STEP_FALL: f64 = 0.4;
/// The resonance rises linearly from Q 0.707 one step out to its full
/// value this many steps out, then holds (measured: about 10).
pub const Q_RAMP_STEPS: (f64, f64) = (1.0, 10.0);
/// The filtered signal drops this much per unit of Q above 0.707 (dB), as
/// ladder-style filters lose level when they resonate: about -1.3 dB at
/// the default resonance (measured).
pub const GAIN_DB_PER_Q: f64 = -2.0;
/// Default resonance (0 to 1): Q 1.355 per stage, the measured value; at
/// 24 dB/oct a peak about 5.5 dB above the passband.
pub const DEFAULT_FILTER_RESONANCE: f32 = 0.72;
/// Q of a stage with no resonance (Butterworth).
const Q_FLAT: f64 = 0.707;
/// Time constant of the cutoff and resonance glide (one-pole, ms).
pub const FILTER_GLIDE_MS: f64 = 12.0;
/// Wet/dry crossfade at the dead zone (ms).
const WET_FADE_MS: f32 = 5.0;
/// Coefficients are recomputed every this many frames while sweeping.
const UPDATE_FRAMES: usize = 16;

/// Slope of the DJ filter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FilterSlope {
    /// One 2-pole stage: smooth, resonant sweeps.
    Db12,
    /// Two cascaded 2-pole stages: steeper, more aggressive club sweep
    /// (the measured reference).
    #[default]
    Db24,
}

impl FilterSlope {
    pub const ALL: [FilterSlope; 2] = [FilterSlope::Db12, FilterSlope::Db24];

    pub fn label(self) -> &'static str {
        match self {
            FilterSlope::Db12 => "12 dB/oct",
            FilterSlope::Db24 => "24 dB/oct",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            FilterSlope::Db12 => "12",
            FilterSlope::Db24 => "24",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    /// Q of each stage for a resonance of 0 (none) to 1 (strong); the
    /// same for both slopes.
    pub fn q(self, resonance: f32) -> f64 {
        Q_FLAT + f64::from(resonance.clamp(0.0, 1.0)) * 0.9
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    LowPass,
    HighPass,
}

/// Stereo DJ filter: one knob from -1 (low-pass, closing) through 0 (off)
/// to +1 (high-pass, opening). Within the dead zone it is out of the
/// signal path entirely (no phase shift). Just past it the filter comes in
/// as a wet/dry mix over a few controller steps (smoothed, 5 ms), and
/// switching between low-pass and high-pass always passes through dry, so
/// there are no clicks.
///
/// The cutoff follows the knob exponentially (even in pitch) and the
/// resonance rises over the first ten steps; the filtered level drops a
/// little as the resonance rises. Cutoff and Q glide with a 12 ms one-pole
/// so stepped MIDI values sweep smoothly; the topology-preserving SVF
/// stays stable under fast sweeps. Linear: no saturation.
#[derive(Clone, Debug)]
pub struct DjFilter {
    sample_rate: f64,
    /// Two stages per channel; the second only runs at 24 dB/oct.
    l: [Svf; 2],
    r: [Svf; 2],
    wet: LinearSmoother,
    target: f32,
    mode: Mode,
    slope: FilterSlope,
    resonance: f32,
    /// The log2 cutoff and the Q in use, gliding to the knob's values.
    log_fc: f64,
    q: f64,
    /// Level of the filtered signal (follows the Q).
    gain: f32,
    /// One-pole coefficient per `UPDATE_FRAMES` block.
    glide: f64,
    counter: usize,
}

impl DjFilter {
    pub fn new(sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate);
        let slope = FilterSlope::default();
        let mut f = Self {
            sample_rate: sr,
            l: [Svf::default(); 2],
            r: [Svf::default(); 2],
            wet: LinearSmoother::with_time(0.0, sample_rate, WET_FADE_MS),
            target: 0.0,
            mode: Mode::LowPass,
            slope,
            resonance: DEFAULT_FILTER_RESONANCE,
            log_fc: 0.0,
            q: 0.0,
            gain: 1.0,
            glide: 1.0 - (-(UPDATE_FRAMES as f64) / (FILTER_GLIDE_MS * 0.001 * sr)).exp(),
            counter: 0,
        };
        f.snap_to_edge();
        f.update_coeffs();
        f
    }

    /// Sets the knob, -1.0 to 1.0.
    pub fn set(&mut self, knob: f32) {
        self.target = knob.clamp(-1.0, 1.0);
    }

    /// Sets the slope and resonance (0 to 1).
    pub fn set_shape(&mut self, slope: FilterSlope, resonance: f32) {
        if slope != self.slope {
            // The second stage starts from rest when it joins.
            self.l[1].reset();
            self.r[1].reset();
        }
        self.slope = slope;
        self.resonance = resonance.clamp(0.0, 1.0);
    }

    /// Controller steps from the centre for knob `amount` (either side).
    fn steps(amount: f32) -> f64 {
        f64::from(amount.abs().min(1.0)) * STEPS
    }

    /// Cutoff for knob `amount` (0 to 1, either side): exponential from
    /// the centre to the end.
    pub fn cutoff_for(low_pass: bool, amount: f32) -> f64 {
        let t = f64::from(amount.abs().min(1.0));
        let (from, to) = if low_pass { LP_RANGE_HZ } else { HP_RANGE_HZ };
        from * (to / from).powf(t)
    }

    /// Q of each stage for knob `amount` (0 to 1, either side): Butterworth
    /// up to the first step, rising linearly to the resonance setting over
    /// [`Q_RAMP_STEPS`], then held.
    pub fn q_for(slope: FilterSlope, resonance: f32, amount: f32) -> f64 {
        let (a, b) = Q_RAMP_STEPS;
        let t = ((Self::steps(amount) - a) / (b - a)).clamp(0.0, 1.0);
        let flat = slope.q(0.0);
        flat + (slope.q(resonance) - flat) * t
    }

    /// How much of the filtered signal is heard at knob `amount` (the rest
    /// is dry): none in the dead zone, nearly all three steps out.
    pub fn wet_for(amount: f32) -> f32 {
        if amount.abs() < FILTER_DEAD_ZONE {
            return 0.0;
        }
        (1.0 - WET_STEP_FALL.powf(Self::steps(amount) - 1.0)).clamp(0.0, 1.0) as f32
    }

    /// Level of the filtered signal at Q `q` (linear).
    pub fn gain_for(q: f64) -> f32 {
        let db = GAIN_DB_PER_Q * (q - Q_FLAT).max(0.0);
        10f64.powf(db / 20.0) as f32
    }

    /// Where the cutoff and Q are heading: the knob's values while it is
    /// outside the dead zone on the current side, the dead zone's edge
    /// (fully open, no resonance) otherwise.
    fn goal(&self) -> (f64, f64) {
        let low = self.mode == Mode::LowPass;
        let on_side = self.target.abs() >= FILTER_DEAD_ZONE && (self.target < 0.0) == low;
        let amount = if on_side { self.target } else { 0.0 };
        (
            Self::cutoff_for(low, amount).log2(),
            Self::q_for(self.slope, self.resonance, amount),
        )
    }

    /// Jumps the cutoff and Q to the dead zone's edge for the current side.
    fn snap_to_edge(&mut self) {
        self.log_fc = Self::cutoff_for(self.mode == Mode::LowPass, 0.0).log2();
        self.q = Self::q_for(self.slope, self.resonance, 0.0);
    }

    fn update_coeffs(&mut self) {
        self.gain = Self::gain_for(self.q);
        let fc = self.log_fc.exp2();
        for s in self.l.iter_mut().chain(self.r.iter_mut()) {
            s.set(fc, self.q, self.sample_rate);
        }
    }

    /// True when the filter is fully out of the signal path.
    pub fn is_bypassed(&self) -> bool {
        self.wet.is_settled() && self.wet.current() == 0.0 && self.target.abs() < FILTER_DEAD_ZONE
    }

    /// Filters a stereo block in place.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let wanted_mode = if self.target < 0.0 {
            Mode::LowPass
        } else {
            Mode::HighPass
        };
        let active = self.target.abs() >= FILTER_DEAD_ZONE;
        if active && wanted_mode != self.mode {
            // Fade out, switch mode at dry, then fade back in from the
            // new side's dead zone edge.
            self.wet.set_target(0.0);
            if self.wet.is_settled() && self.wet.current() == 0.0 {
                self.mode = wanted_mode;
                for s in self.l.iter_mut().chain(self.r.iter_mut()) {
                    s.reset();
                }
                self.snap_to_edge();
                self.counter = 0;
            }
        } else {
            self.wet.set_target(Self::wet_for(self.target));
        }
        if self.is_bypassed() {
            for s in self.l.iter_mut().chain(self.r.iter_mut()) {
                s.reset();
            }
            self.snap_to_edge();
            self.counter = 0;
            return;
        }
        let (goal_fc, goal_q) = self.goal();
        let two = self.slope == FilterSlope::Db24;
        let low = self.mode == Mode::LowPass;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            if self.counter == 0 {
                self.log_fc += self.glide * (goal_fc - self.log_fc);
                self.q += self.glide * (goal_q - self.q);
                self.update_coeffs();
            }
            self.counter = (self.counter + 1) % UPDATE_FRAMES;
            let w = self.wet.next_value();
            let g = self.gain;
            let pick = |o: SvfOut| if low { o.low } else { o.high };
            let mut ol = pick(self.l[0].process(*l));
            let mut or = pick(self.r[0].process(*r));
            if two {
                ol = pick(self.l[1].process(ol));
                or = pick(self.r[1].process(or));
            }
            *l += w * (g * ol - *l);
            *r += w * (g * or - *r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A 7-bit knob value as the controller maps it: 64 is the centre.
    fn centred(m: f32) -> f32 {
        if m < 64.0 {
            (m - 64.0) / 64.0
        } else {
            (m - 64.0) / 63.0
        }
    }

    fn gain_with(f: &mut DjFilter, freq: f64) -> f64 {
        let n = 48_000;
        let mut l: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * freq * i as f64 / f64::from(SR)).sin() as f32)
            .collect();
        let mut r = l.clone();
        let input_rms = (0.5f64).sqrt();
        for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            f.process(cl, cr);
        }
        let tail = &l[n / 2..];
        let rms = (tail
            .iter()
            .map(|&v| f64::from(v) * f64::from(v))
            .sum::<f64>()
            / tail.len() as f64)
            .sqrt();
        rms / input_rms
    }

    fn gain_through(knob: f32, freq: f64) -> f64 {
        let mut f = DjFilter::new(SR);
        f.set(knob);
        gain_with(&mut f, freq)
    }

    #[test]
    fn midi_63_to_65_are_off() {
        for m in [63.0f32, 64.0, 65.0] {
            let mut f = DjFilter::new(SR);
            f.set(centred(m));
            let orig: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.37).sin()).collect();
            let mut l = orig.clone();
            let mut r = orig.clone();
            f.process(&mut l, &mut r);
            assert_eq!(l, orig, "MIDI {m}");
            assert!(f.is_bypassed());
        }
        // One step further either way the filter is in.
        for m in [62.0f32, 66.0] {
            let mut f = DjFilter::new(SR);
            f.set(centred(m));
            let mut l = vec![0.5f32; 2048];
            let mut r = l.clone();
            f.process(&mut l, &mut r);
            assert!(!f.is_bypassed(), "MIDI {m}");
        }
    }

    #[test]
    fn cutoff_follows_the_measured_curve() {
        assert!((DjFilter::cutoff_for(true, 0.0) - 17_400.0).abs() < 1e-6);
        assert!((DjFilter::cutoff_for(true, 1.0) - 80.0).abs() < 1e-6);
        assert!((DjFilter::cutoff_for(false, 0.0) - 61.0).abs() < 1e-6);
        assert!((DjFilter::cutoff_for(false, 1.0) - 17_150.0).abs() < 1e-3);
        // Measured cutoffs (fit f0) at a few MIDI values, within 3 %.
        for (m, hz) in [(16.0f32, 310.0), (32.0, 1190.0), (48.0, 4540.0)] {
            let f = DjFilter::cutoff_for(true, centred(m).abs());
            assert!((f / hz - 1.0).abs() < 0.03, "MIDI {m}: {f}");
        }
        for (m, hz) in [
            (80.0f32, 0.0),
            (96.0, 1070.0),
            (112.0, 4420.0),
            (124.0, 13_170.0),
        ] {
            if hz == 0.0 {
                continue;
            }
            let f = DjFilter::cutoff_for(false, centred(m));
            assert!((f / hz - 1.0).abs() < 0.03, "MIDI {m}: {f}");
        }
    }

    #[test]
    fn filter_comes_in_as_a_mix_past_the_dead_zone() {
        assert_eq!(DjFilter::wet_for(centred(65.0)), 0.0);
        assert_eq!(DjFilter::wet_for(centred(63.0)), 0.0);
        // Measured: about 0.6, 0.84, 0.94 one, two, three steps out.
        for (m, want) in [(66.0f32, 0.6f32), (67.0, 0.84), (68.0, 0.94), (62.0, 0.6)] {
            let w = DjFilter::wet_for(centred(m));
            assert!((w - want).abs() < 0.06, "MIDI {m}: {w}");
        }
        assert!(DjFilter::wet_for(centred(70.0)) > 0.98);
        assert_eq!(DjFilter::wet_for(1.0), 1.0);
    }

    #[test]
    fn resonance_rises_over_the_first_ten_steps() {
        let q = |amount| DjFilter::q_for(FilterSlope::Db24, DEFAULT_FILTER_RESONANCE, amount);
        let full = FilterSlope::Db24.q(DEFAULT_FILTER_RESONANCE);
        assert!((full - 1.355).abs() < 0.005, "{full}");
        // At the dead zone edge the ramp has only just started.
        assert!((q(FILTER_DEAD_ZONE) - Q_FLAT).abs() < 0.05);
        assert!((q(1.0 / 63.5) - Q_FLAT).abs() < 1e-9);
        assert!((q(10.01 / 63.5) - full).abs() < 1e-9);
        assert!((q(0.6) - full).abs() < 1e-9 && (q(1.0) - full).abs() < 1e-9);
        let mut prev = 0.0;
        for i in 0..=100 {
            let v = q(i as f32 / 100.0);
            assert!(v >= prev - 1e-12, "rising at {i}");
            prev = v;
        }
    }

    #[test]
    fn level_drops_as_it_resonates() {
        assert_eq!(DjFilter::gain_for(Q_FLAT), 1.0);
        let db = 20.0 * f64::from(DjFilter::gain_for(1.355)).log10();
        assert!((db + 1.3).abs() < 0.05, "{db}");
    }

    #[test]
    fn resonant_peak_at_the_cutoff() {
        // Two stages of Q 1.355 and -1.3 dB: about +4 dB at the cutoff,
        // the pass band about -1.3 dB.
        for (knob, low) in [(-0.6f32, true), (0.6, false)] {
            let fc = DjFilter::cutoff_for(low, knob);
            let g = gain_through(knob, fc);
            assert!(g > 1.5 && g < 1.66, "{knob}: {g}");
            let pass = if low { fc / 8.0 } else { fc * 8.0 };
            let p = gain_through(knob, pass);
            assert!(p > 0.8 && p < 0.92, "{knob} pass band: {p}");
        }
    }

    #[test]
    fn no_bump_just_past_the_centre() {
        // One step out the resonance has barely started: no audible boost.
        for freq in [1_000.0, 5_000.0, 10_000.0, 13_000.0] {
            let g = gain_through(centred(62.0), freq);
            assert!(g < 1.03, "{freq} Hz: {g}");
        }
    }

    #[test]
    fn cutoff_glides_with_a_12_ms_time_constant() {
        let mut f = DjFilter::new(SR);
        f.set(-1.0);
        let start = f.log_fc;
        let end = LP_RANGE_HZ.1.log2();
        let frames = (FILTER_GLIDE_MS * 0.001 * f64::from(SR)) as usize;
        let mut l = vec![0.0f32; frames];
        let mut r = l.clone();
        f.process(&mut l, &mut r);
        let moved = (start - f.log_fc) / (start - end);
        // 1 - 1/e of the way after one time constant.
        assert!((moved - 0.632).abs() < 0.03, "{moved}");
    }

    #[test]
    fn steeper_at_24_db() {
        let through = |slope: FilterSlope| {
            let mut f = DjFilter::new(SR);
            f.set_shape(slope, 0.0);
            f.set(-0.6);
            let n = 48_000;
            // Two octaves above the cutoff.
            let fc = DjFilter::cutoff_for(true, 0.6);
            let mut l: Vec<f32> = (0..n)
                .map(|i| (2.0 * PI * fc * 4.0 * i as f64 / f64::from(SR)).sin() as f32)
                .collect();
            let mut r = l.clone();
            for (cl, cr) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
                f.process(cl, cr);
            }
            l[n / 2..].iter().fold(0.0f32, |m, v| m.max(v.abs()))
        };
        let g12 = through(FilterSlope::Db12);
        let g24 = through(FilterSlope::Db24);
        // About -24 dB and -48 dB two octaves out.
        assert!(g12 < 0.1 && g12 > 0.03, "{g12}");
        assert!(g24 < 0.012, "{g24}");
    }

    #[test]
    fn centre_is_transparent() {
        assert!((gain_through(0.0, 100.0) - 1.0).abs() < 1e-3);
        assert!((gain_through(0.02, 8000.0) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn low_pass_cuts_highs() {
        let g_hi = gain_through(-0.8, 5000.0);
        let g_lo = gain_through(-0.8, 40.0);
        assert!(g_hi < 0.05, "highs {g_hi}");
        assert!(g_lo > 0.7, "lows {g_lo}");
    }

    #[test]
    fn high_pass_cuts_lows() {
        let g_lo = gain_through(0.8, 80.0);
        let g_hi = gain_through(0.8, 15_000.0);
        assert!(g_lo < 0.05, "lows {g_lo}");
        assert!(g_hi > 0.7, "highs {g_hi}");
    }

    #[test]
    fn fully_turned_keeps_the_ends() {
        // Fully left stops at 80 Hz: the sub bass stays, the rest goes.
        assert!(gain_through(-1.0, 40.0) > 0.7);
        assert!(gain_through(-1.0, 400.0) < 0.01);
        // Fully right stops at 17 kHz: the mids are gone.
        assert!(gain_through(1.0, 4_000.0) < 0.01);
    }

    #[test]
    fn sweeping_across_centre_has_no_clicks() {
        let mut f = DjFilter::new(SR);
        let freq = 150.0;
        let mut prev = 0.0f32;
        let mut max_step = 0.0f32;
        let mut i = 0usize;
        for block in 0..400 {
            // Jump from full LP to full HP and back.
            f.set(if (block / 50) % 2 == 0 { -0.7 } else { 0.7 });
            let mut l: Vec<f32> = (0..128)
                .map(|k| (0.5 * (2.0 * PI * freq * (i + k) as f64 / f64::from(SR)).sin()) as f32)
                .collect();
            let mut r = l.clone();
            f.process(&mut l, &mut r);
            for v in l {
                max_step = max_step.max((v - prev).abs());
                prev = v;
            }
            i += 128;
        }
        // The input's own maximum step is about 0.0098.
        assert!(max_step < 0.03, "max step {max_step}");
    }
}
