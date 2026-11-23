//! The redesigned controls call the same functions as before: each test
//! clicks, drags or types on a control headless (synthetic pointer and key
//! events, no window) and checks the engine state, config or library state
//! it changed.

use dj_audio::EqBand;
use dj_core::AppConfig;
use dj_core::config::{DeckLayout, WaveformColorMode};
use dj_ui::AppEvent;
use dj_ui::headless::Demo;
use dj_ui::library::{Selection, SortKey};
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, pos2, vec2};

struct T {
    demo: Demo,
    size: egui::Vec2,
}

fn moved(p: Pos2) -> Event {
    Event::PointerMoved(p)
}

fn button(p: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
    }
}

fn key(k: Key, pressed: bool) -> Event {
    Event::Key {
        key: k,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::default(),
    }
}

impl T {
    fn new(layout: DeckLayout) -> Self {
        let mut cfg = AppConfig::default();
        cfg.ui.layout = layout;
        // These tests were written with the mixer open (the default before
        // the Extended layout, where it starts closed).
        cfg.ui.mixer_open = true;
        let mut t = Self {
            demo: Demo::new(cfg),
            size: vec2(1920.0, 1080.0),
        };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        self.demo.run(n, self.size, 1.0);
    }

    /// Runs one frame per entry of `seq`, then three idle frames so the
    /// engine applies the commands and the snapshot shows them.
    fn input(&mut self, seq: Vec<Vec<Event>>) {
        let n = seq.len() + 3;
        self.demo.run_events(n, self.size, 1.0, |i| {
            seq.get(i).cloned().unwrap_or_default()
        });
    }

    fn rect(&self, name: &str, deck: Option<u8>) -> Rect {
        self.demo
            .app
            .ui_map()
            .find(name, deck)
            .first()
            .unwrap_or_else(|| panic!("control {name} (deck {deck:?}) not on screen"))
            .rect
    }

    fn click_at(&mut self, p: Pos2) {
        // Two frames before the press: a window that appears in this run
        // (the settings window) is laid out invisibly in its first frame.
        self.input(vec![
            vec![moved(p)],
            vec![moved(p)],
            vec![button(p, true)],
            vec![button(p, false)],
        ]);
    }

    fn click(&mut self, name: &str, deck: Option<u8>) {
        let p = self.rect(name, deck).center();
        self.click_at(p);
    }

    /// Scrolls the settings window until the control is in view, then
    /// clicks it.
    fn click_setting(&mut self, name: &str, idx: Option<u8>) {
        // Positions as a fresh run lays them out (not scrolled).
        self.frames(2);
        let (r, clip) = self
            .demo
            .app
            .ui_map()
            .find(name, idx)
            .first()
            .map(|c| (c.rect, c.clip))
            .unwrap_or_else(|| panic!("control {name} ({idx:?}) not in the settings"));
        if clip.shrink(2.0).contains(r.center()) {
            self.click_at(r.center());
            return;
        }
        // Each run starts a new egui context (the scroll position does not
        // carry over), so scroll, let egui's scroll animation settle and
        // click in one run. The control comes to the middle of the view.
        let target = clip.center().y;
        let dy = target - r.center().y;
        let at = clip.center();
        let p = pos2(r.center().x, target);
        let mut seq = vec![vec![moved(at)], vec![moved(at)], vec![moved(at)]];
        seq.push(vec![Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, dy),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Modifiers::default(),
        }]);
        seq.extend(std::iter::repeat_n(vec![], 40));
        seq.extend([
            vec![moved(p)],
            vec![moved(p)],
            vec![button(p, true)],
            vec![button(p, false)],
        ]);
        self.input(seq);
    }

    fn double_click(&mut self, name: &str, deck: Option<u8>) {
        let p = self.rect(name, deck).center();
        self.input(vec![
            vec![moved(p)],
            vec![button(p, true)],
            vec![button(p, false)],
            vec![button(p, true)],
            vec![button(p, false)],
        ]);
    }

    /// Press at `from`, move to `to` in steps, release.
    fn drag(&mut self, from: Pos2, to: Pos2) {
        let mut seq = vec![vec![moved(from)], vec![button(from, true)]];
        for k in 1..=6 {
            seq.push(vec![moved(from + (to - from) * (k as f32 / 6.0))]);
        }
        seq.push(vec![button(to, false)]);
        self.input(seq);
    }

    fn drag_control(&mut self, name: &str, deck: Option<u8>, d: egui::Vec2) {
        let c = self.rect(name, deck).center();
        self.drag(c, c + d);
    }

    /// Press and keep holding (the hold ends with the next run).
    fn hold(&mut self, name: &str, deck: Option<u8>) {
        let p = self.rect(name, deck).center();
        self.demo.run_events(30, self.size, 1.0, |i| match i {
            0 => vec![moved(p)],
            1 => vec![button(p, true)],
            _ => vec![],
        });
    }

    fn snap(&self) -> dj_audio::EngineSnapshot {
        *self.demo.app.snapshot()
    }

    fn config_changed(&mut self) -> bool {
        self.demo
            .app
            .take_events()
            .contains(&AppEvent::ConfigChanged)
    }
}

