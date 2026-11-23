//! Interpolation for varispeed playback.
//!
//! [`SincResampler`] is the deck's varispeed (key lock off): a 32-tap
//! Kaiser-windowed sinc with a polyphase table of 512 fractional phases,
//! interpolated linearly between phases. When the deck plays faster than
//! normal the kernel's cut-off drops with the speed, so pitching up and fast
//! scratching do not alias. At normal speed on whole frames it passes the
//! source through unchanged.
//!
//! [`hermite`] is the cheap 4-point interpolator used by effects' delay
//! lines.

use crate::stretch::FrameSource;
use std::sync::OnceLock;

/// Third-order Hermite (Catmull-Rom) interpolation between `x0` and `x1`.
/// `t` is the fractional position in `[0, 1)`; `xm1` and `x2` are the
/// neighbours before and after.
#[inline]
pub fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c0 = x0;
    let c1 = 0.5 * (x1 - xm1);
    let c2 = xm1 - 2.5 * x0 + 2.0 * x1 - 0.5 * x2;
    let c3 = 0.5 * (x2 - xm1) + 1.5 * (x0 - x1);
    ((c3 * t + c2) * t + c1) * t + c0
}

/// Taps of the sinc kernel (source frames per output frame).
pub const SINC_TAPS: usize = 32;
const HALF: usize = SINC_TAPS / 2;
/// Fractional phases in the table.
pub const SINC_PHASES: usize = 512;
const KAISER_BETA: f64 = 8.0;
/// Speeds the tables are built for: a deck playing at `rate` uses the first
/// table whose speed is at least `rate` (its cut-off is `1 / speed` of the
/// source's Nyquist frequency). Above the last one the last table is used.
const TABLE_RATES: [f64; 9] = [1.0, 1.06, 1.12, 1.2, 1.3, 1.45, 1.7, 2.2, 3.5];

fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let q = x * x / 4.0;
    for k in 1..60 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < 1e-14 * sum {
            break;
        }
    }
    sum
}

/// The kernel tables, built once and shared by every deck.
pub struct SincBank {
    /// One table per [`TABLE_RATES`] entry: `(SINC_PHASES + 1) * SINC_TAPS`
    /// coefficients, phase-major.
    tables: Vec<Vec<f32>>,
}

impl SincBank {
    fn build() -> Self {
        let tables = TABLE_RATES
            .iter()
            .map(|&rate| {
                let fc = 1.0 / rate;
                let mut t = vec![0.0f32; (SINC_PHASES + 1) * SINC_TAPS];
                let i0b = bessel_i0(KAISER_BETA);
                for p in 0..=SINC_PHASES {
                    let frac = p as f64 / SINC_PHASES as f64;
                    let row = &mut t[p * SINC_TAPS..(p + 1) * SINC_TAPS];
                    let mut sum = 0.0;
                    let mut vals = [0.0f64; SINC_TAPS];
                    for (k, v) in vals.iter_mut().enumerate() {
                        // Tap k reads source frame floor(pos) - (HALF - 1) + k.
                        let x = (k as f64 - (HALF as f64 - 1.0)) - frac;
                        let r = x / HALF as f64;
                        let w = if r.abs() >= 1.0 {
                            0.0
                        } else {
                            bessel_i0(KAISER_BETA * (1.0 - r * r).sqrt()) / i0b
                        };
                        let arg = std::f64::consts::PI * fc * x;
                        let s = if x.abs() < 1e-12 {
                            fc
                        } else {
                            fc * arg.sin() / arg
                        };
                        *v = s * w;
                        sum += *v;
                    }
                    // Unity gain at DC for every phase.
                    for (o, v) in row.iter_mut().zip(vals) {
                        *o = (v / sum) as f32;
                    }
                }
                if rate <= 1.0 {
                    // Whole frames at normal speed: exactly the source.
                    for p in [0, SINC_PHASES] {
                        let row = &mut t[p * SINC_TAPS..(p + 1) * SINC_TAPS];
                        row.fill(0.0);
                        let k = if p == 0 { HALF - 1 } else { HALF };
                        row[k] = 1.0;
                    }
                }
                t
            })
            .collect();
        Self { tables }
    }

