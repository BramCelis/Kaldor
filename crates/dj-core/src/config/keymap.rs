//! The keyboard map: what a key can do, the bindings and the default laptop
//! keymap.

use serde::{Deserialize, Serialize};

/// Something a key can do. Deck numbers are 1 to 4.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyAction {
    /// Hold: CUE button.
    Cue(u8),
    /// Press: play/pause.
    Play(u8),
    /// Hold: pitch bend down / up.
    BendDown(u8),
    BendUp(u8),
    /// Press: toggle key lock.
    KeyLock(u8),
    /// Press: toggle headphone cue.
    Pfl(u8),
    CrossfaderLeft,
    CrossfaderRight,
    CrossfaderCenter,
    ZoomIn,
    ZoomOut,
    // The actions below work on the keyboard deck (chosen with FocusDeck).
    /// Press: the deck the keys below work on (1 to 4).
    FocusDeck(u8),
    /// Hold: pad 1 to 8 in the deck's pad mode.
    Pad(u8),
    /// Press: the pad's second function (delete a cue or saved loop, stop a
    /// sample).
    PadSecondary(u8),
    /// Press: pad mode 1 to 8 (Cue, Saved loop, Auto loop, Roll, Sampler,
    /// Slicer, Pitch play, Stems).
    PadMode(u8),
    LoopIn,
    LoopOut,
    /// Leave the loop, or start the last one again.
    Reloop,
    /// Auto loop of the deck's loop size (again: leave it).
    AutoLoop,
    /// Halve / double the loop and the auto loop size.
    LoopHalve,
    LoopDouble,
    LoopMoveBack,
    LoopMoveForward,
    BeatJumpBack,
    BeatJumpForward,
    /// Smaller / larger beat jump size.
    BeatJumpSizeDown,
    BeatJumpSizeUp,
    Sync,
    SyncMaster,
    Slip,
    /// All decks.
    Quantize,
    KeyShiftDown,
    KeyShiftUp,
    KeyShiftReset,
    GridSetBeatOne,
    GridNudgeEarlier,
    GridNudgeLater,
    GridAdjustEarlier,
    GridAdjustLater,
    GridHalve,
    GridDouble,
    GridTap,
    GridLock,
    SamplerBankPrev,
    SamplerBankNext,
    /// Press: FX unit 1 or 2 on or off.
    FxUnit(u8),
    /// Press: effect 1-3 of FX unit 1-2 on or off (unit, effect).
    FxEffect(u8, u8),
    /// Shorter / longer beats setting of both FX units.
    FxBeatsDown,
    FxBeatsUp,
    /// Tap the FX tempo.
    FxTap,
    /// Master recording on or off.
    Record,
    /// Full screen on or off (the window covers the taskbar).
    ToggleFullscreen,
}

impl KeyAction {
    /// A name for the keyboard settings ("Deck 1: play / pause").
    pub fn label(self) -> String {
        use KeyAction::*;
        match self {
            Cue(d) => format!("Deck {d}: CUE"),
            Play(d) => format!("Deck {d}: play / pause"),
            BendDown(d) => format!("Deck {d}: pitch bend down"),
            BendUp(d) => format!("Deck {d}: pitch bend up"),
            KeyLock(d) => format!("Deck {d}: key lock"),
            Pfl(d) => format!("Deck {d}: headphone cue"),
            CrossfaderLeft => "Crossfader to A".into(),
            CrossfaderRight => "Crossfader to B".into(),
            CrossfaderCenter => "Crossfader to the middle".into(),
            ZoomIn => "Waveform zoom in".into(),
            ZoomOut => "Waveform zoom out".into(),
            FocusDeck(d) => format!("Keyboard deck: deck {d}"),
            Pad(n) => format!("Pad {n}"),
            PadSecondary(n) => format!("Pad {n}: delete / stop"),
            PadMode(n) => format!(
                "Pad mode: {}",
                [
                    "Cue",
                    "Saved loop",
                    "Auto loop",
                    "Roll",
                    "Sampler",
                    "Slicer",
                    "Pitch play",
                    "Stems"
                ]
                .get(usize::from(n.saturating_sub(1)))
                .copied()
                .unwrap_or("?")
            ),
            LoopIn => "Loop in".into(),
            LoopOut => "Loop out".into(),
            Reloop => "Reloop / exit loop".into(),
            AutoLoop => "Auto loop".into(),
            LoopHalve => "Loop halve".into(),
            LoopDouble => "Loop double".into(),
            LoopMoveBack => "Loop move back".into(),
            LoopMoveForward => "Loop move forward".into(),
            BeatJumpBack => "Beat jump back".into(),
            BeatJumpForward => "Beat jump forward".into(),
            BeatJumpSizeDown => "Beat jump size smaller".into(),
            BeatJumpSizeUp => "Beat jump size larger".into(),
            Sync => "SYNC".into(),
            SyncMaster => "Make sync master".into(),
            Slip => "Slip".into(),
            Quantize => "Quantize (all decks)".into(),
            KeyShiftDown => "Key shift down".into(),
            KeyShiftUp => "Key shift up".into(),
            KeyShiftReset => "Key shift reset".into(),
            GridSetBeatOne => "Grid: set beat one".into(),
            GridNudgeEarlier => "Grid: nudge earlier".into(),
            GridNudgeLater => "Grid: nudge later".into(),
            GridAdjustEarlier => "Grid: beats closer".into(),
            GridAdjustLater => "Grid: beats wider".into(),
            GridHalve => "Grid: halve tempo".into(),
            GridDouble => "Grid: double tempo".into(),
            GridTap => "Grid: tap tempo".into(),
            GridLock => "Grid: lock".into(),
            SamplerBankPrev => "Sampler: previous bank".into(),
            SamplerBankNext => "Sampler: next bank".into(),
            FxUnit(u) => format!("FX unit {u} on / off"),
            FxEffect(u, e) => format!("FX unit {u}: effect {e}"),
            FxBeatsDown => "FX beats shorter".into(),
            FxBeatsUp => "FX beats longer".into(),
            FxTap => "FX tap tempo".into(),
            Record => "Record the master".into(),
            ToggleFullscreen => "Full screen".into(),
        }
    }

