//! Top-level UI state.

mod drag_drop;
mod keys;
mod top_bar;

use crate::audio::{AudioBackend, AudioPicker, OpenedAudio};
use crate::deck_view::DeckUi;
use crate::layout;
use crate::library::LibraryUi;
use crate::loader::{LoadJob, LoaderEvent};
use crate::theme::Palette;
use crate::uimap::{Region, UiMap};
use dj_analysis::Waveform;
use dj_audio::{
    AudioOutput, Command, DeckCommand, DeckSnapshot, EngineHandle, MixerCommand, OutputInfo,
    TrackAudio,
};
use dj_core::config::{DeckLayout, KeyAction};
use dj_core::{AppConfig, AudioConfig, DeckId, NUM_DECKS};
use egui::{Id, Layout, Rect, Sense, UiBuilder};
use std::path::PathBuf;
use std::sync::Arc;

/// Something the UI wants the host application to do (save config, ...).
#[derive(Clone, Debug, PartialEq)]
pub enum AppEvent {
    ConfigChanged,
}

type ConfigSaver = Box<dyn FnMut(&AppConfig)>;

/// How often a stopped audio output is retried.
const AUDIO_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);
/// No audio rendered for this long counts as a stopped output.
const AUDIO_STALL: std::time::Duration = std::time::Duration::from_millis(1500);

/// Pull time constant of the waveform position while scrubbing or scratching.
const SCRUB_SMOOTH_SECONDS: f64 = 0.03;
/// Frame and audio clocks for smooth waveform scrolling (see
/// [`crate::frame_clock`]).
#[derive(Clone, Copy, Debug, Default)]
struct Clock {
    pacer: crate::frame_clock::FramePacer,
    audio: crate::frame_clock::AudioClock,
    /// Paced time of this frame and the paced time since the last one
    /// (seconds).
    time: f64,
    dt: f64,
    /// Frame counter, so a deck's scroll position advances once a frame.
    frame: u64,
}

/// Where a dragged track or file would land (see [`KaldorApp::drop_target_now`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropTarget {
    Deck(usize),
    Sample(usize),
    /// A crate in the crate list (or the list showing it, for files).
    Crate(dj_library::CrateId),
    /// The empty space under the crates: a new crate.
    NewCrate,
    /// The library (All tracks, or the list of All tracks): files only.
    Library,
}

/// All UI state.
pub struct KaldorApp {
    pub(crate) engine: EngineHandle,
    pub(crate) output_info: OutputInfo,
    pub(crate) output: Option<Box<dyn AudioOutput>>,
    /// When the audio output was last checked, and whether it is lost.
    audio_checked: Option<std::time::Instant>,
    audio_lost: bool,
    /// The last rendered-frame count seen and when it changed.
    audio_progress: (u64, std::time::Instant),
    pub(crate) config: AppConfig,
    pub(crate) events: Vec<AppEvent>,
    pub(crate) show_setup: bool,
    pub(crate) status_line: String,
    /// The status message shown in the top bar and since when (it shows
    /// for 10 s).
    status_since: Option<(String, std::time::Instant)>,
    /// The CPU load shown in the top bar (smoothed) and when it was last
    /// updated; the number shown and when it changed.
    cpu_smooth: Option<(f32, std::time::Instant)>,
    cpu_label: Option<(String, std::time::Instant)>,
    /// The full screen state last sent to the window.
    fullscreen_sent: Option<bool>,
    /// The section shown in the settings window.
    pub(crate) setup_tab: crate::setup::SetupTab,
    pub(crate) decks: [DeckUi; NUM_DECKS],
    /// Screen rectangles of each deck's panel and waveform strip in the
    /// last frame (for file drops).
    pub(crate) drop_rects: Vec<(usize, egui::Rect)>,
    /// Pads in SAMPLE mode: drop targets for sampler slots (slot, rect).
    pub(crate) pad_sample_drops: Vec<(usize, egui::Rect)>,
    /// What the drop outline marked in the last frame.
    drop_highlight: Option<DropTarget>,
    /// LAYER deck of the left and right side: shown in the 2-deck layout,
    /// kept in full when space is short in the 4-deck layouts.
    pub(crate) layer: [usize; 2],
    /// The mixer is shown in place of the waveforms (window too small to
    /// show both); not saved.
    pub(crate) mixer_swap: bool,
    /// Crate tree width in the library (points).
    pub(crate) crate_tree_width: f32,
    /// Where every control was drawn in the last frame.
    pub(crate) map: crate::uimap::UiMap,
    /// Layout of the last frame.
    pub(crate) frame: Option<crate::layout::Frame>,
    next_serial: u64,
    clock: Clock,
    pub(crate) library: Option<LibraryUi>,
    /// The DJ controller (NS4FX), when one is attached.
    pub(crate) controller: Option<crate::controller::ControllerLink>,
    /// MIDI learn waiting for a control: the function and its deck (1-4).
    pub(crate) learn: Option<(dj_core::config::MidiFunction, Option<u8>)>,
    /// The last MIDI learn message for Setup.
    pub(crate) learn_status: String,
    /// The function (index into `MidiFunction::all()`) and deck chosen in
    /// the MIDI learn panel.
    pub(crate) learn_pick: (usize, u8),
    config_saver: Option<ConfigSaver>,
    /// Keeps `config.mixer` in step with the engine (see `mixer_setup`).
    pub(crate) mixer_sync: crate::mixer_setup::MixerSync,
    /// The UI scale last applied (the settings can change it).
    pub(crate) ui_scale_applied: Option<f32>,
    /// Closing was asked for while a deck plays: the question is shown.
    pub(crate) close_ask: bool,
    /// The user confirmed closing.
    pub(crate) close_ok: bool,
    /// Settings work that runs on the library (backups, watch folders,
    /// history, XML auto-export).
    pub(crate) lib_tasks: crate::library_settings::LibraryTasks,
    /// Settings > Keyboard: the action waiting for its new key, and what
    /// the last change did.
    pub(crate) key_capture: Option<KeyAction>,
    pub(crate) key_message: String,
    /// Underruns counted before the last reset (Settings > Audio).
    pub(crate) xrun_base: u64,
    /// Clipped samples per output at the last Reset in Setup.
    pub(crate) clip_base: [u64; 3],
    /// The mapping file field while it is being typed in.
    pub(crate) profile_edit: Option<String>,
    /// Opens outputs when the user picks another one in Setup.
    pub(crate) audio_backend: Option<Box<dyn AudioBackend>>,
    /// Settings the current output runs with (can differ from
    /// `config.audio` after a start-up fallback).
    running_audio: AudioConfig,
    pub(crate) picker: AudioPicker,
    /// Deck the keyboard's pad, loop, jump and grid keys work on.
    pub(crate) kb_deck: usize,
    pub(crate) sampler: crate::sampler_view::SamplerUi,
    /// SAMPLER tab shown in the bottom area (Setup wins when both are set).
    pub(crate) show_sampler: bool,
    /// The Files panel over the library.
    pub(crate) files_panel: crate::files_panel::FilesPanel,
    /// RECORD tab shown in the bottom area (Setup wins).
    pub(crate) show_record: bool,
    pub(crate) fx: crate::fx_view::FxUi,
    pub(crate) rec: crate::record_view::RecordUi,
}

