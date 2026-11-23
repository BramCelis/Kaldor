//! Performance features through the offline renderer: hot cues,
//! loops, rolls, slicer, slip, beat jump, quantize, sync, key shift and the
//! sampler.

use dj_audio::offline::OfflineRenderer;
use dj_audio::{
    Command, DeckCommand, DeckGrid, DeckSnapshot, Engine, EngineConfig, EngineHandle, MixerCommand,
    RecordBuffer, SampleMode, SamplerCommand, SyncMode, TrackAudio,
};
use dj_core::{DeckId, OutputRouting};
use std::f64::consts::PI;
use std::sync::Arc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;
/// 120 BPM at 48 kHz.
const BEAT: f64 = 24_000.0;

struct Rig {
    r: OfflineRenderer,
    h: EngineHandle,
}

impl Rig {
    fn new() -> Self {
        let (engine, h) = Engine::new(EngineConfig {
            sample_rate: SR,
            max_block_frames: BLOCK,
            routing: OutputRouting::laptop(),
        });
        Self {
            r: OfflineRenderer::new(engine, 2, BLOCK),
            h,
        }
    }

    fn send(&mut self, c: Command) {
        self.h.send(c).unwrap();
    }

    fn deck(&mut self, d: usize, c: DeckCommand) {
        self.send(Command::Deck(DeckId::ALL[d], c));
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

    /// Renders whole blocks covering `beats` beats at normal speed.
    fn beats(&mut self, beats: f64) -> Vec<f32> {
        self.render((beats * BEAT) as usize)
    }

    fn snap(&self, d: usize) -> DeckSnapshot {
        self.h.snapshot().decks[d]
    }

    /// Loads `track` on deck `d` with a 120 BPM grid starting at 0.
    fn load(&mut self, d: usize, track: &Arc<TrackAudio>, grid: Option<DeckGrid>) {
        self.deck(
            d,
            DeckCommand::Load {
                audio: Arc::clone(track),
                serial: 1,
            },
        );
        self.deck(d, DeckCommand::SetGrid(grid));
        self.render(BLOCK);
    }
}

fn grid(bpm: f64) -> DeckGrid {
    DeckGrid {
        first_beat: 0.0,
        beat_len: 60.0 * f64::from(SR) / bpm,
        first_beat_in_bar: 1,
    }
}

/// A sine whose period divides a 120 BPM beat (100 Hz: 480 frames), so
/// loops of whole beats are seamless.
fn sine_track(freq: f64, seconds: f64) -> Arc<TrackAudio> {
    let n = (seconds * f64::from(SR)) as usize;
    let mut s = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = (2.0 * PI * freq * i as f64 / f64::from(SR)).sin() as f32 * 0.4;
        s.push(v);
        s.push(v);
    }
    TrackAudio::from_interleaved(SR, &s)
}

/// Silence with a short click on every beat at `bpm`.
fn click_track(bpm: f64, seconds: f64) -> Arc<TrackAudio> {
    let n = (seconds * f64::from(SR)) as usize;
    let period = 60.0 * f64::from(SR) / bpm;
    let mut s = vec![0.0f32; n * 2];
    let mut k = 0.0;
    while (k * period) as usize + 8 < n {
        let i = (k * period) as usize;
        for j in 0..8 {
            s[(i + j) * 2] = 0.8;
            s[(i + j) * 2 + 1] = 0.8;
        }
        k += 1.0;
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

fn near(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn hot_cues_set_trigger_and_delete() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 20.0);
    rig.load(0, &t, Some(grid(120.0)));
    // Paused at 1.3 beats: pad 1 sets a hot cue there (no quantize).
    rig.deck(0, DeckCommand::Seek(1.3 * BEAT));
    rig.render(BLOCK);
    for (pressed, slot) in [(true, 0), (false, 0)] {
        rig.deck(
            0,
            DeckCommand::HotCue {
                slot,
                pressed,
                loop_beats: 0.0,
            },
        );
    }
    rig.render(BLOCK);
    assert_eq!(rig.snap(0).hot_cues[0], Some(1.3 * BEAT));
    assert!(!rig.snap(0).playing, "setting a cue does not play");
    // With quantize the next cue lands on the beat.
    rig.send(Command::Quantize(true));
    rig.deck(0, DeckCommand::Seek(5.2 * BEAT));
    rig.deck(
        0,
        DeckCommand::HotCue {
            slot: 1,
            pressed: true,
            loop_beats: 0.0,
        },
    );
    rig.render(BLOCK);
    assert_eq!(rig.snap(0).hot_cues[1], Some(5.0 * BEAT));
    // Triggering a set cue while paused jumps there and plays.
    rig.deck(
        0,
        DeckCommand::HotCue {
            slot: 0,
            pressed: true,
            loop_beats: 0.0,
        },
    );
    rig.render(BLOCK);
    let s = rig.snap(0);
    assert!(s.playing);
    assert!(
        near(s.position, 1.3 * BEAT + BLOCK as f64, 2.0),
        "{}",
        s.position
    );
    // Quantized trigger while playing keeps the beat phase.
    rig.beats(2.0);
    let before = rig.snap(0).position;
    let phase = (before / BEAT).fract();
    rig.deck(
        0,
        DeckCommand::HotCue {
            slot: 1,
            pressed: true,
            loop_beats: 0.0,
        },
    );
    rig.render(BLOCK * 20);
    let after = rig.snap(0).position;
    assert!(
        near(
            (after / BEAT).fract(),
            (phase + 20.0 * BLOCK as f64 / BEAT).fract(),
            1e-3
        ),
        "phase {phase} -> {}",
        (after / BEAT).fract()
    );
    assert!(after > 4.5 * BEAT && after < 6.0 * BEAT, "{after}");
    // Delete.
    rig.deck(
        0,
        DeckCommand::SetHotCue {
            slot: 1,
            position: None,
        },
    );
    rig.render(BLOCK);
    assert_eq!(rig.snap(0).hot_cues[1], None);
}

#[test]
fn auto_loop_is_seamless_and_stays_in_the_loop() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 30.0);
    rig.load(0, &t, Some(grid(120.0)));
    rig.send(Command::Quantize(true));
    rig.deck(0, DeckCommand::Seek(4.1 * BEAT));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK * 4);
    rig.deck(0, DeckCommand::AutoLoop(1.0));
    rig.render(BLOCK);
    let l = rig.snap(0).loop_info.expect("loop");
    assert!(l.active);
    assert_eq!((l.start, l.end, l.beats), (4.0 * BEAT, 5.0 * BEAT, 1.0));
    let out = rig.beats(6.0);
    let s = rig.snap(0);
    assert!(
        s.position >= l.start && s.position < l.end,
        "{}",
        s.position
    );
    // No click and no gap at the wraps.
    assert!(max_step(&out) < 0.02, "step {}", max_step(&out));
    // Pressing the same size again leaves the loop.
    rig.deck(0, DeckCommand::AutoLoop(1.0));
    rig.beats(2.0);
    assert!(!rig.snap(0).loop_info.unwrap().active);
    assert!(rig.snap(0).position > l.end);
}

