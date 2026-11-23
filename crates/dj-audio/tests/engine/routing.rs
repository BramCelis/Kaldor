//! Test tone and output routing, through the offline renderer.

use dj_audio::offline::{OfflineRenderer, channel, rms};
use dj_audio::{Command, Engine, EngineConfig, TestTone, ToneTarget};
use dj_core::OutputRouting;

const SR: u32 = 48_000;

fn renderer(routing: OutputRouting, channels: usize) -> (OfflineRenderer, dj_audio::EngineHandle) {
    let (engine, handle) = Engine::new(EngineConfig {
        sample_rate: SR,
        max_block_frames: 256,
        routing,
    });
    (OfflineRenderer::new(engine, channels, 256), handle)
}

/// Counts upward zero crossings to estimate frequency.
fn frequency(signal: &[f32]) -> f32 {
    let crossings = signal
        .windows(2)
        .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
        .count();
    crossings as f32 * SR as f32 / signal.len() as f32
}

#[test]
fn master_tone_only_on_outputs_1_2() {
    let (mut r, mut h) = renderer(OutputRouting::ns4fx(), 4);
    h.send(Command::SetTestTone(Some(TestTone::new(
        ToneTarget::Master,
    ))))
    .unwrap();
    let _ = r.render(4800); // past the fade-in
    let out = r.render(48_000);
    let expected = dj_core::time::db_to_gain(-18.0) / 2f32.sqrt();
    for ch in 0..2 {
        let c = channel(&out, 4, ch);
        assert!((rms(&c) - expected).abs() < 0.01, "ch{ch} rms {}", rms(&c));
        assert!((frequency(&c) - 440.0).abs() < 2.0);
    }
    for ch in 2..4 {
        assert_eq!(rms(&channel(&out, 4, ch)), 0.0, "ch{ch} must be silent");
    }
    // The snapshot queue holds only the oldest unread snapshots once full;
    // drain it, render one more block, and read the fresh state.
    h.update();
    let _ = r.render_block();
    let snap = h.update();
    assert_eq!(snap.test_tone, Some(ToneTarget::Master));
    assert!(snap.frames_rendered >= 48_000);
}

#[test]
fn cue_tone_only_on_outputs_3_4() {
    let (mut r, mut h) = renderer(OutputRouting::ns4fx(), 4);
    h.send(Command::SetTestTone(Some(TestTone::new(ToneTarget::Cue))))
        .unwrap();
    let _ = r.render(4800);
    let out = r.render(24_000);
    for ch in 0..2 {
        assert_eq!(rms(&channel(&out, 4, ch)), 0.0, "ch{ch} must be silent");
    }
    for ch in 2..4 {
        assert!(rms(&channel(&out, 4, ch)) > 0.05);
    }
}

#[test]
fn cue_tone_is_silent_without_cue_output() {
    let (mut r, mut h) = renderer(OutputRouting::laptop(), 2);
    h.send(Command::SetTestTone(Some(TestTone::new(ToneTarget::Cue))))
        .unwrap();
    let out = r.render(9600);
    assert_eq!(rms(&out), 0.0);
}

#[test]
fn split_cue_puts_cue_left_and_master_right() {
    let (mut r, mut h) = renderer(OutputRouting::laptop_split_cue(), 2);
    h.send(Command::SetTestTone(Some(TestTone::new(ToneTarget::Cue))))
        .unwrap();
    let _ = r.render(4800);
    let out = r.render(9600);
    assert!(rms(&channel(&out, 2, 0)) > 0.05, "cue on left");
    assert_eq!(rms(&channel(&out, 2, 1)), 0.0, "master (silent) on right");
}

#[test]
fn tone_fades_in_and_out_without_clicks() {
    let (mut r, mut h) = renderer(OutputRouting::laptop(), 2);
    h.send(Command::SetTestTone(Some(TestTone::new(
        ToneTarget::Master,
    ))))
    .unwrap();
    let start = r.render(256);
    let l = channel(&start, 2, 0);
    assert!(l[0].abs() < 0.001, "starts from silence");
    let _ = r.render(4800);
    h.send(Command::SetTestTone(None)).unwrap();
    let fade = channel(&r.render(1024), 2, 0);
    let max_step = fade
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    // A 440 Hz sine at -18 dBFS changes at most ~0.0073 per sample.
    assert!(max_step < 0.01, "no discontinuity, max step {max_step}");
    let tail = channel(&r.render(1024), 2, 0);
    assert_eq!(rms(&tail), 0.0, "silent after the fade");
}

#[test]
fn odd_callback_sizes_and_channel_counts() {
    // A callback larger than the internal block size and a 6-channel device.
    let (engine, mut h) = Engine::new(EngineConfig {
        sample_rate: SR,
        max_block_frames: 64,
        routing: OutputRouting::ns4fx(),
    });
    let mut r = OfflineRenderer::new(engine, 6, 1000);
    h.send(Command::SetTestTone(Some(TestTone::new(
        ToneTarget::Master,
    ))))
    .unwrap();
    let out = r.render(10_000);
    assert!(rms(&channel(&out, 6, 0)) > 0.05);
    assert_eq!(rms(&channel(&out, 6, 4)), 0.0);
    assert_eq!(rms(&channel(&out, 6, 5)), 0.0);
}
