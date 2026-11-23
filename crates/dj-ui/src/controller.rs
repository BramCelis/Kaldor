//! The Numark NS4FX: what each control does.
//!
//! Messages are decoded by the profile-driven [`dj_midi::Mapper`]
//! (`profiles/numark-ns4fx.toml`, built from a capture of the
//! controller). [`Ns4fx`] turns the control events into
//!
//! - engine commands, sent from the MIDI thread straight into the engine's
//!   controller queue (transport, pitch, jog, mixer, hot cue / roll /
//!   sampler / slicer pads): they reach the audio thread at its next block;
//! - UI actions, applied by the UI at its next frame (layers, pad modes,
//!   pads that need UI state, browse and load, key shift, quantize).
//!
//! The behaviour is a plain state machine, so tests replay MIDI bytes
//! through it without a device (see [`ControllerLink::offline`]).

use crate::perform::{PadMode, ROLL_PADS};
use dj_audio::{
    Command, ControllerQueue, DeckCommand, EqBand, FxCommand, MixerCommand, SamplerCommand,
    XfaderSide,
};
use dj_core::DeckId;
use dj_core::config::{FunctionKind, LearnedBinding, MidiFunction};
use dj_midi::profile::Lights;
use dj_midi::{Event, Mapper, Profile, Unit, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

/// Pitch bend while a bend button is held (fraction), as on screen.
const BEND: f64 = 0.04;
/// Fast search (SHIFT + jog): seconds of audio per jog revolution.
const SEARCH_SECONDS_PER_TURN: f64 = 30.0;
/// SHIFT + CUE/LOOP CALL (DDJ-FLX6): beat jump of this many beats.
const FLX_JUMP_BEATS: f64 = 4.0;
/// How many messages the profile did not know are kept for the Setup tab.
/// How long the answer to the state request counts as the dump.
const STATE_DUMP_WINDOW: std::time::Duration = std::time::Duration::from_millis(1000);
/// After connecting, ask again this long after each unanswered request (a
/// controller just plugged in needs a moment to start up).
const STATE_RETRY_AFTER: [std::time::Duration; 4] = [
    std::time::Duration::from_millis(1000),
    std::time::Duration::from_millis(1500),
    std::time::Duration::from_millis(2000),
    std::time::Duration::from_millis(2000),
];
const UNKNOWN_KEPT: usize = 8;
/// After the jog is let go, its ticks keep scratching (a spinning
/// platter) until they pause this long.
const FREE_SPIN_GAP_SECONDS: f64 = 0.1;
/// The controller thread sends lights and displays this often...
const OUTPUT_TICK: Duration = Duration::from_millis(30);
/// Half a flicker period of PLAY and CUE (milliseconds).
const FLICKER_HALF_MS: u128 = 250;
/// ...and checks the MIDI port every this many ticks (about a second).
const PORT_CHECK_TICKS: u32 = 33;
/// The CUE GAIN knob's range in dB, as the on-screen PHONES knob.
const CUE_GAIN_DB: (f32, f32) = (-40.0, 6.0);

/// A level knob (0-1) in dB, over the range of the on-screen level knobs.
fn db(v: f32) -> f32 {
    CUE_GAIN_DB.0 + (CUE_GAIN_DB.1 - CUE_GAIN_DB.0) * v
}

/// Something the UI does for the controller (at its next frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UiAction {
    /// The LAYER of a side changed (side 0 = left, 1 = right; deck 0-3).
    Layer {
        side: usize,
        deck: usize,
    },
    PadMode {
        deck: usize,
        mode: PadMode,
    },
    /// A pad in a mode that needs the UI's state (saved loops, auto loop,
    /// pitch play).
    Pad {
        deck: usize,
        pad: usize,
        pressed: bool,
    },
    /// SHIFT + pad in saved loop mode: delete the saved loop.
    PadSecondary {
        deck: usize,
        pad: usize,
    },
    /// LOOP ON/OFF: leave the active loop, or start an auto loop.
    LoopToggle {
        deck: usize,
    },
    /// LOOP 1/2 (0.5) and LOOP x2 (2.0).
    ScaleLoop {
        deck: usize,
        factor: f32,
    },
    /// The browse knob moved this many clicks.
    Browse(i32),
    /// SHIFT + browse: a page of tracks per click.
    BrowsePage(i32),
    /// The browse knob was pressed: switch between tracks and crates.
    BrowseFocus,
    /// BACK (DDJ-FLX6): to the crate list.
    BrowseBack,
    /// LOAD: the selected track onto a deck (the controller sends a
    /// different note for each deck, depending on its LAYER).
    Load {
        deck: usize,
    },
    /// SHIFT + LOAD: eject the deck.
    Eject {
        deck: usize,
    },
    KeyShift {
        deck: usize,
        delta: i8,
    },
    ToggleQuantize,
    /// MIDI learn: this message came while learning (channel 1-16).
    Learned {
        note: bool,
        channel: u8,
        number: u8,
    },
    /// A learned button pressed or released (deck 0-3, if the function has
    /// one).
    LearnedPress {
        function: MidiFunction,
        deck: Option<usize>,
        pressed: bool,
    },
    /// SCRATCH: the jog mode of a deck (vinyl = touching scratches).
    SetVinyl {
        deck: usize,
        on: bool,
    },
}

/// MIDI learn bindings shared with the processor: a version (bumped on every
/// change) and the list.
pub(crate) type Learned = Arc<Mutex<(u64, Vec<LearnedBinding>)>>;

/// One result of a control event.
#[derive(Debug)]
pub enum Out {
    Engine(Command),
    Ui(UiAction),
}

/// UI state the controller needs, published by the UI every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shared {
    pub pad_mode: [PadMode; 4],
    /// Cue loop size per deck (0 when cue loops are off).
    pub cue_loop_beats: [f32; 4],
    pub slicer_domain: [f32; 4],
    pub sampler_bank: usize,
    pub sample_rate: u32,
    /// Jog mode per deck: vinyl (touching scratches) or nudge only.
    pub vinyl: [bool; 4],
    /// MIDI learn is waiting for a control.
    pub learning: bool,
    /// Jog wheel sensitivity (1.0: one turn moves one record turn).
    pub jog_sensitivity: f32,
    /// Soft takeover for knobs and the pitch fader.
    pub soft_takeover: bool,
    /// The chosen effect (0-5), as the engine plays it.
    pub fx_lit: Option<usize>,
    /// What the filter knobs play, as the engine has it.
    pub color_fx: dj_audio::ColorFx,
    /// A loop is playing, per deck (RELOOP/EXIT leaves it or starts the
    /// last one again).
    pub loop_active: [bool; 4],
}

impl Default for Shared {
    fn default() -> Self {
        Self {
            pad_mode: [PadMode::Cue; 4],
            cue_loop_beats: [0.0; 4],
            slicer_domain: [8.0; 4],
            sampler_bank: 0,
            sample_rate: 48_000,
            vinyl: [true; 4],
            learning: false,
            jog_sensitivity: 1.0,
            soft_takeover: true,
            fx_lit: None,
            color_fx: dj_audio::ColorFx::Filter,
            loop_active: [false; 4],
        }
    }
}

/// The NS4FX behaviour (see the module docs).
pub struct Ns4fx {
    mapper: Mapper,
    events: Vec<Event>,
    ticks_per_turn: f64,
    /// SHIFT held, per side.
    shift: [bool; 2],
    /// The deck each side controls (from LAYER, or the channel of the last
    /// deck message).
    active: [usize; 2],
    /// A jog mode chosen with SCRATCH that the UI has not reported back.
    vinyl: [Option<bool>; 4],
    /// The deck is held by the jog (scratching).
    scratching: [bool; 4],
    /// The jog was let go while scratching and may still be spinning: its
    /// ticks keep moving the deck (a backspin) until they pause.
    free_spin: [bool; 4],
    /// When the last jog tick came (seconds since start).
    last_tick: [f64; 4],
    /// The time of the message being handled (seconds since start).
    now: f64,
    start: std::time::Instant,
    /// FADER CUTS: the controller moves the crossfader itself, the pads do
    /// nothing in the software.
    fader_cuts: [bool; 4],
    /// FADER CUTS pads held per deck.
    cut_pads: [u8; 4],
    /// How far each jog has turned (revolutions), for the lights.
    jog_turns: [f64; 4],
    /// The crossfader where the hand left it (0-1).
    xfader_hand: f32,
    /// Bend buttons held: [down, up].
    bend: [[bool; 2]; 4],
    /// A pad mode chosen here that the UI has not reported back yet.
    mode: [Option<PadMode>; 4],
    /// The mode a held pad was pressed in (its release goes there too).
    held: [[Option<PadMode>; 8]; 4],
    /// FX TAP presses.
    tap: crate::fx_view::TapTempo,
    /// An effect chosen here (0-5 or none, see [`crate::fx_panel::BUTTONS`])
    /// that the UI has not reported back yet.
    fx_pick: Option<Option<usize>>,
    /// A color FX chosen here that the UI has not reported back yet.
    color_pick: Option<dj_audio::ColorFx>,
    /// Where WET/DRY is, per unit (it sends on both FX channels).
    fx_wet: [f32; 2],
    /// The message being handled is part of the state dump.
    pub(crate) in_dump: bool,
    /// Where each side's physical pitch fader is (-1..1), if known.
    side_pitch: [Option<f64>; 2],
    /// The DDJ-FLX6's KEY SHIFT pad mode is on, per deck (a mode of the
    /// controller only: the screen keeps its pad mode).
    pub(crate) key_shift_mode: [bool; 4],
}

/// What a DDJ-FLX6 pad control plays: a Kaldor pad mode, or the KEY SHIFT
/// pads (controller only). Every FLX6 pad mode sends its own notes, so the
/// pad itself says the mode (`pad_<mode>` and `pad_<mode>_shift` in the
/// profile).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PadTarget {
    Mode(PadMode),
    KeyShift,
}

/// The pad mode of a `mode_<name>` button or a `pad_<name>` pad (the
/// DDJ-FLX6's eight modes).
fn flx_target(name: &str) -> Option<PadTarget> {
    Some(match name {
        "hot_cue" => PadTarget::Mode(PadMode::Cue),
        "keyboard" => PadTarget::Mode(PadMode::PitchPlay),
        "pad_fx" | "fx" => PadTarget::Mode(PadMode::Roll),
        "key_shift" => PadTarget::KeyShift,
        "beat_jump" => PadTarget::Mode(PadMode::BeatJump),
        "beat_loop" => PadTarget::Mode(PadMode::AutoLoop),
        "sampler" => PadTarget::Mode(PadMode::Sampler),
        "sample_scratch" => PadTarget::Mode(PadMode::Slicer),
        _ => return None,
    })
}

/// Controller pitch fader value (0..1) to the deck pitch (-1..1). The
/// pitch probe shows the NS4FX
/// sends 0 at the TOP and 127 at the BOTTOM on every deck (the first
/// capture's note "127 = top" was wrong). The top is the slow end, so the
/// controller moves the same way as the on-screen fader (- at the top).
fn pitch_from_midi(x: f32) -> f64 {
    2.0 * f64::from(x) - 1.0
}

fn deck_index(u: Unit) -> Option<usize> {
    match u {
        Unit::Deck(d) | Unit::Strip(d) if (1..=4).contains(&d) => Some(usize::from(d - 1)),
        _ => None,
    }
}

