//! Types for the performance features: the beatgrid as the engine
//! sees it, loops, sync mode and the loop-aware frame source.

use dj_decode::TrackAudio;
use dj_dsp::FrameSource;

/// Hot cues (and saved loops, and pads) per deck.
pub const HOT_CUES: usize = 8;
/// Shortest loop, in frames.
pub const MIN_LOOP_FRAMES: f64 = 32.0;
/// Largest key shift, in semitones (each way).
pub const MAX_KEY_SHIFT: i8 = 12;

/// A constant-tempo beatgrid in frames at the engine rate (the UI converts
/// [`dj_core::Beatgrid`], which is in seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckGrid {
    /// Frame of beat 0.
    pub first_beat: f64,
    /// Beat length in track frames.
    pub beat_len: f64,
    /// Position of beat 0 in its bar, 1 to 4.
    pub first_beat_in_bar: u8,
}

impl DeckGrid {
    /// Converts a beatgrid in seconds for an engine running at
    /// `sample_rate`. `None` for a grid without a usable tempo.
    pub fn from_beatgrid(g: &dj_core::Beatgrid, sample_rate: u32) -> Option<Self> {
        let sr = f64::from(sample_rate);
        let beat_len = g.period() * sr;
        (beat_len.is_finite() && beat_len > 1.0).then_some(Self {
            first_beat: g.first_beat * sr,
            beat_len,
            first_beat_in_bar: g.first_beat_in_bar.clamp(1, 4),
        })
    }

    /// Tempo of the track (at normal speed) in BPM.
    pub fn bpm(&self, sample_rate: u32) -> f64 {
        60.0 * f64::from(sample_rate) / self.beat_len
    }

    /// Fractional beat index at frame `pos` (beat 0 is `first_beat`).
    pub fn beat_at(&self, pos: f64) -> f64 {
        (pos - self.first_beat) / self.beat_len
    }

    /// Frame of (fractional) beat `beat`.
    pub fn frame_of(&self, beat: f64) -> f64 {
        self.first_beat + beat * self.beat_len
    }

    /// Beats since a bar-aligned origin (a multiple of 4 is a downbeat).
    pub fn bar_beat(&self, pos: f64) -> f64 {
        self.beat_at(pos) + f64::from(self.first_beat_in_bar.clamp(1, 4) - 1)
    }

    /// Frame of a bar-aligned beat count (inverse of [`Self::bar_beat`]).
    pub fn frame_of_bar_beat(&self, b: f64) -> f64 {
        self.frame_of(b - f64::from(self.first_beat_in_bar.clamp(1, 4) - 1))
    }

    /// Position within the beat, 0 to 1.
    pub fn phase(&self, pos: f64) -> f64 {
        self.beat_at(pos).rem_euclid(1.0)
    }

    /// Nearest grid point of size `step` beats (1.0 = beats, 0.25 =
    /// sixteenths) to `pos`.
    pub fn round_to(&self, pos: f64, step: f64) -> f64 {
        let s = step.max(1.0 / 64.0);
        self.frame_of((self.beat_at(pos) / s).round() * s)
    }

    /// Grid point of size `step` beats at or before `pos` (a hair of
    /// tolerance, so a position just before a grid point counts as on it).
    pub fn floor_to(&self, pos: f64, step: f64) -> f64 {
        let s = step.max(1.0 / 64.0);
        self.frame_of((self.beat_at(pos) / s + 1e-6).floor() * s)
    }
}

/// Wraps `x` into [-0.5, 0.5).
pub fn wrap_half(x: f64) -> f64 {
    (x + 0.5).rem_euclid(1.0) - 0.5
}

/// A loop on a deck.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoopInfo {
    /// Start and end frames.
    pub start: f64,
    pub end: f64,
    pub active: bool,
    /// Length in beats for loops made from the grid (auto loops, halved or
    /// doubled ones); 0 for manual loops.
    pub beats: f32,
}

impl LoopInfo {
    pub fn len(&self) -> f64 {
        self.end - self.start
    }