    /// The shared bank (built on first use; call [`SincBank::get`] once off
    /// the audio thread, which [`SincResampler::new`] does).
    pub fn get() -> &'static SincBank {
        static BANK: OnceLock<SincBank> = OnceLock::new();
        BANK.get_or_init(SincBank::build)
    }

    fn table(&self, rate: f64) -> &[f32] {
        let r = rate.abs();
        let i = TABLE_RATES
            .iter()
            .position(|&t| r <= t + 1e-9)
            .unwrap_or(TABLE_RATES.len() - 1);
        self.tables.get(i).map_or(&[], |t| t.as_slice())
    }
}

/// How far a block of `n` frames moves when the speed ramps from `rate0`
/// to `rate1` the way [`SincResampler::render`] ramps it.
#[inline]
pub fn ramp_advance(rate0: f64, rate1: f64, n: usize) -> f64 {
    rate0 * n as f64 + (rate1 - rate0) * (n as f64 + 1.0) * 0.5
}

/// Windowed-sinc varispeed reader with a per-block source cache.
pub struct SincResampler {
    bank: &'static SincBank,
    cache_l: Vec<f32>,
    cache_r: Vec<f32>,
    coeffs: [f32; SINC_TAPS],
}

impl SincResampler {
    /// `max_block` frames per call at up to `max_rate` times normal speed
    /// are read through the cache; faster blocks read the source directly.
    pub fn new(max_block: usize, max_rate: f64) -> Self {
        let cap = (max_block as f64 * max_rate.abs().max(1.0)).ceil() as usize + SINC_TAPS + 8;
        Self {
            bank: SincBank::get(),
            cache_l: crate::prefaulted_zeros(cap),
            cache_r: crate::prefaulted_zeros(cap),
            coeffs: [0.0; SINC_TAPS],
        }
    }

    /// Kernel for the fractional position `t` in `[0, 1)`.
    #[inline]
    fn kernel(&mut self, table: &[f32], t: f64) {
        let f = t * SINC_PHASES as f64;
        let p = (f as usize).min(SINC_PHASES - 1);
        let w = (f - p as f64) as f32;
        let (Some(a), Some(b)) = (
            table.get(p * SINC_TAPS..(p + 1) * SINC_TAPS),
            table.get((p + 1) * SINC_TAPS..(p + 2) * SINC_TAPS),
        ) else {
            self.coeffs = [0.0; SINC_TAPS];
            return;
        };
        if w == 0.0 {
            self.coeffs.copy_from_slice(a);
        } else {
            for ((c, x), y) in self.coeffs.iter_mut().zip(a).zip(b) {
                *c = x + w * (y - x);
            }
        }
    }

    /// Renders `out_l.len()` frames starting at source position `start`.
    /// The speed moves linearly from `rate0` to `rate1` over the block (so
    /// speed changes never step at block edges). Returns the position after
    /// the block.
    pub fn render<S: FrameSource>(
        &mut self,
        src: &S,
        start: f64,
        rate0: f64,
        rate1: f64,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) -> f64 {
        let n = out_l.len().min(out_r.len());
        if n == 0 {
            return start;
        }
        let step = (rate1 - rate0) / n as f64;
        let rate_at = |i: usize| rate0 + step * (i as f64 + 1.0);
        // Range of source frames the block touches.
        let (mut lo, mut hi, mut pos) = (start, start, start);
        for i in 0..n {
            lo = lo.min(pos);
            hi = hi.max(pos);
            pos += rate_at(i);
        }
        let first = lo.floor() as i64 - (HALF as i64 - 1);
        let last = hi.floor() as i64 + HALF as i64;
        let count = (last - first + 1).max(0) as usize;
        let table = self.bank.table(rate0.abs().max(rate1.abs()));
        let cached = count <= self.cache_l.len();
        if cached {
            for (k, (cl, cr)) in self
                .cache_l
                .iter_mut()
                .zip(self.cache_r.iter_mut())
                .take(count)
                .enumerate()
            {
                let [a, b] = src.frame(first + k as i64);
                *cl = a;
                *cr = b;
            }
        }
        let mut pos = start;
        for (i, (l, r)) in out_l.iter_mut().zip(out_r.iter_mut()).enumerate() {
            let fl = pos.floor();
            self.kernel(table, pos - fl);
            let base = fl as i64 - (HALF as i64 - 1);
            let (mut yl, mut yr) = (0.0f32, 0.0f32);
            if cached {
                let off = (base - first) as usize;
                if let (Some(wl), Some(wr)) = (
                    self.cache_l.get(off..off + SINC_TAPS),
                    self.cache_r.get(off..off + SINC_TAPS),
                ) {
                    for ((c, a), b) in self.coeffs.iter().zip(wl).zip(wr) {
                        yl += c * a;
                        yr += c * b;
                    }
                }
            } else {
                for (k, c) in self.coeffs.iter().enumerate() {
                    let [a, b] = src.frame(base + k as i64);
                    yl += c * a;
                    yr += c * b;
                }
            }
            *l = yl;
            *r = yr;
            pos += rate_at(i);
        }
        start + ramp_advance(rate0, rate1, n)
    }

