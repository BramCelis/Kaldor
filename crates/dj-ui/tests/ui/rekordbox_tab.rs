//! The crate list's rekordbox tab: rekordbox's playlists (taken in with
//! `apply_rekordbox`, as the sync does after reading `master.db`) show in
//! their own tab, read-only, not among the user's crates. No real
//! rekordbox is read: a library in memory never looks at this computer's.

use dj_core::AppConfig;
use dj_library::interop::rekordbox_db::RekordboxDb;
use dj_library::interop::{ForeignLibrary, ForeignNode, ForeignTrack};
use dj_ui::headless::Demo;
use dj_ui::library::{CrateTab, Selection};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use std::path::PathBuf;

const SIZE: egui::Vec2 = vec2(1920.0, 1080.0);

fn rects(demo: &Demo, name: &str) -> Vec<egui::Rect> {
    demo.app
        .ui_map()
        .find(name, None)
        .iter()
        .map(|c| c.rect)
        .collect()
}

fn click(demo: &mut Demo, p: Pos2) {
    let press = move |pressed| Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
    };
    demo.run_events(7, SIZE, 1.0, |i| match i {
        0..=1 => vec![Event::PointerMoved(p)],
        2 => vec![press(true)],
        3 => vec![press(false)],
        _ => vec![],
    });
}

fn track(name: &str) -> ForeignTrack {
    ForeignTrack {
        path: PathBuf::from(format!("Z:\\no such drive\\{name}.mp3")),
        title: name.into(),
        artist: "rb".into(),
        bpm: Some(124.0),
        ..ForeignTrack::default()
    }
}

fn rekordbox(warm_up: Vec<usize>) -> RekordboxDb {
    RekordboxDb {
        library: ForeignLibrary {
            source: "rekordbox".into(),
            tracks: vec![track("Rb one"), track("Rb two")],
            playlists: vec![
                ForeignNode::Folder {
                    name: "Gigs".into(),
                    children: vec![ForeignNode::Playlist {
                        name: "Friday".into(),
                        tracks: vec![1, 0],
                    }],
                },
                ForeignNode::Playlist {
                    name: "Warm-up".into(),
                    tracks: warm_up,
                },
            ],
            warnings: Vec::new(),
        },
        ids: vec!["1".into(), "2".into()],
        fingerprints: vec![11, 22],
        smart_playlists: 1,
        misplaced_cues: Vec::new(),
    }
}

