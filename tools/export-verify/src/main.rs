//! `export-verify <stick> [--xml <file>] [--tracks]`
//!
//! Reads the rekordbox library on a USB stick (or a folder holding one) and
//! checks it; with `--xml`, against the rekordbox XML it was exported from
//! (Kaldor keeps it as `kaldor-usb-export.xml` in its app folder). Prints
//! what it found; exit code 0 when nothing is wrong, 1 when something is,
//! 2 when the stick cannot be read.

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "usage: export-verify <stick, e.g. E:\\> [--xml <export xml>] [--tracks]
       export-verify --compare <our stick> <rekordbox's stick>";

fn main() -> ExitCode {
    let mut device: Option<PathBuf> = None;
    let mut xml: Option<PathBuf> = None;
    let mut list = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--xml" => xml = args.next().map(PathBuf::from),
            "--tracks" => list = true,
            "--compare" => {
                let (Some(a), Some(b)) = (args.next(), args.next()) else {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                };
                return compare(&PathBuf::from(a), &PathBuf::from(b));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ if device.is_none() => device = Some(PathBuf::from(a)),
            _ => {
                eprintln!("{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let Some(device) = device else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let r = match export_verify::verify(&device, xml.as_deref()) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot read the stick: {e}");
            return ExitCode::from(2);
        }
    };
    let (hot, mem): (usize, usize) = r
        .tracks
        .iter()
        .fold((0, 0), |a, t| (a.0 + t.cues.0, a.1 + t.cues.1));
    println!("stick      {}", device.display());
    println!("tracks     {}", r.tracks.len());
    println!(
        "playlists  {} in {} folders, {} entries",
        r.playlists, r.folders, r.entries
    );
    println!("cues       {hot} hot, {mem} memory");
    println!(
        "OneLibrary {}",
        if r.onelibrary {
            "present"
        } else {
            "not on the stick"
        }
    );
    if let Some(x) = &r.expected {
        println!(
            "XML        {} tracks, {} playlists, {} folders, {} entries, {} hot, {} memory",
            x.tracks, x.playlists, x.folders, x.entries, x.hot, x.memory
        );
    }
    if list {
        for t in &r.tracks {
            let beats = t.dat.as_ref().map_or(&[][..], |a| a.beats.as_slice());
            let first = beats
                .first()
                .map_or("-".to_string(), |b| format!("{} ms", b.time));
            let grid = t
                .grid_error_ms
                .map_or("-".to_string(), |e| format!("±{e:.1} ms"));
            println!(
                "  {:>4}  {:>7.2} BPM  {:>4} beats from {first}, grid {grid}  {} hot  {} memory  {}  ({})",
                t.pdb.id,
                f64::from(t.pdb.tempo) / 100.0,
                beats.len(),
                t.cues.0,
                t.cues.1,
                t.pdb.title,
                t.pdb.file_path
            );
        }
    }
    for n in &r.notes {
        println!("note: {n}");
    }
    for p in &r.problems {
        println!("PROBLEM: {p}");
    }
    if r.ok() {
        println!("OK");
        ExitCode::SUCCESS
    } else {
        println!("{} problem(s)", r.problems.len());
        ExitCode::from(1)
    }
}

/// `--compare`: prints the structure diff; exit code 1 when the structure
/// differs.
fn compare(ours: &std::path::Path, golden: &std::path::Path) -> ExitCode {
    match export_verify::compare::compare(ours, golden) {
        Ok(c) => {
            for l in &c.lines {
                println!("{l}");
            }
            println!("== Structure differences: {}", c.structure.len());
            for s in &c.structure {
                println!("  {s}");
            }
            if c.structure.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("cannot compare: {e}");
            ExitCode::from(2)
        }
    }
}