#[test]
fn loop_seams_are_crossfaded() {
    // 137 Hz does not fit a half beat at 120 BPM (12 000 frames is 34.25
    // periods): without the seam crossfade the wrap would jump by up to
    // the full amplitude.
    let mut rig = Rig::new();
    let t = sine_track(137.0, 30.0);
    rig.load(0, &t, Some(grid(120.0)));
    rig.send(Command::Quantize(true));
    rig.deck(0, DeckCommand::Seek(4.1 * BEAT));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK * 4);
    rig.deck(0, DeckCommand::AutoLoop(0.5));
    rig.render(BLOCK);
    assert!(rig.snap(0).loop_info.expect("loop").active);
    let out = rig.beats(4.0);
    // The sine's own largest step is 0.4 * 2 pi * 137 / 48 000 = 0.0072.
    assert!(max_step(&out) < 0.012, "step {}", max_step(&out));
}

#[test]
fn key_locked_loops_wrap_without_dropouts() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 30.0);
    rig.load(0, &t, Some(grid(120.0)));
    rig.deck(0, DeckCommand::SetKeyLock(true));
    rig.deck(0, DeckCommand::SetPitch(0.5));
    rig.deck(0, DeckCommand::Seek(4.0 * BEAT));
    rig.deck(0, DeckCommand::Play(true));
    rig.beats(1.0);
    for beats in [0.25f32, 2.0] {
        rig.deck(0, DeckCommand::AutoLoop(beats));
        rig.beats(0.5);
        let out = rig.beats(4.0);
        let s = rig.snap(0);
        let l = s.loop_info.unwrap();
        assert!(
            l.active && s.position >= l.start && s.position < l.end,
            "{beats}: {l:?} at {}",
            s.position
        );
        // Every 10 ms window has sound: wraps do not drop out.
        let quiet = out
            .chunks(480)
            .filter(|w| w.iter().map(|v| v * v).sum::<f32>() / (w.len() as f32) < 0.01)
            .count();
        assert_eq!(quiet, 0, "{beats} beats: {quiet} quiet windows");
        if beats >= 1.0 {
            // Key lock keeps the pitch (a quarter-beat loop is shorter than
            // the stretcher's analysis block, so it is not measured).
            assert!((freq(&out) - 100.0).abs() < 2.0, "{}", freq(&out));
        }
        rig.deck(0, DeckCommand::LoopExit);
        rig.beats(1.0);
    }
}

#[test]
fn manual_loop_in_out_reloop_halve_double_move() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 60.0);
    rig.load(0, &t, Some(grid(120.0)));
    rig.deck(0, DeckCommand::Seek(2.0 * BEAT));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK);
    rig.deck(0, DeckCommand::LoopIn);
    rig.render(BLOCK);
    let a = rig.snap(0).loop_in.expect("in point");
    rig.beats(2.0);
    rig.deck(0, DeckCommand::LoopOut);
    rig.render(BLOCK);
    let s = rig.snap(0);
    let l = s.loop_info.unwrap();
    assert!(l.active && l.start == a && near(l.len(), 2.0 * BEAT, 2.0 * BLOCK as f64));
    rig.beats(5.0);
    let p = rig.snap(0).position;
    assert!(p >= l.start && p < l.end);
    // Halve and double.
    rig.deck(0, DeckCommand::LoopHalve);
    rig.render(BLOCK);
    assert!(near(
        rig.snap(0).loop_info.unwrap().len(),
        l.len() / 2.0,
        1e-6
    ));
    rig.deck(0, DeckCommand::LoopDouble);
    rig.deck(0, DeckCommand::LoopDouble);
    rig.render(BLOCK);
    assert!(near(
        rig.snap(0).loop_info.unwrap().len(),
        l.len() * 2.0,
        1e-6
    ));
    // Move forward by its length: the position moves along.
    let before = rig.snap(0);
    rig.deck(0, DeckCommand::LoopMove(1));
    rig.render(BLOCK * 8);
    let after = rig.snap(0);
    let la = after.loop_info.unwrap();
    assert!(near(la.start, before.loop_info.unwrap().end, 1e-6));
    assert!(after.position >= la.start && after.position < la.end);
    // Reloop: exit, then back to the loop start.
    rig.deck(0, DeckCommand::Reloop);
    rig.render(BLOCK);
    assert!(!rig.snap(0).loop_info.unwrap().active);
    rig.beats(1.0);
    rig.deck(0, DeckCommand::Reloop);
    rig.render(BLOCK * 10);
    let s = rig.snap(0);
    assert!(s.loop_info.unwrap().active);
    assert!(
        near(s.position, la.start + 10.0 * BLOCK as f64, 400.0),
        "{}",
        s.position
    );
}

