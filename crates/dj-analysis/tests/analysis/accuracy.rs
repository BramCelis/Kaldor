//! Analysis accuracy on synthetic audio with known answers.
//! Spec targets: BPM within 0.01 on constant-tempo tracks, first beat within
//! 5 ms.

use dj_analysis::synth::{ClickTrack, Rng};
use dj_analysis::tempo::{TempoOptions, detect_beatgrid};
use dj_analysis::{AnalysisOptions, analyze_audio};
use dj_core::MusicalKey;
use dj_decode::TrackAudio;
use std::f64::consts::PI;

fn mono(s: &[f32]) -> Vec<f32> {
    s.as_chunks::<2>()
        .0
        .iter()
        .map(|f| 0.5 * (f[0] + f[1]))
        .collect()
}

/// Phase error between the detected grid and the true one, in seconds.
fn phase_error(g: &dj_core::Beatgrid, true_first: f64) -> f64 {
    let p = g.period();
    let d = (g.first_beat - true_first).rem_euclid(p);
    d.min(p - d)
}

fn check(bpm: f64, first: f64, seconds: f64, noise: f32, sr: u32, seed: u64) {
    let c = ClickTrack {
        sample_rate: sr,
        first_beat: first,
        noise,
        seed,
        ..ClickTrack::new(bpm, seconds)
    };
    let g = detect_beatgrid(&mono(&c.render()), sr, TempoOptions::default())
        .unwrap_or_else(|| panic!("no grid for {bpm} BPM"));
    assert!(
        (g.bpm - bpm).abs() < 0.01,
        "{bpm} BPM: detected {:.4}",
        g.bpm
    );
    let err = phase_error(&g, first);
    assert!(
        err < 0.005,
        "{bpm} BPM: first beat off by {:.1} ms",
        err * 1000.0
    );
    // The click track accents every 4th beat starting at `first`: that is
    // the downbeat.
    let n = g.beat_at(first).round() as i64;
    assert!(g.is_downbeat(n), "{bpm} BPM: wrong downbeat");
}

#[test]
fn bpm_and_first_beat_across_the_range() {
    let cases = [
        (70.0, 0.31),
        (93.5, 0.05),
        (100.37, 0.42),
        (120.0, 0.0),
        (124.0, 0.237),
        (128.0, 0.111),
        (140.0, 0.399),
        (174.0, 0.2),
        (179.9, 0.07),
    ];
    for (i, (bpm, first)) in cases.into_iter().enumerate() {
        check(bpm, first, 60.0, 0.05, 44_100, i as u64 + 1);
    }
}

#[test]
fn works_at_48k_and_with_heavy_noise() {
    check(126.0, 0.18, 60.0, 0.05, 48_000, 9);
    check(132.5, 0.33, 60.0, 0.25, 44_100, 10);
}

#[test]
fn clicks_without_kick_body() {
    let c = ClickTrack {
        kick_body: false,
        noise: 0.02,
        first_beat: 0.2,
        ..ClickTrack::new(122.0, 60.0)
    };
    let g = detect_beatgrid(&mono(&c.render()), 44_100, TempoOptions::default()).unwrap();
    assert!((g.bpm - 122.0).abs() < 0.01, "{}", g.bpm);
    assert!(phase_error(&g, 0.2) < 0.005);
}

#[test]
fn silence_and_noise_give_no_grid_or_no_panic() {
    assert!(detect_beatgrid(&vec![0.0; 44_100 * 10], 44_100, TempoOptions::default()).is_none());
    let mut rng = Rng::new(3);
    let noise: Vec<f32> = (0..44_100 * 10).map(|_| 0.2 * rng.bipolar()).collect();
    // White noise has no tempo; any answer is fine as long as it is in range.
    if let Some(g) = detect_beatgrid(&noise, 44_100, TempoOptions::default()) {
        assert!((63.0..=198.0).contains(&g.bpm));
    }
}

/// Renders a chord progression; each chord is a set of MIDI notes with a
/// few harmonics.
fn chords(progression: &[&[u8]], sr: u32, chord_seconds: f64, repeats: usize) -> Vec<f32> {
    let per = (chord_seconds * f64::from(sr)) as usize;
    let mut out = Vec::with_capacity(per * progression.len() * repeats * 2);
    for _ in 0..repeats {
        for chord in progression {
            for i in 0..per {
                let t = i as f64 / f64::from(sr);
                let env = (1.0 - (i as f64 / per as f64)).max(0.2);
                let mut v = 0.0;
                for &n in *chord {
                    let f = 440.0 * 2f64.powf((f64::from(n) - 69.0) / 12.0);
                    for (h, a) in [(1.0, 1.0), (2.0, 0.4), (3.0, 0.2)] {
                        v += a * (2.0 * PI * f * h * t).sin();
                    }
                }
                let s = (0.08 * env * v) as f32;
                out.push(s);
                out.push(s);
            }
        }
    }
    out
}

