//! Application configuration. Serialised as TOML by `dj-app`; this module only
//! defines the types and defaults.

use crate::key::KeyNotation;
use crate::routing::OutputRouting;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod keymap;
mod midi_learn;

pub use keymap::{KeyAction, KeyBinding, default_keymap, key_label, merge_default_keymap};
pub use midi_learn::{FunctionKind, LearnedBinding, MidiFunction};

/// Top-level configuration file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub audio: AudioConfig,
    pub ui: UiConfig,
    pub library: LibraryConfig,
    pub performance: PerformanceConfig,
    pub controller: ControllerConfig,
    pub recording: RecordingConfig,
    pub fx: FxConfig,
    pub mixer: MixerConfig,
}

/// Mixer setup (Setup > Mixer): how the EQ, filter, faders and limiter
/// work. Not the knob and fader positions, which come from the controller
/// or the screen. Names are the ids of the curves in `dj-dsp`; unknown ones
/// fall back to the defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MixerConfig {
    /// "isolator" (full kill, Linkwitz-Riley) or "classic" (shelves and a
    /// bell).
    pub eq_mode: String,
    /// Most an EQ knob boosts: 6 or 12 dB.
    pub eq_boost_db: f32,
    /// EQ crossover frequencies (low/mid and mid/high) in Hz.
    pub eq_low_hz: f32,
    pub eq_high_hz: f32,
    /// Channel filter slope: "12" or "24" (dB per octave).
    pub filter_slope: String,
    /// Channel filter resonance, 0 to 1.
    pub filter_resonance: f32,
    /// "club", "smooth", "linear" or "sharp".
    pub fader_curve: String,
    /// "smooth", "dip", "linear", "sharp" or "cut".
    pub crossfader_curve: String,
    pub crossfader_reverse: bool,
    /// Scratch cut-in distance of the "cut" curve (0.005 to 0.05 of the
    /// crossfader's -1..1 travel).
    pub cut_lag: f32,
    pub limiter: bool,
    /// Limiter ceiling in dBFS (-2.0 to -0.1).
    pub limiter_ceiling_db: f32,
    /// How long the meters' peak lines stay up, in seconds.
    pub meter_hold_seconds: f32,
    /// Headphone cue taken before the channel EQ and filter (off: after).
    pub cue_pre_eq: bool,
    /// Crossfader side of each channel at start: "a", "thru" or "b".
    pub crossfader_assign: Vec<String>,
    /// Settings version: older files get the newer defaults where those
    /// changed (missing = 0).
    #[serde(default)]
    pub version: u32,
}

impl MixerConfig {
    /// 1: the first `[mixer]`. 2: softer filter resonance
    /// default. 3: the filter's resonance is now its peak at full
    /// turn, tapering to none at the centre, default 0.77.
    /// 4: the filter follows the measured reference: 24 dB/oct,
    /// resonance 0.72.
    /// 5: the EQ follows the measured reference: crossovers 150 Hz
    /// / 4 kHz (were 250 Hz / 2.5 kHz).
    pub const VERSION: u32 = 5;

    /// Brings settings saved by an older version up to date: values that
    /// were only the old defaults move to the new ones. Returns whether a
    /// setting changed.
    pub fn migrate(&mut self) -> bool {
        let mut changed = false;
        if self.version < 2 && (self.filter_resonance - 0.75).abs() < 1e-4 {
            self.filter_resonance = MixerConfig::default().filter_resonance;
            changed = true;
        }
        if self.version < 3 && (self.filter_resonance - 0.5).abs() < 1e-4 {
            self.filter_resonance = MixerConfig::default().filter_resonance;
            changed = true;
        }
        if self.version < 4 {
            if (self.filter_resonance - 0.77).abs() < 1e-4 {
                self.filter_resonance = MixerConfig::default().filter_resonance;
                changed = true;
            }
            if self.filter_slope == "12" {
                self.filter_slope = MixerConfig::default().filter_slope;
                changed = true;
            }
        }
        if self.version < 5 && self.eq_low_hz == 250.0 && self.eq_high_hz == 2500.0 {
            let d = MixerConfig::default();
            self.eq_low_hz = d.eq_low_hz;
            self.eq_high_hz = d.eq_high_hz;
            changed = true;
        }
        self.version = Self::VERSION;
        changed
    }
}

