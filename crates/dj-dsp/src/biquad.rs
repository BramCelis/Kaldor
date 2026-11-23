//! Biquad filters (transposed direct form II, `f64` state) and
//! Linkwitz-Riley crossovers.
//!
//! Coefficient formulas are the standard RBJ "Audio EQ Cookbook" ones.

use std::f64::consts::{FRAC_1_SQRT_2, PI};

/// Normalised biquad coefficients (`a0 == 1`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Coeffs {
    /// Pass-through.
    pub const IDENTITY: Coeffs = Coeffs {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    fn normalise(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }

    fn w0(freq: f64, sample_rate: f64) -> (f64, f64) {
        let f = freq.clamp(1.0, sample_rate * 0.49);
        let w0 = 2.0 * PI * f / sample_rate;
        (w0.cos(), w0.sin())
    }

    pub fn lowpass(freq: f64, q: f64, sample_rate: f64) -> Self {
        let (cos, sin) = Self::w0(freq, sample_rate);
        let alpha = sin / (2.0 * q);
        Self::normalise(
            (1.0 - cos) / 2.0,
            1.0 - cos,
            (1.0 - cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    pub fn highpass(freq: f64, q: f64, sample_rate: f64) -> Self {
        let (cos, sin) = Self::w0(freq, sample_rate);
        let alpha = sin / (2.0 * q);
        Self::normalise(
            (1.0 + cos) / 2.0,
            -(1.0 + cos),
            (1.0 + cos) / 2.0,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// First-order allpass (bilinear, prewarped at `freq`): flat
    /// magnitude, 90 degrees of phase at `freq`.
    pub fn allpass1(freq: f64, sample_rate: f64) -> Self {
        let f = freq.clamp(1.0, sample_rate * 0.49);
        let k = (PI * f / sample_rate).tan();
        let a = (k - 1.0) / (k + 1.0);
        Self {
            b0: a,
            b1: 1.0,
            b2: 0.0,
            a1: a,
            a2: 0.0,
        }
    }

    pub fn allpass(freq: f64, q: f64, sample_rate: f64) -> Self {
        let (cos, sin) = Self::w0(freq, sample_rate);
        let alpha = sin / (2.0 * q);
        Self::normalise(
            1.0 - alpha,
            -2.0 * cos,
            1.0 + alpha,
            1.0 + alpha,
            -2.0 * cos,
            1.0 - alpha,
        )
    }

    /// Low shelf with shelf slope `S = 1` (no bump, no ringing).
    pub fn low_shelf(freq: f64, gain_db: f64, sample_rate: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, sin) = Self::w0(freq, sample_rate);
        let alpha = sin / 2.0 * std::f64::consts::SQRT_2;
        let beta = 2.0 * a.sqrt() * alpha;
        Self::normalise(
            a * ((a + 1.0) - (a - 1.0) * cos + beta),
            2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
            a * ((a + 1.0) - (a - 1.0) * cos - beta),
            (a + 1.0) + (a - 1.0) * cos + beta,
            -2.0 * ((a - 1.0) + (a + 1.0) * cos),
            (a + 1.0) + (a - 1.0) * cos - beta,
        )
    }

    /// High shelf with shelf slope `S = 1`.
    pub fn high_shelf(freq: f64, gain_db: f64, sample_rate: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, sin) = Self::w0(freq, sample_rate);
        let alpha = sin / 2.0 * std::f64::consts::SQRT_2;
        let beta = 2.0 * a.sqrt() * alpha;
        Self::normalise(
            a * ((a + 1.0) + (a - 1.0) * cos + beta),
            -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
            a * ((a + 1.0) + (a - 1.0) * cos - beta),
            (a + 1.0) - (a - 1.0) * cos + beta,
            2.0 * ((a - 1.0) - (a + 1.0) * cos),
            (a + 1.0) - (a - 1.0) * cos - beta,
        )
    }

    /// Peaking bell.
    pub fn peaking(freq: f64, q: f64, gain_db: f64, sample_rate: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let (cos, sin) = Self::w0(freq, sample_rate);
        let alpha = sin / (2.0 * q);
        Self::normalise(
            1.0 + alpha * a,
            -2.0 * cos,
            1.0 - alpha * a,
            1.0 + alpha / a,
            -2.0 * cos,
            1.0 - alpha / a,
        )
    }

    /// Magnitude response at `freq` (for tests and plots).
    pub fn magnitude(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        let (c1, s1) = (w.cos(), -w.sin());
        let (c2, s2) = ((2.0 * w).cos(), -(2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = self.b1 * s1 + self.b2 * s2;
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = self.a1 * s1 + self.a2 * s2;
        ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).sqrt()
    }
}

/// One biquad section with its state.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    pub c: Coeffs,
    z1: f64,
    z2: f64,
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new(Coeffs::IDENTITY)
    }
}

impl Biquad {
    pub fn new(c: Coeffs) -> Self {
        Self {
            c,
            z1: 0.0,
            z2: 0.0,
        }
    }

    /// Replaces the coefficients, keeping the state (for smooth sweeps).
    pub fn set(&mut self, c: Coeffs) {
        self.c = c;
    }

    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        let x = f64::from(x);
        let y = self.c.b0 * x + self.z1;
        self.z1 = self.c.b1 * x - self.c.a1 * y + self.z2;
        self.z2 = self.c.b2 * x - self.c.a2 * y;
        y as f32
    }
}

/// 4th-order Linkwitz-Riley low-pass or high-pass: two cascaded 2nd-order
/// Butterworth sections. LR4 low-pass plus high-pass at the same frequency
/// sums to a flat magnitude response.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lr4 {
    a: Biquad,
    b: Biquad,
}