    /// Renders one frame per entry of `positions` (source positions, any
    /// path: scratching). The anti-alias cut-off follows the speed at each
    /// frame.
    pub fn render_positions<S: FrameSource>(
        &mut self,
        src: &S,
        positions: &[f64],
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let n = positions.len().min(out_l.len()).min(out_r.len());
        if n == 0 {
            return;
        }
        let (lo, hi) = positions[..n]
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), p| {
                (a.min(*p), b.max(*p))
            });
        let first = lo.floor() as i64 - (HALF as i64 - 1);
        let last = hi.floor() as i64 + HALF as i64;
        let count = (last - first + 1).max(0) as usize;
        let cached = count <= self.cache_l.len();
        if cached {
            for (k, (cl, cr)) in self
                .cache_l
                .iter_mut()
                .zip(self.cache_r.iter_mut())
                .take(count)
                .enumerate()
            {
                let [a, b] = src.frame(first + k as i64);
                *cl = a;
                *cr = b;
            }
        }
        let bank = self.bank;
        for i in 0..n {
            let pos = positions[i];
            let speed = if i + 1 < n {
                positions[i + 1] - pos
            } else if i > 0 {
                pos - positions[i - 1]
            } else {
                1.0
            };
            let table = bank.table(speed);
            let fl = pos.floor();
            self.kernel(table, pos - fl);
            let base = fl as i64 - (HALF as i64 - 1);
            let (mut yl, mut yr) = (0.0f32, 0.0f32);
            if cached {
                let off = (base - first) as usize;
                if let (Some(wl), Some(wr)) = (
                    self.cache_l.get(off..off + SINC_TAPS),
                    self.cache_r.get(off..off + SINC_TAPS),
                ) {
                    for ((c, a), b) in self.coeffs.iter().zip(wl).zip(wr) {
                        yl += c * a;
                        yr += c * b;
                    }
                }
            } else {
                for (k, c) in self.coeffs.iter().enumerate() {
                    let [a, b] = src.frame(base + k as i64);
                    yl += c * a;
                    yr += c * b;
                }
            }
            if let (Some(l), Some(r)) = (out_l.get_mut(i), out_r.get_mut(i)) {
                *l = yl;
                *r = yr;
            }
        }
    }
}

/// How far apart the jog's steps arrive, so the platter can follow slow
/// movements smoothly. A jog sends one step per small angle (about 86
/// frames of audio on the NS4FX); moved slowly, the steps come tens of
/// milliseconds apart, and a quick follower would move, stop and move
/// again on each one.
#[derive(Clone, Copy, Debug)]
pub struct JogPace {
    /// Frames since the target last moved.
    since: usize,
    /// Smoothed frames between moves.
    interval: f64,
}

impl Default for JogPace {
    fn default() -> Self {
        Self {
            since: 0,
            interval: 0.0,
        }
    }
}

impl JogPace {
    /// After a block of `n` frames in which the target `moved` or not;
    /// returns the time constant (frames) to follow with: at least
    /// `min_tau`, about the step interval for slow movements, at most
    /// `max_tau`. A hand that stops keeps the last (quick) value, so stops
    /// stay tight.
    pub fn update(&mut self, moved: bool, n: usize, min_tau: f64, max_tau: f64) -> f64 {
        self.since += n;
        if moved {
            let gap = (self.since as f64).min(max_tau * 2.0);
            self.interval = if self.interval == 0.0 {
                gap
            } else {
                0.6 * self.interval + 0.4 * gap
            };
            self.since = 0;
        }
        self.interval.clamp(min_tau, max_tau)
    }
}

/// A platter that follows a target position smoothly: a critically damped
/// second-order follower, stepped once per output frame. Jog ticks arrive
/// in bursts (once per audio block); following them directly modulates the
/// speed at the block rate, which sounds rough. The follower turns the
/// bursts into a continuous speed.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlatterFollower {
    pub position: f64,
    /// Frames per frame.
    pub velocity: f64,
}

