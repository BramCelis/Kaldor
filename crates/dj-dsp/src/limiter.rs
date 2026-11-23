//! Master bus protection, two stages:
//!
//! 1. **Soft clipper**: transparent up to -3 dBFS, then a `tanh` knee that
//!    rounds peaks off instead of squaring them. It runs at 2x oversampling
//!    so its harmonics do not fold back as aliasing. Only the clipper's
//!    *correction* (clipped minus input) goes through the oversampler; the
//!    signal itself is just delayed, so below -3 dBFS the stage is a pure,
//!    bit-exact delay.
//! 2. **Lookahead brickwall limiter**: ceiling -0.3 dBFS, 1.5 ms lookahead.
//!    The gain needed by every sample is known 1.5 ms early, so the gain
//!    ramps down *before* a peak arrives (no overshoot, no distortion from
//!    an instant gain change). Release is program dependent: 50 ms after a
//!    short transient, slowing to 250 ms when the limiter works
//!    continuously, so it does not pump.
//!
//! The whole chain delays the signal by [`MasterLimiter::latency`] frames
//! (about 2 ms at 48 kHz), also when it is switched off, so switching it
//! never jumps.

use crate::oversample::{HalfbandDown, HalfbandUp, ROUND_TRIP_LATENCY};

/// Soft clipper threshold, -3 dBFS.
pub const CLIP_THRESHOLD: f32 = 0.707_945_8;
/// Limiter ceiling, -0.3 dBFS.
pub const CEILING: f32 = 0.966_050_9;
const LOOKAHEAD_MS: f32 = 1.5;
const RELEASE_FAST_MS: f32 = 50.0;
const RELEASE_SLOW_MS: f32 = 250.0;
/// Continuous limiting for this long moves the release to the slow end.
const RELEASE_SLOW_AFTER_S: f32 = 0.5;

/// The soft clip curve: identity up to [`CLIP_THRESHOLD`], then a `tanh`
/// knee towards 1.0.
#[inline]
pub fn soft_clip(x: f32) -> f32 {
    let a = x.abs();
    if a <= CLIP_THRESHOLD {
        return x;
    }
    let room = 1.0 - CLIP_THRESHOLD;
    (CLIP_THRESHOLD + room * ((a - CLIP_THRESHOLD) / room).tanh()).copysign(x)
}

/// A fixed delay line.
#[derive(Clone, Debug)]
struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    fn new(frames: usize) -> Self {
        Self {
            buf: vec![0.0; frames.max(1)],
            pos: 0,
        }
    }

    /// Pushes `x`, returns the value `len` frames ago (`len` = 0 is not
    /// supported; the minimum delay is one frame per slot).
    #[inline]
    fn push(&mut self, x: f32) -> f32 {
        let Some(slot) = self.buf.get_mut(self.pos) else {
            return x;
        };
        let y = std::mem::replace(slot, x);
        self.pos += 1;
        if self.pos >= self.buf.len() {
            self.pos = 0;
        }
        y
    }
}

/// Soft clipper plus lookahead limiter for a stereo bus.
#[derive(Clone, Debug)]
pub struct MasterLimiter {
    up: [HalfbandUp; 2],
    down: [HalfbandDown; 2],
    clip_delay: [Delay; 2],
    /// Lookahead length in frames.
    look: usize,
    out_delay: [Delay; 2],
    /// Last `look` required gains (sliding minimum).
    hold: Vec<f32>,
    hold_pos: usize,
    min_val: f32,
    min_age: usize,
    /// Last `look` held minima (box filter) and their sum.
    boxbuf: Vec<f32>,
    box_pos: usize,
    box_sum: f64,
    env: f32,
    rel_fast: f32,
    rel_slow: f32,
    /// Frames of recent continuous limiting (for the release).
    busy: f32,
    busy_full: f32,
    /// Clipper on/off ramp (0 to 1) and its step.
    clip_amount: f32,
    clip_step: f32,
    /// Lowest gain of the last block (1.0 = no gain reduction) of either
    /// stage, for the LIM light.
    pub gain: f32,
    /// Where the limiter catches (linear); [`CEILING`] by default.
    ceiling: f32,
}

