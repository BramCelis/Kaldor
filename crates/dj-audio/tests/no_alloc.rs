//! Checks that `Engine::process` never allocates or frees memory.
//!
//! This test binary installs `assert_no_alloc`'s allocator. In debug builds
//! any allocation inside `assert_no_alloc(..)` aborts the test process, which
//! fails the test run.

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use dj_audio::{Command, Engine, EngineConfig, EngineHandle, TestTone, ToneTarget};
use dj_core::OutputRouting;

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[test]
fn process_does_not_allocate() {
    let (mut engine, mut handle) = Engine::new(EngineConfig {
        sample_rate: 48_000,
        max_block_frames: 256,
        routing: OutputRouting::ns4fx(),
    });
    let mut buf = vec![0.0f32; 512 * 4];
    handle
        .send(Command::SetTestTone(Some(TestTone::new(ToneTarget::Cue))))
        .unwrap();
    handle
        .send(Command::SetRouting(OutputRouting::laptop_split_cue()))
        .unwrap();
    for _ in 0..200 {
        assert_no_alloc(|| engine.process(&mut buf, 4));
    }
    handle.send(Command::SetTestTone(None)).unwrap();
    for _ in 0..10 {
        assert_no_alloc(|| engine.process(&mut buf, 4));
    }
    assert!(handle.update().frames_rendered > 0);
}

#[test]
fn deck_playback_does_not_allocate() {
    use dj_audio::{DeckCommand, TrackAudio};
    use dj_core::DeckId;
    use std::sync::Arc;

    let (mut engine, mut h) = Engine::new(EngineConfig {
        sample_rate: 48_000,
        max_block_frames: 256,
        routing: OutputRouting::ns4fx(),
    });
    let samples: Vec<f32> = (0..48_000 * 4)
        .map(|i| ((i as f32) * 0.01).sin() * 0.3)
        .collect();
    let a = TrackAudio::from_interleaved(48_000, &samples);
    let b = TrackAudio::from_interleaved(48_000, &samples);
    let mut buf = vec![0.0f32; 300 * 4];
    let mut run = |engine: &mut Engine, blocks: usize| {
        for _ in 0..blocks {
            assert_no_alloc(|| engine.process(&mut buf, 4));
        }
    };
    let d = |i: usize| DeckId::ALL[i];
    for i in 0..4 {
        h.send(Command::Deck(
            d(i),
            DeckCommand::Load {
                audio: Arc::clone(&a),
                serial: 1,
            },
        ))
        .unwrap();
        h.send(Command::Deck(d(i), DeckCommand::Play(true)))
            .unwrap();
    }
    h.send(Command::Deck(d(1), DeckCommand::SetKeyLock(true)))
        .unwrap();
    h.send(Command::Deck(d(1), DeckCommand::SetPitch(0.7)))
        .unwrap();
    h.send(Command::Deck(d(2), DeckCommand::SetPitch(-0.3)))
        .unwrap();
    {
        use dj_audio::{EqBand, MixerCommand as M, XfaderSide};
        for cmd in [
            M::Eq(d(0), EqBand::High, 0.9),
            M::Kill(d(1), EqBand::Low, true),
            M::Filter(d(2), -0.6),
            M::Filter(d(3), 0.6),
            M::Pfl(d(0), true),
            M::Fader(d(1), 0.3),
            M::XfaderAssign(d(0), XfaderSide::A),
            M::XfaderAssign(d(1), XfaderSide::B),
            M::Crossfader(0.4),
            M::CueMix(0.5),
            M::MasterGain(3.0),
            M::Trim(d(2), 0.8),
            M::EqCrossovers(300.0, 3000.0),
            M::EqMode(dj_dsp::curves::EqMode::Classic),
            M::EqBoost(12.0),
            M::FilterShape(dj_dsp::svf::FilterSlope::Db24, 0.9),
            M::CrossfaderCutLag(0.01),
        ] {
            h.send(Command::Mixer(cmd)).unwrap();
        }
    }
    run(&mut engine, 100);
    h.send(Command::Deck(d(0), DeckCommand::Seek(10_000.0)))
        .unwrap();
    h.send(Command::Deck(
        d(2),
        DeckCommand::Scrub {
            active: true,
            target: 50_000.0,
        },
    ))
    .unwrap();
    h.send(Command::Deck(d(3), DeckCommand::Cue(true))).unwrap();
    run(&mut engine, 50);
    // Replacing a track inside the callback hands the old one back to the
    // UI thread instead of freeing it.
    h.send(Command::Deck(
        d(0),
        DeckCommand::Load {
            audio: Arc::clone(&b),
            serial: 2,
        },
    ))
    .unwrap();
    h.send(Command::Deck(d(3), DeckCommand::Cue(false)))
        .unwrap();
    h.update(); // drain the snapshot queue so the next snapshot is fresh
    run(&mut engine, 50);
    h.update();
    assert_eq!(h.snapshot().decks[0].track_serial, 2);
}

