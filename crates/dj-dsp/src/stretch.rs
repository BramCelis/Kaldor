//! Time-stretching for key lock (master tempo).
//!
//! [`TimeStretcher`] is the interface the deck uses. Two implementations:
//! [`crate::SignalsmithStretch`] (spectral, the first choice, used by
//! the decks) and [`Wsola`] here (waveform-similarity overlap-add, written
//! from the textbook algorithm; lighter, but it warbles on full mixes).
//!
//! The stretcher reads its input by random access from a [`FrameSource`]
//! (decks hold the whole decoded track), so it needs no input FIFO: the
//! caller passes the deck's exact position for the first output frame of
//! each block, and the stretcher keeps the output aligned to it.

/// Random-access stereo input. Out-of-range indices return silence.
pub trait FrameSource {
    fn frame(&self, index: i64) -> [f32; 2];
}

/// A time-stretcher: plays `source` at `tempo` times normal speed without
/// changing pitch.
pub trait TimeStretcher: Send {
    /// Renders `out_l.len()` frames. `position` is the source position (in
    /// frames) of the first output frame; after the call the caller advances
    /// its own position by `tempo * out_l.len()`. A `position` that does not
    /// follow on from the previous call (a seek) resets the stretcher.
    fn process(
        &mut self,
        source: &dyn FrameSource,
        position: f64,
        tempo: f64,
        out_l: &mut [f32],
        out_r: &mut [f32],
    );

    /// Forgets all history (call after loading a new track).
    fn reset(&mut self);

    /// Shifts the pitch by `semitones` (key shift). Stretchers that cannot
    /// shift pitch ignore it.
    fn set_transpose_semitones(&mut self, semitones: f64) {
        let _ = semitones;
    }
}

/// WSOLA time-stretcher. All buffers are allocated in [`Wsola::new`];
/// `process` never allocates.
pub struct Wsola {
    hop: usize,
    win: usize,
    tol: usize,
    window: Vec<f32>,
    ola_l: Vec<f32>,
    ola_r: Vec<f32>,
    fifo_l: Vec<f32>,
    fifo_r: Vec<f32>,
    fifo_len: usize,
    fifo_pos: usize,
    prev_start: Option<i64>,
    expected_position: Option<f64>,
    region_l: Vec<f32>,
    region_r: Vec<f32>,
    region_mono: Vec<f32>,
    template: Vec<f32>,
}

