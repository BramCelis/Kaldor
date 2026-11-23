//! One deck, through the offline renderer.

use dj_audio::offline::OfflineRenderer;
use dj_audio::{
    Command, DeckCommand, DeckSnapshot, Engine, EngineConfig, EngineHandle, PitchRange, TrackAudio,
};
use dj_core::{DeckId, OutputRouting};
use std::f64::consts::PI;
use std::sync::Arc;

const SR: u32 = 48_000;
const D1: DeckId = DeckId::ALL[0];

struct Rig {
    r: OfflineRenderer,
    h: EngineHandle,
}

impl Rig {
    fn new() -> Self {
        let (engine, h) = Engine::new(EngineConfig {
            sample_rate: SR,
            max_block_frames: 256,
            routing: OutputRouting::laptop(),
        });
        Self {
            r: OfflineRenderer::new(engine, 2, 256),
            h,
        }
    }

    fn cmd(&mut self, c: DeckCommand) {
        self.h.send(Command::Deck(D1, c)).unwrap();
    }

    /// Renders whole blocks covering `frames`; returns the left channel.
    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        while out.len() < frames {
            let block = self.r.render_block();
            out.extend(block.as_chunks::<2>().0.iter().map(|f| f[0]));
            self.h.update();
        }
        out
    }

    fn deck(&self) -> DeckSnapshot {
        self.h.snapshot().decks[0]
    }
}

fn tone_track(freqs: &[f64], seconds: f64) -> Arc<TrackAudio> {
    let n = (seconds * f64::from(SR)) as usize;
    let mut s = Vec::with_capacity(n * 2);
    for i in 0..n {
        let t = i as f64 / f64::from(SR);
        let v: f64 =
            freqs.iter().map(|f| (2.0 * PI * f * t).sin()).sum::<f64>() * 0.4 / freqs.len() as f64;
        s.push(v as f32);
        s.push(v as f32);
    }
    TrackAudio::from_interleaved(SR, &s)
}

fn freq(s: &[f32]) -> f64 {
    let c = s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    c as f64 * f64::from(SR) / s.len() as f64
}

fn max_step(s: &[f32]) -> f32 {
    s.windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0, f32::max)
}

fn load(rig: &mut Rig, track: &Arc<TrackAudio>) {
    rig.cmd(DeckCommand::Load {
        audio: Arc::clone(track),
        serial: 7,
    });
}

#[test]
fn plays_sample_exact_at_normal_speed() {
    let track = tone_track(&[440.0, 1234.0], 5.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::Play(true));
    let out = rig.render(48_000);
    // After the 4 ms fade-in the output is the track, sample for sample
    // (late by the master limiter's fixed latency).
    let lat = rig.r.engine().output_latency_frames();
    for (i, y) in out.iter().enumerate().skip(256 + lat) {
        let x = track.frame((i - lat) as u64).unwrap()[0];
        assert!((x - y).abs() < 1e-6, "frame {i}: {x} vs {y}");
    }
    let d = rig.deck();
    assert_eq!(d.track_serial, 7);
    assert!(d.playing);
    assert_eq!(d.position, out.len() as f64);
    assert_eq!(d.length, 240_000.0);
}

#[test]
fn pitch_fader_changes_speed_and_pitch() {
    let track = tone_track(&[1000.0], 10.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::SetPitch(1.0)); // +8 %
    rig.cmd(DeckCommand::Play(true));
    rig.render(24_000); // let the tempo glide settle
    let p0 = rig.deck().position;
    let out = rig.render(48_000);
    let p1 = rig.deck().position;
    let speed = (p1 - p0) / out.len() as f64;
    assert!((speed - 1.08).abs() < 1e-9, "speed {speed}");
    assert!((freq(&out) - 1080.0).abs() < 3.0, "freq {}", freq(&out));
}

#[test]
fn key_lock_keeps_pitch() {
    let track = tone_track(&[1000.0], 10.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::SetKeyLock(true));
    rig.cmd(DeckCommand::SetPitchRange(PitchRange::Sixteen));
    rig.cmd(DeckCommand::SetPitch(-0.5)); // -8 %
    rig.cmd(DeckCommand::Play(true));
    rig.render(24_000);
    let p0 = rig.deck().position;
    let out = rig.render(48_000);
    let p1 = rig.deck().position;
    let speed = (p1 - p0) / out.len() as f64;
    assert!((speed - 0.92).abs() < 1e-9, "speed {speed}");
    assert!((freq(&out) - 1000.0).abs() < 5.0, "freq {}", freq(&out));
    assert!(rig.deck().key_lock);
}

#[test]
fn cue_sets_previews_and_returns() {
    let track = tone_track(&[300.0], 10.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::Seek(48_000.0));
    rig.render(256);

    // Paused away from the cue: CUE sets the cue point here and previews.
    rig.cmd(DeckCommand::Cue(true));
    rig.render(12_000);
    let d = rig.deck();
    assert_eq!(d.cue_point, 48_000.0);
    assert!(d.previewing && d.playing);
    assert!(d.position > 55_000.0);

    // Release: back to the cue point, paused, exactly.
    rig.cmd(DeckCommand::Cue(false));
    rig.render(2_048);
    let d = rig.deck();
    assert!(!d.playing);
    assert_eq!(d.position, 48_000.0);

    // Playing: CUE returns to the cue point and pauses.
    rig.cmd(DeckCommand::Play(true));
    rig.render(24_000);
    rig.cmd(DeckCommand::Cue(true));
    rig.render(2_048);
    rig.cmd(DeckCommand::Cue(false));
    rig.render(512);
    let d = rig.deck();
    assert!(!d.playing);
    assert_eq!(d.position, 48_000.0);

    // Holding CUE and pressing PLAY keeps playing after CUE is released.
    rig.cmd(DeckCommand::Cue(true));
    rig.render(4_096);
    rig.cmd(DeckCommand::Play(true));
    rig.render(512);
    rig.cmd(DeckCommand::Cue(false));
    rig.render(4_096);
    let d = rig.deck();
    assert!(d.playing && !d.previewing);
    assert!(d.position > 48_000.0 + 8_000.0);
}

