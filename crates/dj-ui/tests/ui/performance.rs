//! Performance features in the headless UI: the PADS panel, SYNC and SLIP on the deck
//! panels, Quantize, the SAMPLER tab, the keyboard and saving cues, loops
//! and grids in the library. Synthetic pointer and key events, no window,
//! no audio device.

use dj_audio::{Command, DeckCommand};
use dj_core::config::DeckLayout;
use dj_core::{AppConfig, DeckId};
use dj_ui::headless::Demo;
use dj_ui::perform::PadMode;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};

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

fn key_event(key: Key, physical: Option<Key>, pressed: bool, shift: bool) -> Event {
    Event::Key {
        key,
        physical_key: physical,
        pressed,
        repeat: false,
        modifiers: if shift {
            Modifiers::SHIFT
        } else {
            Modifiers::default()
        },
    }
}

impl T {
    fn new(layout: DeckLayout, pads: bool) -> Self {
        let mut cfg = AppConfig::default();
        cfg.ui.layout = layout;
        let mut demo = Demo::new(cfg);
        demo.app.show_pads(pads);
        let mut t = Self {
            demo,
            size: vec2(1920.0, 1080.0),
        };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        self.demo.run(n, self.size, 1.0);
    }

    fn input(&mut self, seq: Vec<Vec<Event>>) {
        let n = seq.len() + 3;
        self.demo.run_events(n, self.size, 1.0, |i| {
            seq.get(i).cloned().unwrap_or_default()
        });
    }

    fn rects(&self, name: &str, deck: Option<u8>) -> Vec<Rect> {
        self.demo
            .app
            .ui_map()
            .find(name, deck)
            .iter()
            .map(|p| p.rect)
            .collect()
    }

