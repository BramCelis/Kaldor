//! Key lock with Signalsmith Stretch (MIT, vendored C++ in `third_party/`).
//!
//! The first choice for key lock. It is a spectral (STFT) stretcher:
//! much less warble than [`crate::Wsola`] on full mixes, at a higher CPU cost.
//! It runs with "split computation" so its spectral work is spread evenly
//! over the audio callbacks instead of arriving in bursts.
//!
//! Alignment: the library delays its output (input latency + output
//! latency). Decks hold the whole track, so the stretcher reads ahead by
//! exactly that much: the output frame written for `position` is the source
//! frame at `position`, so the audio stays in line with the waveform, the
//! beatgrid and the cue points.
//!
//! Real-time: all memory is allocated in [`SignalsmithStretch::new`]. The C++
//! side sizes its working buffers when it is configured; `reset`, `seek` and
//! `process` reuse them.

use crate::stretch::{FrameSource, TimeStretcher};
use std::ffi::c_void;
use std::ptr::NonNull;

// Hand-written declarations for cpp/signalsmith_wrapper.cpp (no bindgen).
unsafe extern "C" {
    fn djs_create(channels: i32, sample_rate: f32, split_computation: i32) -> *mut c_void;
    fn djs_destroy(handle: *mut c_void);
    fn djs_reset(handle: *mut c_void);
    fn djs_set_transpose(handle: *mut c_void, semitones: f64);
    fn djs_input_latency(handle: *const c_void) -> i32;
    fn djs_output_latency(handle: *const c_void) -> i32;
    fn djs_block_samples(handle: *const c_void) -> i32;
    fn djs_interval_samples(handle: *const c_void) -> i32;
    fn djs_seek(handle: *mut c_void, input: *const f32, frames: i32, channels: i32, rate: f64);
    fn djs_process(
        handle: *mut c_void,
        input: *const f32,
        input_frames: i32,
        output: *mut f32,
        output_frames: i32,
        channels: i32,
    );
}

const CHANNELS: usize = 2;
/// Output frames handed to the library per call.
const CHUNK: usize = 1024;
/// Highest tempo the input buffer is sized for (the deck allows 1.5 plus bend).
const MAX_TEMPO: f64 = 4.0;

/// Signalsmith Stretch behind the [`TimeStretcher`] interface.
pub struct SignalsmithStretch {
    handle: NonNull<c_void>,
    input_latency: f64,
    output_latency: f64,
    /// Frames fed by `seek` after a jump: one block plus one interval.
    preroll: usize,
    /// Interleaved input scratch, sized for the pre-roll or a chunk at MAX_TEMPO.
    input: Vec<f32>,
    /// Interleaved output scratch for one chunk.
    output: Vec<f32>,
    /// Source frame of the next input frame to feed.
    read_index: i64,
    expected_position: Option<f64>,
    /// Current pitch shift in semitones.
    transpose: f64,
}

// SAFETY: the C++ object is owned exclusively by this value, has no thread
// affinity and is only reached through `&mut self` (or `&self` for the
// read-only latency queries in `new`).
unsafe impl Send for SignalsmithStretch {}

impl SignalsmithStretch {
    /// Creates a stereo stretcher for `sample_rate` with the library's default
    /// preset (120 ms blocks, 30 ms interval) and split computation.
    ///
    /// # Panics
    /// If the C++ side cannot allocate its buffers (out of memory).
    pub fn new(sample_rate: u32) -> Self {
        // SAFETY: plain C call; returns an owned object or null.
        let raw = unsafe { djs_create(CHANNELS as i32, sample_rate.max(1) as f32, 1) };
        let handle = NonNull::new(raw).expect("Signalsmith Stretch: out of memory");
        let ptr = handle.as_ptr().cast_const();
        // SAFETY: `ptr` is a live object created above.
        let (input_latency, output_latency, block, interval) = unsafe {
            (
                djs_input_latency(ptr),
                djs_output_latency(ptr),
                djs_block_samples(ptr),
                djs_interval_samples(ptr),
            )
        };
        let preroll = usize::try_from(block + interval).unwrap_or(0).max(1);
        let input_frames = preroll.max((CHUNK as f64 * MAX_TEMPO) as usize + 2);
        Self {
            handle,
            input_latency: f64::from(input_latency.max(0)),
            output_latency: f64::from(output_latency.max(0)),
            preroll,
            input: vec![0.0; input_frames * CHANNELS],
            output: vec![0.0; CHUNK * CHANNELS],
            read_index: 0,
            expected_position: None,
            transpose: 0.0,
        }
    }

    /// Total delay the stretcher compensates for, in source frames at `tempo`.
    pub fn lookahead(&self, tempo: f64) -> f64 {
        self.input_latency + self.output_latency * tempo
    }