/// Regression: the PLAY button and the W key send `TogglePlay`, not
/// `Play(true)`. During a CUE preview the deck is already playing, so the
/// toggle must latch play instead of pausing.
#[test]
fn toggle_play_while_holding_cue_keeps_playing() {
    let track = tone_track(&[300.0], 10.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::Seek(48_000.0));
    rig.render(256);

    rig.cmd(DeckCommand::Cue(true));
    rig.render(4_096);
    rig.cmd(DeckCommand::TogglePlay);
    rig.render(512);
    let d = rig.deck();
    assert!(d.playing, "PLAY during the preview paused the deck");
    rig.cmd(DeckCommand::Cue(false));
    rig.render(4_096);
    let d = rig.deck();
    assert!(d.playing && !d.previewing);
    assert!(d.position > 48_000.0 + 8_000.0, "position {}", d.position);

    // Once latched, PLAY pauses as usual.
    rig.cmd(DeckCommand::TogglePlay);
    rig.render(2_048);
    assert!(!rig.deck().playing);
}

#[test]
fn seeking_and_stopping_do_not_click() {
    let track = tone_track(&[200.0], 10.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::Play(true));
    let mut out = rig.render(12_000);
    rig.cmd(DeckCommand::Seek(100_123.0));
    out.extend(rig.render(12_000));
    rig.cmd(DeckCommand::Play(false));
    out.extend(rig.render(4_096));
    // 200 Hz at amplitude 0.4 changes at most ~0.0105 per sample; the
    // fades must not add a step much larger than that.
    let step = max_step(&out);
    assert!(step < 0.02, "max step {step}");
    assert!(!rig.deck().playing);
    assert_eq!(out.last().copied(), Some(0.0));
}

#[test]
fn stops_at_end_of_track() {
    let track = tone_track(&[440.0], 0.5);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::Play(true));
    rig.render(48_000);
    let d = rig.deck();
    assert!(!d.playing);
    assert_eq!(d.position, 24_000.0);
    // Play at the end does nothing.
    rig.cmd(DeckCommand::Play(true));
    rig.render(256);
    assert!(!rig.deck().playing);
}

#[test]
fn replaced_tracks_are_freed_on_the_ui_thread() {
    let first = tone_track(&[440.0], 1.0);
    let second = tone_track(&[880.0], 1.0);
    let mut rig = Rig::new();
    load(&mut rig, &first);
    rig.render(256);
    assert_eq!(Arc::strong_count(&first), 2, "engine holds a reference");
    rig.cmd(DeckCommand::Load {
        audio: Arc::clone(&second),
        serial: 8,
    });
    rig.render(256); // engine swaps and retires; update() drops it
    assert_eq!(Arc::strong_count(&first), 1);
    assert_eq!(rig.deck().track_serial, 8);
    assert_eq!(rig.h.snapshot().leaked_tracks, 0);
    rig.cmd(DeckCommand::Eject);
    rig.render(256);
    assert_eq!(Arc::strong_count(&second), 1);
    assert!(!rig.deck().loaded);
}

#[test]
fn scrub_follows_the_target_then_stops() {
    let track = tone_track(&[440.0], 5.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::Scrub {
        active: true,
        target: 24_000.0,
    });
    let out = rig.render(24_000);
    let d = rig.deck();
    assert!(d.scrubbing);
    assert!((d.position - 24_000.0).abs() < 500.0, "pos {}", d.position);
    assert!(out.iter().any(|v| v.abs() > 0.1), "scrubbing is audible");
    rig.cmd(DeckCommand::Scrub {
        active: false,
        target: 0.0,
    });
    rig.render(2_048);
    let tail = rig.render(1_024);
    assert!(!rig.deck().playing);
    assert!(tail.iter().all(|v| *v == 0.0));
}

#[test]
fn pitch_ranges_and_bend() {
    let track = tone_track(&[440.0], 30.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::SetPitchRange(PitchRange::Fifty));
    rig.cmd(DeckCommand::SetPitch(1.0));
    rig.cmd(DeckCommand::PitchBend(0.04));
    rig.cmd(DeckCommand::Play(true));
    rig.render(48_000);
    assert!((rig.deck().tempo - 1.54).abs() < 1e-9);
    rig.cmd(DeckCommand::PitchBend(0.0));
    rig.cmd(DeckCommand::SetPitch(-1.0));
    rig.render(96_000);
    assert!((rig.deck().tempo - 0.5).abs() < 1e-9);
}

#[test]
fn key_lock_toggle_while_playing_is_click_free() {
    let track = tone_track(&[250.0], 10.0);
    let mut rig = Rig::new();
    load(&mut rig, &track);
    rig.cmd(DeckCommand::SetPitch(0.5));
    rig.cmd(DeckCommand::Play(true));
    let mut out = rig.render(12_000);
    rig.cmd(DeckCommand::SetKeyLock(true));
    out.extend(rig.render(12_000));
    rig.cmd(DeckCommand::SetKeyLock(false));
    out.extend(rig.render(12_000));
    let step = max_step(&out);
    assert!(step < 0.025, "max step {step}");
    assert!(!rig.deck().key_lock);
}
