//! One deck: a loaded track, transport, tempo, key lock and scrub, plus the
//! performance features (hot cues, loops, rolls, slicer, slip, beat jump,
//! key shift and the deck side of sync).
//!
//! Runs on the audio thread. Every state change that would make a jump in
//! the output (play, stop, seek, cue return, key lock switch, hot cue and
//! loop jumps) goes through a short gain fade so it never clicks. Loop wraps
//! themselves are seamless: the renderer reads the track through
//! [`DeckSource`], which maps a continuously counting "virtual" position
//! into the loop, so the time-stretcher never sees a jump.

use crate::perf::{
    DeckGrid, DeckSource, HOT_CUES, LoopInfo, MAX_KEY_SHIFT, MIN_LOOP_FRAMES, SyncMode, wrap_half,
};
use dj_decode::TrackAudio;
use dj_dsp::interp::{JogPace, PlatterFollower, ramp_advance};
use dj_dsp::{LinearSmoother, SignalsmithStretch, SincResampler, TimeStretcher, Wsola};
use std::sync::Arc;

/// Pitch fader range.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PitchRange {
    #[default]
    Eight,
    Sixteen,
    Fifty,
}

impl PitchRange {
    pub const ALL: [PitchRange; 3] = [PitchRange::Eight, PitchRange::Sixteen, PitchRange::Fifty];

    /// Maximum tempo change as a fraction (0.08 for ±8 %).
    pub fn fraction(self) -> f64 {
        match self {
            PitchRange::Eight => 0.08,
            PitchRange::Sixteen => 0.16,
            PitchRange::Fifty => 0.50,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PitchRange::Eight => "±8%",
            PitchRange::Sixteen => "±16%",
            PitchRange::Fifty => "±50%",
        }
    }
}

/// Commands for one deck.
#[derive(Debug)]
pub enum DeckCommand {
    /// Load a track. `serial` is chosen by the sender and echoed in the
    /// snapshot so the UI knows when the engine has switched. Clears the
    /// grid, cues and loops: send them again after the load.
    Load {
        audio: Arc<TrackAudio>,
        serial: u64,
    },
    /// Load with the track's grid and cues in the same command, so the UI
    /// never sees the new track without them (no other command can come
    /// between).
    LoadWithMarkers {
        audio: Arc<TrackAudio>,
        serial: u64,
        markers: TrackMarkers,
    },
    Eject,
    Play(bool),
    TogglePlay,
    /// CUE button pressed (`true`) or released (`false`). CDJ behaviour:
    /// while playing, return to the cue point and pause; while paused, set
    /// the cue point here and play while held.
    Cue(bool),
    /// Jump to a frame position. Leaves an active loop if the target is
    /// outside it.
    Seek(f64),
    /// Pitch fader, -1.0 to 1.0 (scaled by the pitch range): the screen,
    /// the keyboard, restoring settings. Sets the pitch absolutely.
    SetPitch(f64),
    /// The controller's pitch fader moved to -1.0..1.0 (soft takeover, as
    /// Serato): while the deck's pitch was set elsewhere (screen, keyboard,
    /// SYNC turned off, another LAYER) the fader does nothing until it
    /// meets or crosses the pitch, then it follows 1:1. A synced follower
    /// ignores it.
    PitchFader(f64),
    /// The controller's pitch fader is here and the deck takes it at once:
    /// the controller's state dump at start-up.
    PitchFaderSync(f64),
    /// The controller's pitch fader now belongs to this deck (LAYER), at
    /// this position if known (the same physical fader the other deck of the
    /// side used): the deck waits until the fader crosses its pitch, and the
    /// screen shows where the fader is at once.
    PitchTakeover(Option<f64>),
    SetPitchRange(PitchRange),
    /// The next pitch range: 8 % -> 16 % -> 50 % -> 8 % (controller button).
    CyclePitchRange,
    /// Temporary tempo offset (fraction) while a bend button is held.
    PitchBend(f64),
    SetKeyLock(bool),
    /// Drag the waveform: the deck follows `target` (frames) while active.
    Scrub {
        active: bool,
        target: f64,
    },
    /// The track's beatgrid (needed by quantize, auto loops, rolls, the
    /// slicer, beat jump and sync).
    SetGrid(Option<DeckGrid>),
    /// Sets the main cue point (frames), e.g. from the library.
    SetCuePoint(f64),
    /// Hot cue pad pressed or released. An empty slot is set at the play
    /// position (on the grid with quantize); a set one jumps there and
    /// plays. `loop_beats` > 0 also starts a loop of that many beats from
    /// the cue (cue loop).
    HotCue {
        slot: u8,
        pressed: bool,
        loop_beats: f32,
    },
    /// Sets or deletes (`None`) a hot cue (frames), e.g. from the library.
    SetHotCue {
        slot: u8,
        position: Option<f64>,
    },
    /// Loop in point at the play position (moves the start of an active
    /// loop).
    LoopIn,
    /// Loop out point: closes the loop from the in point and starts it (moves
    /// the end of an active loop).
    LoopOut,
    /// Leaves an active loop, or starts the last loop again from its start.
    Reloop,
    LoopExit,
    LoopHalve,
    LoopDouble,
    /// Moves the loop (and the play position) by its own length, -1 or +1.
    LoopMove(i8),
    /// Starts a loop of this many beats (on the grid with quantize), or
    /// leaves it if it is the active loop.
    AutoLoop(f32),
    /// Starts a loop with these bounds (frames), e.g. a saved loop; with
    /// `jump` the deck jumps to its start.
    SetLoop {
        start: f64,
        end: f64,
        jump: bool,
    },
    /// Loop roll: a momentary loop of `beats` beats; on release the deck
    /// continues where it would have been (slip).
    Roll {
        beats: f32,
        pressed: bool,
    },
    /// Slicer: the `domain_beats` beats around the play position are cut
    /// into 8 slices; pad `slice` plays (and loops) its slice while held,
    /// then the deck continues where it would have been.
    Slicer {
        slice: u8,
        pressed: bool,
        domain_beats: f32,
    },
    /// Jumps this many beats (negative = back). Moves an active loop along.
    BeatJump(f64),
    /// Slip mode: loops, hot cues and scrubbing are momentary and the deck
    /// returns to where it would have been.
    SetSlip(bool),
    /// Key shift in semitones (turns key lock on).
    KeyShift(i8),
    /// Slip mode on or off, whatever it is now (controller buttons).
    ToggleSlip,
    /// Key lock on or off, whatever it is now.
    ToggleKeyLock,
    /// The jog wheel top was touched (`true`) or released in scratch mode:
    /// the deck follows the platter from where it is, like a record held
    /// by hand.
    JogTouch(bool),
    /// The touched platter turned by this many revolutions (negative =
    /// backwards); one revolution is [`JOG_SECONDS_PER_TURN`] of audio.
    JogScratch(f64),
    /// The jog turned without scratching (outer ring, or the top in
    /// nudge mode), by this many revolutions: a short tempo bend while
    /// playing, a small move of the position while paused.
    JogNudge(f64),
    /// Moves the play position by this many frames (fast search).
    SeekBy(f64),
    /// Seconds a paused deck takes to slow down to a stop, like a
    /// turntable (0: the usual short fade).
    SetStopBrake(f32),
    /// CUE while playing: back to the cue point and keep playing (`true`,
    /// stutter) or pause there (`false`, CDJ).
    SetStutterCue(bool),
    /// Key lock with WSOLA (`true`, lighter) or Signalsmith Stretch.
    SetKeyLockWsola(bool),
}

/// Audio per jog revolution in scratch mode: a 33 1/3 rpm record turns once
/// in 1.8 s.
pub const JOG_SECONDS_PER_TURN: f64 = 1.8;
/// Tempo bend per revolution of the jog in nudge mode (decays with
/// [`JOG_NUDGE_DECAY_SECONDS`]): about +10 % while turning one revolution
/// a second.
const JOG_NUDGE_PER_TURN: f64 = 1.25;
const JOG_NUDGE_DECAY_SECONDS: f64 = 0.08;
/// A jog let go faster than this (times normal speed) away from play speed
/// spins on by itself; slower, it is a plain release.
const JOG_FREE_MIN_SPEED: f64 = 0.3;
/// A released spinning deck slows down (or speeds up) towards play speed
/// at this rate (times normal speed per second), easing in over the last
/// [`JOG_COAST_EASE_SECONDS`].
const JOG_COAST_RATE: f64 = 4.0;
const JOG_COAST_EASE_SECONDS: f64 = 0.08;

/// A track's grid and cues (frames), applied together with its load.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrackMarkers {
    pub grid: Option<DeckGrid>,
    pub cue_point: Option<f64>,
    pub hot_cues: [Option<f64>; HOT_CUES],
    /// Where the deck starts (frames); `None`: the start of the track.
    pub start: Option<f64>,
}

