//! `kaldor --export-rekordbox-xml` end to end. Files live under target/.

use std::path::PathBuf;
use std::process::Command;

#[test]
fn exports_a_library_to_rekordbox_xml() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export-cli");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("library.db");
    {
        let mut lib = dj_library::Library::open(&db).unwrap();
        let (id, _) = lib
            .upsert_track(&dj_library::TrackMeta {
                path: dir.join("song one.mp3"),
                artist: "Artist".into(),
                title: "Song One".into(),
                duration: 180.0,
                ..Default::default()
            })
            .unwrap();
        lib.set_beatgrid(id, &dj_core::Beatgrid::new(124.0, 0.1))
            .unwrap();
        let c = lib.create_crate("Friday", None).unwrap();
        lib.add_to_crate(c, &[id]).unwrap();
    }
    let out = dir.join("rekordbox.xml");
    let run = Command::new(env!("CARGO_BIN_EXE_kaldor"))
        .arg("--library")
        .arg(&db)
        .arg("--config")
        .arg(dir.join("no-config.toml"))
        .arg("--export-rekordbox-xml")
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    assert!(String::from_utf8_lossy(&run.stdout).contains("1 tracks, 1 playlists"));
    let xml = std::fs::read_to_string(&out).unwrap();
    assert!(xml.contains("<DJ_PLAYLISTS Version=\"1.0.0\">"));
    assert!(xml.contains("Name=\"Friday\" Type=\"1\" KeyType=\"0\" Entries=\"1\""));
    assert!(xml.contains("song%20one.mp3"));
    assert!(xml.contains("Bpm=\"124.00\""));

    // A missing library is an error, not a new empty library.
    let bad = Command::new(env!("CARGO_BIN_EXE_kaldor"))
        .args(["--library"])
        .arg(dir.join("nope.db"))
        .arg("--export-rekordbox-xml")
        .arg(dir.join("x.xml"))
        .output()
        .unwrap();
    assert!(!bad.status.success());
    assert!(!dir.join("nope.db").exists());
}