impl Lr4 {
    pub fn lowpass(freq: f64, sample_rate: f64) -> Self {
        let c = Coeffs::lowpass(freq, FRAC_1_SQRT_2, sample_rate);
        Self {
            a: Biquad::new(c),
            b: Biquad::new(c),
        }
    }

    pub fn highpass(freq: f64, sample_rate: f64) -> Self {
        let c = Coeffs::highpass(freq, FRAC_1_SQRT_2, sample_rate);
        Self {
            a: Biquad::new(c),
            b: Biquad::new(c),
        }
    }

    #[inline]
    pub fn process(&mut self, x: f32) -> f32 {
        self.b.process(self.a.process(x))
    }

    pub fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }
}

/// Splits a mono signal into low, mid and high bands with LR4 crossovers.
/// The low band also passes through an allpass at the upper crossover so the
/// three bands sum back to an allpass (flat magnitude, no comb filtering).
#[derive(Clone, Copy, Debug)]
pub struct ThreeBandSplit {
    low_lp: Lr4,
    low_ap: Biquad,
    rest_hp: Lr4,
    mid_lp: Lr4,
    high_hp: Lr4,
}

impl ThreeBandSplit {
    /// Default crossovers: about 250 Hz and 2.5 kHz.
    pub const DEFAULT_LOW_HZ: f64 = 250.0;
    pub const DEFAULT_HIGH_HZ: f64 = 2500.0;

    pub fn new(low_hz: f64, high_hz: f64, sample_rate: f64) -> Self {
        Self {
            low_lp: Lr4::lowpass(low_hz, sample_rate),
            low_ap: Biquad::new(Coeffs::allpass(high_hz, FRAC_1_SQRT_2, sample_rate)),
            rest_hp: Lr4::highpass(low_hz, sample_rate),
            mid_lp: Lr4::lowpass(high_hz, sample_rate),
            high_hp: Lr4::highpass(high_hz, sample_rate),
        }
    }

    /// Returns `(low, mid, high)`.
    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32, f32) {
        let low = self.low_ap.process(self.low_lp.process(x));
        let rest = self.rest_hp.process(x);
        (low, self.mid_lp.process(rest), self.high_hp.process(rest))
    }

    pub fn reset(&mut self) {
        self.low_lp.reset();
        self.low_ap.reset();
        self.rest_hp.reset();
        self.mid_lp.reset();
        self.high_hp.reset();
    }
}

