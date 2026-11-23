//! A decoded track in memory, readable while it is still being written.
//!
//! Audio is stored in fixed-size chunks of interleaved stereo `f32`. The
//! writer publishes a chunk by setting its `OnceLock` and then raising
//! `ready_frames` with release ordering; readers load `ready_frames` with
//! acquire ordering and only touch chunks below it. Reads are lock-free and
//! allocation-free, and no `unsafe` is needed.

use dj_dsp::FrameSource;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

/// Frames per chunk (about 0.68 s at 48 kHz).
pub const CHUNK_FRAMES: usize = 1 << 15;
const CHUNK_SHIFT: u32 = 15;
const CHUNK_MASK: usize = CHUNK_FRAMES - 1;

/// Longest track kept in memory, in seconds. Longer files are truncated
/// (streaming from disk is not implemented yet).
pub const MAX_SECONDS: u64 = 3 * 60 * 60;

/// Loading state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum TrackState {
    Loading = 0,
    Complete = 1,
    Failed = 2,
    Cancelled = 3,
}

/// A decoded stereo track at a fixed sample rate.
pub struct TrackAudio {
    sample_rate: u32,
    chunks: Box<[OnceLock<Box<[f32]>>]>,
    ready_frames: AtomicU64,
    estimated_frames: u64,
    state: AtomicU8,
}

impl std::fmt::Debug for TrackAudio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackAudio")
            .field("sample_rate", &self.sample_rate)
            .field("ready_frames", &self.ready_frames())
            .field("state", &self.state())
            .finish()
    }
}

impl TrackAudio {
    /// Creates an empty track and the writer that fills it.
    /// `estimated_frames` sizes the chunk table; `None` reserves room for
    /// [`MAX_SECONDS`].
    pub fn new(sample_rate: u32, estimated_frames: Option<u64>) -> (Arc<TrackAudio>, TrackWriter) {
        let max_frames = MAX_SECONDS * u64::from(sample_rate);
        // Leave 10 % headroom over the estimate: container durations are
        // not always exact.
        let cap = estimated_frames
            .map(|e| (e + e / 10 + CHUNK_FRAMES as u64).min(max_frames))
            .unwrap_or(max_frames);
        let slots = (cap as usize).div_ceil(CHUNK_FRAMES).max(1);
        let chunks = (0..slots).map(|_| OnceLock::new()).collect();
        let audio = Arc::new(TrackAudio {
            sample_rate,
            chunks,
            ready_frames: AtomicU64::new(0),
            estimated_frames: estimated_frames.unwrap_or(0),
            state: AtomicU8::new(TrackState::Loading as u8),
        });
        let writer = TrackWriter {
            audio: Arc::clone(&audio),
            buf: Vec::with_capacity(CHUNK_FRAMES * 2),
            next_chunk: 0,
            frames: 0,
            truncated: false,
        };
        (audio, writer)
    }

    /// Builds a complete track from interleaved stereo samples (tests,
    /// generated audio).
    pub fn from_interleaved(sample_rate: u32, samples: &[f32]) -> Arc<TrackAudio> {
        let frames = (samples.len() / 2) as u64;
        let (audio, mut w) = TrackAudio::new(sample_rate, Some(frames));
        w.push_interleaved(samples);
        w.finish();
        audio
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frames that can be read now (contiguous from frame 0).
    #[inline]
    pub fn ready_frames(&self) -> u64 {
        self.ready_frames.load(Ordering::Acquire)
    }

    pub fn state(&self) -> TrackState {
        match self.state.load(Ordering::Acquire) {
            1 => TrackState::Complete,
            2 => TrackState::Failed,
            3 => TrackState::Cancelled,
            _ => TrackState::Loading,
        }
    }

    pub fn is_complete(&self) -> bool {
        self.state() == TrackState::Complete
    }

    /// Track length in frames: exact once complete, the decoder's estimate
    /// (or what is ready, if larger) while loading.
    pub fn len_frames(&self) -> u64 {
        let ready = self.ready_frames();
        if self.state() == TrackState::Loading {
            ready.max(self.estimated_frames)
        } else {
            ready
        }
    }

    /// Duration in seconds (see [`Self::len_frames`]).
    pub fn duration_seconds(&self) -> f64 {
        self.len_frames() as f64 / f64::from(self.sample_rate.max(1))
    }

    /// One stereo frame, or `None` if it is not decoded (yet).
    #[inline]
    pub fn frame(&self, index: u64) -> Option<[f32; 2]> {
        if index >= self.ready_frames() {
            return None;
        }
        let i = index as usize;
        let chunk = self.chunks.get(i >> CHUNK_SHIFT)?.get()?;
        let off = (i & CHUNK_MASK) * 2;
        match chunk.get(off..off + 2) {
            Some(&[l, r]) => Some([l, r]),
            _ => None,
        }
    }

    /// Copies frames starting at `start` into `out` (interleaved stereo);
    /// frames that are not ready are written as silence. Returns how many
    /// real frames were copied.
    pub fn read_interleaved(&self, start: u64, out: &mut [f32]) -> usize {
        let ready = self.ready_frames();
        let mut copied = 0;
        for (k, frame) in out.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let idx = start + k as u64;
            if idx < ready
                && let Some([l, r]) = self.frame(idx)
            {
                frame[0] = l;
                frame[1] = r;
                copied += 1;
                continue;
            }
            frame[0] = 0.0;
            frame[1] = 0.0;
        }
        copied
    }
}