impl Default for MixerConfig {
    fn default() -> Self {
        Self {
            eq_mode: "isolator".into(),
            eq_boost_db: 6.0,
            eq_low_hz: 150.0,
            eq_high_hz: 4000.0,
            filter_slope: "24".into(),
            filter_resonance: 0.72,
            fader_curve: "club".into(),
            crossfader_curve: "smooth".into(),
            crossfader_reverse: false,
            cut_lag: 0.03,
            limiter: true,
            limiter_ceiling_db: -0.3,
            meter_hold_seconds: 0.8,
            cue_pre_eq: false,
            crossfader_assign: vec!["thru".into(); 4],
            version: MixerConfig::VERSION,
        }
    }
}

/// Audio device settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Audio host name (for example "WASAPI" or "ASIO"). `None` uses the
    /// platform default host.
    pub host: Option<String>,
    /// Output device name or id. `None` uses the default output device.
    pub device: Option<String>,
    /// Requested sample rate in Hz.
    pub sample_rate: u32,
    /// Requested buffer size in frames (64 to 1024).
    pub buffer_frames: u32,
    pub routing: OutputRouting,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            host: None,
            device: None,
            sample_rate: 48_000,
            buffer_frames: 256,
            routing: OutputRouting::default(),
        }
    }
}

impl AudioConfig {
    /// Smallest and largest buffer sizes offered in settings.
    pub const BUFFER_RANGE: (u32, u32) = (64, 1024);

    /// Returns a copy with the buffer size clamped into [`Self::BUFFER_RANGE`].
    pub fn sanitized(&self) -> Self {
        let mut c = self.clone();
        c.buffer_frames = c
            .buffer_frames
            .clamp(Self::BUFFER_RANGE.0, Self::BUFFER_RANGE.1);
        if c.sample_rate == 0 {
            c.sample_rate = 48_000;
        }
        c
    }
}

/// Deck layout on screen. How many decks it shows (2 or 4) is
/// `UiConfig::two_decks`; `TwoDecks` is the old "2 decks" layout, read from
/// older config files and shown as Stacked with 2 decks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DeckLayout {
    TwoDecks,
    FourStacked,
    FourGrid,
    /// Four deck panels in a 2x2 grid across the top (1 and 3 left, 2 and
    /// 4 right), four full-width waveforms under them.
    #[default]
    Extended,
}

/// Waveform colour mode.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WaveformColorMode {
    /// Low, mid and high band energy mapped to red, green and blue.
    #[default]
    ThreeBand,
    Monochrome,
    /// Blue tones with brightness by high-frequency content.
    Blue,
}

/// Reads a list, dropping entries that do not parse (key actions or MIDI
/// learn functions that a newer version added).
fn lenient_list<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Entry<E> {
        Known(E),
        Unknown(serde::de::IgnoredAny),
    }
    let entries = Vec::<Entry<T>>::deserialize(d)?;
    Ok(entries
        .into_iter()
        .filter_map(|e| match e {
            Entry::Known(b) => Some(b),
            Entry::Unknown(_) => None,
        })
        .collect())
}

/// What the area under the waveforms shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CentrePanel {
    #[default]
    Mixer,
    /// Pads, loops, beat jump, key shift and grid editing of the LAYER decks.
    Pads,
    /// The two FX units.
    Fx,
}

/// Master recording.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecordingConfig {
    /// Folder for recordings. `None`: `Music/Kaldor Recordings` in the
    /// user's home folder.
    pub folder: Option<PathBuf>,
    pub format: RecordFormat,
    /// File name: `{date}` (2026-11-15), `{time}` (21-30-05) and `{n}`
    /// (number of the recording this session) are filled in. Empty: the
    /// default `Kaldor {date} {time}`.
    pub name_pattern: String,
    /// Start a new file every this many minutes (0: one file).
    pub split_minutes: u32,
    /// Add each finished recording to the library, in the crate
    /// "Recordings".
    pub add_to_library: bool,
}

impl RecordingConfig {
    pub const DEFAULT_PATTERN: &'static str = "Kaldor {date} {time}";
}

/// File format of master recordings (both 24-bit).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordFormat {
    #[default]
    Wav,
    Flac,
}

impl RecordFormat {
    pub fn extension(self) -> &'static str {
        match self {
            RecordFormat::Wav => "wav",
            RecordFormat::Flac => "flac",
        }
    }
}

