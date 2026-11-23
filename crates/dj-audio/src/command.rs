//! Commands sent from the UI (and later MIDI) thread to the audio thread.

use crate::deck::DeckCommand;
use crate::fx::FxCommand;
use crate::mixer::MixerCommand;
use crate::perf::SyncMode;
use crate::sampler::SamplerCommand;
use crate::tone::TestTone;
use dj_core::{DeckId, OutputRouting};

/// A command for the audio thread. Commands are applied at the start of the
/// next callback, in the order they were sent.
#[derive(Debug)]
pub enum Command {
    /// Start, change or stop (`None`) the test tone.
    SetTestTone(Option<TestTone>),
    /// Change where master, cue and booth go on the device.
    SetRouting(OutputRouting),
    /// A command for one deck.
    Deck(DeckId, DeckCommand),
    /// A mixer setting.
    Mixer(MixerCommand),
    /// SYNC on a deck. The first deck to sync with no master around becomes
    /// the master; the others follow it.
    Sync(DeckId, bool),
    /// SYNC on or off, whatever it is now (controller buttons).
    ToggleSync(DeckId),
    /// Makes a deck the sync master.
    SyncMaster(DeckId),
    /// Simple (tempo) or smart (tempo and phase) sync.
    SetSyncMode(SyncMode),
    /// When the sync master stops, a playing synced deck becomes the master
    /// (`true`); otherwise the master stays until it is changed.
    SetAutoSyncMaster(bool),
    /// Quantize cues, loops and jumps to the beatgrid (all decks).
    Quantize(bool),
    /// A sampler command.
    Sampler(SamplerCommand),
    /// An FX unit command.
    Fx(FxCommand),
    /// Master recording on or off (the writer thread must be ready; see
    /// [`crate::EngineHandle::take_record_reader`]).
    Record(bool),
}
