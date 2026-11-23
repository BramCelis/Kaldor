//! Four decks through the mixer, rendered offline. Levels are
//! measured per frequency (Goertzel) so each deck can be told apart.

use dj_audio::offline::{OfflineRenderer, channel, rms};
use dj_audio::{
    Command, DeckCommand, EngineConfig, EngineHandle, EqBand, MixerCommand, TrackAudio, XfaderSide,
};
use dj_core::{ChannelPair, DeckId, OutputRouting};
use dj_dsp::curves::{CrossfaderCurve, FaderCurve};
use std::f64::consts::PI;
use std::sync::Arc;

const SR: u32 = 48_000;
const FREQS: [f64; 4] = [200.0, 500.0, 1200.0, 3000.0];

struct Rig {
    r: OfflineRenderer,
    h: EngineHandle,
    channels: usize,
}

impl Rig {
    fn new(routing: OutputRouting, channels: usize) -> Self {
        let (engine, h) = dj_audio::Engine::new(EngineConfig {
            sample_rate: SR,
            max_block_frames: 256,
            routing,
        });
        Self {
            r: OfflineRenderer::new(engine, channels, 256),
            h,
            channels,
        }
    }

    fn ns4fx() -> Self {
        Self::new(OutputRouting::ns4fx(), 4)
    }

    fn mix(&mut self, m: MixerCommand) {
        self.h.send(Command::Mixer(m)).unwrap();
    }

    fn deck(&mut self, i: usize, c: DeckCommand) {
        self.h.send(Command::Deck(DeckId::ALL[i], c)).unwrap();
    }

    fn load_tone(&mut self, i: usize, freq: f64, amp: f64) {
        let n = SR as usize * 20;
        let mut s = Vec::with_capacity(n * 2);
        for k in 0..n {
            let v = (amp * (2.0 * PI * freq * k as f64 / f64::from(SR)).sin()) as f32;
            s.push(v);
            s.push(v);
        }
        let audio: Arc<TrackAudio> = TrackAudio::from_interleaved(SR, &s);
        self.deck(
            i,
            DeckCommand::Load {
                audio,
                serial: i as u64 + 1,
            },
        );
        self.deck(i, DeckCommand::Play(true));
    }

    /// Renders and returns the interleaved output.
    fn render(&mut self, frames: usize) -> Vec<f32> {
        let mut out = Vec::new();
        while out.len() < frames * self.channels {
            out.extend_from_slice(self.r.render_block());
            self.h.update();
        }
        out
    }

    /// Lets smoothers and filters settle, then renders `frames`.
    fn settle_and_render(&mut self, frames: usize) -> Vec<f32> {
        self.render(9600);
        self.render(frames)
    }
}

/// Amplitude of the `freq` component (Goertzel).
fn amp_at(s: &[f32], freq: f64) -> f64 {
    // Analyse a whole number of 100 ms windows: every test frequency is a
    // multiple of 10 Hz, so there is no spectral leakage between tones.
    let s = &s[..s.len() / 4800 * 4800];
    let w = 2.0 * PI * freq / f64::from(SR);
    let (mut re, mut im) = (0.0, 0.0);
    for (i, &v) in s.iter().enumerate() {
        re += f64::from(v) * (w * i as f64).cos();
        im += f64::from(v) * (w * i as f64).sin();
    }
    2.0 * (re * re + im * im).sqrt() / s.len() as f64
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol
}

#[test]
fn four_decks_sum_on_master() {
    let mut rig = Rig::ns4fx();
    for (i, f) in FREQS.iter().enumerate() {
        rig.load_tone(i, *f, 0.2);
    }
    let out = rig.settle_and_render(48_000);
    let master = channel(&out, 4, 0);
    for f in FREQS {
        let a = amp_at(&master, f);
        assert!(close(a, 0.2, 0.005), "{f} Hz: {a}");
    }
    // Nothing on the headphones without PFL.
    assert_eq!(rms(&channel(&out, 4, 2)), 0.0);
    let snap = rig.h.snapshot();
    for m in snap.channel_meters {
        assert!(close(f64::from(m.rms[0]), 0.2 / 2f64.sqrt(), 0.01), "{m:?}");
    }
}

