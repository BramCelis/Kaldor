//! Library inside the headless UI: background import, background analysis,
//! loading a library track onto a deck, search and crates.

use dj_audio::offline::OfflineRenderer;
use dj_audio::{Command, DeckCommand, Engine, EngineConfig, OutputInfo};
use dj_core::{AppConfig, DeckId};
use dj_library::Library;
use dj_ui::library::Selection;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn info() -> OutputInfo {
    OutputInfo {
        host: "offline".into(),
        device: "test".into(),
        sample_rate: 48_000,
        channels: 2,
        buffer_frames: Some(256),
        sample_format: "f32".into(),
    }
}

fn write_wav(path: &std::path::Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32_000.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn import_analyse_and_load_from_library() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-library");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let music = dir.join("music");
    std::fs::create_dir_all(&music).unwrap();
    for bpm in [122.0, 135.0] {
        let c = dj_analysis::synth::ClickTrack {
            noise: 0.03,
            ..dj_analysis::synth::ClickTrack::new(bpm, 25.0)
        };
        write_wav(&music.join(format!("clicks {bpm}.wav")), &c.render());
    }

    let (engine, handle) = Engine::new(EngineConfig::default());
    let mut renderer = OfflineRenderer::new(engine, 2, 256);
    let lib = Library::open(&dir.join("library.db")).unwrap();
    let mut app =
        dj_ui::KaldorApp::new(handle, info(), None, AppConfig::default()).with_library(lib);
    app.library().unwrap().start_import(music.clone());

    // Run frames until the import and both analyses are done.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        dj_ui::headless::run_frames(&mut app, 1, egui::vec2(1600.0, 950.0), |_, _| {
            renderer.render_block();
        });
        let l = app.library().unwrap();
        let ids = l.view().to_vec();
        let analysed = l.track_count() == 2
            && l.import_progress().is_none()
            && l.analysis_progress().1 == 0
            && ids
                .iter()
                .all(|id| l.track(*id).is_some_and(|t| t.analysis_version.is_some()));
        if analysed {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "import and analysis took too long"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let l = app.library().unwrap();
    l.search = "135".into();
    let hits = l.view().to_vec();
    assert_eq!(hits.len(), 1);
    let id = hits[0];
    assert!((l.track(id).unwrap().bpm.unwrap() - 135.0).abs() < 0.01);
    let crate_id = l.lib.create_crate("Warm-up", None).unwrap();
    l.lib.add_to_crate(crate_id, &[id]).unwrap();
    l.search.clear();
    l.selection = Selection::Crate(crate_id);
    l.mark_dirty();
    assert_eq!(l.view(), &[id]);

    // Load it on deck 2 from the library: grid and cached waveform come
    // from the database.
    let d2 = DeckId::ALL[1];
    app.load_library_track(d2, id);
    let deadline = Instant::now() + Duration::from_secs(30);
    while !app.snapshot().decks[1].load_complete {
        dj_ui::headless::run_frames(&mut app, 1, egui::vec2(1600.0, 950.0), |_, _| {
            renderer.render_block();
        });
        assert!(Instant::now() < deadline, "deck load took too long");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.has_waveform(d2));
    let grid = app.deck_grid(d2).expect("deck has the library beatgrid");
    assert!((grid.bpm - 135.0).abs() < 0.01);
    // Auto-gain was sent to the mixer.
    assert!(app.snapshot().mixer.channels[1].auto_gain_db.abs() > 0.1);

    app.send(Command::Deck(d2, DeckCommand::Play(true)));
    dj_ui::headless::run_frames(&mut app, 5, egui::vec2(1600.0, 950.0), |_, _| {
        renderer.render_block();
    });
    let l = app.library().unwrap();
    assert_eq!(
        l.lib.track(id).unwrap().unwrap().play_count,
        1,
        "play recorded once"
    );
}