/// Performance features: hot cues, loops, rolls, slicer, slip, beat jump,
/// sync, key shift and the sampler (play and record) inside the callback.
#[test]
fn performance_features_do_not_allocate() {
    use dj_audio::{
        DeckCommand as D, DeckGrid, RecordBuffer, SampleMode, SamplerCommand as S, TrackAudio,
    };
    use dj_core::DeckId;
    use std::sync::Arc;

    let (mut engine, mut h) = Engine::new(EngineConfig {
        sample_rate: 48_000,
        max_block_frames: 256,
        routing: OutputRouting::ns4fx(),
    });
    let samples: Vec<f32> = (0..48_000 * 20)
        .map(|i| ((i as f32) * 0.013).sin() * 0.3)
        .collect();
    let a = TrackAudio::from_interleaved(48_000, &samples);
    let short = TrackAudio::from_interleaved(48_000, &samples[..48_000]);
    let grid = |bpm: f64| DeckGrid {
        first_beat: 100.0,
        beat_len: 60.0 * 48_000.0 / bpm,
        first_beat_in_bar: 2,
    };
    let mut buf = vec![0.0f32; 256 * 4];
    let mut run = |engine: &mut Engine, h: &mut EngineHandle, blocks: usize| {
        for _ in 0..blocks {
            assert_no_alloc(|| engine.process(&mut buf, 4));
        }
        h.update();
    };
    let d = |i: usize| DeckId::ALL[i];
    for i in 0..4 {
        for c in [
            D::Load {
                audio: Arc::clone(&a),
                serial: 1,
            },
            D::SetGrid(Some(grid(120.0 + i as f64 * 3.0))),
            D::Seek(96_000.0),
            D::Play(true),
        ] {
            h.send(Command::Deck(d(i), c)).unwrap();
        }
    }
    h.send(Command::Deck(d(1), D::SetKeyLock(true))).unwrap();
    h.send(Command::Quantize(true)).unwrap();
    h.send(Command::Sync(d(1), true)).unwrap();
    h.send(Command::Sync(d(2), true)).unwrap();
    run(&mut engine, &mut h, 40);
    let steps: Vec<Command> = vec![
        Command::Deck(
            d(0),
            D::HotCue {
                slot: 0,
                pressed: true,
                loop_beats: 0.0,
            },
        ),
        Command::Deck(
            d(0),
            D::HotCue {
                slot: 0,
                pressed: false,
                loop_beats: 0.0,
            },
        ),
        Command::Deck(
            d(0),
            D::HotCue {
                slot: 0,
                pressed: true,
                loop_beats: 4.0,
            },
        ),
        Command::Deck(d(1), D::AutoLoop(0.5)),
        Command::Deck(d(1), D::LoopHalve),
        Command::Deck(d(1), D::LoopMove(1)),
        Command::Deck(d(2), D::SetSlip(true)),
        Command::Deck(
            d(2),
            D::Roll {
                beats: 0.125,
                pressed: true,
            },
        ),
        Command::Deck(
            d(3),
            D::Slicer {
                slice: 2,
                pressed: true,
                domain_beats: 8.0,
            },
        ),
        Command::Deck(d(1), D::KeyShift(3)),
        Command::Deck(d(0), D::BeatJump(-4.0)),
        Command::Deck(
            d(2),
            D::Roll {
                beats: 0.125,
                pressed: false,
            },
        ),
        Command::Deck(
            d(3),
            D::Slicer {
                slice: 2,
                pressed: false,
                domain_beats: 8.0,
            },
        ),
        Command::Deck(d(1), D::Reloop),
        Command::Deck(d(0), D::LoopIn),
        Command::Deck(d(0), D::LoopOut),
        Command::Deck(d(0), D::LoopDouble),
        Command::Deck(
            d(0),
            D::SetLoop {
                start: 48_000.0,
                end: 72_000.0,
                jump: true,
            },
        ),
        Command::SyncMaster(d(3)),
        // Controller toggles and the jog wheel.
        Command::ToggleSync(d(1)),
        Command::Mixer(dj_audio::MixerCommand::TogglePfl(d(2))),
        Command::Deck(d(0), D::ToggleSlip),
        Command::Deck(d(1), D::CyclePitchRange),
        Command::Deck(d(1), D::ToggleKeyLock),
        Command::Deck(d(0), D::JogTouch(true)),
        Command::Deck(d(0), D::JogScratch(0.05)),
        Command::Deck(d(0), D::JogScratch(-0.02)),
        Command::Deck(d(0), D::JogTouch(false)),
        Command::Deck(d(1), D::JogNudge(0.01)),
        Command::Deck(d(2), D::SeekBy(4_800.0)),
        Command::Sampler(S::Load {
            slot: 9,
            audio: Arc::clone(&short),
            bpm: Some(124.0),
        }),
        Command::Sampler(S::SetMode(9, SampleMode::Loop)),
        Command::Sampler(S::SetSync(9, true)),
        Command::Sampler(S::Trigger {
            slot: 9,
            pressed: true,
        }),
        Command::Sampler(S::Record(RecordBuffer::new(10, 4096))),
        Command::Deck(
            d(2),
            D::Scrub {
                active: true,
                target: 50_000.0,
            },
        ),
        Command::Deck(
            d(2),
            D::Scrub {
                active: false,
                target: 0.0,
            },
        ),
        Command::Sync(d(1), false),
        Command::Sampler(S::Eject(9)),
        Command::Sampler(S::Record(RecordBuffer::new(11, 48_000))),
        Command::Sampler(S::StopRecord),
        Command::Deck(d(0), D::LoopExit),
        // Settings: WSOLA key lock, stop brake, stutter cue,
        // pre-EQ cue, limiter ceiling, meter hold, automatic sync master.
        Command::Deck(d(1), D::SetKeyLockWsola(true)),
        Command::Deck(d(1), D::SetKeyLock(true)),
        Command::Mixer(dj_audio::MixerCommand::CuePreEq(true)),
        Command::Mixer(dj_audio::MixerCommand::LimiterCeiling(-1.0)),
        Command::Mixer(dj_audio::MixerCommand::MeterHold(2.0)),
        Command::SetAutoSyncMaster(true),
        Command::Deck(d(0), D::SetStutterCue(true)),
        Command::Deck(d(0), D::Cue(true)),
        Command::Deck(d(0), D::Cue(false)),
        Command::Deck(d(2), D::SetStopBrake(0.05)),
        Command::Deck(d(2), D::Play(false)),
        Command::Deck(d(2), D::Play(true)),
        Command::Deck(d(2), D::Play(false)),
        Command::Deck(d(1), D::SetKeyLockWsola(false)),
    ];
    for c in steps {
        h.send(c).unwrap();
        run(&mut engine, &mut h, 6);
    }
    run(&mut engine, &mut h, 40);
    assert_eq!(h.take_recordings().len(), 2);
    assert_eq!(h.snapshot().leaked_tracks, 0);
}

