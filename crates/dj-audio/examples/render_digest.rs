//! Prints a hash of the engine's exact output for three scripted scenarios,
//! so a change that must not alter the sound can be checked bit for bit:
//! run it before and after the change, with the same build profile, and
//! compare the lines.
//!
//! ```text
//! cargo run --release -p dj-audio --example render_digest > target\digest-before.txt
//! (make the change)
//! cargo run --release -p dj-audio --example render_digest > target\digest-after.txt
//! ```
//!
//! Each line is an FNV-1a hash of the bits of every output sample and of the
//! recording stream. The scenarios run on the offline renderer:
//! - `mixer`: EQ (both modes), filter (both slopes), faders, trim, curves,
//!   crossfader, key lock, key shift, scratch, bend and the limiter;
//! - `perf_fx`: the bench's `--perf` and `--fx` loads (grids, quantize,
//!   sync, loops, roll, slip, both FX units, recording) with all 20 effects
//!   in turn;
//! - `sampler2`: the sampler in every mode at 44.1 kHz with 97-frame blocks.
//!
//! The hashes are only comparable on the same machine and toolchain: another
//! OS, CPU or compiler may round differently. That is why this is a tool and
//! not a test with stored values.

use dj_analysis::synth::ClickTrack;
use dj_audio::{
    Command, DeckCommand, DeckGrid, EffectKind, Engine, EngineConfig, EqBand, FxCommand,
    MASTER_TARGET, MixerCommand, SampleMode, SamplerCommand, TrackAudio, XfaderSide,
};
use dj_core::{DeckId, OutputRouting};
use dj_dsp::curves::{CrossfaderCurve, EqMode};
use dj_dsp::svf::FilterSlope;
use std::sync::Arc;