impl KaldorApp {
    /// Creates the UI. `output` is the running device (or null output);
    /// pass `None` with an `OutputInfo` describing the offline renderer in
    /// headless mode.
    pub fn new(
        engine: EngineHandle,
        output_info: OutputInfo,
        output: Option<Box<dyn AudioOutput>>,
        mut config: AppConfig,
    ) -> Self {
        let mut engine = engine;
        let record_reader = engine.take_record_reader();
        let running_audio = config.audio.clone();
        // Config files from before an action existed get its default key.
        dj_core::config::merge_default_keymap(&mut config.ui.keymap);
        let perf_cfg = config.performance.clone();
        let mut app = Self {
            engine,
            output_info,
            output,
            audio_checked: None,
            audio_lost: false,
            audio_progress: (0, std::time::Instant::now()),
            config,
            audio_backend: None,
            running_audio,
            picker: AudioPicker::new(),
            events: Vec::new(),
            show_setup: false,
            status_line: String::new(),
            status_since: None,
            cpu_smooth: None,
            cpu_label: None,
            fullscreen_sent: None,
            setup_tab: Default::default(),
            kb_deck: 0,
            sampler: crate::sampler_view::SamplerUi::new(None),
            show_sampler: false,
            files_panel: Default::default(),
            show_record: false,
            fx: crate::fx_view::FxUi::default(),
            rec: crate::record_view::RecordUi {
                reader: record_reader,
                ..Default::default()
            },
            decks: std::array::from_fn(|_| {
                let mut d = DeckUi::new();
                d.perf.auto_loop_beats = perf_cfg.auto_loop_beats;
                d.perf.jump_beats = perf_cfg.beat_jump_beats;
                d.perf.slicer_domain = perf_cfg.slicer_domain_beats;
                d
            }),
            drop_rects: Vec::new(),
            pad_sample_drops: Vec::new(),
            drop_highlight: None,
            layer: [0, 1],
            mixer_swap: false,
            crate_tree_width: 200.0,
            map: crate::uimap::UiMap::default(),
            frame: None,
            next_serial: 1,
            clock: Clock::default(),
            library: None,
            controller: None,
            learn: None,
            learn_status: String::new(),
            learn_pick: (0, 1),
            config_saver: None,
            mixer_sync: Default::default(),
            ui_scale_applied: None,
            close_ask: false,
            close_ok: false,
            lib_tasks: Default::default(),
            key_capture: None,
            key_message: String::new(),
            xrun_base: 0,
            clip_base: [0; 3],
            profile_edit: None,
        };
        // A config from before the Extended layout switches to it once.
        if app.config.ui.migrate_layout() {
            app.events.push(AppEvent::ConfigChanged);
        }
        // A track list from before the waveform column gets it once.
        if app.config.ui.migrate_columns() {
            app.events.push(AppEvent::ConfigChanged);
        }
        app.send_performance_settings();
        app.send_pitch_range();
        app.send_fx_config();
        app.send_mixer_config();
        // The master starts at 12 o'clock of its knob, well below the
        // limiter (the engine's own default is 0 dB).
        app.send(Command::Mixer(dj_audio::MixerCommand::MasterGain(
            crate::mixer_view::MASTER_CENTRE_DB,
        )));
        let vinyl = app.config.performance.jog_vinyl;
        for d in &mut app.decks {
            d.jog_vinyl = vinyl;
        }
        // Key lock on every deck from the start, unless turned off
        // in the config; it stays as set across track loads.
        if app.config.performance.key_lock_default {
            for d in DeckId::ALL {
                app.send(Command::Deck(d, DeckCommand::SetKeyLock(true)));
            }
        }
        app
    }

    /// Quantize, sync and the deck settings from the config to the engine.
    pub(crate) fn send_performance_settings(&mut self) {
        let p = self.config.performance.clone();
        self.send(Command::Quantize(p.quantize));
        self.send(Command::SetSyncMode(if p.smart_sync {
            dj_audio::SyncMode::Phase
        } else {
            dj_audio::SyncMode::Tempo
        }));
        self.send(Command::SetAutoSyncMaster(p.auto_sync_master));
        for d in DeckId::ALL {
            self.send(Command::Deck(
                d,
                DeckCommand::SetStopBrake(p.stop_brake_seconds),
            ));
            self.send(Command::Deck(
                d,
                DeckCommand::SetStutterCue(p.cue_mode == dj_core::config::CueMode::Stutter),
            ));
            self.send(Command::Deck(
                d,
                DeckCommand::SetKeyLockWsola(
                    p.key_lock_engine == dj_core::config::KeyLockEngine::Wsola,
                ),
            ));
        }
    }

    /// The pitch range from the settings on every deck (at start, and when
    /// it is changed in the settings).
    pub(crate) fn send_pitch_range(&mut self) {
        let range = match self.config.performance.pitch_range_percent {
            16 => dj_audio::PitchRange::Sixteen,
            50 => dj_audio::PitchRange::Fifty,
            _ => dj_audio::PitchRange::Eight,
        };
        for d in DeckId::ALL {
            self.send(Command::Deck(d, DeckCommand::SetPitchRange(range)));
        }
    }