#[test]
fn beat_jump_moves_whole_beats_and_takes_the_loop_along() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 60.0);
    rig.load(0, &t, Some(grid(120.0)));
    rig.deck(0, DeckCommand::Seek(8.0 * BEAT));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK * 10);
    let p0 = rig.snap(0).position;
    rig.deck(0, DeckCommand::BeatJump(4.0));
    rig.render(BLOCK * 10);
    assert!(near(
        rig.snap(0).position,
        p0 + 4.0 * BEAT + 10.0 * BLOCK as f64,
        1.0
    ));
    rig.deck(0, DeckCommand::BeatJump(-8.0));
    rig.render(BLOCK * 10);
    assert!(near(
        rig.snap(0).position,
        p0 - 4.0 * BEAT + 20.0 * BLOCK as f64,
        1.0
    ));
    // With a loop: the loop moves too.
    rig.deck(0, DeckCommand::AutoLoop(2.0));
    rig.render(BLOCK);
    let l = rig.snap(0).loop_info.unwrap();
    rig.deck(0, DeckCommand::BeatJump(16.0));
    rig.render(BLOCK);
    let l2 = rig.snap(0).loop_info.unwrap();
    assert!(near(l2.start, l.start + 16.0 * BEAT, 1e-6) && l2.active);
    // No grid: nothing happens.
    rig.deck(0, DeckCommand::SetGrid(None));
    rig.deck(0, DeckCommand::LoopExit);
    rig.render(BLOCK);
    let p = rig.snap(0).position;
    rig.deck(0, DeckCommand::BeatJump(4.0));
    rig.render(BLOCK);
    assert!(near(rig.snap(0).position, p + BLOCK as f64, 1.0));
}

/// Deck 1 does something momentary; deck 2 plays the same track untouched.
/// Afterwards both must be at the same place.
fn assert_returns_like_reference(mut action: impl FnMut(&mut Rig, bool), slip: bool) {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 60.0);
    for d in [0, 1] {
        rig.load(d, &t, Some(grid(120.0)));
        rig.deck(d, DeckCommand::Seek(16.0 * BEAT + 1234.0));
    }
    rig.deck(0, DeckCommand::SetSlip(slip));
    rig.render(BLOCK);
    for d in [0, 1] {
        rig.deck(d, DeckCommand::Play(true));
    }
    rig.beats(1.3);
    action(&mut rig, true);
    rig.beats(3.0);
    let during = rig.snap(0);
    assert!(during.slip_position.is_some(), "slip position shown");
    assert!(
        (during.position - rig.snap(1).position).abs() > 1000.0,
        "deck 1 is elsewhere while held"
    );
    action(&mut rig, false);
    rig.beats(1.0);
    let (a, b) = (rig.snap(0), rig.snap(1));
    assert!(
        near(a.position, b.position, 2.0),
        "returned to {} not {}",
        a.position,
        b.position
    );
    assert!(a.slip_position.is_none());
}

#[test]
fn loop_roll_returns_to_where_the_track_would_be() {
    assert_returns_like_reference(
        |rig, pressed| {
            rig.deck(
                0,
                DeckCommand::Roll {
                    beats: 0.25,
                    pressed,
                },
            )
        },
        false,
    );
}

#[test]
fn slip_mode_makes_loops_and_hot_cues_momentary() {
    // Loop: on with slip, off returns.
    assert_returns_like_reference(
        |rig, pressed| {
            if pressed {
                rig.deck(0, DeckCommand::AutoLoop(1.0));
            } else {
                rig.deck(0, DeckCommand::LoopExit);
            }
        },
        true,
    );
    // Hot cue held.
    assert_returns_like_reference(
        |rig, pressed| {
            if pressed {
                rig.deck(
                    0,
                    DeckCommand::SetHotCue {
                        slot: 2,
                        position: Some(2.0 * BEAT),
                    },
                );
            }
            rig.deck(
                0,
                DeckCommand::HotCue {
                    slot: 2,
                    pressed,
                    loop_beats: 0.0,
                },
            );
        },
        true,
    );
}

#[test]
fn slicer_plays_a_slice_and_returns() {
    assert_returns_like_reference(
        |rig, pressed| {
            rig.deck(
                0,
                DeckCommand::Slicer {
                    slice: 5,
                    pressed,
                    domain_beats: 8.0,
                },
            )
        },
        false,
    );
    // While held the deck loops slice 5 of the 8-beat domain.
    let mut rig = Rig::new();
    let t = sine_track(100.0, 60.0);
    rig.load(0, &t, Some(grid(120.0)));
    rig.deck(0, DeckCommand::Seek(17.5 * BEAT));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK);
    rig.deck(
        0,
        DeckCommand::Slicer {
            slice: 5,
            pressed: true,
            domain_beats: 8.0,
        },
    );
    rig.beats(3.0);
    let s = rig.snap(0);
    assert_eq!(s.slicer_held, Some(5));
    // Domain 16..24 beats, slice 5 = beats 21..22.
    assert!(
        s.position >= 21.0 * BEAT && s.position < 22.0 * BEAT,
        "{}",
        s.position / BEAT
    );
}

#[test]
fn sync_matches_tempo_and_phase() {
    let mut rig = Rig::new();
    let a = click_track(120.0, 60.0);
    let b = click_track(125.0, 60.0);
    rig.load(0, &a, Some(grid(120.0)));
    rig.load(1, &b, Some(grid(125.0)));
    rig.deck(0, DeckCommand::Play(true));
    rig.deck(1, DeckCommand::Seek(0.37 * 60.0 * f64::from(SR) / 125.0));
    rig.deck(1, DeckCommand::Play(true));
    rig.beats(2.0);
    // Deck 2 syncs to the playing deck 1 (which becomes the master).
    rig.send(Command::Sync(DeckId::ALL[1], true));
    rig.beats(4.0);
    let snap = *rig.h.snapshot();
    assert_eq!(snap.sync_master, Some(0));
    let (sa, sb) = (snap.decks[0], snap.decks[1]);
    assert!(sb.sync);
    assert!(near(sb.tempo, 120.0 / 125.0, 0.002), "tempo {}", sb.tempo);
    let pa = (sa.position / BEAT).fract();
    let pb = (sb.position / (60.0 * f64::from(SR) / 125.0)).fract();
    let err = ((pa - pb + 0.5).rem_euclid(1.0) - 0.5).abs();
    assert!(err < 0.01, "phase error {err} beats");
    // The master's pitch changes: the follower follows and stays in phase.
    rig.deck(0, DeckCommand::SetPitch(0.5)); // +4 %
    rig.beats(8.0);
    let snap = *rig.h.snapshot();
    let (sa, sb) = (snap.decks[0], snap.decks[1]);
    assert!(
        near(sa.tempo * 120.0, sb.tempo * 125.0, 0.3),
        "{} {}",
        sa.tempo,
        sb.tempo
    );
    let pa = (sa.position / BEAT).fract();
    let pb = (sb.position / (60.0 * f64::from(SR) / 125.0)).fract();
    let err = ((pa - pb + 0.5).rem_euclid(1.0) - 0.5).abs();
    assert!(err < 0.02, "phase error {err} beats after a tempo change");
    // Off: the tempo stays where it is.
    let t = sb.tempo;
    rig.send(Command::Sync(DeckId::ALL[1], false));
    rig.beats(1.0);
    assert!(near(rig.snap(1).tempo, t, 0.003) && !rig.snap(1).sync);
}