/// The settings window after the 2026-11-15 review: every setting is on
/// its tab, and the new rows change the config.
#[test]
fn settings_tabs_and_new_rows() {
    let mut t = T::new(DeckLayout::Extended);
    t.click("settings", None);
    // (The audio picker's sample rate row is checked in audio_switch: this
    // demo has no device choice.)
    // Decks: sync and quantize moved here from Display, plus the sizes.
    t.click("setup_tab_decks", None);
    for name in [
        "quantize",
        "smart_sync",
        "jump_size",
        "auto_loop_size",
        "slicer_size",
        "sampler_record_beats",
    ] {
        t.rect(name, None);
    }
    assert!(t.demo.app.config().performance.key_lock_default);
    t.click_setting("key_lock_default", None);
    assert!(!t.demo.app.config().performance.key_lock_default);
    assert!(t.config_changed());
    // Display: no deck settings any more; waveform zoom and key notation.
    t.click("setup_tab_display", None);
    assert!(t.demo.app.ui_map().find("quantize", None).is_empty());
    t.rect("waveform_zoom", None);
    // Options are numbered from 1 in the UI map.
    t.click_setting("key_notation", Some(3));
    assert_eq!(
        t.demo.app.config().ui.key_notation,
        dj_core::KeyNotation::Musical
    );
    assert!(t.config_changed());
    t.click_setting("key_notation", Some(1));
    assert_eq!(
        t.demo.app.config().ui.key_notation,
        dj_core::KeyNotation::Camelot
    );
    // Mixer: auto gain and its target.
    t.click("setup_tab_mixer", None);
    t.rect("target_lufs", None);
    assert!(t.demo.app.config().library.auto_gain);
    t.click_setting("auto_gain", None);
    assert!(!t.demo.app.config().library.auto_gain);
    assert!(t.config_changed());
    t.rect("mixer_defaults", None);
    // Library: the analysis settings; no import or export there any more
    // (Files panel, and Export… in a crate's right-click menu).
    t.click("setup_tab_library", None);
    for name in ["bpm_min", "bpm_max", "analysis_workers", "reanalyse_all"] {
        t.rect(name, None);
    }
    assert!(t.rect("bpm_min", None).top() < t.rect("reanalyse_all", None).top());
    for gone in [
        "import",
        "import_path",
        "export",
        "export_path",
        "usb_export",
        "xml_auto",
    ] {
        assert!(t.demo.app.ui_map().find(gone, None).is_empty(), "{gone}");
    }
    // Controller: whether to use one.
    t.click("setup_tab_controller", None);
    assert!(t.demo.app.config().controller.enabled);
    t.click_setting("controller_enabled", None);
    assert!(!t.demo.app.config().controller.enabled);
}

/// D18, D19, D23-D25: transport, bend and key lock.
#[test]
fn deck_transport_bend_and_key_lock() {
    let mut t = T::new(DeckLayout::FourStacked);
    assert!(t.snap().decks[0].playing);
    t.click("play", Some(1));
    assert!(!t.snap().decks[0].playing, "PLAY toggles to pause");
    t.click("play", Some(1));
    assert!(t.snap().decks[0].playing, "PLAY toggles back");
    t.click("play", Some(1));

    // CUE held on a paused deck previews; release stops.
    t.hold("cue", Some(1));
    assert!(t.snap().decks[0].previewing, "CUE hold previews");
    t.frames(4);
    assert!(!t.snap().decks[0].previewing, "CUE release stops");

    t.hold("bend_up", Some(1));
    assert!(t.snap().decks[0].bend > 0.0, "bend + while held");
    t.frames(4);
    assert_eq!(t.snap().decks[0].bend, 0.0, "bend ends on release");
    t.hold("bend_down", Some(1));
    assert!(t.snap().decks[0].bend < 0.0, "bend − while held");
    t.frames(4);

    // Key lock is on from the start (config key_lock_default).
    assert!(t.snap().decks[0].key_lock);
    t.click("key_lock", Some(1));
    assert!(!t.snap().decks[0].key_lock, "key lock off");
    t.click("key_lock", Some(1));
    assert!(t.snap().decks[0].key_lock, "key lock on");
}

/// D20-D22: pitch fader (top = −), double-click and "0" reset, typed value.
#[test]
fn pitch_controls() {
    let mut t = T::new(DeckLayout::FourStacked);
    t.drag_control("pitch_fader", Some(1), vec2(0.0, 40.0));
    let p = t.snap().decks[0].pitch;
    assert!(p > 0.05, "dragging down raises the pitch: {p}");
    t.click("pitch_reset", Some(1));
    assert_eq!(t.snap().decks[0].pitch, 0.0, "0 resets");
    t.drag_control("pitch_fader", Some(1), vec2(0.0, -40.0));
    assert!(
        t.snap().decks[0].pitch < -0.05,
        "dragging up lowers the pitch"
    );
    t.double_click("pitch_fader", Some(1));
    assert_eq!(t.snap().decks[0].pitch, 0.0, "double-click resets");
    // The value field: drag right raises the pitch.
    t.drag_control("pitch_value", Some(1), vec2(60.0, 0.0));
    assert!(t.snap().decks[0].pitch > 0.0, "value field drag");
}