    /// Lock playing deck: true (and a message) when `deck` plays and may
    /// not get a new track.
    fn load_refused(&mut self, deck: DeckId) -> bool {
        let playing = self.engine.snapshot().decks[deck.index()].playing;
        if self.config.performance.lock_playing_deck && playing {
            self.status_line = format!(
                "Deck {} is playing: loading is locked (Settings > Decks)",
                deck.number()
            );
            return true;
        }
        false
    }

    /// Where a newly loaded track starts (frames), from the settings or the
    /// deck it doubles.
    fn load_start(
        &self,
        deck: usize,
        markers: &dj_audio::TrackMarkers,
        double: Option<usize>,
    ) -> Option<f64> {
        use dj_core::config::LoadPosition;
        if let Some(j) = double {
            return Some(self.engine.snapshot().decks[j].position);
        }
        match self.config.performance.load_position {
            LoadPosition::Start => None,
            LoadPosition::MainCue => markers.cue_point.filter(|c| *c > 0.0),
            // The bar 1 marker (BEAT 1, or the first downbeat of the track).
            LoadPosition::FirstBeat => self.decks[deck]
                .grid
                .map(|g| g.bar_one_time() * f64::from(self.engine.snapshot().sample_rate.max(1))),
        }
    }

    /// After a user load reached the engine: the doubled deck's tempo, key
    /// lock, loop and play state, or the tempo reset.
    fn after_fresh_load(&mut self, deck: usize, double: Option<usize>) {
        let id = DeckId::ALL[deck];
        if let Some(j) = double {
            let d = self.engine.snapshot().decks[j];
            self.send(Command::Deck(id, DeckCommand::SetPitchRange(d.pitch_range)));
            self.send(Command::Deck(id, DeckCommand::SetPitch(d.pitch)));
            self.send(Command::Deck(id, DeckCommand::SetKeyLock(d.key_lock)));
            if let Some(l) = d.loop_info.filter(|l| l.active) {
                self.send(Command::Deck(
                    id,
                    DeckCommand::SetLoop {
                        start: l.start,
                        end: l.end,
                        jump: false,
                    },
                ));
            }
            if d.playing {
                self.send(Command::Deck(id, DeckCommand::Play(true)));
            }
        } else if self.config.performance.reset_tempo_on_load {
            self.send(Command::Deck(id, DeckCommand::SetPitch(0.0)));
        }
    }

    /// Where the app keeps its files (sample sets go in `samples/`).
    pub fn with_data_dir(mut self, dir: PathBuf) -> Self {
        if self.sampler.set_path.is_empty() {
            self.sampler.set_path = dir
                .join("samples")
                .join("sampler.toml")
                .display()
                .to_string();
        }
        self.lib_tasks.data_dir = Some(dir);
        self
    }

    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// The settings, to change them directly (tests).
    pub fn config_mut_for_test(&mut self) -> &mut AppConfig {
        &mut self.config
    }

    /// Latest engine snapshot (for tests and the smoke test).
    pub fn snapshot(&self) -> &dj_audio::EngineSnapshot {
        self.engine.snapshot()
    }

    /// Shows the GRID tab (or the CUES tab) on a deck's Extended panel
    /// (screenshots, tests).
    pub fn show_grid_tab(&mut self, deck: DeckId, grid: bool) {
        self.decks[deck.index()].panel_tab = if grid {
            crate::deck_view::PanelTab::Grid
        } else {
            crate::deck_view::PanelTab::Cues
        };
    }

    /// True once a deck's waveform has been built.
    pub fn has_waveform(&self, deck: DeckId) -> bool {
        self.decks[deck.index()].waveform.is_some()
    }

    /// The beatgrid shown on a deck, if its track has one.
    pub fn deck_grid(&self, deck: DeckId) -> Option<dj_core::Beatgrid> {
        self.decks[deck.index()].grid
    }

    /// Title shown on a deck ("Artist - Title" for library tracks).
    pub fn deck_title(&self, deck: DeckId) -> &str {
        &self.decks[deck.index()].title
    }

    /// Error or warning shown for a deck, if any.
    pub fn deck_status(&self, deck: DeckId) -> Option<&str> {
        self.decks[deck.index()].status.as_deref()
    }

    /// Events raised since the last call.
    pub fn take_events(&mut self) -> Vec<AppEvent> {
        std::mem::take(&mut self.events)
    }

    /// Sends a command to the engine; a full queue is shown in the status
    /// line instead of blocking.
    pub fn send(&mut self, cmd: Command) {
        if self.engine.send(cmd).is_err() {
            self.status_line = "Engine busy: command dropped".into();
        }
    }

    fn sample_rate(&self) -> u32 {
        self.output_info.sample_rate
    }

    /// Starts loading a file (not from the library) onto a deck in the
    /// background.
    pub fn load_path(&mut self, deck: DeckId, path: PathBuf) {
        if self.load_refused(deck) {
            return;
        }
        // A file from the file manager becomes (or already is) a library
        // track, so what is edited on the deck is saved.
        if let Some(id) = self.library.as_mut().and_then(|l| l.add_file(&path)) {
            self.load_library_track(deck, id);
            return;
        }
        let double = self.double_source(deck, |u| {
            u.track_id.is_none() && u.path.as_ref() == Some(&path)
        });
        let ui = &mut self.decks[deck.index()];
        ui.clear_track_info();
        ui.fresh_load = true;
        ui.double_of = double;
        self.set_auto_gain(deck, None, None);
        self.start_load(deck, path, None);
    }

    /// Instant doubles: another loaded deck holding the same track.
    fn double_source(&self, deck: DeckId, same: impl Fn(&DeckUi) -> bool) -> Option<usize> {
        if !self.config.performance.instant_doubles {
            return None;
        }
        let snap = self.engine.snapshot();
        (0..NUM_DECKS).find(|&j| j != deck.index() && snap.decks[j].loaded && same(&self.decks[j]))
    }

    fn start_load(&mut self, deck: DeckId, path: PathBuf, cached: Option<Arc<Waveform>>) {
        let build_waveform = cached.is_none();
        let ui = &mut self.decks[deck.index()];
        ui.status = None;
        // The fine detail waveform is never cached: built after every load.
        ui.detail = None;
        match LoadJob::start(
            path.clone(),
            self.output_info.sample_rate,
            build_waveform,
            true,
        ) {
            Ok(job) => {
                if ui.track_id.is_none() {
                    ui.title = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                }
                ui.path = Some(path);
                ui.waveform = cached;
                ui.job = Some(job);
            }
            Err(e) => ui.status = Some(format!("cannot start loader: {e}")),
        }
    }