#[test]
fn simple_sync_matches_tempo_only_and_handles_double_time() {
    let mut rig = Rig::new();
    let a = click_track(140.0, 60.0);
    let b = click_track(70.5, 60.0);
    rig.load(0, &a, Some(grid(140.0)));
    rig.load(1, &b, Some(grid(70.5)));
    rig.send(Command::SetSyncMode(SyncMode::Tempo));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK);
    rig.send(Command::Sync(DeckId::ALL[1], true));
    rig.deck(1, DeckCommand::Play(true));
    rig.beats(2.0);
    // 70.5 follows 140 at half time: 70 BPM.
    assert!(
        near(rig.snap(1).tempo, 70.0 / 70.5, 0.002),
        "{}",
        rig.snap(1).tempo
    );
    // Explicit master change.
    rig.send(Command::SyncMaster(DeckId::ALL[1]));
    rig.render(BLOCK);
    assert_eq!(rig.h.snapshot().sync_master, Some(1));
    // A deck without a grid cannot sync.
    rig.deck(
        2,
        DeckCommand::Load {
            audio: Arc::clone(&a),
            serial: 3,
        },
    );
    rig.send(Command::Sync(DeckId::ALL[2], true));
    rig.render(BLOCK);
    assert!(!rig.snap(2).sync);
}

#[test]
fn key_shift_changes_pitch_with_key_lock() {
    let mut rig = Rig::new();
    let t = sine_track(440.0, 20.0);
    rig.load(0, &t, None);
    rig.deck(0, DeckCommand::KeyShift(12));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(SR as usize / 2);
    let out = rig.render(SR as usize);
    let s = rig.snap(0);
    assert!(s.key_lock && s.key_shift == 12);
    assert!((freq(&out) - 880.0).abs() < 9.0, "{}", freq(&out));
    rig.deck(0, DeckCommand::KeyShift(-5));
    rig.render(SR as usize / 2);
    let out = rig.render(SR as usize);
    let want = 440.0 * 2f64.powf(-5.0 / 12.0);
    assert!((freq(&out) - want).abs() < 4.0, "{} vs {want}", freq(&out));
}

#[test]
fn quantize_snaps_the_main_cue_and_loop_points() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 20.0);
    rig.load(0, &t, Some(grid(120.0)));
    rig.send(Command::Quantize(true));
    rig.deck(0, DeckCommand::Seek(3.2 * BEAT));
    rig.render(BLOCK);
    rig.deck(0, DeckCommand::Cue(true));
    rig.render(BLOCK);
    rig.deck(0, DeckCommand::Cue(false));
    rig.render(BLOCK * 4);
    assert_eq!(rig.snap(0).cue_point, 3.0 * BEAT);
    assert!(rig.h.snapshot().quantize && rig.snap(0).quantize);
    rig.deck(0, DeckCommand::LoopIn);
    rig.render(BLOCK);
    assert_eq!(rig.snap(0).loop_in, Some(3.0 * BEAT));
}

fn sampler(rig: &mut Rig, c: SamplerCommand) {
    rig.send(Command::Sampler(c));
}

#[test]
fn sampler_modes_volume_and_sync() {
    let mut rig = Rig::new();
    // 0.25 s of 1 kHz.
    let sample = sine_track(1000.0, 0.25);
    sampler(
        &mut rig,
        SamplerCommand::Load {
            slot: 3,
            audio: Arc::clone(&sample),
            bpm: Some(120.0),
        },
    );
    rig.render(BLOCK);
    let rms = |s: &[f32]| (s.iter().map(|v| v * v).sum::<f32>() / s.len().max(1) as f32).sqrt();
    // One-shot: plays to the end, then silence.
    sampler(
        &mut rig,
        SamplerCommand::Trigger {
            slot: 3,
            pressed: true,
        },
    );
    let a = rig.render(SR as usize / 8);
    sampler(
        &mut rig,
        SamplerCommand::Trigger {
            slot: 3,
            pressed: false,
        },
    );
    assert!(rms(&a) > 0.1, "rms {}", rms(&a));
    assert!((freq(&a) - 1000.0).abs() < 20.0);
    assert!(rig.h.snapshot().sampler.slots[3].playing);
    rig.render(SR as usize / 4);
    assert!(!rig.h.snapshot().sampler.slots[3].playing);
    assert!(rms(&rig.render(BLOCK * 4)) < 1e-4);
    // Hold: stops on release.
    sampler(&mut rig, SamplerCommand::SetMode(3, SampleMode::Hold));
    sampler(
        &mut rig,
        SamplerCommand::Trigger {
            slot: 3,
            pressed: true,
        },
    );
    rig.render(BLOCK * 8);
    sampler(
        &mut rig,
        SamplerCommand::Trigger {
            slot: 3,
            pressed: false,
        },
    );
    rig.render(BLOCK * 2);
    assert!(rms(&rig.render(BLOCK * 4)) < 1e-4);
    // Loop: keeps going past the end, until pressed again.
    sampler(&mut rig, SamplerCommand::SetMode(3, SampleMode::Loop));
    sampler(
        &mut rig,
        SamplerCommand::Trigger {
            slot: 3,
            pressed: true,
        },
    );
    sampler(
        &mut rig,
        SamplerCommand::Trigger {
            slot: 3,
            pressed: false,
        },
    );
    let l = rig.render(SR as usize);
    assert!(rms(&l[l.len() - 4800..]) > 0.1);
    // Volume.
    sampler(&mut rig, SamplerCommand::SetVolume(3, 0.25));
    rig.render(SR as usize / 10);
    let quiet = rig.render(SR as usize / 10);
    assert!(rms(&quiet) < rms(&l) * 0.4 && rms(&quiet) > rms(&l) * 0.1);
    sampler(
        &mut rig,
        SamplerCommand::Trigger {
            slot: 3,
            pressed: true,
        },
    );
    rig.render(BLOCK * 2);
    assert!(rms(&rig.render(BLOCK * 4)) < 1e-4);
    // Sync: with a 125 BPM deck playing, a 120 BPM sample plays 125/120 faster.
    let deck = click_track(125.0, 30.0);
    rig.load(0, &deck, Some(grid(125.0)));
    rig.send(Command::Mixer(MixerCommand::Fader(DeckId::ALL[0], 0.0)));
    rig.deck(0, DeckCommand::Play(true));
    sampler(&mut rig, SamplerCommand::SetSync(3, true));
    sampler(&mut rig, SamplerCommand::SetVolume(3, 1.0));
    rig.render(BLOCK * 8);
    sampler(
        &mut rig,
        SamplerCommand::Trigger {
            slot: 3,
            pressed: true,
        },
    );
    rig.render(BLOCK * 8);
    let f = freq(&rig.render(SR as usize / 2));
    assert!((f - 1000.0 * 125.0 / 120.0).abs() < 20.0, "{f}");
    // Eject.
    sampler(&mut rig, SamplerCommand::Eject(3));
    rig.render(BLOCK);
    assert!(!rig.h.snapshot().sampler.slots[3].loaded);
}