#[test]
fn channel_fader_and_curves() {
    let mut rig = Rig::ns4fx();
    rig.load_tone(0, 500.0, 0.4);
    rig.mix(MixerCommand::FaderCurve(FaderCurve::Linear));
    rig.mix(MixerCommand::Fader(DeckId::ALL[0], 0.5));
    let a = amp_at(&channel(&rig.settle_and_render(24_000), 4, 0), 500.0);
    assert!(close(a, 0.2, 0.004), "linear half: {a}");
    rig.mix(MixerCommand::FaderCurve(FaderCurve::Smooth));
    let a = amp_at(&channel(&rig.settle_and_render(24_000), 4, 0), 500.0);
    // x^2.2 power taper.
    assert!(close(a, 0.4 * 0.5f64.powf(2.2), 0.002), "smooth half: {a}");
    rig.mix(MixerCommand::FaderCurve(FaderCurve::Club));
    let a = amp_at(&channel(&rig.settle_and_render(24_000), 4, 0), 500.0);
    // Club taper: -60 dB to -6 dB linear in dB over the first 80 %.
    let club = 0.4 * 10f64.powf((-60.0 + 54.0 * 0.5 / 0.8) / 20.0);
    assert!(close(a, club, 0.001), "club half: {a} vs {club}");
    rig.mix(MixerCommand::Fader(DeckId::ALL[0], 0.0));
    assert_eq!(rms(&channel(&rig.settle_and_render(4_800), 4, 0)), 0.0);
}

#[test]
fn crossfader_assign_curve_and_reverse() {
    let mut rig = Rig::ns4fx();
    rig.load_tone(0, 200.0, 0.4);
    rig.load_tone(1, 3000.0, 0.4);
    rig.mix(MixerCommand::XfaderAssign(DeckId::ALL[0], XfaderSide::A));
    rig.mix(MixerCommand::XfaderAssign(DeckId::ALL[1], XfaderSide::B));

    rig.mix(MixerCommand::Crossfader(-1.0));
    let m = channel(&rig.settle_and_render(24_000), 4, 0);
    assert!(close(amp_at(&m, 200.0), 0.4, 0.005));
    assert!(amp_at(&m, 3000.0) < 1e-4);

    rig.mix(MixerCommand::Crossfader(0.0));
    let m = channel(&rig.settle_and_render(24_000), 4, 0);
    let half_power = 0.4 / 2f64.sqrt();
    assert!(close(amp_at(&m, 200.0), half_power, 0.005));
    assert!(close(amp_at(&m, 3000.0), half_power, 0.005));

    rig.mix(MixerCommand::CrossfaderCurve(CrossfaderCurve::Cut));
    rig.mix(MixerCommand::Crossfader(0.9));
    let m = channel(&rig.settle_and_render(24_000), 4, 0);
    assert!(close(amp_at(&m, 200.0), 0.4, 0.005), "cut keeps A full");

    rig.mix(MixerCommand::Crossfader(1.0));
    rig.mix(MixerCommand::CrossfaderReverse(true));
    let m = channel(&rig.settle_and_render(24_000), 4, 0);
    assert!(
        close(amp_at(&m, 200.0), 0.4, 0.005),
        "reversed: B end plays A"
    );
    assert!(amp_at(&m, 3000.0) < 1e-4);

    // A channel on "thru" ignores the crossfader.
    rig.mix(MixerCommand::XfaderAssign(DeckId::ALL[1], XfaderSide::Thru));
    let m = channel(&rig.settle_and_render(24_000), 4, 0);
    assert!(close(amp_at(&m, 3000.0), 0.4, 0.005));
}

