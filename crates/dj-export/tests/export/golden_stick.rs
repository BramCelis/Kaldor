//! Golden test: the structure of Kaldor's stick against
//! one rekordbox wrote from the same tracks.
//!
//! Needs a reference stick in `local/stick-comparison/REKORDBOX` (not in
//! git: it holds music), so it is ignored by default:
//!
//! ```sh
//! cargo test -p dj-export --test export golden_stick -- --ignored --nocapture
//! ```
//!
//! It imports the audio from that stick's `Contents/` into a fresh library,
//! exports it with Kaldor into a temp folder and compares the two with
//! `export-verify --compare`. Grids, cues and waveforms come from different
//! analyses and are only printed; the structure must match, apart from the
//! sections listed in [`KNOWN`].

use dj_core::Beatgrid;
use dj_export::usb::{CancelToken, UsbExportOptions, UsbFormat, export_usb, plan_usb};
use dj_library::Library;
use export_verify::compare::{compare, sections};
use std::path::{Path, PathBuf};

/// Sections that differ for known reasons (golden
/// stick): PQT2 (empty extended grid, written by baken; this rekordbox
/// leaves it out), PSSI (rekordbox's phrase analysis; Kaldor has none),
/// PVDI (new in rekordbox 7, layout unknown).
const KNOWN: [&str; 3] = ["PQT2", "PSSI", "PVDI"];

/// Files only rekordbox writes: its "My Settings" and device files, which
/// Kaldor leaves out on purpose, and `exportExt.pdb` (My Tags). rekordbox
/// adds all of them to a Kaldor stick when it mounts it (a mounted Kaldor
/// stick has them, dated after Kaldor's export).
const KNOWN_FILES: [&str; 7] = [
    "DEVSETTING.DAT",
    "DJMMYSETTING.DAT",
    "MYSETTING.DAT",
    "MYSETTING2.DAT",
    "djprofile.nxs",
    "extracted/gcred.dat",
    "rekordbox/exportExt.pdb",
];

fn golden() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/stick-comparison/REKORDBOX")
}

fn audio(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            audio(&p, out);
        } else if dj_library::is_audio_file(&p) {
            out.push(p);
        }
    }
}

#[test]
#[ignore = "needs a reference stick in local/stick-comparison"]
fn kaldor_stick_has_rekordbox_structure() {
    let golden = golden();
    assert!(
        golden.join("PIONEER").is_dir(),
        "no reference stick at {}",
        golden.display()
    );
    let mut files = Vec::new();
    audio(&golden.join("Contents"), &mut files);
    assert!(!files.is_empty());

    let root = std::env::temp_dir().join(format!("kaldor-golden-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let stick = root.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    let mut lib = Library::open_in_memory().unwrap();
    let mut ids = Vec::new();
    for f in &files {
        let (t, _) = lib.upsert_track(&dj_library::read_meta(f)).unwrap();
        lib.set_beatgrid(t, &Beatgrid::new(120.0, 0.0)).unwrap();
        ids.push(t);
    }
    let c = lib.create_crate("Golden", None).unwrap();
    lib.add_to_crate(c, &ids).unwrap();
    let plan = plan_usb(
        &lib,
        &UsbExportOptions {
            device: stick.clone(),
            crates: Vec::new(),
            format: UsbFormat::Both,
            prune: false,
            artwork: false,
            work_dir: root.join("work"),
        },
    )
    .unwrap();
    let r = export_usb(&plan, &(), &CancelToken::new()).unwrap();
    assert!(r.failures.is_empty(), "{:?}", r.failures);

    let cmp = compare(&stick, &golden).unwrap();
    for l in &cmp.lines {
        println!("{l}");
    }
    println!("structure differences: {:#?}", cmp.structure);

    // Every analysis file: the same sections in the same order with the
    // same header sizes, apart from the known ones.
    let anlz = |d: &Path| {
        let mut v = Vec::new();
        fn walk(d: &Path, v: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, v);
                } else {
                    v.push(p);
                }
            }
        }
        walk(&d.join("PIONEER").join("USBANLZ"), &mut v);
        v.into_iter()
            .map(|p| {
                let rel = p.strip_prefix(d).unwrap().to_string_lossy().to_lowercase();
                (rel, p)
            })
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let (ours, theirs) = (anlz(&stick), anlz(&golden));
    assert_eq!(
        ours.keys().collect::<Vec<_>>(),
        theirs.keys().collect::<Vec<_>>(),
        "the same analysis files (same USBANLZ folders)"
    );
    for (k, p) in &ours {
        let strip = |p: &Path| {
            sections(&std::fs::read(p).unwrap())
                .unwrap()
                .into_iter()
                .filter(|s| !KNOWN.contains(&s.tag.as_str()))
                .map(|s| (s.tag, s.header))
                .collect::<Vec<_>>()
        };
        assert_eq!(strip(p), strip(&theirs[k]), "{k}");
    }

    // export.pdb: the same tables, the same tracks with the same fields.
    let rest: Vec<&String> = cmp
        .structure
        .iter()
        .filter(|s| !KNOWN.iter().any(|t| s.contains(t)))
        .filter(|s| {
            !KNOWN_FILES
                .iter()
                .any(|f| *s == &format!("only on rekordbox's: PIONEER/{f}"))
        })
        .collect();
    assert!(rest.is_empty(), "{rest:#?}");
    let _ = std::fs::remove_dir_all(&root);
}
