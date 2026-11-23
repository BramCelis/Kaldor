//! The play history: tracks played on the demo's decks are logged in this
//! run's session; the History window lists it and makes a crate of it.

use dj_core::AppConfig;
use dj_ui::headless::Demo;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use std::time::{Duration, Instant};

const SIZE: egui::Vec2 = vec2(1920.0, 1080.0);

fn rects(demo: &Demo, name: &str) -> Vec<egui::Rect> {
    demo.app
        .ui_map()
        .find(name, None)
        .iter()
        .map(|c| c.rect)
        .collect()
}

fn click_with(demo: &mut Demo, p: Pos2, b: PointerButton) {
    let press = move |pressed| Event::PointerButton {
        pos: p,
        button: b,
        pressed,
        modifiers: Modifiers::default(),
    };
    demo.run_events(7, SIZE, 1.0, |i| match i {
        0..=1 => vec![Event::PointerMoved(p)],
        2 => vec![press(true)],
        3 => vec![press(false)],
        _ => vec![],
    });
}

#[test]
fn played_tracks_are_logged_and_shown_in_the_history_window() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    // Two library tracks loaded on decks 1 and 2 and played: each play is
    // logged once.
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-history");
    std::fs::create_dir_all(&dir).unwrap();
    let mut ids = Vec::new();
    for (k, bpm) in [124.0, 128.0].into_iter().enumerate() {
        let path = dir.join(format!("played{k}.wav"));
        let samples = dj_analysis::synth::ClickTrack::new(bpm, 8.0).render();
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
        ids.push(demo.app.library().unwrap().add_file(&path).unwrap());
    }
    for (k, id) in ids.iter().enumerate() {
        let deck = dj_core::DeckId::ALL[k];
        demo.app.load_library_track(deck, *id);
    }
    let t0 = Instant::now();
    loop {
        // Play until both decks play (a Play right after a load can be
        // lost on a busy machine: sent again each round).
        for k in 0..2 {
            if !demo.app.snapshot().decks[k].playing && demo.app.snapshot().decks[k].load_complete {
                demo.app.send(dj_audio::Command::Deck(
                    dj_core::DeckId::ALL[k],
                    dj_audio::DeckCommand::Play(true),
                ));
            }
        }
        demo.run(2, SIZE, 1.0);
        let plays = {
            let l = demo.app.library().unwrap();
            l.lib.sessions().unwrap().first().map_or(0, |s| s.plays)
        };
        if plays >= 2 {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(30), "no plays logged");
        std::thread::sleep(Duration::from_millis(10));
    }
    demo.run(10, SIZE, 1.0);
    let l = demo.app.library().unwrap();
    let sessions = l.lib.sessions().unwrap();
    assert_eq!(sessions.len(), 1, "one session per run");
    let plays = l.lib.session_plays(sessions[0].id).unwrap();
    let decks: Vec<u8> = plays.iter().map(|p| p.deck).collect();
    assert!(decks.iter().all(|d| (1..=4).contains(d)), "{decks:?}");
    let mut unique = decks.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        decks.len(),
        "each deck's track once: {decks:?}"
    );

    // The right-click menu has no History… any more: the button next to
    // Files opens the panel (and Files closes).
    let all = rects(&demo, "all_tracks")[0].center();
    click_with(&mut demo, all, PointerButton::Secondary);
    assert!(rects(&demo, "history_menu").is_empty());
    demo.app.open_files_at(dir.clone());
    demo.run(2, SIZE, 1.0);
    let button = rects(&demo, "history_button");
    assert_eq!(button.len(), 1, "History next to Files");
    click_with(&mut demo, button[0].center(), PointerButton::Primary);
    let h = demo.app.library().unwrap().history.clone().expect("open");
    assert_eq!(h.selected, Some(sessions[0].id));
    assert_eq!(h.plays.len(), plays.len());
    assert!(
        !demo.app.files_state().0,
        "Files closed: one panel at a time"
    );
    demo.run(2, SIZE, 1.0);
    assert_eq!(rects(&demo, "history_panel").len(), 1);
    assert!(rects(&demo, "files_panel").is_empty());
    assert_eq!(rects(&demo, "history_session").len(), 1);
    assert_eq!(rects(&demo, "history_play").len(), 1, "the tracks listed");
    // The panel sits above the library's toolbar, as Files does.
    let panel = rects(&demo, "history_panel")[0];
    assert!(panel.bottom() <= rects(&demo, "search")[0].top());

    // Make a crate: the session's tracks in the order played.
    let b = rects(&demo, "history_crate")[0].center();
    click_with(&mut demo, b, PointerButton::Primary);
    let l = demo.app.library().unwrap();
    let status = l.status.clone();
    let (c, name, _) = l
        .crate_list()
        .into_iter()
        .find(|(_, n, _)| n.starts_with("History "))
        .unwrap_or_else(|| panic!("a crate from the history; status {status:?}"));
    assert!(name.len() > "History ".len());
    let want: Vec<_> = plays.iter().filter_map(|p| p.track).collect();
    assert_eq!(l.lib.crate_tracks(c).unwrap(), want);

    let close = rects(&demo, "history_close")[0].center();
    click_with(&mut demo, close, PointerButton::Primary);
    assert!(demo.app.library().unwrap().history.is_none());

    // Open again with the button, then Files: History closes.
    let button = rects(&demo, "history_button")[0].center();
    click_with(&mut demo, button, PointerButton::Primary);
    assert!(demo.app.library().unwrap().history.is_some());
    let files = rects(&demo, "files_button")[0].center();
    click_with(&mut demo, files, PointerButton::Primary);
    assert!(demo.app.files_state().0);
    assert!(demo.app.library().unwrap().history.is_none());
}