    /// The group the keyboard settings show this action in.
    pub fn group(self) -> &'static str {
        use KeyAction::*;
        match self {
            Cue(_) | Play(_) | BendDown(_) | BendUp(_) | KeyLock(_) | Pfl(_) => "Decks",
            CrossfaderLeft | CrossfaderRight | CrossfaderCenter | ZoomIn | ZoomOut | Record
            | ToggleFullscreen => "Mixer and view",
            FocusDeck(_) | Pad(_) | PadSecondary(_) | PadMode(_) => "Keyboard deck: pads",
            LoopIn | LoopOut | Reloop | AutoLoop | LoopHalve | LoopDouble | LoopMoveBack
            | LoopMoveForward | BeatJumpBack | BeatJumpForward | BeatJumpSizeDown
            | BeatJumpSizeUp | Sync | SyncMaster | Slip | Quantize | KeyShiftDown | KeyShiftUp
            | KeyShiftReset => "Keyboard deck: loops, jumps, sync",
            GridSetBeatOne | GridNudgeEarlier | GridNudgeLater | GridAdjustEarlier
            | GridAdjustLater | GridHalve | GridDouble | GridTap | GridLock => {
                "Keyboard deck: beatgrid"
            }
            SamplerBankPrev | SamplerBankNext | FxUnit(_) | FxEffect(..) | FxBeatsDown
            | FxBeatsUp | FxTap => "FX and sampler",
        }
    }
}

/// A key as people read it: "Shift+Q", "Space", "Left arrow".
pub fn key_label(key: &str, shift: bool) -> String {
    let k = match key {
        "ArrowLeft" => "Left arrow".to_string(),
        "ArrowRight" => "Right arrow".to_string(),
        "ArrowUp" => "Up arrow".to_string(),
        "ArrowDown" => "Down arrow".to_string(),
        "OpenBracket" => "[".to_string(),
        "CloseBracket" => "]".to_string(),
        "Semicolon" => ";".to_string(),
        "Quote" => "'".to_string(),
        "Backslash" => "\\".to_string(),
        "Comma" => ",".to_string(),
        "Period" => ".".to_string(),
        "Slash" => "/".to_string(),
        "Backtick" => "`".to_string(),
        "Plus" => "+".to_string(),
        "Minus" => "-".to_string(),
        "PageUp" => "Page Up".to_string(),
        "PageDown" => "Page Down".to_string(),
        k if k.starts_with("Digit") => k.trim_start_matches("Digit").to_string(),
        k => k.to_string(),
    };
    if shift { format!("Shift+{k}") } else { k }
}

/// One key binding. `key` is an egui key name such as "Q", "Space",
/// "ArrowLeft" or "Digit1"; `shift` means the binding needs Shift held.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyBinding {
    pub key: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub shift: bool,
    pub action: KeyAction,
}

impl KeyBinding {
    pub fn new(key: &str, action: KeyAction) -> Self {
        Self {
            key: key.to_string(),
            shift: false,
            action,
        }
    }