/// D14, D15, D17: scrub, wheel zoom, overview seek.
#[test]
fn waveform_scrub_zoom_and_seek() {
    let mut t = T::new(DeckLayout::FourStacked);
    t.click("play", Some(1));
    let before = t.snap().decks[0].position;
    t.drag_control("wave", Some(1), vec2(-200.0, 0.0));
    let after = t.snap().decks[0].position;
    assert!(
        after > before + 1_000.0,
        "scrub left moves forward: {before} -> {after}"
    );
    assert!(!t.snap().decks[0].scrubbing, "scrub ends on release");

    // Wheel zoom in steps: one notch up zooms in by a third of a doubling
    // (egui spreads the notch over several frames), not all the way.
    let p = t.rect("wave", Some(1)).center();
    let notches = |t: &mut T, n: i32| {
        let mut seq = vec![vec![moved(p)]];
        for _ in 0..n.abs() {
            seq.push(vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, n.signum() as f32),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Modifiers::default(),
            }]);
            seq.extend(std::iter::repeat_n(vec![], 6));
        }
        seq.extend(std::iter::repeat_n(vec![], 60));
        t.input(seq);
    };
    t.demo.app.config_mut_for_test().ui.zoom_beats = 8.0;
    notches(&mut t, 1);
    let z = t.demo.app.config().ui.zoom_beats;
    let want = 8.0 * 2f32.powf(-1.0 / 3.0);
    assert!(
        (z - want).abs() < 0.15,
        "one notch up: 8 -> {z}, want {want:.2}"
    );
    notches(&mut t, -1);
    let z = t.demo.app.config().ui.zoom_beats;
    assert!((z - 8.0).abs() < 0.15, "one notch down: back to 8, got {z}");
    // The range: 2 beats (closest) to 128 (farthest), 18 notches apart.
    use dj_core::config::{ZOOM_MAX_BEATS, ZOOM_MIN_BEATS};
    assert_eq!((ZOOM_MIN_BEATS, ZOOM_MAX_BEATS), (2.0, 128.0));
    t.demo.app.config_mut_for_test().ui.zoom_beats = 128.0;
    notches(&mut t, 17);
    let z = t.demo.app.config().ui.zoom_beats;
    assert!(z > 2.3 && z < 2.8, "17 notches in from 128: {z}");
    notches(&mut t, 3);
    assert_eq!(t.demo.app.config().ui.zoom_beats, 2.0, "stops at 2 beats");
    notches(&mut t, -20);
    assert_eq!(
        t.demo.app.config().ui.zoom_beats,
        128.0,
        "zooms out to 128 beats and stops"
    );
    // A config saved with the old closest zoom shows 2 beats.
    t.demo.app.config_mut_for_test().ui.zoom_beats = 1.0;
    assert_eq!(t.demo.app.waveform_window_seconds(), 1.0);
    t.demo.app.config_mut_for_test().ui.zoom_beats = 8.0;

    let ov = t.rect("overview", Some(1));
    t.click_at(pos2(ov.left() + ov.width() * 0.75, ov.center().y));
    let d = t.snap().decks[0];
    let frac = d.position / d.length;
    assert!((frac - 0.75).abs() < 0.03, "overview seek to {frac}");
}