/// Deck state for the UI.
#[derive(Clone, Copy, Debug, Default)]
pub struct DeckSnapshot {
    pub track_serial: u64,
    pub loaded: bool,
    /// Playhead in frames at the end of the last callback.
    pub position: f64,
    /// Track length in frames (estimate while loading).
    pub length: f64,
    pub ready_frames: u64,
    pub load_complete: bool,
    pub playing: bool,
    pub previewing: bool,
    pub cue_point: f64,
    /// The deck's tempo (1.0 = normal speed): pitch fader, bend and sync.
    /// Not changed by scrubbing; use it for BPM displays and zoom.
    pub tempo: f64,
    /// The tempo the deck is set to, for BPM and pitch displays: the pitch
    /// fader or sync and the pitch-bend buttons, without the jog nudge (a
    /// nudge on the jog's outer ring only lines the beats up, it does not
    /// change the BPM shown) and without the tempo glide.
    pub set_tempo: f64,
    /// The tempo set by the pitch fader or by sync, without the bend and
    /// the phase correction: the waveform scale (a bend only changes
    /// how fast the waveform scrolls).
    pub pitch_tempo: f64,
    /// How fast the play position moves right now: `tempo` while playing,
    /// the scrub speed (can be negative) while scrubbing.
    pub speed: f64,
    pub pitch: f64,
    pub pitch_range: PitchRange,
    pub bend: f64,
    pub key_lock: bool,
    pub scrubbing: bool,
    pub peak: [f32; 2],
    /// Grid the engine uses.
    pub grid: Option<DeckGrid>,
    /// Hot cue positions (frames).
    pub hot_cues: [Option<f64>; HOT_CUES],
    /// Hot cue pad held.
    pub held_hot_cue: Option<u8>,
    /// Loop in point waiting for its out point.
    pub loop_in: Option<f64>,
    /// Current (active or last) loop.
    pub loop_info: Option<LoopInfo>,
    /// A loop roll is held.
    pub rolling: bool,
    /// Slicer pad held.
    pub slicer_held: Option<u8>,
    pub slip: bool,
    /// Where the deck would be without the slip action (ghost playhead).
    pub slip_position: Option<f64>,
    /// SYNC is on (follower, or the master with SYNC pressed).
    pub sync: bool,
    /// Tempo the sync master sets (without the phase correction).
    pub sync_rate: Option<f64>,
    pub key_shift: i8,
    /// Where the controller's pitch fader is (-1..1) while it waits to take
    /// the pitch over (soft takeover); `None` when it follows the pitch or
    /// its position is unknown. The screen draws it as a ghost marker.
    pub pitch_fader_ghost: Option<f64>,
    pub quantize: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Pending {
    None,
    Seek(f64),
    /// Jump by this many frames when the fade-out ends (keeps the beat
    /// phase: the time spent fading is not lost).
    Jump(f64),
    /// Back to the slip position when the fade-out ends.
    SlipReturn,
    /// Restart the time-stretcher (its look-ahead holds audio that a loop
    /// change made wrong).
    Restart,
    Stop {
        return_to: Option<f64>,
    },
    KeyLock(bool),
}

#[derive(Clone, Copy, Debug, Default)]
struct Scrub {
    active: bool,
    target: f64,
    /// The target at the end of the last block (the ticks of a block are
    /// spread over it from here).
    prev_target: f64,
    velocity: f64,
    was_playing: bool,
    follower: PlatterFollower,
    pace: JogPace,
}

/// Reasons the deck is away from its slip position.
const DEV_LOOP: u8 = 1;
const DEV_HOT_CUE: u8 = 2;
const DEV_ROLL: u8 = 4;
const DEV_SCRUB: u8 = 8;
const DEV_SLICER: u8 = 16;
/// Rolls and the slicer always slip, slip mode or not.
const DEV_FORCED: u8 = DEV_ROLL | DEV_SLICER;

/// Fade time for transport changes.
const FADE_MS: f32 = 4.0;
/// Highest scrub speed (times normal).
const MAX_SCRUB_SPEED: f64 = 8.0;
/// Time constant of the platter following a hand on the jog.
const SCRATCH_TAU_SECONDS: f64 = 0.004;
/// The controller pitch fader counts as at the pitch when this close (of
/// -1..1): about 0.08 % of tempo on the 8 % range.
const FADER_MATCH: f64 = 0.01;
/// The slowest and fastest a deck plays (tempo, 1.0 = normal).
const MIN_TEMPO: f64 = 0.1;
const MAX_TEMPO: f64 = 4.0;
/// The longest time constant, for very slow movements.
const SCRATCH_MAX_TAU_SECONDS: f64 = 0.025;
/// Switching key lock while playing crossfades the two paths this long.
const KEY_LOCK_FADE_SECONDS: f64 = 0.03;
/// A restarted Signalsmith stretcher is silent at first and reaches full
/// level after about 75 ms (measured at 48 kHz).
const STRETCH_WARM_SECONDS: f64 = 0.08;
/// Phase correction of a synced deck: tempo change per beat of phase error,
/// and its limit.
const SYNC_GAIN: f64 = 0.5;
const SYNC_MAX_CORRECTION: f64 = 0.02;

/// What the engine tells a synced deck about the sync master.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MasterBeat {
    /// Effective tempo in BPM.
    pub bpm: f64,
    /// Fractional beat count at the start of this block.
    pub beat: f64,
    pub playing: bool,
    /// The master is bent or scrubbed: synced decks hold their phase
    /// correction and keep the new offset afterwards.
    pub nudging: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct SyncState {
    on: bool,
    /// Tempo set by the sync master (replaces the pitch fader).
    rate: Option<f64>,
    correction: f64,
    /// Phase offset (beats) the user set by bending while synced.
    offset: f64,
    /// Align the phase at the next opportunity.
    snap: bool,
    bending: bool,
}

pub(crate) struct Deck {
    track: Option<Arc<TrackAudio>>,
    track_serial: u64,
    /// Track position (frames).
    position: f64,
    /// Virtual position minus track position (see [`DeckSource`]).
    virt_offset: i64,
    playing: bool,
    cue_point: f64,
    previewing: bool,
    play_during_preview: bool,
    pitch: f64,
    pitch_range: PitchRange,
    bend: f64,
    /// Tempo bend from turning the jog (decays by itself).
    jog_bend: f64,
    /// The jog was let go while spinning: the deck brakes from its speed
    /// back to play speed by itself (Pioneer style), the wheel ignored.
    jog_free: bool,
    key_lock: bool,
    key_shift: i8,
    tempo: f64,
    tempo_step_per_frame: f64,
    scrub: Scrub,
    gain: LinearSmoother,
    pending: Pending,
    /// Key lock. Signalsmith Stretch (spectral); `dj_dsp::Wsola` is the lighter
    /// fallback behind the same trait.
    stretcher: SignalsmithStretch,
    /// The lighter key lock, used instead when `use_wsola` is set (both are
    /// built up front, so switching never allocates).
    wsola: Wsola,
    use_wsola: bool,
    /// Stop brake length in seconds (0: off), and whether the deck is
    /// braking to a stop now (a free-running platter, see `jog_free`).
    brake_seconds: f32,
    braking: bool,
    /// How fast a free-running platter changes speed (times normal speed
    /// per second): the jog release, or the stop brake.
    coast_rate: f64,
    /// CUE while playing keeps playing from the cue point.
    stutter_cue: bool,
    /// Varispeed (key lock off) and scratching: windowed sinc.
    resampler: SincResampler,
    /// Frames crossfaded at loop seams (3 ms).
    loop_xfade: i64,
    /// Key lock heard: 0 = the varispeed, 1 = the stretcher; glides towards
    /// the key-lock setting by `kl_step` per frame (30 ms end to end).
    /// Where the controller's pitch fader was last (-1..1), if known.
    fader_last: Option<f64>,
    /// The pitch was set elsewhere: the controller fader waits until it
    /// crosses the pitch (soft takeover).
    fader_locked: bool,
    kl_mix: f32,
    kl_step: f32,
    /// The stretcher ran in the last block (it continues without a restart).
    stretch_live: bool,
    /// Frames left while a restarted stretcher warms up (the varispeed is
    /// heard meanwhile), and the warm-up length.
    kl_warm: usize,
    kl_warm_len: usize,
    /// The stretcher's output while both paths run.
    xf_l: Vec<f32>,
    xf_r: Vec<f32>,
    /// Per-frame positions while scratching.
    scrub_pos: Vec<f64>,
    pub(crate) out_l: Vec<f32>,
    pub(crate) out_r: Vec<f32>,
    peak: [f32; 2],
    sample_rate: u32,
    // Performance features.
    grid: Option<DeckGrid>,
    pub(crate) quantize: bool,
    hot_cues: [Option<f64>; HOT_CUES],
    held_hot_cue: Option<u8>,
    loop_in: Option<f64>,
    lp: Option<LoopInfo>,
    /// Loop that a roll or slice replaced (restored on release).
    saved_loop: Option<Option<LoopInfo>>,
    rolling: bool,
    slicer_held: Option<u8>,
    slip_on: bool,
    slip_pos: Option<f64>,
    deviation: u8,
    sync: SyncState,
}

impl Deck {
    pub(crate) fn new(sample_rate: u32, max_block: usize) -> Self {
        Self {
            track: None,
            track_serial: 0,
            position: 0.0,
            virt_offset: 0,
            playing: false,
            cue_point: 0.0,
            previewing: false,
            play_during_preview: false,
            pitch: 0.0,
            pitch_range: PitchRange::default(),
            bend: 0.0,
            jog_bend: 0.0,
            jog_free: false,
            key_lock: false,
            key_shift: 0,
            tempo: 1.0,
            // Tempo glides at most 1.0 per second (0.08 in 80 ms).
            tempo_step_per_frame: 1.0 / f64::from(sample_rate.max(1)),
            scrub: Scrub::default(),
            gain: LinearSmoother::with_time(0.0, sample_rate, FADE_MS),
            pending: Pending::None,
            stretcher: SignalsmithStretch::new(sample_rate),
            wsola: Wsola::new(sample_rate),
            use_wsola: false,
            brake_seconds: 0.0,
            braking: false,
            coast_rate: JOG_COAST_RATE,
            stutter_cue: false,
            resampler: SincResampler::new(max_block, MAX_SCRUB_SPEED + 1.0),
            loop_xfade: (f64::from(sample_rate) * 0.003) as i64,
            fader_last: None,
            // Until the controller reports where its fader is.
            fader_locked: true,
            kl_mix: 0.0,
            kl_step: 1.0 / (f64::from(sample_rate.max(1)) * KEY_LOCK_FADE_SECONDS) as f32,
            stretch_live: false,
            kl_warm: 0,
            kl_warm_len: (f64::from(sample_rate) * STRETCH_WARM_SECONDS) as usize,
            xf_l: vec![0.0; max_block],
            xf_r: vec![0.0; max_block],
            scrub_pos: vec![0.0; max_block],
            out_l: vec![0.0; max_block],
            out_r: vec![0.0; max_block],
            peak: [0.0; 2],
            sample_rate,
            grid: None,
            quantize: false,
            hot_cues: [None; HOT_CUES],
            held_hot_cue: None,
            loop_in: None,
            lp: None,
            saved_loop: None,
            rolling: false,
            slicer_held: None,
            slip_on: false,
            slip_pos: None,
            deviation: 0,
            sync: SyncState::default(),
        }
    }

