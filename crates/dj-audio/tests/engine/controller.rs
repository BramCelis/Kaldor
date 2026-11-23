//! What the controller (MIDI) thread needs from the engine: its own
//! command queue, toggle commands, and the jog wheel (scratch, nudge,
//! search). Offline renderer, no devices.

use dj_audio::offline::OfflineRenderer;
use dj_audio::{
    Command, ControllerQueue, DeckCommand, DeckGrid, DeckSnapshot, Engine, EngineConfig,
    EngineHandle, JOG_SECONDS_PER_TURN, MixerCommand, TrackAudio,
};
use dj_core::{DeckId, OutputRouting};
use std::sync::Arc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;

struct Rig {
    r: OfflineRenderer,
    h: EngineHandle,
    ctl: ControllerQueue,
}

impl Rig {
    fn new() -> Self {
        let (engine, mut h) = Engine::new(EngineConfig {
            sample_rate: SR,
            max_block_frames: BLOCK,
            routing: OutputRouting::laptop(),
        });
        let ctl = h.take_controller_queue().expect("controller queue");
        assert!(h.take_controller_queue().is_none(), "given out once");
        let mut rig = Self {
            r: OfflineRenderer::new(engine, 2, BLOCK),
            h,
            ctl,
        };
        let audio = sine(30.0);
        rig.h
            .send(Command::Deck(
                DeckId::ALL[0],
                DeckCommand::Load { audio, serial: 1 },
            ))
            .unwrap();
        rig.blocks(2);
        rig
    }

    /// A command through the controller queue (as the MIDI thread sends).
    fn ctl(&mut self, c: DeckCommand) {
        self.ctl.send(Command::Deck(DeckId::ALL[0], c)).unwrap();
    }

    fn blocks(&mut self, n: usize) {
        for _ in 0..n {
            self.r.render_block();
            self.h.update();
        }
    }

    fn d(&self) -> DeckSnapshot {
        self.h.snapshot().decks[0]
    }
}

fn sine(seconds: f64) -> Arc<TrackAudio> {
    let n = (seconds * f64::from(SR)) as usize;
    let s: Vec<f32> = (0..n * 2)
        .map(|i| ((i / 2) as f32 * 0.03).sin() * 0.5)
        .collect();
    TrackAudio::from_interleaved(SR, &s)
}

#[test]
fn controller_queue_and_toggles_reach_the_engine() {
    let mut r = Rig::new();
    r.ctl(DeckCommand::TogglePlay);
    r.blocks(2);
    assert!(r.d().playing, "play through the controller queue");
    r.ctl(DeckCommand::ToggleKeyLock);
    r.ctl(DeckCommand::ToggleSlip);
    r.ctl
        .send(Command::Mixer(MixerCommand::TogglePfl(DeckId::ALL[0])))
        .unwrap();
    r.blocks(20);
    let d = r.d();
    assert!(d.key_lock && d.slip);
    assert!(r.h.snapshot().mixer.channels[0].pfl);
    r.ctl(DeckCommand::ToggleKeyLock);
    r.ctl(DeckCommand::ToggleSlip);
    r.ctl
        .send(Command::Mixer(MixerCommand::TogglePfl(DeckId::ALL[0])))
        .unwrap();
    r.blocks(20);
    let d = r.d();
    assert!(!d.key_lock && !d.slip);
    assert!(!r.h.snapshot().mixer.channels[0].pfl);
    // SYNC toggles too (sync needs a beatgrid).
    r.ctl(DeckCommand::SetGrid(Some(DeckGrid {
        first_beat: 0.0,
        beat_len: 24_000.0,
        first_beat_in_bar: 1,
    })));
    r.ctl.send(Command::ToggleSync(DeckId::ALL[0])).unwrap();
    r.blocks(2);
    assert!(r.d().sync);
    r.ctl.send(Command::ToggleSync(DeckId::ALL[0])).unwrap();
    r.blocks(2);
    assert!(!r.d().sync);
}