    /// Loads a library track: audio in the background, and the cached
    /// waveform, beatgrid, key and auto-gain from the library.
    pub fn load_library_track(&mut self, deck: DeckId, id: dj_library::TrackId) {
        if self.load_refused(deck) {
            return;
        }
        // A file known to be gone is not loaded: the deck keeps its track
        // (a fresh look first, in case the stick was just plugged in).
        if let Some(lib) = self.library.as_ref()
            && let Some(t) = lib.track(id)
            && lib.file_state(id).is_missing()
            && !t.meta.path.is_file()
        {
            self.status_line = match lib.file_state(id) {
                crate::file_check::FileState::DriveMissing => format!(
                    "Not loaded: the drive of \u{201c}{}\u{201d} is not connected ({})",
                    t.display_name(),
                    crate::file_check::drive_root(&t.meta.path)
                        .map(|r| r.display().to_string())
                        .unwrap_or_default()
                ),
                _ => format!(
                    "Not loaded: \u{201c}{}\u{201d} is not at {}",
                    t.display_name(),
                    t.meta.path.display()
                ),
            };
            return;
        }
        let double = self.double_source(deck, |u| u.track_id == Some(id));
        let Some(lib) = self.library.as_ref() else {
            return;
        };
        let Some(track) = lib.track(id).cloned() else {
            return;
        };
        let grid = lib.lib.beatgrid(id).ok().flatten();
        let waveform = lib.lib.waveform(id).ok().flatten().map(Arc::new);
        let cues = lib.lib.cues(id).unwrap_or_default();
        let loops = lib.lib.loops(id).unwrap_or_default();
        let ui = &mut self.decks[deck.index()];
        ui.clear_track_info();
        ui.perf.set_from_library(&cues, &loops);
        ui.track_id = Some(id);
        ui.title = track.display_name();
        ui.bpm = grid.map(|g| g.bpm).or(track.bpm);
        ui.key = track.key;
        ui.grid = grid;
        ui.fresh_load = true;
        ui.double_of = double;
        self.set_auto_gain(deck, track.lufs, track.gain_db);
        self.start_load(deck, track.meta.path.clone(), waveform);
        // Analysis on import off: a track is analysed when first loaded.
        if let Some(lib) = self.library.as_mut()
            && !lib.analyse_on_import
        {
            lib.analyse_if_needed(id);
        }
    }

    /// Auto gain for a newly loaded track: from its loudness and the target
    /// in the settings (or the gain stored with it when the loudness is
    /// unknown).
    fn set_auto_gain(&mut self, deck: DeckId, lufs: Option<f64>, gain_db: Option<f64>) {
        self.decks[deck.index()].loudness = (lufs, gain_db);
        self.send_auto_gain(deck);
    }

    fn send_auto_gain(&mut self, deck: DeckId) {
        let lib = &self.config.library;
        let db = if lib.auto_gain {
            match self.decks[deck.index()].loudness {
                (Some(lufs), _) => dj_analysis::loudness::auto_gain_db(lufs, lib.target_lufs),
                (None, g) => g.unwrap_or(0.0),
            }
        } else {
            0.0
        };
        self.send(Command::Mixer(MixerCommand::AutoGain(deck, db as f32)));
    }

    /// Applies changed auto gain settings to the tracks on the decks.
    pub(crate) fn reapply_auto_gain(&mut self) {
        for d in DeckId::ALL {
            self.send_auto_gain(d);
        }
    }

    /// Loads audio that is already in memory (generated audio, tests). The
    /// waveform is built on this thread, so keep it short.
    pub fn load_audio(&mut self, deck: DeckId, title: &str, audio: Arc<TrackAudio>) {
        let serial = self.bump_serial();
        let wf = Waveform::build(&audio);
        let detail = Waveform::build_detail(&audio);
        let ui = &mut self.decks[deck.index()];
        ui.clear_track_info();
        ui.job = None;
        ui.title = title.to_string();
        ui.path = None;
        ui.waveform = Some(Arc::new(wf));
        ui.detail = Some(Arc::new(detail));
        ui.status = None;
        ui.perf.serial = serial;
        self.send(Command::Deck(deck, DeckCommand::Load { audio, serial }));
    }

    /// Like [`Self::load_audio`], with analysis results (beatgrid, key,
    /// waveform, gain) shown on the deck.
    pub fn load_audio_analyzed(
        &mut self,
        deck: DeckId,
        title: &str,
        audio: Arc<TrackAudio>,
        analysis: &dj_analysis::Analysis,
    ) {
        let serial = self.bump_serial();
        let detail = Waveform::build_detail(&audio);
        let ui = &mut self.decks[deck.index()];
        ui.clear_track_info();
        ui.job = None;
        ui.title = title.to_string();
        ui.path = None;
        ui.waveform = Some(Arc::new(analysis.waveform.clone()));
        ui.detail = Some(Arc::new(detail));
        ui.status = None;
        ui.grid = analysis.beatgrid;
        ui.bpm = analysis.beatgrid.map(|g| g.bpm);
        ui.key = analysis.key;
        ui.perf.serial = serial;
        self.set_auto_gain(deck, analysis.lufs, analysis.gain_db);
        let markers = self.track_markers(deck.index());
        self.send(Command::Deck(
            deck,
            DeckCommand::LoadWithMarkers {
                audio,
                serial,
                markers,
            },
        ));
    }

    /// Opens the library panel on an open library and starts analysing
    /// tracks that need it.
    /// Empties a deck (SHIFT + LOAD on the controller).
    pub(crate) fn eject_deck(&mut self, deck: usize) {
        let Some(id) = DeckId::from_index(deck) else {
            return;
        };
        let ui = &mut self.decks[deck];
        ui.clear_track_info();
        ui.job = None;
        ui.title.clear();
        ui.path = None;
        ui.waveform = None;
        ui.detail = None;
        ui.status = None;
        self.send(Command::Deck(id, DeckCommand::Eject));
    }

    /// Attaches a DJ controller (see [`crate::controller::ControllerLink`]).
    pub fn with_controller(mut self, link: crate::controller::ControllerLink) -> Self {
        self.attach_controller(link);
        self
    }