    fn fader_tempo(&self) -> f64 {
        1.0 + self.pitch * self.pitch_range.fraction()
    }

    /// `p` within the pitch a deck can hold: the fader's travel is -1..1,
    /// but a deck that leaves sync keeps its tempo also beyond the fader's
    /// range (Serato's relative pitch: a 146 BPM track synced to 172.8 BPM
    /// is at +18.4 % on a ±8 % fader), up to the engine's tempo limits.
    fn clamp_pitch(&self, p: f64) -> f64 {
        let r = self.pitch_range.fraction().max(1e-6);
        p.clamp((MIN_TEMPO - 1.0) / r, (MAX_TEMPO - 1.0) / r)
    }

    /// The pitch is beyond where the fader can go (see [`Self::clamp_pitch`]).
    fn beyond_fader(&self) -> bool {
        self.pitch.abs() > 1.0
    }

    /// A new pitch range: an in-range pitch keeps its fader place, a pitch
    /// beyond the fader keeps its tempo.
    fn change_pitch_range(&mut self, r: PitchRange) {
        if self.beyond_fader() {
            let tempo = self.fader_tempo();
            self.pitch_range = r;
            self.pitch = self.clamp_pitch((tempo - 1.0) / r.fraction().max(1e-6));
            self.relock_fader();
        } else {
            self.pitch_range = r;
        }
    }

    /// Tempo before the bend: the fader, or the sync rate with its phase
    /// correction.
    fn base_tempo(&self) -> f64 {
        match self.sync.rate {
            Some(r) => r * (1.0 + self.sync.correction),
            None => self.fader_tempo(),
        }
    }

    fn target_tempo(&self) -> f64 {
        (self.base_tempo() + self.bend + self.jog_bend).clamp(MIN_TEMPO, MAX_TEMPO)
    }

    fn length(&self) -> f64 {
        self.track.as_ref().map_or(0.0, |t| t.len_frames() as f64)
    }

    fn audible(&self) -> bool {
        self.playing || self.gain.current() > 0.0 || !self.gain.is_settled()
    }

    fn stopping(&self) -> bool {
        matches!(self.pending, Pending::Stop { .. })
    }

    /// Playing (or previewing) and not about to stop.
    fn moving(&self) -> bool {
        self.playing && !self.stopping()
    }

    pub(crate) fn grid(&self) -> Option<DeckGrid> {
        self.grid
    }

    pub(crate) fn is_loaded(&self) -> bool {
        self.track.is_some()
    }

    pub(crate) fn sync_on(&self) -> bool {
        self.sync.on
    }

    pub(crate) fn is_moving(&self) -> bool {
        self.track.is_some() && self.moving()
    }

    /// The FX clock of this deck: (BPM, bar-aligned beat count) when it has
    /// a track with a grid. The BPM follows the pitch fader or sync, not a
    /// bend (like [`Self::master_beat`]).
    pub(crate) fn fx_beat(&self) -> Option<(f64, f64)> {
        let g = self.grid?;
        self.track.as_ref()?;
        Some((
            g.bpm(self.sample_rate) * self.base_tempo().clamp(0.1, 4.0),
            g.bar_beat(self.position),
        ))
    }

    /// The deck as a sync master: effective BPM and beat count.
    pub(crate) fn master_beat(&self) -> Option<MasterBeat> {
        let g = self.grid?;
        self.track.as_ref()?;
        Some(MasterBeat {
            // The tempo without the bend: a bend or a scrub of the master
            // nudges its phase and must not drag the synced decks along.
            bpm: g.bpm(self.sample_rate) * self.base_tempo().clamp(0.1, 4.0),
            beat: g.beat_at(self.position),
            playing: self.moving(),
            nudging: self.bend != 0.0 || self.jog_bend != 0.0 || self.scrub.active,
        })
    }

