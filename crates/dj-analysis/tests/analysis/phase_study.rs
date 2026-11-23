//! Study: which cue tells the kick's lines from the lines half a
//! beat away (where a loud off-beat bass sits on bounce and bassline
//! tracks). Uses private audio in `Test music/` and is ignored:
//! `cargo test -p dj-analysis --release --test analysis phase_study -- --ignored --nocapture`.
//!
//! Truth: grid lines checked by hand on the kick, including tracks where
//! the grid had to move half a beat.

use dj_analysis::AnalysisOptions;
use dj_analysis::tempo::detect_beatgrid;
use std::path::Path;

/// File, and whether the analysed lines are on the kick (false: the kick
/// is half a beat away).
const TRACKS: [(&str, bool); 9] = [
    (
        "1SELF - Sister Nancy Bam Bam (Speed Garage Bootleg).mp3",
        true,
    ),
    (
        "Dj Casper - Cha Cha Slide ( shortstrumentals Bassline Remix).mp3",
        true,
    ),
    ("Dr Cryptic_Silly Billy.mp3", true),
    ("Fck That Sht.mp3", true),
    ("Jacks - Kiss Me.mp3", true),
    ("K dot ft tempo876 - take the piss.mp3", true),
    ("Pluggerz - U Lift Me Up (Master).mp3", true),
    ("SCREAM AND SHOUT.mp3", true),
    ("Sexy Bitch (Remix).mp3", false),
];

#[test]
#[ignore = "needs private audio in Test music/"]
fn half_beat_cues() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Test music");
    println!(
        "{:<28} {:>7} {:>7}  {:>15} {:>15} {:>15} {:>15} {:>15}",
        "track (kick / off)",
        "BPM",
        "first",
        "low_rise",
        "high_rise",
        "mid_rise",
        "low_decay",
        "section"
    );
    for (file, on_kick) in TRACKS {
        let (_, audio) = dj_decode::decode_file(&dir.join(file), 0).unwrap();
        let sr = audio.sample_rate();
        let mono = dj_analysis::analyze::mono(&audio);
        let g = detect_beatgrid(&mono, sr, AnalysisOptions::default().tempo).unwrap();
        let p = g.period();
        let (kick, off) = if on_kick {
            (g.first_beat, g.first_beat + p / 2.0)
        } else {
            (g.first_beat + p / 2.0, g.first_beat)
        };
        let (a, b) = (line_cues(&mono, sr, kick, p), line_cues(&mono, sr, off, p));
        let pair = |x: f64, y: f64| format!("{x:6.2}/{y:6.2}{}", if x > y { "+" } else { "-" });
        println!(
            "{:<28} {:>7.2} {:>7.3}  {:>15} {:>15} {:>15} {:>15} {:>15}",
            &file[..file.len().min(28)],
            g.bpm,
            g.first_beat,
            pair(a.low_rise, b.low_rise),
            pair(a.high_rise, b.high_rise),
            pair(a.mid_rise, b.mid_rise),
            pair(a.low_decay, b.low_decay),
            pair(a.section, b.section),
        );
    }
    println!("(+ = the cue is larger on the kick's lines, as a good cue should be on every track)");
}

/// Cues that tell a grid's lines from lines half a beat away: averages over the lines `o + n p`.
#[derive(Clone, Copy, Debug, Default)]
struct LineCues {
    /// Low-band (< 150 Hz) rise in ±3 ms, the measure the grid uses now.
    low_rise: f64,
    /// High-band (> 2 kHz) rise from -5 to +15 ms (a kick's click).
    high_rise: f64,
    /// Mid-band (150 Hz-2 kHz) rise from -5 to +15 ms.
    mid_rise: f64,
    /// Low-band level at its peak (0-30 ms) minus 80-120 ms later, dB
    /// (a kick decays; a bass note holds).
    low_decay: f64,
    /// Section changes on the lines: mean of the largest 10 % of the
    /// changes between the 4 beats before and the 4 beats after a line.
    section: f64,
}

fn line_cues(mono: &[f32], sample_rate: u32, o: f64, p: f64) -> LineCues {
    let sr = f64::from(sample_rate.max(1));
    let block = ((sr / 1000.0).round() as usize).max(1);
    let mut split = dj_dsp::biquad::ThreeBandSplit::new(150.0, 2_000.0, sr);
    let mut level = vec![[0.0f64; 3]; mono.len() / block + 1];
    for (i, x) in mono.iter().enumerate() {
        let (l, m, h) = split.process(*x);
        let e = &mut level[i / block];
        e[0] += f64::from(l * l);
        e[1] += f64::from(m * m);
        e[2] += f64::from(h * h);
    }
    let db: Vec<[f64; 3]> = level
        .iter()
        .map(|e| e.map(|v| 10.0 * (v / block as f64 + 1e-12).log10()))
        .collect();
    let n_ms = db.len() as i64;
    let rise = |k: i64, b: usize| -> f64 {
        if k < 10 || k >= n_ms {
            return 0.0;
        }
        let k = k as usize;
        let before = db[k - 10..k - 2].iter().map(|e| e[b]).sum::<f64>() / 8.0;
        (db[k][b] - before).max(0.0)
    };
    let max_rise = |a: i64, z: i64, b: usize| (a..=z).map(|k| rise(k, b)).fold(0.0, f64::max);
    let mut c = LineCues::default();
    let mut lines = 0.0f64;
    let mut changes = Vec::new();
    let beat_ms = p * 1000.0;
    let mut n = (-o / p).ceil() as i64;
    loop {
        let t = o + n as f64 * p;
        let k = (t * 1000.0).round() as i64;
        if k + 130 >= n_ms {
            break;
        }
        n += 1;
        if k < 10 {
            continue;
        }
        lines += 1.0;
        c.low_rise += max_rise(k - 3, k + 3, 0);
        c.high_rise += max_rise(k - 5, k + 15, 2);
        c.mid_rise += max_rise(k - 5, k + 15, 1);
        let peak = (k..k + 30)
            .map(|j| db[j as usize][0])
            .fold(f64::MIN, f64::max);
        let later = (k + 80..k + 120).map(|j| db[j as usize][0]).sum::<f64>() / 40.0;
        c.low_decay += peak - later;
        // Mean log level per band over 4 beats before and after the line.
        let span = (4.0 * beat_ms) as i64;
        if k - span >= 0 && k + span < n_ms {
            let mean = |a: i64, z: i64| {
                let mut m = [0.0; 3];
                for j in a..z {
                    for (b, v) in m.iter_mut().enumerate() {
                        *v += db[j as usize][b];
                    }
                }
                m.map(|v| v / (z - a) as f64)
            };
            let (before, after) = (mean(k - span, k), mean(k, k + span));
            changes.push(
                before
                    .iter()
                    .zip(after.iter())
                    .map(|(x, y)| (y - x).powi(2))
                    .sum::<f64>(),
            );
        }
    }
    let l = lines.max(1.0);
    c.low_rise /= l;
    c.high_rise /= l;
    c.mid_rise /= l;
    c.low_decay /= l;
    changes.sort_by(|a, b| b.total_cmp(a));
    let top = (changes.len() / 10).max(1).min(changes.len());
    c.section = changes[..top].iter().sum::<f64>() / top.max(1) as f64;
    c
}