/// FX settings: the two units as last set up, and saved presets.
/// Effects are named by their ids (`echo`, `reverb`, ...); unknown ones
/// are skipped when loading.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FxConfig {
    /// One entry per unit (missing ones keep their defaults).
    pub units: Vec<FxUnitConfig>,
    pub presets: Vec<FxPreset>,
    /// Settings version (missing = 0, see [`FxConfig::migrate`]).
    pub version: u32,
}

impl FxConfig {
    /// 2: WET/DRY's middle is the track and the effects at full level, fully
    /// wet only the effects; before, 1.0 was the track
    /// and the effects at full level.
    pub const VERSION: u32 = 2;

    /// Brings settings saved by an older version up to date, so they sound
    /// the same: an old WET/DRY value `m` is `m / 2` now. Returns whether a
    /// setting changed.
    pub fn migrate(&mut self) -> bool {
        let mut changed = false;
        if self.version < 2 {
            for u in self
                .units
                .iter_mut()
                .chain(self.presets.iter_mut().map(|p| &mut p.unit))
            {
                u.mix *= 0.5;
                changed = true;
            }
        }
        self.version = Self::VERSION;
        changed
    }
}

/// One FX unit's setup (not whether it is on).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FxUnitConfig {
    /// "multi" or "single".
    pub mode: String,
    /// Decks 1-4 and the master.
    pub assign: Vec<bool>,
    /// Beats setting (1/16 to 32).
    pub beats: f64,
    /// Wet/dry, 0 to 1.
    pub mix: f32,
    pub slots: Vec<FxSlotConfig>,
}

impl Default for FxUnitConfig {
    fn default() -> Self {
        Self {
            mode: "multi".into(),
            assign: Vec::new(),
            beats: 1.0,
            mix: 0.5,
            slots: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FxSlotConfig {
    pub effect: String,
    pub params: Vec<f32>,
}

/// A named FX unit setup.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FxPreset {
    pub name: String,
    pub unit: FxUnitConfig,
}

/// DJ controller settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ControllerConfig {
    /// Use a controller when one is connected.
    pub enabled: bool,
    /// Which controller: "auto" (whichever supported one is plugged in),
    /// "ns4fx" or "ddj-flx6". A `profile` file replaces it.
    pub model: String,
    /// A profile file to use instead of the bundled profiles.
    pub profile: Option<std::path::PathBuf>,
    /// Controls given another function with MIDI learn. They replace what
    /// the profile does with the same message.
    #[serde(deserialize_with = "lenient_list")]
    pub learned: Vec<LearnedBinding>,
    /// Jog wheel sensitivity (1.0: one turn moves the audio of one record
    /// turn).
    pub jog_sensitivity: f32,
    /// Soft takeover: knobs and the pitch fader do nothing until they reach
    /// the software's value (after a LAYER switch or a change on screen).
    pub soft_takeover: bool,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            model: "auto".into(),
            profile: None,
            learned: Vec::new(),
            jog_sensitivity: 1.0,
            soft_takeover: true,
        }
    }
}

/// Performance settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PerformanceConfig {
    /// Snap cues, loops and jumps to the beatgrid.
    pub quantize: bool,
    /// SYNC matches the beat phase too (off: tempo only).
    pub smart_sync: bool,
    /// Beat jump size in beats.
    pub beat_jump_beats: f32,
    /// Auto loop size in beats.
    pub auto_loop_beats: f32,
    /// Beats the slicer cuts into 8 slices.
    pub slicer_domain_beats: f32,
    /// Length of a sampler recording in beats of the master tempo (8
    /// seconds when no tempo is known).
    pub sampler_record_beats: f32,
    /// Key lock on for every deck when Kaldor starts.
    pub key_lock_default: bool,
    /// Pitch range of every deck at start, in percent (8, 16 or 50).
    pub pitch_range_percent: u8,
    /// Refuse to load a track onto a deck that is playing.
    pub lock_playing_deck: bool,
    /// Where a newly loaded track starts.
    pub load_position: LoadPosition,
    /// A newly loaded track starts at 0 % pitch (off: the deck keeps its
    /// tempo).
    pub reset_tempo_on_load: bool,
    /// Loading the track that plays on another deck copies its position,
    /// tempo, key lock and loop (instant doubles).
    pub instant_doubles: bool,
    /// Jog mode of every deck at start: vinyl (touching the top scratches)
    /// or nudge only.
    pub jog_vinyl: bool,
    /// Seconds a deck takes to slow down to a stop when paused, like a
    /// turntable (0: stops at once).
    pub stop_brake_seconds: f32,
    /// What CUE does while the deck plays.
    pub cue_mode: CueMode,
    /// Which time the deck panels show.
    pub time_display: TimeDisplay,
    /// Time-stretcher used by key lock.
    pub key_lock_engine: KeyLockEngine,
    /// SYNC: the master moves to another playing deck when the master stops
    /// (on), or stays where you put it (off).
    pub auto_sync_master: bool,
}

