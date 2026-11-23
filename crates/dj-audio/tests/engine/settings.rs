//! Engine behaviour behind the settings window: stop brake,
//! stutter cue, start position on load, the WSOLA key lock, the automatic
//! sync master, the limiter ceiling and the headphone cue before the EQ.

use dj_audio::offline::OfflineRenderer;
use dj_audio::{
    Command, DeckCommand, DeckGrid, DeckSnapshot, Engine, EngineConfig, EngineHandle, EqBand,
    MixerCommand, TrackAudio, TrackMarkers,
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
    channels: usize,
}

impl Rig {
    fn new(routing: OutputRouting, channels: usize) -> Self {
        let (engine, h) = Engine::new(EngineConfig {
            sample_rate: SR,
            max_block_frames: BLOCK,
            routing,
        });
        Self {
            r: OfflineRenderer::new(engine, channels, BLOCK),
            h,
            channels,
        }
    }

    fn laptop() -> Self {
        Self::new(OutputRouting::laptop(), 2)
    }

    fn send(&mut self, c: Command) {
        self.h.send(c).unwrap();
    }

    fn deck(&mut self, d: usize, c: DeckCommand) {
        self.send(Command::Deck(DeckId::ALL[d], c));
    }

    /// Renders whole blocks covering `frames`; returns channel `ch`.
    fn render_ch(&mut self, frames: usize, ch: usize) -> Vec<f32> {
        let mut out = Vec::new();
        while out.len() < frames {
            let block = self.r.render_block();
            out.extend(block.chunks(self.channels).map(|f| f[ch]));
            self.h.update();
        }
        out
    }

    fn render(&mut self, frames: usize) -> Vec<f32> {
        self.render_ch(frames, 0)
    }

    fn snap(&self, d: usize) -> DeckSnapshot {
        self.h.snapshot().decks[d]
    }

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

fn sine_track(freq: f64, seconds: f64, amp: f64) -> Arc<TrackAudio> {
    let n = (seconds * f64::from(SR)) as usize;
    let mut s = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = ((2.0 * PI * freq * i as f64 / f64::from(SR)).sin() * amp) as f32;
        s.push(v);
        s.push(v);
    }
    TrackAudio::from_interleaved(SR, &s)
}

fn freq(s: &[f32]) -> f64 {
    let c = s.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    c as f64 * f64::from(SR) / s.len() as f64
}

