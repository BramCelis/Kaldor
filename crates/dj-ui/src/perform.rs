//! Performance logic shared by the pads panel, the deck panels and the
//! keyboard: pad modes, loops, beat jump, key shift, sync, grid editing, and
//! keeping hot cues, saved loops and grids in the library.
//!
//! Positions here are in seconds (library units); the engine works in frames
//! at its sample rate, so everything is converted on the way in and out.

use crate::app::KaldorApp;
use dj_audio::{Command, DeckCommand, DeckGrid, HOT_CUES, SamplerCommand, TrackMarkers};
use dj_core::{Beatgrid, DeckId};
use dj_library::{Cue, CueKind, SavedLoop};
use std::time::Instant;

/// What the 8 pads of a deck do.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PadMode {
    #[default]
    Cue,
    SavedLoop,
    AutoLoop,
    Roll,
    Sampler,
    Slicer,
    PitchPlay,
    /// Each pad jumps a number of beats back or forward
    /// ([`BEAT_JUMP_PADS`]).
    BeatJump,
    Stems,
}

impl PadMode {
    pub const ALL: [PadMode; 9] = [
        PadMode::Cue,
        PadMode::SavedLoop,
        PadMode::AutoLoop,
        PadMode::Roll,
        PadMode::Sampler,
        PadMode::Slicer,
        PadMode::PitchPlay,
        PadMode::BeatJump,
        PadMode::Stems,
    ];

    /// Tab label.
    pub fn short(self) -> &'static str {
        match self {
            PadMode::Cue => "CUES",
            PadMode::SavedLoop => "LOOPS",
            PadMode::AutoLoop => "AUTO",
            PadMode::Roll => "ROLL",
            PadMode::Sampler => "SAMPLE",
            PadMode::Slicer => "SLICER",
            PadMode::PitchPlay => "PITCH",
            PadMode::BeatJump => "JUMP",
            PadMode::Stems => "STEMS",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            PadMode::Cue => {
                "Hot cues: an empty pad sets a cue, a set pad jumps there. Right-click: name, colour, delete"
            }
            PadMode::SavedLoop => {
                "Saved loops: an empty pad saves the current loop, a saved pad starts it. Right-click: name, colour, delete"
            }
            PadMode::AutoLoop => "Auto loops of 1/8 to 16 beats; press again to leave",
            PadMode::Roll => "Loop rolls while held; the track carries on underneath",
            PadMode::Sampler => "Sampler slots of the bank shown in the SAMPLER tab",
            PadMode::Slicer => {
                "Slicer: the beats around the playhead in 8 slices; hold a pad to play its slice"
            }
            PadMode::PitchPlay => "Pitch play: plays the chosen hot cue shifted by semitones",
            PadMode::BeatJump => "Beat jump: each pad jumps back or forward 1, 2, 4 or 8 beats",
            PadMode::Stems => "Stems need stem separation, which is not available yet",
        }
    }
}