#[test]
fn eq_kill_boost_and_trim() {
    let mut rig = Rig::ns4fx();
    let d = DeckId::ALL[0];
    // Low (60 Hz), mid (1 kHz) and high (10 kHz) on one deck.
    let n = SR as usize * 20;
    let mut s = Vec::with_capacity(n * 2);
    for k in 0..n {
        let t = k as f64 / f64::from(SR);
        let v = 0.1
            * ((2.0 * PI * 60.0 * t).sin()
                + (2.0 * PI * 1000.0 * t).sin()
                + (2.0 * PI * 10_000.0 * t).sin());
        s.push(v as f32);
        s.push(v as f32);
    }
    rig.deck(
        0,
        DeckCommand::Load {
            audio: TrackAudio::from_interleaved(SR, &s),
            serial: 1,
        },
    );
    rig.deck(0, DeckCommand::Play(true));

    rig.mix(MixerCommand::Kill(d, EqBand::Low, true));
    let m = channel(&rig.settle_and_render(48_000), 4, 0);
    // 12 dB/oct crossovers (150 Hz): a killed low band leaves x^2 / (1 + x^2)
    // of a 60 Hz tone (x = 0.4, about -17 dB), as the measured reference.
    assert!(
        close(amp_at(&m, 60.0), 0.1 * 0.138, 0.002),
        "low killed: {}",
        amp_at(&m, 60.0)
    );
    assert!(close(amp_at(&m, 1000.0), 0.1, 0.01));
    assert!(close(amp_at(&m, 10_000.0), 0.1, 0.01));

    rig.mix(MixerCommand::Kill(d, EqBand::Low, false));
    rig.mix(MixerCommand::Eq(d, EqBand::High, 1.0)); // +6 dB
    let m = channel(&rig.settle_and_render(48_000), 4, 0);
    // +6 dB on the high band reaches (1 + g x^2) / (1 + x^2) at 10 kHz
    // (x = 2.67 after prewarping, g = 2): 1.87, not quite the full 2.
    assert!(
        close(amp_at(&m, 10_000.0), 0.187, 0.006),
        "{}",
        amp_at(&m, 10_000.0)
    );
    assert!(close(amp_at(&m, 60.0), 0.1, 0.01));

    rig.mix(MixerCommand::Eq(d, EqBand::High, 0.5));
    rig.mix(MixerCommand::Trim(d, 0.5)); // +6 dB
    let m = channel(&rig.settle_and_render(48_000), 4, 0);
    assert!(close(amp_at(&m, 1000.0), 0.1995, 0.01));

    // 12 dB EQ boost setting.
    rig.mix(MixerCommand::Trim(d, 0.0));
    rig.mix(MixerCommand::EqBoost(12.0));
    rig.mix(MixerCommand::Eq(d, EqBand::High, 1.0));
    let m = channel(&rig.settle_and_render(48_000), 4, 0);
    assert!(
        close(
            amp_at(&m, 10_000.0),
            0.1 * (1.0 + 3.98 * 7.12) / 8.12,
            0.015
        ),
        "{}",
        amp_at(&m, 10_000.0)
    );

    // Classic EQ: shelves and bell, -26 dB fully left, kill much deeper.
    rig.mix(MixerCommand::EqBoost(6.0));
    rig.mix(MixerCommand::Eq(d, EqBand::High, 0.5));
    rig.mix(MixerCommand::EqMode(dj_dsp::curves::EqMode::Classic));
    rig.mix(MixerCommand::Eq(d, EqBand::Low, 0.0));
    let m = channel(&rig.settle_and_render(48_000), 4, 0);
    let low = amp_at(&m, 60.0);
    assert!(
        low < 0.1 * 0.07 && low > 0.1 * 0.03,
        "classic -26 dB: {low}"
    );
    assert!(close(amp_at(&m, 10_000.0), 0.1, 0.01));
    rig.mix(MixerCommand::Kill(d, EqBand::Low, true));
    let m = channel(&rig.settle_and_render(48_000), 4, 0);
    assert!(amp_at(&m, 60.0) < 0.1 * 0.01, "classic kill");
    // Back to the isolator, flat: the channel is the dry signal again.
    rig.mix(MixerCommand::Kill(d, EqBand::Low, false));
    rig.mix(MixerCommand::Eq(d, EqBand::Low, 0.5));
    rig.mix(MixerCommand::EqMode(dj_dsp::curves::EqMode::Isolator));
    let m = channel(&rig.settle_and_render(48_000), 4, 0);
    assert!(close(amp_at(&m, 60.0), 0.1, 0.005));
}

