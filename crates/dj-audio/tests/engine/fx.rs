//! FX units and master recording in the engine (offline renderer,
//! no devices).

use dj_audio::offline::{OfflineRenderer, channel, rms};
use dj_audio::{
    BEATS, Command, DeckCommand, DeckGrid, EffectKind, Engine, EngineConfig, EngineHandle,
    FX_SLOTS, FxCommand, FxMode, MASTER_TARGET, MixerCommand, TrackAudio,
};
use dj_core::{DeckId, OutputRouting};
use std::sync::Arc;

const SR: u32 = 48_000;
const BLOCK: usize = 256;

struct Rig {
    r: OfflineRenderer,
    h: EngineHandle,
}

impl Rig {
    /// Decks 1 and 2 loaded (120 BPM grid) and playing.
    fn new() -> Self {
        let (engine, h) = Engine::new(EngineConfig {
            sample_rate: SR,
            max_block_frames: BLOCK,
            routing: OutputRouting::laptop(),
        });
        let mut rig = Self {
            r: OfflineRenderer::new(engine, 2, BLOCK),
            h,
        };
        for i in 0..2 {
            let d = DeckId::ALL[i];
            rig.send(Command::Deck(
                d,
                DeckCommand::Load {
                    audio: tone(30.0, 0.02 + 0.01 * i as f32),
                    serial: i as u64 + 1,
                },
            ));
            rig.send(Command::Deck(
                d,
                DeckCommand::SetGrid(Some(DeckGrid {
                    first_beat: 0.0,
                    beat_len: f64::from(SR) / 2.0,
                    first_beat_in_bar: 1,
                })),
            ));
            rig.send(Command::Deck(d, DeckCommand::TogglePlay));
        }
        rig.render(BLOCK * 4);
        rig
    }

    fn send(&mut self, c: Command) {
        self.h.send(c).unwrap();
    }

    fn fx(&mut self, c: FxCommand) {
        self.send(Command::Fx(c));
    }

    /// Renders and returns the left master channel (reading snapshots as
    /// it goes, as the UI does, so the newest state is seen).
    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(frames * 2);
        for _ in 0..frames.div_ceil(BLOCK) {
            out.extend_from_slice(self.r.render_block());
            self.h.update();
        }
        channel(&out, 2, 0)
    }
}

fn tone(seconds: f64, step: f32) -> Arc<TrackAudio> {
    let n = (seconds * f64::from(SR)) as usize;
    let s: Vec<f32> = (0..n * 2)
        .map(|i| ((i / 2) as f32 * step).sin() * 0.4)
        .collect();
    TrackAudio::from_interleaved(SR, &s)
}

/// Only deck 1 heard, unit 1 on deck 1 with an echo.
fn echo_rig() -> Rig {
    let mut r = Rig::new();
    r.send(Command::Mixer(MixerCommand::Fader(DeckId::ALL[1], 0.0)));
    for s in 0..FX_SLOTS {
        r.fx(FxCommand::SlotOn(0, s, false));
    }
    r.fx(FxCommand::SlotKind(0, 0, EffectKind::Echo));
    r.fx(FxCommand::SlotOn(0, 0, true));
    r.fx(FxCommand::Mix(0, 0.5));
    r
}

#[test]
fn defaults_put_unit_1_on_the_left_decks_and_unit_2_on_the_right() {
    let r = Rig::new();
    let p = r.h.snapshot().fx.params;
    assert_eq!(p.units[0].assign, [true, false, true, false, false]);
    assert_eq!(p.units[1].assign, [false, true, false, true, false]);
    assert!(!p.units[0].on && !p.units[1].on);
    assert_eq!(BEATS[p.units[0].beats], 1.0);
}