    /// Applies a command. Returns a track the caller must release off the
    /// audio thread (never dropped here, so no deallocation happens).
    pub(crate) fn command(&mut self, cmd: DeckCommand) -> Option<Arc<TrackAudio>> {
        match cmd {
            DeckCommand::Load { audio, serial } => {
                let old = self.track.replace(audio);
                self.track_serial = serial;
                self.reset_transport();
                return old;
            }
            DeckCommand::LoadWithMarkers {
                audio,
                serial,
                markers,
            } => {
                let old = self.track.replace(audio);
                self.track_serial = serial;
                self.reset_transport();
                self.grid = markers.grid;
                // Not clamped to the length: while loading it is an estimate.
                if let Some(c) = markers.cue_point.filter(|c| c.is_finite()) {
                    self.cue_point = c.max(0.0);
                }
                for (dst, src) in self.hot_cues.iter_mut().zip(markers.hot_cues) {
                    *dst = src.filter(|p| p.is_finite()).map(|p| p.max(0.0));
                }
                if let Some(s) = markers.start.filter(|s| s.is_finite()) {
                    self.position = s.max(0.0);
                }
                return old;
            }
            DeckCommand::Eject => {
                let old = self.track.take();
                self.track_serial = 0;
                self.reset_transport();
                return old;
            }
            DeckCommand::Play(on) => self.set_playing(on),
            DeckCommand::TogglePlay => {
                // During a CUE preview the deck plays only because CUE is
                // held: PLAY latches it (CDJ behaviour) instead of pausing.
                let latch = self.previewing && !self.play_during_preview;
                let on = latch || !self.moving();
                self.set_playing(on);
            }
            DeckCommand::Cue(pressed) => self.cue(pressed),
            DeckCommand::Seek(frame) => self.seek(frame),
            DeckCommand::SetPitch(p) => {
                // A synced follower takes its tempo from the master. Beyond
                // the fader's travel only when copied from a deck that is
                // there (instant doubles, a new audio output).
                if self.sync.rate.is_none() {
                    self.pitch = self.clamp_pitch(p);
                    self.relock_fader();
                }
            }
            DeckCommand::PitchFader(p) => {
                let p = p.clamp(-1.0, 1.0);
                let before = self.fader_last.replace(p);
                if self.sync.rate.is_none() {
                    if self.fader_locked && self.beyond_fader() {
                        // The pitch is beyond the fader's reach (kept from
                        // sync): the fader moves it relatively (Serato's
                        // relative pitch) until it is back within reach.
                        if let Some(b) = before {
                            self.pitch = self.clamp_pitch(self.pitch + (p - b));
                        }
                    } else if self.fader_locked {
                        // Taken over once the fader meets the pitch or moves
                        // across it (also in one big jump).
                        let crossed = before.is_some_and(|b| {
                            (b - self.pitch).signum() != (p - self.pitch).signum()
                        });
                        if crossed || (p - self.pitch).abs() < FADER_MATCH {
                            self.fader_locked = false;
                            self.pitch = p;
                        }
                    } else {
                        self.pitch = p;
                    }
                }
            }
            DeckCommand::PitchFaderSync(p) => {
                let p = p.clamp(-1.0, 1.0);
                self.fader_last = Some(p);
                // A pitch beyond the fader's reach is kept (the fader then
                // moves it relatively).
                if self.sync.rate.is_none() && !self.beyond_fader() {
                    self.pitch = p;
                    self.fader_locked = false;
                } else if self.sync.rate.is_none() {
                    self.relock_fader();
                }
            }
            DeckCommand::PitchTakeover(at) => {
                self.fader_last = at.map(|p| p.clamp(-1.0, 1.0));
                self.relock_fader();
            }
            DeckCommand::SetPitchRange(r) => self.change_pitch_range(r),
            DeckCommand::CyclePitchRange => {
                let i = PitchRange::ALL
                    .iter()
                    .position(|r| *r == self.pitch_range)
                    .unwrap_or(0);
                self.change_pitch_range(PitchRange::ALL[(i + 1) % PitchRange::ALL.len()]);
            }
            DeckCommand::PitchBend(b) => self.bend = b.clamp(-0.5, 0.5),
            DeckCommand::SetKeyLock(on) => self.set_key_lock(on),
            DeckCommand::Scrub { active, target } => self.scrub_cmd(active, target),
            DeckCommand::SetGrid(g) => self.grid = g,
            DeckCommand::SetCuePoint(p) => {
                if self.track.is_some() && p.is_finite() {
                    self.cue_point = self.clamp_pos(p);
                }
            }
            DeckCommand::HotCue {
                slot,
                pressed,
                loop_beats,
            } => self.hot_cue(usize::from(slot), pressed, loop_beats),
            DeckCommand::SetHotCue { slot, position } => {
                if let Some(c) = self.hot_cues.get_mut(usize::from(slot)) {
                    *c = position.filter(|p| p.is_finite()).map(|p| p.max(0.0));
                }
            }
            DeckCommand::LoopIn => self.loop_in_cmd(),
            DeckCommand::LoopOut => self.loop_out_cmd(),
            DeckCommand::Reloop => self.reloop(),
            DeckCommand::LoopExit => self.exit_loop(),
            DeckCommand::LoopHalve => self.resize_loop(0.5),
            DeckCommand::LoopDouble => self.resize_loop(2.0),
            DeckCommand::LoopMove(dir) => self.move_loop(f64::from(dir.signum())),
            DeckCommand::AutoLoop(beats) => self.auto_loop(beats),
            DeckCommand::SetLoop { start, end, jump } => self.set_loop(start, end, jump),
            DeckCommand::Roll { beats, pressed } => self.roll(beats, pressed),
            DeckCommand::Slicer {
                slice,
                pressed,
                domain_beats,
            } => self.slicer(slice, pressed, domain_beats),
            DeckCommand::BeatJump(beats) => self.beat_jump(beats),
            DeckCommand::SetSlip(on) => self.set_slip(on),
            DeckCommand::KeyShift(semis) => {
                self.key_shift = semis.clamp(-MAX_KEY_SHIFT, MAX_KEY_SHIFT);
                self.stretcher
                    .set_transpose_semitones(f64::from(self.key_shift));
                if self.key_shift != 0 {
                    self.set_key_lock(true);
                }
            }
            DeckCommand::ToggleSlip => self.set_slip(!self.slip_on),
            DeckCommand::ToggleKeyLock => {
                let on = match self.pending {
                    Pending::KeyLock(on) => on,
                    _ => self.key_lock,
                };
                self.set_key_lock(!on);
            }
            DeckCommand::JogTouch(true) => {
                // Grabbing a freely spinning platter holds it again.
                self.jog_free = false;
                self.braking = false;
                if self.track.is_some() && !self.scrub.active {
                    self.scrub_cmd(true, self.position);
                }
            }
            DeckCommand::JogTouch(false) => {
                if self.scrub.active {
                    let play = self.free_play_speed();
                    let off = self.scrub.velocity - play;
                    // The wheel itself must be turning (a held, still
                    // platter is released at once).
                    let spinning = self.scrub.velocity.abs() > JOG_FREE_MIN_SPEED;
                    if spinning && off.abs() > JOG_FREE_MIN_SPEED {
                        // Let go while spinning (a backspin, a throw): the
                        // deck keeps its speed and brakes back to play speed
                        // by itself, Pioneer style.
                        self.jog_free = true;
                        self.coast_rate = JOG_COAST_RATE;
                    } else {
                        self.scrub_cmd(false, 0.0);
                    }
                }
            }
            DeckCommand::JogScratch(turns) => {
                // After a release the wheel no longer drives the deck.
                if self.scrub.active && !self.jog_free && turns.is_finite() {
                    let frames = turns * JOG_SECONDS_PER_TURN * f64::from(self.sample_rate);
                    self.scrub.target = (self.scrub.target + frames).clamp(0.0, self.length());
                }
            }
            DeckCommand::JogNudge(turns) => {
                if !turns.is_finite() || self.track.is_none() || self.scrub.active {
                    return None;
                }
                if self.moving() {
                    self.jog_bend = (self.jog_bend + turns * JOG_NUDGE_PER_TURN).clamp(-0.5, 0.5);
                } else {
                    // Paused: the jog moves the track like a platter.
                    let frames = turns * JOG_SECONDS_PER_TURN * f64::from(self.sample_rate);
                    self.seek(self.position + frames);
                }
            }
            DeckCommand::SeekBy(frames) => {
                if frames.is_finite() {
                    self.seek(self.position + frames);
                }
            }
            DeckCommand::SetStopBrake(s) => {
                self.brake_seconds = if s.is_finite() {
                    s.clamp(0.0, 10.0)
                } else {
                    0.0
                };
            }
            DeckCommand::SetStutterCue(on) => self.stutter_cue = on,
            DeckCommand::SetKeyLockWsola(on) => {
                if on != self.use_wsola {
                    self.use_wsola = on;
                    self.restart_stretcher();
                }
            }
        }
        None
    }

    /// Pause with the stop brake: the platter runs on freely and slows down
    /// to a stop in `brake_seconds` (pitch falling, as on a turntable),
    /// then the deck pauses where it stopped.
    fn start_brake(&mut self) {
        let v = self.tempo;
        self.scrub = Scrub {
            active: true,
            target: self.position,
            prev_target: self.position,
            velocity: v,
            was_playing: false,
            follower: PlatterFollower {
                position: self.position,
                velocity: v,
            },
            pace: JogPace::default(),
        };
        self.jog_free = true;
        self.braking = true;
        self.coast_rate = v.abs() / f64::from(self.brake_seconds.max(0.01));
        self.pending = Pending::None;
        self.previewing = false;
        self.playing = false;
        self.gain.set_target(1.0);
    }

    fn reset_transport(&mut self) {
        self.position = 0.0;
        self.virt_offset = 0;
        self.cue_point = 0.0;
        self.playing = false;
        self.previewing = false;
        self.play_during_preview = false;
        self.pending = Pending::None;
        self.scrub = Scrub::default();
        self.gain.set_immediate(0.0);
        self.stretcher.reset();
        self.wsola.reset();
        self.grid = None;
        self.hot_cues = [None; HOT_CUES];
        self.held_hot_cue = None;
        self.loop_in = None;
        self.lp = None;
        self.saved_loop = None;
        self.rolling = false;
        self.slicer_held = None;
        self.slip_pos = None;
        self.deviation = 0;
        self.key_shift = 0;
        self.stretcher.set_transpose_semitones(0.0);
        self.sync.rate = None;
        self.sync.correction = 0.0;
        self.sync.offset = 0.0;
    }

    /// Restarts the time-stretcher after a jump. It needs about 75 ms
    /// before its output reaches full level, so while key lock is on the
    /// varispeed covers that time and then crossfades into it (before, a
    /// key-locked jump or key-lock switch had a short gap).
    fn restart_stretcher(&mut self) {
        self.stretcher.reset();
        self.wsola.reset();
        // Jumps happen under the deck's own short fade, so going back to
        // the varispeed at once is not heard.
        self.stretch_live = false;
        self.kl_mix = 0.0;
    }

    fn set_key_lock(&mut self, on: bool) {
        if on != self.key_lock {
            if self.audible()
                && !self.scrub.active
                && matches!(self.pending, Pending::None)
                && self.gain.is_settled()
            {
                // The render glides between the two paths (see `kl_mix`).
                self.key_lock = on;
            } else if self.audible() {
                self.pending = Pending::KeyLock(on);
                self.gain.set_target(0.0);
            } else {
                self.key_lock = on;
                self.restart_stretcher();
            }
        }
    }