#[test]
fn filter_slope_setting() {
    // Two octaves above the cutoff the 24 dB/oct filter is far quieter.
    let level = |slope| {
        let mut rig = Rig::ns4fx();
        rig.mix(MixerCommand::FilterShape(slope, 0.0));
        rig.load_tone(0, 4.0 * dj_dsp::svf::DjFilter::cutoff_for(true, 0.6), 0.4);
        rig.mix(MixerCommand::Filter(DeckId::ALL[0], -0.6));
        let m = channel(&rig.settle_and_render(24_000), 4, 0);
        amp_at(&m, 4.0 * dj_dsp::svf::DjFilter::cutoff_for(true, 0.6))
    };
    let a12 = level(dj_dsp::svf::FilterSlope::Db12);
    let a24 = level(dj_dsp::svf::FilterSlope::Db24);
    assert!(a12 < 0.4 * 0.1 && a12 > 0.4 * 0.03, "12 dB: {a12}");
    assert!(a24 < 0.4 * 0.012, "24 dB: {a24}");
}

#[test]
fn filter_knob_on_a_channel() {
    let mut rig = Rig::ns4fx();
    rig.load_tone(0, 5000.0, 0.4);
    rig.mix(MixerCommand::Filter(DeckId::ALL[0], -0.8));
    let m = channel(&rig.settle_and_render(24_000), 4, 0);
    assert!(amp_at(&m, 5000.0) < 0.02);
    rig.mix(MixerCommand::Filter(DeckId::ALL[0], 0.0));
    let m = channel(&rig.settle_and_render(24_000), 4, 0);
    assert!(close(amp_at(&m, 5000.0), 0.4, 0.005));
}

/// Color FX on the filter knob, DJM style. With one
/// chosen the knob plays it (middle: the dry track); back to FILTER the
/// knob filters again.
#[test]
fn color_fx_play_on_the_filter_knob() {
    use dj_audio::ColorFx;
    let mut rig = Rig::ns4fx();
    let d = DeckId::ALL[0];
    rig.load_tone(0, 500.0, 0.4);
    for color in ColorFx::ALL.into_iter().skip(1) {
        rig.mix(MixerCommand::ColorFx(color));
        rig.mix(MixerCommand::Filter(d, 0.0));
        let m = channel(&rig.settle_and_render(24_000), 4, 0);
        // Echo and reverb tails of the effect before may still ring: only
        // the tone itself counts.
        assert!(
            close(amp_at(&m, 500.0), 0.4, 0.02),
            "{color:?}: middle is dry"
        );
        assert_eq!(rig.h.snapshot().mixer.color_fx, color);
    }
    // Turned either way, each color FX changes the channel: two identical
    // engines (the offline render is deterministic), one with the knob in
    // the middle, one turned; their outputs differ.
    for color in ColorFx::ALL.into_iter().skip(1) {
        for knob in [-0.7, 0.7] {
            if color == ColorFx::Sweep && knob < 0.0 {
                // The gate leaves steady tones alone (nothing to tighten);
                // dj-dsp's color_fx tests check it on drum-like hits.
                continue;
            }
            let render = |k: f32| {
                let mut rig = Rig::ns4fx();
                rig.load_tone(0, 500.0, 0.2);
                rig.load_tone(1, 3000.0, 0.2);
                rig.mix(MixerCommand::ColorFx(color));
                rig.mix(MixerCommand::Filter(d, k));
                rig.mix(MixerCommand::Filter(DeckId::ALL[1], k));
                channel(&rig.settle_and_render(24_000), 4, 0)
            };
            let (dry, wet) = (render(0.0), render(knob));
            let diff: Vec<f32> = wet.iter().zip(dry.iter()).map(|(a, b)| a - b).collect();
            assert!(rms(&diff) > 0.005, "{color:?} {knob}: {}", rms(&diff));
            assert!(rms(&wet) < rms(&dry) * 1.5, "{color:?} {knob}: too loud");
        }
    }
    // FILTER again: the knob is the low-pass / high-pass filter.
    rig.mix(MixerCommand::ColorFx(ColorFx::Filter));
    rig.mix(MixerCommand::Filter(d, 0.8));
    let m = channel(&rig.settle_and_render(24_000), 4, 0);
    assert!(amp_at(&m, 500.0) < 0.02, "high-passed");
}

