//! `--smoke-test`: builds the whole app with the offline audio engine,
//! renders UI frames headless, checks the results and exits. Opens no window,
//! no audio device and writes no files.

use anyhow::{Result, ensure};
use dj_analysis::synth::ClickTrack;
use dj_audio::offline::OfflineRenderer;
use dj_audio::{Command, DeckCommand, Engine, EngineConfig, EqBand, MixerCommand, OutputInfo};
use dj_core::config::DeckLayout;
use dj_core::{AppConfig, DeckId, OutputRouting};
use std::process::ExitCode;

const SR: u32 = 48_000;
const BLOCK: usize = 256;

pub fn run(frames: usize) -> Result<ExitCode> {
    let routing = OutputRouting::ns4fx();
    let (engine, handle) = Engine::new(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
        routing,
    });
    let mut renderer = OfflineRenderer::new(engine, 4, BLOCK);
    let info = OutputInfo {
        host: "offline".into(),
        device: "smoke test renderer".into(),
        sample_rate: SR,
        channels: 4,
        buffer_frames: Some(BLOCK as u32),
        sample_format: "f32".into(),
    };
    let mut app = dj_ui::KaldorApp::new(handle, info, None, AppConfig::default());

    // An in-memory library (nothing is written to disk) with four analysed
    // generated tracks in a crate. The same tracks play on the four decks,
    // with their beatgrids drawn on the waveforms.
    let mut lib = dj_library::Library::open_in_memory()?;
    let crate_id = lib.create_crate("Smoke test", None)?;
    let mut decks_audio = Vec::new();
    for (i, bpm) in [124.0, 128.0, 140.0, 174.0].into_iter().enumerate() {
        let click = ClickTrack {
            sample_rate: SR,
            noise: 0.02,
            seed: i as u64 + 1,
            ..ClickTrack::new(bpm, 30.0)
        };
        let audio = click.to_track();
        let analysis = dj_analysis::analyze_audio(&audio, &dj_analysis::AnalysisOptions::default());
        let detected = analysis.beatgrid.map(|g| g.bpm).unwrap_or(0.0);
        ensure!(
            (detected - bpm).abs() < 0.01,
            "analysis found {detected} BPM, expected {bpm}"
        );
        let title = format!("Clicks {bpm} BPM");
        let (id, _) = lib.upsert_track(&dj_library::TrackMeta {
            path: format!("smoke-test://deck{}.wav", i + 1).into(),
            artist: "Smoke Test".into(),
            title: title.clone(),
            duration: 30.0,
            ..Default::default()
        })?;
        lib.store_analysis(id, &analysis)?;
        lib.add_to_crate(crate_id, &[id])?;
        decks_audio.push((title, audio, analysis));
    }
    app = app.with_library(lib);
    if let Some(l) = app.library() {
        l.search = "smoke 128".into();
    }
    for (deck, (title, audio, analysis)) in DeckId::ALL.iter().zip(decks_audio) {
        app.load_audio_analyzed(*deck, &title, audio, &analysis);
        app.send(Command::Deck(*deck, DeckCommand::Play(true)));
    }
    app.send(Command::Deck(DeckId::ALL[1], DeckCommand::SetKeyLock(true)));
    app.send(Command::Deck(DeckId::ALL[1], DeckCommand::SetPitch(0.5)));
    app.send(Command::Mixer(MixerCommand::Eq(
        DeckId::ALL[2],
        EqBand::Low,
        0.2,
    )));
    app.send(Command::Mixer(MixerCommand::Filter(DeckId::ALL[3], 0.4)));
    app.send(Command::Mixer(MixerCommand::Pfl(DeckId::ALL[0], true)));

    // One UI frame at 60 fps is 800 frames of audio at 48 kHz.
    let blocks_per_frame = (SR as usize / 60).div_ceil(BLOCK);
    let layouts = [
        DeckLayout::Extended,
        DeckLayout::FourStacked,
        DeckLayout::FourGrid,
        DeckLayout::TwoDecks,
    ];
    let report =
        dj_ui::headless::run_frames(&mut app, frames, egui::vec2(1600.0, 950.0), |app, i| {
            app.set_layout(layouts[(i * layouts.len() / frames.max(1)).min(layouts.len() - 1)]);
            // Second half: the PADS panel and the SAMPLER tab.
            app.show_pads(i >= frames / 2);
            app.show_sampler(i >= frames / 2);
            for _ in 0..blocks_per_frame {
                renderer.render_block();
            }
        });
    println!(
        "smoke test: {} frames, last frame {} primitives / {} vertices, slowest frame {:.2} ms, total {:.0} ms",
        report.frames,
        report.primitives,
        report.vertices,
        report.slowest_frame.as_secs_f64() * 1000.0,
        report.total.as_secs_f64() * 1000.0
    );
    ensure!(
        report.frames == frames,
        "rendered {} of {frames} frames",
        report.frames
    );
    ensure!(report.vertices > 0, "UI produced no geometry");

    let snap = *app.snapshot();
    // The UI reads the snapshot at the start of each frame, so it lags the
    // renderer by at most one frame.
    let expected = (frames.saturating_sub(1) * blocks_per_frame * BLOCK) as f64;
    for (i, d) in snap.decks.iter().enumerate() {
        println!(
            "smoke test: deck {} at {:.2} s, tempo {:.3}, playing {}",
            i + 1,
            d.position / f64::from(SR),
            d.tempo,
            d.playing
        );
        ensure!(d.loaded, "deck {} has no track", i + 1);
        let want = expected * if i == 1 { d.tempo } else { 1.0 };
        ensure!(
            frames < 2
                || (d.position - want).abs() <= want * 0.01 + (blocks_per_frame * BLOCK) as f64,
            "deck {} position {} does not follow the audio clock (expected about {want})",
            i + 1,
            d.position
        );
    }
    println!(
        "smoke test: callback load {:.1}%, master peak {:.2}",
        snap.callback_load * 100.0,
        snap.master_meter.peak[0].max(snap.master_meter.peak[1])
    );
    println!("smoke test: OK");
    Ok(ExitCode::SUCCESS)
}