#[test]
fn echo_tail_rings_on_after_the_fader_closes_then_the_signal_is_untouched() {
    let mut fx = echo_rig();
    let mut dry = echo_rig();
    // Dry stays off; fx gets the unit on.
    fx.fx(FxCommand::UnitOn(0, true));
    let a = fx.render(SR as usize);
    let b = dry.render(SR as usize);
    assert!(
        a.iter().zip(b.iter()).any(|(x, y)| (x - y).abs() > 1e-3),
        "the echo changes the sound"
    );
    assert!(fx.h.snapshot().fx.sounding[0][0]);
    // Fader down (and unit off): the echo keeps sounding for a while.
    for r in [&mut fx, &mut dry] {
        r.send(Command::Mixer(MixerCommand::Fader(DeckId::ALL[0], 0.0)));
    }
    fx.fx(FxCommand::UnitOn(0, false));
    let a = fx.render(SR as usize / 2);
    let b = dry.render(SR as usize / 2);
    assert!(rms(&a[2_400..]) > 0.01, "tail: {}", rms(&a[2_400..]));
    assert!(rms(&b[2_400..]) < 1e-6, "dry is silent");
    // Long after: the tail is gone and the instance stopped.
    let a = fx.render(SR as usize * 20);
    let _ = dry.render(SR as usize * 20);
    assert!(rms(&a[a.len() - 4_800..]) < 1e-4);
    assert!(!fx.h.snapshot().fx.sounding[0][0]);
    // Fader up again: bit-identical to the engine that never had FX on.
    for r in [&mut fx, &mut dry] {
        r.send(Command::Mixer(MixerCommand::Fader(DeckId::ALL[0], 1.0)));
    }
    let a = fx.render(SR as usize / 4);
    let b = dry.render(SR as usize / 4);
    assert_eq!(a, b);
}

#[test]
fn a_unit_only_touches_its_targets() {
    let mut fx = echo_rig();
    let mut dry = echo_rig();
    // Only deck 2 heard; unit 1 is on decks 1 and 3.
    for r in [&mut fx, &mut dry] {
        r.send(Command::Mixer(MixerCommand::Fader(DeckId::ALL[0], 0.0)));
        r.send(Command::Mixer(MixerCommand::Fader(DeckId::ALL[1], 1.0)));
        // Let deck 1's fader close first (the echo would catch its fade).
        r.render(BLOCK * 8);
    }
    fx.fx(FxCommand::UnitOn(0, true));
    assert_eq!(fx.render(SR as usize / 2), dry.render(SR as usize / 2));
    // Assign deck 2 as well: now it changes.
    fx.fx(FxCommand::Assign(0, 1, true));
    let a = fx.render(SR as usize);
    let b = dry.render(SR as usize);
    assert!(a.iter().zip(b.iter()).any(|(x, y)| (x - y).abs() > 1e-3));
}

#[test]
fn master_fx_filter_the_whole_mix() {
    let mut r = Rig::new();
    let before = rms(&r.render(SR as usize / 2));
    for s in 0..FX_SLOTS {
        r.fx(FxCommand::SlotOn(1, s, false));
    }
    for t in 0..MASTER_TARGET {
        r.fx(FxCommand::Assign(1, t, false));
    }
    r.fx(FxCommand::Assign(1, MASTER_TARGET, true));
    r.fx(FxCommand::Mode(1, FxMode::Single));
    r.fx(FxCommand::SlotKind(1, 0, EffectKind::Filter));
    // High-pass far up: the low test tones (about 150 and 230 Hz) vanish.
    r.fx(FxCommand::SlotParam(1, 0, 0, 1.0));
    r.fx(FxCommand::SlotParam(1, 0, 1, 1.0));
    r.fx(FxCommand::SlotOn(1, 0, true));
    r.fx(FxCommand::Mix(1, 0.5));
    r.fx(FxCommand::UnitOn(1, true));
    r.render(BLOCK * 8);
    let after = rms(&r.render(SR as usize / 2));
    assert!(after < before * 0.05, "{before} -> {after}");
}

