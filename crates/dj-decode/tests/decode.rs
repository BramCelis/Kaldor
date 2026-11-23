//! Decoding generated WAV and AIFF files, with and without resampling.
//! Files are written under cargo's target directory, never elsewhere.

use dj_decode::{LoadEvent, TrackState, decode_file, spawn_load};
use std::f64::consts::PI;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn tmp(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-decode");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

fn write_wav(path: &PathBuf, sr: u32, channels: u16, seconds: f64, freq: f64, amp: f64) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (seconds * f64::from(sr)) as usize;
    for i in 0..n {
        let v = (amp * (2.0 * PI * freq * i as f64 / f64::from(sr)).sin() * 32767.0) as i16;
        for _ in 0..channels {
            w.write_sample(v).unwrap();
        }
    }
    w.finalize().unwrap();
}

/// Minimal 16-bit big-endian AIFF writer for tests.
fn write_aiff(path: &PathBuf, sr: u32, seconds: f64, freq: f64) {
    let n = (seconds * f64::from(sr)) as u32;
    let mut ssnd = Vec::new();
    ssnd.extend_from_slice(&0u32.to_be_bytes()); // offset
    ssnd.extend_from_slice(&0u32.to_be_bytes()); // block size
    for i in 0..n {
        let v = (0.5 * (2.0 * PI * freq * f64::from(i) / f64::from(sr)).sin() * 32767.0) as i16;
        ssnd.extend_from_slice(&v.to_be_bytes());
        ssnd.extend_from_slice(&v.to_be_bytes());
    }
    // 80-bit IEEE extended sample rate.
    let mut ext = [0u8; 10];
    let rate = f64::from(sr);
    let exp = rate.log2().floor() as i32;
    let mant = (rate / 2f64.powi(exp) * 2f64.powi(63)) as u64;
    ext[..2].copy_from_slice(&((exp + 16383) as u16).to_be_bytes());
    ext[2..].copy_from_slice(&mant.to_be_bytes());
    let mut comm = Vec::new();
    comm.extend_from_slice(&2u16.to_be_bytes());
    comm.extend_from_slice(&n.to_be_bytes());
    comm.extend_from_slice(&16u16.to_be_bytes());
    comm.extend_from_slice(&ext);
    let mut body = Vec::new();
    body.extend_from_slice(b"AIFF");
    body.extend_from_slice(b"COMM");
    body.extend_from_slice(&(comm.len() as u32).to_be_bytes());
    body.extend_from_slice(&comm);
    body.extend_from_slice(b"SSND");
    body.extend_from_slice(&(ssnd.len() as u32).to_be_bytes());
    body.extend_from_slice(&ssnd);
    let mut file = Vec::new();
    file.extend_from_slice(b"FORM");
    file.extend_from_slice(&(body.len() as u32).to_be_bytes());
    file.extend_from_slice(&body);
    std::fs::write(path, file).unwrap();
}

fn left(audio: &dj_decode::TrackAudio) -> Vec<f32> {
    (0..audio.ready_frames())
        .map(|i| audio.frame(i).unwrap()[0])
        .collect()
}

fn rms(s: &[f32]) -> f64 {
    (s.iter().map(|&v| f64::from(v) * f64::from(v)).sum::<f64>() / s.len() as f64).sqrt()
}

/// Best-fitting phase lag (in frames) between `s` and a reference sine.
fn phase_lag(s: &[f32], sr: f64, freq: f64) -> f64 {
    let (mut c, mut q) = (0.0, 0.0);
    for (i, &v) in s.iter().enumerate() {
        let t = 2.0 * PI * freq * i as f64 / sr;
        c += f64::from(v) * t.sin();
        q += f64::from(v) * t.cos();
    }
    let phase = q.atan2(c);
    -phase / (2.0 * PI * freq) * sr
}

#[test]
fn wav_same_rate_is_bit_exact_in_length_and_level() {
    let p = tmp("same_rate.wav");
    write_wav(&p, 48_000, 2, 2.0, 1000.0, 0.5);
    let (info, audio) = decode_file(&p, 48_000).unwrap();
    assert_eq!(info.source_sample_rate, 48_000);
    assert_eq!(audio.state(), TrackState::Complete);
    assert_eq!(audio.ready_frames(), 96_000);
    let l = left(&audio);
    assert!((rms(&l) - 0.5 / 2f64.sqrt()).abs() < 0.001);
}

#[test]
fn resampling_44k1_to_48k_keeps_length_pitch_level_and_timing() {
    let p = tmp("resample.wav");
    write_wav(&p, 44_100, 2, 3.0, 1000.0, 0.5);
    let (info, audio) = decode_file(&p, 48_000).unwrap();
    assert_eq!(info.source_sample_rate, 44_100);
    // 3 s at 48 kHz.
    let frames = audio.ready_frames() as i64;
    assert!((frames - 144_000).abs() <= 1, "frames {frames}");
    let l = left(&audio);
    let mid = &l[24_000..120_000];
    assert!(
        (rms(mid) - 0.5 / 2f64.sqrt()).abs() < 0.005,
        "rms {}",
        rms(mid)
    );
    // The resampler delay is trimmed: the sine is not shifted in time.
    let lag = phase_lag(mid, 48_000.0, 1000.0);
    let wrapped = (lag + 24_000.0).rem_euclid(48.0);
    let err = wrapped.min(48.0 - wrapped);
    assert!(err < 0.5, "timing error {err} frames");
}

#[test]
fn mono_is_duplicated_to_both_channels() {
    let p = tmp("mono.wav");
    write_wav(&p, 48_000, 1, 0.5, 440.0, 0.3);
    let (info, audio) = decode_file(&p, 48_000).unwrap();
    assert_eq!(info.source_channels, 1);
    for i in (0..audio.ready_frames()).step_by(97) {
        let [l, r] = audio.frame(i).unwrap();
        assert_eq!(l, r);
    }
}

#[test]
fn aiff_decodes() {
    let p = tmp("tone.aiff");
    write_aiff(&p, 44_100, 1.0, 440.0);
    let (info, audio) = decode_file(&p, 44_100).unwrap();
    assert_eq!(info.source_sample_rate, 44_100);
    assert_eq!(audio.ready_frames(), 44_100);
    assert!((rms(&left(&audio)) - 0.5 / 2f64.sqrt()).abs() < 0.002);
}

#[test]
fn errors_for_missing_and_garbage_files() {
    assert!(decode_file(&tmp("does_not_exist.wav"), 48_000).is_err());
    let p = tmp("garbage.mp3");
    std::fs::write(&p, vec![0x5Au8; 10_000]).unwrap();
    assert!(decode_file(&p, 48_000).is_err());
}

#[test]
fn background_load_reports_progress() {
    let p = tmp("bg.wav");
    write_wav(&p, 44_100, 2, 4.0, 300.0, 0.4);
    let rx = spawn_load(p, 48_000, Arc::new(AtomicBool::new(false))).unwrap();
    let audio = match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
        LoadEvent::Started { audio, info } => {
            assert_eq!(info.estimated_frames, Some(192_000));
            audio
        }
        other => panic!("unexpected {other:?}"),
    };
    match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
        LoadEvent::Finished { frames, truncated } => {
            assert!((frames as i64 - 192_000).abs() <= 1);
            assert!(!truncated);
        }
        other => panic!("unexpected {other:?}"),
    }
    assert!(audio.is_complete());
}