    fn set_playing(&mut self, on: bool) {
        if self.track.is_none() {
            return;
        }
        if self.scrub.active && !self.braking {
            // PLAY / PAUSE while the platter is held or spinning free (a
            // backspin, a throw): the spin is heard to its end, which then
            // brakes to play speed or to a stop. Before, PAUSE cut the sound
            // at once and the deck played on after the backspin.
            let at_end = on
                && self.track.as_ref().is_some_and(|t| t.is_complete())
                && self.position >= self.length();
            if !at_end {
                self.scrub.was_playing = on;
                self.playing = on;
            }
            return;
        }
        if on {
            if self.braking && self.scrub.active {
                // PLAY during the stop brake: back to play speed from here.
                self.braking = false;
                self.jog_free = false;
                self.scrub.was_playing = true;
                self.scrub_cmd(false, 0.0);
            }
            if self.previewing {
                self.play_during_preview = true;
            }
            let at_end = self.track.as_ref().is_some_and(|t| t.is_complete())
                && self.position >= self.length();
            if at_end {
                return;
            }
            if !self.moving() && self.sync.on {
                self.sync.snap = true;
            }
            self.playing = true;
            if self.stopping() {
                self.pending = Pending::None;
            }
            if self.pending == Pending::None {
                self.gain.set_target(1.0);
            }
        } else if self.playing {
            if self.brake_seconds > 0.0
                && !self.previewing
                && !self.scrub.active
                && self.moving()
                && self.pending == Pending::None
            {
                self.start_brake();
                return;
            }
            self.previewing = false;
            self.pending = Pending::Stop { return_to: None };
            self.gain.set_target(0.0);
        }
    }

    fn cue(&mut self, pressed: bool) {
        if self.track.is_none() {
            return;
        }
        if pressed {
            let stopping = self.stopping();
            if self.playing && !self.previewing && !stopping && self.stutter_cue {
                // Stutter: back to the cue point, still playing.
                self.seek(self.cue_point);
            } else if self.playing && !self.previewing && !stopping {
                self.pending = Pending::Stop {
                    return_to: Some(self.cue_point),
                };
                self.gain.set_target(0.0);
            } else if !self.playing || stopping {
                if (self.position - self.cue_point).abs() > 1.0 && !stopping {
                    self.cue_point = self.clamp_pos(self.quantized(self.position));
                    if (self.cue_point - self.position).abs() > 0.5 {
                        // Snapped to the grid: preview from there.
                        self.position = self.cue_point;
                        self.restart_stretcher();
                    }
                }
                if stopping {
                    // Pressed again during the stop fade: finish the return
                    // first, then preview from the cue point.
                    self.pending = Pending::Seek(self.cue_point);
                }
                self.previewing = true;
                self.play_during_preview = false;
                self.playing = true;
                if self.pending == Pending::None {
                    self.gain.set_target(1.0);
                }
            }
        } else if self.previewing {
            self.previewing = false;
            if !self.play_during_preview {
                self.pending = Pending::Stop {
                    return_to: Some(self.cue_point),
                };
                self.gain.set_target(0.0);
            }
        }
    }

    /// `pos` on the beat grid when quantize is on and there is a grid.
    fn quantized(&self, pos: f64) -> f64 {
        match self.grid {
            Some(g) if self.quantize => g.round_to(pos, 1.0).max(0.0),
            _ => pos,
        }
    }

    fn clamp_pos(&self, p: f64) -> f64 {
        let len = self.length();
        if len > 0.0 {
            p.clamp(0.0, len)
        } else {
            p.max(0.0)
        }
    }

    /// `p` folded into the active loop if it lies past the loop's end.
    fn folded_into_loop(&self, p: f64) -> f64 {
        match self.lp.filter(|l| l.active).map(|l| l.frames()) {
            Some((s, len)) if len > 0 && p >= (s + len) as f64 => {
                s as f64 + (p - s as f64).rem_euclid(len as f64)
            }
            _ => p,
        }
    }

    fn seek(&mut self, frame: f64) {
        if self.track.is_none() {
            return;
        }
        let frame = self.clamp_pos(frame);
        if let Some(l) = self.lp
            && l.active
            && (frame < l.start || frame >= l.end)
        {
            self.exit_loop();
        }
        if self.audible() && !self.scrub.active {
            match self.pending {
                // Keep a pending stop, just change where it lands.
                Pending::Stop { .. } => {
                    self.pending = Pending::Stop {
                        return_to: Some(frame),
                    }
                }
                _ => self.pending = Pending::Seek(frame),
            }
            self.gain.set_target(0.0);
        } else {
            self.position = frame;
            self.restart_stretcher();
        }
    }

    /// Moves the play position by `delta` frames: after a fade while
    /// audible (the time spent fading still counts, so the beat phase is
    /// kept), at once otherwise.
    fn jump_by(&mut self, delta: f64) {
        if self.track.is_none() || delta == 0.0 {
            return;
        }
        if self.audible() && !self.scrub.active {
            self.pending = match self.pending {
                Pending::Stop { return_to } => Pending::Stop {
                    return_to: Some(return_to.unwrap_or(self.position) + delta),
                },
                Pending::Seek(p) => Pending::Seek(p + delta),
                Pending::Jump(d) => Pending::Jump(d + delta),
                Pending::KeyLock(on) => {
                    // Rare: switch key lock now, under the jump's fade.
                    self.key_lock = on;
                    Pending::Jump(delta)
                }
                Pending::SlipReturn => {
                    // A new jump wins over the return.
                    self.slip_pos = None;
                    Pending::Jump(delta)
                }
                Pending::None | Pending::Restart => Pending::Jump(delta),
            };
            self.gain.set_target(0.0);
        } else {
            self.position = self.clamp_pos(self.position + delta);
            self.restart_stretcher();
        }
    }

    // ----- slip ---------------------------------------------------------

    fn begin_deviation(&mut self, why: u8) {
        if self.track.is_none() {
            return;
        }
        let forced = why & DEV_FORCED != 0;
        if self.slip_pos.is_none() && (self.slip_on || forced) {
            self.slip_pos = Some(self.position);
            self.deviation = 0;
        }
        if self.slip_pos.is_some() {
            self.deviation |= why;
        }
    }

    fn end_deviation(&mut self, why: u8) {
        if self.deviation & why == 0 {
            return;
        }
        self.deviation &= !why;
        if self.deviation == 0 {
            self.slip_return();
        }
    }

    /// Goes back to the slip position (after a fade if audible), folded into
    /// a loop that is still active.
    fn slip_return(&mut self) {
        let Some(p) = self.slip_pos else {
            return;
        };
        self.deviation = 0;
        if self.audible() && !self.scrub.active {
            self.pending = Pending::SlipReturn;
            self.gain.set_target(0.0);
        } else {
            self.slip_pos = None;
            self.position = self.clamp_pos(self.folded_into_loop(p));
            self.restart_stretcher();
        }
    }

    fn set_slip(&mut self, on: bool) {
        if on == self.slip_on {
            return;
        }
        self.slip_on = on;
        if !on {
            // Leaving slip mode ends a slip in progress (rolls and the slicer
            // keep theirs).
            let keep = self.deviation & DEV_FORCED;
            if keep == 0 {
                self.slip_return();
            } else {
                self.deviation = keep;
            }
        }
    }

    // ----- hot cues -------------------------------------------------------

    fn hot_cue(&mut self, slot: usize, pressed: bool, loop_beats: f32) {
        if self.track.is_none() || slot >= HOT_CUES {
            return;
        }
        if !pressed {
            if self.held_hot_cue == Some(slot as u8) {
                self.held_hot_cue = None;
                self.end_deviation(DEV_HOT_CUE);
            }
            return;
        }
        let Some(cue) = self.hot_cues[slot] else {
            self.hot_cues[slot] = Some(self.clamp_pos(self.quantized(self.position)));
            return;
        };
        let mut target = cue;
        if self.quantize
            && self.moving()
            && let Some(g) = self.grid
        {
            // Keep the beat phase: land as far into the cue's beat as the
            // deck is into its current beat.
            target = cue + wrap_half(g.phase(self.position) - g.phase(cue)) * g.beat_len;
        }
        let target = self.clamp_pos(target);
        self.held_hot_cue = Some(slot as u8);
        if self.slip_on {
            self.begin_deviation(DEV_HOT_CUE);
        }
        if let Some(l) = self.lp
            && l.active
            && (target < l.start || target >= l.end)
        {
            self.exit_loop();
        }
        self.jump_by(target - self.position);
        if loop_beats > 0.0
            && let Some(g) = self.grid
        {
            let end = cue + f64::from(loop_beats) * g.beat_len;
            self.activate_loop(
                LoopInfo {
                    start: cue,
                    end,
                    active: true,
                    beats: loop_beats,
                },
                true,
                false,
            );
        }
        if !self.moving() {
            self.set_playing(true);
        }
    }