impl FrameSource for TrackAudio {
    #[inline]
    fn frame(&self, index: i64) -> [f32; 2] {
        if index < 0 {
            return [0.0; 2];
        }
        TrackAudio::frame(self, index as u64).unwrap_or([0.0; 2])
    }
}

/// Fills a [`TrackAudio`]. Owned by the decoder thread.
pub struct TrackWriter {
    audio: Arc<TrackAudio>,
    buf: Vec<f32>,
    next_chunk: usize,
    frames: u64,
    truncated: bool,
}

impl TrackWriter {
    /// Appends interleaved stereo samples.
    pub fn push_interleaved(&mut self, mut samples: &[f32]) {
        while !samples.is_empty() && !self.truncated {
            let room = CHUNK_FRAMES * 2 - self.buf.len();
            let take = room.min(samples.len() & !1);
            if take == 0 {
                break;
            }
            self.buf.extend_from_slice(&samples[..take]);
            samples = &samples[take..];
            if self.buf.len() == CHUNK_FRAMES * 2 {
                self.publish_chunk();
            }
        }
    }

    fn publish_chunk(&mut self) {
        if self.buf.is_empty() {
            return;
        }
        let Some(slot) = self.audio.chunks.get(self.next_chunk) else {
            self.truncated = true;
            self.buf.clear();
            return;
        };
        let frames = (self.buf.len() / 2) as u64;
        let data = std::mem::replace(&mut self.buf, Vec::with_capacity(CHUNK_FRAMES * 2));
        let _ = slot.set(data.into_boxed_slice());
        self.next_chunk += 1;
        self.frames += frames;
        self.audio
            .ready_frames
            .store(self.frames, Ordering::Release);
    }

    /// Frames written so far.
    pub fn frames_written(&self) -> u64 {
        self.frames + (self.buf.len() / 2) as u64
    }

    /// True if the track was longer than the chunk table and got cut.
    pub fn was_truncated(&self) -> bool {
        self.truncated
    }

    /// Publishes the last partial chunk and marks the track complete.
    pub fn finish(mut self) {
        self.publish_chunk();
        self.audio
            .state
            .store(TrackState::Complete as u8, Ordering::Release);
    }

    /// Marks the track failed or cancelled; what was decoded stays playable.
    pub fn abort(mut self, state: TrackState) {
        self.publish_chunk();
        self.audio.state.store(state as u8, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progressive_reads() {
        let (audio, mut w) = TrackAudio::new(48_000, Some(100_000));
        assert_eq!(audio.ready_frames(), 0);
        assert!(audio.frame(0).is_none());
        let samples: Vec<f32> = (0..(CHUNK_FRAMES + 10) * 2).map(|i| i as f32).collect();
        w.push_interleaved(&samples);
        // One full chunk is published; the tail is still buffered.
        assert_eq!(audio.ready_frames(), CHUNK_FRAMES as u64);
        assert_eq!(audio.frame(1), Some([2.0, 3.0]));
        assert!(audio.frame(CHUNK_FRAMES as u64).is_none());
        assert_eq!(audio.state(), TrackState::Loading);
        assert_eq!(audio.len_frames(), 100_000);
        w.finish();
        assert_eq!(audio.ready_frames(), CHUNK_FRAMES as u64 + 10);
        let last = CHUNK_FRAMES as u64 + 9;
        assert_eq!(
            audio.frame(last),
            Some([(last * 2) as f32, (last * 2 + 1) as f32])
        );
        assert!(audio.is_complete());
        assert_eq!(audio.len_frames(), CHUNK_FRAMES as u64 + 10);
    }

    #[test]
    fn truncates_beyond_capacity() {
        // Estimate of 10 frames gets one chunk slot (plus headroom rounding).
        let (audio, mut w) = TrackAudio::new(48_000, Some(10));
        let samples = vec![0.5f32; CHUNK_FRAMES * 2 * 3];
        w.push_interleaved(&samples);
        assert!(w.was_truncated());
        w.finish();
        assert!(audio.ready_frames() <= 2 * CHUNK_FRAMES as u64);
    }

    #[test]
    fn read_interleaved_pads_with_silence() {
        let audio = TrackAudio::from_interleaved(48_000, &[1.0, 2.0, 3.0, 4.0]);
        let mut out = [9.0f32; 8];
        assert_eq!(audio.read_interleaved(1, &mut out), 1);
        assert_eq!(out, [3.0, 4.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]);
        assert_eq!(FrameSource::frame(&*audio, -1), [0.0, 0.0]);
    }
}