    /// Like [`Self::with_controller`], in place.
    pub fn attach_controller(&mut self, link: crate::controller::ControllerLink) {
        self.controller = Some(link);
        self.hand_controller_queue();
    }

    /// Messages from the controller that its profile does not know (the
    /// last few, oldest first).
    pub fn controller_unknown(&self) -> Vec<String> {
        self.controller
            .as_ref()
            .map(|c| c.unknown())
            .unwrap_or_default()
    }

    /// The controller's connection status (empty without a controller).
    pub fn controller_status(&self) -> String {
        self.controller
            .as_ref()
            .map(|c| c.status())
            .unwrap_or_default()
    }

    fn hand_controller_queue(&mut self) {
        if let Some(c) = &self.controller
            && let Ok(mut q) = c.queue.lock()
        {
            *q = self.engine.take_controller_queue();
        }
    }

    pub fn with_library(mut self, lib: dj_library::Library) -> Self {
        let c = &self.config.library;
        let opts = dj_analysis::AnalysisOptions {
            tempo: dj_analysis::TempoOptions {
                min_bpm: c.bpm_min,
                max_bpm: c.bpm_max,
            },
            target_lufs: c.target_lufs,
        };
        let mut lib = LibraryUi::with_settings(
            lib,
            opts,
            c.analysis_workers,
            c.analyse_on_import,
            c.protect,
        );
        // rekordbox's own folder only when the real app asks for it
        // (`use_rekordbox_default_folder`): tests never look at this
        // computer's rekordbox.
        lib.rekordbox
            .configure(c.rekordbox_sync, c.rekordbox_folder.clone());
        // The crates opened last time (ids still in the library); every
        // other crate with sub-crates starts closed.
        let mut ids: Vec<_> = lib.crate_list().into_iter().map(|c| c.0).collect();
        ids.extend(lib.rb_crate_ids());
        lib.opened = self
            .config
            .ui
            .open_crates
            .iter()
            .copied()
            .filter(|c| ids.contains(c))
            .collect();
        lib.refold();
        self.library = Some(lib);
        self
    }

    /// Lets the rekordbox tab use the folder where rekordbox keeps its
    /// library on this computer (when no folder is set). Only the real app
    /// calls this: tests never read this computer's rekordbox.
    pub fn use_rekordbox_default_folder(mut self) -> Self {
        if let Some(lib) = self.library.as_mut() {
            lib.rekordbox.use_default = true;
            lib.rekordbox.sync_now();
        }
        self
    }

    /// Calls `f` with the new config whenever the UI changes a setting that
    /// should be saved.
    pub fn on_config_change(mut self, f: impl FnMut(&AppConfig) + 'static) -> Self {
        self.config_saver = Some(Box::new(f));
        self
    }

    /// Lets the user change the audio output in Setup. `running` is the
    /// settings the current output runs with; `notice` (from a start-up
    /// fallback) is shown in the status bar and opens Setup.
    pub fn with_audio_backend(
        mut self,
        backend: Box<dyn AudioBackend>,
        running: AudioConfig,
        notice: Option<String>,
    ) -> Self {
        self.audio_backend = Some(backend);
        self.running_audio = running;
        if let Some(n) = notice {
            self.status_line = n.clone();
            self.picker.message = Some(n);
            self.show_setup = true;
            self.setup_tab = crate::setup::SetupTab::Audio;
        }
        self
    }

    /// Output info of the running audio output.
    pub fn output_info(&self) -> &OutputInfo {
        &self.output_info
    }

    /// Switches to the output described by `cfg` and saves it in the config.
    ///
    /// The old output is stopped first (exclusive mode and ASIO allow only
    /// one user of a device). A new engine is built at the new device's
    /// rate; mixer settings, pitch and key lock carry over, and tracks on
    /// the decks reload, paused, at the same place. If `cfg` cannot be
    /// opened, the previous output is reopened (or the null output) and the
    /// error is returned.
    pub fn switch_audio(&mut self, cfg: AudioConfig) -> Result<(), String> {
        let Some(mut backend) = self.audio_backend.take() else {
            return Err("this window cannot change the audio output".into());
        };
        let snap = *self.engine.snapshot();
        let old_rate = self.output_info.sample_rate;
        // Stop the old output before opening the new one.
        self.output = None;
        let result = match backend.open(&cfg) {
            Ok(audio) => {
                self.running_audio = cfg.clone();
                if self.config.audio != cfg {
                    self.config.audio = cfg;
                    self.events.push(AppEvent::ConfigChanged);
                }
                self.install_audio(audio);
                Ok(())
            }
            Err(e) => {
                let back = backend
                    .open(&self.running_audio)
                    .or_else(|_| crate::audio::open_null(&self.running_audio));
                match back {
                    Ok(audio) => self.install_audio(audio),
                    Err(e2) => tracing::error!("cannot reopen any output: {e2}"),
                }
                Err(e)
            }
        };
        self.audio_backend = Some(backend);
        self.restore_engine_state(&snap, old_rate);
        self.status_line = match &result {
            Ok(()) => format!("Audio output: {}", self.output_info.device),
            Err(e) => format!("Could not open that output: {e}"),
        };
        result
    }

    /// Reopens the audio output after it stopped (the device was unplugged):
    /// checked once a second; when the same output opens again, the
    /// mixer and deck settings carry over as after a switch in Setup.
    fn watch_audio(&mut self) {
        let now = std::time::Instant::now();
        if self
            .audio_checked
            .is_some_and(|t| now.duration_since(t) < AUDIO_WATCH_INTERVAL)
        {
            return;
        }
        self.audio_checked = Some(now);
        // Stopped: the driver said so, or no audio was rendered for a while
        // (some drivers just stop calling back when the device goes).
        let frames = self.engine.snapshot().frames_rendered;
        if frames != self.audio_progress.0 {
            self.audio_progress = (frames, now);
        }
        let stalled = now.duration_since(self.audio_progress.1) > AUDIO_STALL;
        let dead = self
            .output
            .as_ref()
            .is_some_and(|o| !o.is_running() || stalled);
        if !(dead || self.audio_lost) {
            return;
        }
        let Some(mut backend) = self.audio_backend.take() else {
            return;
        };
        if !self.audio_lost {
            self.audio_lost = true;
            tracing::warn!(
                "audio output {} stopped; reopening it",
                self.output_info.device
            );
            self.status_line = format!(
                "Audio output lost ({}): reconnecting when it is back...",
                self.output_info.device
            );
        }
        // The dead stream goes first (exclusive mode allows one user).
        self.output = None;
        let snap = *self.engine.snapshot();
        let old_rate = self.output_info.sample_rate;
        if let Ok(audio) = backend.open(&self.running_audio) {
            self.install_audio(audio);
            self.restore_engine_state(&snap, old_rate);
            self.audio_lost = false;
            self.audio_progress = (0, std::time::Instant::now());
            self.status_line = format!("Audio output back: {}", self.output_info.device);
        }
        self.audio_backend = Some(backend);
    }