/// Noise turned left silenced the whole track. Through the
/// whole engine (mixer, master, limiter), turned either way, the track's
/// tone stays at its level.
#[test]
fn noise_color_fx_keeps_the_track() {
    use dj_audio::ColorFx;
    for knob in [-0.3, -0.6, -0.95, 0.3, 0.6, 0.95] {
        let mut rig = Rig::ns4fx();
        let d = DeckId::ALL[0];
        rig.load_tone(0, 1200.0, 0.4);
        rig.mix(MixerCommand::ColorFx(ColorFx::Noise));
        rig.mix(MixerCommand::Filter(d, knob));
        let m = channel(&rig.settle_and_render(24_000), 4, 0);
        let tone = amp_at(&m, 1200.0);
        assert!(close(tone, 0.4, 0.04), "knob {knob}: tone {tone}");
    }
}

/// Occasional clicks. A hot track on the headphones
/// (pre-fader, EQ boost, headphone gain +6 dB) and the booth turned up are
/// limited like the master, not hard-clipped: no sample is cut, the level
/// stays at the ceiling. With the master limiter switched off and the
/// master pushed, the cut samples are counted.
#[test]
fn headphones_and_booth_are_limited_and_clipping_is_counted() {
    let mut rig = Rig::ns4fx();
    let d = DeckId::ALL[0];
    rig.load_tone(0, 200.0, 0.9);
    rig.mix(MixerCommand::Eq(d, EqBand::Low, 1.0)); // +6 dB on the tone
    rig.mix(MixerCommand::Pfl(d, true));
    rig.mix(MixerCommand::CueGain(6.0));
    rig.mix(MixerCommand::BoothGain(12.0));
    let out = rig.settle_and_render(24_000);
    let phones = channel(&out, 4, 2);
    let peak = phones.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(
        peak <= 0.97 && peak > 0.8,
        "headphones at the ceiling: {peak}"
    );
    let s = rig.h.snapshot().clipped;
    assert_eq!(s, [0, 0, 0], "nothing cut");
    // The master limiter off and the master pushed: counted.
    rig.mix(MixerCommand::Limiter(false));
    rig.mix(MixerCommand::MasterGain(12.0));
    rig.settle_and_render(24_000);
    assert!(rig.h.snapshot().clipped[0] > 0, "master clips counted");
}

#[test]
fn headphone_cue_is_pre_fader_with_mix_and_gain() {
    let mut rig = Rig::ns4fx();
    let d = DeckId::ALL[0];
    rig.load_tone(0, 500.0, 0.4);
    rig.load_tone(1, 1200.0, 0.4);
    rig.mix(MixerCommand::Pfl(d, true));
    rig.mix(MixerCommand::Fader(d, 0.0));
    let out = rig.settle_and_render(24_000);
    let master = channel(&out, 4, 0);
    let phones = channel(&out, 4, 2);
    assert!(amp_at(&master, 500.0) < 1e-4, "fader down: not on master");
    assert!(
        close(amp_at(&phones, 500.0), 0.4, 0.005),
        "PFL ignores the fader"
    );
    assert!(amp_at(&phones, 1200.0) < 1e-4, "deck 2 has no PFL");

    // Mix fully to master: headphones hear deck 2 (on master), not deck 1.
    rig.mix(MixerCommand::CueMix(1.0));
    let phones = channel(&rig.settle_and_render(24_000), 4, 2);
    assert!(amp_at(&phones, 500.0) < 1e-4);
    assert!(close(amp_at(&phones, 1200.0), 0.4, 0.005));

    rig.mix(MixerCommand::CueMix(0.0));
    rig.mix(MixerCommand::CueGain(-6.0206));
    let phones = channel(&rig.settle_and_render(24_000), 4, 2);
    assert!(close(amp_at(&phones, 500.0), 0.2, 0.004));
}

#[test]
fn limiter_holds_the_ceiling() {
    let mut rig = Rig::ns4fx();
    for i in 0..4 {
        rig.load_tone(i, 100.0, 0.6); // in phase: sum 2.4
    }
    let out = rig.settle_and_render(48_000);
    let master = channel(&out, 4, 0);
    let peak = master.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let ceiling = 10f32.powf(-0.3 / 20.0);
    assert!(peak <= ceiling + 1e-6, "peak {peak}");
    assert!(peak > 0.85, "still loud: {peak}");
    // The soft clipper rounds the peaks off near 0 dBFS; the brickwall
    // stage trims what is left.
    assert!(rig.h.snapshot().limiter_gain < 1.0);
}