/// Both FX units on every target with every effect (switching
/// effects, tails dying away) and master recording, without allocating.
#[test]
fn fx_and_recording_do_not_allocate() {
    use dj_audio::{DeckCommand, DeckGrid, EffectKind, FX_SLOTS, FxCommand, FxMode, TrackAudio};
    use dj_core::DeckId;

    let (mut engine, mut h) = Engine::new(EngineConfig {
        sample_rate: 48_000,
        max_block_frames: 256,
        routing: OutputRouting::ns4fx(),
    });
    let mut reader = h.take_record_reader().unwrap();
    let samples: Vec<f32> = (0..48_000 * 8)
        .map(|i| ((i as f32) * 0.013).sin() * 0.3)
        .collect();
    for d in DeckId::ALL {
        h.send(Command::Deck(
            d,
            DeckCommand::Load {
                audio: TrackAudio::from_interleaved(48_000, &samples),
                serial: d.index() as u64 + 1,
            },
        ))
        .unwrap();
        h.send(Command::Deck(
            d,
            DeckCommand::SetGrid(Some(DeckGrid {
                first_beat: 0.0,
                beat_len: 24_000.0,
                first_beat_in_bar: 1,
            })),
        ))
        .unwrap();
        h.send(Command::Deck(d, DeckCommand::TogglePlay)).unwrap();
    }
    let mut buf = vec![0.0f32; 256 * 4];
    for _ in 0..4 {
        engine.process(&mut buf, 4);
    }
    let send = |h: &mut EngineHandle, c: FxCommand| h.send(Command::Fx(c)).unwrap();
    for u in 0..2 {
        for t in 0..5 {
            send(&mut h, FxCommand::Assign(u, t, true));
        }
        send(&mut h, FxCommand::UnitOn(u, true));
    }
    h.send(Command::Record(true)).unwrap();
    for (i, kind) in EffectKind::ALL.into_iter().enumerate() {
        let (u, s) = (i % 2, (i / 2) % FX_SLOTS);
        send(&mut h, FxCommand::SlotKind(u, s, kind));
        send(&mut h, FxCommand::SlotOn(u, s, true));
        send(
            &mut h,
            FxCommand::BeatsStep(u, if i % 3 == 0 { 1 } else { -1 }),
        );
        for _ in 0..20 {
            assert_no_alloc(|| engine.process(&mut buf, 4));
            while reader.pop().is_ok() {}
            h.update();
        }
        send(&mut h, FxCommand::SlotOn(u, s, false));
    }
    send(&mut h, FxCommand::Mode(0, FxMode::Single));
    send(&mut h, FxCommand::Tap(Some(126.0)));
    send(&mut h, FxCommand::UnitOn(0, false));
    // Every color FX on the filter knobs, turned both ways and back (the
    // tails ringing out), switching between them.
    for color in dj_audio::ColorFx::ALL {
        h.send(Command::Mixer(dj_audio::MixerCommand::ColorFx(color)))
            .unwrap();
        for knob in [-0.9, 0.0, 0.9, 0.0] {
            for d in DeckId::ALL {
                h.send(Command::Mixer(dj_audio::MixerCommand::Filter(d, knob)))
                    .unwrap();
            }
            for _ in 0..10 {
                assert_no_alloc(|| engine.process(&mut buf, 4));
                while reader.pop().is_ok() {}
                h.update();
            }
        }
    }
    h.send(Command::Record(false)).unwrap();
    for _ in 0..200 {
        assert_no_alloc(|| engine.process(&mut buf, 4));
        while reader.pop().is_ok() {}
        h.update();
    }
    assert!(!h.update().recording);
}