    // ----- loops ----------------------------------------------------------

    /// Makes `l` the active loop. `user` loops count as a slip deviation
    /// (rolls and slices handle their own). With `jump_inside`, a position
    /// already past the loop's end jumps into the loop.
    fn activate_loop(&mut self, mut l: LoopInfo, user: bool, jump_inside: bool) {
        if self.track.is_none() || l.len() < MIN_LOOP_FRAMES {
            return;
        }
        l.start = l.start.max(0.0);
        l.active = true;
        let old = self.lp;
        self.lp = Some(l);
        self.loop_in = None;
        if user && self.slip_on {
            self.begin_deviation(DEV_LOOP);
        }
        let (s, len) = l.frames();
        if jump_inside && self.position >= (s + len) as f64 {
            // An absolute seek: the loop wraps the position by itself during
            // the fade, so a relative jump went back one loop too far.
            let p = s as f64 + (self.position - s as f64).rem_euclid(len as f64);
            self.seek(p);
        } else {
            self.loop_changed(old);
        }
    }

    fn exit_loop(&mut self) {
        if let Some(l) = self.lp.as_mut()
            && l.active
        {
            l.active = false;
            let old = Some(LoopInfo { active: true, ..*l });
            self.loop_changed(old);
            self.end_deviation(DEV_LOOP);
        }
    }

    /// With key lock, the stretcher has read ahead through the old loop;
    /// restart it when the change is within its look-ahead.
    fn loop_changed(&mut self, old: Option<LoopInfo>) {
        if !self.key_lock || !self.audible() || self.pending != Pending::None {
            return;
        }
        let horizon = self.position + self.stretcher.lookahead(self.tempo) + 1024.0;
        let near = |l: Option<LoopInfo>| {
            l.filter(|l| l.active).is_some_and(|l| {
                let (s, len) = l.frames();
                let end = (s + len) as f64;
                end >= self.position - 1.0 && end <= horizon
            })
        };
        if near(old) || near(self.lp) {
            self.pending = Pending::Restart;
            self.gain.set_target(0.0);
        }
    }

    fn loop_in_cmd(&mut self) {
        if self.track.is_none() {
            return;
        }
        let p = self.quantized(self.position);
        match self.lp {
            Some(l) if l.active => {
                if p < l.end - MIN_LOOP_FRAMES {
                    let old = self.lp;
                    self.lp = Some(LoopInfo {
                        start: p,
                        beats: 0.0,
                        ..l
                    });
                    self.loop_changed(old);
                }
            }
            _ => self.loop_in = Some(p),
        }
    }

    fn loop_out_cmd(&mut self) {
        if self.track.is_none() {
            return;
        }
        let mut p = self.quantized(self.position);
        match (self.loop_in, self.lp) {
            (Some(start), _) => {
                if p <= start + MIN_LOOP_FRAMES
                    && let Some(g) = self.grid.filter(|_| self.quantize)
                {
                    p = start + g.beat_len;
                }
                if p > start + MIN_LOOP_FRAMES {
                    self.activate_loop(
                        LoopInfo {
                            start,
                            end: p,
                            active: true,
                            beats: 0.0,
                        },
                        true,
                        true,
                    );
                }
            }
            (None, Some(l)) if l.active && p > l.start + MIN_LOOP_FRAMES => {
                let old = self.lp;
                self.lp = Some(LoopInfo {
                    end: p,
                    beats: 0.0,
                    ..l
                });
                self.loop_changed(old);
            }
            _ => {}
        }
    }

    fn reloop(&mut self) {
        match self.lp {
            Some(l) if l.active => self.exit_loop(),
            Some(l) => {
                self.activate_loop(l, true, false);
                // Absolute, for the same reason as in `activate_loop`.
                self.seek(l.start);
            }
            None => {}
        }
    }

    fn resize_loop(&mut self, factor: f64) {
        let Some(l) = self.lp else {
            return;
        };
        let len = l.len() * factor;
        if len < MIN_LOOP_FRAMES || l.start + len > self.length() {
            return;
        }
        let beats = if l.beats > 0.0 {
            l.beats * factor as f32
        } else {
            0.0
        };
        let new = LoopInfo {
            end: l.start + len,
            beats,
            ..l
        };
        self.lp = Some(new);
        if new.active {
            let (s, n) = new.frames();
            if self.position >= (s + n) as f64 {
                // Absolute (see `activate_loop`): halving in the second half
                // of a loop went one loop too far back.
                let p = s as f64 + (self.position - s as f64).rem_euclid(n as f64);
                self.seek(p);
            } else {
                self.loop_changed(Some(l));
            }
        }
    }

    fn move_loop(&mut self, dir: f64) {
        let Some(l) = self.lp else {
            return;
        };
        let shift = l.len() * dir;
        if l.start + shift < 0.0 || l.end + shift > self.length() {
            return;
        }
        self.lp = Some(LoopInfo {
            start: l.start + shift,
            end: l.end + shift,
            ..l
        });
        if l.active {
            self.jump_by(shift);
        }
    }

    fn auto_loop(&mut self, beats: f32) {
        let (Some(g), true) = (self.grid, self.track.is_some()) else {
            return;
        };
        if beats <= 0.0 {
            return;
        }
        let len = f64::from(beats) * g.beat_len;
        if let Some(l) = self.lp
            && l.active
            && (l.len() - len).abs() < 2.0
            && l.beats > 0.0
        {
            self.exit_loop();
            return;
        }
        let start = if self.quantize {
            g.round_to(self.position, f64::from(beats).min(1.0))
        } else {
            self.position
        };
        self.activate_loop(
            LoopInfo {
                start,
                end: start + len,
                active: true,
                beats,
            },
            true,
            true,
        );
    }

    fn set_loop(&mut self, start: f64, end: f64, jump: bool) {
        if self.track.is_none() || !(start.is_finite() && end.is_finite()) {
            return;
        }
        // Loops of a power-of-two number of beats keep their beat count.
        let beats = self.grid.map_or(0.0, |g| {
            let b = (end - start) / g.beat_len;
            let lg = b.log2();
            if (lg - lg.round()).abs() < 1e-3 {
                b as f32
            } else {
                0.0
            }
        });
        self.activate_loop(
            LoopInfo {
                start,
                end,
                active: true,
                beats,
            },
            true,
            !jump,
        );
        if jump {
            self.jump_by(start.max(0.0) - self.position);
        }
    }

    /// Starts a momentary loop (roll or slice) with slip.
    fn momentary_loop(&mut self, why: u8, l: LoopInfo) {
        if self.saved_loop.is_none() {
            self.saved_loop = Some(self.lp);
        }
        self.begin_deviation(why);
        let old = self.lp;
        self.lp = Some(l);
        self.loop_changed(old);
    }

    /// Ends a roll or slice: the loop it replaced comes back, and the deck
    /// goes to where it would have been (inside that loop if it is active).
    fn end_momentary_loop(&mut self, why: u8) {
        if let Some(prev) = self.saved_loop.take() {
            self.lp = prev;
        }
        if self.deviation & why == 0 {
            return;
        }
        self.deviation &= !why;
        let Some(shadow) = self.slip_pos else {
            return;
        };
        if self.deviation == 0 {
            self.slip_return();
        } else {
            // Slip mode is still holding the deck (a looping saved/manual
            // loop): go back into that loop, keep the slip position running.
            let target = self.folded_into_loop(shadow);
            self.jump_by(target - self.position);
        }
    }

    fn roll(&mut self, beats: f32, pressed: bool) {
        if !pressed {
            if self.rolling {
                self.rolling = false;
                self.end_momentary_loop(DEV_ROLL);
            }
            return;
        }
        let (Some(g), true) = (self.grid, self.track.is_some()) else {
            return;
        };
        if beats <= 0.0 {
            return;
        }
        let len = f64::from(beats) * g.beat_len;
        let start = if self.quantize {
            g.floor_to(self.position, f64::from(beats).min(1.0))
        } else {
            self.position
        };
        self.rolling = true;
        self.momentary_loop(
            DEV_ROLL,
            LoopInfo {
                start,
                end: start + len,
                active: true,
                beats,
            },
        );
    }

    fn slicer(&mut self, slice: u8, pressed: bool, domain_beats: f32) {
        if !pressed {
            if self.slicer_held == Some(slice) {
                self.slicer_held = None;
                self.end_momentary_loop(DEV_SLICER);
            }
            return;
        }
        let (Some(g), true) = (self.grid, self.track.is_some()) else {
            return;
        };
        let domain = f64::from(domain_beats.max(1.0));
        let slice_beats = domain / HOT_CUES as f64;
        // The domain follows the position the deck would be at.
        let reference = self.slip_pos.unwrap_or(self.position);
        let b = g.bar_beat(reference);
        let domain_start = (b / domain).floor() * domain;
        let into_slice = b - (b / slice_beats).floor() * slice_beats;
        let slice_start = domain_start + f64::from(slice.min(7)) * slice_beats;
        let start = g.frame_of_bar_beat(slice_start);
        let target = g.frame_of_bar_beat(slice_start + into_slice);
        self.slicer_held = Some(slice);
        self.momentary_loop(
            DEV_SLICER,
            LoopInfo {
                start,
                end: start + slice_beats * g.beat_len,
                active: true,
                beats: slice_beats as f32,
            },
        );
        self.jump_by(target - self.position);
    }