/// Beat jump pads (beats; negative = back), in pairs as on Pioneer DJ
/// controllers: pad 1 back 1, pad 2 forward 1, pad 3 back 2 ...
pub const BEAT_JUMP_PADS: [f64; 8] = [-1.0, 1.0, -2.0, 2.0, -4.0, 4.0, -8.0, 8.0];
/// Key shift pads (the key in semitones; the DDJ-FLX6's KEY SHIFT mode):
/// top row up 1-4, bottom row down 1-4.
pub const KEY_SHIFT_PADS: [i8; 8] = [1, 2, 3, 4, -1, -2, -3, -4];
/// Auto loop pad sizes (beats).
pub const AUTO_LOOP_PADS: [f32; 8] = [0.125, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
/// Loop roll pad sizes (beats).
pub const ROLL_PADS: [f32; 8] = [0.03125, 0.0625, 0.125, 0.25, 0.5, 1.0, 2.0, 4.0];
/// Pitch play pad steps (semitones).
pub const PITCH_PADS: [i8; 8] = [-3, -2, -1, 0, 1, 2, 3, 4];
/// Auto loop sizes: 1/32 to 32 beats.
pub const LOOP_SIZE_RANGE: (f32, f32) = (0.03125, 32.0);
/// Beat jump sizes: 1/8 to 64 beats.
pub const JUMP_SIZE_RANGE: (f32, f32) = (0.125, 64.0);
/// Slicer domain sizes (beats).
pub const SLICER_DOMAINS: [f32; 4] = [4.0, 8.0, 16.0, 32.0];
/// Grid SLIP steps (seconds): small, and large with Shift.
pub const GRID_NUDGE: f64 = 0.002;
pub const GRID_NUDGE_LARGE: f64 = 0.010;
/// Grid ADJUST steps: change of the beat length (seconds) per step.
pub const GRID_ADJUST: f64 = 0.0002;
pub const GRID_ADJUST_LARGE: f64 = 0.001;

/// Hot cue and saved loop colours (our own palette), one per pad.
pub const PAD_COLORS: [u32; 8] = [
    0xE8_4A_4A, 0xF0_8C_2E, 0xE8_C8_3A, 0x5C_C8_5C, 0x3A_C8_C8, 0x4A_7C_F0, 0x9C_5C_E8, 0xE0_5C_B4,
];

/// "1/4", "1", "16".
pub fn size_label(beats: f32) -> String {
    if beats >= 1.0 {
        format!("{}", beats.round() as i32)
    } else {
        format!("1/{}", (1.0 / beats).round() as i32)
    }
}

/// Name and colour of a hot cue or saved loop.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkerMeta {
    pub name: String,
    pub color: u32,
}

/// A saved loop (seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct SavedLoopUi {
    pub start: f64,
    pub end: f64,
    pub meta: MarkerMeta,
}

/// Per-deck performance state on the UI side.
#[derive(Clone, Debug)]
pub struct DeckPerf {
    pub pad_mode: PadMode,
    /// Pads the mouse or keyboard holds.
    pub held: [bool; HOT_CUES],
    /// Hot cue positions (seconds) as last seen in the engine.
    pub hot_cues: [Option<f64>; HOT_CUES],
    pub cue_meta: [MarkerMeta; HOT_CUES],
    /// Main cue (seconds) as last seen.
    pub main_cue: Option<f64>,
    pub saved_loops: [Option<SavedLoopUi>; HOT_CUES],
    pub auto_loop_beats: f32,
    pub jump_beats: f32,
    pub slicer_domain: f32,
    /// Cue pads also start a loop of the auto loop size.
    pub cue_loop: bool,
    /// Hot cue pitch play starts from.
    pub pitch_cue: usize,
    /// Tap tempo: track times (seconds) of the taps.
    pub taps: Vec<f64>,
    pub last_tap: Option<Instant>,
    /// Cues, loops or grid changed and are not in the library yet.
    pub dirty: bool,
    /// Markers to send after the next Load (from the library).
    pub send_after_load: bool,
    /// The Load serial the engine state above belongs to.
    pub serial: u64,
    /// Turn SYNC (and, with `remaster`, MASTER) back on after the next
    /// load (the audio output changed, so the engine is new).
    pub resync: bool,
    pub remaster: bool,
}

impl Default for DeckPerf {
    fn default() -> Self {
        Self {
            pad_mode: PadMode::Cue,
            held: [false; HOT_CUES],
            hot_cues: [None; HOT_CUES],
            cue_meta: std::array::from_fn(default_meta),
            main_cue: None,
            saved_loops: Default::default(),
            auto_loop_beats: 4.0,
            jump_beats: 4.0,
            slicer_domain: 8.0,
            cue_loop: false,
            pitch_cue: 0,
            taps: Vec::new(),
            last_tap: None,
            dirty: false,
            send_after_load: false,
            serial: 0,
            resync: false,
            remaster: false,
        }
    }
}

fn default_meta(slot: usize) -> MarkerMeta {
    MarkerMeta {
        name: String::new(),
        color: PAD_COLORS[slot % PAD_COLORS.len()],
    }
}

