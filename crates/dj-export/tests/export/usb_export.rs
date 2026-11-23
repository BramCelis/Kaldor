//! Outputs B and C end to end: a small library with real audio files,
//! exported to a folder standing in for the USB stick, then read back with
//! export-verify (rekordcrate for export.pdb, its own walker for the ANLZ
//! files; no code shared with the writer) and checked against the XML.
//!
//! What the players make of the stick is checked on real hardware, never
//! here.

use dj_core::Beatgrid;
use dj_export::usb::{CancelToken, PLAN_XML, UsbExportOptions, UsbFormat, export_usb, plan_usb};
use dj_library::{Cue, CueKind, Library, SavedLoop, TrackMeta};
use std::path::{Path, PathBuf};

const SR: u32 = 44_100;

/// A fresh, empty folder under the temp directory.
fn temp_dir(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let d = std::env::temp_dir().join(format!("kaldor-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A 16-bit stereo WAV: a click on every beat at 120 BPM over a low hum.
fn write_wav(path: &Path, seconds: f64) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: SR,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let n = (seconds * f64::from(SR)) as usize;
    let beat = SR as usize / 2;
    for i in 0..n {
        let t = i as f64 / f64::from(SR);
        let mut v = 0.2 * (2.0 * std::f64::consts::PI * 110.0 * t).sin();
        if i % beat < 400 {
            v += 0.6;
        }
        let s = (v.clamp(-1.0, 1.0) * 30_000.0) as i16;
        w.write_sample(s).unwrap();
        w.write_sample(s).unwrap();
    }
    w.finalize().unwrap();
}

fn meta(path: &Path, title: &str, seconds: f64) -> TrackMeta {
    TrackMeta {
        path: path.to_path_buf(),
        artist: "Kaldor Test".into(),
        title: title.into(),
        album: "Probe Album".into(),
        genre: "House".into(),
        duration: seconds,
        bitrate: Some(1411),
        sample_rate: Some(SR),
        file_size: std::fs::metadata(path).unwrap().len(),
        ..TrackMeta::default()
    }
}

/// Two tracks: "One" with a grid, two coloured hot cues and a saved loop;
/// "Two" with a grid only. Crates: Sets (folder) > Friday [One, Two],
/// and Warm-up [Two] at the top.
fn library(music: &Path) -> Library {
    let (p1, p2) = (music.join("one.wav"), music.join("two.wav"));
    write_wav(&p1, 12.0);
    write_wav(&p2, 60.0);
    let mut lib = Library::open_in_memory().unwrap();
    let (t1, _) = lib.upsert_track(&meta(&p1, "One", 12.0)).unwrap();
    let (t2, _) = lib.upsert_track(&meta(&p2, "Two", 60.0)).unwrap();
    lib.set_beatgrid(t1, &Beatgrid::new(120.0, 0.0)).unwrap();
    // A grid whose BPM is not a round number, as analysis often gives;
    // with 2 decimals in the XML it drifts 2 ms off over the track.
    lib.set_beatgrid(t2, &Beatgrid::new(127.995, 0.25)).unwrap();
    lib.set_cues(
        t1,
        &[
            Cue {
                kind: CueKind::Hot(0),
                position: 2.0,
                name: "Drop".into(),
                color: Some(0xE6_2828),
            },
            Cue {
                kind: CueKind::Hot(1),
                position: 4.0,
                name: String::new(),
                color: Some(0x28_E2_14),
            },
        ],
    )
    .unwrap();
    lib.set_loops(
        t1,
        &[SavedLoop {
            slot: 0,
            start: 6.0,
            end: 8.0,
            name: "4 beats".into(),
            color: None,
        }],
    )
    .unwrap();
    let sets = lib.create_crate("Sets", None).unwrap();
    let fri = lib.create_crate("Friday", Some(sets)).unwrap();
    let warm = lib.create_crate("Warm-up", None).unwrap();
    lib.add_to_crate(fri, &[t1, t2]).unwrap();
    lib.add_to_crate(warm, &[t2]).unwrap();
    lib
}

/// Files under `dir` with extension `ext`.
fn files(dir: &Path, ext: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, ext, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)) {
                out.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(dir, ext, &mut v);
    v
}

/// The analysis files on the stick.
fn anlz_files(device: &Path, ext: &str) -> Vec<PathBuf> {
    files(&device.join("PIONEER").join("USBANLZ"), ext)
}

/// The audio on the stick.
fn audio_files(device: &Path) -> Vec<PathBuf> {
    files(&device.join("Contents"), "wav")
}

#[test]
fn a_small_library_on_a_stick_reads_back() {
    let root = temp_dir("usb");
    let music = root.join("music");
    let stick = root.join("stick");
    std::fs::create_dir_all(&music).unwrap();
    std::fs::create_dir_all(&stick).unwrap();
    let lib = library(&music);
    let opts = UsbExportOptions {
        device: stick.clone(),
        crates: Vec::new(),
        format: UsbFormat::Both,
        prune: true,
        artwork: false,
        work_dir: root.join("work"),
    };

    // Plan: nothing written to the stick yet.
    let plan = plan_usb(&lib, &opts).unwrap();
    assert_eq!(plan.tracks, 2, "skipped: {:?}", plan.skipped);
    assert!(plan.skipped.is_empty(), "{:?}", plan.skipped);
    assert_eq!(plan.without_grid, 0);
    assert!(
        plan.playlists.iter().any(|p| p.ends_with("Friday")),
        "{:?}",
        plan.playlists
    );
    assert!(!stick.join("PIONEER").exists(), "the plan writes nothing");

    let report = export_usb(&plan, &(), &CancelToken::new()).unwrap();
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.copied, 2);
    assert!(report.onelibrary_written, "{:?}", report.onelibrary_error);

    // The files.
    let pdb = stick.join("PIONEER").join("rekordbox").join("export.pdb");
    assert!(pdb.is_file());
    assert!(
        stick
            .join("PIONEER")
            .join("rekordbox")
            .join("exportLibrary.db")
            .is_file()
    );
    assert_eq!(anlz_files(&stick, "DAT").len(), 2);
    assert_eq!(anlz_files(&stick, "EXT").len(), 2);
    assert_eq!(audio_files(&stick).len(), 2, "the audio survives the prune");

    // export-verify: everything there, and the counts match the XML.
    let xml = root.join("work").join(PLAN_XML);
    let v = export_verify::verify(&stick, Some(&xml)).unwrap();
    assert!(v.ok(), "{:#?}\nnotes: {:#?}", v.problems, v.notes);
    assert!(v.onelibrary);
    assert_eq!(v.tracks.len(), 2);
    assert_eq!((v.playlists, v.folders, v.entries), (2, 1, 3));
    let names: Vec<String> = export_verify::read_pdb(&pdb)
        .unwrap()
        .nodes
        .into_iter()
        .map(|n| n.2)
        .collect();
    for name in ["Sets", "Friday", "Warm-up"] {
        assert!(names.iter().any(|p| p == name), "{name}: {names:?}");
    }

    // "One": 120 BPM, a grid from 0 s every 500 ms, hot cues A and B at
    // 2 s and 4 s, and the saved loop (slot 0, taken by hot cue A) on C.
    let one = v.tracks.iter().find(|t| t.pdb.title == "One").unwrap();
    assert_eq!(one.pdb.tempo, 12000);
    assert!(
        one.pdb.file_path.starts_with("/Contents/"),
        "{}",
        one.pdb.file_path
    );
    let beats = &one.dat.as_ref().unwrap().beats;
    assert!(
        (22..=25).contains(&beats.len()),
        "12 s at 120 BPM: {} beats",
        beats.len()
    );
    assert_eq!(
        (beats[0].time, beats[1].time, beats[0].tempo),
        (0, 500, 12000)
    );
    assert_eq!(
        beats
            .iter()
            .take(5)
            .map(|b| b.beat_number)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 1]
    );
    let mut cues: Vec<(u32, u8, u32, u32)> = one
        .ext
        .as_ref()
        .unwrap()
        .cues()
        .into_iter()
        .map(|(_, c)| (c.hot_cue, c.kind, c.time, c.loop_time))
        .collect();
    cues.sort_unstable();
    assert_eq!(
        cues,
        [
            (1, 1, 2000, u32::MAX),
            (2, 1, 4000, u32::MAX),
            (3, 2, 6000, 8000)
        ]
    );
    assert_eq!(one.cues, (3, 0));

    // "Two": every beat on the stick within 1 ms of Kaldor's own grid
    // (127.995 BPM from 0.25 s) to the end of the track, not only of the
    // XML's (which export-verify checks).
    let two = v.tracks.iter().find(|t| t.pdb.title == "Two").unwrap();
    let beats = &two.dat.as_ref().unwrap().beats;
    assert!(beats.len() >= 125, "60 s at 128 BPM: {} beats", beats.len());
    assert_eq!(beats[0].time, 250);
    let err = export_verify::grid_error_ms(beats, 0.25, 127.995);
    assert!(err <= 1.0, "stick grid {err:.2} ms off Kaldor's");
    assert!(two.grid_error_ms.is_some_and(|e| e <= 1.0));

    // A second export writes no audio again (incremental) and prunes a
    // file no track has any more, keeping the rest.
    let stale = stick.join("Contents").join("Gone").join("old.wav");
    std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
    std::fs::write(&stale, b"RIFF").unwrap();
    let plan = plan_usb(&lib, &opts).unwrap();
    let again = export_usb(&plan, &(), &CancelToken::new()).unwrap();
    assert_eq!(again.copied, 0);
    assert_eq!(again.kept, 2);
    assert!(!stale.exists() && !stale.parent().unwrap().exists());
    assert_eq!(again.pruned, 1);
    assert_eq!(audio_files(&stick).len(), 2);
    assert_eq!(anlz_files(&stick, "DAT").len(), 2);

    std::fs::remove_dir_all(&root).ok();
}