    fn beat_jump(&mut self, beats: f64) {
        let (Some(g), true) = (self.grid, self.track.is_some()) else {
            return;
        };
        let delta = beats * g.beat_len;
        let target = self.position + delta;
        if target < 0.0 || target > self.length() {
            return;
        }
        if let Some(l) = self.lp.as_mut()
            && l.active
        {
            l.start += delta;
            l.end += delta;
        }
        self.jump_by(delta);
    }

    fn scrub_cmd(&mut self, active: bool, target: f64) {
        if self.track.is_none() {
            return;
        }
        let target = target.clamp(0.0, self.length().max(0.0));
        if active {
            if !self.scrub.active {
                let was_playing = self.moving();
                if self.slip_on && was_playing {
                    self.begin_deviation(DEV_SCRUB);
                }
                // The platter starts at the play speed and is braked by the
                // hand, rather than stopping in zero time.
                let v0 = if was_playing { self.tempo } else { 0.0 };
                self.scrub = Scrub {
                    active: true,
                    target,
                    prev_target: target,
                    velocity: v0,
                    was_playing,
                    follower: PlatterFollower {
                        position: self.position,
                        velocity: v0,
                    },
                    pace: JogPace::default(),
                };
                self.pending = Pending::None;
                self.previewing = false;
                self.gain.set_target(1.0);
            }
            self.scrub.target = target;
        } else if self.scrub.active {
            self.scrub.active = false;
            self.restart_stretcher();
            if self.scrub.was_playing {
                self.playing = true;
                self.gain.set_target(1.0);
            } else {
                self.playing = false;
                self.gain.set_target(0.0);
            }
            self.end_deviation(DEV_SCRUB);
        }
    }

    // ----- sync -------------------------------------------------------------

    /// Turns SYNC on or off. Returns false when the deck cannot sync (no
    /// track or no grid).
    pub(crate) fn set_sync(&mut self, on: bool) -> bool {
        if on {
            if self.grid.is_none() || self.track.is_none() {
                return false;
            }
            if !self.sync.on {
                self.sync.on = true;
                self.sync.offset = 0.0;
                self.sync.snap = true;
            }
        } else if self.sync.on {
            self.sync.on = false;
            self.leave_follow();
        }
        true
    }

    /// The pitch changed without the controller fader: lock the fader until
    /// it crosses the new pitch (unless it is already there).
    fn relock_fader(&mut self) {
        self.fader_locked = !self
            .fader_last
            .is_some_and(|f| (f - self.pitch).abs() < FADER_MATCH);
    }

    /// Stops following (the deck becomes the master, or SYNC goes off): it
    /// keeps its tempo, also beyond the fader's range (it used to be cut to
    /// the range: a 146 BPM track synced to 172.8 BPM dropped to +8 %,
    /// 157.7 BPM), and the pitch fader takes over from there.
    pub(crate) fn leave_follow(&mut self) {
        if let Some(r) = self.sync.rate.take() {
            let range = self.pitch_range.fraction();
            self.pitch = self.clamp_pitch((r - 1.0) / range);
            // SYNC moved the pitch: the fader picks it up again (Serato).
            self.relock_fader();
        }
        self.sync.correction = 0.0;
    }

    /// Called by the engine before each block for a synced deck that is not
    /// the master. `master` is `None` when there is no usable master.
    pub(crate) fn follow(&mut self, master: Option<MasterBeat>, mode: SyncMode) {
        let (Some(g), Some(m)) = (self.grid, master) else {
            self.sync.correction = 0.0;
            return;
        };
        let own_bpm = g.bpm(self.sample_rate);
        if !(own_bpm > 0.0 && m.bpm > 0.0) {
            return;
        }
        // Half or double time when that is closer to the master.
        let ratio = m.bpm / own_bpm;
        let k = (-ratio.log2()).round().clamp(-2.0, 2.0);
        let rate = ratio * 2f64.powf(k);
        self.sync.rate = Some(rate);
        let range = self.pitch_range.fraction();
        self.pitch = ((rate - 1.0) / range).clamp(-1.0, 1.0);

        if mode != SyncMode::Phase || !m.playing {
            self.sync.correction = 0.0;
            self.sync.snap = false;
            return;
        }
        // The master's beat count in this deck's beats.
        let master_beat = m.beat * 2f64.powf(k);
        let err = wrap_half(g.beat_at(self.position) - master_beat - self.sync.offset);
        if self.bend != 0.0 || self.jog_bend != 0.0 || m.nudging {
            self.sync.bending = true;
            self.sync.correction = 0.0;
            return;
        }
        if self.sync.bending {
            // The user moved the phase by bending: keep it there.
            self.sync.bending = false;
            self.sync.offset = wrap_half(self.sync.offset + err);
            self.sync.correction = 0.0;
            return;
        }
        if !self.moving() || self.scrub.active || self.slip_pos.is_some() {
            self.sync.correction = 0.0;
            return;
        }
        if self.sync.snap {
            self.sync.snap = false;
            if err.abs() > 0.002 {
                let delta = -err * g.beat_len;
                if self.gain.current() == 0.0 && self.pending == Pending::None {
                    // Just starting (still silent): move before the fade-in.
                    self.position = self.clamp_pos(self.position + delta);
                    self.restart_stretcher();
                } else {
                    self.jump_by(delta);
                }
            }
            self.sync.correction = 0.0;
            return;
        }
        self.sync.correction = (-err * SYNC_GAIN).clamp(-SYNC_MAX_CORRECTION, SYNC_MAX_CORRECTION);
    }

    // ----- rendering ------------------------------------------------------------

    /// Renders `n` frames into `out_l` / `out_r`.
    /// Speed the deck plays at after a jog release: its tempo if it was
    /// playing before the scratch, else 0.
    fn free_play_speed(&self) -> f64 {
        if self.scrub.was_playing {
            self.tempo
        } else {
            0.0
        }
    }