impl DeckPerf {
    /// Forgets the track's markers (a new track is loading).
    pub fn clear_markers(&mut self) {
        self.hot_cues = [None; HOT_CUES];
        self.cue_meta = std::array::from_fn(default_meta);
        self.main_cue = None;
        self.saved_loops = Default::default();
        self.taps.clear();
        self.held = [false; HOT_CUES];
        self.dirty = false;
        self.send_after_load = false;
    }

    /// Takes cues and loops read from the library.
    pub fn set_from_library(&mut self, cues: &[Cue], loops: &[SavedLoop]) {
        self.clear_markers();
        for c in cues {
            match c.kind {
                CueKind::Main => self.main_cue = Some(c.position),
                CueKind::Hot(s) if usize::from(s) < HOT_CUES => {
                    let s = usize::from(s);
                    self.hot_cues[s] = Some(c.position);
                    self.cue_meta[s] = MarkerMeta {
                        name: c.name.clone(),
                        color: c.color.unwrap_or(PAD_COLORS[s]),
                    };
                }
                _ => {}
            }
        }
        for l in loops {
            let s = usize::from(l.slot);
            if s < HOT_CUES && l.end > l.start {
                self.saved_loops[s] = Some(SavedLoopUi {
                    start: l.start,
                    end: l.end,
                    meta: MarkerMeta {
                        name: l.name.clone(),
                        color: l.color.unwrap_or(PAD_COLORS[s]),
                    },
                });
            }
        }
        self.send_after_load = true;
    }

    /// Cues as stored in the library (memory cues are kept by the caller).
    pub fn library_cues(&self) -> Vec<Cue> {
        let mut v = Vec::new();
        if let Some(p) = self.main_cue.filter(|p| *p > 0.0) {
            v.push(Cue {
                kind: CueKind::Main,
                position: p,
                name: String::new(),
                color: None,
            });
        }
        for (s, p) in self.hot_cues.iter().enumerate() {
            if let Some(p) = p {
                v.push(Cue {
                    kind: CueKind::Hot(s as u8),
                    position: *p,
                    name: self.cue_meta[s].name.clone(),
                    color: Some(self.cue_meta[s].color),
                });
            }
        }
        v
    }

    pub fn library_loops(&self) -> Vec<SavedLoop> {
        self.saved_loops
            .iter()
            .enumerate()
            .filter_map(|(s, l)| {
                l.as_ref().map(|l| SavedLoop {
                    slot: s as u8,
                    start: l.start,
                    end: l.end,
                    name: l.meta.name.clone(),
                    color: Some(l.meta.color),
                })
            })
            .collect()
    }
}

/// A Ctrl-drag on a zoomed waveform that slips (moves) or, with Shift,
/// adjusts (stretches) the grid; the beat under the mouse follows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridDrag {
    /// The grid when the drag started.
    pub start: Beatgrid,
    pub adjust: bool,
    /// Track time under the mouse at the start (seconds).
    pub at: f64,
    /// The play position at the start (seconds): an adjust keeps the beat
    /// there in place.
    pub pivot: f64,
}

/// Changes to a beatgrid.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GridEdit {
    /// Beat one at the play position.
    SetBeatOne,
    /// Slip: moves the whole grid (seconds; negative = earlier).
    Nudge(f64),
    /// Adjust: changes the beat length by this many seconds (negative = a
    /// faster grid) around the play position. The beat at the play position
    /// (the middle of the scrolling waveform) stays where it is; beats
    /// further away move more.
    Adjust(f64),
    Halve,
    Double,
    /// A tap of tap tempo at the play position.
    Tap,
    ToggleLock,
}

impl KaldorApp {
    fn deck_rate(&self) -> f64 {
        f64::from(self.engine.snapshot().sample_rate.max(1))
    }

    pub(crate) fn deck_command(&mut self, deck: usize, c: DeckCommand) {
        if let Some(id) = DeckId::from_index(deck) {
            self.send(Command::Deck(id, c));
        }
    }