/// T2-T9: the top bar (clock, layout menu, 2 / 4 decks, master volume,
/// settings), LAYER, and the display settings in Setup.
#[test]
fn top_bar_and_layer() {
    let mut t = T::new(DeckLayout::FourStacked);
    t.rect("clock", None);
    t.rect("cpu", None);
    t.rect("master_meter", None);
    t.click("layout_menu", None);
    t.click_at(pos2(5.0, 400.0)); // close the menu
    // 2 / 4 decks, in any layout.
    t.click("decks_2", None);
    assert!(t.demo.app.config().ui.two_decks);
    assert!(t.config_changed());
    t.click("decks_4", None);
    assert!(!t.demo.app.config().ui.two_decks);

    t.click("layer", Some(3));
    assert_eq!(t.demo.app.layer(), [2, 1]);
    t.click("layer", Some(4));
    assert_eq!(t.demo.app.layer(), [2, 3]);
    // With 2 decks the LAYER decks are the ones shown.
    t.click("decks_2", None);
    assert!(!t.demo.app.ui_map().find("play", Some(3)).is_empty());
    assert!(t.demo.app.ui_map().find("play", Some(1)).is_empty());
    t.click("layer", Some(1));
    assert!(!t.demo.app.ui_map().find("play", Some(1)).is_empty());
    t.click("decks_4", None);

    // Master volume: starts at 12 o'clock (-9 dB, below the limiter); drag
    // up for louder, double-click back to 12 o'clock.
    assert_eq!(t.snap().mixer.master_gain_db, -9.0, "the start level");
    t.drag_control("master_volume", None, vec2(0.0, -20.0));
    assert!(
        t.snap().mixer.master_gain_db > -8.0,
        "{}",
        t.snap().mixer.master_gain_db
    );
    t.double_click("master_volume", None);
    assert_eq!(t.snap().mixer.master_gain_db, -9.0);

    // Settings opens the settings window (over the app) on the Audio tab;
    // each section has a tab.
    t.click("settings", None);
    assert!(
        !t.demo.app.ui_map().find("tone_off", None).is_empty(),
        "settings shown"
    );
    assert!(
        t.demo
            .app
            .ui_map()
            .region(dj_ui::uimap::Region::Settings)
            .is_some()
    );
    for tab in [
        "setup_tab_audio",
        "setup_tab_decks",
        "setup_tab_mixer",
        "setup_tab_display",
        "setup_tab_library",
        "setup_tab_controller",
        "setup_tab_keyboard",
        "setup_tab_recording",
    ] {
        t.rect(tab, None);
    }
    t.click("setup_tab_library", None);
    t.rect("reanalyse_all", None);
    t.click("setup_tab_recording", None);
    t.rect("record_wav", None);
    // Recording starts from REC in the top bar, not from the settings.
    assert!(
        t.demo.app.ui_map().find("record_start", None).is_empty(),
        "no start button in the settings"
    );
    t.click("setup_tab_mixer", None);
    t.rect("mixer_defaults", None);
    // Theme on the Display tab.
    t.click("setup_tab_display", None);
    t.click("theme", None);
    assert!(!t.demo.app.config().ui.dark_theme, "light theme");
    assert!(t.config_changed());
    t.click("theme", None);
    assert!(t.demo.app.config().ui.dark_theme);

    // Full screen: the Setup switch and Shift+F11, saved in the config.
    t.click("fullscreen", None);
    assert!(t.demo.app.config().ui.fullscreen);
    assert!(t.config_changed());
    t.demo
        .app
        .key_action(dj_core::config::KeyAction::ToggleFullscreen, true);
    assert!(!t.demo.app.config().ui.fullscreen);
    assert!(t.demo.app.config().ui.keymap.iter().any(|b| b.key == "F11"
        && b.shift
        && b.action == dj_core::config::KeyAction::ToggleFullscreen));
    t.click("close_setup", None);
    assert!(
        !t.demo.app.ui_map().find("search", None).is_empty(),
        "library back"
    );
    t.click("settings", None);
    t.click("setup_tab_audio", None);
    t.click("tone_master", None);
    assert_eq!(t.snap().test_tone, Some(dj_audio::ToneTarget::Master));
    t.click("tone_off", None);
    assert_eq!(t.snap().test_tone, None);
    t.click("close_setup", None);

    // Waveform colours (Setup): the combo opens; the colour mode itself is
    // a config value read by the strips.
    assert_eq!(
        t.demo.app.config().ui.waveform_colors,
        WaveformColorMode::ThreeBand
    );
    t.click("settings", None);
    t.click("setup_tab_display", None);
    t.click("waveform_colors", None);
    // Escape closes the window.
    t.input(vec![
        vec![key(Key::Escape, true)],
        vec![key(Key::Escape, false)],
    ]);
    assert!(
        t.demo
            .app
            .ui_map()
            .region(dj_ui::uimap::Region::Settings)
            .is_none()
    );
}

/// M2-M20: mixer controls.
#[test]
#[ignore = "the PADS / FX / mixer panel is off screen in every layout; re-enable when it has a new place"]
fn mixer_controls() {
    let mut t = T::new(DeckLayout::FourStacked);
    let ch = |t: &T, i: usize| t.snap().mixer.channels[i];
    t.drag_control("trim", Some(1), vec2(0.0, -30.0));
    assert!(ch(&t, 0).trim > 0.2, "trim up {}", ch(&t, 0).trim);
    t.double_click("trim", Some(1));
    assert_eq!(ch(&t, 0).trim, 0.0, "trim reset");
    t.drag_control("eq_low", Some(2), vec2(0.0, 30.0));
    assert!(
        ch(&t, 1).eq[EqBand::Low as usize] < 0.4,
        "low down: {:?}",
        ch(&t, 1).eq
    );
    t.double_click("eq_low", Some(2));
    assert_eq!(ch(&t, 1).eq[EqBand::Low as usize], 0.5, "low reset");
    t.drag_control("eq_hi", Some(3), vec2(0.0, -30.0));
    assert!(ch(&t, 2).eq[EqBand::High as usize] > 0.6, "hi up");
    t.drag_control("eq_mid", Some(3), vec2(0.0, 30.0));
    assert!(ch(&t, 2).eq[EqBand::Mid as usize] < 0.4, "mid down");
    t.drag_control("filter", Some(4), vec2(0.0, -30.0));
    assert!(ch(&t, 3).filter > 0.2, "filter");
    for (name, band) in [
        ("kill_hi", EqBand::High as usize),
        ("kill_mid", EqBand::Mid as usize),
        ("kill_low", EqBand::Low as usize),
    ] {
        t.click(name, Some(1));
        assert!(ch(&t, 0).kill[band], "{name} on");
        t.click(name, Some(1));
        assert!(!ch(&t, 0).kill[band], "{name} off");
    }
    let pfl = ch(&t, 1).pfl;
    t.click("pfl", Some(2));
    assert_eq!(ch(&t, 1).pfl, !pfl, "PFL toggles");
    t.drag_control("fader", Some(1), vec2(0.0, 40.0));
    assert!(ch(&t, 0).fader < 0.9, "fader down");
    t.double_click("fader", Some(1));
    assert_eq!(ch(&t, 0).fader, 1.0, "fader double-click full");
    let assign = t
        .demo
        .app
        .ui_map()
        .find("xf_assign", Some(1))
        .iter()
        .map(|c| c.rect)
        .collect::<Vec<_>>();
    t.click_at(assign[0].center());
    assert_eq!(ch(&t, 0).xfader, dj_audio::XfaderSide::A);
    t.click_at(assign[2].center());
    assert_eq!(ch(&t, 0).xfader, dj_audio::XfaderSide::B);
    t.click_at(assign[1].center());
    assert_eq!(ch(&t, 0).xfader, dj_audio::XfaderSide::Thru);

    t.drag_control("crossfader", None, vec2(80.0, 0.0));
    assert!(t.snap().mixer.crossfader > 0.1, "crossfader right");
    t.double_click("crossfader", None);
    assert_eq!(t.snap().mixer.crossfader, 0.0, "crossfader centre");
    let rev = t.snap().mixer.crossfader_reverse;
    t.click("xf_reverse", None);
    assert_eq!(t.snap().mixer.crossfader_reverse, !rev);
    let lim = t.snap().mixer.limiter;
    t.click("limiter", None);
    assert_eq!(t.snap().mixer.limiter, !lim, "limiter toggles");
    t.drag_control("master_gain", None, vec2(0.0, 30.0));
    assert!(t.snap().mixer.master_gain_db < -10.0, "master down");
    t.double_click("master_gain", None);
    assert_eq!(
        t.snap().mixer.master_gain_db,
        -9.0,
        "master reset to 12 o'clock"
    );
    t.drag_control("booth_gain", None, vec2(0.0, 30.0));
    assert!(t.snap().mixer.booth_gain_db < -1.0, "booth down");
    t.drag_control("phones_gain", None, vec2(0.0, 30.0));
    assert!(t.snap().mixer.cue_gain_db < -1.0, "phones down");
    t.drag_control("cue_mix", None, vec2(0.0, -40.0));
    assert!(t.snap().mixer.cue_mix > 0.1, "cue mix");
}

