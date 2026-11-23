//! State snapshots published by the audio thread for the UI.

use crate::deck::DeckSnapshot;
use crate::fx::FxSnapshot;
use crate::mixer::{Meter, MixerParams};
use crate::perf::SyncMode;
use crate::sampler::SamplerSnapshot;
use crate::tone::ToneTarget;
use dj_core::{NUM_DECKS, OutputRouting};

/// Engine state as seen at the end of one audio callback. Plain `Copy` data,
/// so publishing it never allocates.
#[derive(Clone, Copy, Debug, Default)]
pub struct EngineSnapshot {
    pub sample_rate: u32,
    /// Total frames rendered since the engine started. This is the audio
    /// clock that drives waveform scrolling.
    pub frames_rendered: u64,
    /// Frames in the most recent callback.
    pub last_block_frames: u32,
    /// Time spent in the last callback as a fraction of its period.
    pub callback_load: f32,
    /// Highest callback load seen recently (slow decay).
    pub callback_load_peak: f32,
    /// Peak absolute sample on the master output since the last snapshot
    /// (after limiter and test tone).
    pub master_peak: [f32; 2],
    /// Peak absolute sample on the headphone output since the last snapshot.
    pub cue_peak: [f32; 2],
    pub test_tone: Option<ToneTarget>,
    pub routing: OutputRouting,
    pub decks: [DeckSnapshot; NUM_DECKS],
    /// Current mixer settings.
    pub mixer: MixerParams,
    /// Post-fader channel meters.
    pub channel_meters: [Meter; NUM_DECKS],
    pub master_meter: Meter,
    pub cue_meter: Meter,
    /// Limiter gain (1.0 = not limiting).
    pub limiter_gain: f32,
    /// Tracks that could not be handed back for freeing because the retire
    /// queue was full (they were leaked instead of freed on the audio
    /// thread). Should always be 0.
    pub leaked_tracks: u64,
    /// Sync master deck (index), if any.
    pub sync_master: Option<usize>,
    pub sync_mode: SyncMode,
    pub quantize: bool,
    /// Tempo synced decks and samples follow (BPM), if there is a master.
    pub master_bpm: Option<f64>,
    pub sampler: SamplerSnapshot,
    pub fx: FxSnapshot,
    /// Master recording is on.
    pub recording: bool,
    /// Frames recorded since the recording started.
    pub record_frames: u64,
    /// Frames lost because the writer did not keep up.
    pub record_dropped: u64,
    /// Samples cut at full scale on the way out since the engine started
    /// (master, headphones, booth): each is a hard clip, heard as a click.
    /// With the limiters on they stay 0.
    pub clipped: [u64; 3],
}