    /// The deck's grid, main cue and hot cues in engine frames, sent with
    /// its Load (a Load clears them in the engine).
    pub(crate) fn track_markers(&self, deck: usize) -> TrackMarkers {
        let rate = self.output_info.sample_rate;
        let sr = f64::from(rate.max(1));
        let ui = &self.decks[deck];
        TrackMarkers {
            grid: ui.grid.and_then(|g| DeckGrid::from_beatgrid(&g, rate)),
            cue_point: ui.perf.main_cue.map(|c| c * sr),
            hot_cues: ui.perf.hot_cues.map(|c| c.map(|p| p * sr)),
            start: None,
        }
    }

    /// Sends the grid only (analysis finished, grid edited).
    pub(crate) fn send_grid(&mut self, deck: usize) {
        let rate = self.engine.snapshot().sample_rate;
        let grid = self.decks[deck]
            .grid
            .and_then(|g| DeckGrid::from_beatgrid(&g, rate));
        self.deck_command(deck, DeckCommand::SetGrid(grid));
    }

    /// Reads hot cues and the main cue back from the engine, and writes
    /// changed cues, loops and grids to the library.
    pub(crate) fn sync_markers(&mut self) {
        let snap = *self.engine.snapshot();
        let sr = f64::from(snap.sample_rate.max(1));
        let fixed_color = self.config.ui.hot_cue_color;
        for i in 0..dj_core::NUM_DECKS {
            let d = snap.decks[i];
            let perf = &mut self.decks[i].perf;
            if !d.loaded || d.track_serial != perf.serial || perf.send_after_load {
                continue;
            }
            for s in 0..HOT_CUES {
                let engine = d.hot_cues[s].map(|f| f / sr);
                let same = match (engine, perf.hot_cues[s]) {
                    (Some(a), Some(b)) => (a - b).abs() < 1e-6,
                    (None, None) => true,
                    _ => false,
                };
                if !same {
                    if engine.is_none() || perf.hot_cues[s].is_none() {
                        // Deleted or new: default name and colour.
                        perf.cue_meta[s] = default_meta(s);
                        // New cues get the colour from the settings.
                        if engine.is_some()
                            && let Some(c) = fixed_color
                        {
                            perf.cue_meta[s].color = c;
                        }
                    }
                    perf.hot_cues[s] = engine;
                    perf.dirty = true;
                }
            }
            let main = (d.cue_point > 0.0).then_some(d.cue_point / sr);
            let same = match (main, perf.main_cue) {
                (Some(a), Some(b)) => (a - b).abs() < 1e-6,
                (None, None) => true,
                _ => false,
            };
            if !same {
                perf.main_cue = main;
                perf.dirty = true;
            }
        }
        self.save_markers();
    }

    /// Writes dirty decks' cues and loops to the library.
    pub(crate) fn save_markers(&mut self) {
        for i in 0..dj_core::NUM_DECKS {
            if !self.decks[i].perf.dirty {
                continue;
            }
            self.decks[i].perf.dirty = false;
            let Some(id) = self.decks[i].track_id else {
                continue;
            };
            let Some(lib) = self.library.as_mut() else {
                continue;
            };
            // A protected library keeps its cues and loops (the deck still
            // has the new ones until the next load).
            if lib.protect {
                continue;
            }
            // Memory cues (from elsewhere) are kept.
            let mut cues: Vec<Cue> = lib
                .lib
                .cues(id)
                .unwrap_or_default()
                .into_iter()
                .filter(|c| c.kind == CueKind::Memory)
                .collect();
            cues.extend(self.decks[i].perf.library_cues());
            // The track list's small waveform shows the new hot cues.
            lib.minis.forget(id);
            if let Err(e) = lib.lib.set_cues(id, &cues) {
                tracing::warn!("cannot save cues: {e}");
            }
            if let Err(e) = lib.lib.set_loops(id, &self.decks[i].perf.library_loops()) {
                tracing::warn!("cannot save loops: {e}");
            }
        }
    }

    // ----- pads ------------------------------------------------------------

