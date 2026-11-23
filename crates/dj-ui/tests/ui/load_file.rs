//! Loads a generated WAV file through the real background loader while the
//! UI renders headless frames.

use dj_audio::offline::OfflineRenderer;
use dj_audio::{Command, DeckCommand, Engine, EngineConfig, OutputInfo};
use dj_core::{AppConfig, DeckId};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[test]
fn load_wav_from_disk_and_play() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("clicks.wav");
    let samples = dj_analysis::synth::ClickTrack::new(128.0, 8.0).render();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&path, spec).unwrap();
    for s in samples {
        w.write_sample((s * 32_000.0) as i16).unwrap();
    }
    w.finalize().unwrap();

    let (engine, handle) = Engine::new(EngineConfig::default());
    let mut renderer = OfflineRenderer::new(engine, 2, 256);
    let info = OutputInfo {
        host: "offline".into(),
        device: "test".into(),
        sample_rate: 48_000,
        channels: 2,
        buffer_frames: Some(256),
        sample_format: "f32".into(),
    };
    let mut app = dj_ui::KaldorApp::new(handle, info, None, AppConfig::default());
    let d1 = DeckId::ALL[0];
    app.load_path(d1, path);

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut loaded = false;
    while Instant::now() < deadline {
        dj_ui::headless::run_frames(&mut app, 1, egui::vec2(1280.0, 800.0), |_, _| {
            for _ in 0..4 {
                renderer.render_block();
            }
        });
        let d = app.snapshot().decks[0];
        if d.loaded && d.load_complete && app.has_waveform(d1) {
            loaded = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(loaded, "track and waveform loaded within 30 s");
    let d = app.snapshot().decks[0];
    // 8 s at 44.1 kHz resampled to 48 kHz.
    assert!((d.length - 384_000.0).abs() <= 1.0, "length {}", d.length);

    app.send(Command::Deck(d1, DeckCommand::Play(true)));
    dj_ui::headless::run_frames(&mut app, 10, egui::vec2(1280.0, 800.0), |_, _| {
        for _ in 0..4 {
            renderer.render_block();
        }
    });
    assert!(app.snapshot().decks[0].playing);
    assert!(app.snapshot().decks[0].position > 0.0);
}

#[test]
fn missing_file_shows_an_error_and_keeps_running() {
    let (engine, handle) = Engine::new(EngineConfig::default());
    drop(engine);
    let info = OutputInfo {
        host: "offline".into(),
        device: "test".into(),
        sample_rate: 48_000,
        channels: 2,
        buffer_frames: None,
        sample_format: "f32".into(),
    };
    let mut app = dj_ui::KaldorApp::new(handle, info, None, AppConfig::default());
    app.load_path(DeckId::ALL[0], PathBuf::from("Z:/definitely/missing.mp3"));
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && app.deck_status(DeckId::ALL[0]).is_none() {
        dj_ui::headless::run_frames(&mut app, 1, egui::vec2(800.0, 600.0), |_, _| {});
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(app.deck_status(DeckId::ALL[0]).is_some());
}