    pub fn shifted(key: &str, action: KeyAction) -> Self {
        Self {
            shift: true,
            ..Self::new(key, action)
        }
    }
}

/// Adds the default binding of every action missing from `keymap` (config
/// files written before an action existed), unless its key is taken.
pub fn merge_default_keymap(keymap: &mut Vec<KeyBinding>) {
    for d in default_keymap() {
        let has_action = keymap.iter().any(|b| b.action == d.action);
        let key_taken = keymap.iter().any(|b| b.key == d.key && b.shift == d.shift);
        if !has_action && !key_taken {
            keymap.push(d);
        }
    }
}

/// Default laptop keymap: one row of keys per deck.
pub fn default_keymap() -> Vec<KeyBinding> {
    use KeyAction::*;
    let mut v = Vec::new();
    // (cue, play, bend-, bend+, key lock, pfl) per deck.
    let rows = [
        ["Q", "W", "E", "R", "T", "Y"],
        ["U", "I", "O", "P", "OpenBracket", "CloseBracket"],
        ["A", "S", "D", "F", "G", "H"],
        ["J", "K", "L", "Semicolon", "Quote", "Backslash"],
    ];
    for (i, r) in rows.iter().enumerate() {
        let d = i as u8 + 1;
        v.push(KeyBinding::new(r[0], Cue(d)));
        v.push(KeyBinding::new(r[1], Play(d)));
        v.push(KeyBinding::new(r[2], BendDown(d)));
        v.push(KeyBinding::new(r[3], BendUp(d)));
        v.push(KeyBinding::new(r[4], KeyLock(d)));
        v.push(KeyBinding::new(r[5], Pfl(d)));
    }
    v.push(KeyBinding::new("Z", CrossfaderLeft));
    v.push(KeyBinding::new("X", CrossfaderCenter));
    v.push(KeyBinding::new("C", CrossfaderRight));
    v.push(KeyBinding::new("Plus", ZoomIn));
    v.push(KeyBinding::new("Minus", ZoomOut));
    // Keyboard deck: F1-F4 choose it; the keys below work on it.
    for n in 1..=4u8 {
        v.push(KeyBinding::new(&format!("F{n}"), FocusDeck(n)));
    }
    for n in 1..=8u8 {
        v.push(KeyBinding::new(&format!("Digit{n}"), Pad(n)));
        v.push(KeyBinding::shifted(&format!("Digit{n}"), PadSecondary(n)));
        v.push(KeyBinding::new(&format!("F{}", n + 4), PadMode(n)));
    }
    let plain = [
        ("V", LoopIn),
        ("B", LoopOut),
        ("N", Reloop),
        ("M", AutoLoop),
        ("ArrowDown", LoopHalve),
        ("ArrowUp", LoopDouble),
        ("ArrowLeft", BeatJumpBack),
        ("ArrowRight", BeatJumpForward),
        ("Comma", BeatJumpSizeDown),
        ("Period", BeatJumpSizeUp),
        ("Slash", Sync),
        ("Backtick", Slip),
        ("Digit9", KeyShiftDown),
        ("Digit0", KeyShiftUp),
        ("PageUp", SamplerBankPrev),
        ("PageDown", SamplerBankNext),
    ];
    for (k, a) in plain {
        v.push(KeyBinding::new(k, a));
    }
    let shifted = [
        ("ArrowLeft", LoopMoveBack),
        ("ArrowRight", LoopMoveForward),
        ("Slash", SyncMaster),
        ("Backtick", Quantize),
        ("Digit0", KeyShiftReset),
        ("V", GridSetBeatOne),
        ("Comma", GridNudgeEarlier),
        ("Period", GridNudgeLater),
        ("OpenBracket", GridAdjustEarlier),
        ("CloseBracket", GridAdjustLater),
        ("ArrowDown", GridHalve),
        ("ArrowUp", GridDouble),
        ("N", GridTap),
        ("M", GridLock),
        ("Z", FxUnit(1)),
        ("C", FxUnit(2)),
        ("X", FxTap),
        ("Q", FxEffect(1, 1)),
        ("W", FxEffect(1, 2)),
        ("E", FxEffect(1, 3)),
        ("U", FxEffect(2, 1)),
        ("I", FxEffect(2, 2)),
        ("O", FxEffect(2, 3)),
        ("R", Record),
        // F11 is pad mode 7.
        ("F11", ToggleFullscreen),
    ];
    for (k, a) in shifted {
        v.push(KeyBinding::shifted(k, a));
    }
    v.push(KeyBinding::new("Home", FxBeatsDown));
    v.push(KeyBinding::new("End", FxBeatsUp));
    v
}