impl Default for PerformanceConfig {
    fn default() -> Self {
        Self {
            quantize: false,
            smart_sync: true,
            beat_jump_beats: 4.0,
            auto_loop_beats: 4.0,
            slicer_domain_beats: 8.0,
            sampler_record_beats: 16.0,
            key_lock_default: true,
            pitch_range_percent: 8,
            lock_playing_deck: false,
            load_position: LoadPosition::default(),
            reset_tempo_on_load: false,
            instant_doubles: false,
            jog_vinyl: true,
            stop_brake_seconds: 0.0,
            cue_mode: CueMode::default(),
            time_display: TimeDisplay::default(),
            key_lock_engine: KeyLockEngine::default(),
            auto_sync_master: false,
        }
    }
}

/// Where a newly loaded track starts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LoadPosition {
    /// The start of the file.
    #[default]
    Start,
    /// The main cue point (the start when there is none).
    MainCue,
    /// The bar 1 marker of the beatgrid: where BEAT 1 was set, else the
    /// first downbeat of the track (the start when there is no grid).
    /// Called "Bar 1" in the settings; the name is kept for saved configs.
    FirstBeat,
}

/// What CUE does while the deck plays.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CueMode {
    /// Back to the cue point and pause (CDJ).
    #[default]
    Cdj,
    /// Back to the cue point and keep playing (stutter).
    Stutter,
}

/// Which time the deck panels show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeDisplay {
    /// Elapsed and remaining.
    #[default]
    Both,
    Elapsed,
    Remaining,
}

/// Time-stretcher used by key lock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyLockEngine {
    /// Signalsmith Stretch: best quality.
    #[default]
    Signalsmith,
    /// WSOLA: lighter on the CPU, can wobble on sustained notes.
    Wsola,
}

/// UI settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub dark_theme: bool,
    pub layout: DeckLayout,
    pub waveform_colors: WaveformColorMode,
    /// Zoom of the waveform strips ([`ZOOM_MIN_BEATS`] to
    /// [`ZOOM_MAX_BEATS`]): this many beats at 120 BPM of real time are
    /// visible, the same window for every deck.
    pub zoom_beats: f32,
    pub key_notation: KeyNotation,
    /// Bindings with an action this version does not know (written by a
    /// newer version) are skipped instead of failing the whole file.
    #[serde(deserialize_with = "lenient_list")]
    pub keymap: Vec<KeyBinding>,
    /// Library height as a fraction of the window height (the divider
    /// between the decks and the library).
    pub library_height: f32,
    /// Mixer shown under the waveforms.
    pub mixer_open: bool,
    /// Mixer or pads under the waveforms.
    pub centre_panel: CentrePanel,
    /// Show 2 decks (the LAYER decks) instead of 4, in any layout.
    pub two_decks: bool,
    /// Full screen: the window covers the whole screen, taskbar included.
    pub fullscreen: bool,
    /// Which layout defaults this file has had applied (0: files from
    /// before the Extended layout). A missing value reads as 0.
    #[serde(default)]
    pub layout_version: u32,
    /// Library text size (1.0 = normal).
    pub library_text_scale: f32,
    /// The folder the Files panel showed last.
    pub files_folder: Option<PathBuf>,
    /// Track list columns shown, in order (names: `dj_ui::library::LibColumn`;
    /// unknown names are skipped).
    pub library_columns: Vec<String>,
    /// See [`UiConfig::migrate_columns`]; a missing value reads as 0.
    #[serde(default)]
    pub library_columns_version: u32,
    /// Track list column widths set by dragging (points, by column name);
    /// a column not in here fits itself to the room.
    pub library_column_widths: std::collections::BTreeMap<String, f32>,
    /// Crates (and rekordbox folders) whose sub-crates are shown in the
    /// crate list (library crate ids); every other one with sub-crates is
    /// closed. (Replaces `collapsed_crates`, where they were open unless
    /// closed; an old file's value is ignored, so all start closed once.)
    #[serde(default)]
    pub open_crates: Vec<i64>,
    /// UI scale; 0 follows the display's scaling.
    pub ui_scale: f32,
    /// 24-hour clock in the top bar (off: 12-hour with am / pm).
    pub clock_24h: bool,
    /// Beat and bar lines on the waveforms.
    pub grid_lines: bool,
    /// Bar numbers on the waveforms (Extended layout).
    pub bar_numbers: bool,
    /// Where the playhead is across a waveform strip (0.5: the middle).
    pub playhead_at: f32,
    /// Most frames per second; 0 = as many as the display shows.
    pub max_fps: u32,
    /// Colour new hot cues get: `None` gives each pad its own colour.
    pub hot_cue_color: Option<u32>,
    /// Ask before closing while a deck plays.
    pub confirm_close: bool,
    /// The FX panel between the decks (FX in the top bar).
    pub fx_panel: bool,
}