    /// The status line text (top bar).
    pub fn status_line(&self) -> &str {
        &self.status_line
    }

    /// The audio output stopped and has not come back yet.
    pub fn audio_lost(&self) -> bool {
        self.audio_lost
    }

    fn install_audio(&mut self, audio: OpenedAudio) {
        tracing::info!(
            "audio: {} / {} at {} Hz",
            audio.info.host,
            audio.info.device,
            audio.info.sample_rate
        );
        self.engine = audio.engine;
        self.output_info = audio.info;
        self.output = Some(audio.output);
        // The controller sends to the new engine from now on.
        self.hand_controller_queue();
        // Recording goes on with the new engine's ring.
        self.new_record_reader();
        // A new engine starts counting frames from zero again.
        self.clock.audio = crate::frame_clock::AudioClock::default();
    }

    /// Carries mixer and deck settings from the old engine (`snap`) over to
    /// a new one, and reloads the decks' tracks at the new sample rate.
    fn restore_engine_state(&mut self, snap: &dj_audio::EngineSnapshot, old_rate: u32) {
        for m in snap.mixer.commands() {
            self.send(Command::Mixer(m));
        }
        // The controller's positions win over the carried-over settings:
        // ask it again now that the new engine runs (with the NS4FX
        // the audio comes back when it is plugged in again).
        if let Some(c) = &self.controller {
            c.request_resync();
        }
        let frames = self.engine.snapshot().frames_rendered;
        self.mixer_sync.wait(frames);
        self.restore_fx(&snap.fx.params);
        self.send_performance_settings();
        self.restore_sampler(snap, old_rate);
        let ratio = f64::from(self.output_info.sample_rate) / f64::from(old_rate.max(1));
        for (i, d) in snap.decks.iter().enumerate() {
            let id = DeckId::ALL[i];
            self.send(Command::Deck(id, DeckCommand::SetPitchRange(d.pitch_range)));
            self.send(Command::Deck(id, DeckCommand::SetPitch(d.pitch)));
            self.send(Command::Deck(id, DeckCommand::SetKeyLock(d.key_lock)));
            self.send(Command::Deck(id, DeckCommand::SetSlip(d.slip)));
            let ui = &mut self.decks[i];
            ui.perf.resync = d.sync;
            ui.perf.remaster = snap.sync_master == Some(i);
            ui.perf.held = [false; dj_audio::HOT_CUES];
            ui.cue_held = false;
            ui.bend_held = 0;
            ui.scrub_anchor = None;
            ui.scroll.reset();
            match ui.path.clone() {
                Some(path) => {
                    ui.resume_at = (d.loaded && d.position > 0.0).then_some(d.position * ratio);
                    let cached = ui.waveform.clone();
                    self.start_load(id, path, cached);
                }
                None if d.loaded => {
                    // Audio that came from memory (generated, tests): nothing
                    // to reload it from.
                    ui.waveform = None;
                    ui.detail = None;
                    ui.status = Some("unloaded: the audio output changed".into());
                }
                None => {}
            }
        }
    }

    /// The library state, if a library is open.
    pub fn library(&mut self) -> Option<&mut LibraryUi> {
        self.library.as_mut()
    }

    /// Library work for this frame: imports, finished analyses, play counts.
    fn poll_library(&mut self) {
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        let done = lib.poll();
        // Refresh decks that hold a track whose analysis just finished.
        for id in done {
            let Some(lib) = self.library.as_ref() else {
                break;
            };
            let grid = lib.lib.beatgrid(id).ok().flatten();
            let wf = lib.lib.waveform(id).ok().flatten().map(Arc::new);
            let track = lib.track(id).cloned();
            for i in 0..NUM_DECKS {
                if self.decks[i].track_id == Some(id) {
                    let ui = &mut self.decks[i];
                    ui.grid = grid;
                    ui.bpm = grid.map(|g| g.bpm).or(track.as_ref().and_then(|t| t.bpm));
                    ui.key = track.as_ref().and_then(|t| t.key);
                    if ui.waveform.is_none() {
                        ui.waveform = wf.clone();
                    }
                    let lufs = track.as_ref().and_then(|t| t.lufs);
                    let gain = track.as_ref().and_then(|t| t.gain_db);
                    self.set_auto_gain(DeckId::ALL[i], lufs, gain);
                    self.send_grid(i);
                }
            }
        }
        // Count a play the first time a loaded library track plays.
        let snap = *self.engine.snapshot();
        let mut played = Vec::new();
        for i in 0..NUM_DECKS {
            let ui = &mut self.decks[i];
            if let Some(id) = ui.track_id
                && !ui.play_recorded
                && snap.decks[i].playing
                && let Some(lib) = self.library.as_mut()
            {
                ui.play_recorded = true;
                let _ = lib.lib.record_play(id);
                played.push((id, i));
            }
        }
        for (id, deck) in played {
            self.history_add(id, deck);
        }
    }

    fn bump_serial(&mut self) -> u64 {
        let s = self.next_serial;
        self.next_serial += 1;
        s
    }

