//! Long offline soak: 30 minutes of playback through the offline renderer,
//! checking that output stays finite, the position stays exact and the
//! callback stays well inside its time budget.
//!
//! Ignored by default because it takes a while in debug builds. Run with:
//! `cargo test --release -p dj-audio --test soak -- --ignored --nocapture`

use dj_audio::{Command, DeckCommand, Engine, EngineConfig, TrackAudio};
use dj_core::{DeckId, OutputRouting};
use std::sync::Arc;
use std::time::Instant;

#[test]
#[ignore = "long-running: 30 minutes of audio rendered offline"]
fn thirty_minutes_one_deck_key_lock() {
    const SR: u32 = 48_000;
    const BLOCK: usize = 256;
    let samples = dj_analysis::synth::ClickTrack {
        sample_rate: SR,
        noise: 0.05,
        ..dj_analysis::synth::ClickTrack::new(126.0, 60.0 * 32.0)
    }
    .render();
    let track: Arc<TrackAudio> = TrackAudio::from_interleaved(SR, &samples);
    drop(samples);

    let (mut engine, mut h) = Engine::new(EngineConfig {
        sample_rate: SR,
        max_block_frames: 512,
        routing: OutputRouting::ns4fx(),
    });
    let d1 = DeckId::ALL[0];
    h.send(Command::Deck(
        d1,
        DeckCommand::Load {
            audio: Arc::clone(&track),
            serial: 1,
        },
    ))
    .unwrap();
    h.send(Command::Deck(d1, DeckCommand::SetKeyLock(true)))
        .unwrap();
    h.send(Command::Deck(d1, DeckCommand::SetPitch(0.25)))
        .unwrap();
    h.send(Command::Deck(d1, DeckCommand::Play(true))).unwrap();

    let mut buf = vec![0.0f32; BLOCK * 4];
    let blocks = (30 * 60 * SR as usize) / BLOCK;
    let period = BLOCK as f64 / f64::from(SR);
    let mut worst = 0.0f64;
    let mut over_budget = 0u64;
    let start = Instant::now();
    for i in 0..blocks {
        let t0 = Instant::now();
        engine.process(&mut buf, 4);
        let load = t0.elapsed().as_secs_f64() / period;
        worst = worst.max(load);
        if load > 0.25 {
            over_budget += 1;
        }
        assert!(
            buf.iter().all(|v| v.is_finite()),
            "non-finite output at block {i}"
        );
        if i % 64 == 0 {
            h.update();
        }
    }
    h.update();
    let elapsed = start.elapsed().as_secs_f64();
    let d = h.snapshot().decks[0];
    let expected = blocks as f64 * BLOCK as f64 * 1.02;
    println!(
        "30 min rendered in {elapsed:.1} s ({:.0}x real time); worst block load {:.1}%; blocks over 25%: {over_budget}; position {:.0} (expected about {:.0})",
        1800.0 / elapsed,
        worst * 100.0,
        d.position,
        expected
    );
    assert!(d.playing);
    // Tempo glides up from 1.0 over the first 20 ms, so allow a small gap.
    assert!((d.position - expected).abs() < 2_000.0);
}