impl UiConfig {
    pub const LAYOUT_VERSION: u32 = 2;
    /// The track list columns shown by default.
    pub const DEFAULT_LIBRARY_COLUMNS: [&'static str; 11] = [
        "number", "found", "artist", "title", "waveform", "bpm", "key", "time", "album", "genre",
        "added",
    ];
    /// Version of the track list columns: 1 adds the waveform column.
    pub const COLUMNS_VERSION: u32 = 1;

    /// Adds new track list columns once to a file written before them
    /// (version 1: the waveform, after Title). Returns true when something
    /// changed.
    pub fn migrate_columns(&mut self) -> bool {
        if self.library_columns_version >= Self::COLUMNS_VERSION {
            return false;
        }
        if !self.library_columns.iter().any(|c| c == "waveform") {
            let at = self
                .library_columns
                .iter()
                .position(|c| c == "title")
                .map_or(self.library_columns.len(), |i| i + 1);
            self.library_columns.insert(at, "waveform".into());
        }
        self.library_columns_version = Self::COLUMNS_VERSION;
        true
    }

    /// Applies new layout defaults once to a file written before them:
    /// version 1 makes Extended the layout, with the mixer / pads / FX
    /// panel closed; version 2 turns the old "2 decks" layout into Stacked
    /// with 2 decks. Returns true when something changed.
    pub fn migrate_layout(&mut self) -> bool {
        if self.layout_version >= Self::LAYOUT_VERSION {
            return false;
        }
        if self.layout_version < 1 {
            self.layout = DeckLayout::Extended;
            self.mixer_open = false;
        }
        if self.layout == DeckLayout::TwoDecks {
            self.layout = DeckLayout::FourStacked;
            self.two_decks = true;
        }
        self.layout_version = Self::LAYOUT_VERSION;
        true
    }

    /// The layout and whether it shows 2 decks, with the old `TwoDecks`
    /// read as Stacked with 2 decks.
    pub fn deck_view(&self) -> (DeckLayout, bool) {
        match self.layout {
            DeckLayout::TwoDecks => (DeckLayout::FourStacked, true),
            l => (l, self.two_decks),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            centre_panel: CentrePanel::default(),
            dark_theme: true,
            layout: DeckLayout::default(),
            waveform_colors: WaveformColorMode::default(),
            zoom_beats: 8.0,
            key_notation: KeyNotation::default(),
            keymap: default_keymap(),
            library_height: 0.30,
            // Closed: with a controller the mixer is on the hardware.
            mixer_open: false,
            two_decks: false,
            fullscreen: false,
            layout_version: Self::LAYOUT_VERSION,
            library_text_scale: 1.0,
            files_folder: None,
            library_columns: Self::DEFAULT_LIBRARY_COLUMNS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            library_columns_version: Self::COLUMNS_VERSION,
            library_column_widths: Default::default(),
            open_crates: Vec::new(),
            ui_scale: 0.0,
            clock_24h: true,
            grid_lines: true,
            bar_numbers: true,
            playhead_at: 0.5,
            max_fps: 0,
            hot_cue_color: None,
            confirm_close: true,
            fx_panel: false,
        }
    }
}