    /// Integer frame bounds used for playback (start, length).
    pub(crate) fn frames(&self) -> (i64, i64) {
        let s = self.start.round() as i64;
        let len = ((self.end - self.start).round() as i64).max(MIN_LOOP_FRAMES as i64);
        (s, len)
    }
}

/// What SYNC matches.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SyncMode {
    /// Simple sync: tempo only.
    Tempo,
    /// Smart sync: tempo and beat phase.
    #[default]
    Phase,
}

/// The deck's track as the renderer reads it: virtual frame indices (which
/// keep counting through loop wraps, so the time-stretcher sees one
/// continuous stream) mapped to track frames.
pub(crate) struct DeckSource<'a> {
    pub track: &'a TrackAudio,
    /// Virtual index minus track index, before the loop.
    pub offset: i64,
    /// Active loop as (start, length) in track frames.
    pub lp: Option<(i64, i64)>,
    /// Frames at the end of the loop that crossfade into the audio just
    /// before the loop start, so the wrap is continuous (0 = none).
    pub xfade: i64,
}

impl DeckSource<'_> {
    /// Track frame of a virtual index (what [`FrameSource::frame`] reads,
    /// apart from the seam crossfade).
    #[cfg(test)]
    pub fn map(&self, index: i64) -> i64 {
        let x = index - self.offset;
        match self.lp {
            Some((s, len)) if len > 0 && x >= s + len => s + (x - s).rem_euclid(len),
            _ => x,
        }
    }
}

impl FrameSource for DeckSource<'_> {
    #[inline]
    fn frame(&self, index: i64) -> [f32; 2] {
        let x = index - self.offset;
        match self.lp {
            Some((s, len)) if len > 0 && x >= s => {
                let p = (x - s).rem_euclid(len);
                let a = FrameSource::frame(self.track, s + p);
                let xf = self.xfade.min(len / 4);
                if xf > 0 && p >= len - xf {
                    // Towards the end of the loop: blend into the audio that
                    // led up to the loop start, which flows on into it.
                    let w = (p - (len - xf) + 1) as f32 / (xf + 1) as f32;
                    let b = FrameSource::frame(self.track, s + p - len);
                    [a[0] + w * (b[0] - a[0]), a[1] + w * (b[1] - a[1])]
                } else {
                    a
                }
            }
            _ => FrameSource::frame(self.track, x),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_math() {
        let g = DeckGrid {
            first_beat: 1000.0,
            beat_len: 24_000.0,
            first_beat_in_bar: 3,
        };
        assert!((g.bpm(48_000) - 120.0).abs() < 1e-9);
        assert_eq!(g.beat_at(25_000.0), 1.0);
        assert_eq!(g.round_to(30_000.0, 1.0), 25_000.0);
        assert_eq!(g.round_to(40_000.0, 1.0), 49_000.0);
        assert_eq!(g.floor_to(48_000.0, 1.0), 25_000.0);
        assert_eq!(g.floor_to(48_999.99, 1.0), 49_000.0);
        assert_eq!(g.floor_to(30_000.0, 0.25), 25_000.0);
        // Beat 0 is beat 3 of its bar: the next downbeat is beat 2.
        assert_eq!(g.bar_beat(g.frame_of(2.0)), 4.0);
        assert_eq!(g.frame_of_bar_beat(4.0), g.frame_of(2.0));
        assert!((g.phase(13_000.0) - 0.5).abs() < 1e-12);
        assert_eq!(wrap_half(0.75), -0.25);
        assert_eq!(wrap_half(-0.75), 0.25);
    }

    #[test]
    fn source_maps_through_the_loop() {
        let t = TrackAudio::from_interleaved(48_000, &[0.0; 2000]);
        let s = DeckSource {
            track: &t,
            offset: 10,
            lp: Some((100, 50)),
            xfade: 0,
        };
        assert_eq!(s.map(10), 0);
        assert_eq!(s.map(159), 149);
        assert_eq!(s.map(160), 100);
        assert_eq!(s.map(215), 105);
        let s = DeckSource { lp: None, ..s };
        assert_eq!(s.map(215), 205);
    }
}