    /// Source frame the input must have reached so that the output frame
    /// after `frames_out` frames lines up with `position_after`.
    fn input_target(&self, position_after: f64, tempo: f64) -> i64 {
        (position_after + self.lookahead(tempo)).round() as i64
    }

    /// Copies `frames` source frames from `start` into the input scratch.
    fn fill_input(&mut self, source: &dyn FrameSource, start: i64, frames: usize) {
        for (i, f) in self
            .input
            .as_chunks_mut::<CHANNELS>()
            .0
            .iter_mut()
            .take(frames)
            .enumerate()
        {
            *f = source.frame(start + i as i64);
        }
    }

    /// Restarts after a jump: clears history and pre-rolls so the next output
    /// frame is the source frame at `position`.
    fn restart(&mut self, source: &dyn FrameSource, position: f64, tempo: f64) {
        let end = self.input_target(position, tempo);
        let start = end - self.preroll as i64;
        self.fill_input(source, start, self.preroll);
        // SAFETY: the handle is live; `input` holds at least `preroll`
        // interleaved stereo frames (checked in `new`).
        unsafe {
            djs_reset(self.handle.as_ptr());
            djs_seek(
                self.handle.as_ptr(),
                self.input.as_ptr(),
                self.preroll as i32,
                CHANNELS as i32,
                tempo,
            );
        }
        self.read_index = end;
    }
}

impl Drop for SignalsmithStretch {
    fn drop(&mut self) {
        // SAFETY: created by djs_create, destroyed exactly once.
        unsafe { djs_destroy(self.handle.as_ptr()) }
    }
}

impl TimeStretcher for SignalsmithStretch {
    fn process(
        &mut self,
        source: &dyn FrameSource,
        position: f64,
        tempo: f64,
        out_l: &mut [f32],
        out_r: &mut [f32],
    ) {
        let n = out_l.len().min(out_r.len());
        let tempo = tempo.clamp(0.05, MAX_TEMPO);
        let continuous = self
            .expected_position
            .is_some_and(|e| (e - position).abs() < 2.0);
        if !continuous {
            self.restart(source, position, tempo);
        }
        let input_capacity = self.input.len() / CHANNELS;
        let mut done = 0;
        let mut pos = position;
        while done < n {
            let m = (n - done).min(CHUNK);
            let after = pos + tempo * m as f64;
            // Feed exactly up to where the output after this chunk needs the
            // input to be. This follows tempo changes without drift.
            let wanted = (self.input_target(after, tempo) - self.read_index).max(0);
            let k = usize::try_from(wanted).unwrap_or(0).min(input_capacity);
            self.fill_input(source, self.read_index, k);
            self.read_index += k as i64;
            // SAFETY: the handle is live; `input` holds `k` frames and
            // `output` holds `m <= CHUNK` frames, both interleaved stereo.
            unsafe {
                djs_process(
                    self.handle.as_ptr(),
                    self.input.as_ptr(),
                    k as i32,
                    self.output.as_mut_ptr(),
                    m as i32,
                    CHANNELS as i32,
                );
            }
            let frames = self.output.as_chunks::<CHANNELS>().0.iter().take(m);
            let dst = out_l[done..done + m]
                .iter_mut()
                .zip(out_r[done..done + m].iter_mut());
            for (&[fl, fr], (l, r)) in frames.zip(dst) {
                *l = fl;
                *r = fr;
            }
            done += m;
            pos = after;
        }
        self.expected_position = Some(position + tempo * n as f64);
    }

    fn set_transpose_semitones(&mut self, semitones: f64) {
        if semitones != self.transpose {
            self.transpose = semitones;
            // SAFETY: the handle is live; this only sets two members.
            unsafe { djs_set_transpose(self.handle.as_ptr(), semitones) }
        }
    }