/// Mixer switch and library divider are saved in the config.
#[test]
#[ignore = "the PADS / FX / mixer panel is off screen in every layout; re-enable when it has a new place"]
fn mixer_switch_and_divider() {
    let mut t = T::new(DeckLayout::FourStacked);
    t.click("mixer_toggle", None);
    assert!(!t.demo.app.config().ui.mixer_open);
    assert!(t.config_changed());
    assert!(
        t.demo.app.ui_map().find("trim", Some(1)).is_empty(),
        "mixer hidden"
    );
    t.click("mixer_toggle", None);
    assert!(t.demo.app.config().ui.mixer_open);
    assert!(
        !t.demo.app.ui_map().find("trim", Some(1)).is_empty(),
        "mixer back"
    );

    let h0 = t.demo.app.config().ui.library_height;
    t.drag_control("divider", None, vec2(0.0, -100.0));
    let h1 = t.demo.app.config().ui.library_height;
    assert!(
        h1 > h0 + 0.05,
        "divider up makes the library taller: {h0} -> {h1}"
    );
    assert!(t.config_changed(), "saved when the drag ends");
    t.double_click("divider", None);
    assert!((t.demo.app.config().ui.library_height - 0.30).abs() < 1e-6);
}

/// L2, L20, L23, L27: search, sorting, double-click and drag to a deck.
#[test]
fn library_controls() {
    let mut t = T::new(DeckLayout::FourStacked);
    t.click("BPM", None);
    assert_eq!(t.demo.app.library().unwrap().sort, (SortKey::Bpm, true));
    t.click("BPM", None);
    assert_eq!(t.demo.app.library().unwrap().sort, (SortKey::Bpm, false));
    t.click("all_tracks", None);
    assert_eq!(t.demo.app.library().unwrap().selection, Selection::All);

    // Typing in the search field.
    let s = t.rect("search", None).center();
    t.input(vec![
        vec![moved(s)],
        vec![button(s, true)],
        vec![button(s, false)],
        vec![Event::Text("night".into())],
    ]);
    assert_eq!(t.demo.app.library().unwrap().search, "night");
    t.click("search_clear", None);
    assert_eq!(t.demo.app.library().unwrap().search, "");

    // Double-click with every deck playing: refused with a message.
    let first = t.demo.app.library().unwrap().view()[0];
    let row = t.rect("track_row", None).center();
    t.input(vec![
        vec![moved(row)],
        vec![button(row, true)],
        vec![button(row, false)],
        vec![button(row, true)],
        vec![button(row, false)],
    ]);
    assert!(
        t.demo
            .app
            .library()
            .unwrap()
            .status
            .contains("all decks are playing")
    );
    // Deck 4 paused: the double-click loads there.
    t.click("play", Some(4));
    t.input(vec![
        vec![moved(row)],
        vec![button(row, true)],
        vec![button(row, false)],
        vec![button(row, true)],
        vec![button(row, false)],
    ]);
    let name = t
        .demo
        .app
        .library()
        .unwrap()
        .track(first)
        .unwrap()
        .display_name();
    t.frames(3);
    let title = t.rect("title", Some(4));
    let _ = title;
    assert_eq!(t.demo.app.deck_title(dj_core::DeckId::ALL[3]), name);

    // Drag the first row onto deck 2's waveform strip.
    let target = t.rect("wave", Some(2)).center();
    let mut seq = vec![vec![moved(row)], vec![button(row, true)]];
    for k in 1..=8 {
        seq.push(vec![moved(row + (target - row) * (k as f32 / 8.0))]);
    }
    seq.push(vec![moved(target)]);
    seq.push(vec![button(target, false)]);
    t.input(seq);
    assert_eq!(t.demo.app.deck_title(dj_core::DeckId::ALL[1]), name);
}