impl PlatterFollower {
    /// Steps one frame towards `target` with time constant `tau` (frames);
    /// the speed is limited to `max_speed`. Returns the position before the
    /// step (the frame to play now).
    #[inline]
    pub fn step(&mut self, target: f64, tau: f64, max_speed: f64) -> f64 {
        let w = 1.0 / tau.max(1.0);
        let here = self.position;
        let acc = w * w * (target - self.position) - 2.0 * w * self.velocity;
        self.velocity = (self.velocity + acc).clamp(-max_speed, max_speed);
        self.position += self.velocity;
        here
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use std::f64::consts::PI;

    #[test]
    fn hits_sample_points() {
        assert_eq!(hermite(1.0, 2.0, 3.0, 5.0, 0.0), 2.0);
        assert!((hermite(1.0, 2.0, 3.0, 5.0, 1.0) - 3.0).abs() < 1e-6);
    }

    proptest! {
        /// Catmull-Rom reproduces quadratics exactly.
        #[test]
        fn exact_for_quadratics(a in -2.0f32..2.0, b in -2.0f32..2.0, c in -2.0f32..2.0, t in 0.0f32..1.0) {
            let f = |x: f32| a * x * x + b * x + c;
            let y = hermite(f(-1.0), f(0.0), f(1.0), f(2.0), t);
            prop_assert!((y - f(t)).abs() < 1e-4, "{} vs {}", y, f(t));
        }
    }

    struct Tone {
        freq: f64,
        sr: f64,
    }

    impl FrameSource for Tone {
        fn frame(&self, i: i64) -> [f32; 2] {
            let v = (0.5 * (2.0 * PI * self.freq * i as f64 / self.sr).sin()) as f32;
            [v, -v]
        }
    }

    struct Ramp;

    impl FrameSource for Ramp {
        fn frame(&self, i: i64) -> [f32; 2] {
            let v = (i as f32 * 0.001).sin();
            [v, v * 0.5]
        }
    }

    #[test]
    fn normal_speed_on_whole_frames_is_exact() {
        let mut rs = SincResampler::new(256, 8.0);
        let mut l = vec![0.0; 256];
        let mut r = vec![0.0; 256];
        let end = rs.render(&Ramp, 1000.0, 1.0, 1.0, &mut l, &mut r);
        assert_eq!(end, 1256.0);
        for (i, (a, b)) in l.iter().zip(r.iter()).enumerate() {
            let [x, y] = Ramp.frame(1000 + i as i64);
            assert_eq!(*a, x);
            assert_eq!(*b, y);
        }
    }

    #[test]
    fn half_frame_offset_is_accurate_up_to_18_khz() {
        let sr = 48_000.0;
        for freq in [100.0, 5_000.0, 12_000.0, 18_000.0] {
            let src = Tone { freq, sr };
            let mut rs = SincResampler::new(512, 8.0);
            let mut l = vec![0.0; 512];
            let mut r = vec![0.0; 512];
            rs.render(&src, 5000.5, 1.0, 1.0, &mut l, &mut r);
            let mut err = 0.0f32;
            for (i, y) in l.iter().enumerate() {
                let t = 5000.5 + i as f64;
                let want = (0.5 * (2.0 * PI * freq * t / sr).sin()) as f32;
                err = err.max((y - want).abs());
            }
            // 0.5 amplitude; -50 dB error or better, Hermite manages
            // about -12 dB at 18 kHz.
            assert!(err < 0.5 * 0.0032, "{freq} Hz: error {err}");
        }
    }

    #[test]
    fn fast_playback_does_not_alias() {
        // An 18 kHz tone played at 2x would land at 36 kHz, above the
        // output's Nyquist frequency: it must come out (nearly) silent,
        // not as a 12 kHz alias.
        let src = Tone {
            freq: 18_000.0,
            sr: 48_000.0,
        };
        let mut rs = SincResampler::new(1024, 8.0);
        let mut l = vec![0.0; 1024];
        let mut r = vec![0.0; 1024];
        rs.render(&src, 10_000.0, 2.0, 2.0, &mut l, &mut r);
        let peak = l.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak < 0.5 * 0.03, "alias peak {peak}");
    }