impl Ns4fx {
    /// The FX section (see [`crate::fx_panel`]). The six
    /// buttons choose one effect (pressing the lit one deselects it); each
    /// paddle plays it on its side: the left one on decks 1 and 3 (unit 1),
    /// the right one on 2 and 4 (unit 2). WET/DRY is how much of it is
    /// heard (for HPF and LPF: how far the filter is closed), BEATS its
    /// timing, TAP the tempo. WET/DRY, BEATS and TAP send on both FX
    /// channels (one knob for both units); the buttons send on channel 9
    /// (1-3) or 10 (4-6).
    fn fx_event(&mut self, u: usize, name: &str, ev: Event, shared: &Shared, out: &mut Vec<Out>) {
        let shift = self.shift[0] || self.shift[1];
        let fx = |out: &mut Vec<Out>, c: FxCommand| out.push(Out::Engine(Command::Fx(c)));
        let send = |out: &mut Vec<Out>, cmds: Vec<FxCommand>| {
            out.extend(cmds.into_iter().map(|c| Out::Engine(Command::Fx(c))));
        };
        // The chosen effect: picked here and not yet seen back, else what
        // the engine plays (the FX panel may have changed it).
        let chosen = self.fx_pick.unwrap_or(shared.fx_lit);
        match (name, ev.value) {
            // The paddle: up = 1 (on), centre = 0, down = 2 (on while held;
            // it springs back to 0). Its unit plays on its own side's decks.
            ("fx_switch", Value::Switch(s)) => {
                let mut cmds = Vec::new();
                crate::fx_panel::side_commands(u, s != 0, &mut cmds);
                send(out, cmds);
            }
            ("fx_select", Value::Press(true)) => {
                // The button (0-5) from the FX channel (unit) and the note.
                // Without SHIFT the note is the button. With SHIFT held the
                // NS4FX sends other notes, some of them another button's
                // (FX capture,
                // steps 2-8). SHIFT + a button chooses the color FX on the
                // filter knobs (see `fx_panel::COLOR`).
                let note = usize::from(ev.index);
                let button = if shift {
                    match (u, note) {
                        (0, 0x01) => Some(0),
                        (0, 0x04) => Some(1),
                        (0, 0x02) => Some(2),
                        (1, 0x00) => Some(3),
                        (1, 0x03) => Some(4),
                        (1, 0x05) => Some(5),
                        _ => None,
                    }
                } else {
                    Some(note).filter(|k| *k < 6 && k / 3 == u)
                };
                let Some(k) = button else { return };
                if shift {
                    let now = self.color_pick.unwrap_or(shared.color_fx);
                    let c = crate::fx_panel::color_for(k, now);
                    self.color_pick = Some(c);
                    out.push(Out::Engine(Command::Mixer(MixerCommand::ColorFx(c))));
                    return;
                }
                let pick = if chosen == Some(k) { None } else { Some(k) };
                self.fx_pick = Some(pick);
                let mut cmds = Vec::new();
                for unit in 0..2 {
                    crate::fx_panel::pick_commands(unit, pick, self.fx_wet[unit], &mut cmds);
                }
                send(out, cmds);
            }
            ("fx_wet_dry", Value::Abs(v)) => {
                if let Some(w) = self.fx_wet.get_mut(u) {
                    *w = v.clamp(0.0, 1.0);
                    let mut cmds = Vec::new();
                    crate::fx_panel::wet_commands(u, chosen, *w, &mut cmds);
                    send(out, cmds);
                }
            }
            ("fx_beats", Value::Rel(r)) => {
                let r = r.clamp(-4, 4);
                fx(
                    out,
                    if shift {
                        FxCommand::DepthStep(u, r as f32 * 0.05)
                    } else {
                        FxCommand::BeatsStep(u, r)
                    },
                );
            }
            // TAP comes on both channels: unit 1's press counts.
            ("fx_tap", Value::Press(true)) if u == 0 => {
                if shift {
                    fx(out, FxCommand::Tap(None));
                } else if let Some(bpm) = self.tap.tap(self.now) {
                    fx(out, FxCommand::Tap(Some(bpm)));
                }
            }

            // ----- DDJ-FLX6 Beat FX: one effect at a time on FX unit 1 -----
            // BFX SELECT steps through six slots (FX1-1..FX1-3 on channel
            // 5, FX2-1..FX2-3 on 6): the slot is one of the six effects
            // (`fx_panel::BUTTONS`), as the NS4FX's six buttons.
            ("beatfx_select", Value::Press(true)) => {
                let k = u * 3 + usize::from(ev.index.min(2));
                self.beatfx_pick(k, out);
            }
            // BFX CH SELECT: where the effect plays (one channel or the
            // master; the switch sends 0x7F for the position it is at).
            ("beatfx_channel" | "beatfx_master", Value::Press(true)) => {
                let target = if name == "beatfx_master" {
                    dj_audio::MASTER_TARGET
                } else {
                    usize::from(ev.index.min(3))
                };
                for t in 0..dj_audio::FX_TARGETS {
                    fx(out, FxCommand::Assign(0, t, t == target));
                }
            }
            ("beatfx_beats_down", Value::Press(true)) => fx(out, FxCommand::BeatsStep(0, -1)),
            ("beatfx_beats_up", Value::Press(true)) => fx(out, FxCommand::BeatsStep(0, 1)),
            ("beatfx_level", Value::Abs(v)) => {
                self.fx_wet[0] = v.clamp(0.0, 1.0);
                let mut cmds = Vec::new();
                crate::fx_panel::wet_commands(0, chosen, self.fx_wet[0], &mut cmds);
                send(out, cmds);
            }
            // ON/OFF sends the note of the chosen slot: that effect, on or
            // off. SHIFT + ON/OFF: off.
            ("beatfx_on", Value::Press(true)) => {
                let k = u * 3 + usize::from(ev.index.min(2));
                if chosen != Some(k) {
                    self.beatfx_pick(k, out);
                }
                fx(out, FxCommand::ToggleUnit(0));
            }
            ("beatfx_off", Value::Press(true)) => fx(out, FxCommand::UnitOn(0, false)),
            _ => {}
        }
    }

    /// The DDJ-FLX6's Beat FX chose effect `k` (0-5): it goes on FX unit 1.
    fn beatfx_pick(&mut self, k: usize, out: &mut Vec<Out>) {
        self.fx_pick = Some(Some(k));
        let mut cmds = Vec::new();
        crate::fx_panel::pick_commands(0, Some(k), self.fx_wet[0], &mut cmds);
        out.extend(cmds.into_iter().map(|c| Out::Engine(Command::Fx(c))));
    }

    pub fn new(profile: Profile) -> Self {
        let ticks_per_turn = profile.jog_ticks_per_turn;
        Self {
            mapper: Mapper::new(profile),
            events: Vec::with_capacity(8),
            ticks_per_turn,
            shift: [false; 2],
            active: [0, 1],
            vinyl: [None; 4],
            scratching: [false; 4],
            free_spin: [false; 4],
            last_tick: [f64::NEG_INFINITY; 4],
            now: 0.0,
            start: std::time::Instant::now(),
            fader_cuts: [false; 4],
            cut_pads: [0; 4],
            jog_turns: [0.0; 4],
            xfader_hand: 0.5,
            bend: [[false; 2]; 4],
            mode: [None; 4],
            held: [[None; 8]; 4],
            tap: crate::fx_view::TapTempo::default(),
            fx_pick: None,
            color_pick: None,
            fx_wet: [0.5; 2],
            in_dump: false,
            side_pitch: [None; 2],
            key_shift_mode: [false; 4],
        }
    }

    /// The deck each side controls.
    pub fn active(&self) -> [usize; 2] {
        self.active
    }

    /// Handles one MIDI message; results go to `out`.
    pub fn handle(&mut self, bytes: &[u8], shared: &Shared, out: &mut Vec<Out>) {
        let now = self.start.elapsed().as_secs_f64();
        self.handle_at(bytes, shared, now, out);
    }

    /// Like [`Self::handle`], at a given time (seconds; tests).
    pub fn handle_at(&mut self, bytes: &[u8], shared: &Shared, now: f64, out: &mut Vec<Out>) {
        self.now = now;
        // The UI took over a pad mode chosen here: it rules from now on.
        for d in 0..4 {
            if self.mode[d] == Some(shared.pad_mode[d]) {
                self.mode[d] = None;
            }
            if self.vinyl[d] == Some(shared.vinyl[d]) {
                self.vinyl[d] = None;
            }
        }
        if self.fx_pick == Some(shared.fx_lit) {
            self.fx_pick = None;
        }
        if self.color_pick == Some(shared.color_fx) {
            self.color_pick = None;
        }
        self.events.clear();
        self.mapper.set_pickup(shared.soft_takeover);
        self.mapper.feed(bytes, &mut self.events);
        for i in 0..self.events.len() {
            let ev = self.events[i];
            self.event(ev, shared, out);
        }
    }