#[test]
fn sampler_records_the_master_output() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 10.0);
    rig.load(0, &t, Some(grid(120.0)));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK * 4);
    let frames = SR as usize / 2;
    sampler(
        &mut rig,
        SamplerCommand::Record(RecordBuffer::new(5, frames)),
    );
    rig.render(BLOCK);
    assert_eq!(rig.h.snapshot().sampler.recording.map(|r| r.0), Some(5));
    rig.render(frames + BLOCK);
    let recs = rig.h.take_recordings();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    assert_eq!((r.slot, r.written), (5, frames));
    let left: Vec<f32> = r.recorded().chunks(2).map(|f| f[0]).collect();
    assert!((freq(&left) - 100.0).abs() < 3.0);
    assert!(rig.h.snapshot().sampler.recording.is_none());
    // Stopping early hands back what was recorded so far.
    sampler(
        &mut rig,
        SamplerCommand::Record(RecordBuffer::new(6, frames)),
    );
    rig.render(BLOCK * 10);
    sampler(&mut rig, SamplerCommand::StopRecord);
    rig.render(BLOCK);
    let recs = rig.h.take_recordings();
    assert_eq!(recs.len(), 1);
    assert!(recs[0].written >= BLOCK * 9 && recs[0].written <= BLOCK * 11);
}

#[test]
fn load_with_markers_applies_grid_and_cues_with_the_track() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 10.0);
    let mut hot = [None; dj_audio::HOT_CUES];
    hot[2] = Some(2.5 * BEAT);
    rig.deck(
        0,
        DeckCommand::LoadWithMarkers {
            audio: Arc::clone(&t),
            serial: 9,
            markers: dj_audio::TrackMarkers {
                grid: Some(grid(120.0)),
                cue_point: Some(1.5 * BEAT),
                hot_cues: hot,
                start: None,
            },
        },
    );
    // The very first snapshot after the load already has everything.
    rig.render(BLOCK);
    let s = rig.snap(0);
    assert_eq!(s.track_serial, 9);
    assert_eq!(s.grid, Some(grid(120.0)));
    assert_eq!(s.cue_point, 1.5 * BEAT);
    assert_eq!(s.hot_cues[2], Some(2.5 * BEAT));
    assert_eq!(s.hot_cues[0], None);
}

/// Bending the sync master nudges only the master. The synced deck
/// keeps its tempo during the bend and keeps the new phase offset after it;
/// a scrub of the master does not change the follower's tempo either.
#[test]
fn bending_the_master_does_not_bend_the_synced_deck() {
    let mut rig = Rig::new();
    let a = click_track(120.0, 60.0);
    let b = click_track(125.0, 60.0);
    rig.load(0, &a, Some(grid(120.0)));
    rig.load(1, &b, Some(grid(125.0)));
    rig.deck(0, DeckCommand::Play(true));
    rig.deck(1, DeckCommand::Play(true));
    rig.beats(1.0);
    rig.send(Command::Sync(DeckId::ALL[1], true));
    rig.beats(4.0);
    assert_eq!(rig.h.snapshot().sync_master, Some(0));
    let follow = rig.snap(1).tempo;
    assert!(near(follow, 120.0 / 125.0, 0.002), "tempo {follow}");
    let phase = |rig: &Rig| {
        let s = rig.h.snapshot();
        let pa = (s.decks[0].position / BEAT).fract();
        let pb = (s.decks[1].position / (60.0 * f64::from(SR) / 125.0)).fract();
        (pa - pb + 0.5).rem_euclid(1.0) - 0.5
    };
    let before = phase(&rig);

    // Bend the master up for two beats.
    rig.deck(0, DeckCommand::PitchBend(0.04));
    rig.beats(2.0);
    assert!(rig.snap(0).tempo > 1.03, "master bends");
    // The waveform scale (pitch_tempo) ignores the bend.
    assert!(near(rig.snap(0).pitch_tempo, 1.0, 1e-9));
    assert!(near(rig.snap(1).pitch_tempo, 120.0 / 125.0, 1e-6));
    assert!(
        near(rig.snap(1).tempo, follow, 0.001),
        "the synced deck bent along: {} vs {follow}",
        rig.snap(1).tempo
    );
    rig.deck(0, DeckCommand::PitchBend(0.0));
    rig.beats(1.0);
    let moved = phase(&rig) - before;
    assert!(moved > 0.03, "the bend moved the master's phase by {moved}");
    // The new offset is kept, not corrected away.
    rig.beats(8.0);
    let kept = phase(&rig) - before;
    assert!(
        (kept - moved).abs() < 0.01,
        "offset {moved} after the bend, {kept} later"
    );
    assert!(near(rig.snap(1).tempo, follow, 0.001));

    // Scrubbing the master leaves the follower's tempo alone.
    let p = rig.snap(0).position;
    for k in 1..=10 {
        rig.deck(
            0,
            DeckCommand::Scrub {
                active: true,
                target: p + f64::from(k) * 4_000.0,
            },
        );
        rig.render(BLOCK);
        assert!(
            near(rig.snap(1).tempo, follow, 0.001),
            "scrub step {k}: follower tempo {}",
            rig.snap(1).tempo
        );
        assert!(
            near(rig.snap(0).tempo, 1.0, 1e-9),
            "the master's tempo is not the scrub speed"
        );
    }
}