/// D26: path field and Load.
#[test]
fn load_by_path() {
    let mut t = T::new(DeckLayout::FourStacked);
    let f = t.rect("path", Some(3)).center();
    let l = t.rect("load", Some(3)).center();
    t.input(vec![
        vec![moved(f)],
        vec![button(f, true)],
        vec![button(f, false)],
        vec![Event::Text("no such folder/missing.wav".into())],
        vec![moved(l)],
        vec![button(l, true)],
        vec![button(l, false)],
    ]);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while t.demo.app.deck_status(dj_core::DeckId::ALL[2]).is_none() {
        assert!(std::time::Instant::now() < deadline, "no load attempt");
        t.frames(1);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// K1-K8: the keyboard still drives the decks and mixer.
#[test]
fn keyboard() {
    let mut t = T::new(DeckLayout::FourStacked);
    t.input(vec![vec![key(Key::W, true)], vec![key(Key::W, false)]]);
    assert!(!t.snap().decks[0].playing, "W pauses deck 1");
    t.input(vec![vec![key(Key::Z, true)], vec![key(Key::Z, false)]]);
    assert!(
        (t.snap().mixer.crossfader + 0.1).abs() < 1e-6,
        "Z moves the crossfader"
    );
    t.input(vec![vec![key(Key::X, true)], vec![key(Key::X, false)]]);
    assert_eq!(t.snap().mixer.crossfader, 0.0, "X centres");
    let kl = t.snap().decks[0].key_lock;
    t.input(vec![vec![key(Key::T, true)], vec![key(Key::T, false)]]);
    assert_eq!(t.snap().decks[0].key_lock, !kl, "T toggles key lock");
    let pfl = t.snap().mixer.channels[0].pfl;
    t.input(vec![vec![key(Key::Y, true)], vec![key(Key::Y, false)]]);
    assert_eq!(t.snap().mixer.channels[0].pfl, !pfl, "Y PFL");
    let z = t.demo.app.config().ui.zoom_beats;
    t.input(vec![
        vec![key(Key::Plus, true)],
        vec![key(Key::Plus, false)],
    ]);
    assert_eq!(t.demo.app.config().ui.zoom_beats, z / 2.0, "+ zooms in");
}

/// The Extended layout (the default): LAYER from the deck badges, the hot
/// cue list (set, delete), loop sizes and halving, the "…" popup with the
/// rest of the deck's controls, and the tab bar that opens the panel under
/// the waveforms and closes it again.
#[test]
fn extended_panel_controls() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.ui.layout, DeckLayout::Extended, "the default layout");
    assert!(
        !cfg.ui.mixer_open,
        "the panel under the waveforms starts closed"
    );
    let mut t = T {
        demo: Demo::new(cfg),
        size: vec2(1920.0, 1080.0),
    };
    t.frames(3);
    // LAYER: the badges of decks 3 and 4, then back to 1 and 2.
    t.click("layer", Some(3));
    t.click("layer", Some(4));
    assert_eq!(t.demo.app.layer(), [2, 3]);
    t.click("layer", Some(1));
    t.click("layer", Some(2));
    assert_eq!(t.demo.app.layer(), [0, 1]);

    // Hot cue list: eight rows; an empty row sets the cue, × deletes it.
    assert_eq!(t.demo.app.ui_map().find("cue_row", Some(1)).len(), 8);
    assert!(t.snap().decks[0].hot_cues[0].is_none());
    let row = t.demo.app.ui_map().find("cue_row", Some(1))[0]
        .rect
        .center();
    t.click_at(row);
    assert!(t.snap().decks[0].hot_cues[0].is_some(), "cue 1 set");
    t.frames(2);
    t.click("cue_delete", Some(1));
    assert!(t.snap().decks[0].hot_cues[0].is_none(), "cue 1 deleted");

    // Loop length: one control. The wheel doubles / halves the size (and
    // the running loop), a click starts and ends a loop of that length.
    for gone in [
        "loop_size",
        "pitch_value",
        "pitch_range",
        "pitch_reset",
        "bend_up",
        "loop_in",
        "loop_half",
    ] {
        assert!(
            t.demo.app.ui_map().find(gone, Some(1)).is_empty(),
            "{gone} is not on the panel any more"
        );
    }
    let start = t.demo.app.config().performance.auto_loop_beats;
    let lp = t.rect("loop_length", Some(1)).center();
    let wheel = |up: bool| {
        vec![
            vec![moved(lp)],
            vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Line,
                delta: vec2(0.0, if up { 1.0 } else { -1.0 }),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Modifiers::default(),
            }],
        ]
    };
    t.input(wheel(false));
    t.click_at(lp);
    let l = t.snap().decks[0].loop_info.expect("loop");
    assert!(l.active && (l.beats - start / 2.0).abs() < 1e-4, "{l:?}");
    t.input(wheel(true));
    let l = t.snap().decks[0].loop_info.expect("loop");
    assert!(l.active && (l.beats - start).abs() < 1e-4, "{l:?}");
    t.click_at(lp);
    assert!(
        !t.snap().decks[0].loop_info.is_some_and(|l| l.active),
        "ended"
    );

    // No "…" (deck popup) any more.
    assert!(t.demo.app.ui_map().find("deck_more", None).is_empty());

    // No MIXER / PADS / FX bar or panel in this layout, even when the
    // config asks for one.
    t.demo.app.show_pads(true);
    t.frames(2);
    let map = t.demo.app.ui_map();
    for gone in [
        "mixer_toggle",
        "tab_mixer",
        "tab_pads",
        "tab_fx",
        "pad",
        "trim",
    ] {
        assert!(
            map.find(gone, None).is_empty() && map.find(gone, Some(1)).is_empty(),
            "{gone}"
        );
    }
}