    /// Pad `pad` (0-7) of `deck` pressed or released, in the deck's pad mode.
    pub fn pad(&mut self, deck: usize, pad: usize, pressed: bool) {
        if deck >= dj_core::NUM_DECKS || pad >= HOT_CUES {
            return;
        }
        let was = self.decks[deck].perf.held[pad];
        if was == pressed {
            return;
        }
        self.decks[deck].perf.held[pad] = pressed;
        let perf = self.decks[deck].perf.clone();
        let snap = self.engine.snapshot().decks[deck];
        let sr = self.deck_rate();
        match perf.pad_mode {
            PadMode::Cue => {
                let loop_beats = if perf.cue_loop {
                    perf.auto_loop_beats
                } else {
                    0.0
                };
                self.deck_command(
                    deck,
                    DeckCommand::HotCue {
                        slot: pad as u8,
                        pressed,
                        loop_beats,
                    },
                );
            }
            PadMode::SavedLoop => {
                if !pressed {
                    return;
                }
                match &perf.saved_loops[pad] {
                    Some(l) => {
                        let active = snap.loop_info.filter(|x| x.active);
                        let same = active.is_some_and(|a| {
                            (a.start / sr - l.start).abs() < 1e-3
                                && (a.end / sr - l.end).abs() < 1e-3
                        });
                        if same {
                            self.deck_command(deck, DeckCommand::LoopExit);
                        } else {
                            self.deck_command(
                                deck,
                                DeckCommand::SetLoop {
                                    start: l.start * sr,
                                    end: l.end * sr,
                                    jump: true,
                                },
                            );
                        }
                    }
                    None => {
                        if let Some(l) = snap.loop_info {
                            self.decks[deck].perf.saved_loops[pad] = Some(SavedLoopUi {
                                start: l.start / sr,
                                end: l.end / sr,
                                meta: default_meta(pad),
                            });
                            self.decks[deck].perf.dirty = true;
                        }
                    }
                }
            }
            PadMode::BeatJump => {
                if pressed {
                    self.deck_command(deck, DeckCommand::BeatJump(BEAT_JUMP_PADS[pad]));
                }
            }
            PadMode::AutoLoop => {
                if pressed {
                    let beats = AUTO_LOOP_PADS[pad];
                    self.decks[deck].perf.auto_loop_beats = beats;
                    self.deck_command(deck, DeckCommand::AutoLoop(beats));
                }
            }
            PadMode::Roll => {
                self.deck_command(
                    deck,
                    DeckCommand::Roll {
                        beats: ROLL_PADS[pad],
                        pressed,
                    },
                );
            }
            PadMode::Sampler => {
                let slot = (self.sampler.bank * dj_audio::SAMPLER_BANK + pad) as u8;
                self.send(Command::Sampler(SamplerCommand::Trigger { slot, pressed }));
            }
            PadMode::Slicer => {
                self.deck_command(
                    deck,
                    DeckCommand::Slicer {
                        slice: pad as u8,
                        pressed,
                        domain_beats: perf.slicer_domain,
                    },
                );
            }
            PadMode::PitchPlay => {
                if pressed {
                    self.deck_command(deck, DeckCommand::KeyShift(PITCH_PADS[pad]));
                }
                if snap.hot_cues[perf.pitch_cue].is_some() {
                    self.deck_command(
                        deck,
                        DeckCommand::HotCue {
                            slot: perf.pitch_cue as u8,
                            pressed,
                            loop_beats: 0.0,
                        },
                    );
                }
            }
            PadMode::Stems => {}
        }
    }

    /// Second function of a pad (Shift + pad, right-click): delete the hot
    /// cue or saved loop, stop the sample.
    pub fn pad_secondary(&mut self, deck: usize, pad: usize) {
        if deck >= dj_core::NUM_DECKS || pad >= HOT_CUES {
            return;
        }
        match self.decks[deck].perf.pad_mode {
            PadMode::Cue | PadMode::PitchPlay => self.delete_hot_cue(deck, pad),
            PadMode::SavedLoop => self.delete_saved_loop(deck, pad),
            PadMode::Sampler => {
                let slot = (self.sampler.bank * dj_audio::SAMPLER_BANK + pad) as u8;
                self.send(Command::Sampler(SamplerCommand::Stop(slot)));
            }
            _ => {}
        }
    }