#[test]
fn controller_pitch_fader_soft_takeover_like_serato() {
    let mut rig = Rig::new();
    let a = click_track(120.0, 60.0);
    let b = click_track(125.0, 60.0);
    rig.load(0, &a, Some(grid(120.0)));
    rig.load(1, &b, Some(grid(125.0)));
    let pitch = |rig: &Rig| rig.snap(1).pitch;
    // Fader position unknown at first: it takes the pitch over once it
    // moves across it (here from +0.3 to -0.1 across 0, in one jump).
    rig.deck(1, DeckCommand::PitchFader(0.3));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), 0.0, 1e-9));
    assert_eq!(rig.snap(1).pitch_fader_ghost, Some(0.3));
    rig.deck(1, DeckCommand::PitchFader(-0.1));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), -0.1, 1e-9));
    assert_eq!(rig.snap(1).pitch_fader_ghost, None);
    rig.deck(1, DeckCommand::PitchFader(0.5));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), 0.5, 1e-9), "follows 1:1");
    // Changed on screen: locked again until the fader crosses the pitch.
    rig.deck(1, DeckCommand::SetPitch(-0.5));
    rig.deck(1, DeckCommand::PitchFader(0.4));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), -0.5, 1e-9), "locked: {}", pitch(&rig));
    assert_eq!(rig.snap(1).pitch_fader_ghost, Some(0.4));
    rig.deck(1, DeckCommand::PitchFader(-0.7));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), -0.7, 1e-9), "crossed: {}", pitch(&rig));
    // The start-up dump: taken at once.
    rig.deck(1, DeckCommand::PitchFaderSync(0.2));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), 0.2, 1e-9));
    // LAYER: the fader now belongs to this deck, position unknown.
    rig.deck(1, DeckCommand::PitchTakeover(None));
    rig.deck(1, DeckCommand::PitchFader(0.9));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), 0.2, 1e-9), "waits after LAYER");
    rig.deck(1, DeckCommand::PitchFader(0.205));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), 0.205, 1e-9), "meets it: takes over");

    // A synced follower ignores its fader; SYNC off
    // locks the fader again until it crosses the synced pitch.
    rig.deck(0, DeckCommand::Play(true));
    rig.deck(1, DeckCommand::Play(true));
    rig.render(BLOCK);
    rig.send(Command::Sync(DeckId::ALL[1], true));
    rig.beats(1.0);
    rig.deck(1, DeckCommand::PitchFader(0.9));
    rig.beats(1.0);
    let s = rig.snap(1);
    assert!(near(s.tempo, 120.0 / 125.0, 0.002), "{}", s.tempo);
    assert!(near(rig.snap(0).tempo, 1.0, 1e-9));
    rig.send(Command::Sync(DeckId::ALL[1], false));
    rig.render(BLOCK);
    let synced = pitch(&rig);
    rig.deck(1, DeckCommand::PitchFader(0.8));
    rig.render(BLOCK);
    assert!(near(pitch(&rig), synced, 1e-9), "locked after SYNC off");
}

#[test]
fn key_lock_switch_while_playing_has_no_gap() {
    let mut rig = Rig::new();
    let t = sine_track(100.0, 20.0);
    rig.load(0, &t, None);
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK * 40);
    for on in [true, false, true] {
        rig.deck(0, DeckCommand::SetKeyLock(on));
        let out = rig.render(BLOCK * 20);
        // No 2.5 ms stretch of near silence (the old switch faded out and
        // back in).
        let quietest = out
            .chunks(120)
            .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
            .fold(f32::MAX, f32::min);
        assert!(
            quietest > 0.4 * 0.4 / 2f32.sqrt(),
            "key lock {on}: {quietest}"
        );
        assert_eq!(rig.snap(0).key_lock, on);
    }
}

#[test]
fn spamming_key_lock_at_zero_pitch_has_no_clicks_or_gaps() {
    // Toggling key lock quickly at 0 % pitch must not cause jumps or gaps.
    let mut rig = Rig::new();
    let t = sine_track(100.0, 30.0);
    rig.load(0, &t, None);
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK * 40);
    let mut out = Vec::new();
    let mut on = false;
    for i in 0..400 {
        // Every 2 to 7 blocks (10 to 37 ms), irregular like a hand.
        if i % (2 + (i * 7) % 6) == 0 {
            on = !on;
            rig.deck(0, DeckCommand::SetKeyLock(on));
        }
        out.extend(rig.render(BLOCK));
    }
    // The sine's own largest step is 0.4 * 2 pi * 100 / 48 000 = 0.0052.
    let step = max_step(&out);
    assert!(step < 0.008, "click: step {step}");
    let quietest = out
        .chunks(480)
        .map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt())
        .fold(f32::MAX, f32::min);
    assert!(quietest > 0.25, "gap: {quietest}");
}

