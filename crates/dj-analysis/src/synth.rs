//! Synthetic test audio: tones, click tracks at a known tempo, noise.
//! Used by tests, benchmarks and the smoke test, so nothing depends on real
//! music files.

use dj_decode::TrackAudio;
use std::f64::consts::PI;
use std::sync::Arc;

/// Small deterministic pseudo-random generator (xorshift64*).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform in `[-1, 1)`.
    pub fn bipolar(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 / (1u64 << 23) as f32) - 1.0
    }
}

/// Description of a click track.
#[derive(Clone, Debug)]
pub struct ClickTrack {
    pub sample_rate: u32,
    pub bpm: f64,
    /// Time of the first beat in seconds.
    pub first_beat: f64,
    pub seconds: f64,
    /// Amplitude of background noise (0 for none).
    pub noise: f32,
    /// Every n-th beat is accented (a bar), 0 for none.
    pub accent_every: u32,
    /// Adds a sustained bass note per beat, like a kick drum's body.
    pub kick_body: bool,
    pub seed: u64,
}

impl ClickTrack {
    pub fn new(bpm: f64, seconds: f64) -> Self {
        Self {
            sample_rate: 44_100,
            bpm,
            first_beat: 0.1,
            seconds,
            noise: 0.0,
            accent_every: 4,
            kick_body: true,
            seed: 1,
        }
    }

    /// Renders interleaved stereo samples.
    pub fn render(&self) -> Vec<f32> {
        let sr = f64::from(self.sample_rate);
        let n = (self.seconds * sr) as usize;
        let mut mono = vec![0.0f32; n];
        let beat = 60.0 / self.bpm;
        let mut rng = Rng::new(self.seed);
        let mut k = 0u32;
        let mut t = self.first_beat;
        while t < self.seconds {
            let start = (t * sr).round() as usize;
            let accent = self.accent_every > 0 && k.is_multiple_of(self.accent_every);
            let amp = if accent { 0.8 } else { 0.55 };
            // Click: 3 ms burst of a 3 kHz tone with a fast decay, plus a
            // decaying 55 Hz body.
            let click_len = (0.003 * sr) as usize;
            let body_len = (0.12 * sr) as usize;
            for i in 0..body_len.max(click_len) {
                let Some(s) = mono.get_mut(start + i) else {
                    break;
                };
                let ti = i as f64 / sr;
                if i < click_len {
                    let env = (-(ti / 0.0008)).exp();
                    *s += (amp * env * (2.0 * PI * 3000.0 * ti).sin()) as f32;
                }
                if self.kick_body {
                    let env = (-(ti / 0.04)).exp();
                    *s += (0.5 * amp * env * (2.0 * PI * 55.0 * ti).sin()) as f32;
                }
            }
            k += 1;
            t = self.first_beat + f64::from(k) * beat;
        }
        let mut out = Vec::with_capacity(n * 2);
        for s in mono {
            let v = s + self.noise * rng.bipolar();
            out.push(v);
            out.push(v);
        }
        out
    }

    pub fn to_track(&self) -> Arc<TrackAudio> {
        TrackAudio::from_interleaved(self.sample_rate, &self.render())
    }
}

/// A stereo sine (or sum of sines) at amplitude `amp`.
pub fn tones(sample_rate: u32, freqs: &[f64], seconds: f64, amp: f64) -> Vec<f32> {
    let sr = f64::from(sample_rate);
    let n = (seconds * sr) as usize;
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / sr;
        let v: f64 = freqs.iter().map(|f| (2.0 * PI * f * t).sin()).sum::<f64>() * amp
            / freqs.len().max(1) as f64;
        out.push(v as f32);
        out.push(v as f32);
    }
    out
}

/// White noise, stereo, deterministic.
pub fn noise(sample_rate: u32, seconds: f64, amp: f32, seed: u64) -> Vec<f32> {
    let n = (seconds * f64::from(sample_rate)) as usize;
    let mut rng = Rng::new(seed);
    (0..n * 2).map(|_| amp * rng.bipolar()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_track_has_beats_where_expected() {
        let c = ClickTrack {
            noise: 0.0,
            kick_body: false,
            ..ClickTrack::new(120.0, 2.0)
        };
        let s = c.render();
        let at = |sec: f64| s[((sec * 44_100.0) as usize + 20) * 2].abs();
        assert!(at(0.1) > 0.1);
        assert!(at(0.6) > 0.1);
        assert_eq!(at(0.35), 0.0);
    }

    #[test]
    fn rng_is_deterministic_and_bounded() {
        let mut a = Rng::new(5);
        let mut b = Rng::new(5);
        for _ in 0..1000 {
            let x = a.bipolar();
            assert_eq!(x, b.bipolar());
            assert!((-1.0..1.0).contains(&x));
        }
    }
}