    pub(crate) fn delete_hot_cue(&mut self, deck: usize, slot: usize) {
        self.deck_command(
            deck,
            DeckCommand::SetHotCue {
                slot: slot as u8,
                position: None,
            },
        );
    }

    pub(crate) fn delete_saved_loop(&mut self, deck: usize, slot: usize) {
        if self.decks[deck].perf.saved_loops[slot].take().is_some() {
            self.decks[deck].perf.dirty = true;
        }
    }

    /// Releases every held pad of a deck (pad mode change, focus loss).
    pub(crate) fn release_pads(&mut self, deck: usize) {
        for p in 0..HOT_CUES {
            if self.decks[deck].perf.held[p] {
                self.pad(deck, p, false);
            }
        }
    }

    pub fn set_pad_mode(&mut self, deck: usize, mode: PadMode) {
        if deck < dj_core::NUM_DECKS && self.decks[deck].perf.pad_mode != mode {
            self.release_pads(deck);
            self.decks[deck].perf.pad_mode = mode;
        }
    }

    pub fn pad_mode(&self, deck: usize) -> PadMode {
        self.decks[deck].perf.pad_mode
    }

    // ----- loops, jump, key, sync -------------------------------------------

    pub(crate) fn auto_loop(&mut self, deck: usize) {
        let beats = self.decks[deck].perf.auto_loop_beats;
        self.deck_command(deck, DeckCommand::AutoLoop(beats));
    }

    /// Halves (`factor` 0.5) or doubles the auto loop size, and the active
    /// loop with it.
    pub(crate) fn scale_loop(&mut self, deck: usize, factor: f32) {
        let p = &mut self.decks[deck].perf;
        p.auto_loop_beats =
            (p.auto_loop_beats * factor).clamp(LOOP_SIZE_RANGE.0, LOOP_SIZE_RANGE.1);
        let active = self.engine.snapshot().decks[deck]
            .loop_info
            .is_some_and(|l| l.active);
        if active {
            self.deck_command(
                deck,
                if factor < 1.0 {
                    DeckCommand::LoopHalve
                } else {
                    DeckCommand::LoopDouble
                },
            );
        }
    }

    pub(crate) fn scale_jump(&mut self, deck: usize, factor: f32) {
        let p = &mut self.decks[deck].perf;
        p.jump_beats = (p.jump_beats * factor).clamp(JUMP_SIZE_RANGE.0, JUMP_SIZE_RANGE.1);
    }

    pub(crate) fn beat_jump(&mut self, deck: usize, dir: f64) {
        let beats = f64::from(self.decks[deck].perf.jump_beats) * dir;
        self.deck_command(deck, DeckCommand::BeatJump(beats));
    }

    pub(crate) fn key_shift_by(&mut self, deck: usize, delta: i8) {
        let now = self.engine.snapshot().decks[deck].key_shift;
        let next = if delta == 0 {
            0
        } else {
            now.saturating_add(delta)
                .clamp(-dj_audio::MAX_KEY_SHIFT, dj_audio::MAX_KEY_SHIFT)
        };
        self.deck_command(deck, DeckCommand::KeyShift(next));
    }

    pub(crate) fn toggle_sync(&mut self, deck: usize) {
        let on = self.engine.snapshot().decks[deck].sync;
        if let Some(id) = DeckId::from_index(deck) {
            self.send(Command::Sync(id, !on));
        }
    }

    pub(crate) fn make_master(&mut self, deck: usize) {
        if let Some(id) = DeckId::from_index(deck) {
            self.send(Command::SyncMaster(id));
        }
    }

    pub(crate) fn toggle_slip(&mut self, deck: usize) {
        let on = self.engine.snapshot().decks[deck].slip;
        self.deck_command(deck, DeckCommand::SetSlip(!on));
    }