#[test]
fn fx_follow_the_deck_tempo_or_a_tapped_one() {
    let mut r = Rig::new();
    r.send(Command::Deck(DeckId::ALL[0], DeckCommand::SetPitch(1.0)));
    r.render(BLOCK * 2);
    let bpm = r.h.snapshot().fx.bpm;
    // Deck 1 at +8 % of 120; deck 3 has nothing (fallback: the master's).
    assert!((bpm[0] - 129.6).abs() < 0.01, "{bpm:?}");
    assert!((bpm[1] - 120.0).abs() < 0.01, "{bpm:?}");
    r.fx(FxCommand::Tap(Some(100.0)));
    r.render(BLOCK * 2);
    assert!(
        r.h.snapshot()
            .fx
            .bpm
            .iter()
            .all(|b| (b - 100.0).abs() < 0.01)
    );
    r.fx(FxCommand::Tap(None));
    r.render(BLOCK * 2);
    assert!((r.h.snapshot().fx.bpm[0] - 129.6).abs() < 0.01);
}

#[test]
fn steps_stay_in_range_and_settings_carry_over() {
    let mut r = Rig::new();
    r.fx(FxCommand::BeatsStep(0, 100));
    r.fx(FxCommand::BeatsStep(1, -100));
    r.fx(FxCommand::SlotKindStep(0, 1, -1));
    r.fx(FxCommand::SlotOn(0, 1, true));
    r.fx(FxCommand::DepthStep(0, 5.0));
    r.fx(FxCommand::Mix(1, 0.15));
    r.fx(FxCommand::Mode(1, FxMode::Single));
    r.render(BLOCK);
    let p = r.h.snapshot().fx.params;
    assert_eq!(p.units[0].beats, BEATS.len() - 1);
    assert_eq!(p.units[1].beats, 0);
    // Slot 2 was Reverb; one step back is Ping-pong, with its defaults.
    assert_eq!(p.units[0].slots[1].kind, EffectKind::PingPong);
    assert_eq!(p.units[0].slots[1].params[0], 1.0, "depth clamped");
    assert_eq!(
        p.units[0].slots[0].params[0],
        EffectKind::Echo.defaults()[0],
        "slot off: unchanged"
    );

    let (engine, mut h) = Engine::new(EngineConfig::default());
    let mut r2 = OfflineRenderer::new(engine, 2, BLOCK);
    for c in p.commands() {
        h.send(Command::Fx(c)).unwrap();
    }
    r2.render_block();
    assert_eq!(h.update().fx.params, p);
}

#[test]
fn every_effect_runs_on_every_target() {
    let mut r = Rig::new();
    for u in 0..2 {
        for t in 0..=MASTER_TARGET {
            r.fx(FxCommand::Assign(u, t, true));
        }
        r.fx(FxCommand::UnitOn(u, true));
        r.fx(FxCommand::Mix(u, 0.5));
    }
    for (i, kind) in EffectKind::ALL.into_iter().enumerate() {
        let (u, s) = (i % 2, (i / 2) % FX_SLOTS);
        r.fx(FxCommand::SlotKind(u, s, kind));
        r.fx(FxCommand::SlotOn(u, s, true));
        let out = r.render(SR as usize / 4);
        assert!(
            out.iter().all(|x| x.is_finite() && x.abs() <= 1.0),
            "{kind:?}"
        );
        r.fx(FxCommand::SlotOn(u, s, false));
    }
}