/// Closest waveform zoom: 2 beats at 120 BPM (1 s across; rekordbox stops
/// at about 3 s). Closer than this adds nothing a DJ can use.
pub const ZOOM_MIN_BEATS: f32 = 2.0;
/// Farthest waveform zoom: 128 beats (32 bars, about a minute at 120 BPM).
pub const ZOOM_MAX_BEATS: f32 = 128.0;

/// `beats` within the waveform zoom range.
pub fn clamp_zoom(beats: f32) -> f32 {
    if beats.is_finite() {
        beats.clamp(ZOOM_MIN_BEATS, ZOOM_MAX_BEATS)
    } else {
        8.0
    }
}

/// Library and analysis settings.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibraryConfig {
    /// Library database file. `None` uses `library.db` in the app directory.
    pub path: Option<PathBuf>,
    /// Apply each track's loudness-based gain when it is loaded.
    pub auto_gain: bool,
    /// Auto-gain target loudness in LUFS.
    pub target_lufs: f64,
    /// BPM detection range.
    pub bpm_min: f64,
    pub bpm_max: f64,
    /// Analysis threads; 0 picks automatically.
    pub analysis_workers: usize,
    /// Folders scanned for new music at start (and with Scan now).
    pub watch_folders: Vec<PathBuf>,
    /// Analyse tracks when they are imported (off: when first loaded).
    pub analyse_on_import: bool,
    /// Copy the library database at start, keeping this many copies
    /// (0: no automatic backups).
    pub backups_keep: usize,
    /// No edits to tags, cues, grids or crates.
    pub protect: bool,
    /// Keep a history of the tracks played, one session per start.
    pub history: bool,
    /// Keep the rekordbox tab in step with rekordbox's library on this
    /// computer.
    pub rekordbox_sync: bool,
    /// rekordbox's folder (the one with `master.db`). `None` is where
    /// rekordbox keeps it (`%APPDATA%\Pioneer\rekordbox`,
    /// `~/Library/Pioneer/rekordbox`).
    pub rekordbox_folder: Option<PathBuf>,
}