impl MasterLimiter {
    pub fn new(sample_rate: u32) -> Self {
        let sr = sample_rate.max(1) as f32;
        let look = ((LOOKAHEAD_MS * 0.001 * sr).round() as usize).max(2);
        let coeff = |ms: f32| 1.0 - (-1.0 / (ms * 0.001 * sr)).exp();
        Self {
            up: Default::default(),
            down: Default::default(),
            clip_delay: [
                Delay::new(ROUND_TRIP_LATENCY),
                Delay::new(ROUND_TRIP_LATENCY),
            ],
            look,
            out_delay: [Delay::new(look - 1), Delay::new(look - 1)],
            hold: vec![1.0; look],
            hold_pos: 0,
            min_val: 1.0,
            min_age: 0,
            boxbuf: vec![1.0; look],
            box_pos: 0,
            box_sum: look as f64,
            env: 1.0,
            rel_fast: coeff(RELEASE_FAST_MS),
            rel_slow: coeff(RELEASE_SLOW_MS),
            busy: 0.0,
            busy_full: RELEASE_SLOW_AFTER_S * sr,
            clip_amount: 1.0,
            clip_step: 1.0 / (0.01 * sr),
            gain: 1.0,
            ceiling: CEILING,
        }
    }

    /// Delay of the chain in frames.
    pub fn latency(&self) -> usize {
        ROUND_TRIP_LATENCY + self.look - 1
    }

    pub fn ceiling(&self) -> f32 {
        self.ceiling
    }

    /// Sets the ceiling in dBFS (clamped to -2.0 to -0.1). Takes effect from
    /// the next frame; the gain envelope follows it smoothly.
    pub fn set_ceiling_db(&mut self, db: f32) {
        self.ceiling = 10f32.powf(db.clamp(-2.0, -0.1) / 20.0);
    }

    #[inline]
    fn sliding_min(&mut self, g: f32) -> f32 {
        let n = self.hold.len();
        if let Some(slot) = self.hold.get_mut(self.hold_pos) {
            *slot = g;
        }
        self.hold_pos = (self.hold_pos + 1) % n;
        if g <= self.min_val {
            self.min_val = g;
            self.min_age = 0;
        } else {
            self.min_age += 1;
            if self.min_age >= n {
                // The minimum left the window: find the new one.
                let mut best = f32::INFINITY;
                let mut age = 0;
                for (i, v) in self.hold.iter().enumerate() {
                    let a = (self.hold_pos + 2 * n - 1 - i) % n;
                    if *v < best || (*v == best && a < age) {
                        best = *v;
                        age = a;
                    }
                }
                self.min_val = best;
                self.min_age = age;
            }
        }
        self.min_val
    }

    #[inline]
    fn box_filter(&mut self, h: f32) -> f32 {
        let n = self.boxbuf.len();
        if let Some(slot) = self.boxbuf.get_mut(self.box_pos) {
            self.box_sum += f64::from(h) - f64::from(*slot);
            *slot = h;
        }
        self.box_pos = (self.box_pos + 1) % n;
        if self.box_pos == 0 {
            // Re-sum once per window so rounding never drifts.
            self.box_sum = self.boxbuf.iter().map(|v| f64::from(*v)).sum();
        }
        let s = (self.box_sum / n as f64) as f32;
        if s > 0.999_999 { 1.0 } else { s }
    }

