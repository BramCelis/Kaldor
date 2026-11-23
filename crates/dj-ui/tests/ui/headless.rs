//! Renders the UI headless (no window, no GPU) with the offline engine.

use dj_audio::offline::OfflineRenderer;
use dj_audio::{Engine, EngineConfig, OutputInfo};
use dj_core::AppConfig;

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

#[test]
fn renders_frames_in_dark_and_light_theme() {
    for dark in [true, false] {
        let (engine, handle) = Engine::new(EngineConfig::default());
        let mut renderer = OfflineRenderer::new(engine, 2, 256);
        let mut cfg = AppConfig::default();
        cfg.ui.dark_theme = dark;
        let mut app = dj_ui::KaldorApp::new(handle, info(), None, cfg);
        let report =
            dj_ui::headless::run_frames(&mut app, 10, egui::vec2(1280.0, 800.0), |_, _| {
                renderer.render_block();
            });
        assert_eq!(report.frames, 10);
        assert!(report.vertices > 100);
    }
}

#[test]
fn renders_at_small_window_size() {
    let (engine, handle) = Engine::new(EngineConfig::default());
    drop(engine);
    let mut app = dj_ui::KaldorApp::new(handle, info(), None, AppConfig::default());
    let report = dj_ui::headless::run_frames(&mut app, 3, egui::vec2(960.0, 600.0), |_, _| {});
    assert_eq!(report.frames, 3);
}

#[test]
fn default_keymap_keys_are_valid_egui_key_names() {
    for b in dj_core::config::default_keymap() {
        assert!(
            egui::Key::from_name(&b.key).is_some(),
            "unknown key name {:?}",
            b.key
        );
    }
}

#[test]
fn all_layouts_render() {
    use dj_core::config::DeckLayout;
    for layout in [
        DeckLayout::TwoDecks,
        DeckLayout::FourStacked,
        DeckLayout::FourGrid,
        DeckLayout::Extended,
    ] {
        let (engine, handle) = Engine::new(EngineConfig::default());
        let mut renderer = OfflineRenderer::new(engine, 2, 256);
        let mut app = dj_ui::KaldorApp::new(handle, info(), None, AppConfig::default());
        for d in dj_core::DeckId::ALL {
            let t = dj_analysis::synth::ClickTrack {
                sample_rate: 48_000,
                ..dj_analysis::synth::ClickTrack::new(120.0, 4.0)
            };
            app.load_audio(d, "t", t.to_track());
        }
        app.set_layout(layout);
        let report = dj_ui::headless::run_frames(&mut app, 5, egui::vec2(1600.0, 950.0), |_, _| {
            renderer.render_block();
        });
        assert_eq!(report.frames, 5, "{layout:?}");
        assert!(report.vertices > 1000, "{layout:?}");
    }
}