#[test]
fn key_detection_on_chord_progressions() {
    // C major: C F G C.
    let c_major: [&[u8]; 4] = [
        &[48, 60, 64, 67],
        &[53, 65, 69, 72],
        &[55, 67, 71, 74],
        &[48, 60, 64, 67],
    ];
    // A minor: Am Dm E Am (harmonic minor, G# leading tone).
    let a_minor: [&[u8]; 4] = [
        &[45, 57, 60, 64],
        &[50, 62, 65, 69],
        &[52, 64, 68, 71],
        &[45, 57, 60, 64],
    ];
    // F# minor: F#m Bm C# F#m.
    let fs_minor: [&[u8]; 4] = [
        &[42, 54, 57, 61],
        &[47, 59, 62, 66],
        &[49, 61, 65, 68],
        &[42, 54, 57, 61],
    ];
    for (prog, want) in [
        (&c_major, MusicalKey::new(0, false)),
        (&a_minor, MusicalKey::new(9, true)),
        (&fs_minor, MusicalKey::new(6, true)),
    ] {
        let s = chords(prog, 44_100, 1.5, 3);
        let (k, conf) = dj_analysis::key::detect_key(&mono(&s), 44_100).unwrap();
        assert_eq!(
            k,
            want,
            "detected {} ({conf:.3}), wanted {}",
            k.musical(),
            want.musical()
        );
    }
}

#[test]
fn full_analysis_of_a_track() {
    let c = ClickTrack {
        sample_rate: 44_100,
        noise: 0.03,
        first_beat: 0.25,
        ..ClickTrack::new(127.0, 45.0)
    };
    let t = TrackAudio::from_interleaved(44_100, &c.render());
    let a = analyze_audio(&t, &AnalysisOptions::default());
    let g = a.beatgrid.unwrap();
    assert!((g.bpm - 127.0).abs() < 0.01);
    assert!(a.lufs.unwrap() < -5.0 && a.lufs.unwrap() > -40.0);
    assert!(a.gain_db.is_some());
    assert!(a.waveform.points_per_second >= 150.0);
    assert!((a.duration - 45.0).abs() < 0.01);
}

/// i-iv-V-i (minor, harmonic V) or I-IV-V-I (major) with a bass note an
/// octave below, tonic `t` as a MIDI note in octave 3.
fn cadence(t: u8, minor: bool) -> [Vec<u8>; 4] {
    let third = if minor { 3 } else { 4 };
    let four_third = if minor { 8 } else { 9 };
    let i = vec![t - 12, t, t + third, t + 7];
    let iv = vec![t - 7, t + 5, t + four_third, t + 12];
    let v = vec![t - 5, t + 7, t + 11, t + 14];
    [i.clone(), iv, v, i]
}

/// Regression: with loud broadband noise (drums, hats) the old
/// detector pulled every track towards A minor / C major (white noise alone
/// came out as 8A). Keys from the reference set plus a few more.
#[test]
fn key_detection_with_loud_noise() {
    let sr = 44_100;
    // (tonic MIDI note in octave 3, minor, Camelot)
    let cases = [
        (50, true, "7A"),  // D minor
        (51, true, "2A"),  // Eb minor
        (52, true, "9A"),  // E minor
        (53, true, "4A"),  // F minor
        (54, true, "11A"), // F# minor
        (57, true, "8A"),  // A minor
        (59, false, "1B"), // B major
        (55, false, "9B"), // G major
    ];
    let mut rng = Rng::new(7);
    for (tonic, minor, want) in cases {
        let prog = cadence(tonic, minor);
        let refs: Vec<&[u8]> = prog.iter().map(|c| c.as_slice()).collect();
        let tone = mono(&chords(&refs, sr, 1.5, 4));
        // Noise about as loud as the chords (RMS), like hats and snares.
        let rms = (tone.iter().map(|v| v * v).sum::<f32>() / tone.len() as f32).sqrt();
        let mixed: Vec<f32> = tone.iter().map(|v| v + 1.7 * rms * rng.bipolar()).collect();
        let (k, conf) = dj_analysis::key::detect_key(&mixed, sr).unwrap();
        assert_eq!(k.camelot(), want, "detected {} ({conf:.3})", k.camelot());
    }
}

#[test]
fn white_noise_has_no_key() {
    let n = dj_analysis::synth::noise(44_100, 60.0, 0.5, 3);
    let k = dj_analysis::key::detect_key(&mono(&n), 44_100);
    // Either no key or a key with a negligible margin over the runner-up.
    assert!(k.is_none_or(|(_, conf)| conf < 0.02), "{k:?}");
}

