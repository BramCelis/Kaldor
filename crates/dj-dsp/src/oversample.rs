//! 2x oversampling with linear-phase half-band FIR filters, for running a
//! non-linear stage (clipper, saturation) without aliasing.
//!
//! A half-band low-pass has every even tap zero except the centre (0.5),
//! so the polyphase up- and down-samplers only need the odd taps. The
//! filter is a Kaiser-windowed sinc with [`ODD_TAPS`] non-zero odd taps.
//!
//! Latency: [`HalfbandUp`] and [`HalfbandDown`] together delay the signal
//! by [`ROUND_TRIP_LATENCY`] frames at the base rate.

use std::f64::consts::PI;

/// Non-zero odd taps on each side of the centre.
const SIDE: usize = 12;
/// Non-zero odd taps in total.
pub const ODD_TAPS: usize = 2 * SIDE;
/// Up then down delay at the base rate, in frames.
pub const ROUND_TRIP_LATENCY: usize = 2 * SIDE - 1;
const KAISER_BETA: f64 = 8.0;

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..50 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-12 * sum {
            break;
        }
    }
    sum
}

/// The odd taps `h[j]` for `j = -(2*SIDE-1), ..., -1, 1, ..., 2*SIDE-1`,
/// in that order.
fn odd_taps() -> [f64; ODD_TAPS] {
    let half_len = (2 * SIDE) as f64; // window reaches zero at |j| = 2*SIDE
    let mut h = [0.0; ODD_TAPS];
    for (k, v) in h.iter_mut().enumerate() {
        let j = 2 * k as i64 - (2 * SIDE as i64 - 1);
        let x = j as f64;
        let sinc = (PI * x / 2.0).sin() / (PI * x);
        let r = x / half_len;
        let w = bessel_i0(KAISER_BETA * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(KAISER_BETA);
        *v = sinc * w;
    }
    // Normalise so the odd taps sum to 0.5 (DC gain of the filter is 1).
    let s: f64 = h.iter().sum();
    for v in &mut h {
        *v *= 0.5 / s;
    }
    h
}

/// Doubles the sample rate of one channel.
#[derive(Clone, Debug)]
pub struct HalfbandUp {
    taps: [f64; ODD_TAPS],
    /// Last `ODD_TAPS` inputs, newest at `pos - 1` (circular, doubled so a
    /// window can be read without wrapping).
    hist: [f64; 2 * ODD_TAPS],
    pos: usize,
}

impl Default for HalfbandUp {
    fn default() -> Self {
        Self {
            taps: odd_taps(),
            hist: [0.0; 2 * ODD_TAPS],
            pos: 0,
        }
    }
}

impl HalfbandUp {
    pub fn reset(&mut self) {
        self.hist = [0.0; 2 * ODD_TAPS];
        self.pos = 0;
    }

    /// Takes one input frame, returns two output frames (delayed by `SIDE`
    /// input frames).
    #[inline]
    pub fn process(&mut self, x: f32) -> [f32; 2] {
        let x = f64::from(x);
        self.hist[self.pos] = x;
        self.hist[self.pos + ODD_TAPS] = x;
        self.pos = (self.pos + 1) % ODD_TAPS;
        // Window oldest..newest = hist[pos .. pos + ODD_TAPS].
        let w = &self.hist[self.pos..self.pos + ODD_TAPS];
        // Even output: the centre tap (0.5 * 2) on x[n - SIDE].
        let even = w[ODD_TAPS - 1 - SIDE];
        // Odd output: sum over odd taps; x[n - SIDE + (1 - j) / 2].
        // With j = 2k - (2*SIDE - 1): (1 - j)/2 = SIDE - k, so the input is
        // x[n - k], which is w[ODD_TAPS - 1 - k].
        let mut odd = 0.0;
        for (k, h) in self.taps.iter().enumerate() {
            odd += h * w[ODD_TAPS - 1 - k];
        }
        [even as f32, (2.0 * odd) as f32]
    }
}

/// Halves the sample rate of one channel (after a 2x stage).
#[derive(Clone, Debug)]
pub struct HalfbandDown {
    taps: [f64; ODD_TAPS],
    /// Odd-phase inputs, doubled like [`HalfbandUp::hist`].
    odd: [f64; 2 * ODD_TAPS],
    /// Even-phase inputs, only the one `SIDE` frames back is used.
    even: [f64; 2 * ODD_TAPS],
    pos: usize,
}

impl Default for HalfbandDown {
    fn default() -> Self {
        Self {
            taps: odd_taps(),
            odd: [0.0; 2 * ODD_TAPS],
            even: [0.0; 2 * ODD_TAPS],
            pos: 0,
        }
    }
}

impl HalfbandDown {
    pub fn reset(&mut self) {
        self.odd = [0.0; 2 * ODD_TAPS];
        self.even = [0.0; 2 * ODD_TAPS];
        self.pos = 0;
    }

    /// Takes the two frames of one base-rate period, returns one frame
    /// (delayed by `SIDE - 1` base frames).
    #[inline]
    pub fn process(&mut self, x: [f32; 2]) -> f32 {
        let (e, o) = (f64::from(x[0]), f64::from(x[1]));
        self.even[self.pos] = e;
        self.even[self.pos + ODD_TAPS] = e;
        self.odd[self.pos] = o;
        self.odd[self.pos + ODD_TAPS] = o;
        self.pos = (self.pos + 1) % ODD_TAPS;
        // Windows oldest..newest; index ODD_TAPS - 1 is this period.
        let we = &self.even[self.pos..self.pos + ODD_TAPS];
        let wo = &self.odd[self.pos..self.pos + ODD_TAPS];
        // y = 0.5 * v[2c] + sum_j h[j] * v[2c - j] (j odd) with the centre
        // c = n - (SIDE - 1). For j = 2k - (2*SIDE - 1), v[2c - j] is the
        // odd sample of period n - k.
        let mut acc = 0.5 * we[ODD_TAPS - SIDE];
        for (k, h) in self.taps.iter().enumerate() {
            acc += h * wo[ODD_TAPS - 1 - k];
        }
        acc as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(input: &[f32], f: impl Fn([f32; 2]) -> [f32; 2]) -> Vec<f32> {
        let mut up = HalfbandUp::default();
        let mut down = HalfbandDown::default();
        input
            .iter()
            .map(|x| down.process(f(up.process(*x))))
            .collect()
    }

    #[test]
    fn round_trip_is_a_delay_in_the_passband() {
        let sr = 48_000.0;
        for freq in [50.0, 1000.0, 10_000.0, 18_000.0] {
            let input: Vec<f32> = (0..8000)
                .map(|i| (2.0 * PI * freq * i as f64 / sr).sin() as f32 * 0.5)
                .collect();
            let out = run(&input, |v| v);
            let mut err = 0.0f32;
            for i in 1000..8000 {
                err = err.max((out[i] - input[i - ROUND_TRIP_LATENCY]).abs());
            }
            assert!(err < 0.006, "{freq} Hz: {err}");
        }
    }

    #[test]
    fn dc_passes_unchanged() {
        let out = run(&[0.25; 400], |v| v);
        assert!((out[300] - 0.25).abs() < 1e-5, "{}", out[300]);
    }

    #[test]
    fn rejects_images_of_a_clipped_tone() {
        // A hard-clipped 7 kHz tone at 2x: the 3rd harmonic (21 kHz) stays,
        // the 5th (35 kHz) would alias to 13 kHz at the base rate but is
        // removed by the down filter.
        let sr = 48_000.0;
        let freq = 7000.0;
        let input: Vec<f32> = (0..16_384)
            .map(|i| (2.0 * PI * freq * i as f64 / sr).sin() as f32)
            .collect();
        let out = run(&input, |v| v.map(|x| x.clamp(-0.5, 0.5)));
        // Energy at 13 kHz (aliased 5th harmonic).
        let probe = |f: f64| {
            let (mut re, mut im) = (0.0, 0.0);
            for (i, y) in out.iter().enumerate().skip(1000) {
                let ph = 2.0 * PI * f * i as f64 / sr;
                re += f64::from(*y) * ph.cos();
                im += f64::from(*y) * ph.sin();
            }
            (re * re + im * im).sqrt() / (out.len() - 1000) as f64
        };
        let fundamental = probe(freq);
        let alias = probe(13_000.0);
        assert!(alias < fundamental * 0.01, "alias {alias} vs {fundamental}");
    }
}