/// The library: no tabs; the new-crate button (next to the search field)
/// makes "New crate", selects it and opens it for renaming with the name
/// selected, so typing replaces it; Enter saves.
/// FX in the top bar next to 2 and 4 opens the FX
/// display between the four decks. It only shows what is on (the NS4FX
/// operates it): clicking it changes nothing.
#[test]
fn fx_panel_between_the_decks() {
    let mut t = T::new(DeckLayout::Extended);
    assert!(t.demo.app.ui_map().find("fx_panel", None).is_empty());
    // FX sits right of the 4 button.
    let (fx, four) = (t.rect("fx_toggle", None), t.rect("decks_4", None));
    assert!(fx.left() >= four.right(), "{fx:?} {four:?}");
    t.click("fx_toggle", None);
    assert!(t.demo.app.config().ui.fx_panel);
    assert!(t.config_changed());
    // Between the left decks (1, 3) and the right ones (2, 4).
    let panel = t.rect("fx_panel", None);
    // (Map indices are 1-based: deck 1, effect 1, side 1.)
    let (d1, d2) = (t.rect("panel", Some(1)), t.rect("panel", Some(2)));
    assert!(
        d1.right() <= panel.left() && panel.right() <= d2.left(),
        "{d1:?} {panel:?} {d2:?}"
    );
    // Echo on the left side, dub echo on the filter knobs (as the NS4FX
    // sets them): the display shows the effect, both sides and the color.
    let mut cmds = Vec::new();
    for u in 0..2 {
        dj_ui::fx_panel::pick_commands(u, Some(3), 0.5, &mut cmds);
    }
    dj_ui::fx_panel::side_commands(0, true, &mut cmds);
    for c in cmds {
        t.demo.app.send(dj_audio::Command::Fx(c));
    }
    t.demo
        .app
        .send(dj_audio::Command::Mixer(dj_audio::MixerCommand::ColorFx(
            dj_audio::ColorFx::DubEcho,
        )));
    t.frames(3);
    for (name, n) in [
        ("fx_name", 1),
        ("fx_side", 2),
        ("fx_wet", 1),
        ("fx_color", 1),
    ] {
        let found = t.demo.app.ui_map().find(name, None).len();
        assert_eq!(found, n, "{name}");
    }
    for c in t.demo.app.ui_map().find("fx_name", None) {
        assert!(panel.contains_rect(c.rect), "inside the display");
    }
    // Display only: a click on it changes nothing.
    let before = t.demo.app.snapshot().fx.params;
    t.click("fx_panel", None);
    assert_eq!(t.demo.app.snapshot().fx.params, before);
    // FX again closes it.
    t.click("fx_toggle", None);
    t.frames(2);
    assert!(t.demo.app.ui_map().find("fx_panel", None).is_empty());
}

#[test]
fn library_new_crate_button_and_no_tabs() {
    let mut t = T::new(DeckLayout::Extended);
    for gone in [
        "tab_library",
        "tab_sampler",
        "tab_record",
        "tab_setup",
        "import",
    ] {
        assert!(t.demo.app.ui_map().find(gone, None).is_empty(), "{gone}");
    }
    // The new crate button at the left end (above the crate list), the
    // search field at the right end.
    let search = t.rect("search", None);
    let add = t.rect("new_crate", None);
    let crates = t.rect("all_tracks", None);
    assert!(add.left() <= crates.left() + 2.0, "{add:?} vs {crates:?}");
    assert!(search.left() > add.right() + 200.0);
    // Click and type in one run (a new run starts a fresh egui context,
    // which drops the keyboard focus).
    let p = t.rect("new_crate", None).center();
    let click_and_type = |text: &str| {
        let mut seq = vec![
            vec![moved(p)],
            vec![button(p, true)],
            vec![button(p, false)],
            vec![],
            vec![],
        ];
        if !text.is_empty() {
            seq.push(vec![Event::Text(text.into())]);
        }
        seq.push(vec![key(Key::Enter, true)]);
        seq.push(vec![key(Key::Enter, false)]);
        seq
    };
    t.input(click_and_type("Peak time"));
    let names = |t: &mut T| -> Vec<String> {
        t.demo
            .app
            .library()
            .expect("library")
            .crate_list()
            .into_iter()
            .map(|(_, n, _)| n)
            .collect()
    };
    // Typing replaced the selected "New crate"; Enter saved it.
    let n = names(&mut t);
    assert!(n.contains(&"Peak time".to_string()), "{n:?}");
    assert!(!n.contains(&"New crate".to_string()), "{n:?}");
    assert!(t.demo.app.library().unwrap().renaming.is_none());
    // Without typing, Enter keeps the free name.
    t.input(click_and_type(""));
    let n = names(&mut t);
    assert!(n.contains(&"New crate".to_string()), "{n:?}");
}

