//! Accuracy on real music, against reference values (for example Serato's).
//!
//! The audio and the reference values are private and never committed:
//! audio in `Test music/`, values in `local/expected.csv` (both git-ignored),
//! one line per track: `file name,bpm,camelot key`. Run with
//! `cargo test -p dj-analysis --release --test analysis real_tracks -- --ignored --nocapture`.
//! It prints a table and fails if any BPM or key is wrong.

use dj_analysis::AnalysisOptions;
use std::path::Path;

/// BPMs match if the detected tempo, halved or doubled or as is, is within
/// 0.6 of the reference (Serato shows whole BPMs; 70 and 140 are the same
/// tempo for mixing).
fn bpm_matches(got: f64, want: f64) -> bool {
    [1.0, 2.0, 0.5]
        .iter()
        .any(|k| (got / k - want).abs() <= 0.6)
}

/// The file in `dir` named `name`, comparing only ASCII characters: names
/// typed from a screenshot can spell accented letters differently (composed
/// or decomposed Unicode) from the file system.
fn find_track(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    let ascii = |s: &str| s.chars().filter(char::is_ascii).collect::<String>();
    let want = ascii(name);
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|f| f.to_str())
                .is_some_and(|f| ascii(f) == want)
        })
}

#[test]
#[ignore = "needs private audio in Test music/ and local/expected.csv"]
fn real_tracks_match_reference() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let csv = std::fs::read_to_string(root.join("local/expected.csv"))
        .expect("local/expected.csv (file,bpm,key per line)");
    let opts = AnalysisOptions::default();
    let (mut bpm_ok, mut key_ok, mut n) = (0, 0, 0);
    for line in csv
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
    {
        let mut parts = line.rsplitn(3, ',');
        let (Some(key), Some(bpm), Some(file)) = (parts.next(), parts.next(), parts.next()) else {
            panic!("bad line: {line}");
        };
        let want_bpm: f64 = bpm.trim().parse().expect("bpm");
        let Some(path) = find_track(&root.join("Test music"), file.trim()) else {
            println!(
                "{:<60} not found, skipped",
                file.chars().take(60).collect::<String>()
            );
            continue;
        };
        let (_, a) = dj_analysis::analyze_file(&path, &opts)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let got_bpm = a.beatgrid.map_or(0.0, |g| g.bpm);
        let got_key = a.key.map_or("-".to_string(), |k| k.camelot());
        let b = bpm_matches(got_bpm, want_bpm);
        let k = got_key == key.trim();
        n += 1;
        bpm_ok += usize::from(b);
        key_ok += usize::from(k);
        println!(
            "{:<60} BPM {:>7.2} (ref {:>5}) {}  key {:>3} (ref {:>3}) {}",
            file.chars().take(60).collect::<String>(),
            got_bpm,
            want_bpm,
            if b { "ok " } else { "BAD" },
            got_key,
            key.trim(),
            if k { "ok" } else { "BAD" }
        );
    }
    println!("BPM {bpm_ok}/{n} right, key {key_ok}/{n} right");
    assert!(n > 0, "no reference lines");
    assert_eq!(
        (bpm_ok, key_ok),
        (n, n),
        "BPM {bpm_ok}/{n}, key {key_ok}/{n}"
    );
}