#[test]
fn master_gain_and_booth() {
    let mut routing = OutputRouting::ns4fx();
    routing.booth = Some(ChannelPair::new(4, 5));
    let mut rig = Rig::new(routing, 6);
    rig.load_tone(0, 500.0, 0.4);
    rig.mix(MixerCommand::MasterGain(-6.0206));
    rig.mix(MixerCommand::BoothGain(-6.0206));
    let out = rig.settle_and_render(24_000);
    assert!(close(amp_at(&channel(&out, 6, 0), 500.0), 0.2, 0.004));
    assert!(close(amp_at(&channel(&out, 6, 4), 500.0), 0.1, 0.003));
}

#[test]
fn fader_jumps_do_not_click() {
    let mut rig = Rig::ns4fx();
    rig.load_tone(0, 200.0, 0.4);
    rig.render(4_800);
    let mut m = Vec::new();
    for v in [0.0, 1.0, 0.3, 1.0] {
        rig.mix(MixerCommand::Fader(DeckId::ALL[0], v));
        m.extend(channel(&rig.render(4_800), 4, 0));
    }
    rig.mix(MixerCommand::Kill(DeckId::ALL[0], EqBand::Low, true));
    m.extend(channel(&rig.render(4_800), 4, 0));
    let step = m
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    // The tone's own maximum step is about 0.0105.
    assert!(step < 0.02, "max step {step}");
}

#[test]
fn meter_peaks_survive_between_ui_reads() {
    // One short click at the start, then silence. The UI reads snapshots
    // only every few blocks; the click's peak must still reach it.
    let mut rig = Rig::ns4fx();
    let mut s = vec![0.0f32; SR as usize * 2];
    // Interleaved stereo: frames 400..440, after the 4 ms fade-in.
    for v in s.iter_mut().skip(800).take(80) {
        *v = 0.5;
    }
    rig.deck(
        0,
        DeckCommand::Load {
            audio: TrackAudio::from_interleaved(SR, &s),
            serial: 1,
        },
    );
    rig.deck(0, DeckCommand::Play(true));
    for _ in 0..10 {
        rig.r.render_block();
    }
    let snap = rig.h.update();
    assert!(snap.master_meter.peak[0] > 0.4, "{:?}", snap.master_meter);
    assert!(snap.channel_meters[0].peak[0] > 0.4);
}

/// `MixerParams::commands` brings a fresh engine to the same settings (used
/// when the audio device changes and a new engine is built).
#[test]
fn mixer_params_round_trip_through_commands() {
    let mut a = Rig::ns4fx();
    let d = DeckId::ALL;
    for m in [
        MixerCommand::Trim(d[0], 0.3),
        MixerCommand::Eq(d[1], EqBand::Low, 0.1),
        MixerCommand::Eq(d[2], EqBand::High, 0.9),
        MixerCommand::Kill(d[3], EqBand::Mid, true),
        MixerCommand::Filter(d[0], -0.4),
        MixerCommand::Fader(d[1], 0.25),
        MixerCommand::XfaderAssign(d[2], XfaderSide::B),
        MixerCommand::Pfl(d[3], true),
        MixerCommand::AutoGain(d[0], -3.5),
        MixerCommand::Crossfader(0.6),
        MixerCommand::CrossfaderCurve(CrossfaderCurve::Cut),
        MixerCommand::CrossfaderReverse(true),
        MixerCommand::FaderCurve(FaderCurve::Sharp),
        MixerCommand::CueGain(-6.0),
        MixerCommand::CueMix(0.3),
        MixerCommand::MasterGain(-2.0),
        MixerCommand::BoothGain(-9.0),
        MixerCommand::Limiter(false),
        MixerCommand::EqCrossovers(150.0, 3000.0),
    ] {
        a.mix(m);
    }
    a.r.render_block();
    let want = a.h.update().mixer;
    assert_ne!(want, dj_audio::MixerParams::default());

    let mut b = Rig::ns4fx();
    for m in want.commands() {
        b.mix(m);
    }
    b.r.render_block();
    assert_eq!(b.h.update().mixer, want);
}