#[test]
fn jog_scratch_holds_and_moves_the_track_like_a_platter() {
    let mut r = Rig::new();
    r.ctl(DeckCommand::Play(true));
    r.blocks(40);
    // Touch: the deck stops following the clock and holds still.
    r.ctl(DeckCommand::JogTouch(true));
    r.blocks(10);
    let held = r.d().position;
    r.blocks(20);
    assert!(
        (r.d().position - held).abs() < 1.0,
        "a held platter does not move"
    );
    assert!(r.d().scrubbing);
    // A quarter turn forward moves 0.45 s of audio.
    r.ctl(DeckCommand::JogScratch(0.25));
    r.blocks(80);
    let want = held + 0.25 * JOG_SECONDS_PER_TURN * f64::from(SR);
    assert!(
        (r.d().position - want).abs() < 2.0,
        "{} vs {want}",
        r.d().position
    );
    // And back.
    r.ctl(DeckCommand::JogScratch(-0.25));
    r.blocks(80);
    assert!((r.d().position - held).abs() < 2.0);
    // Release: it plays on.
    r.ctl(DeckCommand::JogTouch(false));
    r.blocks(4);
    let p = r.d().position;
    r.blocks(20);
    assert!(r.d().playing && r.d().position > p + 1_000.0);
}

#[test]
fn jog_nudge_bends_while_playing_and_moves_while_paused() {
    let mut r = Rig::new();
    r.ctl(DeckCommand::Play(true));
    r.blocks(40);
    for _ in 0..20 {
        r.ctl(DeckCommand::JogNudge(0.01));
        r.blocks(1);
    }
    assert!(r.d().tempo > 1.02, "nudged up: {}", r.d().tempo);
    // The tempo shown (BPM, pitch) stays put: a nudge only lines up the
    // beats.
    assert!((r.d().set_tempo - 1.0).abs() < 1e-9, "{}", r.d().set_tempo);
    // The pitch-bend buttons and the fader still show.
    r.ctl(DeckCommand::PitchBend(0.04));
    r.blocks(1);
    assert!((r.d().set_tempo - 1.04).abs() < 1e-9, "{}", r.d().set_tempo);
    r.ctl(DeckCommand::PitchBend(0.0));
    r.blocks(1);
    // It fades out by itself.
    r.blocks(200);
    assert!((r.d().tempo - 1.0).abs() < 0.001, "{}", r.d().tempo);
    // Paused: a nudge moves the position (a quarter turn = 0.45 s).
    r.ctl(DeckCommand::Play(false));
    r.blocks(40);
    let p = r.d().position;
    r.ctl(DeckCommand::JogNudge(0.25));
    r.blocks(2);
    let want = p + 0.25 * JOG_SECONDS_PER_TURN * f64::from(SR);
    assert!(
        (r.d().position - want).abs() < 1.0,
        "{} vs {want}",
        r.d().position
    );
    // Fast search.
    r.ctl(DeckCommand::SeekBy(48_000.0));
    r.blocks(2);
    assert!((r.d().position - want - 48_000.0).abs() < 1.0);
}

/// Positions over the next blocks (frames), after a command.
fn trace(r: &mut Rig, blocks: usize) -> Vec<f64> {
    (0..blocks)
        .map(|_| {
            r.blocks(1);
            r.d().position
        })
        .collect()
}

/// LOOP OUT after the end point, and RELOOP after the loop, went one
/// loop length too far back (the loop wrap and a relative jump both moved
/// the deck). Now they land on the loop start and stay in the loop.
#[test]
fn loop_out_and_reloop_land_on_the_loop_start() {
    for key_lock in [false, true] {
        let mut r = Rig::new();
        r.ctl(DeckCommand::SetGrid(Some(DeckGrid {
            first_beat: 0.0,
            beat_len: 24_000.0,
            first_beat_in_bar: 1,
        })));
        r.ctl(DeckCommand::SetKeyLock(key_lock));
        r.ctl(DeckCommand::Play(true));
        while r.d().position < 5.0 * 48_000.0 {
            r.blocks(1);
        }
        r.ctl(DeckCommand::LoopIn);
        let start = r.d().position;
        while r.d().position < 7.0 * 48_000.0 {
            r.blocks(1);
        }
        r.ctl(DeckCommand::LoopOut);
        let t = trace(&mut r, 400);
        let l = r.d().loop_info.unwrap();
        assert!((l.start - start).abs() < 1.0);
        let inside = |t: &[f64]| t.iter().all(|p| *p >= l.start - 1.0 && *p < l.end);
        assert!(inside(&t), "key lock {key_lock}: after OUT {:?}", &t[..6]);
        assert!(
            (t[0] - l.start).abs() < 600.0,
            "starts at the loop start: {}",
            t[0]
        );
        // Exit, play past the loop, reloop.
        r.ctl(DeckCommand::LoopExit);
        while r.d().position < l.end + 48_000.0 {
            r.blocks(1);
        }
        r.ctl(DeckCommand::Reloop);
        let t = trace(&mut r, 400);
        assert!(
            inside(&t),
            "key lock {key_lock}: after RELOOP {:?}",
            &t[..6]
        );
        assert!(
            (t[0] - l.start).abs() < 600.0,
            "reloop starts at the loop start: {}",
            t[0]
        );
    }
}