/// The channel isolator's band split: 2nd-order Linkwitz-Riley crossovers
/// (12 dB/oct, Q 0.5), gentler than [`ThreeBandSplit`]'s LR4, as measured on
/// a reference DJ mixer's EQ. LR2 bands are 180
/// degrees apart at the crossover, so the upper band of each split is
/// inverted; the low band also passes through a first-order allpass at
/// the upper crossover. The three bands then sum to an allpass (flat
/// magnitude), and a band turned down leaves its neighbours' gentle skirts.
#[derive(Clone, Copy, Debug)]
pub struct IsolatorSplit {
    low_lp: Biquad,
    low_ap: Biquad,
    rest_hp: Biquad,
    mid_lp: Biquad,
    high_hp: Biquad,
}

impl IsolatorSplit {
    /// Default crossovers: the measured reference's upper crossover is
    /// about 4 kHz; the lower one (about 150 Hz) is an estimate from a
    /// partly spoiled measurement.
    pub const DEFAULT_LOW_HZ: f64 = 150.0;
    pub const DEFAULT_HIGH_HZ: f64 = 4000.0;
    const Q: f64 = 0.5;

    pub fn new(low_hz: f64, high_hz: f64, sample_rate: f64) -> Self {
        Self {
            low_lp: Biquad::new(Coeffs::lowpass(low_hz, Self::Q, sample_rate)),
            low_ap: Biquad::new(Coeffs::allpass1(high_hz, sample_rate)),
            rest_hp: Biquad::new(Coeffs::highpass(low_hz, Self::Q, sample_rate)),
            mid_lp: Biquad::new(Coeffs::lowpass(high_hz, Self::Q, sample_rate)),
            high_hp: Biquad::new(Coeffs::highpass(high_hz, Self::Q, sample_rate)),
        }
    }

    /// Returns `(low, mid, high)`.
    #[inline]
    pub fn process(&mut self, x: f32) -> (f32, f32, f32) {
        let low = self.low_ap.process(self.low_lp.process(x));
        let rest = -self.rest_hp.process(x);
        (low, self.mid_lp.process(rest), -self.high_hp.process(rest))
    }

    pub fn reset(&mut self) {
        self.low_lp.reset();
        self.low_ap.reset();
        self.rest_hp.reset();
        self.mid_lp.reset();
        self.high_hp.reset();
    }
}

/// Mid bell Q of the classic EQ (wide, so cuts sound smooth, not nasal).
pub const CLASSIC_MID_Q: f64 = 0.7;
/// Classic EQ coefficients are recomputed every this many frames while a
/// gain is moving.
const CLASSIC_UPDATE: u32 = 16;

/// Classic three-band mixer EQ for a stereo channel: low shelf, mid bell
/// and high shelf in series.
///
/// The band gains are smoothed in dB (one-pole, about 8 ms) and the
/// coefficients recomputed from the smoothed values every 16 frames; the
/// coefficients themselves are never interpolated (that can go unstable).
#[derive(Clone, Debug)]
pub struct ClassicEq {
    sample_rate: f64,
    /// Shelf / bell frequencies (low, mid, high).
    freqs: [f64; 3],
    db: [f64; 3],
    target: [f64; 3],
    k: f64,
    sections: [[Biquad; 3]; 2],
    counter: u32,
}

impl ClassicEq {
    /// `low_hz` and `high_hz` are the shelf corners; the bell sits at their
    /// geometric mean.
    pub fn new(low_hz: f64, high_hz: f64, sample_rate: f64) -> Self {
        let mut eq = Self {
            sample_rate,
            freqs: [0.0; 3],
            db: [0.0; 3],
            target: [0.0; 3],
            k: 1.0 - (-1.0 / (0.008 * sample_rate.max(1.0))).exp(),
            sections: [[Biquad::default(); 3]; 2],
            counter: 0,
        };
        eq.set_freqs(low_hz, high_hz);
        eq
    }