    fn poll_loaders(&mut self) {
        for i in 0..NUM_DECKS {
            loop {
                let Some(ev) = self.decks[i].job.as_ref().and_then(LoadJob::poll) else {
                    break;
                };
                match ev {
                    LoaderEvent::Started { audio, .. } => {
                        let serial = self.bump_serial();
                        // Grid and cues travel with the load, so the engine
                        // never shows the track without them.
                        let mut markers = self.track_markers(i);
                        let fresh = std::mem::take(&mut self.decks[i].fresh_load);
                        let double = self.decks[i].double_of.take();
                        if fresh {
                            markers.start = self.load_start(i, &markers, double);
                        }
                        self.send(Command::Deck(
                            DeckId::ALL[i],
                            DeckCommand::LoadWithMarkers {
                                audio,
                                serial,
                                markers,
                            },
                        ));
                        self.decks[i].perf.serial = serial;
                        self.decks[i].perf.send_after_load = false;
                        if std::mem::take(&mut self.decks[i].perf.resync) {
                            self.send(Command::Sync(DeckId::ALL[i], true));
                        }
                        if std::mem::take(&mut self.decks[i].perf.remaster) {
                            self.send(Command::SyncMaster(DeckId::ALL[i]));
                        }
                        if let Some(p) = self.decks[i].resume_at.take() {
                            self.send(Command::Deck(DeckId::ALL[i], DeckCommand::Seek(p)));
                        }
                        if fresh {
                            self.after_fresh_load(i, double);
                        }
                    }
                    LoaderEvent::Finished { truncated, .. } => {
                        if truncated {
                            self.decks[i].status = Some("track longer than 3 h was cut".into());
                        }
                    }
                    LoaderEvent::Waveform(wf) => {
                        self.decks[i].waveform = Some(wf);
                    }
                    // The last event of a deck load.
                    LoaderEvent::Detail(wf) => {
                        self.decks[i].detail = Some(wf);
                        self.decks[i].job = None;
                        break;
                    }
                    LoaderEvent::Failed(e) => {
                        tracing::warn!("load failed: {e}");
                        self.decks[i].status = Some(e);
                        self.decks[i].job = None;
                        break;
                    }
                }
            }
        }
    }

    /// Playhead estimate for this frame: the last reported position plus
    /// the audio rendered since that report by the paced frame time (see
    /// [`crate::frame_clock::AudioClock`]), at the deck's speed. Never more
    /// than 50 ms from the last report.
    pub(crate) fn estimate_position(&self, d: &DeckSnapshot) -> f64 {
        if !(d.playing || d.scrubbing) {
            return d.position;
        }
        let rate = f64::from(self.sample_rate());
        let since = self
            .clock
            .audio
            .frames_at(self.clock.time, rate, 0.05)
            .map_or(0.0, |f| f - self.clock.audio.last_frames() as f64);
        (d.position + d.speed * since).max(0.0)
    }

    /// Position to draw deck `index` at: the smooth scroll clock while
    /// playing, the raw estimate while paused or scrubbing (so scrubbing
    /// follows the mouse without lag). Advances once a frame, by the paced
    /// frame time, the same for every deck.
    pub(crate) fn display_position(&mut self, index: usize, d: &DeckSnapshot) -> f64 {
        let raw = self.estimate_position(d);
        let rate = f64::from(self.sample_rate());
        let (frame, dt) = (self.clock.frame, self.clock.dt);
        let Some(deck) = self.decks.get_mut(index) else {
            return raw;
        };
        if deck.scroll_frame == Some(frame) {
            return deck.scroll_pos;
        }
        deck.scroll_frame = Some(frame);
        deck.scroll_pos = if d.scrubbing {
            // Scrubbing or scratching (also a released, spinning jog):
            // smoothed with a short time constant at the current speed, so
            // the jog's bursts do not make the waveform stutter.
            deck.scroll
                .update_with_tau(raw, d.speed, rate, dt, SCRUB_SMOOTH_SECONDS)
        } else if !d.playing {
            deck.scroll.reset();
            raw
        } else {
            deck.scroll.update(raw, d.tempo, rate, dt)
        };
        deck.scroll_pos
    }

    /// Draws one frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        // Frame clock first: every deck's waveform advances by the same
        // steady step this frame.
        let now = ui.input(|i| i.time);
        let (time, dt) = self.clock.pacer.tick(now);
        self.clock.time = time;
        self.clock.dt = dt;
        self.clock.frame += 1;
        let frames = self.engine.update().frames_rendered;
        let rate = f64::from(self.sample_rate());
        self.clock.audio.observe(frames, now, rate);
        self.poll_loaders();
        self.poll_library();
        self.poll_sampler();
        self.poll_controller();
        self.poll_recorder();
        self.sync_fx_config();
        self.sync_mixer_config();
        self.watch_audio();
        self.sync_markers();
        // The applied theme is remembered in the egui context, so a new
        // context (headless runs) gets it too.
        let dark = self.config.ui.dark_theme;
        let theme_id = Id::unique("kaldor_theme_applied");
        if ui.ctx().data(|d| d.get_temp::<bool>(theme_id)) != Some(dark) {
            crate::theme::apply(ui.ctx(), dark);
            ui.ctx().data_mut(|d| d.insert_temp(theme_id, dark));
        }
        // Archivo for the name in the top bar (once per context).
        crate::theme::install_logo_font(ui.ctx());
        self.apply_fullscreen(ui.ctx());
        self.apply_ui_scale(ui.ctx());
        self.close_guard(ui.ctx());
        self.poll_library_tasks();
        self.handle_dropped_files(ui);
        self.handle_keys(ui);

        let screen = ui.max_rect();
        let mut input = layout::LayoutInput {
            size: screen.size(),
            layout: self.config.ui.layout,
            two_decks: self.config.ui.two_decks,
            layer: self.layer,
            mixer_open: self.config.ui.mixer_open,
            mixer_swap: self.mixer_swap,
            library_fraction: self.config.ui.library_height,
            fx_panel: self.config.ui.fx_panel,
        };
        // Files or History open: a taller library, and two decks while four
        // do not fit (the setting itself does not change).
        self.files_panel.two_decks = (self.files_panel.open || self.history_open())
            && crate::files_panel::make_room(&mut input);
        let mut frame = layout::compute(&input);
        frame.translate(screen.min.to_vec2());
        let pal = Palette::get(self.config.ui.dark_theme);
        ui.painter().rect_filled(screen, 0.0, pal.bg);
        self.map.begin(screen);
        self.drop_rects.clear();
        self.pad_sample_drops.clear();

