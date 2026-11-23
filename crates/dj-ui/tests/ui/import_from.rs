//! Import from… (the Import button in the library toolbar) and Save as playlist (right-click
//! a crate), driven headless: the file "picked" in the system's dialog is
//! set by the test.

use dj_core::AppConfig;
use dj_ui::browse;
use dj_ui::headless::Demo;
use dj_ui::interop_view::{SAVE_ID, Stage};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SIZE: egui::Vec2 = vec2(1920.0, 1080.0);

fn moved(p: Pos2) -> Event {
    Event::PointerMoved(p)
}

fn press(p: Pos2, b: PointerButton, pressed: bool) -> Event {
    Event::PointerButton {
        pos: p,
        button: b,
        pressed,
        modifiers: Modifiers::default(),
    }
}

fn rects(demo: &Demo, name: &str) -> Vec<egui::Rect> {
    demo.app
        .ui_map()
        .find(name, None)
        .iter()
        .map(|c| c.rect)
        .collect()
}

fn click_with(demo: &mut Demo, p: Pos2, b: PointerButton) {
    demo.run_events(7, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(p)],
        2 => vec![press(p, b, true)],
        3 => vec![press(p, b, false)],
        _ => vec![],
    });
}

fn click(demo: &mut Demo, p: Pos2) {
    click_with(demo, p, PointerButton::Primary);
}

fn wait(demo: &mut Demo, what: &str, mut done: impl FnMut(&mut Demo) -> bool) {
    let t0 = Instant::now();
    while !done(demo) {
        assert!(t0.elapsed() < Duration::from_secs(30), "waited for {what}");
        demo.run(1, SIZE, 1.0);
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn write_wav(path: &Path) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..8_820 {
        w.write_sample(0i16).unwrap();
    }
    w.finalize().unwrap();
}

fn url(p: &Path) -> String {
    let s = p.display().to_string().replace('\\', "/");
    format!("file://localhost/{}", s.trim_start_matches('/'))
}

/// A folder with one audio file and a rekordbox XML that lists it and a
/// track that is not on this computer.
fn fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let here = dir.join("here.wav");
    write_wav(&here);
    let gone = dir.join("gone.mp3");
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<DJ_PLAYLISTS Version="1.0.0">
  <PRODUCT Name="rekordbox" Version="6.8.5" Company="AlphaTheta"/>
  <COLLECTION Entries="2">
    <TRACK TrackID="1" Name="Here" Artist="Someone" Location="{}" AverageBpm="120.00" Rating="153">
      <TEMPO Inizio="0.100" Bpm="120.00" Metro="4/4" Battito="1"/>
      <POSITION_MARK Name="Drop" Type="0" Start="1.500" Num="0"/>
    </TRACK>
    <TRACK TrackID="2" Name="Gone" Artist="Nobody" Location="{}" TotalTime="200"/>
  </COLLECTION>
  <PLAYLISTS>
    <NODE Type="0" Name="ROOT" Count="1">
      <NODE Name="Friday" Type="1" KeyType="0" Entries="2">
        <TRACK Key="2"/>
        <TRACK Key="1"/>
      </NODE>
    </NODE>
  </PLAYLISTS>
</DJ_PLAYLISTS>"#,
        url(&here),
        url(&gone)
    );
    let file = dir.join("rekordbox.xml");
    std::fs::write(&file, xml).unwrap();
    (dir, here, file)
}

