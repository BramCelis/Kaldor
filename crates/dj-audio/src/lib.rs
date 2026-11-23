//! Real-time audio engine for Kaldor.
//!
//! The [`Engine`] owns all audio state and runs inside the device callback
//! (or the [`offline`] renderer in tests). The UI talks to it only through an
//! [`EngineHandle`]: commands go in over a lock-free SPSC queue and state
//! snapshots come back over another one.
//!
//! Real-time rules for everything reachable from [`Engine::process`]: no
//! allocation, locks, I/O, logging or panics. `unwrap`, `expect` and `panic!`
//! are denied in this crate outside tests.

#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod command;
pub mod deck;
pub mod device;
pub mod engine;
pub mod fx;
pub mod mixer;
pub mod null_output;
pub mod offline;
pub mod perf;
pub mod record;
pub mod sampler;
pub mod snapshot;
pub mod tone;
#[cfg(windows)]
pub mod wasapi_exclusive;

pub use command::Command;
pub use deck::{DeckCommand, DeckSnapshot, JOG_SECONDS_PER_TURN, PitchRange, TrackMarkers};
pub use device::{AudioOutput, DeviceInfo, OutputInfo, OutputTiming};
pub use dj_decode::TrackAudio;
pub use dj_dsp::color_fx::ColorFx;
pub use dj_dsp::fx::EffectKind;
pub use engine::{ControllerQueue, Engine, EngineConfig, EngineHandle};
pub use fx::{
    BEATS, FX_SLOTS, FX_TARGETS, FX_UNITS, FxCommand, FxMode, FxParams, FxSnapshot, MASTER_TARGET,
    SlotParams, UnitParams, beats_label,
};
pub use mixer::{ChannelParams, EqBand, Meter, MixerCommand, MixerParams, XfaderSide};
pub use perf::{DeckGrid, HOT_CUES, LoopInfo, MAX_KEY_SHIFT, SyncMode};
pub use record::RecordReader;
pub use sampler::{
    RecordBuffer, SAMPLER_BANK, SAMPLER_SLOTS, SampleMode, SamplerCommand, SamplerSnapshot,
    SlotSnapshot,
};
pub use snapshot::EngineSnapshot;
pub use tone::{TestTone, ToneTarget};