        self.top_bar(ui, frame.top, frame.top_two_rows);
        for side in 0..2 {
            crate::deck_view::side_column(self, ui, &frame, side);
        }
        crate::deck_view::waveforms(self, ui, &frame);
        if let Some(r) = frame.fx {
            crate::fx_panel::fx_panel(self, ui, r);
        }
        crate::mixer_view::mixer_area(self, ui, &frame);
        self.divider(ui, frame.divider, screen);
        crate::library_view::bottom_area(self, ui, frame.bottom);
        self.paint_drop_highlight(ui);
        // The settings window, over everything.
        crate::setup::setup_window(self, ui);
        crate::relink::locate_window(self, ui.ctx());
        crate::library_dialogs::dialogs(self, ui.ctx());
        crate::usb_export::window(self, ui.ctx());
        crate::interop_view::window(self, ui.ctx());
        crate::smart_view::window(self, ui.ctx());
        crate::history_view::take_saves(self);
        self.close_question(ui.ctx());
        self.frame = Some(frame);
        // Crates opened or closed in the crate list are remembered.
        if let Some(lib) = self.library.as_ref() {
            let mut open: Vec<i64> = lib.opened.iter().copied().collect();
            open.sort_unstable();
            if open != self.config.ui.open_crates {
                self.config.ui.open_crates = open;
                self.events.push(AppEvent::ConfigChanged);
            }
        }

        if self.events.contains(&AppEvent::ConfigChanged)
            && let Some(save) = self.config_saver.as_mut()
        {
            save(&self.config);
            self.events.retain(|e| *e != AppEvent::ConfigChanged);
        }
    }

    /// Where every control was drawn in the last frame (for tests).
    pub fn ui_map(&self) -> &UiMap {
        &self.map
    }

    /// The layout of the last frame (for tests).
    pub fn layout_frame(&self) -> Option<&layout::Frame> {
        self.frame.as_ref()
    }

    /// Sets the LAYER deck of a side (0 = left: deck index 0 or 2; 1 =
    /// right: 1 or 3), like the LAYER buttons.
    pub fn set_layer(&mut self, side: usize, deck: usize) {
        if side < 2 && deck < NUM_DECKS && deck % 2 == side {
            self.layer[side] = deck;
        }
    }

    /// The LAYER decks (left, right).
    pub fn layer(&self) -> [usize; 2] {
        self.layer
    }

    /// Opens the SAMPLER tab (true) or the library (false) in the bottom area.
    pub fn show_sampler(&mut self, on: bool) {
        self.show_sampler = on;
        if on {
            self.show_setup = false;
        }
    }

    /// True when the SAMPLER tab is open in the bottom area.
    pub fn sampler_shown(&self) -> bool {
        self.show_sampler && !self.show_setup && !self.show_record
    }

    /// Shows the FX panel under the waveforms.
    pub fn show_fx(&mut self) {
        self.config.ui.centre_panel = dj_core::config::CentrePanel::Fx;
        self.config.ui.mixer_open = true;
    }

    /// Opens the RECORD tab (true) or the library (false) in the bottom area.
    pub fn show_record(&mut self, on: bool) {
        // Recording lives in the settings window now.
        self.show_record = on;
        if on {
            self.show_setup_tab(crate::setup::SetupTab::Recording);
        }
    }

    /// Shows the PADS panel (true) or the mixer (false) under the waveforms.
    pub fn show_pads(&mut self, on: bool) {
        self.config.ui.centre_panel = if on {
            dj_core::config::CentrePanel::Pads
        } else {
            dj_core::config::CentrePanel::Mixer
        };
        self.config.ui.mixer_open = true;
    }

    /// Opens the Setup tab (true) or the library (false) in the bottom area.
    pub fn show_setup(&mut self, on: bool) {
        self.show_setup = on;
    }

    /// Opens the settings window on `tab`.
    pub fn show_setup_tab(&mut self, tab: crate::setup::SetupTab) {
        self.show_setup = true;
        self.setup_tab = tab;
    }

    /// The library divider: drag to resize, double-click to reset. The
    /// position is saved in the config when the drag ends.
    fn divider(&mut self, ui: &mut egui::Ui, rect: Rect, screen: Rect) {
        self.map.enter(Region::Divider, rect);
        let pal = Palette::get(self.config.ui.dark_theme);
        let r = ui.interact(rect, Id::unique("library_divider"), Sense::click_and_drag());
        let r = r.on_hover_text("Drag to resize the library; double-click to reset");
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        let hot = r.hovered() || r.dragged();
        ui.painter()
            .rect_filled(rect, 0.0, if hot { pal.border } else { pal.bg });
        let c = rect.center();
        for dx in [-12.0, 0.0, 12.0] {
            ui.painter()
                .circle_filled(egui::pos2(c.x + dx, c.y), 1.5, pal.text_weak);
        }
        if r.double_clicked() {
            self.config.ui.library_height = dj_core::config::UiConfig::default().library_height;
            self.events.push(AppEvent::ConfigChanged);
        } else if r.dragged()
            && let Some(p) = r.interact_pointer_pos()
        {
            self.config.ui.library_height =
                layout::fraction_for_divider(screen.height(), p.y - screen.top());
        }
        if r.drag_stopped() {
            self.events.push(AppEvent::ConfigChanged);
        }
        self.map.add(ui, "divider", None, rect);
    }
}
impl KaldorApp {
    /// Sets the screen layout (also used by the smoke test).
    pub fn set_layout(&mut self, layout: DeckLayout) {
        self.config.ui.layout = layout;
    }

    /// Shows 2 decks (the LAYER decks) or all 4, in any layout.
    pub fn set_two_decks(&mut self, two: bool) {
        self.config.ui.two_decks = two;
    }

    /// Full screen on or off (kept in the config; the window follows at the
    /// next frame).
    pub fn set_fullscreen(&mut self, on: bool) {
        if self.config.ui.fullscreen != on {
            self.config.ui.fullscreen = on;
            self.events.push(AppEvent::ConfigChanged);
        }
    }

    /// Asks the window to go full screen (or back) when the setting
    /// changed since it was last sent.
    fn apply_fullscreen(&mut self, ctx: &egui::Context) {
        let want = self.config.ui.fullscreen;
        if self.fullscreen_sent != Some(want) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(want));
            self.fullscreen_sent = Some(want);
        }
    }
}

/// A child UI confined (and clipped) to `rect`.
pub(crate) fn region_ui(
    ui: &mut egui::Ui,
    rect: Rect,
    layout: Layout,
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> egui::Ui {
    let mut child = ui.new_child(UiBuilder::new().id_salt(salt).max_rect(rect).layout(layout));
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    child
}