#[test]
fn released_backspin_brakes_back_to_play_speed() {
    // Pioneer-style brake: let go while spinning and the deck
    // keeps its speed, then slows down smoothly to play speed by itself,
    // whatever the wheel does.
    let mut rig = Rig::new();
    let t = sine_track(100.0, 60.0);
    rig.load(0, &t, None);
    rig.deck(0, DeckCommand::Seek(20.0 * f64::from(SR)));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(BLOCK * 20);
    rig.deck(0, DeckCommand::JogTouch(true));
    // A backspin: about 3x backwards for 0.2 s.
    for _ in 0..40 {
        rig.deck(0, DeckCommand::JogScratch(-3.0 * 256.0 / 48_000.0 / 1.8));
        rig.render(BLOCK);
    }
    assert!(rig.snap(0).speed < -2.0, "{}", rig.snap(0).speed);
    rig.deck(0, DeckCommand::JogTouch(false));
    let mut speeds = Vec::new();
    for _ in 0..400 {
        // The wheel keeps sending ticks: ignored after the release.
        rig.deck(0, DeckCommand::JogScratch(0.01));
        rig.render(BLOCK);
        speeds.push(rig.snap(0).speed);
    }
    // Rises steadily (no jumps back and forth) and plays on at play speed.
    for w in speeds.windows(2) {
        assert!(w[1] >= w[0] - 1e-9, "{} -> {}", w[0], w[1]);
        assert!(w[1] - w[0] < 0.05, "jump {} -> {}", w[0], w[1]);
    }
    let s = rig.snap(0);
    assert!(!s.scrubbing && s.playing, "{s:?}");
    assert!(near(s.speed, 1.0, 1e-6), "{}", s.speed);
}

#[test]
fn pause_during_a_backspin_lets_it_finish_then_stops() {
    // PAUSE during a backspin cut the sound at once and
    // the track played on after it. The backspin must be heard to its end
    // and the deck stop there. Pressed after the release (spinning free) and
    // while the platter is still held.
    for pause_while_held in [false, true] {
        let mut rig = Rig::new();
        let t = sine_track(100.0, 60.0);
        rig.load(0, &t, None);
        rig.deck(0, DeckCommand::Seek(20.0 * f64::from(SR)));
        rig.deck(0, DeckCommand::Play(true));
        rig.render(BLOCK * 20);
        rig.deck(0, DeckCommand::JogTouch(true));
        for _ in 0..40 {
            rig.deck(0, DeckCommand::JogScratch(-3.0 * 256.0 / 48_000.0 / 1.8));
            rig.render(BLOCK);
        }
        if pause_while_held {
            rig.deck(0, DeckCommand::TogglePlay);
        }
        rig.deck(0, DeckCommand::JogTouch(false));
        rig.render(BLOCK);
        if !pause_while_held {
            rig.deck(0, DeckCommand::TogglePlay);
        }
        // Still spinning backwards and audible right after PAUSE.
        let mut speeds = Vec::new();
        let mut positions = Vec::new();
        let mut first = Vec::new();
        for i in 0..400 {
            let out = rig.render(BLOCK);
            if i < 4 {
                first.extend(out);
            }
            let s = rig.snap(0);
            if s.scrubbing {
                speeds.push(s.speed);
            }
            positions.push(s.position);
        }
        let rms = (first.iter().map(|v| v * v).sum::<f32>() / first.len() as f32).sqrt();
        assert!(rms > 0.25, "held={pause_while_held}: cut, rms {rms}");
        assert!(speeds[0] < -1.0, "held={pause_while_held}: {}", speeds[0]);
        // Slows down steadily to a stop while spinning.
        for w in speeds.windows(2) {
            assert!(w[1] >= w[0] - 1e-9, "{} -> {}", w[0], w[1]);
            assert!(w[1] <= 1e-9, "held={pause_while_held}: turns: {}", w[1]);
        }
        // Never moves forwards again: it stops where the backspin ends.
        for w in positions.windows(2) {
            assert!(
                w[1] <= w[0] + 1e-6,
                "held={pause_while_held}: plays on {} -> {}",
                w[0],
                w[1]
            );
        }
        let s = rig.snap(0);
        assert!(!s.scrubbing && !s.playing, "held={pause_while_held}: {s:?}");
        let end = s.position;
        // And stays stopped and silent.
        let out = rig.render(BLOCK * 50);
        let s = rig.snap(0);
        assert!(!s.playing && near(s.position, end, 1e-6), "{s:?}");
        assert!(
            out.iter().all(|v| v.abs() < 1e-6),
            "held={pause_while_held}"
        );
    }
}

#[test]
fn layer_takeover_shows_the_fader_at_once() {
    // The fader position comes with the LAYER switch, so the ghost
    // is there before the fader moves.
    let mut rig = Rig::new();
    let t = sine_track(100.0, 20.0);
    rig.load(2, &t, Some(grid(120.0)));
    rig.deck(2, DeckCommand::PitchTakeover(Some(-0.3)));
    rig.render(BLOCK);
    assert_eq!(rig.snap(2).pitch_fader_ghost, Some(-0.3));
    assert!(near(rig.snap(2).pitch, 0.0, 1e-9), "waits");
    // At its own pitch already: no ghost, it follows.
    rig.deck(2, DeckCommand::PitchTakeover(Some(0.0)));
    rig.render(BLOCK);
    assert_eq!(rig.snap(2).pitch_fader_ghost, None);
}