#[test]
fn import_from_rekordbox_through_the_window_then_save_a_crate_as_a_playlist() {
    let (dir, here, xml) = fixture("dj-ui-import-from");
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    let before = demo.app.library().unwrap().track_count();

    // The Import button in the library toolbar: the three imports are in
    // its menu.
    let button = rects(&demo, "import_button")[0].center();
    click(&mut demo, button);
    for id in ["import_rekordbox", "import_itunes", "import_playlist"] {
        assert_eq!(rects(&demo, id).len(), 1, "{id} in the menu");
    }
    let item = rects(&demo, "import_rekordbox")[0].center();
    click(&mut demo, item);
    // (Dialogs are off in tests: the file is picked here.)
    browse::set_for_test("import_rekordbox", xml.clone());
    wait(&mut demo, "the file to be read", |d| {
        matches!(d.app.library().unwrap().foreign.stage, Stage::Ready(_))
    });
    demo.run(2, SIZE, 1.0);
    if let Stage::Ready(r) = &demo.app.library().unwrap().foreign.stage {
        assert_eq!(r.foreign.tracks.len(), 2);
        assert_eq!((r.not_found(), r.playlists()), (1, 1));
        assert_eq!(r.root, "rekordbox");
    }
    let yes = rects(&demo, "import_confirm");
    assert_eq!(yes.len(), 1, "the Import window asks");
    click(&mut demo, yes[0].center());

    let l = demo.app.library().unwrap();
    let Stage::Done { report, .. } = &l.foreign.stage else {
        panic!("the report is shown");
    };
    assert_eq!(
        (report.tracks_added, report.missing_files, report.cues),
        (2, 1, 1)
    );
    assert!(
        l.status.starts_with("Imported from rekordbox"),
        "{}",
        l.status
    );
    assert_eq!(l.track_count(), before + 2);
    let id = l.track_of_path(&here).expect("the track is in the library");
    assert_eq!(l.track(id).unwrap().rating, 3);
    assert_eq!(l.lib.cues(id).unwrap().len(), 1);
    let tree = l.lib.crate_tree().unwrap();
    let root = tree.iter().find(|c| c.name == "rekordbox").unwrap();
    assert_eq!(root.children[0].name, "Friday");
    let friday = root.children[0].id;
    assert_eq!(l.lib.crate_tracks(friday).unwrap().len(), 2);

    demo.run(2, SIZE, 1.0);
    let close = rects(&demo, "import_close");
    click(&mut demo, close[0].center());
    assert!(matches!(
        demo.app.library().unwrap().foreign.stage,
        Stage::Idle
    ));

    // Save Friday as a playlist from its right-click menu (its crate
    // "rekordbox" starts closed: open it first).
    {
        let l = demo.app.library().unwrap();
        let friday = l
            .crate_list()
            .into_iter()
            .find(|c| c.1 == "Friday")
            .unwrap()
            .0;
        l.reveal_crate(friday);
    }
    demo.run(2, SIZE, 1.0);
    let rows = rects(&demo, "crate_row");
    let l = demo.app.library().unwrap();
    let friday = l
        .crate_list()
        .into_iter()
        .find(|c| c.1 == "Friday")
        .unwrap()
        .0;
    let at = l
        .visible_crates()
        .iter()
        .position(|c| *c == friday)
        .unwrap();
    click_with(
        &mut demo,
        rows[at].left_center() + vec2(40.0, 0.0),
        PointerButton::Secondary,
    );
    let save = rects(&demo, "crate_save_playlist");
    assert_eq!(save.len(), 1, "Save as playlist… in the crate's menu");
    click(&mut demo, save[0].center());
    assert_eq!(demo.app.library().unwrap().foreign.save_crate, Some(friday));
    let out = dir.join("Friday.m3u8");
    browse::set_for_test(SAVE_ID, out.clone());
    demo.run(2, SIZE, 1.0);
    let text = std::fs::read_to_string(&out).expect("the playlist is written");
    assert!(text.starts_with("#EXTM3U"));
    assert_eq!(text.matches("#EXTINF").count(), 2, "{text}");
    assert!(text.contains("here.wav") && text.contains("gone.mp3"));
}

/// A file that is not what it says: the window says why, Close closes it.
#[test]
fn a_wrong_file_says_why() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-import-wrong");
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("not-a-library.xml");
    std::fs::write(&f, "<html><body>hello</body></html>").unwrap();
    let mut demo = Demo::new(AppConfig::default());
    demo.run(2, SIZE, 1.0);
    demo.app
        .library()
        .unwrap()
        .foreign
        .start(dj_ui::interop_view::Source::Itunes, f);
    wait(&mut demo, "the read to fail", |d| {
        matches!(d.app.library().unwrap().foreign.stage, Stage::Failed(_))
    });
    demo.run(2, SIZE, 1.0);
    let close = rects(&demo, "import_close");
    assert_eq!(close.len(), 1);
    click(&mut demo, close[0].center());
    assert!(matches!(
        demo.app.library().unwrap().foreign.stage,
        Stage::Idle
    ));
}