    /// Frequencies of the classic EQ from the crossover settings: low shelf
    /// at the low crossover, high shelf at 1.6 times the high crossover,
    /// bell between them (250 / 1000 / 4000 Hz with the defaults).
    pub fn freqs_for(low_xover: f64, high_xover: f64) -> (f64, f64) {
        (low_xover, high_xover * 1.6)
    }

    pub fn set_freqs(&mut self, low_hz: f64, high_hz: f64) {
        self.freqs = [low_hz, (low_hz * high_hz).sqrt(), high_hz];
        self.update();
    }

    /// Target band gains in dB (low, mid, high).
    pub fn set_gains_db(&mut self, db: [f32; 3]) {
        self.target = db.map(f64::from);
    }

    /// Jumps to the targets without smoothing.
    pub fn settle(&mut self) {
        self.db = self.target;
        self.update();
    }

    fn update(&mut self) {
        let sr = self.sample_rate;
        let c = [
            Coeffs::low_shelf(self.freqs[0], self.db[0], sr),
            Coeffs::peaking(self.freqs[1], CLASSIC_MID_Q, self.db[1], sr),
            Coeffs::high_shelf(self.freqs[2], self.db[2], sr),
        ];
        for ch in &mut self.sections {
            for (s, c) in ch.iter_mut().zip(c) {
                s.set(c);
            }
        }
    }

    pub fn reset(&mut self) {
        for s in self.sections.iter_mut().flatten() {
            s.reset();
        }
    }