    pub fn set_quantize(&mut self, on: bool) {
        self.config.performance.quantize = on;
        self.send(Command::Quantize(on));
        self.events.push(crate::app::AppEvent::ConfigChanged);
    }

    pub fn set_smart_sync(&mut self, on: bool) {
        self.config.performance.smart_sync = on;
        self.send(Command::SetSyncMode(if on {
            dj_audio::SyncMode::Phase
        } else {
            dj_audio::SyncMode::Tempo
        }));
        self.events.push(crate::app::AppEvent::ConfigChanged);
    }

    // ----- grid editing -------------------------------------------------------

    /// Edits the deck's beatgrid, sends it to the engine and saves it.
    pub fn grid_edit(&mut self, deck: usize, edit: GridEdit) {
        let snap = self.engine.snapshot().decks[deck];
        if !snap.loaded {
            return;
        }
        let sr = self.deck_rate();
        let t = self.estimate_position(&snap) / sr;
        let ui = &mut self.decks[deck];
        let mut grid = ui.grid;
        match edit {
            GridEdit::ToggleLock => {
                if let Some(g) = grid.as_mut() {
                    g.locked = !g.locked;
                }
            }
            _ if grid.is_some_and(|g| g.locked) => return,
            GridEdit::SetBeatOne => {
                match grid.as_mut() {
                    Some(g) => g.set_beat_one(t),
                    None => {
                        // No grid yet: start one from the tag BPM if there is one.
                        let Some(bpm) = ui.bpm.filter(|b| *b > 0.0) else {
                            return;
                        };
                        grid = Some(Beatgrid::new(bpm, t));
                    }
                }
            }
            GridEdit::Nudge(dt) => {
                if let Some(g) = grid.as_mut() {
                    g.nudge(dt);
                }
            }
            GridEdit::Adjust(dp) => {
                if let Some(g) = grid.as_mut() {
                    // The next beat moves by `dp`: the beat length changes by
                    // `dp` with the beat at the play position fixed.
                    let next = t + g.period();
                    g.stretch(t, next, dp);
                }
            }
            GridEdit::Halve => {
                if let Some(g) = grid.as_mut() {
                    g.halve();
                }
            }
            GridEdit::Double => {
                if let Some(g) = grid.as_mut() {
                    g.double();
                }
            }
            GridEdit::Tap => {
                let p = &mut ui.perf;
                let now = Instant::now();
                if p.last_tap
                    .is_none_or(|l| now.duration_since(l).as_secs_f64() > 2.0)
                {
                    p.taps.clear();
                }
                p.last_tap = Some(now);
                p.taps.push(t);
                if p.taps.len() < 4 {
                    return;
                }
                let Some(bpm) = dj_core::beatgrid::tap_tempo(&p.taps) else {
                    return;
                };
                // The last tap is a beat; keep its place in the bar.
                let bar_pos = grid.map_or(1, |old| {
                    let n = old.beat_at(t).round() as i64;
                    ((n + i64::from(old.first_beat_in_bar) - 1).rem_euclid(4) + 1) as u8
                });
                let mut g = Beatgrid {
                    bpm,
                    first_beat: t,
                    first_beat_in_bar: bar_pos,
                    locked: false,
                    bar_zero: 0,
                };
                // Bar 1 stays where it was (its downbeat nearest the old
                // one), else it is the first downbeat of the track.
                match grid {
                    Some(old) => {
                        let zero = old.beat_time(old.bar_zero);
                        let n = g.beat_at(zero).round() as i64;
                        // The downbeat of the bar that beat is in.
                        let in_bar = (n + i64::from(bar_pos) - 1).rem_euclid(4);
                        g.bar_zero = n - in_bar;
                        g.nudge(0.0);
                    }
                    None => {
                        g.nudge(0.0);
                        g.number_bars_from_start();
                    }
                }
                grid = Some(g);
            }
        }
        if grid == ui.grid {
            return;
        }
        self.apply_grid(deck, grid, true);
    }