struct Fnv(u64);
impl Fnv {
    fn add(&mut self, x: u32) {
        for b in x.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
}

type Script<'a> = &'a dyn Fn(usize, usize, &mut dyn FnMut(Command));

fn run(name: &str, rate: u32, block: usize, seconds: f64, script: Script<'_>) {
    let track: Arc<TrackAudio> = ClickTrack {
        sample_rate: rate,
        noise: 0.05,
        ..ClickTrack::new(126.0, seconds + 5.0)
    }
    .to_track();
    let (mut engine, mut h) = Engine::new(EngineConfig {
        sample_rate: rate,
        max_block_frames: 512,
        routing: OutputRouting::ns4fx(),
    });
    let mut reader = h.take_record_reader();
    let channels = 4;
    let mut buf = vec![0.0f32; block * channels];
    let blocks = (seconds * f64::from(rate)) as usize / block;
    let mut out = Fnv(0xcbf2_9ce4_8422_2325);
    let mut rec = Fnv(0xcbf2_9ce4_8422_2325);
    let mut rec_n = 0usize;
    {
        let mut send = |c: Command| {
            let _ = h.send(c);
        };
        for (i, d) in DeckId::ALL.iter().enumerate() {
            send(Command::Deck(
                *d,
                DeckCommand::Load {
                    audio: Arc::clone(&track),
                    serial: 1,
                },
            ));
            send(Command::Deck(*d, DeckCommand::SetPitch(0.03 * i as f64)));
            send(Command::Deck(*d, DeckCommand::Play(true)));
            send(Command::Mixer(MixerCommand::XfaderAssign(
                *d,
                if i % 2 == 0 {
                    XfaderSide::A
                } else {
                    XfaderSide::B
                },
            )));
        }
    }
    for b in 0..blocks {
        {
            let mut send = |c: Command| {
                let _ = h.send(c);
            };
            script(b, blocks, &mut send);
        }
        engine.process(&mut buf, channels);
        for s in &buf {
            out.add(s.to_bits());
        }
        if b % 32 == 0 {
            h.update();
        }
        if let Some(r) = reader.as_mut() {
            while let Ok(s) = r.pop() {
                rec.add(s.to_bits());
                rec_n += 1;
            }
        }
    }
    println!(
        "{name:<10} out {:016x}  rec {:016x} ({rec_n} samples)",
        out.0, rec.0
    );
}

fn main() {
    let d = DeckId::ALL;
    // Mixer, EQ (both modes), filter (both slopes), faders, curves, key
    // lock, scratch, bend, brake.
    run("mixer", 48_000, 256, 30.0, &|b, n, send| {
        if b == 0 {
            send(Command::Deck(d[0], DeckCommand::SetKeyLock(true)));
            send(Command::Deck(d[1], DeckCommand::SetKeyLock(true)));
            send(Command::Mixer(MixerCommand::Pfl(d[0], true)));
            send(Command::Mixer(MixerCommand::CueMix(0.4)));
        }
        let t = b as f32 / n as f32;
        if b % 8 == 0 {
            for (i, dk) in d.iter().enumerate() {
                let ph = (t * 7.0 + i as f32 * 0.25).fract();
                send(Command::Mixer(MixerCommand::Eq(*dk, EqBand::Low, ph)));
                send(Command::Mixer(MixerCommand::Eq(*dk, EqBand::Mid, 1.0 - ph)));
                send(Command::Mixer(MixerCommand::Eq(
                    *dk,
                    EqBand::High,
                    (ph * 2.0).fract(),
                )));
                send(Command::Mixer(MixerCommand::Filter(*dk, ph * 2.0 - 1.0)));
                send(Command::Mixer(MixerCommand::Fader(*dk, 0.5 + 0.5 * ph)));
                send(Command::Mixer(MixerCommand::Trim(*dk, ph - 0.5)));
            }
            send(Command::Mixer(MixerCommand::Crossfader(
                (t * 4.0).fract() * 2.0 - 1.0,
            )));
        }
        match b * 10 / n.max(1) {
            2 if b * 10 % n < 10 => {
                send(Command::Mixer(MixerCommand::EqMode(EqMode::Classic)));
                send(Command::Mixer(MixerCommand::FilterShape(
                    FilterSlope::Db12,
                    0.7,
                )));
                send(Command::Mixer(MixerCommand::CrossfaderCurve(
                    CrossfaderCurve::Dip,
                )));
                send(Command::Mixer(MixerCommand::Kill(d[2], EqBand::Low, true)));
            }
            4 if b * 10 % n < 10 => {
                send(Command::Mixer(MixerCommand::EqMode(EqMode::Isolator)));
                send(Command::Mixer(MixerCommand::FilterShape(
                    FilterSlope::Db24,
                    0.2,
                )));
                send(Command::Mixer(MixerCommand::MasterGain(4.0)));
                send(Command::Mixer(MixerCommand::Kill(d[2], EqBand::Low, false)));
                send(Command::Deck(d[3], DeckCommand::JogTouch(true)));
            }
            5 => send(Command::Deck(
                d[3],
                DeckCommand::JogScratch(if b % 40 < 20 { 0.01 } else { -0.013 }),
            )),
            6 if b * 10 % n < 10 => {
                send(Command::Deck(d[3], DeckCommand::JogTouch(false)));
                send(Command::Deck(d[2], DeckCommand::PitchBend(0.04)));
                send(Command::Deck(d[1], DeckCommand::Play(false)));
                send(Command::Deck(d[0], DeckCommand::KeyShift(-3)));
            }
            8 if b * 10 % n < 10 => {
                send(Command::Deck(d[1], DeckCommand::Play(true)));
                send(Command::Deck(d[2], DeckCommand::PitchBend(0.0)));
                send(Command::Mixer(MixerCommand::Limiter(false)));
            }
            _ => {}
        }
    });

    // The bench's --perf and --fx loads, plus every effect in turn.
    run("perf_fx", 48_000, 256, 30.0, &|b, n, send| {
        if b == 0 {
            let grid = DeckGrid {
                first_beat: 0.0,
                beat_len: 60.0 * 48_000.0 / 126.0,
                first_beat_in_bar: 1,
            };
            send(Command::Quantize(true));
            for (i, dk) in d.iter().enumerate() {
                send(Command::Deck(*dk, DeckCommand::SetGrid(Some(grid))));
                if i > 0 {
                    send(Command::Sync(*dk, true));
                }
            }
            send(Command::Deck(d[0], DeckCommand::AutoLoop(1.0)));
            send(Command::Deck(
                d[1],
                DeckCommand::Roll {
                    beats: 0.25,
                    pressed: true,
                },
            ));
            send(Command::Deck(d[2], DeckCommand::SetSlip(true)));
            send(Command::Deck(d[2], DeckCommand::AutoLoop(2.0)));
            send(Command::Deck(d[3], DeckCommand::KeyShift(2)));
            send(Command::Deck(d[3], DeckCommand::SetKeyLock(true)));
            let units = [
                (
                    [EffectKind::Echo, EffectKind::Reverb, EffectKind::Flanger],
                    vec![0, 2],
                ),
                (
                    [
                        EffectKind::PingPong,
                        EffectKind::Phaser,
                        EffectKind::PitchShifter,
                    ],
                    vec![1, 3, MASTER_TARGET],
                ),
            ];
            for (u, (kinds, targets)) in units.into_iter().enumerate() {
                for t in 0..=MASTER_TARGET {
                    send(Command::Fx(FxCommand::Assign(u, t, targets.contains(&t))));
                }
                for (s, k) in kinds.into_iter().enumerate() {
                    send(Command::Fx(FxCommand::SlotKind(u, s, k)));
                    send(Command::Fx(FxCommand::SlotOn(u, s, true)));
                }
                send(Command::Fx(FxCommand::UnitOn(u, true)));
            }
            send(Command::Record(true));
        }
        let step = n / 21;
        if step > 0 && b % step == 0 && b / step < 20 {
            let k = EffectKind::ALL[b / step];
            send(Command::Fx(FxCommand::SlotKind(0, 0, k)));
            send(Command::Fx(FxCommand::Mix(
                0,
                0.3 + 0.03 * (b / step) as f32,
            )));
            send(Command::Fx(FxCommand::Beats(1, (b / step) % 6)));
            send(Command::Fx(FxCommand::SlotParam(
                1,
                1,
                0,
                ((b / step) as f32 * 0.17).fract(),
            )));
        }
        if b == n / 2 {
            send(Command::Deck(
                d[1],
                DeckCommand::Roll {
                    beats: 0.25,
                    pressed: false,
                },
            ));
            send(Command::Deck(d[2], DeckCommand::SetSlip(false)));
            send(Command::Deck(d[0], DeckCommand::BeatJump(4.0)));
            send(Command::Deck(d[0], DeckCommand::LoopHalve));
        }
    });

    // Sampler in every mode, odd block size and rate, all decks key-locked.
    sampler_run();
}

fn sampler_run() {
    let rate = 44_100;
    let track: Arc<TrackAudio> = ClickTrack {
        sample_rate: rate,
        noise: 0.05,
        ..ClickTrack::new(120.0, 10.0)
    }
    .to_track();
    run("sampler2", rate, 97, 20.0, &move |b, n, send| {
        if b == 0 {
            for dk in DeckId::ALL {
                send(Command::Deck(dk, DeckCommand::SetKeyLock(true)));
            }
            let modes = [
                SampleMode::OneShot,
                SampleMode::Hold,
                SampleMode::Toggle,
                SampleMode::Loop,
            ];
            for slot in 0..8u8 {
                send(Command::Sampler(SamplerCommand::Load {
                    slot,
                    audio: Arc::clone(&track),
                    bpm: Some(120.0),
                }));
                send(Command::Sampler(SamplerCommand::SetMode(
                    slot,
                    modes[slot as usize % 4],
                )));
                send(Command::Sampler(SamplerCommand::SetSync(
                    slot,
                    slot % 2 == 0,
                )));
            }
        }
        let step = n / 16;
        if step > 0 && b % step == 0 {
            let slot = ((b / step) % 8) as u8;
            send(Command::Sampler(SamplerCommand::Trigger {
                slot,
                pressed: (b / step) % 2 == 0,
            }));
        }
    });
}