/// Let go of the jog during a
/// backspin or a throw and the deck brakes back to play speed by itself
/// (Pioneer style), whatever the wheel sends; let go of a still platter and
/// it plays on at once.
#[test]
fn a_released_spinning_jog_brakes_back_to_play() {
    let mut r = Rig::new();
    r.ctl(DeckCommand::Play(true));
    while r.d().position < 10.0 * 48_000.0 {
        r.blocks(1);
    }
    // Grab and spin backwards fast (about 6.75x).
    r.ctl(DeckCommand::JogTouch(true));
    for _ in 0..10 {
        r.ctl(DeckCommand::JogScratch(-0.02));
        r.blocks(1);
    }
    let at_release = r.d().position;
    r.ctl(DeckCommand::JogTouch(false));
    // Still going back for a while (whatever the wheel sends)...
    for _ in 0..10 {
        r.ctl(DeckCommand::JogScratch(0.05));
        r.blocks(1);
    }
    assert!(r.d().scrubbing, "still braking");
    assert!(
        r.d().position < at_release - 10_000.0,
        "kept going back: {} vs {at_release}",
        r.d().position
    );
    // ...then plays forward again (about 2 s from -6.75x to 1x).
    r.blocks(400);
    let p = r.d().position;
    r.blocks(20);
    assert!(!r.d().scrubbing && r.d().playing);
    assert!(r.d().position > p + 4_000.0, "plays on");

    // A forward throw: slows down to play speed, then plays on.
    r.ctl(DeckCommand::JogTouch(true));
    for _ in 0..10 {
        r.ctl(DeckCommand::JogScratch(0.02));
        r.blocks(1);
    }
    r.ctl(DeckCommand::JogTouch(false));
    r.blocks(5);
    assert!(r.d().scrubbing, "still coasting");
    assert!(r.d().speed > 1.5, "{}", r.d().speed);
    r.blocks(400);
    assert!(!r.d().scrubbing && r.d().playing, "plays on at play speed");

    // A still platter: released at once.
    r.ctl(DeckCommand::JogTouch(true));
    r.blocks(10);
    r.ctl(DeckCommand::JogTouch(false));
    r.blocks(1);
    assert!(!r.d().scrubbing, "released at once");
}

/// LOOP 1/2 in the second half of a loop keeps the deck inside the
/// (now shorter) loop instead of jumping before it.
#[test]
fn halving_a_loop_in_its_second_half_stays_inside() {
    let mut r = Rig::new();
    r.ctl(DeckCommand::SetGrid(Some(DeckGrid {
        first_beat: 0.0,
        beat_len: 24_000.0,
        first_beat_in_bar: 1,
    })));
    r.ctl(DeckCommand::Play(true));
    while r.d().position < 4.0 * 48_000.0 {
        r.blocks(1);
    }
    r.ctl(DeckCommand::AutoLoop(4.0));
    r.blocks(2);
    let l = r.d().loop_info.unwrap();
    // Into the last beat of the 4-beat loop, then halve.
    while r.d().position < l.start + 3.5 * 24_000.0 {
        r.blocks(1);
    }
    r.ctl(DeckCommand::LoopHalve);
    let t = trace(&mut r, 400);
    let h = r.d().loop_info.unwrap();
    assert!((h.end - h.start - 48_000.0).abs() < 1.0);
    assert!(
        t.iter().all(|p| *p >= h.start - 1.0 && *p < h.end),
        "left the halved loop: min {}",
        t.iter().cloned().fold(f64::MAX, f64::min)
    );
}

/// SHIFT + PITCH BEND - on the NS4FX: the next pitch range, round and round.
#[test]
fn pitch_range_cycles() {
    let mut r = Rig::new();
    let ranges: Vec<_> = (0..4)
        .map(|_| {
            r.ctl(DeckCommand::CyclePitchRange);
            r.blocks(1);
            r.d().pitch_range
        })
        .collect();
    use dj_audio::PitchRange::*;
    assert_eq!(ranges, vec![Sixteen, Fifty, Eight, Sixteen]);
}