    pub(crate) fn render(&mut self, n: usize) {
        let n = n.min(self.out_l.len()).min(self.out_r.len());
        // Tempo glides towards the target.
        let target = self.target_tempo();
        let max_step = self.tempo_step_per_frame * n as f64;
        // The varispeed ramps from the old tempo to the new one within the
        // block, so a tempo change never steps at a block edge.
        let tempo_before = self.tempo;
        self.tempo += (target - self.tempo).clamp(-max_step, max_step);
        // A released spinning deck that has braked back to play speed (or
        // to a stop) plays on normally.
        if self.jog_free && self.scrub.active {
            let play = self.free_play_speed();
            if (self.scrub.follower.velocity - play).abs() < 0.005 {
                if play == 0.0 && (self.gain.current() > 0.0 || !self.gain.is_settled()) {
                    // Braked to a stop: fade out standing still first, or
                    // the fade would play a blip forwards at the tempo.
                    self.gain.set_target(0.0);
                } else {
                    self.jog_free = false;
                    self.scrub_cmd(false, 0.0);
                }
            }
        } else if !self.scrub.active {
            self.jog_free = false;
        }
        // A jog nudge fades out by itself.
        if self.jog_bend != 0.0 {
            self.jog_bend *=
                (-(n as f64) / (JOG_NUDGE_DECAY_SECONDS * f64::from(self.sample_rate))).exp();
            if self.jog_bend.abs() < 1e-5 {
                self.jog_bend = 0.0;
            }
        }
        let audible = self.audible();

        let (out_l, out_r) = (&mut self.out_l[..n], &mut self.out_r[..n]);
        out_l.fill(0.0);
        out_r.fill(0.0);
        let Some(track) = self.track.as_deref() else {
            return;
        };
        let len = track.len_frames() as f64;
        let complete = track.is_complete();
        let active_loop = self.lp.filter(|l| l.active).map(|l| l.frames());

        // The slip position runs on as if nothing had happened.
        let forward = if self.scrub.active {
            self.scrub.was_playing
        } else {
            self.playing && !matches!(self.pending, Pending::Stop { .. })
        };
        if forward && let Some(p) = self.slip_pos.as_mut() {
            *p = (*p + self.tempo * n as f64).min(len.max(0.0));
        }

        if self.scrub.active {
            // The jog ticks of this block are spread evenly over it, and a
            // critically damped platter follows them frame by frame: the
            // speed changes smoothly instead of jumping once per block. A
            // released, spinning jog brakes back to play speed by itself.
            let sr = f64::from(self.sample_rate);
            if self.jog_free {
                // Released while spinning: brake (or speed up) towards play
                // speed at a steady rate, easing in at the end, whatever the
                // wheel does (a Pioneer-style brake).
                let play = if self.scrub.was_playing {
                    self.tempo
                } else {
                    0.0
                };
                let rate = self.coast_rate / sr;
                let ease = 1.0 / (JOG_COAST_EASE_SECONDS * sr);
                let f = &mut self.scrub.follower;
                for p in self.scrub_pos[..n].iter_mut() {
                    *p = f.position;
                    let d = play - f.velocity;
                    f.velocity += d.signum() * rate.min(d.abs() * ease);
                    f.position += f.velocity;
                }
                self.scrub.target = f.position;
                self.scrub.prev_target = f.position;
            } else {
                let (t0, t1) = (self.scrub.prev_target, self.scrub.target);
                // Slow movements: follow over about the time between jog
                // steps.
                let tau = self.scrub.pace.update(
                    t1 != t0,
                    n,
                    sr * SCRATCH_TAU_SECONDS,
                    sr * SCRATCH_MAX_TAU_SECONDS,
                );
                let step = (t1 - t0) / n.max(1) as f64;
                for (i, p) in self.scrub_pos[..n].iter_mut().enumerate() {
                    let target = t0 + step * (i + 1) as f64;
                    *p = self.scrub.follower.step(target, tau, MAX_SCRUB_SPEED);
                }
                self.scrub.prev_target = t1;
            }
            self.scrub.velocity = self.scrub.follower.velocity;
            if self.scrub.follower.position < 0.0 {
                self.scrub.follower.position = 0.0;
                self.scrub.follower.velocity = 0.0;
            }
            let src = DeckSource {
                track,
                offset: 0,
                lp: None,
                xfade: 0,
            };
            self.resampler
                .render_positions(&src, &self.scrub_pos[..n], out_l, out_r);
            self.position = self.scrub.follower.position.max(0.0);
            self.stretch_live = false;
        } else if audible {
            let src = DeckSource {
                track,
                offset: self.virt_offset,
                lp: active_loop,
                xfade: self.loop_xfade,
            };
            let virt = self.position + self.virt_offset as f64;
            // Key lock: `kl_mix` glides between the varispeed (0) and the
            // stretcher (1), always towards the key-lock setting, so
            // switching (even rapidly) never steps. After a (re)start the
            // stretcher is silent for a while, so it warms up unheard first.
            // The stretcher runs on every playing deck, key lock on or off,
            // so switching is only the glide.
            let run_stretch = true;
            if run_stretch && !self.stretch_live {
                // (Re)starting: silent at first, so the varispeed is heard.
                self.kl_warm = self.kl_warm_len;
                self.kl_mix = 0.0;
            }
            let target = if self.key_lock && self.kl_warm == 0 {
                1.0
            } else if self.key_lock {
                self.kl_mix
            } else {
                0.0
            };
            let mix_end = if target > self.kl_mix {
                (self.kl_mix + self.kl_step * n as f32).min(target)
            } else {
                (self.kl_mix - self.kl_step * n as f32).max(target)
            };
            let need_vari = self.kl_mix < 1.0 || mix_end < 1.0;
            if run_stretch {
                let (sl, sr) = (&mut self.xf_l[..n], &mut self.xf_r[..n]);
                if self.use_wsola {
                    self.wsola.process(&src, virt, self.tempo, sl, sr);
                } else {
                    self.stretcher.process(&src, virt, self.tempo, sl, sr);
                }
                self.position += self.tempo * n as f64;
                if need_vari {
                    // Same speed as the stretcher, so both read the same
                    // positions.
                    self.resampler
                        .render(&src, virt, self.tempo, self.tempo, out_l, out_r);
                }
            } else {
                self.resampler
                    .render(&src, virt, tempo_before, self.tempo, out_l, out_r);
                self.position += ramp_advance(tempo_before, self.tempo, n);
            }
            self.stretch_live = run_stretch;
            if run_stretch {
                let step = (mix_end - self.kl_mix) / n.max(1) as f32;
                let mut m = self.kl_mix;
                for (((l, r), a), b) in out_l
                    .iter_mut()
                    .zip(out_r.iter_mut())
                    .zip(self.xf_l[..n].iter())
                    .zip(self.xf_r[..n].iter())
                {
                    m += step;
                    if m >= 1.0 {
                        *l = *a;
                        *r = *b;
                    } else if m > 0.0 {
                        let t = m * std::f32::consts::FRAC_PI_2;
                        let (g_s, g_v) = (t.sin(), t.cos());
                        *l = *l * g_v + a * g_s;
                        *r = *r * g_v + b * g_s;
                    }
                }
                self.kl_mix = mix_end;
                self.kl_warm = self.kl_warm.saturating_sub(n);
            } else {
                self.kl_mix = 0.0;
                self.kl_warm = 0;
            }
            if let Some((s, l)) = active_loop
                && l > 0
                && self.position >= (s + l) as f64
            {
                let wraps = ((self.position - s as f64) / l as f64).floor() as i64;
                self.position -= (wraps * l) as f64;
                self.virt_offset += wraps * l;
            }
        } else {
            self.peak = [0.0; 2];
            self.stretch_live = false;
            return;
        }

        let mut peak = [0.0f32; 2];
        for (l, r) in out_l.iter_mut().zip(out_r.iter_mut()) {
            let g = self.gain.next_value();
            *l *= g;
            *r *= g;
            peak[0] = peak[0].max(l.abs());
            peak[1] = peak[1].max(r.abs());
        }
        self.peak = [self.peak[0].max(peak[0]), self.peak[1].max(peak[1])];

        if self.gain.is_settled() && self.gain.current() == 0.0 {
            self.apply_pending();
        }

        if complete && self.position >= len && !self.scrub.active {
            self.position = len;
            self.playing = false;
            self.previewing = false;
            self.pending = Pending::None;
            self.gain.set_immediate(0.0);
            self.slip_pos = None;
            self.deviation = 0;
        }
    }

    fn apply_pending(&mut self) {
        match std::mem::replace(&mut self.pending, Pending::None) {
            Pending::None => {
                if self.playing {
                    self.gain.set_target(1.0);
                }
            }
            Pending::Seek(p) => {
                self.position = self.clamp_pos(p);
                self.after_jump();
            }
            Pending::Jump(d) => {
                self.position = self.clamp_pos(self.position + d);
                self.after_jump();
            }
            Pending::SlipReturn => {
                if let Some(p) = self.slip_pos.take() {
                    self.position = self.clamp_pos(self.folded_into_loop(p));
                }
                self.after_jump();
            }
            Pending::Restart => self.after_jump(),
            Pending::Stop { return_to } => {
                self.playing = false;
                if let Some(p) = return_to {
                    self.position = self.clamp_pos(p);
                }
                self.restart_stretcher();
            }
            Pending::KeyLock(on) => {
                self.key_lock = on;
                self.after_jump();
            }
        }
    }

    fn after_jump(&mut self) {
        self.restart_stretcher();
        if self.playing {
            self.gain.set_target(1.0);
        }
    }

    pub(crate) fn snapshot(&mut self) -> DeckSnapshot {
        DeckSnapshot {
            track_serial: self.track_serial,
            loaded: self.track.is_some(),
            position: self.position,
            length: self.length(),
            ready_frames: self.track.as_ref().map_or(0, |t| t.ready_frames()),
            load_complete: self.track.as_ref().is_some_and(|t| t.is_complete()),
            playing: self.moving(),
            previewing: self.previewing,
            cue_point: self.cue_point,
            tempo: self.tempo,
            set_tempo: (self.base_tempo() + self.bend).clamp(MIN_TEMPO, MAX_TEMPO),
            pitch_tempo: self
                .sync
                .rate
                .unwrap_or_else(|| self.fader_tempo())
                .clamp(0.1, 4.0),
            speed: if self.scrub.active {
                self.scrub.velocity
            } else {
                self.tempo
            },
            pitch: self.pitch,
            pitch_range: self.pitch_range,
            bend: self.bend,
            key_lock: match self.pending {
                Pending::KeyLock(on) => on,
                _ => self.key_lock,
            },
            scrubbing: self.scrub.active,
            peak: self.peak,
            grid: self.grid,
            hot_cues: self.hot_cues,
            held_hot_cue: self.held_hot_cue,
            loop_in: self.loop_in,
            loop_info: self.lp,
            rolling: self.rolling,
            slicer_held: self.slicer_held,
            slip: self.slip_on,
            slip_position: self.slip_pos,
            sync: self.sync.on,
            sync_rate: self.sync.rate,
            key_shift: self.key_shift,
            pitch_fader_ghost: if self.fader_locked && self.sync.rate.is_none() {
                self.fader_last
            } else {
                None
            },
            quantize: self.quantize,
        }
    }

    pub(crate) fn clear_peak(&mut self) {
        self.peak = [0.0; 2];
    }
}