fn peak(s: &[f32]) -> f32 {
    s.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

#[test]
fn stop_brake_slows_the_deck_down_to_a_stop() {
    let mut rig = Rig::laptop();
    let t = sine_track(200.0, 20.0, 0.4);
    rig.load(0, &t, None);
    rig.deck(0, DeckCommand::SetStopBrake(1.0));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(SR as usize / 2);
    let at = rig.snap(0).position;
    rig.deck(0, DeckCommand::Play(false));
    // Halfway through the brake the pitch has fallen (about 100 Hz).
    rig.render(SR as usize / 4);
    let mid = rig.render(SR as usize / 4);
    let f = freq(&mid);
    assert!(f > 40.0 && f < 160.0, "pitch during the brake: {f:.0} Hz");
    assert!(!rig.snap(0).playing, "shows paused while braking");
    rig.render(SR as usize);
    let s = rig.snap(0);
    // A linear brake over 1 s covers half a second of audio.
    let moved = (s.position - at) / f64::from(SR);
    assert!((0.4..0.62).contains(&moved), "ran on {moved:.2} s");
    let tail = rig.render(SR as usize / 4);
    assert!(peak(&tail) < 1e-3, "silent after the stop");
    let still = rig.snap(0).position;
    rig.render(SR as usize / 4);
    assert_eq!(rig.snap(0).position, still, "stays where it stopped");
}

#[test]
fn play_during_the_brake_plays_on_and_no_brake_stops_at_once() {
    let mut rig = Rig::laptop();
    let t = sine_track(200.0, 20.0, 0.4);
    rig.load(0, &t, None);
    rig.deck(0, DeckCommand::SetStopBrake(2.0));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(SR as usize / 4);
    rig.deck(0, DeckCommand::Play(false));
    rig.render(SR as usize / 4);
    rig.deck(0, DeckCommand::Play(true));
    rig.render(SR as usize / 4);
    let a = rig.snap(0);
    assert!(a.playing);
    let f = freq(&rig.render(SR as usize / 4));
    assert!((f - 200.0).abs() < 5.0, "back at play speed: {f:.0} Hz");
    // Brake off: the usual short stop.
    rig.deck(0, DeckCommand::SetStopBrake(0.0));
    rig.deck(0, DeckCommand::Play(false));
    rig.render(SR as usize / 20);
    let p = rig.snap(0).position;
    rig.render(SR as usize / 4);
    assert_eq!(rig.snap(0).position, p);
}

#[test]
fn stutter_cue_jumps_back_and_keeps_playing() {
    let mut rig = Rig::laptop();
    let t = sine_track(100.0, 20.0, 0.4);
    rig.load(0, &t, Some(grid(120.0)));
    // Cue point at beat 1 (paused there: CUE sets it).
    rig.deck(0, DeckCommand::Seek(BEAT));
    rig.render(BLOCK * 4);
    rig.deck(0, DeckCommand::Cue(true));
    rig.render(BLOCK);
    rig.deck(0, DeckCommand::Cue(false));
    rig.render(BLOCK * 8);
    assert_eq!(rig.snap(0).cue_point, BEAT);
    rig.deck(0, DeckCommand::Play(true));
    rig.render(3 * BEAT as usize);
    // CDJ (default): back to the cue and paused.
    rig.deck(0, DeckCommand::Cue(true));
    rig.render(BLOCK * 8);
    rig.deck(0, DeckCommand::Cue(false));
    rig.render(BLOCK * 8);
    let s = rig.snap(0);
    assert!(!s.playing);
    assert!((s.position - BEAT).abs() < 1.0);
    // Stutter: back to the cue and still playing.
    rig.deck(0, DeckCommand::SetStutterCue(true));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(3 * BEAT as usize);
    rig.deck(0, DeckCommand::Cue(true));
    rig.render(BLOCK * 8);
    rig.deck(0, DeckCommand::Cue(false));
    let s = rig.snap(0);
    assert!(s.playing, "still playing");
    assert!(
        s.position > BEAT && s.position < BEAT + 0.2 * f64::from(SR),
        "restarted at the cue: {}",
        s.position
    );
    rig.render(BLOCK * 8);
    assert!(rig.snap(0).playing);
}

#[test]
fn load_with_a_start_position_starts_there() {
    let mut rig = Rig::laptop();
    let t = sine_track(100.0, 20.0, 0.4);
    rig.deck(
        0,
        DeckCommand::LoadWithMarkers {
            audio: Arc::clone(&t),
            serial: 3,
            markers: TrackMarkers {
                grid: Some(grid(120.0)),
                cue_point: Some(2.0 * BEAT),
                start: Some(2.0 * BEAT),
                ..Default::default()
            },
        },
    );
    rig.render(BLOCK);
    let s = rig.snap(0);
    assert_eq!(s.position, 2.0 * BEAT);
    assert_eq!(s.cue_point, 2.0 * BEAT);
    // Without a start: the start of the track.
    rig.deck(
        1,
        DeckCommand::LoadWithMarkers {
            audio: Arc::clone(&t),
            serial: 4,
            markers: TrackMarkers {
                cue_point: Some(2.0 * BEAT),
                ..Default::default()
            },
        },
    );
    rig.render(BLOCK);
    assert_eq!(rig.snap(1).position, 0.0);
}

#[test]
fn wsola_key_lock_keeps_the_pitch() {
    let mut rig = Rig::laptop();
    let t = sine_track(220.0, 20.0, 0.4);
    rig.load(0, &t, None);
    rig.deck(0, DeckCommand::SetKeyLockWsola(true));
    rig.deck(0, DeckCommand::SetKeyLock(true));
    rig.deck(0, DeckCommand::SetPitch(1.0)); // +8 %
    rig.deck(0, DeckCommand::Play(true));
    rig.render(SR as usize);
    let f = freq(&rig.render(SR as usize / 2));
    assert!((f - 220.0).abs() < 220.0 * 0.01, "WSOLA: {f:.1} Hz");
    // And back to Signalsmith while playing.
    rig.deck(0, DeckCommand::SetKeyLockWsola(false));
    rig.render(SR as usize / 2);
    let f = freq(&rig.render(SR as usize / 2));
    assert!((f - 220.0).abs() < 220.0 * 0.01, "Signalsmith: {f:.1} Hz");
    assert!(rig.snap(0).key_lock);
}

#[test]
fn a_stopped_master_hands_sync_over_only_when_set() {
    for auto in [false, true] {
        let mut rig = Rig::laptop();
        let t = sine_track(100.0, 30.0, 0.2);
        rig.load(0, &t, Some(grid(120.0)));
        rig.load(1, &t, Some(grid(124.0)));
        rig.send(Command::SetAutoSyncMaster(auto));
        rig.deck(0, DeckCommand::Play(true));
        rig.deck(1, DeckCommand::Play(true));
        rig.send(Command::Sync(DeckId::ALL[0], true));
        rig.send(Command::Sync(DeckId::ALL[1], true));
        rig.send(Command::SyncMaster(DeckId::ALL[0]));
        rig.render(SR as usize / 2);
        assert_eq!(rig.h.snapshot().sync_master, Some(0));
        rig.deck(0, DeckCommand::Play(false));
        rig.render(SR as usize / 2);
        let want = if auto { Some(1) } else { Some(0) };
        assert_eq!(rig.h.snapshot().sync_master, want, "auto {auto}");
    }
}

#[test]
fn limiter_ceiling_is_settable() {
    let mut rig = Rig::laptop();
    // 0.9 with +12 dB trim: far over any ceiling.
    let t = sine_track(100.0, 10.0, 0.9);
    rig.load(0, &t, None);
    rig.send(Command::Mixer(MixerCommand::Trim(DeckId::ALL[0], 1.0)));
    rig.send(Command::Mixer(MixerCommand::LimiterCeiling(-1.0)));
    rig.deck(0, DeckCommand::Play(true));
    rig.render(SR as usize / 2);
    let out = rig.render(SR as usize / 2);
    let ceiling = 10f32.powf(-1.0 / 20.0);
    assert!(peak(&out) <= ceiling + 1e-4, "peak {}", peak(&out));
    assert!(peak(&out) > ceiling * 0.95, "reaches the ceiling");
    assert_eq!(rig.h.snapshot().mixer.limiter_ceiling_db, -1.0);
}

#[test]
fn headphone_cue_before_the_eq_ignores_the_eq() {
    for pre in [false, true] {
        let mut rig = Rig::new(OutputRouting::ns4fx(), 4);
        let t = sine_track(60.0, 10.0, 0.4);
        rig.load(0, &t, None);
        rig.send(Command::Mixer(MixerCommand::Pfl(DeckId::ALL[0], true)));
        rig.send(Command::Mixer(MixerCommand::CueMix(0.0)));
        rig.send(Command::Mixer(MixerCommand::CuePreEq(pre)));
        // Low band killed: a 60 Hz tone disappears after the EQ.
        rig.send(Command::Mixer(MixerCommand::Kill(
            DeckId::ALL[0],
            EqBand::Low,
            true,
        )));
        rig.deck(0, DeckCommand::Play(true));
        rig.render_ch(SR as usize / 2, 2);
        let cue = rig.render_ch(SR as usize / 2, 2);
        if pre {
            assert!(
                peak(&cue) > 0.3,
                "pre-EQ cue keeps the bass: {}",
                peak(&cue)
            );
        } else {
            // The killed low band leaves about -17 dB of 60 Hz (12 dB/oct).
            assert!(peak(&cue) < 0.07, "post-EQ cue is killed: {}", peak(&cue));
        }
    }
}