impl Wsola {
    /// Creates a stretcher for `sample_rate`. Grain length is about 32 ms
    /// with 50 % overlap; the similarity search covers about ±8 ms.
    pub fn new(sample_rate: u32) -> Self {
        let hop = ((sample_rate as f64 * 0.016).round() as usize).max(64);
        let win = hop * 2;
        let tol = hop / 2;
        // Periodic Hann: two copies offset by `hop` sum to exactly 1.
        let window = (0..win)
            .map(|n| {
                (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / win as f64).cos()) as f32
            })
            .collect();
        let region = win + 2 * tol;
        Self {
            hop,
            win,
            tol,
            window,
            ola_l: vec![0.0; win],
            ola_r: vec![0.0; win],
            fifo_l: vec![0.0; hop],
            fifo_r: vec![0.0; hop],
            fifo_len: 0,
            fifo_pos: 0,
            prev_start: None,
            expected_position: None,
            region_l: vec![0.0; region],
            region_r: vec![0.0; region],
            region_mono: vec![0.0; region],
            template: vec![0.0; hop],
        }
    }

    /// Grain hop in frames.
    pub fn hop(&self) -> usize {
        self.hop
    }

    /// Normalised cross-correlation of the template with the region at
    /// `offset`, using every `step`-th sample.
    #[inline]
    fn similarity(&self, offset: usize, step: usize) -> f32 {
        let mut dot = 0.0f32;
        let mut energy = 1e-9f32;
        let cand = match self.region_mono.get(offset..offset + self.hop) {
            Some(c) => c,
            None => return f32::MIN,
        };
        for (t, c) in self
            .template
            .iter()
            .step_by(step)
            .zip(cand.iter().step_by(step))
        {
            dot += t * c;
            energy += c * c;
        }
        dot / energy.sqrt()
    }

    /// Finds the best grain start near `nominal` and adds that grain to the
    /// overlap-add buffer. Returns the chosen start.
    fn add_grain(&mut self, source: &dyn FrameSource, nominal: i64) -> i64 {
        let tol = self.tol as i64;
        let region_start = nominal - tol;
        for (i, ((l, r), m)) in self
            .region_l
            .iter_mut()
            .zip(self.region_r.iter_mut())
            .zip(self.region_mono.iter_mut())
            .enumerate()
        {
            let [a, b] = source.frame(region_start + i as i64);
            *l = a;
            *r = b;
            *m = 0.5 * (a + b);
        }

        let mut best: usize = self.tol; // offset of `nominal` inside the region
        if let Some(prev) = self.prev_start {
            let natural = prev + self.hop as i64;
            for (i, t) in self.template.iter_mut().enumerate() {
                let [a, b] = source.frame(natural + i as i64);
                *t = 0.5 * (a + b);
            }
            // Coarse search on a 4-sample grid, then refine.
            let penalty = 1e-4 / self.tol.max(1) as f32;
            let score = |s: &Self, off: usize| {
                s.similarity(off, 4) - penalty * (off as i64 - tol).unsigned_abs() as f32
            };
            let mut best_score = score(self, best);
            let mut off = 0;
            while off <= 2 * self.tol {
                let sc = score(self, off);
                if sc > best_score {
                    best_score = sc;
                    best = off;
                }
                off += 4;
            }
            let lo = best.saturating_sub(3);
            let hi = (best + 3).min(2 * self.tol);
            let fine = |s: &Self, off: usize| {
                s.similarity(off, 1) - penalty * (off as i64 - tol).unsigned_abs() as f32
            };
            let mut best_fine = fine(self, best);
            for off in lo..=hi {
                let sc = fine(self, off);
                if sc > best_fine {
                    best_fine = sc;
                    best = off;
                }
            }
        }

        if let (Some(gl), Some(gr)) = (
            self.region_l.get(best..best + self.win),
            self.region_r.get(best..best + self.win),
        ) {
            for ((((ol, or), w), l), r) in self
                .ola_l
                .iter_mut()
                .zip(self.ola_r.iter_mut())
                .zip(self.window.iter())
                .zip(gl.iter())
                .zip(gr.iter())
            {
                *ol += w * l;
                *or += w * r;
            }
        }
        let start = region_start + best as i64;
        self.prev_start = Some(start);
        start
    }

    /// Moves the finished first `hop` samples of the overlap-add buffer to
    /// the FIFO (or drops them) and shifts the buffer.
    fn emit(&mut self, keep: bool) {
        let hop = self.hop;
        if keep {
            self.fifo_l.copy_from_slice(&self.ola_l[..hop]);
            self.fifo_r.copy_from_slice(&self.ola_r[..hop]);
            self.fifo_len = hop;
            self.fifo_pos = 0;
        }
        self.ola_l.copy_within(hop.., 0);
        self.ola_r.copy_within(hop.., 0);
        self.ola_l[self.win - hop..].fill(0.0);
        self.ola_r[self.win - hop..].fill(0.0);
    }

    fn prime(&mut self, source: &dyn FrameSource, position: f64, tempo: f64) {
        self.ola_l.fill(0.0);
        self.ola_r.fill(0.0);
        self.fifo_len = 0;
        self.fifo_pos = 0;
        self.prev_start = None;
        // A grain one hop before `position`, so the first real output hop
        // is fully overlapped instead of fading in from silence.
        let before = (position - tempo * self.hop as f64).round() as i64;
        self.add_grain(source, before);
        self.emit(false);
    }
}