impl Default for LibraryConfig {
    fn default() -> Self {
        Self {
            path: None,
            auto_gain: true,
            target_lufs: -12.0,
            bpm_min: 70.0,
            bpm_max: 180.0,
            analysis_workers: 0,
            watch_folders: Vec::new(),
            analyse_on_import: true,
            backups_keep: 5,
            protect: false,
            history: true,
            rekordbox_sync: true,
            rekordbox_folder: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A config from before the Extended layout switches to it once, with
    /// the panel under the waveforms closed; after that the user's choice
    /// stays.
    #[test]
    fn old_files_move_to_the_extended_layout_once() {
        let mut old: AppConfig =
            toml::from_str("[ui]\nlayout = \"FourStacked\"\nmixer_open = true\n").expect("parse");
        assert_eq!(old.ui.layout_version, 0);
        assert!(old.ui.migrate_layout());
        assert_eq!(old.ui.layout, DeckLayout::Extended);
        assert!(!old.ui.mixer_open);
        // Saved and read back: chosen again by the user, it stays.
        old.ui.layout = DeckLayout::FourGrid;
        let text = toml::to_string(&old).expect("serialise");
        let mut back: AppConfig = toml::from_str(&text).expect("parse");
        assert!(!back.ui.migrate_layout());
        assert_eq!(back.ui.layout, DeckLayout::FourGrid);
        // A new config is already there.
        assert!(!AppConfig::default().ui.migrate_layout());
        // Version 1 with the old "2 decks" layout: Stacked with 2 decks.
        let mut v1: AppConfig =
            toml::from_str("[ui]\nlayout = \"TwoDecks\"\nlayout_version = 1\n").expect("parse");
        assert_eq!(v1.ui.deck_view(), (DeckLayout::FourStacked, true));
        assert!(v1.ui.migrate_layout());
        assert_eq!(v1.ui.layout, DeckLayout::FourStacked);
        assert!(v1.ui.two_decks);
    }

    #[test]
    fn toml_roundtrip_and_partial_files() {
        let cfg = AppConfig::default();
        let text = toml::to_string(&cfg).expect("serialise");
        let back: AppConfig = toml::from_str(&text).expect("parse");
        assert_eq!(cfg, back);

        // A file with only one key keeps the defaults for everything else.
        let partial: AppConfig = toml::from_str("[audio]\nbuffer_frames = 128\n").expect("parse");
        assert_eq!(partial.audio.buffer_frames, 128);
        assert_eq!(partial.audio.sample_rate, 48_000);
    }

    #[test]
    fn keymap_has_no_duplicate_keys_and_old_files_get_new_actions() {
        let km = default_keymap();
        for (i, a) in km.iter().enumerate() {
            for b in &km[i + 1..] {
                assert!(
                    !(a.key == b.key && a.shift == b.shift),
                    "{} (shift {}) bound twice",
                    a.key,
                    a.shift
                );
            }
        }
        // A file from before the pad, loop and grid keys: only the deck keys, one remapped.
        let mut old: Vec<KeyBinding> = km
            .iter()
            .filter(|b| matches!(b.action, KeyAction::Cue(_) | KeyAction::Play(_)))
            .cloned()
            .collect();
        old[0].key = "V".into(); // the user moved deck 1's CUE onto V
        merge_default_keymap(&mut old);
        assert!(old.iter().any(|b| b.action == KeyAction::Sync));
        // V stays the user's; loop in gets no key rather than a clash.
        assert!(!old.iter().any(|b| b.action == KeyAction::LoopIn));
        assert_eq!(old.iter().filter(|b| b.key == "V" && !b.shift).count(), 1);
        // Shift is written only when set.
        let text = toml::to_string(&UiConfig::default()).expect("toml");
        assert!(text.contains("shift = true"));
        let back: UiConfig = toml::from_str(&text).expect("parse");
        assert_eq!(back.keymap, km);
    }

    #[test]
    fn unknown_key_actions_are_skipped() {
        let text = r#"
            [ui]
            dark_theme = false
            [[ui.keymap]]
            key = "Q"
            action = { Cue = 1 }
            [[ui.keymap]]
            key = "F9"
            action = "SomethingFromTheFuture"
            [[ui.keymap]]
            key = "W"
            action = { Play = 1 }
        "#;
        let cfg: AppConfig = toml::from_str(text).expect("parse");
        assert!(!cfg.ui.dark_theme);
        assert_eq!(
            cfg.ui.keymap,
            vec![
                KeyBinding::new("Q", KeyAction::Cue(1)),
                KeyBinding::new("W", KeyAction::Play(1))
            ]
        );
    }

    #[test]
    fn sanitize_clamps_buffer() {
        let mut a = AudioConfig {
            buffer_frames: 8,
            ..AudioConfig::default()
        };
        assert_eq!(a.sanitized().buffer_frames, 64);
        a.buffer_frames = 99_999;
        assert_eq!(a.sanitized().buffer_frames, 1024);
    }
}

#[cfg(test)]
mod removed_settings_tests {
    use super::*;

    /// A config file from before "Load the last tracks" was removed
    /// still loads; the old keys are ignored.
    #[test]
    fn an_old_config_with_the_last_tracks_still_loads() {
        let old = "[ui]\nrestore_tracks = true\nconfirm_close = false\n\n\
                   [session]\ntracks = [\"C:/Music/a.mp3\", \"\"]\n";
        let cfg: AppConfig = toml::from_str(old).expect("old config loads");
        assert!(!cfg.ui.confirm_close);
        let text = toml::to_string(&cfg).expect("config saves");
        assert!(!text.contains("restore_tracks") && !text.contains("[session]"));
    }

    /// A config file from before the rekordbox XML auto-export was removed
    /// still loads; the old key is ignored.
    #[test]
    fn an_old_config_with_the_xml_auto_export_still_loads() {
        let old =
            "[library]\nxml_auto_export = \"C:/Music/kaldor-rekordbox.xml\"\nprotect = true\n";
        let cfg: AppConfig = toml::from_str(old).expect("old config loads");
        assert!(cfg.library.protect);
        let text = toml::to_string(&cfg).expect("config saves");
        assert!(!text.contains("xml_auto_export"));
    }
}

#[cfg(test)]
mod learn_tests {
    use super::*;

    #[test]
    fn learned_bindings_round_trip_and_unknown_ones_are_skipped() {
        let mut cfg = AppConfig::default();
        cfg.controller.learned = vec![
            LearnedBinding {
                note: true,
                channel: 1,
                number: 0x30,
                function: MidiFunction::Pad(3),
                deck: Some(2),
            },
            LearnedBinding {
                note: false,
                channel: 16,
                number: 0x0C,
                function: MidiFunction::MasterGain,
                deck: None,
            },
        ];
        let text = toml::to_string(&cfg).unwrap();
        let back: AppConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.controller.learned, cfg.controller.learned);
        // A function from a newer version is dropped, the rest kept.
        let newer = text.replace("MasterGain", "TimeTravel");
        let back: AppConfig = toml::from_str(&newer).unwrap();
        assert_eq!(back.controller.learned.len(), 1);
        // Labels and kinds.
        assert!(MidiFunction::all().len() > 90);
        assert_eq!(MidiFunction::Pitch.kind(), FunctionKind::Continuous);
        assert!(!MidiFunction::Crossfader.needs_deck());
        assert_eq!(MidiFunction::PadMode(4).label(), "Pad mode: roll");
    }
}

#[cfg(test)]
mod fx_migration_tests {
    use super::*;