    /// Processes a stereo block in place. With `enabled` off the signal is
    /// only delayed (the gain recovers smoothly if it was limiting).
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32], enabled: bool) {
        let mut lowest = 1.0f32;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // Stage 1: soft clip at 2x.
            if enabled {
                self.clip_amount = (self.clip_amount + self.clip_step).min(1.0);
            } else {
                self.clip_amount = (self.clip_amount - self.clip_step).max(0.0);
            }
            let amount = self.clip_amount;
            let mut c = [0.0f32; 2];
            for (ch, x) in [*l, *r].into_iter().enumerate() {
                let u = self.up[ch].process(x);
                let d = [
                    (soft_clip(u[0]) - u[0]) * amount,
                    (soft_clip(u[1]) - u[1]) * amount,
                ];
                let dd = self.down[ch].process(d);
                let xd = self.clip_delay[ch].push(x);
                c[ch] = xd + dd;
                // The clipper's own gain reduction counts for the LIM light.
                if dd != 0.0 && xd.abs() > CLIP_THRESHOLD {
                    lowest = lowest.min((c[ch] / xd).abs().min(1.0));
                }
            }
            // Stage 2: lookahead limiter.
            let peak = c[0].abs().max(c[1].abs());
            let ceiling = self.ceiling;
            let need = if enabled && peak > ceiling {
                ceiling / peak
            } else {
                1.0
            };
            if need < 1.0 {
                self.busy = (self.busy + 1.0).min(self.busy_full);
            } else {
                self.busy = (self.busy - 0.05).max(0.0);
            }
            let held = self.sliding_min(need);
            let s = self.box_filter(held);
            if s <= self.env {
                self.env = s;
            } else {
                let t = self.busy / self.busy_full;
                let rel = self.rel_fast + (self.rel_slow - self.rel_fast) * t;
                self.env += (s - self.env) * rel;
                if self.env > 0.999_999 {
                    self.env = 1.0;
                }
            }
            lowest = lowest.min(self.env);
            let g = self.env;
            let ol = self.out_delay[0].push(c[0]) * g;
            let or = self.out_delay[1].push(c[1]) * g;
            if enabled {
                *l = ol.clamp(-ceiling, ceiling);
                *r = or.clamp(-ceiling, ceiling);
            } else {
                *l = ol;
                *r = or;
            }
        }
        self.gain = lowest;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn sine(amp: f64, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * PI * 100.0 * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    #[test]
    fn never_exceeds_ceiling() {
        let mut lim = MasterLimiter::new(48_000);
        let mut l = sine(3.0, 48_000);
        let mut r = sine(-2.0, 48_000);
        for (a, b) in l.chunks_mut(256).zip(r.chunks_mut(256)) {
            lim.process(a, b, true);
        }
        let max = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(max <= lim.ceiling() + 1e-6, "max {max}");
    }

    #[test]
    fn transparent_below_the_clip_threshold() {
        let mut lim = MasterLimiter::new(48_000);
        let orig = sine(0.5, 4800);
        let mut l = orig.clone();
        let mut r = orig.clone();
        lim.process(&mut l, &mut r, true);
        let d = lim.latency();
        assert_eq!(&l[d..], &orig[..orig.len() - d], "a pure delay");
        assert_eq!(lim.gain, 1.0);
    }

    #[test]
    fn clipper_alone_shows_in_the_gain() {
        // -1 dBFS peaks: the clipper works, the brickwall stage does not.
        let mut lim = MasterLimiter::new(48_000);
        let mut l = sine(0.89, 4800);
        let mut r = l.clone();
        lim.process(&mut l, &mut r, true);
        assert!(lim.gain < 0.999 && lim.gain > 0.9, "{}", lim.gain);
        let mut l = sine(0.5, 4800);
        let mut r = l.clone();
        lim.process(&mut l, &mut r, true);
        assert_eq!(lim.gain, 1.0, "quiet again");
    }

    #[test]
    fn soft_clip_curve() {
        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.7), -0.7);
        let y = soft_clip(1.0);
        assert!(y > 0.9 && y < 0.95, "{y}");
        assert!(soft_clip(10.0) <= 1.0);
        assert!((soft_clip(-2.0) + soft_clip(2.0)).abs() < 1e-7);
    }

    #[test]
    fn gain_ramps_down_before_a_peak() {
        // A single loud click after silence-level signal: the gain is
        // already reduced when it comes out, and the samples just before
        // it are reduced a little too (the lookahead ramp).
        let mut lim = MasterLimiter::new(48_000);
        let mut l = vec![0.1f32; 2000];
        l[1000] = 2.0;
        let mut r = l.clone();
        lim.process(&mut l, &mut r, true);
        let d = lim.latency();
        let out_peak = l[1000 + d];
        assert!(out_peak <= CEILING + 1e-6, "{out_peak}");
        assert!(out_peak > 0.9, "the peak reaches the ceiling: {out_peak}");
        assert!(l[1000 + d - 10] < 0.1, "ramped before the peak");
        assert!((l[900 + d] - 0.1).abs() < 1e-7, "untouched well before");
    }

    #[test]
    fn release_is_slower_after_long_limiting() {
        let measure = |loud_frames: usize| {
            let mut lim = MasterLimiter::new(48_000);
            let mut l = sine(2.0, loud_frames);
            let mut r = l.clone();
            lim.process(&mut l, &mut r, true);
            // Then quiet: frames until the gain is back to (nearly) 1.
            let mut frames = 0;
            loop {
                let mut a = [0.1f32; 64];
                let mut b = a;
                lim.process(&mut a, &mut b, true);
                frames += 64;
                if lim.gain >= 0.9999 || frames > 480_000 {
                    return frames;
                }
            }
        };
        let short = measure(480);
        let long = measure(48_000);
        assert!(long > short * 2, "short {short}, long {long}");
    }

    #[test]
    fn off_is_a_pure_delay() {
        let mut lim = MasterLimiter::new(48_000);
        let orig = sine(1.5, 4800);
        let mut l = orig.clone();
        let mut r = orig.clone();
        // Start switched off (the clip ramp starts on: run it down first).
        let mut a = vec![0.0; 1000];
        let mut b = a.clone();
        lim.process(&mut a, &mut b, false);
        let mut lim2 = lim.clone();
        lim2.process(&mut l, &mut r, false);
        let d = lim2.latency();
        assert_eq!(&l[d..], &orig[..orig.len() - d]);
    }
}