    fn reset(&mut self) {
        self.expected_position = None;
        // SAFETY: the handle is live.
        unsafe { djs_reset(self.handle.as_ptr()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    struct Sine(f64);

    impl FrameSource for Sine {
        fn frame(&self, index: i64) -> [f32; 2] {
            let v = (2.0 * std::f64::consts::PI * self.0 * index as f64 / f64::from(SR)).sin();
            [0.5 * v as f32; 2]
        }
    }

    /// A short click every `period` frames, silence elsewhere.
    struct Clicks {
        period: i64,
    }

    impl FrameSource for Clicks {
        fn frame(&self, index: i64) -> [f32; 2] {
            let v = if index >= 0 && index % self.period < 4 {
                0.9
            } else {
                0.0
            };
            [v; 2]
        }
    }

    /// Renders `frames` frames from `start` in blocks of `block`, advancing the
    /// position like the deck does.
    fn render(
        src: &dyn FrameSource,
        start: f64,
        tempo: f64,
        frames: usize,
        block: usize,
    ) -> Vec<f32> {
        let mut s = SignalsmithStretch::new(SR);
        let mut pos = start;
        let mut out = Vec::with_capacity(frames);
        let (mut l, mut r) = (vec![0.0; block], vec![0.0; block]);
        while out.len() < frames {
            s.process(src, pos, tempo, &mut l, &mut r);
            out.extend_from_slice(&l);
            pos += tempo * block as f64;
        }
        out.truncate(frames);
        out
    }

    fn zero_crossing_freq(x: &[f32]) -> f64 {
        let crossings = x.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        crossings as f64 * f64::from(SR) / x.len() as f64
    }

    #[test]
    fn keeps_pitch_when_tempo_changes() {
        for tempo in [0.92, 1.0, 1.08, 1.5] {
            let out = render(&Sine(440.0), 10_000.0, tempo, 96_000, 256);
            // Skip the first block of the stretcher's settling.
            let f = zero_crossing_freq(&out[12_000..]);
            assert!((f - 440.0).abs() < 4.0, "tempo {tempo}: {f} Hz");
            assert!(out.iter().all(|v| v.is_finite()));
        }
    }

    #[test]
    fn transposes_by_semitones() {
        for (semis, want) in [
            (12.0, 880.0),
            (-12.0, 220.0),
            (3.0, 440.0 * 2f64.powf(0.25)),
        ] {
            let mut s = SignalsmithStretch::new(SR);
            s.set_transpose_semitones(semis);
            let (mut l, mut r) = (vec![0.0; 256], vec![0.0; 256]);
            let mut out = Vec::new();
            let mut pos = 10_000.0;
            while out.len() < 96_000 {
                s.process(&Sine(440.0), pos, 1.0, &mut l, &mut r);
                out.extend_from_slice(&l);
                pos += 256.0;
            }
            let f = zero_crossing_freq(&out[12_000..]);
            assert!(
                (f - want).abs() < want * 0.01,
                "{semis} st: {f} Hz, want {want}"
            );
        }
    }

    #[test]
    fn level_is_preserved() {
        let out = render(&Sine(440.0), 10_000.0, 0.92, 96_000, 256);
        let rms =
            (out[12_000..].iter().map(|v| v * v).sum::<f32>() / (out.len() - 12_000) as f32).sqrt();
        // Input RMS is 0.5 / sqrt(2) = 0.354.
        assert!((rms - 0.354).abs() < 0.05, "rms {rms}");
    }

    /// Each click comes out where the deck position says it is: the
    /// look-ahead cancels the library's latency, also after long playback.
    #[test]
    fn output_is_aligned_with_the_position() {
        let period = 24_000;
        for tempo in [1.0, 0.92, 1.08] {
            let start = 50_000.0;
            let out = render(&Clicks { period }, start, tempo, 10 * SR as usize, 256);
            // Clicks at source frames k * period, from the 3rd one on.
            for k in [3, 10, 19] {
                let src = (k * period) as f64;
                let expected = ((src - start) / tempo).round() as usize;
                if expected + 2_000 > out.len() {
                    continue;
                }
                let lo = expected - 1_500;
                let found = (lo..expected + 1_500)
                    .max_by(|&a, &b| out[a].abs().total_cmp(&out[b].abs()))
                    .unwrap_or(lo);
                let err_ms = (found as f64 - expected as f64) * 1000.0 / f64::from(SR);
                assert!(
                    err_ms.abs() < 1.5,
                    "tempo {tempo}, click {k}: off by {err_ms:.2} ms"
                );
            }
        }
    }

    #[test]
    fn a_jump_restarts_cleanly() {
        let mut s = SignalsmithStretch::new(SR);
        let (mut l, mut r) = (vec![0.0; 512], vec![0.0; 512]);
        let src = Sine(440.0);
        s.process(&src, 0.0, 1.0, &mut l, &mut r);
        for pos in [500_000.0, 1_000.0, 2_000_000.0] {
            for i in 0..20 {
                s.process(&src, pos + f64::from(i) * 512.0, 1.0, &mut l, &mut r);
            }
            assert!(l.iter().all(|v| v.is_finite()));
            assert!(
                l.iter().any(|v| v.abs() > 0.1),
                "silent after a jump to {pos}"
            );
        }
        s.reset();
        s.process(&src, 7.0, 0.9, &mut l, &mut r);
        assert!(l.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn large_blocks_are_split() {
        let out = render(&Sine(440.0), 0.0, 1.2, 48_000, 4_096);
        let f = zero_crossing_freq(&out[12_000..]);
        assert!((f - 440.0).abs() < 4.0, "{f} Hz");
    }
}