#[test]
fn rekordbox_playlists_have_their_own_read_only_tab() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    {
        let l = demo.app.library().unwrap();
        assert!(
            !l.rekordbox.use_default,
            "tests never read this computer's rekordbox"
        );
        l.apply_rekordbox(rekordbox(vec![0]));
        // Not among the user's crates; their own list.
        assert!(
            l.crate_list()
                .iter()
                .all(|c| c.1 != "Gigs" && c.1 != "Warm-up")
        );
        let names: Vec<&str> = l.rb_crates.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["Gigs", "Warm-up"]);
    }
    demo.run(3, SIZE, 1.0);
    assert!(
        rects(&demo, "rekordbox_collection").is_empty(),
        "the crates tab first"
    );
    assert_eq!(rects(&demo, "new_crate").len(), 1);

    // The rekordbox tab: its collection is open, Gigs > Friday and Warm-up
    // are listed; no new crate buttons.
    let tab = rects(&demo, "crate_tab_rekordbox")[0].center();
    click(&mut demo, tab);
    demo.run(2, SIZE, 1.0);
    {
        let l = demo.app.library().unwrap();
        assert_eq!(l.crate_tab, CrateTab::Rekordbox);
        assert_eq!(l.selection, Selection::Crate(l.rb_root.unwrap()));
        assert_eq!(l.view().len(), 2);
    }
    assert_eq!(rects(&demo, "rekordbox_collection").len(), 1);
    // The folder starts closed: Gigs and Warm-up.
    assert_eq!(rects(&demo, "rekordbox_row").len(), 2);
    assert!(rects(&demo, "new_crate").is_empty());

    // A click on a folder shows all its playlists' tracks (it stays
    // closed); its arrow opens it.
    let gigs = rects(&demo, "rekordbox_row")[0].center();
    click(&mut demo, gigs);
    demo.run(2, SIZE, 1.0);
    {
        let l = demo.app.library().unwrap();
        assert_eq!(l.selection, Selection::Crate(l.rb_crates[0].id));
        assert_eq!(l.view().len(), 2, "Friday's two tracks");
    }
    assert_eq!(rects(&demo, "rekordbox_row").len(), 2);
    let arrow = rects(&demo, "crate_arrow")[0].center();
    click(&mut demo, arrow);
    demo.run(2, SIZE, 1.0);
    assert_eq!(rects(&demo, "rekordbox_row").len(), 3);
    // A playlist opens with a click.
    let warm = rects(&demo, "rekordbox_row")[2].center();
    click(&mut demo, warm);
    demo.run(2, SIZE, 1.0);
    {
        let l = demo.app.library().unwrap();
        let c = l.rb_crates[1].id;
        assert_eq!(l.selection, Selection::Crate(c));
        assert_eq!(l.view().len(), 1);
        // Read-only: no reordering.
        assert!(l.reorder_blocked().is_some_and(|m| m.contains("rekordbox")));
        // rekordbox changed: the open playlist follows.
        l.apply_rekordbox(rekordbox(vec![1, 0]));
        assert_eq!(l.view().len(), 2);
    }

    // Back to the crates tab: All tracks.
    let tab = rects(&demo, "crate_tab_crates")[0].center();
    click(&mut demo, tab);
    demo.run(2, SIZE, 1.0);
    let l = demo.app.library().unwrap();
    assert_eq!(l.crate_tab, CrateTab::Crates);
    assert_eq!(l.selection, Selection::All);
}

/// A library in a file (as in the real app) still never looks at this
/// computer's rekordbox unless the app asks for it: only `dj-app`'s main
/// calls `use_rekordbox_default_folder`.
#[test]
fn tests_never_read_this_computers_rekordbox() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-rekordbox-tab");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let lib = dj_library::Library::open(&dir.join("library.db")).unwrap();
    let (_engine, handle) = dj_audio::Engine::new(dj_audio::EngineConfig::default());
    let info = dj_audio::OutputInfo {
        host: "offline".into(),
        device: "test".into(),
        sample_rate: 48_000,
        channels: 2,
        buffer_frames: Some(256),
        sample_format: "f32".into(),
    };
    let mut app = dj_ui::KaldorApp::new(handle, info, None, AppConfig::default()).with_library(lib);
    let l = app.library().unwrap();
    assert!(l.rekordbox.enabled, "on by default");
    assert!(!l.rekordbox.use_default);
    assert_eq!(l.rekordbox.dir(), None);
}

/// The browse knob in the rekordbox tab goes from the collection through
/// the rows shown; a folder starts closed and opens with a push of the
/// knob, which then turns through its playlists.
#[test]
fn the_browse_knob_walks_rekordbox_playlists() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    let l = demo.app.library().unwrap();
    l.apply_rekordbox(rekordbox(vec![0]));
    l.crate_tab = CrateTab::Rekordbox;
    let root = l.rb_root.unwrap();
    let gigs = l.rb_crates[0].id;
    let friday = l.rb_crates[0].children[0].id;
    let warm = l.rb_crates[1].id;
    assert!(l.collapsed.contains(&gigs), "a folder starts closed");
    l.open_crate(Selection::Crate(root));
    l.browse_crates = true;
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(gigs));
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(warm), "closed: passed by");
    l.move_crate(-1);
    assert!(l.toggle_selected_parent(), "the push opens it");
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(friday));
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(warm));
    l.move_crate(5);
    assert_eq!(l.selection, Selection::Crate(warm), "stays at the end");
    l.move_crate(-10);
    assert_eq!(l.selection, Selection::Crate(root));
}