impl TimeStretcher for Wsola {
    fn process(
        &mut self,
        source: &dyn FrameSource,
        position: f64,
        tempo: f64,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let tempo = tempo.clamp(0.25, 4.0);
        let n = out_l.len().min(out_r.len());
        let continuous = self
            .expected_position
            .is_some_and(|e| (e - position).abs() < 2.0);
        if !continuous {
            self.prime(source, position, tempo);
        }
        let mut written = 0;
        while written < n {
            if self.fifo_pos < self.fifo_len {
                let take = (self.fifo_len - self.fifo_pos).min(n - written);
                out_l[written..written + take]
                    .copy_from_slice(&self.fifo_l[self.fifo_pos..self.fifo_pos + take]);
                out_r[written..written + take]
                    .copy_from_slice(&self.fifo_r[self.fifo_pos..self.fifo_pos + take]);
                self.fifo_pos += take;
                written += take;
                continue;
            }
            let nominal = (position + tempo * written as f64).round() as i64;
            self.add_grain(source, nominal);
            self.emit(true);
        }
        self.expected_position = Some(position + tempo * n as f64);
    }

    fn reset(&mut self) {
        self.expected_position = None;
        self.prev_start = None;
        self.fifo_len = 0;
        self.fifo_pos = 0;
        self.ola_l.fill(0.0);
        self.ola_r.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sine {
        freqs: Vec<f64>,
        sr: f64,
    }

    impl FrameSource for Sine {
        fn frame(&self, index: i64) -> [f32; 2] {
            if index < 0 {
                return [0.0; 2];
            }
            let t = index as f64 / self.sr;
            let v: f64 = self
                .freqs
                .iter()
                .map(|f| (2.0 * std::f64::consts::PI * f * t).sin())
                .sum::<f64>()
                / self.freqs.len() as f64
                * 0.5;
            [v as f32, v as f32]
        }
    }

    fn run(src: &Sine, tempo: f64, frames: usize, block: usize) -> Vec<f32> {
        let mut w = Wsola::new(48_000);
        let mut pos = 10_000.0;
        let mut out = Vec::new();
        let mut l = vec![0.0; block];
        let mut r = vec![0.0; block];
        while out.len() < frames {
            w.process(src, pos, tempo, &mut l, &mut r);
            pos += tempo * block as f64;
            out.extend_from_slice(&l);
        }
        out
    }

    fn zero_crossing_freq(s: &[f32], sr: f64) -> f64 {
        let c = s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        c as f64 * sr / s.len() as f64
    }

    #[test]
    fn unity_tempo_reconstructs_input() {
        let src = Sine {
            freqs: vec![220.0, 1234.5],
            sr: 48_000.0,
        };
        let out = run(&src, 1.0, 48_000, 256);
        for (i, y) in out.iter().enumerate() {
            let x = src.frame(10_000 + i as i64)[0];
            assert!((x - y).abs() < 1e-4, "frame {i}: {x} vs {y}");
        }
    }

    #[test]
    fn keeps_pitch_when_tempo_changes() {
        let src = Sine {
            freqs: vec![440.0],
            sr: 48_000.0,
        };
        for tempo in [0.92, 1.08, 1.16, 0.84] {
            let out = run(&src, tempo, 96_000, 300);
            let f = zero_crossing_freq(&out[4800..], 48_000.0);
            assert!((f - 440.0).abs() < 4.0, "tempo {tempo}: {f} Hz");
        }
    }

    #[test]
    fn output_has_no_clicks_on_a_sine() {
        let src = Sine {
            freqs: vec![300.0],
            sr: 48_000.0,
        };
        let out = run(&src, 1.06, 48_000, 128);
        // A 300 Hz sine of amplitude 0.5 changes at most ~0.0197 per sample.
        let max_step = out[100..]
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.03, "max step {max_step}");
    }

    #[test]
    fn seek_resets_cleanly() {
        let src = Sine {
            freqs: vec![500.0],
            sr: 48_000.0,
        };
        let mut w = Wsola::new(48_000);
        let mut l = vec![0.0; 512];
        let mut r = vec![0.0; 512];
        w.process(&src, 0.0, 1.1, &mut l, &mut r);
        w.process(&src, 96_000.0, 1.1, &mut l, &mut r);
        assert!(l.iter().all(|v| v.is_finite() && v.abs() <= 0.6));
    }
}