/// The Extended deck panel's hot cue area has two tabs stacked on its
/// outer edge: CUES (the hot cue list) and GRID (beatgrid
/// controls). The grid controls edit the deck's beatgrid: slip, halve,
/// double, lock.
#[test]
fn extended_grid_tab_edits_the_beatgrid() {
    let mut t = T::new(DeckLayout::Extended);
    let deck = dj_core::DeckId::ALL[0];
    let grid = |t: &T| {
        t.demo
            .app
            .deck_grid(deck)
            .expect("the demo track has a grid")
    };
    // Two tabs, stacked, on the outer edge: left of deck 1's cues, right
    // of deck 2's.
    let (cues, grid_tab) = (t.rect("tab_cues", Some(1)), t.rect("tab_grid", Some(1)));
    assert!(
        grid_tab.top() >= cues.bottom() - 0.5 && (grid_tab.left() - cues.left()).abs() < 0.5,
        "stacked: {cues:?} {grid_tab:?}"
    );
    assert!(cues.right() <= t.rect("cue_row", Some(1)).left());
    assert!(t.rect("tab_cues", Some(2)).left() >= t.rect("cue_row", Some(2)).right());
    // CUES first: no grid controls.
    assert!(t.demo.app.ui_map().find("grid_beat1", Some(1)).is_empty());

    t.click("tab_grid", Some(1));
    assert!(
        t.demo.app.ui_map().find("cue_row", Some(1)).is_empty(),
        "the cue list makes way"
    );
    assert_eq!(
        t.demo.app.ui_map().find("cue_row", Some(2)).len(),
        8,
        "deck 2 keeps its cues"
    );
    for name in [
        "grid_beat1",
        "grid_lock",
        "grid_slip_earlier",
        "grid_slip_later",
        "grid_stretch_down",
        "grid_stretch_up",
        "grid_halve",
        "grid_double",
        "grid_tap",
    ] {
        t.rect(name, Some(1));
    }

    // SLIP later: the whole grid moves 2 ms later.
    let g0 = grid(&t);
    t.click("grid_slip_later", Some(1));
    let g1 = grid(&t);
    let moved = (g0.beat_at(10.0) - g1.beat_at(10.0)) * g1.period();
    assert!((moved - 0.002).abs() < 1e-6, "slipped {moved} s");
    // ÷2 and ×2.
    t.click("grid_halve", Some(1));
    assert!((grid(&t).bpm - g1.bpm / 2.0).abs() < 1e-9, "halved");
    t.click("grid_double", Some(1));
    assert!((grid(&t).bpm - g1.bpm).abs() < 1e-9, "doubled back");
    // LOCK: no edits until unlocked.
    t.click("grid_lock", Some(1));
    assert!(grid(&t).locked);
    t.click("grid_slip_later", Some(1));
    t.click("grid_halve", Some(1));
    assert_eq!(grid(&t).bpm, g1.bpm, "a locked grid stays");
    assert!((grid(&t).beat_at(10.0) - g1.beat_at(10.0)).abs() < 1e-9);
    t.click("grid_lock", Some(1));
    assert!(!grid(&t).locked);

    // BEAT 1 makes the bar at the playhead bar 1 (the first kick; the bar
    // before it is -1, there is no bar 0), and SLIP carries it along with
    // the grid instead of renumbering from the track start.
    let sr = f64::from(t.snap().sample_rate);
    let at = t.snap().decks[0].position / sr;
    t.click("grid_beat1", Some(1));
    let g = grid(&t);
    let one = g.beat_time(g.bar_zero);
    assert!(
        one > at && one - at < 0.5,
        "bar 1 at the playhead: {one} vs {at}"
    );
    assert_eq!(g.bar_number(g.bar_zero), 1);
    assert_eq!(g.bar_number(g.bar_zero + 4), 2);
    assert_eq!(g.bar_number(g.bar_zero - 4), -1);
    for _ in 0..3 {
        t.click("grid_slip_later", Some(1));
    }
    let s = grid(&t);
    let moved = s.beat_time(s.bar_zero) - one;
    assert!(
        (moved - 0.006).abs() < 1e-6,
        "bar 1 moved {moved} s with the grid"
    );

    // Back to the cues.
    t.click("tab_cues", Some(1));
    assert_eq!(t.demo.app.ui_map().find("cue_row", Some(1)).len(), 8);
}