    #[test]
    fn old_wet_dry_values_keep_their_sound() {
        let mut c: AppConfig =
            toml::from_str("[[fx.units]]\nmix = 1.0\n\n[[fx.presets]]\nname = \"p\"\n[fx.presets.unit]\nmix = 0.6\n")
                .unwrap();
        assert_eq!(c.fx.version, 0);
        assert!(c.fx.migrate());
        assert_eq!(c.fx.units[0].mix, 0.5);
        assert!((c.fx.presets[0].unit.mix - 0.3).abs() < 1e-6);
        assert_eq!(c.fx.version, FxConfig::VERSION);
        // A second run changes nothing.
        assert!(!c.fx.migrate());
        assert_eq!(c.fx.units[0].mix, 0.5);
    }

    #[test]
    fn old_filter_resonance_defaults_move_to_the_new_one() {
        for (version, old) in [(1, 0.75f32), (2, 0.5), (3, 0.77)] {
            let mut m = MixerConfig {
                filter_resonance: old,
                filter_slope: "12".into(),
                version,
                ..MixerConfig::default()
            };
            assert!(m.migrate(), "version {version}");
            assert_eq!(m.filter_resonance, MixerConfig::default().filter_resonance);
            assert_eq!(m.filter_slope, "24");
            assert_eq!(m.version, MixerConfig::VERSION);
        }
        // The old EQ crossover defaults move; chosen ones stay.
        let mut m = MixerConfig {
            eq_low_hz: 250.0,
            eq_high_hz: 2500.0,
            version: 4,
            ..MixerConfig::default()
        };
        assert!(m.migrate());
        assert_eq!((m.eq_low_hz, m.eq_high_hz), (150.0, 4000.0));
        let mut m = MixerConfig {
            eq_low_hz: 300.0,
            eq_high_hz: 2500.0,
            version: 4,
            ..MixerConfig::default()
        };
        assert!(!m.migrate());
        assert_eq!((m.eq_low_hz, m.eq_high_hz), (300.0, 2500.0));
        // A value someone chose stays.
        let mut m = MixerConfig {
            filter_resonance: 0.3,
            version: 3,
            ..MixerConfig::default()
        };
        assert!(!m.migrate());
        assert_eq!(m.filter_resonance, 0.3);
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;

    /// A config from before the waveform column gets it after Title, once;
    /// a new config already has it; taking it away later sticks.
    #[test]
    fn the_waveform_column_is_added_once() {
        let mut old: AppConfig = toml::from_str(
            "[ui]\nlibrary_columns = [\"number\", \"artist\", \"title\", \"bpm\"]\n",
        )
        .expect("parse");
        assert!(old.ui.migrate_columns());
        assert_eq!(
            old.ui.library_columns,
            ["number", "artist", "title", "waveform", "bpm"]
        );
        assert!(!old.ui.migrate_columns(), "only once");
        old.ui.library_columns.retain(|c| c != "waveform");
        let text = toml::to_string(&old).unwrap();
        let mut back: AppConfig = toml::from_str(&text).unwrap();
        assert!(!back.ui.migrate_columns(), "taken away by hand: stays away");
        assert!(!AppConfig::default().ui.migrate_columns());
        assert!(
            AppConfig::default()
                .ui
                .library_columns
                .contains(&"waveform".to_string())
        );
    }
}