    /// Shows `grid` on the deck, sends it to the engine and (with `save`)
    /// stores it in the library, where re-analysis keeps it.
    fn apply_grid(&mut self, deck: usize, grid: Option<Beatgrid>, save: bool) {
        let ui = &mut self.decks[deck];
        ui.grid = grid;
        ui.bpm = grid.map(|g| g.bpm).or(ui.bpm);
        self.send_grid(deck);
        if !save {
            return;
        }
        if let (Some(id), Some(g), Some(lib)) =
            (self.decks[deck].track_id, grid, self.library.as_mut())
        {
            if lib.protect {
                lib.status =
                    "Library is protected: the grid edit is not saved (Settings > Library)".into();
                return;
            }
            if let Err(e) = lib.lib.set_beatgrid(id, &g) {
                tracing::warn!("cannot save the beatgrid: {e}");
            }
            lib.mark_dirty();
        }
    }

    /// Starts a grid drag on the waveform at track time `at`, with the play
    /// position at `pivot` (seconds). False when there is no grid or it is
    /// locked.
    pub(crate) fn grid_drag_begin(
        &mut self,
        deck: usize,
        at: f64,
        pivot: f64,
        adjust: bool,
    ) -> bool {
        let ui = &mut self.decks[deck];
        let Some(start) = ui.grid.filter(|g| !g.locked) else {
            return false;
        };
        ui.grid_drag = Some(GridDrag {
            start,
            adjust,
            at,
            pivot,
        });
        true
    }

    /// Moves a grid drag: the beat that was under the mouse is now `dt`
    /// seconds later (negative: earlier). An adjust keeps the beat at the
    /// play position in place.
    pub(crate) fn grid_drag_to(&mut self, deck: usize, dt: f64) {
        let Some(d) = self.decks[deck].grid_drag else {
            return;
        };
        let mut g = d.start;
        if d.adjust {
            g.stretch(d.pivot, d.at, dt);
        } else {
            g.nudge(dt);
        }
        if Some(g) != self.decks[deck].grid {
            self.apply_grid(deck, Some(g), false);
        }
    }

    /// Ends a grid drag and saves the grid.
    pub(crate) fn grid_drag_end(&mut self, deck: usize) {
        let Some(d) = self.decks[deck].grid_drag.take() else {
            return;
        };
        let g = self.decks[deck].grid;
        if g != Some(d.start) {
            self.apply_grid(deck, g, true);
        }
    }

    /// The name of hot cue `slot` of `deck` (empty when unnamed).
    pub fn hot_cue_name(&self, deck: usize, slot: usize) -> &str {
        &self.decks[deck].perf.cue_meta[slot].name
    }

    /// The keyboard deck (F1-F4).
    pub fn keyboard_deck(&self) -> usize {
        self.kb_deck
    }

    pub fn set_keyboard_deck(&mut self, deck: usize) {
        if deck < dj_core::NUM_DECKS && deck != self.kb_deck {
            let old = self.kb_deck;
            self.release_pads(old);
            self.kb_deck = deck;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_library_round_trip() {
        assert_eq!(size_label(0.25), "1/4");
        assert_eq!(size_label(16.0), "16");
        assert_eq!(size_label(0.03125), "1/32");
        let mut p = DeckPerf::default();
        p.hot_cues[2] = Some(12.5);
        p.cue_meta[2].name = "drop".into();
        p.main_cue = Some(1.0);
        p.saved_loops[5] = Some(SavedLoopUi {
            start: 3.0,
            end: 5.0,
            meta: MarkerMeta {
                name: "intro".into(),
                color: 0x123456,
            },
        });
        let (cues, loops) = (p.library_cues(), p.library_loops());
        let mut q = DeckPerf::default();
        q.set_from_library(&cues, &loops);
        assert_eq!(q.hot_cues, p.hot_cues);
        assert_eq!(q.cue_meta, p.cue_meta);
        assert_eq!(q.main_cue, p.main_cue);
        assert_eq!(q.saved_loops, p.saved_loops);
        assert!(q.send_after_load);
    }
}