    #[test]
    fn speed_ramps_within_the_block() {
        let mut rs = SincResampler::new(256, 8.0);
        let mut l = vec![0.0; 100];
        let mut r = vec![0.0; 100];
        let end = rs.render(&Ramp, 0.0, 1.0, 2.0, &mut l, &mut r);
        // Sum of 1.01, 1.02, ..., 2.00.
        let want: f64 = (1..=100).map(|i| 1.0 + i as f64 / 100.0).sum();
        assert!((end - want).abs() < 1e-9, "{end} vs {want}");
        let end = rs.render(&Ramp, 50.0, -1.0, -1.0, &mut l, &mut r);
        assert!((end + 50.0).abs() < 1e-9);
    }

    #[test]
    fn uncached_reads_match_cached() {
        let src = Tone {
            freq: 440.0,
            sr: 48_000.0,
        };
        let mut a = SincResampler::new(64, 1.0);
        let mut b = SincResampler::new(64, 8.0);
        let (mut l1, mut r1) = (vec![0.0; 64], vec![0.0; 64]);
        let (mut l2, mut r2) = (vec![0.0; 64], vec![0.0; 64]);
        // 6x: too much for `a`'s cache.
        a.render(&src, 77.3, 6.0, 6.0, &mut l1, &mut r1);
        b.render(&src, 77.3, 6.0, 6.0, &mut l2, &mut r2);
        assert_eq!(l1, l2);
        assert_eq!(r1, r2);
    }

    #[test]
    fn platter_follower_smooths_bursty_jog_ticks() {
        // Play speed on the jog, but the ticks of each 256-frame block
        // alternate 230 / 282 frames (MIDI messages do not line up with
        // the blocks): the followed speed stays within 3 % of 1.0, where
        // following block by block would jump by 10 %.
        let tau = 48_000.0 * 0.004;
        let mut f = PlatterFollower::default();
        let mut target = 0.0f64;
        let mut speeds = Vec::new();
        for b in 0..400 {
            let prev = target;
            target += if b % 2 == 0 { 230.0 } else { 282.0 };
            for i in 0..256 {
                let t = prev + (target - prev) * (i + 1) as f64 / 256.0;
                f.step(t, tau, 8.0);
                speeds.push(f.velocity);
            }
        }
        let tail = &speeds[speeds.len() / 2..];
        let worst = tail.iter().fold(0.0f64, |m, v| m.max((v - 1.0).abs()));
        assert!(worst < 0.03, "speed ripple {worst}");
        // And it keeps up: the position stays within 10 ms of the jog.
        assert!(
            (f.position - target).abs() < 480.0,
            "{}",
            f.position - target
        );
    }

    /// Slow scratching: a jog step (86 frames) every 3 or 4 blocks, about
    /// 10 % speed. The platter keeps moving between the steps instead of
    /// stopping (with a fixed quick follower it stops dead between them).
    fn slow_scratch_speeds(adaptive: bool) -> Vec<f64> {
        let sr = 48_000.0;
        let mut f = PlatterFollower::default();
        let mut pace = JogPace::default();
        let mut target = 0.0f64;
        let mut speeds = Vec::new();
        for b in 0..600usize {
            let prev = target;
            if b % 7 == 0 || b % 7 == 4 {
                target += 86.0;
            }
            let tau = if adaptive {
                pace.update(target != prev, 256, sr * 0.004, sr * 0.025)
            } else {
                sr * 0.004
            };
            for i in 0..256 {
                let t = prev + (target - prev) * (i + 1) as f64 / 256.0;
                f.step(t, tau, 8.0);
                speeds.push(f.velocity);
            }
        }
        speeds.split_off(speeds.len() / 2)
    }

    #[test]
    fn slow_scratching_moves_continuously() {
        let mean = 2.0 * 86.0 / (7.0 * 256.0);
        let s = slow_scratch_speeds(true);
        let lo = s.iter().fold(f64::MAX, |m, v| m.min(*v));
        let hi = s.iter().fold(0.0f64, |m, v| m.max(*v));
        assert!(lo > 0.85 * mean, "slows between steps: {lo} (mean {mean})");
        assert!(hi < 1.15 * mean, "jerks: {hi} (mean {mean})");
        // The fixed quick follower (before) swung from about 20 % to 175 %
        // of the speed on every step: the rough sound.
        let old = slow_scratch_speeds(false);
        let (olo, ohi) = (
            old.iter().fold(f64::MAX, |m, v| m.min(*v)),
            old.iter().fold(0.0f64, |m, v| m.max(*v)),
        );
        assert!(olo < 0.5 * mean && ohi > 1.5 * mean, "{olo} {ohi}");
    }
}
