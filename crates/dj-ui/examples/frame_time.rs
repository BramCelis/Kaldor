//! Headless UI frame time with four playing decks (scrolling waveforms
//! with beatgrids) and the full mixer, in every layout. Opens no window and
//! no audio device; exits by itself.
//!
//! ```sh
//! cargo run --release -p dj-ui --example frame_time -- [frames] [width] [height]
//! ```
//!
//! Prints mean, 99th percentile and worst UI frame time (egui layout plus
//! tessellation on the CPU; GPU time cannot be measured headless).

use dj_audio::offline::OfflineRenderer;
use dj_audio::{Command, DeckCommand, Engine, EngineConfig, OutputInfo};
use dj_core::config::DeckLayout;
use dj_core::{AppConfig, DeckId};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let frames: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(600);
    let w: f32 = args.get(2).and_then(|a| a.parse().ok()).unwrap_or(1920.0);
    let h: f32 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(1080.0);
    // "pads": the PADS panel and SAMPLER tab, with hot cues, a loop and sync.
    let pads = args.get(4).is_some_and(|a| a == "pads");
    const SR: u32 = 48_000;
    const BLOCK: usize = 256;
    for layout in [
        DeckLayout::TwoDecks,
        DeckLayout::FourStacked,
        DeckLayout::FourGrid,
        DeckLayout::Extended,
    ] {
        let (engine, handle) = Engine::new(EngineConfig {
            sample_rate: SR,
            ..EngineConfig::default()
        });
        let mut renderer = OfflineRenderer::new(engine, 2, BLOCK);
        let info = OutputInfo {
            host: "offline".into(),
            device: "frame time".into(),
            sample_rate: SR,
            channels: 2,
            buffer_frames: Some(BLOCK as u32),
            sample_format: "f32".into(),
        };
        let mut app = dj_ui::KaldorApp::new(handle, info, None, AppConfig::default());
        for (i, d) in DeckId::ALL.into_iter().enumerate() {
            let click = dj_analysis::synth::ClickTrack {
                sample_rate: SR,
                noise: 0.05,
                seed: i as u64 + 1,
                ..dj_analysis::synth::ClickTrack::new(120.0 + 6.0 * i as f64, 60.0)
            };
            let audio = click.to_track();
            let analysis =
                dj_analysis::analyze_audio(&audio, &dj_analysis::AnalysisOptions::default());
            app.load_audio_analyzed(d, &format!("Deck {}", i + 1), audio, &analysis);
            app.send(Command::Deck(d, DeckCommand::Play(true)));
        }
        if pads {
            app.show_pads(true);
            app.show_sampler(true);
            for d in DeckId::ALL {
                for slot in 0..4u8 {
                    app.send(Command::Deck(
                        d,
                        DeckCommand::SetHotCue {
                            slot,
                            position: Some(f64::from(slot) * 5.0 * f64::from(SR)),
                        },
                    ));
                }
                app.send(Command::Deck(d, DeckCommand::AutoLoop(4.0)));
                app.send(Command::Sync(d, true));
            }
        }
        app.set_layout(layout);
        let blocks_per_frame = (SR as usize / 60).div_ceil(BLOCK);
        // One run (one egui context); the first 30 frames are warm-up (font
        // atlas, caches) and not counted.
        let r = dj_ui::headless::run_frames(&mut app, frames + 30, egui::vec2(w, h), |_, _| {
            for _ in 0..blocks_per_frame {
                renderer.render_block();
            }
        });
        let vertices = r.vertices;
        let mut times: Vec<f64> = r
            .frame_times
            .iter()
            .skip(30)
            .map(|t| t.as_secs_f64() * 1000.0)
            .collect();
        times.sort_by(f64::total_cmp);
        let mean = times.iter().sum::<f64>() / times.len().max(1) as f64;
        let p99 = times[(times.len() * 99 / 100).min(times.len() - 1)];
        let max = times.last().copied().unwrap_or(0.0);
        println!(
            "{layout:?} at {w}x{h}{}: mean {mean:.2} ms, p99 {p99:.2} ms, max {max:.2} ms, {vertices} vertices",
            if pads { " (pads, sampler)" } else { "" }
        );
    }
}
