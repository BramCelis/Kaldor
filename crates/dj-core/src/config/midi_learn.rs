//! MIDI learn: controller messages given a function by the user.

use serde::{Deserialize, Serialize};

/// A controller message given a function with MIDI learn.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct LearnedBinding {
    /// A note (true) or a controller (CC).
    pub note: bool,
    /// MIDI channel 1-16.
    pub channel: u8,
    /// Note or controller number 0-127.
    pub number: u8,
    pub function: MidiFunction,
    /// The deck (1-4) for deck functions.
    #[serde(default)]
    pub deck: Option<u8>,
}

/// What a learned control does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MidiFunction {
    // Buttons on a deck.
    Play,
    Cue,
    Sync,
    SyncMaster,
    KeyLock,
    Slip,
    HeadphoneCue,
    BendDown,
    BendUp,
    /// Pad 1-8 in the deck's pad mode.
    Pad(u8),
    /// The pad's second function (delete a cue or saved loop, stop a sample).
    PadSecondary(u8),
    /// Pad mode 1-8 (cues, saved loops, auto loop, roll, sampler, slicer,
    /// pitch play, stems).
    PadMode(u8),
    LoopIn,
    LoopOut,
    Reloop,
    AutoLoop,
    LoopHalve,
    LoopDouble,
    LoopMoveBack,
    LoopMoveForward,
    BeatJumpBack,
    BeatJumpForward,
    KeyShiftDown,
    KeyShiftUp,
    KeyShiftReset,
    GridSetBeatOne,
    GridTap,
    LoadSelected,
    // Buttons for everything.
    /// Sampler slot 1-32 (in its play mode).
    SamplerSlot(u8),
    SamplerBankPrev,
    SamplerBankNext,
    Quantize,
    // Knobs and faders on a deck / mixer channel.
    Volume,
    Trim,
    EqHigh,
    EqMid,
    EqLow,
    Filter,
    Pitch,
    // Knobs and faders for everything.
    Crossfader,
    CueGain,
    CueMix,
    MasterGain,
    BoothGain,
    SamplerVolume,
    // Encoders.
    /// Moves the library selection (relative encoder).
    Browse,
}

/// How a function is played: a button, a knob or fader, or an encoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FunctionKind {
    Button,
    Continuous,
    Encoder,
}

impl MidiFunction {
    /// Every function, for the learn list (pads, pad modes and sampler
    /// slots with their numbers).
    pub fn all() -> Vec<MidiFunction> {
        use MidiFunction::*;
        let mut v = vec![
            Play,
            Cue,
            Sync,
            SyncMaster,
            KeyLock,
            Slip,
            HeadphoneCue,
            BendDown,
            BendUp,
        ];
        v.extend((1..=8).map(Pad));
        v.extend((1..=8).map(PadSecondary));
        v.extend((1..=8).map(PadMode));
        v.extend([
            LoopIn,
            LoopOut,
            Reloop,
            AutoLoop,
            LoopHalve,
            LoopDouble,
            LoopMoveBack,
            LoopMoveForward,
            BeatJumpBack,
            BeatJumpForward,
            KeyShiftDown,
            KeyShiftUp,
            KeyShiftReset,
            GridSetBeatOne,
            GridTap,
            LoadSelected,
        ]);
        v.extend((1..=32).map(SamplerSlot));
        v.extend([
            SamplerBankPrev,
            SamplerBankNext,
            Quantize,
            Volume,
            Trim,
            EqHigh,
            EqMid,
            EqLow,
            Filter,
            Pitch,
            Crossfader,
            CueGain,
            CueMix,
            MasterGain,
            BoothGain,
            SamplerVolume,
            Browse,
        ]);
        v
    }

    pub fn kind(self) -> FunctionKind {
        use MidiFunction::*;
        match self {
            Volume | Trim | EqHigh | EqMid | EqLow | Filter | Pitch | Crossfader | CueGain
            | CueMix | MasterGain | BoothGain | SamplerVolume => FunctionKind::Continuous,
            Browse => FunctionKind::Encoder,
            _ => FunctionKind::Button,
        }
    }

    /// Does the function work on one deck (or mixer channel)?
    pub fn needs_deck(self) -> bool {
        use MidiFunction::*;
        !matches!(
            self,
            SamplerSlot(_)
                | SamplerBankPrev
                | SamplerBankNext
                | Quantize
                | Crossfader
                | CueGain
                | CueMix
                | MasterGain
                | BoothGain
                | SamplerVolume
                | Browse
        )
    }

    /// A name for the learn list.
    pub fn label(self) -> String {
        use MidiFunction::*;
        const MODES: [&str; 8] = [
            "cues",
            "saved loops",
            "auto loop",
            "roll",
            "sampler",
            "slicer",
            "pitch play",
            "stems",
        ];
        match self {
            Play => "Play / pause".into(),
            Cue => "Cue".into(),
            Sync => "Sync".into(),
            SyncMaster => "Sync master".into(),
            KeyLock => "Key lock".into(),
            Slip => "Slip".into(),
            HeadphoneCue => "Headphone cue".into(),
            BendDown => "Pitch bend -".into(),
            BendUp => "Pitch bend +".into(),
            Pad(n) => format!("Pad {n}"),
            PadSecondary(n) => format!("Pad {n}, second function"),
            PadMode(n) => format!(
                "Pad mode: {}",
                MODES.get(usize::from(n.saturating_sub(1))).unwrap_or(&"?")
            ),
            LoopIn => "Loop in".into(),
            LoopOut => "Loop out".into(),
            Reloop => "Reloop / exit".into(),
            AutoLoop => "Auto loop".into(),
            LoopHalve => "Loop 1/2".into(),
            LoopDouble => "Loop x2".into(),
            LoopMoveBack => "Move loop back".into(),
            LoopMoveForward => "Move loop forward".into(),
            BeatJumpBack => "Beat jump back".into(),
            BeatJumpForward => "Beat jump forward".into(),
            KeyShiftDown => "Key shift -".into(),
            KeyShiftUp => "Key shift +".into(),
            KeyShiftReset => "Key shift reset".into(),
            GridSetBeatOne => "Grid: beat one".into(),
            GridTap => "Grid: tap tempo".into(),
            LoadSelected => "Load the selected track".into(),
            SamplerSlot(n) => format!("Sampler slot {n}"),
            SamplerBankPrev => "Sampler bank -".into(),
            SamplerBankNext => "Sampler bank +".into(),
            Quantize => "Quantize".into(),
            Volume => "Channel volume (fader)".into(),
            Trim => "Trim (level)".into(),
            EqHigh => "EQ high".into(),
            EqMid => "EQ mid".into(),
            EqLow => "EQ low".into(),
            Filter => "Filter".into(),
            Pitch => "Pitch fader".into(),
            Crossfader => "Crossfader".into(),
            CueGain => "Headphone level".into(),
            CueMix => "Headphone mix".into(),
            MasterGain => "Master level".into(),
            BoothGain => "Booth level".into(),
            SamplerVolume => "Sampler volume".into(),
            Browse => "Browse (encoder)".into(),
        }
    }
}