/// Renaming a crate in place (rename did not work; it read the name
/// from an unrelated text box).
#[test]
fn rename_crate_in_place() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-rename");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let mut lib = Library::open(&dir.join("library.db")).unwrap();
    let id = lib.create_crate("Old name", None).unwrap();
    let (engine, handle) = Engine::new(EngineConfig::default());
    drop(engine);
    let mut app =
        dj_ui::KaldorApp::new(handle, info(), None, AppConfig::default()).with_library(lib);

    let l = app.library().unwrap();
    l.start_rename(id);
    assert_eq!(l.renaming.as_ref().unwrap().name, "Old name");
    // The edit field draws (and takes the focus) in the crate tree.
    dj_ui::headless::run_frames(&mut app, 3, egui::vec2(1280.0, 800.0), |_, _| {});
    let l = app.library().unwrap();
    assert!(l.renaming.is_some(), "still editing");

    // An empty name keeps the old one.
    l.renaming.as_mut().unwrap().name = "   ".into();
    l.commit_rename();
    assert_eq!(l.crates[0].name, "Old name");

    l.start_rename(id);
    l.renaming.as_mut().unwrap().name = "Bassline".into();
    l.commit_rename();
    assert!(l.renaming.is_none());
    assert_eq!(l.crates[0].name, "Bassline");
    assert_eq!(l.lib.crate_tree().unwrap()[0].name, "Bassline");
}

/// The USB export dialog end to end, into a folder standing in for the
/// stick: check (plan), export on the worker thread, done.
#[test]
fn usb_export_dialog_plans_and_writes_a_stick() {
    use dj_ui::usb_export::{Stage, UsbExport};
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-usb");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let stick = dir.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    let wav = dir.join("clicks.wav");
    let c = dj_analysis::synth::ClickTrack::new(124.0, 6.0);
    write_wav(&wav, &c.render());
    let mut lib = Library::open(&dir.join("library.db")).unwrap();
    let (t, _) = lib
        .upsert_track(&dj_library::TrackMeta {
            path: wav.clone(),
            title: "Clicks".into(),
            artist: "Kaldor Test".into(),
            duration: 6.0,
            file_size: std::fs::metadata(&wav).unwrap().len(),
            ..dj_library::TrackMeta::default()
        })
        .unwrap();
    lib.set_beatgrid(t, &dj_core::Beatgrid::new(124.0, 0.0))
        .unwrap();
    let crate_id = lib.create_crate("Set", None).unwrap();
    lib.add_to_crate(crate_id, &[t]).unwrap();
    let (engine, handle) = Engine::new(EngineConfig::default());
    drop(engine);
    let mut app =
        dj_ui::KaldorApp::new(handle, info(), None, AppConfig::default()).with_library(lib);
    let frame = |app: &mut dj_ui::KaldorApp| {
        dj_ui::headless::run_frames(app, 1, egui::vec2(1600.0, 950.0), |_, _| {});
    };
    let wait = |app: &mut dj_ui::KaldorApp, done: &dyn Fn(&Stage) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            frame(app);
            if done(&app.library().unwrap().usb.stage) {
                break;
            }
            assert!(Instant::now() < deadline, "the USB export took too long");
            std::thread::sleep(Duration::from_millis(10));
        }
    };

    let l = app.library().unwrap();
    l.usb.open_for(vec![crate_id]);
    l.usb.device = stick.display().to_string();
    frame(&mut app);
    UsbExport::start_plan(app.library().unwrap());
    wait(&mut app, &|s| !matches!(s, Stage::Planning(_)));
    match &app.library().unwrap().usb.stage {
        Stage::Planned(p) => {
            assert_eq!(p.tracks, 1);
            assert_eq!(p.playlists.len(), 1);
            assert!(!p.replaces_library);
        }
        Stage::Done(Err(e)) => panic!("plan failed: {e}"),
        _ => panic!("not planned"),
    }
    frame(&mut app);
    app.library().unwrap().usb.start_export();
    wait(&mut app, &|s| matches!(s, Stage::Done(_)));
    match &app.library().unwrap().usb.stage {
        Stage::Done(Ok(r)) => {
            assert_eq!(r.copied, 1);
            assert!(r.failures.is_empty(), "{:?}", r.failures);
        }
        Stage::Done(Err(e)) => panic!("export failed: {e}"),
        _ => unreachable!(),
    }
    frame(&mut app);
    assert!(stick.join("PIONEER/rekordbox/export.pdb").is_file());
    // The XML for export-verify sits next to the library.
    assert!(
        dir.join("usb-export")
            .join(dj_export::usb::PLAN_XML)
            .is_file()
    );
}
