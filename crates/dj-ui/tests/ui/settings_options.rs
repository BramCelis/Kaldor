//! The options added to the settings window on 2026-11-15, end to end on
//! the headless demo: instant doubles, lock playing deck, the pitch range
//! at start, the close question, a remapped key, the recording file name.

use dj_core::config::KeyAction;
use dj_core::{AppConfig, DeckId};
use dj_ui::headless::Demo;
use egui::{Event, Key, Modifiers, vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SIZE: egui::Vec2 = vec2(1600.0, 950.0);

fn write_clicks(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-settings");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let samples = dj_analysis::synth::ClickTrack::new(128.0, 20.0).render();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for s in samples {
        w.write_sample((s * 32_000.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    path
}

/// Runs the demo until `done` or 20 s.
fn run_until(demo: &mut Demo, mut done: impl FnMut(&Demo) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        demo.run(2, SIZE, 1.0);
        if done(demo) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

#[test]
fn instant_doubles_copy_position_tempo_and_play_state() {
    let mut cfg = AppConfig::default();
    cfg.performance.instant_doubles = true;
    let mut demo = Demo::new(cfg);
    demo.run(3, SIZE, 1.0);
    let path = write_clicks("doubles.wav");
    let (a, b) = (DeckId::ALL[0], DeckId::ALL[2]);
    demo.app.send(dj_audio::Command::Deck(
        a,
        dj_audio::DeckCommand::Play(false),
    ));
    demo.run(3, SIZE, 1.0);
    demo.app.load_path(a, path.clone());
    assert!(run_until(&mut demo, |d| {
        let s = d.app.snapshot().decks[0];
        s.loaded && s.load_complete && d.app.deck_title(a).contains("doubles")
    }));
    demo.app.send(dj_audio::Command::Deck(
        a,
        dj_audio::DeckCommand::SetPitch(0.5),
    ));
    // Play, sent again when it was lost: on a slow machine (full workspace
    // runs) a Play right after a load was sometimes undone (a known
    // issue). This test is about the double, so it makes sure deck
    // 1 really plays first.
    let mut playing = false;
    for attempt in 0..3 {
        demo.app.send(dj_audio::Command::Deck(
            a,
            dj_audio::DeckCommand::Play(true),
        ));
        if run_until(&mut demo, |d| d.app.snapshot().decks[0].position > 48_000.0) {
            playing = true;
            break;
        }
        eprintln!("deck 1 did not start (attempt {attempt}): Play sent again");
    }
    assert!(playing, "deck 1 never started playing");
    // The same track onto deck 3: it continues where deck 1 is.
    demo.app.load_path(b, path);
    let ok = run_until(&mut demo, |d| {
        let s = d.app.snapshot().decks[2];
        s.loaded && s.playing && d.app.deck_title(b).contains("doubles")
    });
    let (s1, s3) = (demo.app.snapshot().decks[0], demo.app.snapshot().decks[2]);
    assert!(
        ok,
        "deck 3: loaded {} playing {} title {:?} status {:?}; deck 1: loaded {} playing {} at {:.2} s",
        s3.loaded,
        s3.playing,
        demo.app.deck_title(b),
        demo.app.deck_status(b),
        s1.loaded,
        s1.playing,
        s1.position / 48_000.0
    );
    demo.run(4, SIZE, 1.0);
    let s = *demo.app.snapshot();
    let gap = (s.decks[2].position - s.decks[0].position).abs() / 48_000.0;
    assert!(gap < 0.3, "deck 3 is {gap:.2} s away from deck 1");
    assert!(
        (s.decks[2].pitch - s.decks[0].pitch).abs() < 1e-9,
        "same pitch"
    );
}

/// "Start at: Bar 1": a loaded track waits at its bar 1
/// marker, where BEAT 1 was set (here 4 s into the track), not at the
/// first beat of the file.
#[test]
fn start_at_bar_1_waits_at_the_bar_1_marker() {
    let mut cfg = AppConfig::default();
    cfg.performance.load_position = dj_core::config::LoadPosition::FirstBeat;
    let mut demo = Demo::new(cfg);
    demo.run(3, SIZE, 1.0);
    let deck = DeckId::ALL[0];
    demo.app.send(dj_audio::Command::Deck(
        deck,
        dj_audio::DeckCommand::Play(false),
    ));
    demo.run(3, SIZE, 1.0);
    let path = write_clicks("bar1.wav");
    let id = {
        let l = demo.app.library().unwrap();
        let id = l.add_file(&path).expect("a library track");
        // BEAT 1 at 4 s (the first kick after an intro), as the GRID tab
        // would set it; the grid's first beat stays near 0 s.
        let mut g = dj_core::Beatgrid::new(128.0, 0.0);
        g.set_beat_one(4.0);
        assert!(g.first_beat < 0.5);
        l.lib.set_beatgrid(id, &g).unwrap();
        l.reload();
        id
    };
    demo.app.load_library_track(deck, id);
    assert!(run_until(&mut demo, |d| {
        let s = d.app.snapshot().decks[0];
        s.loaded && s.load_complete
    }));
    demo.run(3, SIZE, 1.0);
    let s = demo.app.snapshot();
    let at = s.decks[0].position / f64::from(s.sample_rate);
    assert!((at - 4.0).abs() < 0.01, "waits at {at:.3} s, want 4.000 s");
    assert!(!s.decks[0].playing);
}

#[test]
fn a_playing_deck_refuses_a_new_track_when_locked() {
    let mut cfg = AppConfig::default();
    cfg.performance.lock_playing_deck = true;
    let mut demo = Demo::new(cfg);
    demo.run(3, SIZE, 1.0);
    assert!(demo.app.snapshot().decks[0].playing);
    let before = demo.app.deck_title(DeckId::ALL[0]).to_string();
    demo.app
        .load_path(DeckId::ALL[0], write_clicks("locked.wav"));
    demo.run(3, SIZE, 1.0);
    assert_eq!(demo.app.deck_title(DeckId::ALL[0]), before);
    assert!(
        demo.app.status_line().contains("locked"),
        "{}",
        demo.app.status_line()
    );
}

#[test]
fn pitch_range_and_deck_settings_reach_the_engine_at_start() {
    let mut cfg = AppConfig::default();
    cfg.performance.pitch_range_percent = 16;
    cfg.mixer.cue_pre_eq = true;
    cfg.mixer.limiter_ceiling_db = -1.0;
    cfg.mixer.crossfader_assign = vec!["a".into(), "b".into(), "thru".into(), "thru".into()];
    let mut demo = Demo::new(cfg);
    demo.run(4, SIZE, 1.0);
    let s = demo.app.snapshot();
    for d in s.decks {
        assert_eq!(d.pitch_range, dj_audio::PitchRange::Sixteen);
    }
    assert!(s.mixer.cue_pre_eq);
    assert_eq!(s.mixer.limiter_ceiling_db, -1.0);
    assert_eq!(s.mixer.channels[0].xfader, dj_audio::XfaderSide::A);
    assert_eq!(s.mixer.channels[1].xfader, dj_audio::XfaderSide::B);
}

#[test]
fn closing_while_playing_asks_first() {
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    demo.app.request_close_for_test();
    assert!(demo.app.close_asked());
    demo.run(3, SIZE, 1.0);
    assert!(!demo.app.ui_map().find("close_confirm", None).is_empty());
    // Turned off: no question.
    let mut cfg = AppConfig::default();
    cfg.ui.confirm_close = false;
    let mut demo = Demo::new(cfg);
    demo.run(3, SIZE, 1.0);
    demo.app.request_close_for_test();
    assert!(!demo.app.close_asked());
}

#[test]
fn a_remapped_key_plays_its_new_action() {
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    assert!(demo.app.snapshot().decks[0].playing);
    demo.app
        .assign_key_for_test(KeyAction::Play(1), "Insert", false);
    let press = |pressed| Event::Key {
        key: Key::Insert,
        physical_key: Some(Key::Insert),
        pressed,
        repeat: false,
        modifiers: Modifiers::default(),
    };
    demo.run_events(6, SIZE, 1.0, |i| match i {
        0 => vec![press(true)],
        1 => vec![press(false)],
        _ => vec![],
    });
    assert!(
        !demo.app.snapshot().decks[0].playing,
        "Insert paused deck 1"
    );
    // W no longer does it.
    assert!(
        !demo
            .app
            .config()
            .ui
            .keymap
            .iter()
            .any(|b| b.key == "W" && b.action == KeyAction::Play(1))
    );
}

#[test]
fn recording_names_fill_in_the_pattern() {
    let n = dj_ui::record_view_name("Set {n} {date}", 3);
    assert!(n.starts_with("Set 3 20"), "{n}");
    assert!(!n.contains('{'));
    // Characters Windows refuses become dashes; empty gives the default.
    assert_eq!(dj_ui::record_view_name("a/b:c", 1), "a-b-c");
    assert!(dj_ui::record_view_name("  ", 1).starts_with("Kaldor "));
}