#[test]
fn recording_gets_exactly_the_master_output_and_an_end_marker() {
    let mut r = Rig::new();
    let mut reader = r.h.take_record_reader().expect("reader");
    assert!(r.h.take_record_reader().is_none(), "given out once");
    r.send(Command::Record(true));
    let blocks = 40;
    let mut master = Vec::new();
    for _ in 0..blocks {
        master.extend_from_slice(r.r.render_block());
    }
    r.send(Command::Record(false));
    r.r.render_block();
    let snap = *r.h.update();
    assert!(!snap.recording);
    assert_eq!(snap.record_frames, (blocks * BLOCK) as u64);
    assert_eq!(snap.record_dropped, 0);
    let mut got = Vec::new();
    while let Ok(s) = reader.pop() {
        got.push(s);
    }
    assert_eq!(got.len(), blocks * BLOCK * 2 + 2);
    assert!(
        got[got.len() - 2].is_nan() && got[got.len() - 1].is_nan(),
        "end marker"
    );
    assert_eq!(&got[..got.len() - 2], &master[..]);
}

#[test]
fn recording_counts_what_the_writer_did_not_take() {
    let mut r = Rig::new();
    let _reader = r.h.take_record_reader().expect("reader");
    r.send(Command::Record(true));
    // Nobody reads: after the 4 s buffer, frames are dropped (no blocking).
    r.render(SR as usize * 5);
    let s = r.h.snapshot();
    assert!(s.recording);
    assert!(
        s.record_dropped > 0 && s.record_dropped < SR as u64 * 2,
        "{}",
        s.record_dropped
    );
}

/// Depth at zero: the effect is on but not heard, for every effect
/// (bit-identical to the same engine without FX).
#[test]
fn depth_zero_is_the_dry_signal_for_every_effect() {
    for kind in EffectKind::ALL {
        let mut fx = echo_rig();
        let mut dry = echo_rig();
        fx.fx(FxCommand::SlotKind(0, 0, kind));
        fx.fx(FxCommand::SlotParam(0, 0, 0, 0.0));
        fx.fx(FxCommand::UnitOn(0, true));
        let a = fx.render(SR as usize / 2);
        let b = dry.render(SR as usize / 2);
        assert_eq!(a, b, "{kind:?}");
    }
}

/// Depth turns an effect up from nothing: more depth, more difference.
#[test]
fn depth_sets_how_much_is_heard() {
    let diff = |depth: f32| {
        let mut fx = echo_rig();
        let mut dry = echo_rig();
        fx.fx(FxCommand::SlotKind(0, 0, EffectKind::Flanger));
        fx.fx(FxCommand::SlotParam(0, 0, 0, depth));
        fx.fx(FxCommand::UnitOn(0, true));
        let a = fx.render(SR as usize);
        let b = dry.render(SR as usize);
        let d: Vec<f32> = a.iter().zip(b.iter()).map(|(x, y)| x - y).collect();
        rms(&d)
    };
    let (low, high) = (diff(0.2), diff(1.0));
    assert!(low > 1e-4 && high > low * 2.0, "{low} {high}");
}

/// WET/DRY fully wet = only the effect. A reverb at full wet
/// leaves only its tail; switched off, the track comes back while the tail
/// rings out. The middle of WET/DRY is the track plus the full effect.
#[test]
fn fully_wet_takes_the_track_out_of_tail_effects() {
    let level = |mix: f32, on: bool| {
        let mut r = echo_rig();
        r.fx(FxCommand::SlotKind(0, 0, EffectKind::Reverb));
        r.fx(FxCommand::SlotParam(0, 0, 0, 0.0)); // depth 0: no reverb
        r.fx(FxCommand::Mix(0, mix));
        r.fx(FxCommand::SlotOn(0, 0, on));
        r.fx(FxCommand::UnitOn(0, true));
        r.render(SR as usize / 4);
        rms(&r.render(SR as usize / 4))
    };
    let dry = level(0.5, false);
    assert!(dry > 0.01);
    assert!(
        (level(0.5, true) - dry).abs() < 1e-6,
        "middle: track at full"
    );
    assert!(level(1.0, true) < dry * 1e-3, "fully wet: no track");
    assert!(
        (level(0.75, true) - dry * 0.5).abs() < dry * 0.02,
        "half way"
    );
    assert!(
        (level(1.0, false) - dry).abs() < 1e-6,
        "off: the track is back"
    );
}