    /// Filters one stereo frame.
    #[inline]
    pub fn process(&mut self, l: f32, r: f32) -> (f32, f32) {
        if self.counter == 0 {
            let mut moving = false;
            for (d, t) in self.db.iter_mut().zip(self.target) {
                if (*d - t).abs() > 1e-4 {
                    // CLASSIC_UPDATE frames of one-pole smoothing at once.
                    let k = 1.0 - (1.0 - self.k).powi(CLASSIC_UPDATE as i32);
                    *d += (t - *d) * k;
                    moving = true;
                } else if *d != t {
                    *d = t;
                    moving = true;
                }
            }
            if moving {
                self.update();
            }
        }
        self.counter = (self.counter + 1) % CLASSIC_UPDATE;
        let [sl, sr] = &mut self.sections;
        let mut yl = l;
        let mut yr = r;
        for s in sl.iter_mut() {
            yl = s.process(yl);
        }
        for s in sr.iter_mut() {
            yr = s.process(yr);
        }
        (yl, yr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f64 = 48_000.0;

    fn sine_gain(f: &mut dyn FnMut(f32) -> f32, freq: f64) -> f64 {
        let n = 48_000;
        let mut in_sq = 0.0;
        let mut out_sq = 0.0;
        for i in 0..n {
            let x = (2.0 * PI * freq * i as f64 / SR).sin() as f32;
            let y = f(x);
            if i > n / 2 {
                in_sq += f64::from(x) * f64::from(x);
                out_sq += f64::from(y) * f64::from(y);
            }
        }
        (out_sq / in_sq).sqrt()
    }

    #[test]
    fn butterworth_is_minus_3db_at_cutoff() {
        let c = Coeffs::lowpass(1000.0, FRAC_1_SQRT_2, SR);
        assert!((c.magnitude(1000.0, SR) - FRAC_1_SQRT_2).abs() < 1e-3);
        assert!((c.magnitude(10.0, SR) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn lr4_is_minus_6db_at_crossover() {
        let mut lp = Lr4::lowpass(1000.0, SR);
        let g = sine_gain(&mut |x| lp.process(x), 1000.0);
        assert!((g - 0.5).abs() < 0.01, "{g}");
    }

    #[test]
    fn three_band_sums_flat() {
        for freq in [40.0, 180.0, 250.0, 700.0, 2500.0, 6000.0, 15000.0] {
            let mut s = ThreeBandSplit::new(250.0, 2500.0, SR);
            let g = sine_gain(
                &mut |x| {
                    let (l, m, h) = s.process(x);
                    l + m + h
                },
                freq,
            );
            assert!((g - 1.0).abs() < 0.012, "{freq} Hz: gain {g}");
        }
    }

    #[test]
    fn shelves_and_bell_reach_their_gain() {
        let db = |c: Coeffs, f: f64| 20.0 * c.magnitude(f, SR).log10();
        let ls = Coeffs::low_shelf(250.0, -20.0, SR);
        assert!((db(ls, 20.0) + 20.0).abs() < 0.3, "{}", db(ls, 20.0));
        assert!(db(ls, 10_000.0).abs() < 0.1);
        let hs = Coeffs::high_shelf(4000.0, 6.0, SR);
        assert!((db(hs, 20_000.0) - 6.0).abs() < 0.3);
        assert!(db(hs, 50.0).abs() < 0.05);
        let pk = Coeffs::peaking(1000.0, 0.7, -12.0, SR);
        assert!((db(pk, 1000.0) + 12.0).abs() < 0.01);
        assert!(db(pk, 20.0).abs() < 0.2);
        // 0 dB is flat.
        for f in [30.0, 300.0, 3000.0, 15_000.0] {
            assert!(db(Coeffs::low_shelf(250.0, 0.0, SR), f).abs() < 1e-9);
            assert!(db(Coeffs::peaking(1000.0, 0.7, 0.0, SR), f).abs() < 1e-9);
        }
    }

    #[test]
    fn classic_eq_cuts_bands() {
        let (lo, hi) = ClassicEq::freqs_for(250.0, 2500.0);
        let mut eq = ClassicEq::new(lo, hi, SR);
        eq.set_gains_db([-26.0, 0.0, 0.0]);
        eq.settle();
        let g_low = sine_gain(&mut |x| eq.process(x, x).0, 40.0);
        let mut eq2 = eq.clone();
        let g_high = sine_gain(&mut |x| eq2.process(x, x).1, 8000.0);
        assert!(g_low < 0.07, "{g_low}");
        assert!((g_high - 1.0).abs() < 0.02, "{g_high}");
    }

    #[test]
    fn allpass1_is_flat() {
        let c = Coeffs::allpass1(1000.0, SR);
        for f in [20.0, 1000.0, 15_000.0] {
            assert!((c.magnitude(f, SR) - 1.0).abs() < 1e-9, "{f}");
        }
    }

    #[test]
    fn isolator_split_sums_flat() {
        for freq in [40.0, 150.0, 400.0, 1000.0, 4000.0, 8000.0, 15000.0] {
            let mut s = IsolatorSplit::new(150.0, 4000.0, SR);
            let g = sine_gain(
                &mut |x| {
                    let (l, m, h) = s.process(x);
                    l + m + h
                },
                freq,
            );
            assert!((g - 1.0).abs() < 0.012, "{freq} Hz: gain {g}");
        }
    }

    /// A band at gain g reads (1 + g) / 2 at its crossover and keeps 12
    /// dB/oct skirts: the measured reference high EQ (half gain: -2.4 dB
    /// at 4 kHz; kill: -6 dB at 4 kHz, about -15 dB an octave above).
    #[test]
    fn isolator_split_matches_the_measured_high_band() {
        let through = |g: f32, freq: f64| {
            let mut s = IsolatorSplit::new(150.0, 4000.0, SR);
            sine_gain(
                &mut |x| {
                    let (l, m, h) = s.process(x);
                    l + m + g * h
                },
                freq,
            )
        };
        let db = |x: f64| 20.0 * x.log10();
        assert!((db(through(0.508, 4000.0)) + 2.4).abs() < 0.3);
        assert!((db(through(0.0, 4000.0)) + 6.0).abs() < 0.3);
        let oct = db(through(0.0, 8000.0));
        assert!(oct < -13.0 && oct > -17.0, "{oct}");
    }

    #[test]
    fn three_band_separates() {
        let mut s = ThreeBandSplit::new(250.0, 2500.0, SR);
        let low = sine_gain(&mut |x| s.process(x).0, 60.0);
        let mut s = ThreeBandSplit::new(250.0, 2500.0, SR);
        let high_of_low = sine_gain(&mut |x| s.process(x).2, 60.0);
        assert!(low > 0.95);
        assert!(high_of_low < 0.01);
    }
}