/// Regression: hard dance / bounce style, an identical kick on every
/// beat (no accent on beat one) and sections that change every 8 bars on the
/// downbeat. The first downbeat is the file's second beat (a pickup).
#[test]
fn downbeat_from_section_changes_without_accent() {
    let sr = 44_100u32;
    let srf = f64::from(sr);
    let bpm = 150.0;
    let beat = 60.0 / bpm;
    let first_down = 0.2 + beat; // pickup beat at 0.2 s
    let seconds = 90.0;
    let mut x = vec![0.0f32; (seconds * srf) as usize];
    let mut rng = Rng::new(5);
    let beats = ((seconds - 0.2) / beat) as usize;
    for b in 0..beats {
        let t = 0.2 + b as f64 * beat;
        let s0 = (t * srf) as usize;
        // Bar number counted from the first downbeat (the pickup is bar -1).
        let bar = ((t - first_down) / (4.0 * beat)).floor() as i64;
        let section = bar.div_euclid(8).rem_euclid(4); // 0 kick, 1 +hats, 2 +bass, 3 breakdown
        for i in 0..(0.35 * srf) as usize {
            let u = i as f64 / srf;
            let mut v = 0.0;
            if section != 3 {
                v += 0.8
                    * (-u / 0.05).exp()
                    * (2.0 * PI * (50.0 + 80.0 * (-u / 0.015).exp()) * u).sin();
            }
            if section >= 1 && u < 0.03 {
                v += 0.15 * (-u / 0.01).exp() * f64::from(rng.bipolar());
            }
            if section == 2 && u > 0.5 * beat && u < 0.5 * beat + 0.12 {
                v += 0.5 * (2.0 * PI * 110.0 * u).sin();
            }
            if section == 3 {
                v += 0.2 * (2.0 * PI * 440.0 * u).sin();
            }
            if let Some(o) = x.get_mut(s0 + i) {
                *o += v as f32;
            }
        }
    }
    let g = detect_beatgrid(&x, sr, TempoOptions::default()).expect("a grid");
    assert!((g.bpm - bpm).abs() < 0.05, "bpm {}", g.bpm);
    // Every true downbeat must be a bar line in the detected grid.
    let wrong: Vec<f64> = g
        .beats_between(first_down - 0.01, seconds - 1.0)
        .filter(|(_, t, _)| {
            let k = (t - first_down) / beat;
            (k.round() - k).abs() < 0.1 && (k.round() as i64).rem_euclid(4) == 0
        })
        .filter(|(_, _, down)| !down)
        .map(|(_, t, _)| t)
        .collect();
    assert!(
        wrong.is_empty(),
        "bar lines off the true downbeats, first at {:?}",
        wrong.first()
    );
}

/// Dance music is made at whole BPMs: a tempo measured within 0.05 BPM of a
/// whole number is rounded (no one produces at 70.05 BPM), and the
/// grid still sits on the kicks.
#[test]
fn near_whole_tempo_is_rounded() {
    let c = ClickTrack {
        first_beat: 0.25,
        noise: 0.05,
        ..ClickTrack::new(128.02, 60.0)
    };
    let g = detect_beatgrid(&mono(&c.render()), 44_100, TempoOptions::default()).unwrap();
    assert_eq!(g.bpm, 128.0);
    // 0.02 BPM over 60 s drifts 9 ms; the grid splits it.
    assert!(phase_error(&g, 0.25) < 0.008, "{}", phase_error(&g, 0.25));
}

/// A loud off-beat bass (hard bounce, bassline) shares the kick's band; the
/// grid lines must still go on the kicks.
#[test]
fn grid_on_the_kick_with_a_loud_off_beat_bass() {
    let sr = 44_100;
    let bpm = 150.0;
    let first = 0.2;
    let c = ClickTrack {
        first_beat: first,
        noise: 0.02,
        ..ClickTrack::new(bpm, 60.0)
    };
    let mut m = mono(&c.render());
    // A 70 Hz bass note on every off-beat, fading in over 15 ms.
    let p = 60.0 / bpm;
    let mut t = first + p / 2.0;
    while t < 59.5 {
        let start = (t * f64::from(sr)) as usize;
        for i in 0..(0.15 * f64::from(sr)) as usize {
            let x = i as f64 / f64::from(sr);
            let env = (x / 0.015).min(1.0) * (-x / 0.1).exp();
            if let Some(s) = m.get_mut(start + i) {
                *s += (0.6 * env * (2.0 * PI * 70.0 * x).sin()) as f32;
            }
        }
        t += p;
    }
    let g = detect_beatgrid(&m, sr, TempoOptions::default()).unwrap();
    assert_eq!(g.bpm, bpm);
    assert!(
        phase_error(&g, first) < 0.005,
        "off by {:.1} ms",
        phase_error(&g, first) * 1000.0
    );
}