/// The reported sync case: deck 1 at +5 % (its fader there),
/// deck 2 synced to it; deck 1 stopped, a 128 BPM track loaded on it,
/// synced and played; deck 2 stopped. Deck 1 keeps 126 BPM (deck 2's synced
/// tempo), with auto sync master on (deck 1 becomes the master) and off
/// (the stopped deck 2 stays the master), also when its fader at +5 % sends
/// again.
#[test]
fn a_synced_deck_keeps_its_tempo_when_the_master_stops() {
    for auto in [false, true] {
        let mut rig = Rig::new();
        rig.send(Command::SetAutoSyncMaster(auto));
        let a = click_track(120.0, 60.0);
        let b = click_track(125.0, 60.0);
        let c = click_track(128.0, 60.0);
        rig.load(0, &a, Some(grid(120.0)));
        rig.deck(0, DeckCommand::SetPitch(0.625)); // +5 % on a ±8 % fader
        rig.deck(0, DeckCommand::PitchFader(0.625)); // the fader is there
        rig.deck(0, DeckCommand::Play(true));
        rig.load(1, &b, Some(grid(125.0)));
        rig.deck(1, DeckCommand::Play(true));
        rig.send(Command::Sync(DeckId::ALL[1], true));
        rig.beats(4.0);
        assert!(near(rig.snap(1).tempo * 125.0, 126.0, 0.2));
        rig.deck(0, DeckCommand::Play(false));
        rig.beats(2.0);
        rig.load(0, &c, Some(grid(128.0)));
        rig.send(Command::Sync(DeckId::ALL[0], true));
        rig.deck(0, DeckCommand::Play(true));
        rig.beats(4.0);
        rig.deck(1, DeckCommand::Play(false));
        rig.beats(4.0);
        let s = *rig.h.snapshot();
        assert_eq!(s.sync_master, Some(if auto { 0 } else { 1 }), "auto {auto}");
        assert!(
            near(s.decks[0].tempo * 128.0, 126.0, 0.05),
            "auto {auto}: deck 1 at {:.2} BPM",
            s.decks[0].tempo * 128.0
        );
        // The fader sends its place again (touched, or jitter).
        rig.deck(0, DeckCommand::PitchFader(0.627));
        rig.beats(2.0);
        assert!(
            near(rig.snap(0).tempo * 128.0, 126.0, 0.05),
            "auto {auto}: after a fader message {:.2} BPM",
            rig.snap(0).tempo * 128.0
        );
    }
}

/// The reported case with its numbers: 160 BPM at +8 % (172.8
/// BPM) on deck 1, a 165 BPM track synced to it on deck 2; deck 1 stopped,
/// a 146 BPM track on it synced (+18.4 %, beyond the ±8 % fader) and
/// played; deck 2 stopped. Deck 1 stays at 172.8 BPM: it used to drop to
/// the edge of its fader range, +8 % = 157.7 BPM. The fader then moves the
/// tempo relatively (Serato's relative pitch); a new pitch range and SYNC
/// off keep the tempo.
#[test]
fn a_tempo_beyond_the_fader_range_survives_leaving_sync() {
    for auto in [false, true] {
        let mut rig = Rig::new();
        rig.send(Command::SetAutoSyncMaster(auto));
        rig.load(0, &click_track(160.0, 60.0), Some(grid(160.0)));
        rig.deck(0, DeckCommand::SetPitch(1.0)); // +8 %
        rig.deck(0, DeckCommand::PitchFader(1.0)); // the fader is there
        rig.deck(0, DeckCommand::Play(true));
        rig.load(1, &click_track(165.0, 60.0), Some(grid(165.0)));
        rig.send(Command::Sync(DeckId::ALL[1], true));
        rig.deck(1, DeckCommand::Play(true));
        rig.beats(4.0);
        assert!(near(rig.snap(1).tempo * 165.0, 172.8, 0.1), "deck 2 synced");
        rig.deck(0, DeckCommand::Play(false));
        rig.beats(2.0);
        rig.load(0, &click_track(146.0, 60.0), Some(grid(146.0)));
        rig.send(Command::Sync(DeckId::ALL[0], true));
        rig.deck(0, DeckCommand::Play(true));
        rig.beats(4.0);
        let rate = rig.snap(0).sync_rate.expect("deck 1 follows");
        assert!(
            near(rate * 146.0, 172.8, 0.05),
            "deck 1 synced to {:.2} BPM",
            rate * 146.0
        );
        rig.deck(1, DeckCommand::Play(false));
        rig.beats(4.0);
        let bpm = rig.snap(0).tempo * 146.0;
        assert!(
            near(bpm, 172.8, 0.05),
            "auto {auto}: deck 1 at {bpm:.2} BPM after deck 2 stopped (157.7 = the old jump)"
        );
    }

    // Deck 1 the master beyond its fader: the fader moves it relatively.
    let mut rig = Rig::new();
    rig.send(Command::SetAutoSyncMaster(true));
    rig.load(0, &click_track(146.0, 60.0), Some(grid(146.0)));
    rig.load(1, &click_track(172.8, 60.0), Some(grid(172.8)));
    rig.deck(0, DeckCommand::PitchFader(1.0)); // the fader at +8 %
    rig.deck(1, DeckCommand::Play(true));
    rig.send(Command::Sync(DeckId::ALL[0], true));
    rig.deck(0, DeckCommand::Play(true));
    rig.beats(2.0);
    rig.deck(1, DeckCommand::Play(false));
    rig.beats(2.0);
    assert_eq!(rig.h.snapshot().sync_master, Some(0));
    let t0 = rig.snap(0).tempo;
    assert!(near(t0 * 146.0, 172.8, 0.05));
    // Fader down by 1/8 of its travel up (1 % on ±8 %): 1 % slower.
    rig.deck(0, DeckCommand::PitchFader(0.875));
    rig.beats(1.0);
    assert!(
        near(rig.snap(0).tempo, t0 - 0.01, 1e-4),
        "{}",
        rig.snap(0).tempo
    );
    // A wider range keeps the tempo (the fader's place changes).
    let t1 = rig.snap(0).tempo;
    rig.deck(0, DeckCommand::SetPitchRange(dj_audio::PitchRange::Sixteen));
    rig.beats(1.0);
    assert!(near(rig.snap(0).tempo, t1, 1e-6), "{}", rig.snap(0).tempo);
    // SYNC off on a follower beyond its range: the tempo stays.
    rig.load(1, &click_track(172.8, 60.0), Some(grid(172.8)));
    rig.deck(1, DeckCommand::Play(true));
    rig.deck(2, DeckCommand::Play(false));
    rig.load(2, &click_track(140.0, 60.0), Some(grid(140.0)));
    rig.send(Command::Sync(DeckId::ALL[2], true));
    rig.deck(2, DeckCommand::Play(true));
    rig.beats(2.0);
    let synced = rig.snap(2).sync_rate.expect("deck 3 follows");
    assert!(synced > 1.08, "beyond ±8 %: {synced}");
    rig.send(Command::Sync(DeckId::ALL[2], false));
    rig.beats(1.0);
    assert!(
        near(rig.snap(2).tempo, synced, 2e-3),
        "{} vs {synced}",
        rig.snap(2).tempo
    );
}