    fn rect(&self, name: &str, deck: Option<u8>) -> Rect {
        *self
            .rects(name, deck)
            .first()
            .unwrap_or_else(|| panic!("control {name} (deck {deck:?}) not on screen"))
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

    /// Clicks the `k`-th control called `name` of `deck`.
    fn click_nth(&mut self, name: &str, deck: Option<u8>, k: usize) {
        let r = self.rects(name, deck);
        assert!(r.len() > k, "{name} {deck:?}: only {} on screen", r.len());
        self.click_at(r[k].center());
    }

    fn key(&mut self, k: Key, shift: bool) {
        self.input(vec![
            vec![key_event(k, Some(k), true, shift)],
            vec![key_event(k, Some(k), false, shift)],
        ]);
    }

    fn snap(&self) -> dj_audio::EngineSnapshot {
        *self.demo.app.snapshot()
    }

    fn deck(&self, d: usize) -> dj_audio::DeckSnapshot {
        self.snap().decks[d]
    }
}

#[test]
#[ignore = "the PADS / FX / mixer panel is off screen in every layout; re-enable when it has a new place"]
fn pads_panel_cues_modes_loops_jump_key_and_grid() {
    let mut t = T::new(DeckLayout::FourStacked, true);
    // Every pads control of both LAYER decks is on screen.
    for d in [1u8, 2] {
        assert_eq!(t.rects("pad", Some(d)).len(), 8);
        assert_eq!(t.rects("pad_mode", Some(d)).len(), 8);
        for n in [
            "loop_in",
            "loop_out",
            "reloop",
            "loop_halve",
            "auto_loop",
            "loop_double",
            "loop_back",
            "loop_fwd",
            "jump_back",
            "jump_fwd",
            "jump_smaller",
            "jump_larger",
            "jump_size",
            "pads_slip",
            "key_down",
            "key_reset",
            "key_up",
            "grid_set1",
            "grid_earlier",
            "grid_later",
            "grid_halve",
            "grid_double",
            "grid_tap",
            "grid_lock",
            "sync_master",
            "cue_loop",
        ] {
            t.rect(n, Some(d));
        }
    }
    // Cue pad 3 of deck 1: sets a hot cue.
    t.click_nth("pad", Some(1), 2);
    assert!(t.deck(0).hot_cues[2].is_some(), "hot cue set");
    assert_eq!(t.demo.app.keyboard_deck(), 0);

    // AUTO tab, pad 4 = 1 beat.
    t.click_nth("pad_mode", Some(1), 2);
    assert_eq!(t.demo.app.pad_mode(0), PadMode::AutoLoop);
    t.click_nth("pad", Some(1), 3);
    let l = t.deck(0).loop_info.expect("loop");
    assert!(l.active && (l.beats - 1.0).abs() < 1e-4, "{l:?}");
    // ×2 doubles it, RELOOP (EXIT) leaves it.
    t.click("loop_double", Some(1));
    assert!((t.deck(0).loop_info.unwrap().beats - 2.0).abs() < 1e-4);
    t.click("reloop", Some(1));
    assert!(!t.deck(0).loop_info.unwrap().active);
    // Auto loop button: the size follows the last pad and ×2 (1 -> 2 beats).
    t.click("auto_loop", Some(1));
    let l = t.deck(0).loop_info.unwrap();
    assert!(l.active && (l.beats - 2.0).abs() < 1e-4, "{l:?}");
    t.click("loop_halve", Some(1));
    assert!((t.deck(0).loop_info.unwrap().beats - 1.0).abs() < 1e-4);
    t.click("reloop", Some(1));

    // Beat jump forward by 4 beats at 124 BPM (frames at 48 kHz).
    let beat = 60.0 * 48_000.0 / 124.0;
    let before = t.deck(0).position;
    t.click("jump_fwd", Some(1));
    let moved = t.deck(0).position - before;
    assert!(
        moved > 3.5 * beat && moved < 5.5 * beat,
        "jumped {}",
        moved / beat
    );
    t.click("jump_larger", Some(1));
    assert_eq!(t.demo.app.keyboard_deck(), 0);

    // Key shift up, then reset.
    t.click("key_up", Some(1));
    assert_eq!(t.deck(0).key_shift, 1);
    assert!(t.deck(0).key_lock);
    t.click("key_reset", Some(1));
    assert_eq!(t.deck(0).key_shift, 0);

    // Slip.
    t.click("pads_slip", Some(1));
    assert!(t.deck(0).slip);

    // Grid: ×2 then lock; a locked grid ignores ÷2.
    let bpm = t.demo.app.deck_grid(DeckId::ALL[0]).unwrap().bpm;
    t.click("grid_double", Some(1));
    assert!((t.demo.app.deck_grid(DeckId::ALL[0]).unwrap().bpm - 2.0 * bpm).abs() < 1e-9);
    assert!(
        t.deck(0)
            .grid
            .is_some_and(|g| (g.bpm(48_000) - 2.0 * bpm).abs() < 1e-6),
        "the engine got the new grid"
    );
    t.click("grid_lock", Some(1));
    t.click("grid_halve", Some(1));
    let g = t.demo.app.deck_grid(DeckId::ALL[0]).unwrap();
    assert!(g.locked && (g.bpm - 2.0 * bpm).abs() < 1e-9);

    // Deck 2 becomes the sync master.
    t.click("sync_master", Some(2));
    assert_eq!(t.snap().sync_master, Some(1));
}

#[test]
#[ignore = "the PADS / FX / mixer panel is off screen in every layout; re-enable when it has a new place"]
fn roll_slicer_and_pitch_play_pads() {
    let mut t = T::new(DeckLayout::TwoDecks, true);
    t.demo.app.set_pad_mode(0, PadMode::Roll);
    let pad = t.rects("pad", Some(1))[4].center();
    // Hold the 1/2-beat roll for a while, then release.
    t.demo.run_events(20, t.size, 1.0, |i| match i {
        0 => vec![moved(pad)],
        1 => vec![button(pad, true)],
        _ => vec![],
    });
    assert!(t.deck(0).rolling && t.deck(0).slip_position.is_some());
    t.input(vec![vec![button(pad, false)]]);
    assert!(!t.deck(0).rolling && t.deck(0).slip_position.is_none());

    t.demo.app.set_pad_mode(0, PadMode::Slicer);
    t.demo.run_events(10, t.size, 1.0, |i| match i {
        0 => vec![moved(pad)],
        1 => vec![button(pad, true)],
        _ => vec![],
    });
    assert_eq!(t.deck(0).slicer_held, Some(4));
    t.input(vec![vec![button(pad, false)]]);
    assert_eq!(t.deck(0).slicer_held, None);

    // Pitch play from hot cue 1: pad 7 = +3 semitones.
    t.demo.app.send(Command::Deck(
        DeckId::ALL[0],
        DeckCommand::SetHotCue {
            slot: 0,
            position: Some(48_000.0),
        },
    ));
    t.demo.app.set_pad_mode(0, PadMode::PitchPlay);
    t.frames(3);
    t.click_nth("pad", Some(1), 6);
    assert_eq!(t.deck(0).key_shift, 3);
    assert!(t.deck(0).key_lock);
}

#[test]
fn deck_panel_sync_slip_and_top_bar_switches() {
    let mut t = T::new(DeckLayout::FourStacked, false);
    t.click("sync", Some(2));
    let s = t.snap();
    assert!(s.decks[1].sync);
    // Deck 1 (playing, with a grid) is picked as the master.
    assert_eq!(s.sync_master, Some(0));
    t.frames(30);
    let s = t.snap();
    assert!(
        (s.decks[1].tempo * 128.0 - 124.0).abs() < 0.2,
        "deck 2 runs at {}",
        s.decks[1].tempo * 128.0
    );
    t.click("slip", Some(3));
    assert!(t.deck(2).slip);
    // Quantize and smart sync in Setup, saved in the config.
    assert!(!t.snap().quantize);
    t.click("settings", None);
    t.click("setup_tab_decks", None);
    t.click("quantize", None);
    assert!(t.snap().quantize && t.demo.app.config().performance.quantize);
    t.click("smart_sync", None);
    assert_eq!(t.snap().sync_mode, dj_audio::SyncMode::Tempo);
    assert!(!t.demo.app.config().performance.smart_sync);
    t.click("close_setup", None);
    // No MIXER / PADS / FX tabs any more.
    assert!(t.demo.app.ui_map().find("tab_pads", None).is_empty());
}

#[test]
fn sampler_tab_play_mode_record_and_sets() {
    let mut t = T::new(DeckLayout::FourStacked, false);
    t.demo.add_performance_state();
    // The SAMPLER tab is gone; the panel opens through the API until it
    // has a new place.
    t.demo.app.show_sampler(true);
    t.frames(3);
    for n in [
        "sample_pad",
        "sample_mode",
        "sample_sync",
        "sample_rec",
        "sample_eject",
        "sample_volume",
    ] {
        assert_eq!(t.rects(n, None).len(), 8, "{n}");
    }
    assert!(t.snap().sampler.slots[0].loaded);
    // Mode: one-shot -> hold.
    t.click("sample_mode", Some(1));
    assert_eq!(t.snap().sampler.slots[0].mode, dj_audio::SampleMode::Hold);
    // Hold the pad: plays while held.
    let pad = t.rect("sample_pad", Some(1)).center();
    t.demo.run_events(8, t.size, 1.0, |i| match i {
        0 => vec![moved(pad)],
        1 => vec![button(pad, true)],
        _ => vec![],
    });
    assert!(t.snap().sampler.slots[0].playing);
    t.input(vec![vec![button(pad, false)]]);
    t.frames(3);
    assert!(!t.snap().sampler.slots[0].playing);
    // SYNC, then eject slot 3.
    t.click("sample_sync", Some(1));
    assert!(t.snap().sampler.slots[0].sync);
    t.click("sample_eject", Some(3));
    assert!(!t.snap().sampler.slots[2].loaded);
    // Bank B.
    t.click_nth("sampler_bank", None, 1);
    assert_eq!(t.demo.app.sampler_bank(), 1);
    t.click_nth("sampler_bank", None, 0);
    // Record the master into slot A5; it stops on its own (16 beats at
    // 124 BPM is under 8 s) or when REC is pressed again.
    t.click("sample_rec", Some(5));
    assert_eq!(t.snap().sampler.recording.map(|r| r.0), Some(4));
    t.frames(20);
    t.click("sample_rec", Some(5));
    t.frames(3);
    assert!(t.snap().sampler.recording.is_none());
    assert!(t.snap().sampler.slots[4].loaded, "the recording is in A5");
    assert!(t.demo.app.sample_name(4).starts_with("Recording"));

    // Save the set (the recording becomes a WAV), clear, load it back.
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-sampler");
    let _ = std::fs::remove_dir_all(&dir);
    let set = dir.join("set.toml");
    let n = t.demo.app.save_sample_set(&set).expect("save");
    assert!(n >= 2, "{n}");
    assert!(dir.join("set-A5.wav").exists());
    t.demo.app.eject_sample(4);
    t.frames(3);
    assert!(!t.snap().sampler.slots[4].loaded);
    t.demo.app.load_sample_set(&set).expect("load");
    for _ in 0..200 {
        t.frames(1);
        if t.snap().sampler.slots[4].loaded {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(t.snap().sampler.slots[4].loaded, "A5 back from the WAV");
}

#[test]
fn keyboard_reaches_the_performance_features() {
    let mut t = T::new(DeckLayout::FourStacked, false);
    // F2: keyboard deck 2; 1: pad 1 (sets hot cue 1 on deck 2).
    t.key(Key::F2, false);
    assert_eq!(t.demo.app.keyboard_deck(), 1);
    t.key(Key::Num1, false);
    assert!(t.deck(1).hot_cues[0].is_some());
    // Shift+1 deletes it; US layouts send "!" with the physical key 1.
    t.input(vec![
        vec![key_event(Key::Exclamationmark, Some(Key::Num1), true, true)],
        vec![key_event(
            Key::Exclamationmark,
            Some(Key::Num1),
            false,
            true,
        )],
    ]);
    assert!(t.deck(1).hot_cues[0].is_none());
    // Slash: sync; Backtick: slip; Shift+Backtick: quantize.
    t.key(Key::Slash, false);
    assert!(t.deck(1).sync);
    t.key(Key::Backtick, false);
    assert!(t.deck(1).slip);
    t.key(Key::Backtick, true);
    assert!(t.snap().quantize);
    // M: auto loop of 4 beats; ArrowDown halves it; N leaves it.
    t.key(Key::M, false);
    assert!((t.deck(1).loop_info.unwrap().beats - 4.0).abs() < 1e-4);
    t.key(Key::ArrowDown, false);
    assert!((t.deck(1).loop_info.unwrap().beats - 2.0).abs() < 1e-4);
    t.key(Key::N, false);
    assert!(!t.deck(1).loop_info.unwrap().active);
    // 0: key shift up; Shift+0: back to 0.
    t.key(Key::Num0, false);
    assert_eq!(t.deck(1).key_shift, 1);
    t.key(Key::Num0, true);
    assert_eq!(t.deck(1).key_shift, 0);
    // F7: pad mode 3 (auto loop).
    t.key(Key::F7, false);
    assert_eq!(t.demo.app.pad_mode(1), PadMode::AutoLoop);
    // Shift+ArrowUp: grid tempo doubled.
    let bpm = t.demo.app.deck_grid(DeckId::ALL[1]).unwrap().bpm;
    t.key(Key::ArrowUp, true);
    assert!((t.demo.app.deck_grid(DeckId::ALL[1]).unwrap().bpm - 2.0 * bpm).abs() < 1e-9);
    // The deck rows still work: W toggles deck 1.
    let playing = t.deck(0).playing;
    t.key(Key::W, false);
    assert_ne!(t.deck(0).playing, playing);
}

#[test]
fn cues_loops_and_grids_are_saved_in_the_library() {
    use dj_audio::offline::OfflineRenderer;
    use dj_audio::{Engine, EngineConfig, OutputInfo};

    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-cues");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let click = dj_analysis::synth::ClickTrack::new(120.0, 20.0);
    let wav = dir.join("clicks.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for s in click.render() {
        w.write_sample((s.clamp(-1.0, 1.0) * 32_000.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
    let mut lib = dj_library::Library::open(&dir.join("library.db")).unwrap();
    let (id, _) = lib
        .upsert_track(&dj_library::TrackMeta {
            path: wav.clone(),
            title: "Clicks".into(),
            duration: 20.0,
            ..Default::default()
        })
        .unwrap();
    let analysis = dj_analysis::analyze_audio(&click.to_track(), &Default::default());
    lib.store_analysis(id, &analysis).unwrap();

    let (engine, handle) = Engine::new(EngineConfig::default());
    let mut r = OfflineRenderer::new(engine, 2, 256);
    let info = OutputInfo {
        host: "offline".into(),
        device: "test".into(),
        sample_rate: 48_000,
        channels: 2,
        buffer_frames: Some(256),
        sample_format: "f32".into(),
    };
    let mut app = dj_ui::KaldorApp::new(handle, info, None, AppConfig::default()).with_library(lib);
    let mut run = |app: &mut dj_ui::KaldorApp, n: usize| {
        dj_ui::headless::run_frames(app, n, vec2(1600.0, 950.0), |_, _| {
            for _ in 0..4 {
                r.render_block();
            }
        });
    };
    let d1 = DeckId::ALL[0];
    app.load_library_track(d1, id);
    for _ in 0..400 {
        run(&mut app, 1);
        if app.snapshot().decks[0].load_complete {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(app.snapshot().decks[0].grid.is_some(), "grid sent on load");
    // Set hot cue 2 at 3 s, a saved loop in slot 4, nudge the grid.
    app.send(Command::Deck(d1, DeckCommand::Seek(3.0 * 48_000.0)));
    run(&mut app, 2);
    app.pad(0, 1, true);
    app.pad(0, 1, false);
    app.send(Command::Deck(
        d1,
        DeckCommand::SetLoop {
            start: 4.0 * 48_000.0,
            end: 6.0 * 48_000.0,
            jump: false,
        },
    ));
    run(&mut app, 3);
    app.set_pad_mode(0, PadMode::SavedLoop);
    app.pad(0, 3, true);
    app.pad(0, 3, false);
    app.grid_edit(0, dj_ui::perform::GridEdit::Nudge(0.004));
    run(&mut app, 3);

    let l = app.library().unwrap();
    let cues = l.lib.cues(id).unwrap();
    let hot = cues
        .iter()
        .find(|c| c.kind == dj_library::CueKind::Hot(1))
        .expect("hot cue 2 saved");
    assert!((hot.position - 3.0).abs() < 0.01, "{}", hot.position);
    let loops = l.lib.loops(id).unwrap();
    assert_eq!(loops.len(), 1);
    assert_eq!(loops[0].slot, 3);
    assert!((loops[0].start - 4.0).abs() < 1e-6 && (loops[0].end - 6.0).abs() < 1e-6);
    let g = l.lib.beatgrid(id).unwrap().unwrap();
    let a = analysis.beatgrid.unwrap();
    assert!(
        ((g.first_beat - a.first_beat).rem_euclid(g.period()) - 0.004).abs() < 1e-6,
        "nudged grid saved"
    );

    // Loading the track on deck 2 brings the cue back into the engine.
    app.load_library_track(DeckId::ALL[1], id);
    for _ in 0..400 {
        run(&mut app, 1);
        if app.snapshot().decks[1].hot_cues[1].is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let c = app.snapshot().decks[1].hot_cues[1].expect("hot cue 2 on deck 2");
    assert!((c / 48_000.0 - 3.0).abs() < 0.01);
}

fn right_button(p: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: p,
        button: PointerButton::Secondary,
        pressed,
        modifiers: Modifiers::default(),
    }
}

/// A right-click on a pad opens its menu without firing the pad, and
/// the menu stays open while the name is typed.
#[test]
#[ignore = "the PADS / FX / mixer panel is off screen in every layout; re-enable when it has a new place"]
fn right_click_names_a_cue_without_firing_the_pad() {
    let mut t = T::new(DeckLayout::TwoDecks, true);
    t.demo.keep_context();
    assert!(t.deck(0).playing, "the demo plays deck 1");
    t.click_nth("pad", Some(1), 2);
    let cue = t.deck(0).hot_cues[2].expect("hot cue set");
    t.frames(30);
    let away = t.deck(0).position;
    assert!(away > cue + 4_000.0);

    // Right-click the pad: no jump back to the cue, the menu opens with the
    // name field focused.
    let pad = t.rects("pad", Some(1))[2].center();
    t.input(vec![
        vec![moved(pad)],
        vec![right_button(pad, true)],
        vec![right_button(pad, false)],
    ]);
    assert!(
        t.deck(0).position > away,
        "the right-click fired the pad (position {} vs {away})",
        t.deck(0).position
    );
    let field = t.rect("marker_name", Some(1)).center();

    // Clicking into the field keeps the menu open; typing names the cue.
    t.click_at(field);
    t.rect("marker_name", Some(1));
    t.input(vec![vec![Event::Text("drop".into())]]);
    assert_eq!(t.demo.app.hot_cue_name(0, 2), "drop");
    // Enter closes it.
    t.key(Key::Enter, false);
    assert!(
        t.rects("marker_name", Some(1)).is_empty(),
        "menu still open"
    );
    assert_eq!(t.demo.app.hot_cue_name(0, 2), "drop");

    // A right-click on an empty pad sets nothing.
    let empty = t.rects("pad", Some(1))[4].center();
    t.input(vec![
        vec![moved(empty)],
        vec![right_button(empty, true)],
        vec![right_button(empty, false)],
    ]);
    assert!(t.deck(0).hot_cues[4].is_none());
}

/// Holding a deck's bend button bends that deck and no other, in
/// every layout and on both layers.
#[test]
fn bend_buttons_bend_their_own_deck() {
    for layout in [
        DeckLayout::TwoDecks,
        DeckLayout::FourStacked,
        DeckLayout::FourGrid,
    ] {
        let mut t = T::new(layout, false);
        for layer in [false, true] {
            if layer {
                t.demo.app.set_layer(0, 2);
                t.demo.app.set_layer(1, 3);
                t.frames(2);
            }
            for name in ["bend_up", "bend_down"] {
                for d in 1..=4u8 {
                    let Some(r) = t.rects(name, Some(d)).first().copied() else {
                        continue;
                    };
                    let p = r.center();
                    t.demo.run_events(6, t.size, 1.0, |i| match i {
                        0 => vec![moved(p)],
                        1 => vec![button(p, true)],
                        _ => vec![],
                    });
                    let bends: Vec<f64> = (0..4).map(|k| t.deck(k).bend).collect();
                    t.input(vec![vec![button(p, false)]]);
                    for (k, b) in bends.iter().enumerate() {
                        assert_eq!(
                            *b != 0.0,
                            k == usize::from(d - 1),
                            "{layout:?}: holding {name} of deck {d} gives bends {bends:?}"
                        );
                    }
                }
            }
        }
    }
}

/// Dragging a library track over a deck's waveform strip outlines
/// that deck (panel and strip) until the drop.
#[test]
fn dragged_track_outlines_the_target_deck() {
    let mut t = T::new(DeckLayout::FourStacked, false);
    let row = t.rect("track_row", None).center();
    let target = t.rect("wave", Some(3)).center();
    let mut seen = Vec::new();
    for to in [target, row] {
        let mut seq = vec![vec![moved(row)], vec![button(row, true)]];
        for k in 1..=6 {
            seq.push(vec![moved(row + (to - row) * (k as f32 / 6.0))]);
        }
        t.demo.run_events(seq.len() + 2, t.size, 1.0, |i| {
            seq.get(i).cloned().unwrap_or_default()
        });
        seen.push(t.demo.app.drop_highlight());
        t.input(vec![vec![button(to, false)]]);
    }
    assert_eq!(
        seen[0],
        Some(dj_ui::DropTarget::Deck(2)),
        "over deck 3's strip"
    );
    assert_eq!(seen[1], None, "over the library: no deck");
    assert_eq!(t.demo.app.drop_highlight(), None, "gone after the drop");
}

/// Changing one deck's tempo must not rescale the other decks' waveforms:
/// the window is fixed real time, set by the zoom only (as in Serato,
/// where only the pitched deck's waveform scales).
#[test]
fn tempo_change_does_not_rescale_other_waveforms() {
    let mut t = T::new(DeckLayout::FourStacked, false);
    let before = t.demo.app.waveform_window_seconds();
    t.demo
        .app
        .send(Command::Deck(DeckId::ALL[0], DeckCommand::SetPitch(1.0)));
    t.frames(20);
    assert!(t.deck(0).tempo > 1.05, "deck 1 sped up");
    assert_eq!(t.demo.app.waveform_window_seconds(), before);
    t.demo
        .app
        .send(Command::Deck(DeckId::ALL[0], DeckCommand::Play(false)));
    t.frames(5);
    assert_eq!(t.demo.app.waveform_window_seconds(), before);
}

/// The grid editor's SLIP and ADJ buttons, and Ctrl+drag (slip) and
/// Ctrl+Shift+drag (adjust) on the waveform.
#[test]
#[ignore = "the PADS / FX / mixer panel is off screen in every layout; re-enable when it has a new place"]
fn grid_slip_and_adjust_with_buttons_and_mouse() {
    let mut t = T::new(DeckLayout::TwoDecks, true);
    t.demo.keep_context();
    let deck = DeckId::ALL[0];
    // Pause deck 1 halfway into the track so the play position stays put.
    t.click("play", Some(1));
    t.demo
        .app
        .send(Command::Deck(deck, DeckCommand::Seek(15.0 * 48_000.0)));
    t.frames(3);
    let grid = |t: &T| t.demo.app.deck_grid(deck).unwrap();
    let beat_near = |g: dj_core::Beatgrid, s: f64| g.nearest_beat(s);
    let pos = t.deck(0).position / 48_000.0;

    // SET 1 at the play position: a beat falls there.
    t.click("grid_set1", Some(1));
    let g0 = grid(&t);
    assert!((beat_near(g0, pos) - pos).abs() < 1e-6);

    // SLIP ▶: the whole grid 2 ms later; ◀ back.
    t.click("grid_later", Some(1));
    let g1 = grid(&t);
    assert!((g1.bpm - g0.bpm).abs() < 1e-9);
    assert!((beat_near(g1, pos) - (pos + 0.002)).abs() < 1e-6);
    t.click("grid_earlier", Some(1));
    assert!((beat_near(grid(&t), pos) - pos).abs() < 1e-6);

    // ADJ ▶: the beat at the play position stays; the beat length grows by
    // 0.2 ms, so the beat 8 beats later moves 1.6 ms and the beat 8 beats
    // earlier 1.6 ms the other way.
    let beat = 60.0 / g0.bpm;
    t.click("grid_adjust_up", Some(1));
    let g2 = grid(&t);
    assert!(g2.bpm < g0.bpm, "a longer beat is a slower grid");
    assert!(
        (beat_near(g2, pos) - pos).abs() < 1e-6,
        "the beat at the play position stays"
    );
    assert!((g2.period() - (beat + 0.0002)).abs() < 1e-9);
    let later = pos + 8.0 * beat;
    let earlier = pos - 8.0 * beat;
    assert!((beat_near(g2, later) - (later + 0.0016)).abs() < 1e-6);
    assert!((beat_near(g2, earlier) - (earlier - 0.0016)).abs() < 1e-6);
    assert!(
        t.deck(0)
            .grid
            .is_some_and(|eg| (eg.bpm(48_000) - g2.bpm).abs() < 1e-6),
        "the engine got the adjusted grid"
    );
    t.click("grid_adjust_down", Some(1));
    assert!((grid(&t).period() - beat).abs() < 1e-9);

    // Holding SLIP ▶ repeats: one step at once, more after the delay
    // (0.4 s, then every 0.05 s; frames are 1/60 s).
    let p = t.rect("grid_later", Some(1)).center();
    let before = beat_near(grid(&t), pos);
    t.demo.run_events(60, t.size, 1.0, |i| match i {
        0 => vec![moved(p)],
        1 => vec![button(p, true)],
        _ => vec![],
    });
    t.input(vec![vec![button(p, false)]]);
    let steps = ((beat_near(grid(&t), pos) - before) / 0.002).round();
    assert!(
        (10.0..=14.0).contains(&steps),
        "held for 1 s: {steps} steps"
    );
    t.click("grid_set1", Some(1));

    // Ctrl+drag 30 px right: the grid slips later.
    let wave = t.rect("wave", Some(1));
    let from = wave.center() + vec2(150.0, 0.0);
    let to = from + vec2(30.0, 0.0);
    let drag = |t: &mut T, mods: Modifiers| {
        t.demo.hold_modifiers(mods);
        let mut seq = vec![vec![moved(from)], vec![button(from, true)]];
        for k in 1..=5 {
            seq.push(vec![moved(from + (to - from) * (k as f32 / 5.0))]);
        }
        seq.push(vec![button(to, false)]);
        t.input(seq);
        t.demo.hold_modifiers(Modifiers::default());
    };
    let before = grid(&t);
    let p_before = t.deck(0).position;
    drag(&mut t, Modifiers::COMMAND);
    let after = grid(&t);
    assert_eq!(t.deck(0).position, p_before, "Ctrl+drag does not scrub");
    assert!(
        (after.bpm - before.bpm).abs() < 1e-9,
        "slip keeps the tempo"
    );
    assert!(beat_near(after, pos + 0.01) > beat_near(before, pos + 0.01) - 1e-9);
    assert!(
        (beat_near(after, pos) - beat_near(before, pos)).abs() > 0.001,
        "the grid moved"
    );

    // Ctrl+Shift+drag to the right of the play position: adjust around the
    // play position (its beat stays, the grid gets slower).
    t.click("grid_set1", Some(1));
    let before = grid(&t);
    drag(&mut t, Modifiers::COMMAND | Modifiers::SHIFT);
    let after = grid(&t);
    assert!(
        after.bpm < before.bpm,
        "stretched: {} -> {}",
        before.bpm,
        after.bpm
    );
    assert!(
        (beat_near(after, pos) - pos).abs() < 1e-6,
        "the beat at the play position stays"
    );
}

/// A library track dropped on a pad in SAMPLE mode loads into that
/// slot; dragging a track onto the SAMPLER tab opens the tab.
#[test]
#[ignore = "the PADS / FX / mixer panel is off screen in every layout; re-enable when it has a new place"]
fn library_tracks_drop_onto_sampler_pads_and_the_sampler_tab() {
    // A real (short) audio file, added to the demo library the way a file
    // loaded from the file manager is (`add_file`).
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-sample-drop");
    std::fs::create_dir_all(&dir).unwrap();
    let wav = dir.join("Stab - One Shot.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for i in 0..24_000 {
        let v = ((i as f32 * 0.05).sin() * 8_000.0) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();

    let mut t = T::new(DeckLayout::FourStacked, true);
    t.demo.keep_context();
    let id = t
        .demo
        .app
        .library()
        .unwrap()
        .add_file(&wav)
        .expect("added to the library");
    assert_eq!(
        t.demo.app.library().unwrap().add_file(&wav),
        Some(id),
        "added once"
    );
    t.demo.app.set_pad_mode(0, PadMode::Sampler);
    t.frames(2);
    let k = t
        .demo
        .app
        .library()
        .unwrap()
        .view()
        .iter()
        .position(|x| *x == id)
        .expect("in the track list");
    let row = t.rects("track_row", None)[k].center();
    let drag_to = |t: &mut T, to: Pos2| {
        let mut seq = vec![vec![moved(row)], vec![button(row, true)]];
        for k in 1..=6 {
            seq.push(vec![moved(row + (to - row) * (k as f32 / 6.0))]);
        }
        let n = seq.len() + 3;
        t.demo
            .run_events(n, t.size, 1.0, |i| seq.get(i).cloned().unwrap_or_default());
    };
    // Pad 3 of deck 1 = slot A3.
    let pad = t.rects("pad", Some(1))[2].center();
    drag_to(&mut t, pad);
    assert_eq!(
        t.demo.app.drop_highlight(),
        Some(dj_ui::DropTarget::Sample(2))
    );
    t.input(vec![vec![button(pad, false)]]);
    t.frames(30);
    assert!(
        t.demo.app.sample_name(2).contains("One Shot"),
        "slot A3: \"{}\"",
        t.demo.app.sample_name(2)
    );
    assert!(t.snap().sampler.slots[2].loaded, "the sample loaded");

    // Onto the SAMPLER tab: the tab opens.
    let tab = t.rect("tab_sampler", None).center();
    drag_to(&mut t, tab);
    assert!(t.demo.app.sampler_shown(), "the SAMPLER tab opened");
    t.input(vec![vec![button(tab, false)]]);
}