    fn event(&mut self, ev: Event, shared: &Shared, out: &mut Vec<Out>) {
        let name = self.mapper.name(ev.control).to_string();
        let pressed = matches!(ev.value, Value::Press(true));
        let deck = deck_index(ev.unit);
        // A message from the other deck of a side means the LAYER changed
        // (also when the app started after the LAYER was pressed).
        if let (Unit::Deck(_), Some(d)) = (ev.unit, deck)
            && self.active[d % 2] != d
            && (name != "layer" || pressed)
        {
            self.active[d % 2] = d;
            out.push(Out::Ui(UiAction::Layer {
                side: d % 2,
                deck: d,
            }));
            // The side's pitch fader now belongs to this deck, where the
            // other deck left it: the deck waits until the fader crosses its
            // pitch, and the screen shows the fader at once (Serato).
            out.push(Out::Engine(Command::Deck(
                DeckId::ALL[d],
                DeckCommand::PitchTakeover(self.side_pitch[d % 2]),
            )));
        }
        let eng = |out: &mut Vec<Out>, d: usize, c: DeckCommand| {
            out.push(Out::Engine(Command::Deck(DeckId::ALL[d], c)));
        };
        let mix = |out: &mut Vec<Out>, c: MixerCommand| out.push(Out::Engine(Command::Mixer(c)));
        match (ev.unit, name.as_str(), ev.value) {
            // ----- browser, load, master -----
            (Unit::Global, "browse", Value::Rel(n)) => out.push(Out::Ui(UiAction::Browse(n))),
            (Unit::Global, "browse_shift", Value::Rel(n)) => {
                out.push(Out::Ui(UiAction::BrowsePage(n)));
            }
            (Unit::Global, "browse_push" | "browse_shift_push", Value::Press(true)) => {
                out.push(Out::Ui(UiAction::BrowseFocus));
            }
            (Unit::Global, "load", Value::Press(true)) => {
                // Index = deck (0x02-0x05: decks 1-4).
                let deck = usize::from(ev.index.min(3));
                out.push(Out::Ui(if self.shift[0] || self.shift[1] {
                    UiAction::Eject { deck }
                } else {
                    UiAction::Load { deck }
                }));
            }
            (Unit::Global, "crossfader", Value::Abs(v)) => {
                // While a left deck's FADER CUTS pad is held, the controller
                // moves the crossfader itself; otherwise this is the hand.
                if !(self.cut_pads[0] > 0 || self.cut_pads[2] > 0) {
                    self.xfader_hand = v;
                }
                mix(out, MixerCommand::Crossfader(v * 2.0 - 1.0));
            }
            (Unit::Global, "cue_gain", Value::Abs(v)) => {
                let (lo, hi) = CUE_GAIN_DB;
                mix(out, MixerCommand::CueGain(lo + (hi - lo) * v));
            }
            (Unit::Global, "cue_mix", Value::Abs(v)) => mix(out, MixerCommand::CueMix(v)),
            // DDJ-FLX6: BACK goes to the crate list, SHIFT + LOAD has its
            // own note per deck, and the master and booth levels are in the
            // software (as the on-screen MASTER and BOOTH knobs).
            (Unit::Global, "browse_back", Value::Press(true)) => {
                out.push(Out::Ui(UiAction::BrowseBack));
            }
            (Unit::Global, "eject", Value::Press(true)) => {
                out.push(Out::Ui(UiAction::Eject {
                    deck: usize::from(ev.index.min(3)),
                }));
            }
            (Unit::Global, "master_level", Value::Abs(v)) => {
                mix(
                    out,
                    MixerCommand::MasterGain(crate::mixer_view::master_db(v)),
                );
            }
            (Unit::Global, "booth_level", Value::Abs(v)) => {
                mix(out, MixerCommand::BoothGain(db(v)));
            }

            // ----- FX section (channel 9 = unit 1, channel 10 = unit 2) -----
            (Unit::Fx(n), _, _) => self.fx_event(
                usize::from(n.saturating_sub(1)).min(1),
                &name,
                ev,
                shared,
                out,
            ),

            // ----- mixer channel strips -----
            (Unit::Strip(_), n, v) => {
                let Some(d) = deck else { return };
                let id = DeckId::ALL[d];
                match (n, v) {
                    ("level", Value::Abs(v)) => mix(out, MixerCommand::Trim(id, v * 2.0 - 1.0)),
                    ("eq_high", Value::Abs(v)) => mix(out, MixerCommand::Eq(id, EqBand::High, v)),
                    ("eq_mid", Value::Abs(v)) => mix(out, MixerCommand::Eq(id, EqBand::Mid, v)),
                    ("eq_low", Value::Abs(v)) => mix(out, MixerCommand::Eq(id, EqBand::Low, v)),
                    ("filter", Value::Abs(v)) => {
                        mix(out, MixerCommand::Filter(id, dj_midi::centred(v)))
                    }
                    ("channel_fader", Value::Abs(v)) => mix(out, MixerCommand::Fader(id, v)),
                    ("headphone_cue", Value::Press(true)) => mix(out, MixerCommand::TogglePfl(id)),
                    ("xfader_assign", Value::Switch(s)) => {
                        // Capture steps 17-41: 1 = left, 0 = middle, 2 = right.
                        let side = match s {
                            1 => XfaderSide::A,
                            2 => XfaderSide::B,
                            _ => XfaderSide::Thru,
                        };
                        mix(out, MixerCommand::XfaderAssign(id, side));
                    }
                    _ => {}
                }
            }

            // ----- SHIFT (per side) -----
            (Unit::Side(s), "shift", Value::Press(p)) => {
                if let Some(x) = self.shift.get_mut(usize::from(s.saturating_sub(1))) {
                    *x = p;
                }
            }

            // ----- decks -----
            (Unit::Deck(_), n, v) => {
                let Some(d) = deck else { return };
                self.deck_event(d, n, ev.index, v, shared, out, &eng);
            }

            // Controls nothing here uses.
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn deck_event(
        &mut self,
        d: usize,
        name: &str,
        index: u8,
        v: Value,
        shared: &Shared,
        out: &mut Vec<Out>,
        eng: &dyn Fn(&mut Vec<Out>, usize, DeckCommand),
    ) {
        let pressed = matches!(v, Value::Press(true));
        let side = d % 2;
        let ui = |out: &mut Vec<Out>, a: UiAction| out.push(Out::Ui(a));
        match (name, v) {
            ("layer", _) => {} // handled with the other deck messages
            ("play", Value::Press(true)) => eng(out, d, DeckCommand::TogglePlay),
            ("play_shift", Value::Press(true)) => eng(out, d, DeckCommand::ToggleSlip),
            ("cue", Value::Press(p)) => eng(out, d, DeckCommand::Cue(p)),
            ("cue_shift", Value::Press(true)) => {
                // Back to the start, stopped.
                eng(out, d, DeckCommand::Play(false));
                eng(out, d, DeckCommand::Seek(0.0));
            }
            ("sync", Value::Press(true)) => {
                out.push(Out::Engine(Command::ToggleSync(DeckId::ALL[d])));
            }
            ("sync_shift", Value::Press(true)) => ui(out, UiAction::ToggleQuantize),
            ("pitch", Value::Abs(x)) => {
                // Top (0) is slower, as on screen (see `pitch_from_midi`).
                // Soft takeover in the deck (Serato); the state dump at
                // connect is taken at once.
                let p = pitch_from_midi(x);
                self.side_pitch[d % 2] = Some(p);
                if self.in_dump || !shared.soft_takeover {
                    eng(out, d, DeckCommand::PitchFaderSync(p));
                } else {
                    eng(out, d, DeckCommand::PitchFader(p));
                }
            }
            ("bend_down" | "bend_up", Value::Press(p)) => {
                let b = usize::from(name == "bend_up");
                let other = self.bend[d][1 - b];
                self.bend[d][b] = p;
                if p && other {
                    // Both bend buttons: key lock on/off.
                    eng(out, d, DeckCommand::ToggleKeyLock);
                }
                let amount = (i8::from(self.bend[d][1]) - i8::from(self.bend[d][0])) as f64;
                eng(out, d, DeckCommand::PitchBend(amount * BEND));
            }
            // SHIFT + bend -: the pitch range; SHIFT + bend +: key lock.
            ("bend_down_shift", Value::Press(true)) => eng(out, d, DeckCommand::CyclePitchRange),
            ("bend_up_shift", Value::Press(true)) => eng(out, d, DeckCommand::ToggleKeyLock),
            ("scratch", Value::Press(true)) => {
                let on = !self.vinyl[d].unwrap_or(shared.vinyl[d]);
                self.vinyl[d] = Some(on);
                ui(out, UiAction::SetVinyl { deck: d, on });
                if !on && self.scratching[d] {
                    self.scratching[d] = false;
                    eng(out, d, DeckCommand::JogTouch(false));
                }
            }
            ("jog_touch", Value::Press(p)) => {
                self.free_spin[d] = false;
                if p && self.vinyl[d].unwrap_or(shared.vinyl[d]) && !self.shift[side] {
                    self.scratching[d] = true;
                    eng(out, d, DeckCommand::JogTouch(true));
                } else if !p && self.scratching[d] {
                    self.scratching[d] = false;
                    // The wheel may spin on (the engine decides whether it
                    // still follows it).
                    self.free_spin[d] = true;
                    eng(out, d, DeckCommand::JogTouch(false));
                }
            }
            ("jog", Value::Rel(n)) => {
                let turns = f64::from(n) / self.ticks_per_turn
                    * f64::from(shared.jog_sensitivity.clamp(0.25, 4.0));
                // For the platter dot of a deck without a track.
                self.jog_turns[d] += turns;
                let gap = self.now - self.last_tick[d];
                self.last_tick[d] = self.now;
                if self.free_spin[d] && gap > FREE_SPIN_GAP_SECONDS {
                    self.free_spin[d] = false;
                }
                if self.shift[side] {
                    let frames = turns * SEARCH_SECONDS_PER_TURN * f64::from(shared.sample_rate);
                    eng(out, d, DeckCommand::SeekBy(frames));
                } else if self.scratching[d] || self.free_spin[d] {
                    eng(out, d, DeckCommand::JogScratch(turns));
                } else {
                    eng(out, d, DeckCommand::JogNudge(turns));
                }
            }
            ("loop_toggle", Value::Press(true)) => ui(out, UiAction::LoopToggle { deck: d }),
            ("loop_toggle_shift", Value::Press(true)) => eng(out, d, DeckCommand::Reloop),
            ("loop_halve", Value::Press(true)) => ui(
                out,
                UiAction::ScaleLoop {
                    deck: d,
                    factor: 0.5,
                },
            ),
            ("loop_double", Value::Press(true)) => ui(
                out,
                UiAction::ScaleLoop {
                    deck: d,
                    factor: 2.0,
                },
            ),
            ("loop_in", Value::Press(true)) => eng(out, d, DeckCommand::LoopIn),
            ("loop_out", Value::Press(true)) => eng(out, d, DeckCommand::LoopOut),
            ("fader_cut", Value::Abs(x)) => {
                // The controller's fader cut on a right deck: 127 = the deck
                // cut, which for crossfader side B is the crossfader fully
                // left (mirrored). On a left deck it is the crossfader as is.
                let pos = if side == 1 { 1.0 - x } else { x };
                out.push(Out::Engine(Command::Mixer(MixerCommand::Crossfader(
                    pos * 2.0 - 1.0,
                ))));
            }
            (mode_button, Value::Press(true)) if mode_button.starts_with("mode_") => {
                self.fader_cuts[d] = mode_button == "mode_fader_cuts";
                if !self.fader_cuts[d] && std::mem::take(&mut self.cut_pads[d]) > 0 && side == 1 {
                    // Left FADER CUTS with a pad still down: crossfader back
                    // to the hand.
                    let x = self.xfader_hand * 2.0 - 1.0;
                    out.push(Out::Engine(Command::Mixer(MixerCommand::Crossfader(x))));
                }
                let mode = match mode_button {
                    "mode_cues" => PadMode::Cue,
                    "mode_cues_shift" => PadMode::SavedLoop,
                    "mode_auto_loop" => PadMode::AutoLoop,
                    "mode_auto_loop_shift" => PadMode::Roll,
                    "mode_sample" => PadMode::Sampler,
                    "mode_sample_shift" => PadMode::PitchPlay,
                    "mode_fader_cuts_shift" => PadMode::Slicer,
                    // The DDJ-FLX6's mode buttons.
                    other => match flx_target(&other["mode_".len()..]) {
                        Some(PadTarget::Mode(m)) => m,
                        Some(PadTarget::KeyShift) => {
                            self.release_pads(d, shared, out);
                            self.key_shift_mode[d] = true;
                            return;
                        }
                        None => return, // FADER CUTS: the controller's own mode
                    },
                };
                self.set_mode(d, mode, shared, out);
            }
            ("pad", Value::Press(p)) => self.pad(d, usize::from(index), p, shared, out),
            ("pad_shift", Value::Press(true)) => {
                if self.fader_cuts[d] {
                    return;
                }
                self.pad_shift(d, usize::from(index), shared, out);
            }

            // ----- DDJ-FLX6 -----
            // SHIFT comes on the deck's channel (one per side).
            ("shift", Value::Press(p)) => self.shift[side] = p,
            ("master", Value::Press(true)) => {
                out.push(Out::Engine(Command::SyncMaster(DeckId::ALL[d])));
            }
            ("master_shift", Value::Press(true)) => eng(out, d, DeckCommand::CyclePitchRange),
            // SHIFT + touch: the platter searches, it does not scratch.
            ("jog_touch_shift", _) => {}
            ("jog_shift" | "jog_side", Value::Rel(n)) => {
                let turns = f64::from(n) / self.ticks_per_turn
                    * f64::from(shared.jog_sensitivity.clamp(0.25, 4.0));
                self.jog_turns[d] += turns;
                if name == "jog_shift" || self.shift[side] {
                    let frames = turns * SEARCH_SECONDS_PER_TURN * f64::from(shared.sample_rate);
                    eng(out, d, DeckCommand::SeekBy(frames));
                } else {
                    // The wheel's side only bends, as on a turntable's rim.
                    eng(out, d, DeckCommand::JogNudge(turns));
                }
            }
            // IN held: a 4-beat loop (the controller sends this after IN).
            ("loop_4_beats", Value::Press(true)) => eng(out, d, DeckCommand::AutoLoop(4.0)),
            ("reloop_exit", Value::Press(true)) => eng(
                out,
                d,
                if shared.loop_active[d] {
                    DeckCommand::LoopExit
                } else {
                    DeckCommand::Reloop
                },
            ),
            ("beat_jump_back", Value::Press(true)) => {
                eng(out, d, DeckCommand::BeatJump(-FLX_JUMP_BEATS));
            }
            ("beat_jump_forward", Value::Press(true)) => {
                eng(out, d, DeckCommand::BeatJump(FLX_JUMP_BEATS));
            }
            (pad_name, Value::Press(p)) if pad_name.starts_with("pad_") => {
                // `pad_<mode>` or `pad_<mode>_shift` (the mode name is tried
                // whole first: "key_shift" is a mode).
                let rest = &pad_name["pad_".len()..];
                let found = match flx_target(rest) {
                    Some(t) => Some((t, false)),
                    None => rest
                        .strip_suffix("_shift")
                        .and_then(flx_target)
                        .map(|t| (t, true)),
                };
                if let Some((t, with_shift)) = found {
                    self.flx_pad(d, t, with_shift, usize::from(index), p, shared, out);
                }
            }
            _ => {}
        }
        let _ = pressed;
    }

    /// SHIFT + pad: delete a hot cue, stop a sample, delete a saved loop.
    fn pad_shift(&mut self, d: usize, k: usize, shared: &Shared, out: &mut Vec<Out>) {
        match self.mode_of(d, shared) {
            PadMode::Cue | PadMode::PitchPlay => out.push(Out::Engine(Command::Deck(
                DeckId::ALL[d],
                DeckCommand::SetHotCue {
                    slot: k as u8,
                    position: None,
                },
            ))),
            PadMode::Sampler => {
                let slot = (shared.sampler_bank * 8 + k) as u8;
                out.push(Out::Engine(Command::Sampler(SamplerCommand::Stop(slot))));
            }
            PadMode::SavedLoop => out.push(Out::Ui(UiAction::PadSecondary { deck: d, pad: k })),
            _ => {}
        }
    }

    /// A DDJ-FLX6 pad: its note says the mode, so the deck follows it if
    /// the screen shows another mode (also when the controller was already
    /// in that mode when the app started).
    #[allow(clippy::too_many_arguments)]
    fn flx_pad(
        &mut self,
        d: usize,
        target: PadTarget,
        with_shift: bool,
        k: usize,
        pressed: bool,
        shared: &Shared,
        out: &mut Vec<Out>,
    ) {
        if k >= 8 {
            return;
        }
        match target {
            PadTarget::KeyShift => {
                self.key_shift_mode[d] = true;
                if pressed {
                    // SHIFT + pad: back to the track's own key.
                    let semis = if with_shift {
                        0
                    } else {
                        crate::perform::KEY_SHIFT_PADS[k]
                    };
                    out.push(Out::Engine(Command::Deck(
                        DeckId::ALL[d],
                        DeckCommand::KeyShift(semis),
                    )));
                }
            }
            PadTarget::Mode(m) => {
                if pressed && (self.mode_of(d, shared) != m || self.key_shift_mode[d]) {
                    self.set_mode(d, m, shared, out);
                }
                if !with_shift {
                    self.pad(d, k, pressed, shared, out);
                } else if pressed {
                    self.pad_shift(d, k, shared, out);
                }
            }
        }
    }

    /// The deck's pad mode, chosen on the controller.
    fn set_mode(&mut self, d: usize, mode: PadMode, shared: &Shared, out: &mut Vec<Out>) {
        self.release_pads(d, shared, out);
        self.key_shift_mode[d] = false;
        self.mode[d] = Some(mode);
        out.push(Out::Ui(UiAction::PadMode { deck: d, mode }));
    }

    fn mode_of(&self, d: usize, shared: &Shared) -> PadMode {
        self.mode[d].unwrap_or(shared.pad_mode[d])
    }

    fn pad(&mut self, d: usize, k: usize, pressed: bool, shared: &Shared, out: &mut Vec<Out>) {
        if k >= 8 {
            return;
        }
        // FADER CUTS: the controller cuts by itself; count the held pads so
        // the crossfader goes back to the hand's position afterwards (the
        // right deck's cuts end wherever the last dip was).
        if self.fader_cuts[d] {
            if pressed {
                self.cut_pads[d] += 1;
            } else if self.cut_pads[d] > 0 {
                self.cut_pads[d] -= 1;
                if self.cut_pads[d] == 0 && d % 2 == 1 {
                    let x = self.xfader_hand * 2.0 - 1.0;
                    out.push(Out::Engine(Command::Mixer(MixerCommand::Crossfader(x))));
                }
            }
            return;
        }
        // A release goes to the mode the pad was pressed in.
        let mode = if pressed {
            let m = self.mode_of(d, shared);
            self.held[d][k] = Some(m);
            m
        } else {
            match self.held[d][k].take() {
                Some(m) => m,
                None => return,
            }
        };
        let id = DeckId::ALL[d];
        let cmd = match mode {
            PadMode::Cue => Command::Deck(
                id,
                DeckCommand::HotCue {
                    slot: k as u8,
                    pressed,
                    loop_beats: shared.cue_loop_beats[d],
                },
            ),
            PadMode::Roll => Command::Deck(
                id,
                DeckCommand::Roll {
                    beats: ROLL_PADS[k],
                    pressed,
                },
            ),
            PadMode::Sampler => Command::Sampler(SamplerCommand::Trigger {
                slot: (shared.sampler_bank * 8 + k) as u8,
                pressed,
            }),
            PadMode::Slicer => Command::Deck(
                id,
                DeckCommand::Slicer {
                    slice: k as u8,
                    pressed,
                    domain_beats: shared.slicer_domain[d],
                },
            ),
            PadMode::SavedLoop | PadMode::AutoLoop | PadMode::PitchPlay => {
                out.push(Out::Ui(UiAction::Pad {
                    deck: d,
                    pad: k,
                    pressed,
                }));
                return;
            }
            PadMode::BeatJump => {
                if !pressed {
                    return;
                }
                Command::Deck(id, DeckCommand::BeatJump(crate::perform::BEAT_JUMP_PADS[k]))
            }
            PadMode::Stems => return,
        };
        out.push(Out::Engine(cmd));
    }

    /// Releases the pads of deck `d` that are held (before a mode change).
    fn release_pads(&mut self, d: usize, shared: &Shared, out: &mut Vec<Out>) {
        for k in 0..8 {
            if self.held[d][k].is_some() {
                self.pad(d, k, false, shared, out);
            }
        }
    }
}

/// Feeds MIDI bytes through [`Ns4fx`] and sends the results: engine
/// commands into the controller queue, UI actions to the UI. Runs on the
/// MIDI driver's thread (or in a test).
pub struct Processor {
    ns4fx: Ns4fx,
    out: Vec<Out>,
    ui_tx: mpsc::Sender<UiAction>,
    shared: Arc<Mutex<Shared>>,
    queue: Arc<Mutex<Option<ControllerQueue>>>,
    unknown: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// MIDI learn bindings (version, list) from the UI, and their lookup.
    learned: Learned,
    learned_version: u64,
    learned_map: std::collections::HashMap<(bool, u8, u8), LearnedBinding>,
    /// The state dump asked for at connect is arriving until then.
    dump_until: Option<std::time::Instant>,
    /// The answer to the last state request arrived (a knob or switch
    /// message inside the window).
    dump_seen: bool,
}

impl Processor {
    /// Another controller was plugged in: its profile decodes from now on
    /// (the controller state starts over).
    pub fn set_profile(&mut self, profile: Profile) {
        self.ns4fx = Ns4fx::new(profile);
    }

    /// The profile in use.
    pub fn profile(&self) -> &Profile {
        self.ns4fx.mapper.profile()
    }

    /// MIDI learn. While learning, the next press or move is reported to the
    /// UI (and does nothing else); SHIFT alone is not learned, so SHIFT
    /// combinations can be. Otherwise a learned message runs its function
    /// instead of what the profile does. True when the message was used.
    fn learned(&mut self, bytes: &[u8], shared: &Shared) -> bool {
        let (note, channel, number, value) = match dj_midi::MidiMessage::parse(bytes) {
            dj_midi::MidiMessage::NoteOn {
                channel,
                note,
                velocity,
            } => (true, channel, note, velocity),
            dj_midi::MidiMessage::NoteOff { channel, note, .. } => (true, channel, note, 0),
            dj_midi::MidiMessage::ControlChange {
                channel,
                control,
                value,
            } => (false, channel, control, value),
            _ => return false,
        };
        if shared.learning {
            if self.ns4fx.mapper.control_name(bytes) == Some("shift") {
                return false;
            }
            if !(note && value == 0) {
                self.out.push(Out::Ui(UiAction::Learned {
                    note,
                    channel: channel + 1,
                    number,
                }));
            }
            return true;
        }
        if let Ok(l) = self.learned.lock()
            && l.0 != self.learned_version
        {
            self.learned_version = l.0;
            self.learned_map =
                l.1.iter()
                    .map(|b| ((b.note, b.channel, b.number), *b))
                    .collect();
        }
        let Some(b) = self.learned_map.get(&(note, channel + 1, number)).copied() else {
            return false;
        };
        let deck = b.deck.map(|d| usize::from(d.clamp(1, 4) - 1));
        let id = DeckId::ALL[deck.unwrap_or(0)];
        let v = f32::from(value) / 127.0;
        let mix = |c: MixerCommand| Out::Engine(Command::Mixer(c));
        let o = match b.function.kind() {
            FunctionKind::Continuous => match b.function {
                MidiFunction::Volume => mix(MixerCommand::Fader(id, v)),
                MidiFunction::Trim => mix(MixerCommand::Trim(id, v * 2.0 - 1.0)),
                MidiFunction::EqHigh => mix(MixerCommand::Eq(id, EqBand::High, v)),
                MidiFunction::EqMid => mix(MixerCommand::Eq(id, EqBand::Mid, v)),
                MidiFunction::EqLow => mix(MixerCommand::Eq(id, EqBand::Low, v)),
                MidiFunction::Filter => mix(MixerCommand::Filter(id, dj_midi::centred(v))),
                // As the NS4FX: 0 (top) is slower.
                MidiFunction::Pitch => Out::Engine(Command::Deck(
                    id,
                    DeckCommand::PitchFader(pitch_from_midi(v)),
                )),
                MidiFunction::Crossfader => mix(MixerCommand::Crossfader(v * 2.0 - 1.0)),
                MidiFunction::CueGain => mix(MixerCommand::CueGain(db(v))),
                MidiFunction::CueMix => mix(MixerCommand::CueMix(v)),
                // The master scale of the on-screen knobs: 12 o'clock is
                // MASTER_CENTRE_DB.
                MidiFunction::MasterGain => {
                    mix(MixerCommand::MasterGain(crate::mixer_view::master_db(v)))
                }
                MidiFunction::BoothGain => mix(MixerCommand::BoothGain(db(v))),
                MidiFunction::SamplerVolume => {
                    Out::Engine(Command::Sampler(SamplerCommand::Volume(v)))
                }
                _ => return true,
            },
            FunctionKind::Encoder => {
                // Two's complement: 1 = one step on, 127 = one step back.
                let n = if value < 64 {
                    i32::from(value)
                } else {
                    i32::from(value) - 128
                };
                Out::Ui(UiAction::Browse(n))
            }
            FunctionKind::Button => {
                let pressed = value > 0;
                match b.function {
                    MidiFunction::SamplerSlot(n) => {
                        Out::Engine(Command::Sampler(SamplerCommand::Trigger {
                            slot: n.clamp(1, 32) - 1,
                            pressed,
                        }))
                    }
                    MidiFunction::LoadSelected if pressed => Out::Ui(UiAction::Load {
                        deck: deck.unwrap_or(0),
                    }),
                    MidiFunction::LoadSelected => return true,
                    f => Out::Ui(UiAction::LearnedPress {
                        function: f,
                        deck,
                        pressed,
                    }),
                }
            }
        };
        self.out.push(o);
        true
    }

    /// The state request is about to be sent: for the next second the
    /// controller's answer is applied as positions (pitch faders taken at
    /// once) and its button releases are ignored (a release can mean
    /// something, for example CUE).
    pub fn begin_state_dump(&mut self) {
        self.dump_until = Some(std::time::Instant::now() + STATE_DUMP_WINDOW);
        self.dump_seen = false;
    }

    /// The controller answered the last state request.
    pub fn dump_seen(&self) -> bool {
        self.dump_seen
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        let in_dump = self
            .dump_until
            .is_some_and(|t| std::time::Instant::now() < t);
        if !in_dump {
            self.dump_until = None;
        } else if matches!(
            dj_midi::MidiMessage::parse(bytes),
            dj_midi::MidiMessage::NoteOn { .. } | dj_midi::MidiMessage::NoteOff { .. }
        ) {
            return;
        }
        self.ns4fx.in_dump = in_dump;
        if in_dump {
            self.dump_seen = true;
        }
        // Messages the profile does not know are kept (the last few) for
        // the Setup tab, so a control that sends something unexpected can
        // be found without guessing.
        if !self.ns4fx.mapper.knows(bytes)
            && let Ok(mut u) = self.unknown.lock()
        {
            if u.len() >= UNKNOWN_KEPT {
                u.pop_front();
            }
            u.push_back(dj_midi::MidiMessage::parse(bytes).to_string());
        }
        let shared = self.shared.lock().map(|s| *s).unwrap_or_default();
        self.out.clear();
        if !self.learned(bytes, &shared) {
            self.ns4fx.handle(bytes, &shared, &mut self.out);
        }
        for o in self.out.drain(..) {
            match o {
                Out::Engine(c) => {
                    if let Ok(mut q) = self.queue.lock()
                        && let Some(q) = q.as_mut()
                    {
                        let _ = q.send(c);
                    }
                }
                Out::Ui(a) => {
                    let _ = self.ui_tx.send(a);
                }
            }
        }
    }
}

/// The UI's end of the controller: actions to apply, the state to publish,
/// where the engine's controller queue goes, and the connection status.
pub struct ControllerLink {
    pub(crate) ui_rx: mpsc::Receiver<UiAction>,
    pub(crate) shared: Arc<Mutex<Shared>>,
    pub(crate) queue: Arc<Mutex<Option<ControllerQueue>>>,
    pub(crate) status: Arc<Mutex<String>>,
    unknown: Arc<Mutex<std::collections::VecDeque<String>>>,
    /// MIDI learn bindings for the processor (version, list).
    pub(crate) learned: Learned,
    stop: Arc<AtomicBool>,
    /// Ask the controller for its positions again (a new audio engine
    /// replaced the one the last answer went to).
    resync: Arc<AtomicBool>,
    /// What the lights and displays should show (from the UI).
    pub(crate) feedback: Arc<Mutex<crate::controller_out::Feedback>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Drop for ControllerLink {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Let the thread hand the controller back (its goodbye message).
        if let Some(h) = self.thread.take() {
            let _ = h.join();
        }
    }
}

impl ControllerLink {
    /// Asks the controller for the position of every control again, for
    /// example after the audio engine was replaced (plugging the
    /// NS4FX back in brings back its audio output, and the new engine
    /// started from the old settings).
    pub(crate) fn request_resync(&self) {
        self.resync.store(true, Ordering::Relaxed);
    }

    /// A link and its processor, without any MIDI port (tests feed the
    /// processor directly).
    pub fn offline(profile: Profile) -> (Self, Processor) {
        let (ui_tx, ui_rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let queue = Arc::new(Mutex::new(None));
        let unknown = Arc::new(Mutex::new(std::collections::VecDeque::new()));
        let learned = Arc::new(Mutex::new((0u64, Vec::new())));
        let link = Self {
            ui_rx,
            shared: Arc::clone(&shared),
            queue: Arc::clone(&queue),
            status: Arc::new(Mutex::new(String::new())),
            unknown: Arc::clone(&unknown),
            learned: Arc::clone(&learned),
            stop: Arc::new(AtomicBool::new(false)),
            resync: Arc::new(AtomicBool::new(false)),
            feedback: Arc::new(Mutex::new(crate::controller_out::Feedback::default())),
            thread: None,
        };
        let p = Processor {
            unknown,
            learned: Arc::clone(&learned),
            learned_version: u64::MAX,
            learned_map: std::collections::HashMap::new(),
            dump_until: None,
            dump_seen: false,
            ns4fx: Ns4fx::new(profile),
            out: Vec::with_capacity(16),
            ui_tx,
            shared,
            queue,
        };
        (link, p)
    }

    /// Opens the controller's MIDI port when it is there, and again after
    /// it was unplugged and plugged back in (checked every second). Stops
    /// when the link is dropped.
    pub fn connect(profile: Profile) -> Self {
        Self::connect_any(vec![profile])
    }

    /// Like [`Self::connect`] for several controllers: the first one whose
    /// MIDI port is there is used (Settings > Controller > Automatic).
    pub fn connect_any(profiles: Vec<Profile>) -> Self {
        let Some(first) = profiles.first().cloned() else {
            return Self::offline(Profile::ns4fx()).0;
        };
        let mut port = first.port.clone();
        let mut name = first.name.clone();
        let mut lights = first.lights;
        let wanted = profiles
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>()
            .join(" or ");
        let (mut link, processor) = Self::offline(first);
        let processor = Arc::new(Mutex::new(processor));
        let status = Arc::clone(&link.status);
        let stop = Arc::clone(&link.stop);
        let resync = Arc::clone(&link.resync);
        let set = move |s: String| {
            tracing::info!("controller: {s}");
            if let Ok(mut st) = status.lock() {
                *st = s;
            }
        };
        set(if profiles.len() == 1 {
            format!("{name}: looking for the MIDI port \"{port}\"")
        } else {
            format!("Looking for a controller: {wanted}")
        });
        let feedback = Arc::clone(&link.feedback);
        let spawned = std::thread::Builder::new()
            .name("kaldor-controller".into())
            .spawn(move || {
                use dj_midi::ns4fx_out::{GOODBYE, LEAVE_DEMO, STATE_REQUEST};
                let mut input: Option<dj_midi::device::Input> = None;
                let mut output: Option<dj_midi::device::Output> = None;
                let mut state = crate::controller_out::OutputState::default();
                let mut msgs: Vec<Vec<u8>> = Vec::new();
                let mut tick = 0u32;
                let started = std::time::Instant::now();
                // The next state request if the controller has not answered
                // (when, and how many were sent).
                let mut retry: Option<(std::time::Instant, usize)> = None;
                // The port name, for the status line.
                let mut connected = String::new();
                loop {
                    if stop.load(Ordering::Relaxed) {
                        if let Some(o) = output.as_mut() {
                            match lights {
                                // Hand the controller back (its idle display).
                                Lights::Ns4fx => {
                                    let _ = o.send(&GOODBYE);
                                }
                                Lights::DdjFlx6 => {
                                    msgs.clear();
                                    state.all_off(&mut msgs);
                                    for m in &msgs {
                                        let _ = o.send(m);
                                    }
                                }
                                Lights::None => {}
                            }
                        }
                        break;
                    }
                    // The port, once a second (hot-plug).
                    if tick.is_multiple_of(PORT_CHECK_TICKS) {
                        let ports = dj_midi::device::input_ports().unwrap_or_default();
                        // Not connected: the first controller that is there
                        // (its profile takes over).
                        if input.is_none()
                            && let Some(p) = profiles.iter().find(|p| {
                                dj_midi::device::find_port(&ports, &p.port).is_some()
                            })
                            && p.name != name
                        {
                            port = p.port.clone();
                            name = p.name.clone();
                            lights = p.lights;
                            if let Ok(mut pr) = processor.lock() {
                                pr.set_profile(p.clone());
                            }
                        }
                        let present = dj_midi::device::find_port(&ports, &port);
                        match (&input, present) {
                            (None, Some(_)) => {
                                let p = Arc::clone(&processor);
                                match dj_midi::device::open_input(&port, move |_, b| {
                                    if let Ok(mut p) = p.lock() {
                                        p.feed(b);
                                    }
                                }) {
                                    Ok(i) => {
                                        set(format!("{name} connected ({})", i.name));
                                        connected = i.name.clone();
                                        input = Some(i);
                                        if lights == Lights::DdjFlx6 {
                                            // Lights only: no start or state
                                            // request messages are documented.
                                            match dj_midi::device::open_output(&port) {
                                                Ok(o) => {
                                                    state.reset();
                                                    output = Some(o);
                                                }
                                                Err(e) => set(format!(
                                                    "{name} connected ({connected}), but its output (lights) did not open: {e}"
                                                )),
                                            }
                                        }
                                        if lights == Lights::Ns4fx {
                                            match dj_midi::device::open_output(&port) {
                                                Ok(mut o) => {
                                                    let _ = o.send(&LEAVE_DEMO);
                                                    // Ask for the position of
                                                    // every control: the
                                                    // input is already open, so
                                                    // the answer is not missed.
                                                    if let Ok(mut p) = processor.lock() {
                                                        p.begin_state_dump();
                                                    }
                                                    let _ = o.send(&STATE_REQUEST);
                                                    retry = Some((
                                                        std::time::Instant::now()
                                                            + STATE_RETRY_AFTER[0],
                                                        1,
                                                    ));
                                                    state.reset();
                                                    output = Some(o);
                                                }
                                                Err(e) => set(format!(
                                                    "{name} connected ({}), but its output (lights, position request) did not open: {e}",
                                                    connected
                                                )),
                                            }
                                        }
                                    }
                                    Err(e) => set(format!("{name}: {e}")),
                                }
                            }
                            (Some(_), None) => {
                                input = None;
                                output = None;
                                set(format!("{name} unplugged; waiting for it"));
                            }
                            _ => {}
                        }
                    }
                    // The app replaced its audio engine: ask again so the
                    // new engine gets the controller's positions.
                    if let Some(o) = output.as_mut()
                        && resync.swap(false, Ordering::Relaxed)
                        && lights == Lights::Ns4fx
                    {
                        if let Ok(mut p) = processor.lock() {
                            p.begin_state_dump();
                        }
                        let _ = o.send(&STATE_REQUEST);
                        retry = Some((std::time::Instant::now() + STATE_RETRY_AFTER[0], 1));
                    }
                    // A controller just plugged in is still starting up and
                    // may miss the first request: ask again until it answers,
                    // with the start message and all lights again.
                    if let (Some(o), Some((at, n))) = (output.as_mut(), retry)
                        && std::time::Instant::now() >= at
                    {
                        let seen = processor.lock().map(|p| p.dump_seen()).unwrap_or(true);
                        retry = match STATE_RETRY_AFTER.get(n) {
                            _ if seen => {
                                set(format!(
                                    "{name} connected ({connected}); positions synced \
                                     (answer to request {n})"
                                ));
                                None
                            }
                            Some(next) => {
                                let _ = o.send(&LEAVE_DEMO);
                                state.reset();
                                if let Ok(mut p) = processor.lock() {
                                    p.begin_state_dump();
                                }
                                let _ = o.send(&STATE_REQUEST);
                                Some((std::time::Instant::now() + *next, n + 1))
                            }
                            None => {
                                set(format!(
                                    "{name} connected ({connected}); no answer to the \
                                     position request after {n} tries"
                                ));
                                None
                            }
                        };
                    }
                    // Lights, displays and meters: what changed.
                    if let Some(o) = output.as_mut() {
                        let fb = feedback.lock().map(|f| *f).unwrap_or_default();
                        let ctl = processor
                            .lock()
                            .map(|p| crate::controller_out::ControllerState {
                                fader_cuts: p.ns4fx.fader_cuts,
                                jog_turns: p.ns4fx.jog_turns,
                                key_shift_mode: p.ns4fx.key_shift_mode,
                            })
                            .unwrap_or_default();
                        // Flicker twice a second.
                        let flicker_on =
                            (started.elapsed().as_millis() / FLICKER_HALF_MS).is_multiple_of(2);
                        msgs.clear();
                        match lights {
                            Lights::Ns4fx => state.messages(&fb, &ctl, flicker_on, &mut msgs),
                            Lights::DdjFlx6 => {
                                state.flx6_messages(&fb, &ctl, flicker_on, &mut msgs);
                            }
                            Lights::None => {}
                        }
                        for m in &msgs {
                            if o.send(m).is_err() {
                                // Unplugged: the port check notices.
                                break;
                            }
                        }
                    }
                    tick = tick.wrapping_add(1);
                    std::thread::sleep(OUTPUT_TICK);
                }
            });
        match spawned {
            Ok(h) => link.thread = Some(h),
            Err(e) => tracing::warn!("controller thread: {e}"),
        }
        link
    }

    /// The last messages the profile did not know (oldest first).
    pub fn unknown(&self) -> Vec<String> {
        self.unknown
            .lock()
            .map(|u| u.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// The connection status for the UI.
    pub fn status(&self) -> String {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

impl crate::app::KaldorApp {
    /// Publishes what the controller needs and applies its UI actions
    /// (once a frame).
    pub(crate) fn poll_controller(&mut self) {
        let Some(link) = &self.controller else {
            return;
        };
        let shared = Shared {
            pad_mode: std::array::from_fn(|d| self.decks[d].perf.pad_mode),
            cue_loop_beats: std::array::from_fn(|d| {
                let p = &self.decks[d].perf;
                if p.cue_loop { p.auto_loop_beats } else { 0.0 }
            }),
            slicer_domain: std::array::from_fn(|d| self.decks[d].perf.slicer_domain),
            sampler_bank: self.sampler.bank,
            sample_rate: self.engine.snapshot().sample_rate.max(1),
            vinyl: std::array::from_fn(|d| self.decks[d].jog_vinyl),
            learning: self.learn.is_some(),
            jog_sensitivity: self.config.controller.jog_sensitivity,
            soft_takeover: self.config.controller.soft_takeover,
            fx_lit: crate::fx_panel::lit(&self.engine.snapshot().fx.params),
            color_fx: self.engine.snapshot().mixer.color_fx,
            loop_active: std::array::from_fn(|d| {
                self.engine.snapshot().decks[d]
                    .loop_info
                    .is_some_and(|l| l.active)
            }),
        };
        if let Ok(mut f) = link.feedback.lock() {
            *f = self.controller_feedback();
        }
        if let Ok(mut s) = link.shared.lock() {
            *s = shared;
        }
        // The learned bindings, when they changed.
        if let Ok(mut l) = link.learned.lock()
            && l.1 != self.config.controller.learned
        {
            l.1 = self.config.controller.learned.clone();
            l.0 = l.0.wrapping_add(1);
        }
        let actions: Vec<UiAction> = link.ui_rx.try_iter().collect();
        for a in actions {
            self.controller_action(a);
        }
    }

    fn controller_action(&mut self, a: UiAction) {
        let snap = *self.engine.snapshot();
        match a {
            UiAction::Layer { side, deck } => self.set_layer(side, deck),
            UiAction::PadMode { deck, mode } => self.set_pad_mode(deck, mode),
            UiAction::Pad { deck, pad, pressed } => self.pad(deck, pad, pressed),
            UiAction::PadSecondary { deck, pad } => self.pad_secondary(deck, pad),
            UiAction::LoopToggle { deck } => {
                if snap.decks[deck].loop_info.is_some_and(|l| l.active) {
                    self.deck_command(deck, DeckCommand::LoopExit);
                } else {
                    self.auto_loop(deck);
                }
            }
            UiAction::ScaleLoop { deck, factor } => self.scale_loop(deck, factor),
            UiAction::Browse(n) => {
                if let Some(lib) = self.library.as_mut() {
                    lib.knob_used = true;
                    if lib.browse_crates {
                        lib.move_crate(n);
                    } else {
                        lib.move_selection(n);
                    }
                }
            }
            UiAction::BrowsePage(n) => {
                if let Some(lib) = self.library.as_mut() {
                    lib.knob_used = true;
                    lib.move_selection(n * 10);
                }
            }
            UiAction::BrowseBack => {
                if let Some(lib) = self.library.as_mut() {
                    lib.knob_used = true;
                    if !lib.browse_crates {
                        lib.toggle_browse_focus();
                    }
                    lib.status = "BROWSE knob: crates".into();
                }
            }
            UiAction::BrowseFocus => {
                if let Some(lib) = self.library.as_mut() {
                    // On a crate with sub-crates: it opens or closes.
                    if lib.toggle_selected_parent() {
                        let open = match lib.selection {
                            crate::library::Selection::Crate(c) => !lib.collapsed.contains(&c),
                            crate::library::Selection::All => false,
                        };
                        lib.status = if open {
                            "Opened: turn BROWSE for its sub-crates (push again to close)".into()
                        } else {
                            "Closed (push again to open)".into()
                        };
                    } else {
                        lib.toggle_browse_focus();
                        lib.status = if lib.browse_crates {
                            "BROWSE knob: crates (press it again for tracks)".into()
                        } else {
                            "BROWSE knob: tracks (press it again for crates)".into()
                        };
                    }
                }
            }
            UiAction::Load { deck } => {
                let deck = deck.min(3);
                let Some(lib) = self.library.as_mut() else {
                    return;
                };
                let Some(id) = lib.selected_track else {
                    lib.status = "select a track first (turn the BROWSE knob)".into();
                    return;
                };
                if snap.decks[deck].playing {
                    lib.status = format!("deck {} is playing: not loaded", deck + 1);
                    return;
                }
                self.load_library_track(DeckId::ALL[deck], id);
            }
            UiAction::Eject { deck } => {
                let deck = deck.min(3);
                if snap.decks[deck].playing {
                    if let Some(lib) = self.library.as_mut() {
                        lib.status = format!("deck {} is playing: not ejected", deck + 1);
                    }
                    return;
                }
                self.eject_deck(deck);
            }
            UiAction::KeyShift { deck, delta } => self.key_shift_by(deck, delta),
            UiAction::SetVinyl { deck, on } => {
                if let Some(d) = self.decks.get_mut(deck) {
                    d.jog_vinyl = on;
                }
            }
            UiAction::Learned {
                note,
                channel,
                number,
            } => {
                let Some((function, deck)) = self.learn.take() else {
                    return;
                };
                let b = LearnedBinding {
                    note,
                    channel,
                    number,
                    function,
                    deck,
                };
                let learned = &mut self.config.controller.learned;
                // One function per control: a control learned again moves.
                learned.retain(|x| (x.note, x.channel, x.number) != (note, channel, number));
                learned.push(b);
                self.events.push(crate::app::AppEvent::ConfigChanged);
                self.learn_status = format!("Learned: {}", learned_text(&b));
            }
            UiAction::LearnedPress {
                function,
                deck,
                pressed,
            } => self.learned_press(function, deck, pressed),
            UiAction::ToggleQuantize => {
                let on = !self.config.performance.quantize;
                self.set_quantize(on);
            }
        }
    }
}

/// "note ch1 0x30 -> Deck 2: Pad 3".
pub fn learned_text(b: &LearnedBinding) -> String {
    let msg = format!(
        "{} ch{} 0x{:02X}",
        if b.note { "note" } else { "CC" },
        b.channel,
        b.number
    );
    let what = match b.deck {
        Some(d) if b.function.needs_deck() => format!("Deck {d}: {}", b.function.label()),
        _ => b.function.label(),
    };
    format!("{msg} -> {what}")
}

impl crate::app::KaldorApp {
    /// MIDI learn: the next control pressed or moved on the controller gets
    /// `function` (on `deck` 1-4 for deck functions).
    pub fn start_learn(&mut self, function: MidiFunction, deck: Option<u8>) {
        self.learn = Some((function, deck.filter(|_| function.needs_deck())));
        self.learn_status = format!(
            "Press, turn or move the control for \"{}\" on the controller...",
            function.label()
        );
    }

    pub fn cancel_learn(&mut self) {
        if self.learn.take().is_some() {
            self.learn_status = "MIDI learn cancelled".into();
        }
    }

    /// The function MIDI learn is waiting for, if any.
    pub fn learning(&self) -> Option<(MidiFunction, Option<u8>)> {
        self.learn
    }

    /// The learned bindings.
    pub fn learned(&self) -> &[LearnedBinding] {
        &self.config.controller.learned
    }

    /// Removes learned binding `i` (the control goes back to the profile).
    pub fn remove_learned(&mut self, i: usize) {
        if i < self.config.controller.learned.len() {
            let b = self.config.controller.learned.remove(i);
            self.learn_status = format!("Removed: {}", learned_text(&b));
            self.events.push(crate::app::AppEvent::ConfigChanged);
        }
    }

    /// Removes every learned binding: the controller works as its profile
    /// says again.
    pub fn clear_learned(&mut self) {
        if !self.config.controller.learned.is_empty() {
            self.config.controller.learned.clear();
            self.learn_status = "All learned controls removed".into();
            self.events.push(crate::app::AppEvent::ConfigChanged);
        }
    }

    /// A learned button: runs like the keyboard action, on its deck.
    fn learned_press(&mut self, f: MidiFunction, deck: Option<usize>, pressed: bool) {
        use dj_core::config::KeyAction as K;
        let d = deck.unwrap_or(self.kb_deck).min(3);
        let n = d as u8 + 1;
        let action = match f {
            MidiFunction::Play => K::Play(n),
            MidiFunction::Cue => K::Cue(n),
            MidiFunction::BendDown => K::BendDown(n),
            MidiFunction::BendUp => K::BendUp(n),
            MidiFunction::KeyLock => K::KeyLock(n),
            MidiFunction::HeadphoneCue => K::Pfl(n),
            MidiFunction::Sync => K::Sync,
            MidiFunction::SyncMaster => K::SyncMaster,
            MidiFunction::Slip => K::Slip,
            MidiFunction::Pad(k) => K::Pad(k),
            MidiFunction::PadSecondary(k) => K::PadSecondary(k),
            MidiFunction::PadMode(k) => K::PadMode(k),
            MidiFunction::LoopIn => K::LoopIn,
            MidiFunction::LoopOut => K::LoopOut,
            MidiFunction::Reloop => K::Reloop,
            MidiFunction::AutoLoop => K::AutoLoop,
            MidiFunction::LoopHalve => K::LoopHalve,
            MidiFunction::LoopDouble => K::LoopDouble,
            MidiFunction::LoopMoveBack => K::LoopMoveBack,
            MidiFunction::LoopMoveForward => K::LoopMoveForward,
            MidiFunction::BeatJumpBack => K::BeatJumpBack,
            MidiFunction::BeatJumpForward => K::BeatJumpForward,
            MidiFunction::KeyShiftDown => K::KeyShiftDown,
            MidiFunction::KeyShiftUp => K::KeyShiftUp,
            MidiFunction::KeyShiftReset => K::KeyShiftReset,
            MidiFunction::GridSetBeatOne => K::GridSetBeatOne,
            MidiFunction::GridTap => K::GridTap,
            MidiFunction::SamplerBankPrev => K::SamplerBankPrev,
            MidiFunction::SamplerBankNext => K::SamplerBankNext,
            MidiFunction::Quantize => K::Quantize,
            // Handled on the MIDI side (or not buttons).
            _ => return,
        };
        // The keyboard actions work on the keyboard deck: use the learned
        // deck for this one action.
        let kb = self.kb_deck;
        self.kb_deck = d;
        self.key_action(action, pressed);
        self.kb_deck = kb;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dj_audio::{EffectKind, FxMode};

    fn run(ns: &mut Ns4fx, shared: &Shared, msgs: &[&[u8]]) -> Vec<Out> {
        let mut out = Vec::new();
        for m in msgs {
            ns.handle(m, shared, &mut out);
        }
        out
    }

    /// `Command` has no `PartialEq` (it can carry track audio): compare
    /// the debug text.
    fn same(a: &[Out], b: &[Out]) {
        assert_eq!(format!("{a:?}"), format!("{b:?}"));
    }

    fn deck(d: usize, c: DeckCommand) -> Out {
        Out::Engine(Command::Deck(DeckId::ALL[d], c))
    }

    fn fx(c: FxCommand) -> Out {
        Out::Engine(Command::Fx(c))
    }

    /// A paddle: its unit on its own side's decks, then on or off.
    fn paddle(u: usize, on: bool) -> Vec<Out> {
        let mut v: Vec<Out> = (0..dj_audio::FX_TARGETS)
            .map(|t| fx(FxCommand::Assign(u, t, t < 4 && t % 2 == u)))
            .collect();
        v.push(fx(FxCommand::UnitOn(u, on)));
        v
    }

    /// Both units switched to one effect (`filter`: the LP / HP knob).
    fn pick(kind: EffectKind, filter: Option<f32>, knob: [Vec<FxCommand>; 2]) -> Vec<Out> {
        let mut v = Vec::new();
        for (u, knob) in knob.into_iter().enumerate() {
            v.push(fx(FxCommand::Mode(u, FxMode::Single)));
            v.push(fx(FxCommand::SlotKind(u, 0, kind)));
            if let Some(hp) = filter {
                v.push(fx(FxCommand::SlotParam(u, 0, 1, hp)));
            }
            v.push(fx(FxCommand::SlotOn(u, 0, true)));
            v.extend(knob.into_iter().map(fx));
        }
        v
    }

    #[test]
    fn fx_section_paddles_buttons_knobs_and_tap() {
        // The six buttons choose HPF, LPF, Flanger, Echo,
        // Reverb or Phaser (one at a time); the left paddle plays it on
        // decks 1 and 3, the right one on decks 2 and 4.
        let mut ns = Ns4fx::new(Profile::ns4fx());
        let s = Shared::default();
        let out = run(
            &mut ns,
            &s,
            &[
                &[0xB8, 0x03, 0x01], // left paddle up: on
                &[0xB8, 0x03, 0x00], // centre: off
                &[0xB9, 0x03, 0x02], // right paddle down (held): on
            ],
        );
        let mut want = paddle(0, true);
        want.extend(paddle(0, false));
        want.extend(paddle(1, true));
        same(&out, &want);
        // Button 4 (Echo, channel 10): both sides, WET/DRY where it was
        // (the middle until the knob sends).
        let out = run(&mut ns, &s, &[&[0x99, 0x03, 0x7F], &[0x89, 0x03, 0x00]]);
        same(
            &out,
            &pick(
                EffectKind::Echo,
                None,
                [vec![FxCommand::Mix(0, 0.5)], vec![FxCommand::Mix(1, 0.5)]],
            ),
        );
        // WET/DRY (sent on both channels) is the mix.
        let out = run(&mut ns, &s, &[&[0xB8, 0x04, 0x7F], &[0xB9, 0x04, 0x7F]]);
        same(
            &out,
            &[fx(FxCommand::Mix(0, 1.0)), fx(FxCommand::Mix(1, 1.0))],
        );
        // Button 1 (HPF): the filter, high-pass; WET/DRY closes it.
        let out = run(&mut ns, &s, &[&[0x98, 0x00, 0x7F]]);
        let knob = |u| vec![FxCommand::SlotParam(u, 0, 0, 1.0), FxCommand::Mix(u, 1.0)];
        same(
            &out,
            &pick(EffectKind::Filter, Some(1.0), [knob(0), knob(1)]),
        );
        let out = run(&mut ns, &s, &[&[0xB8, 0x04, 0x00]]);
        same(
            &out,
            &[
                fx(FxCommand::SlotParam(0, 0, 0, 0.0)),
                fx(FxCommand::Mix(0, 1.0)),
            ],
        );
        // Button 2 (LPF): low-pass.
        let out = run(&mut ns, &s, &[&[0x98, 0x01, 0x7F]]);
        let knob = |u, v| vec![FxCommand::SlotParam(u, 0, 0, v), FxCommand::Mix(u, 1.0)];
        same(
            &out,
            &pick(EffectKind::Filter, Some(0.0), [knob(0, 0.0), knob(1, 1.0)]),
        );
        // The lit button again: nothing chosen.
        let out = run(&mut ns, &s, &[&[0x98, 0x01, 0x7F]]);
        same(
            &out,
            &[
                fx(FxCommand::SlotOn(0, 0, false)),
                fx(FxCommand::SlotOn(1, 0, false)),
            ],
        );
        // Buttons 3, 5 and 6: Flanger, Reverb, Phaser.
        for (msg, kind) in [
            ([0x98, 0x02, 0x7F], EffectKind::Flanger),
            ([0x99, 0x04, 0x7F], EffectKind::Reverb),
            ([0x99, 0x05, 0x7F], EffectKind::Phaser),
        ] {
            let out = run(&mut ns, &s, &[&msg]);
            same(
                &out,
                &pick(
                    kind,
                    None,
                    [vec![FxCommand::Mix(0, 0.0)], vec![FxCommand::Mix(1, 1.0)]],
                ),
            );
        }
        // BEATS: the timing, on both units.
        let out = run(&mut ns, &s, &[&[0xB8, 0x05, 0x01], &[0xB9, 0x05, 0x01]]);
        same(
            &out,
            &[
                fx(FxCommand::BeatsStep(0, 1)),
                fx(FxCommand::BeatsStep(1, 1)),
            ],
        );
        // SHIFT: BEATS changes the depth, TAP goes back to the decks' tempo.
        let out = run(
            &mut ns,
            &s,
            &[
                &[0x90, 0x20, 0x7F],
                &[0xB8, 0x05, 0x7F],
                &[0x98, 0x09, 0x7F],
                &[0x99, 0x09, 0x7F],
                &[0x80, 0x20, 0x00],
            ],
        );
        same(
            &out,
            &[fx(FxCommand::DepthStep(0, -0.05)), fx(FxCommand::Tap(None))],
        );
        // SHIFT + button 4 as the NS4FX sends it (FX capture step 5:
        // channel 10, note 0x00): the color FX dub echo on the filter
        // knobs; the main effect stays.
        let shifted = |ns: &mut Ns4fx, s: &Shared, msg: &[u8]| {
            run(ns, s, &[&[0x90, 0x20, 0x7F], msg, &[0x80, 0x20, 0x00]])
        };
        let color = |c| [Out::Engine(Command::Mixer(MixerCommand::ColorFx(c)))];
        let out = shifted(&mut ns, &s, &[0x99, 0x00, 0x7F]);
        same(&out, &color(dj_audio::ColorFx::DubEcho));
        // Again (the engine has it now): back to the plain filter.
        let s2 = Shared {
            color_fx: dj_audio::ColorFx::DubEcho,
            ..s
        };
        let out = shifted(&mut ns, &s2, &[0x99, 0x00, 0x7F]);
        same(&out, &color(dj_audio::ColorFx::Filter));
        // TAP twice half a second apart: 120 BPM (once, though TAP comes on
        // both channels).
        let mut out = Vec::new();
        for (t, m) in [
            (1.0, [0x98, 0x09, 0x7F]),
            (1.0, [0x99, 0x09, 0x7F]),
            (1.5, [0x98, 0x09, 0x7F]),
            (1.5, [0x99, 0x09, 0x7F]),
        ] {
            ns.handle_at(&m, &s, t, &mut out);
        }
        assert_eq!(out.len(), 1, "{out:?}");
        let Out::Engine(Command::Fx(FxCommand::Tap(Some(bpm)))) = out[0] else {
            panic!("{out:?}");
        };
        assert!((bpm - 120.0).abs() < 1e-6);
    }

    #[test]
    fn transport_mixer_and_layers() {
        let mut ns = Ns4fx::new(Profile::ns4fx());
        let s = Shared::default();
        let out = run(
            &mut ns,
            &s,
            &[
                &[0x90, 0x00, 0x7F], // deck 1 PLAY
                &[0x80, 0x00, 0x00],
                &[0x91, 0x01, 0x7F], // deck 2 CUE down
                &[0x81, 0x01, 0x00], // and up
                &[0xB2, 0x1C, 0x7F], // strip 3 fader up
                &[0x93, 0x1B, 0x7F], // strip 4 headphone cue
                &[0xB0, 0x1E, 0x01], // strip 1 assign left
                &[0xBF, 0x08, 0x00], // crossfader fully left
            ],
        );
        same(
            &out,
            &[
                deck(0, DeckCommand::TogglePlay),
                deck(1, DeckCommand::Cue(true)),
                deck(1, DeckCommand::Cue(false)),
                Out::Engine(Command::Mixer(MixerCommand::Fader(DeckId::ALL[2], 1.0))),
                Out::Engine(Command::Mixer(MixerCommand::TogglePfl(DeckId::ALL[3]))),
                Out::Engine(Command::Mixer(MixerCommand::XfaderAssign(
                    DeckId::ALL[0],
                    XfaderSide::A,
                ))),
                Out::Engine(Command::Mixer(MixerCommand::Crossfader(-1.0))),
            ],
        );
        // LAYER to deck 3, then deck 3's PLAY.
        let out = run(
            &mut ns,
            &s,
            &[
                &[0x90, 0x08, 0x00],
                &[0x92, 0x08, 0x7F],
                &[0x92, 0x00, 0x7F],
            ],
        );
        same(
            &out,
            &[
                Out::Ui(UiAction::Layer { side: 0, deck: 2 }),
                // The pitch fader now belongs to deck 3: soft takeover.
                deck(2, DeckCommand::PitchTakeover(None)),
                deck(2, DeckCommand::TogglePlay),
            ],
        );
        assert_eq!(ns.active(), [2, 1]);
    }

    #[test]
    fn jog_scratches_nudges_and_searches() {
        let mut ns = Ns4fx::new(Profile::ns4fx());
        let s = Shared::default();
        // Touch and turn +10 ticks: scratch. Let go while it spins: the
        // next ticks still scratch (a backspin); after a pause the wheel
        // (outer ring) nudges.
        let mut out = Vec::new();
        for (t, m) in [
            (0.00, [0x90, 0x06, 0x7F]),
            (0.01, [0xB0, 0x06, 0x0A]),
            (0.02, [0x80, 0x06, 0x00]),
            (0.03, [0xB0, 0x06, 0x76]),
            (0.50, [0xB0, 0x06, 0x7E]),
        ] {
            ns.handle_at(&m, &s, t, &mut out);
        }
        same(
            &out,
            &[
                deck(0, DeckCommand::JogTouch(true)),
                deck(0, DeckCommand::JogScratch(0.01)),
                deck(0, DeckCommand::JogTouch(false)),
                deck(0, DeckCommand::JogScratch(-0.01)),
                deck(0, DeckCommand::JogNudge(-0.002)),
            ],
        );
        // SCRATCH off: touching does not hold the deck.
        let out = run(
            &mut ns,
            &s,
            &[
                &[0x90, 0x07, 0x7F],
                &[0x90, 0x06, 0x7F],
                &[0xB0, 0x06, 0x01],
            ],
        );
        same(
            &out,
            &[
                Out::Ui(UiAction::SetVinyl { deck: 0, on: false }),
                deck(0, DeckCommand::JogNudge(0.001)),
            ],
        );
        // SHIFT + jog: fast search, 30 s per turn.
        let out = run(&mut ns, &s, &[&[0x90, 0x20, 0x7F], &[0xB0, 0x06, 0x0A]]);
        same(
            &out,
            &[deck(0, DeckCommand::SeekBy(0.01 * 30.0 * 48_000.0))],
        );
    }

    #[test]
    fn pads_follow_the_mode_and_bend_buttons_combine() {
        let mut ns = Ns4fx::new(Profile::ns4fx());
        let mut s = Shared::default();
        // CUES mode: hot cue 3.
        let out = run(&mut ns, &s, &[&[0x94, 0x16, 0x7F], &[0x84, 0x16, 0x00]]);
        same(
            &out,
            &[
                deck(
                    0,
                    DeckCommand::HotCue {
                        slot: 2,
                        pressed: true,
                        loop_beats: 0.0,
                    },
                ),
                deck(
                    0,
                    DeckCommand::HotCue {
                        slot: 2,
                        pressed: false,
                        loop_beats: 0.0,
                    },
                ),
            ],
        );
        // SHIFT + SAMPLE: pitch play (through the UI).
        let out = run(&mut ns, &s, &[&[0x94, 0x0F, 0x7F]]);
        same(
            &out,
            &[Out::Ui(UiAction::PadMode {
                deck: 0,
                mode: PadMode::PitchPlay,
            })],
        );
        let out = run(&mut ns, &s, &[&[0x94, 0x14, 0x7F]]);
        same(
            &out,
            &[Out::Ui(UiAction::Pad {
                deck: 0,
                pad: 0,
                pressed: true,
            })],
        );
        // SAMPLE mode with bank B: pad 2 is slot 10; a pad held when the
        // mode changes is released in its own mode.
        s.sampler_bank = 1;
        let out = run(&mut ns, &s, &[&[0x94, 0x0B, 0x7F], &[0x94, 0x15, 0x7F]]);
        same(
            &out,
            &[
                Out::Ui(UiAction::Pad {
                    deck: 0,
                    pad: 0,
                    pressed: false,
                }),
                Out::Ui(UiAction::PadMode {
                    deck: 0,
                    mode: PadMode::Sampler,
                }),
                Out::Engine(Command::Sampler(SamplerCommand::Trigger {
                    slot: 9,
                    pressed: true,
                })),
            ],
        );
        // FADER CUTS: pads do nothing here.
        let out = run(
            &mut ns,
            &s,
            &[
                &[0x84, 0x15, 0x00],
                &[0x94, 0x07, 0x7F],
                &[0x94, 0x14, 0x7F],
            ],
        );
        same(
            &out,
            &[Out::Engine(Command::Sampler(SamplerCommand::Trigger {
                slot: 9,
                pressed: false,
            }))],
        );
        // Both bend buttons: key lock.
        let out = run(&mut ns, &s, &[&[0x90, 0x0B, 0x7F], &[0x90, 0x0C, 0x7F]]);
        same(
            &out,
            &[
                deck(0, DeckCommand::PitchBend(0.04)),
                deck(0, DeckCommand::ToggleKeyLock),
                deck(0, DeckCommand::PitchBend(0.0)),
            ],
        );
        // SHIFT + bend -: the next pitch range; SHIFT + bend +: key lock.
        let out = run(
            &mut ns,
            &s,
            &[
                &[0x90, 0x2C, 0x7F],
                &[0x80, 0x2C, 0x00],
                &[0x91, 0x2B, 0x7F],
                &[0x81, 0x2B, 0x00],
            ],
        );
        same(
            &out,
            &[
                deck(0, DeckCommand::CyclePitchRange),
                deck(1, DeckCommand::ToggleKeyLock),
            ],
        );
    }

    /// An NS4FX's answer to the state request
    /// (`ns4fx-capture-7.midilog`, step 3), knobs and switches only
    /// (the processor drops the button releases): every position you set
    /// before the probe arrives as the right command.
    #[test]
    fn state_dump_from_the_capture_sets_every_control() {
        let capture = include_str!("../../dj-midi/tests/captures/ns4fx-capture-7.midilog");
        let mut msgs: Vec<Vec<u8>> = Vec::new();
        let mut step = 0;
        for line in capture.lines() {
            if let Some(rest) = line.strip_prefix("step ") {
                step = rest
                    .split_whitespace()
                    .next()
                    .and_then(|n| n.parse().ok())
                    .unwrap_or(0);
                continue;
            }
            if step != 3 || line.starts_with('#') || line.trim().is_empty() {
                continue;
            }
            // "  <ms> B0 09 00"
            let bytes: Vec<u8> = line
                .split_whitespace()
                .skip(1)
                .filter_map(|h| u8::from_str_radix(h, 16).ok())
                .collect();
            if bytes.first().is_some_and(|b| b & 0xF0 == 0xB0) {
                msgs.push(bytes);
            }
        }
        assert!(msgs.len() > 30, "{} control messages", msgs.len());
        let mut ns = Ns4fx::new(Profile::ns4fx());
        ns.in_dump = true;
        let refs: Vec<&[u8]> = msgs.iter().map(Vec::as_slice).collect();
        let out = format!("{:?}", run(&mut ns, &Shared::default(), &refs));
        let has = |o: Out| {
            let s = format!("{o:?}");
            assert!(out.contains(&s), "missing {s}");
        };
        let mix = |c: MixerCommand| Out::Engine(Command::Mixer(c));
        // Left pitch fader at the top (slow end), right one at the bottom.
        has(deck(0, DeckCommand::PitchFaderSync(-1.0)));
        has(deck(1, DeckCommand::PitchFaderSync(1.0)));
        // Channel 1 fader up, channel 2 down; filters fully left / right.
        has(mix(MixerCommand::Fader(DeckId::ALL[0], 1.0)));
        has(mix(MixerCommand::Fader(DeckId::ALL[1], 0.0)));
        has(mix(MixerCommand::Filter(DeckId::ALL[0], -1.0)));
        has(mix(MixerCommand::Filter(DeckId::ALL[1], 1.0)));
        // High EQ 1 fully left; crossfader fully left; assigns in the middle.
        has(mix(MixerCommand::Eq(DeckId::ALL[0], EqBand::High, 0.0)));
        has(mix(MixerCommand::Crossfader(-1.0)));
        has(mix(MixerCommand::XfaderAssign(
            DeckId::ALL[0],
            XfaderSide::Thru,
        )));
        // Both FX paddles were up: both units on.
        has(fx(FxCommand::UnitOn(0, true)));
        has(fx(FxCommand::UnitOn(1, true)));
    }

    /// Deck 1 has the left pitch fader a little up; pressing LAYER
    /// (deck 3's note, capture 4 step 5) hands that position to deck 3 at
    /// once, so the screen shows the fader before it moves.
    #[test]
    fn layer_hands_the_pitch_fader_position_to_the_other_deck() {
        let mut ns = Ns4fx::new(Profile::ns4fx());
        let s = Shared::default();
        // Left pitch fader slightly up from the middle (MSB 0x38).
        let out = run(&mut ns, &s, &[&[0xB0, 0x09, 0x38], &[0xB0, 0x29, 0x00]]);
        let p = pitch_from_midi((f32::from(0x38u8) * 128.0) / 16_383.0);
        same(&out, &[deck(0, DeckCommand::PitchFader(p))]);
        // LAYER: deck 3's note on channel 3.
        let out = run(&mut ns, &s, &[&[0x92, 0x08, 0x7F]]);
        let s_out = format!("{out:?}");
        let want = format!("{:?}", deck(2, DeckCommand::PitchTakeover(Some(p))));
        assert!(s_out.contains(&want), "{s_out}");
    }
}

/// The DDJ-FLX6 profile driven through the behaviour with messages from
/// AlphaTheta's MIDI list (no device): what each control does in Kaldor.
#[cfg(test)]
mod flx6_tests {
    use super::*;

    fn run(c: &mut Ns4fx, shared: &Shared, msgs: &[&[u8]]) -> String {
        let mut out = Vec::new();
        for m in msgs {
            c.handle(m, shared, &mut out);
        }
        format!("{out:?}")
    }

    fn has(out: &str, want: &str) {
        assert!(out.contains(want), "expected {want} in {out}");
    }

    #[test]
    fn transport_decks_and_deck_select() {
        let mut c = Ns4fx::new(Profile::ddj_flx6());
        let s = Shared::default();
        has(
            &run(&mut c, &s, &[&[0x90, 0x0B, 0x7F]]),
            "Deck(DeckId(0), TogglePlay)",
        );
        has(
            &run(&mut c, &s, &[&[0x91, 0x0C, 0x7F]]),
            "Deck(DeckId(1), Cue(true))",
        );
        has(
            &run(&mut c, &s, &[&[0x91, 0x58, 0x7F]]),
            "ToggleSync(DeckId(1))",
        );
        has(
            &run(&mut c, &s, &[&[0x90, 0x5C, 0x7F]]),
            "SyncMaster(DeckId(0))",
        );
        // DECK 3 on the left: 0x92 0x3C on, 0x90 0x3C off (Fig. 1-5L).
        let out = run(&mut c, &s, &[&[0x92, 0x3C, 0x7F], &[0x90, 0x3C, 0x00]]);
        has(&out, "Layer { side: 0, deck: 2 }");
        assert_eq!(c.active(), [2, 1]);
        has(
            &run(&mut c, &s, &[&[0x92, 0x0B, 0x7F]]),
            "Deck(DeckId(2), TogglePlay)",
        );
        // TEMPO 14-bit: top (0) is the slow end.
        let out = run(&mut c, &s, &[&[0xB0, 0x00, 0x00], &[0xB0, 0x20, 0x00]]);
        has(&out, "PitchFader(-1.0)");
    }

    #[test]
    fn mixer_master_and_browser() {
        let mut c = Ns4fx::new(Profile::ddj_flx6());
        let s = Shared::default();
        // CH FADER (CH2) at the top: MSB then LSB.
        let out = run(&mut c, &s, &[&[0xB1, 0x13, 0x7F], &[0xB1, 0x33, 0x7F]]);
        has(&out, "Fader(DeckId(1), 1.0)");
        // FILTER (CH3) on channel 7, centred.
        let out = run(&mut c, &s, &[&[0xB6, 0x19, 0x40], &[0xB6, 0x39, 0x00]]);
        has(&out, "Filter(DeckId(2), ");
        has(
            &run(&mut c, &s, &[&[0x93, 0x54, 0x7F]]),
            "TogglePfl(DeckId(3))",
        );
        let out = run(&mut c, &s, &[&[0xB6, 0x08, 0x7F], &[0xB6, 0x28, 0x7F]]);
        has(&out, "MasterGain(6.0)");
        let out = run(&mut c, &s, &[&[0xB6, 0x1F, 0x00], &[0xB6, 0x3F, 0x00]]);
        has(&out, "Crossfader(-1.0)");
        has(&run(&mut c, &s, &[&[0xB6, 0x40, 0x01]]), "Browse(1)");
        has(&run(&mut c, &s, &[&[0x96, 0x41, 0x7F]]), "BrowseFocus");
        has(&run(&mut c, &s, &[&[0x96, 0x65, 0x7F]]), "BrowseBack");
        has(&run(&mut c, &s, &[&[0x96, 0x48, 0x7F]]), "Load { deck: 2 }");
        has(
            &run(&mut c, &s, &[&[0x96, 0x61, 0x7F]]),
            "Eject { deck: 3 }",
        );
    }

    #[test]
    fn jogs_scratch_bend_and_search() {
        let mut c = Ns4fx::new(Profile::ddj_flx6());
        let s = Shared::default();
        let turn = 1.0 / 7200.0;
        // Touch the platter (vinyl on): scratch; +1 tick.
        let out = run(&mut c, &s, &[&[0x90, 0x36, 0x7F], &[0xB0, 0x22, 0x41]]);
        has(&out, "JogTouch(true)");
        has(&out, &format!("JogScratch({turn})"));
        run(&mut c, &s, &[&[0x90, 0x36, 0x00]]);
        // The wheel's side bends; SHIFT + platter searches.
        has(
            &run(&mut c, &s, &[&[0xB0, 0x21, 0x3F]]),
            &format!("JogNudge(-{turn})"),
        );
        has(&run(&mut c, &s, &[&[0xB0, 0x29, 0x41]]), "SeekBy(");
        // SHIFT on the deck's channel, then the side: search too.
        let out = run(&mut c, &s, &[&[0x90, 0x3F, 0x7F], &[0xB0, 0x21, 0x41]]);
        has(&out, "SeekBy(");
    }

    #[test]
    fn pads_follow_the_controller_mode() {
        let mut c = Ns4fx::new(Profile::ddj_flx6());
        let s = Shared::default();
        // HOT CUE pad 1 (deck 1, channel 8): the screen is in CUES already.
        let out = run(&mut c, &s, &[&[0x97, 0x00, 0x7F]]);
        has(&out, "HotCue { slot: 0, pressed: true");
        assert!(!out.contains("PadMode"), "{out}");
        run(&mut c, &s, &[&[0x97, 0x00, 0x00]]);
        // BEAT JUMP pad 4: the deck goes to the beat jump mode, +2 beats.
        let out = run(&mut c, &s, &[&[0x97, 0x23, 0x7F]]);
        has(&out, "PadMode { deck: 0, mode: BeatJump }");
        has(&out, "BeatJump(2.0)");
        // The BEAT LOOP button (SHIFT + BEAT JUMP, Fig. 5-3): auto loops.
        has(
            &run(&mut c, &s, &[&[0x91, 0x6D, 0x7F]]),
            "PadMode { deck: 1, mode: AutoLoop }",
        );
        // KEY SHIFT pads (deck 3, channel 12): set the key; SHIFT + pad: back.
        let out = run(&mut c, &s, &[&[0x9B, 0x52, 0x7F]]);
        has(&out, "Deck(DeckId(2), KeyShift(3))");
        assert!(c.key_shift_mode[2]);
        has(
            &run(&mut c, &s, &[&[0x9C, 0x52, 0x7F]]),
            "Deck(DeckId(2), KeyShift(0))",
        );
        // SHIFT + HOT CUE pad: delete the hot cue (Fig. 5-5, channel 9).
        let out = run(&mut c, &s, &[&[0x98, 0x05, 0x7F]]);
        has(&out, "SetHotCue { slot: 5, position: None }");
    }

    #[test]
    fn loops_and_beat_fx() {
        let mut c = Ns4fx::new(Profile::ddj_flx6());
        let mut s = Shared::default();
        has(&run(&mut c, &s, &[&[0x90, 0x10, 0x7F]]), "LoopIn");
        has(&run(&mut c, &s, &[&[0x90, 0x14, 0x7F]]), "AutoLoop(4.0)");
        has(&run(&mut c, &s, &[&[0x90, 0x4D, 0x7F]]), "Reloop");
        s.loop_active[0] = true;
        has(&run(&mut c, &s, &[&[0x90, 0x4D, 0x7F]]), "LoopExit");
        has(&run(&mut c, &s, &[&[0x90, 0x62, 0x7F]]), "BeatJump(4.0)");
        // BFX SELECT to FX2-2 (channel 6, note 0x71): the fifth effect on
        // unit 1; CH SELECT CH3; ON/OFF of that slot toggles the unit.
        let out = run(&mut c, &s, &[&[0x95, 0x71, 0x7F]]);
        has(
            &out,
            &format!("SlotKind(0, 0, {:?})", crate::fx_panel::BUTTONS[4]),
        );
        let out = run(&mut c, &s, &[&[0x94, 0x1E, 0x7F]]);
        has(&out, "Assign(0, 2, true)");
        has(&out, "Assign(0, 0, false)");
        let out = run(&mut c, &s, &[&[0x95, 0x48, 0x7F]]);
        has(&out, "ToggleUnit(0)");
        let out = run(&mut c, &s, &[&[0x94, 0x07, 0x7F]]);
        has(&out, "BeatsStep(0, 1)");
        let out = run(&mut c, &s, &[&[0xB5, 0x04, 0x7F], &[0xB5, 0x24, 0x7F]]);
        has(&out, "Mix(0, 1.0)");
    }
}
