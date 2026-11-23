//! Master recording end to end. The offline engine renders, the
//! writer thread writes real WAV and FLAC files under `target/`, and the
//! files are read back and compared with what the engine played. No audio
//! device.

use dj_audio::offline::OfflineRenderer;
use dj_audio::{Command, DeckCommand, Engine, EngineConfig, EngineHandle, TrackAudio};
use dj_core::config::RecordFormat;
use dj_core::{DeckId, OutputRouting};
use dj_ui::recorder::{Recorder, RecorderEvent};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SR: u32 = 48_000;
const BLOCK: usize = 256;

fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("recording")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn rig() -> (OfflineRenderer, EngineHandle) {
    let (engine, mut h) = Engine::new(EngineConfig {
        sample_rate: SR,
        max_block_frames: BLOCK,
        routing: OutputRouting::laptop(),
    });
    let s: Vec<f32> = (0..SR as usize * 20)
        .map(|i| ((i / 2) as f32 * 0.021).sin() * 0.6)
        .collect();
    h.send(Command::Deck(
        DeckId::ALL[0],
        DeckCommand::Load {
            audio: TrackAudio::from_interleaved(SR, &s),
            serial: 1,
        },
    ))
    .unwrap();
    h.send(Command::Deck(DeckId::ALL[0], DeckCommand::TogglePlay))
        .unwrap();
    let mut r = OfflineRenderer::new(engine, 2, BLOCK);
    r.render(BLOCK * 4);
    h.update();
    (r, h)
}

/// Renders `blocks` blocks and returns the master output (interleaved).
fn play(r: &mut OfflineRenderer, h: &mut EngineHandle, blocks: usize) -> Vec<f32> {
    let mut out = Vec::new();
    for _ in 0..blocks {
        out.extend_from_slice(r.render_block());
        h.update();
    }
    out
}

/// Waits for `n` Finished events (the writer runs on its own thread).
fn finished(rec: &Recorder, n: usize) -> Vec<(PathBuf, u64)> {
    let start = Instant::now();
    let mut done = Vec::new();
    while done.len() < n && start.elapsed() < Duration::from_secs(20) {
        for e in rec.poll() {
            match e {
                RecorderEvent::Finished { path, frames, .. } => done.push((path, frames)),
                RecorderEvent::Failed(e) => panic!("{e}"),
                RecorderEvent::Started(_) => {}
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    done
}

fn i24(v: f32) -> f32 {
    ((v.clamp(-1.0, 1.0) * 8_388_607.0).round()) / 8_388_607.0
}

fn read_wav(path: &std::path::Path) -> Vec<f32> {
    let mut r = hound::WavReader::open(path).unwrap();
    let spec = r.spec();
    assert_eq!(
        (spec.channels, spec.sample_rate, spec.bits_per_sample),
        (2, SR, 24)
    );
    r.samples::<i32>()
        .map(|s| s.unwrap() as f32 / 8_388_607.0)
        .collect()
}

fn read_any(path: &std::path::Path) -> Vec<f32> {
    let (info, audio) = dj_decode::decode_file(path, 0).unwrap();
    assert_eq!(info.source_sample_rate, SR);
    let mut out = vec![0.0; audio.len_frames() as usize * 2];
    audio.read_interleaved(0, &mut out);
    out
}

fn assert_same(file: &[f32], played: &[f32], what: &str) {
    assert_eq!(file.len(), played.len(), "{what}: length");
    for (i, (a, b)) in file.iter().zip(played.iter()).enumerate() {
        assert!((a - i24(*b)).abs() < 2e-7, "{what}: sample {i}: {a} vs {b}");
    }
}

#[test]
fn wav_and_flac_hold_exactly_what_was_played() {
    for format in [RecordFormat::Wav, RecordFormat::Flac] {
        let d = dir(format.extension());
        let (mut r, mut h) = rig();
        let rec = Recorder::spawn(h.take_record_reader().unwrap());
        let path = d.join(format!("take.{}", format.extension()));
        rec.start(path.clone(), format, SR);
        h.send(Command::Record(true)).unwrap();
        // 3 s and a bit: several FLAC blocks and a short last one.
        let played = play(&mut r, &mut h, 3 * SR as usize / BLOCK + 7);
        h.send(Command::Record(false)).unwrap();
        // The ring holds 4 s, so nothing is lost even if the writer is slow.
        play(&mut r, &mut h, 10);
        let done = finished(&rec, 1);
        assert_eq!(done.len(), 1, "{format:?}");
        assert_eq!(done[0], (path.clone(), (played.len() / 2) as u64));
        assert_eq!(h.snapshot().record_dropped, 0);
        let file = match format {
            RecordFormat::Wav => read_wav(&path),
            RecordFormat::Flac => read_any(&path),
        };
        assert_same(&file, &played, format.extension());
    }
}

#[test]
fn back_to_back_recordings_go_to_their_own_files() {
    let d = dir("two");
    let (mut r, mut h) = rig();
    let rec = Recorder::spawn(h.take_record_reader().unwrap());
    let a = d.join("a.flac");
    let b = d.join("b.wav");
    rec.start(a.clone(), RecordFormat::Flac, SR);
    h.send(Command::Record(true)).unwrap();
    let pa = play(&mut r, &mut h, 100);
    h.send(Command::Record(false)).unwrap();
    // The next one starts before the writer has seen the end of the first.
    rec.start(b.clone(), RecordFormat::Wav, SR);
    h.send(Command::Record(true)).unwrap();
    let pb = play(&mut r, &mut h, 60);
    h.send(Command::Record(false)).unwrap();
    play(&mut r, &mut h, 4);
    let done = finished(&rec, 2);
    assert_eq!(done.len(), 2);
    assert_same(&read_any(&a), &pa, "first");
    assert_same(&read_wav(&b), &pb, "second");
}

#[test]
fn closing_the_app_finishes_the_open_file() {
    let d = dir("quit");
    let (mut r, mut h) = rig();
    let rec = Recorder::spawn(h.take_record_reader().unwrap());
    let path = d.join("open.wav");
    rec.start(path.clone(), RecordFormat::Wav, SR);
    h.send(Command::Record(true)).unwrap();
    let played = play(&mut r, &mut h, 50);
    // No stop: the recorder is dropped (the app closes).
    drop(rec);
    assert_same(&read_wav(&path), &played, "dropped");
}
