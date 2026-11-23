//! The library, second round: selecting several tracks,
//! removing them (out of a crate, or out of the library after a question,
//! as in Serato), resetting the library, and the Files panel with its
//! drops onto crates and under the crates.

use dj_core::{AppConfig, DeckId};
use dj_ui::DropTarget;
use dj_ui::headless::Demo;
use dj_ui::library::Selection;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SIZE: egui::Vec2 = vec2(1920.0, 1080.0);

fn moved(p: Pos2) -> Event {
    Event::PointerMoved(p)
}

fn button(p: Pos2, pressed: bool) -> Event {
    Event::PointerButton {
        pos: p,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
    }
}

fn key(k: Key) -> Vec<Event> {
    [true, false]
        .map(|pressed| Event::Key {
            key: k,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::default(),
        })
        .to_vec()
}

fn rects(demo: &Demo, name: &str) -> Vec<egui::Rect> {
    demo.app
        .ui_map()
        .find(name, None)
        .iter()
        .map(|c| c.rect)
        .collect()
}

fn click(demo: &mut Demo, p: Pos2) {
    demo.run_events(7, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(p)],
        2 => vec![button(p, true)],
        3 => vec![button(p, false)],
        _ => vec![],
    });
}

fn write_wav(path: &std::path::Path, seconds: f32) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..(44_100.0 * seconds) as usize * 2 {
        w.write_sample(0i16).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn ctrl_and_shift_select_several_tracks() {
    let mut demo = Demo::new(AppConfig::default());
    let l = demo.app.library().unwrap();
    let ids = l.view().to_vec();
    l.click_track(ids[0], false, false);
    l.click_track(ids[2], true, false);
    assert_eq!(l.picked_in_view(), vec![ids[0], ids[2]]);
    // Ctrl on a selected one takes it away.
    l.click_track(ids[0], true, false);
    assert_eq!(l.picked_in_view(), vec![ids[2]]);
    // Shift: the rows from the last click.
    l.click_track(ids[1], false, false);
    l.click_track(ids[3], false, true);
    assert_eq!(l.picked_in_view(), vec![ids[1], ids[2], ids[3]]);
    // A drag of a selected row carries the selection; of another, itself.
    assert_eq!(l.drag_set(ids[2]).len(), 3);
    assert_eq!(l.drag_set(ids[0]), vec![ids[0]]);
    l.select_all();
    assert_eq!(l.picked_in_view(), ids);
}

/// As in Serato: Delete in a crate takes the tracks out of the crate only;
/// in All tracks it asks, then takes them out of the library (the files
/// stay).
#[test]
fn delete_takes_tracks_out_of_the_crate_or_out_of_the_library() {
    let mut demo = Demo::new(AppConfig::default());
    let (c, total) = {
        let l = demo.app.library().unwrap();
        let c = l.crate_list()[0].0;
        l.selection = Selection::Crate(c);
        l.mark_dirty();
        (c, l.track_count())
    };
    demo.run(3, SIZE, 1.0);
    let row = rects(&demo, "track_row")[0].center();
    demo.run_events(9, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(row)],
        2 => vec![button(row, true)],
        3 => vec![button(row, false)],
        5 => key(Key::Delete),
        _ => vec![],
    });
    let l = demo.app.library().unwrap();
    assert_eq!(
        l.lib.crate_tracks(c).unwrap().len(),
        total - 1,
        "out of the crate"
    );
    assert_eq!(l.track_count(), total, "still in the library");
    assert!(l.confirm_remove.is_none(), "no question for a crate");

    // All tracks: Delete asks first.
    demo.app.library().unwrap().selection = Selection::All;
    demo.app.library().unwrap().mark_dirty();
    demo.run(3, SIZE, 1.0);
    let row = rects(&demo, "track_row")[0].center();
    demo.run_events(9, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(row)],
        2 => vec![button(row, true)],
        3 => vec![button(row, false)],
        5 => key(Key::Delete),
        _ => vec![],
    });
    let gone = demo.app.library().unwrap().confirm_remove.clone();
    assert_eq!(gone.as_ref().map(Vec::len), Some(1), "asks about one track");
    assert_eq!(demo.app.library().unwrap().track_count(), total);
    demo.run(2, SIZE, 1.0);
    let yes = rects(&demo, "remove_confirm")[0].center();
    click(&mut demo, yes);
    let l = demo.app.library().unwrap();
    assert_eq!(l.track_count(), total - 1);
    assert!(l.status.contains("the file stays"), "{}", l.status);
    assert!(l.track(gone.unwrap()[0]).is_none());
}

#[test]
fn reset_empties_the_library_and_can_forget_the_watch_folders() {
    let mut cfg = AppConfig::default();
    cfg.library.watch_folders = vec![PathBuf::from("X:/somewhere/Music")];
    let mut demo = Demo::new(cfg);
    // Track 1 on deck 1 was loaded from the library: it is forgotten too.
    demo.app.library().unwrap().confirm_reset = Some(true);
    demo.run(3, SIZE, 1.0);
    assert!(
        !rects(&demo, "reset_watch").is_empty(),
        "the watch folders are listed"
    );
    let yes = rects(&demo, "reset_confirm")[0].center();
    click(&mut demo, yes);
    let l = demo.app.library().unwrap();
    assert_eq!(l.track_count(), 0);
    assert!(l.crate_list().is_empty());
    assert!(l.status.starts_with("Library reset"), "{}", l.status);
    assert!(demo.app.config().library.watch_folders.is_empty());
}

fn music_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(dir.join("Set")).unwrap();
    write_wav(&dir.join("a.wav"), 0.2);
    write_wav(&dir.join("b.wav"), 0.3);
    write_wav(&dir.join("Set").join("c.wav"), 0.2);
    std::fs::write(dir.join("notes.txt"), b"not music").unwrap();
    dir
}

#[test]
fn the_files_panel_opens_over_the_library_and_makes_room() {
    let dir = music_dir("dj-ui-files-panel");
    let mut demo = Demo::new(AppConfig::default());
    demo.app.open_files_at(dir.clone());
    demo.run(3, SIZE, 1.0);
    assert!(!rects(&demo, "files_panel").is_empty());
    assert_eq!(demo.app.files_state(), (true, false), "1080p keeps 4 decks");
    let panel = rects(&demo, "files_panel")[0];
    let crates = rects(&demo, "all_tracks")[0];
    assert!(
        panel.bottom() < crates.top(),
        "the library is under the panel"
    );
    // Above the New crate / Files / search bar, not between it and the
    // crates.
    let bar = rects(&demo, "new_crate")[0];
    assert!(panel.bottom() <= bar.top(), "{panel:?} vs {bar:?}");
    assert!(rects(&demo, "search")[0].top() >= panel.bottom());
    // The Set folder, then a.wav and b.wav (notes.txt is not music).
    assert_eq!(rects(&demo, "files_row").len(), 1);
    // A small screen: two decks while Files is open.
    demo.run(3, vec2(1366.0, 700.0), 1.0);
    assert_eq!(demo.app.files_state(), (true, true));
    // The Files button closes it again.
    let b = rects(&demo, "files_button")[0].center();
    demo.run_events(7, vec2(1366.0, 700.0), 1.0, |i| match i {
        0..=1 => vec![moved(b)],
        2 => vec![button(b, true)],
        3 => vec![button(b, false)],
        _ => vec![],
    });
    assert!(!demo.app.files_state().0);
}

/// Press at `from`, move to `to` and release, in one run.
fn drag(demo: &mut Demo, from: Pos2, to: Pos2) {
    let mut seq = vec![vec![moved(from)], vec![button(from, true)]];
    for k in 1..=8 {
        seq.push(vec![moved(from + (to - from) * (k as f32 / 8.0))]);
    }
    seq.push(vec![moved(to)]);
    seq.push(vec![button(to, false)]);
    let n = seq.len() + 3;
    demo.run_events(n, SIZE, 1.0, |i| seq.get(i).cloned().unwrap_or_default());
}

fn wait(demo: &mut Demo, what: &str, mut done: impl FnMut(&mut Demo) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(demo) {
        assert!(Instant::now() < deadline, "{what}");
        demo.run(1, SIZE, 1.0);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn files_and_folders_drop_onto_crates_and_under_the_crates() {
    let dir = music_dir("dj-ui-files-drop");
    let mut demo = Demo::new(AppConfig::default());
    demo.app.open_files_at(dir.clone());
    demo.run(3, SIZE, 1.0);
    let row0 = rects(&demo, "files_row")[0];
    // Row 1 is a.wav (the Set folder is row 0).
    let a_row = row0.center() + vec2(0.0, row0.height());
    let demo_crate = demo.app.library().unwrap().crate_list()[0].0;
    let crate_row = rects(&demo, "crate_row")[0].center();
    drag(&mut demo, a_row, crate_row);
    let a = dir.join("a.wav");
    wait(&mut demo, "a.wav in the crate", |d| {
        let l = d.app.library().unwrap();
        l.track_of_path(&a)
            .is_some_and(|id| l.lib.crate_tracks(demo_crate).unwrap().contains(&id))
    });

    // The Set folder under the crates: a new crate "Set" with c.wav.
    let empty = rects(&demo, "new_crate_area")[0];
    let to = egui::pos2(empty.center().x, empty.top() + 12.0);
    drag(&mut demo, row0.center(), to);
    let c = dir.join("Set").join("c.wav");
    wait(&mut demo, "the Set crate with c.wav", |d| {
        let l = d.app.library().unwrap();
        let Some((set, _, _)) = l.crate_list().into_iter().find(|(_, n, _)| n == "Set") else {
            return false;
        };
        l.track_of_path(&c)
            .is_some_and(|id| l.lib.crate_tracks(set).unwrap() == vec![id])
    });
    assert_eq!(demo.app.drop_highlight(), None);
}

#[test]
fn a_file_dragged_from_the_files_panel_loads_on_a_deck() {
    let dir = music_dir("dj-ui-files-deck");
    let mut demo = Demo::new(AppConfig::default());
    demo.app.open_files_at(dir.clone());
    demo.run(3, SIZE, 1.0);
    let row0 = rects(&demo, "files_row")[0];
    let b_row = row0.center() + vec2(0.0, 2.0 * row0.height());
    let wave = demo
        .app
        .ui_map()
        .find("wave", Some(2))
        .first()
        .map(|c| c.rect.center())
        .expect("deck 2's waveform");
    demo.keep_context();
    let mut seq = vec![vec![moved(b_row)], vec![button(b_row, true)]];
    for k in 1..=8 {
        seq.push(vec![moved(b_row + (wave - b_row) * (k as f32 / 8.0))]);
    }
    let n = seq.len() + 2;
    demo.run_events(n, SIZE, 1.0, |i| seq.get(i).cloned().unwrap_or_default());
    assert_eq!(demo.app.drop_highlight(), Some(DropTarget::Deck(1)));
    demo.run_events(4, SIZE, 1.0, |i| {
        if i == 0 {
            vec![button(wave, false)]
        } else {
            vec![]
        }
    });
    wait(&mut demo, "b.wav on deck 2", |d| {
        d.app.deck_title(DeckId::ALL[1]).contains('b')
    });
}

fn ctrl_a() -> Vec<Event> {
    let m = Modifiers {
        ctrl: true,
        command: true,
        ..Default::default()
    };
    [true, false]
        .map(|pressed| Event::Key {
            key: Key::A,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: m,
        })
        .to_vec()
}

/// Ctrl+A after a click on a crate selects the crates (was the
/// crate's tracks); after a click on the empty part of the list under the
/// last track it selects the tracks (not only after a click on a track).
#[test]
fn ctrl_a_after_a_click_on_the_crate_or_under_the_tracks() {
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    // On the crate's name.
    let crate_row = rects(&demo, "crate_row")[0].left_center() + vec2(30.0, 0.0);
    demo.run_events(9, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(crate_row)],
        2 => vec![button(crate_row, true)],
        3 => vec![button(crate_row, false)],
        5 => ctrl_a(),
        _ => vec![],
    });
    let l = demo.app.library().unwrap();
    let c = l.crate_list()[0].0;
    assert_eq!(l.selection, Selection::Crate(c), "the crate is open");
    let all = l.lib.crate_tracks(c).unwrap();
    assert_eq!(
        l.picked_crates,
        vec![c],
        "Ctrl+A after a click on a crate: the crates"
    );
    assert!(l.picked_in_view().is_empty(), "not the tracks");

    // Again after a click under the last track (one track first, so the
    // test sees Ctrl+A grow the selection).
    demo.app.library().unwrap().clear_picked();
    let row = rects(&demo, "track_row")[0];
    let under = egui::pos2(row.center().x, row.top() + 12.0 * row.height());
    demo.run_events(9, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(under)],
        2 => vec![button(under, true)],
        3 => vec![button(under, false)],
        5 => ctrl_a(),
        _ => vec![],
    });
    assert_eq!(
        demo.app.library().unwrap().picked_in_view().len(),
        all.len()
    );

    // A click elsewhere (the search field): Ctrl+A is not for the list.
    demo.app.library().unwrap().clear_picked();
    let deck = rects(&demo, "new_crate")[0].center() + vec2(0.0, -200.0);
    demo.run_events(9, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(deck)],
        2 => vec![button(deck, true)],
        3 => vec![button(deck, false)],
        5 => ctrl_a(),
        _ => vec![],
    });
    assert!(demo.app.library().unwrap().picked.is_empty());
}

/// Ctrl+A again deselects; another crate starts with nothing selected; a
/// click in the library but not on a track (under the tracks) deselects,
/// a click on a deck keeps the selection.
#[test]
fn ctrl_a_toggles_and_a_click_elsewhere_or_another_crate_deselects() {
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    let row = rects(&demo, "track_row")[0];
    let total = demo.app.library().unwrap().track_count();
    // Click a track, Ctrl+A: all; Ctrl+A again: none.
    let p = row.center();
    demo.run_events(12, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(p)],
        2 => vec![button(p, true)],
        3 => vec![button(p, false)],
        5 => ctrl_a(),
        _ => vec![],
    });
    assert_eq!(demo.app.library().unwrap().picked_in_view().len(), total);
    demo.run_events(4, SIZE, 1.0, |i| if i == 0 { ctrl_a() } else { vec![] });
    assert!(
        demo.app.library().unwrap().picked_in_view().is_empty(),
        "Ctrl+A again: none"
    );

    // A selected track, then a click under the tracks: none.
    click(&mut demo, p);
    assert_eq!(demo.app.library().unwrap().picked_in_view().len(), 1);
    let under = egui::pos2(p.x, row.top() + 12.0 * row.height());
    click(&mut demo, under);
    assert!(
        demo.app.library().unwrap().picked_in_view().is_empty(),
        "under the tracks"
    );

    // A selected track, then a click on a deck (its waveform, its GRID
    // tab): still selected (while DJing the loaded track stays
    // selected, so the next one is right below it).
    click(&mut demo, p);
    let picked = demo.app.library().unwrap().picked_in_view();
    assert_eq!(picked.len(), 1);
    for name in ["wave", "tab_grid"] {
        let at = demo
            .app
            .ui_map()
            .find(name, Some(1))
            .first()
            .map(|c| c.rect.center())
            .unwrap_or_else(|| panic!("deck 1's {name}"));
        click(&mut demo, at);
        assert_eq!(
            demo.app.library().unwrap().picked_in_view(),
            picked,
            "kept after a click on the deck's {name}"
        );
    }
    // A click on the empty part of the list still clears it.
    click(&mut demo, under);
    assert!(demo.app.library().unwrap().picked_in_view().is_empty());

    // A selected track in All tracks, then the crate: nothing selected there.
    click(&mut demo, p);
    let crate_name = rects(&demo, "crate_row")[0].left_center() + vec2(30.0, 0.0);
    click(&mut demo, crate_name);
    let l = demo.app.library().unwrap();
    assert!(matches!(l.selection, Selection::Crate(_)));
    assert!(l.selected_track.is_none() && l.picked.is_empty());
}

/// The Waveform column: every analysed track gets a small waveform with
/// its hot cues; a cue saved later shows up after the deck saves it.
#[test]
fn the_track_list_shows_small_waveforms_with_hot_cues() {
    let mut demo = Demo::new(AppConfig::default());
    let id = {
        let l = demo.app.library().unwrap();
        let id = l.view()[0];
        l.lib
            .set_cues(
                id,
                &[dj_library::Cue {
                    kind: dj_library::CueKind::Hot(2),
                    position: 15.0,
                    name: String::new(),
                    color: Some(0xFF0000),
                }],
            )
            .unwrap();
        id
    };

    demo.run(4, SIZE, 1.0);
    assert!(!rects(&demo, "Waveform").is_empty(), "the column is shown");
    let l = demo.app.library().unwrap();
    let m = l.minis.get(id).flatten().expect("a small waveform");
    assert!(m.points.iter().any(|p| p.peak > 0), "the bars");
    assert_eq!(m.hot_cues.len(), 1);
    // 15 s into a 30 s demo track: the middle.
    assert!((m.hot_cues[0].0 - 0.5).abs() < 0.02, "{:?}", m.hot_cues);
    assert_eq!(m.hot_cues[0].1, egui::Color32::from_rgb(255, 0, 0));
}

fn right_click(demo: &mut Demo, p: Pos2) {
    let right = |pressed| Event::PointerButton {
        pos: p,
        button: PointerButton::Secondary,
        pressed,
        modifiers: Modifiers::default(),
    };
    demo.run_events(6, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(p)],
        2 => vec![right(true)],
        3 => vec![right(false)],
        _ => vec![],
    });
}

/// Export… in the right-click menu of a crate opens the USB export window
/// for that crate (not the one that is open in the list); on All tracks it
/// exports all crates.
#[test]
fn export_from_the_right_click_menu_of_a_crate() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    let (first, second) = {
        let l = demo.app.library().unwrap();
        l.lib.create_crate("Second", None).unwrap();
        l.reload();
        let list = l.crate_list();
        assert_eq!(list.len(), 2, "the demo crate and a second one");
        (list[0].0, list[1].0)
    };
    demo.app.library().unwrap().selection = Selection::Crate(first);
    demo.run(2, SIZE, 1.0);
    assert!(rects(&demo, "crate_export").is_empty(), "no menu yet");

    let row = rects(&demo, "crate_row")[1].left_center() + vec2(30.0, 0.0);
    right_click(&mut demo, row);
    let item = rects(&demo, "crate_export");
    assert_eq!(item.len(), 1, "Export… in the crate's menu");
    click(&mut demo, item[0].center());
    let l = demo.app.library().unwrap();
    assert!(l.usb.open, "the export window is open");
    assert_eq!(l.usb.target, vec![second], "for the right-clicked crate");
    assert_eq!(l.selection, Selection::Crate(first), "the open crate stays");
    l.usb.open = false;

    let all = rects(&demo, "all_tracks")[0].center();
    right_click(&mut demo, all);
    let item = rects(&demo, "crate_export");
    assert_eq!(item.len(), 1, "Export… in the menu of All tracks");
    click(&mut demo, item[0].center());
    let l = demo.app.library().unwrap();
    assert!(l.usb.open);
    assert!(l.usb.target.is_empty(), "all crates");
}

/// The export window: a modal in the middle of the screen with the drives
/// as tiles, the libraries as two choices and the options as switches;
/// Check stick needs a stick; Cancel and Escape close it.
#[test]
fn the_export_window_is_a_centred_modal_with_tiles() {
    use dj_export::volume::Drive;
    use dj_ui::usb_export::{DriveText, Stage};
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    {
        let l = demo.app.library().unwrap();
        let c = l.crate_list()[0].0;
        l.usb.open_for(vec![c]);
        // Made-up drives, the same on every machine; none chosen.
        l.usb.drives = vec![
            Drive {
                root: PathBuf::from("Q:\\"),
                label: "STICK".into(),
                removable: true,
            },
            Drive {
                root: PathBuf::from("R:\\"),
                label: String::new(),
                removable: false,
            },
        ];
        l.usb.drive_text = vec![
            DriveText {
                name: "STICK (Q:)".into(),
                detail: "7.4 GB free".into(),
            },
            DriveText {
                name: "Local disk (R:)".into(),
                detail: "Fixed drive".into(),
            },
        ];
        l.usb.device.clear();
    }
    demo.run(3, SIZE, 1.0);
    let win = rects(&demo, "usb_window");
    assert_eq!(win.len(), 1, "the window is open");
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SIZE);
    assert!(
        (win[0].center() - screen.center()).length() < 4.0,
        "centred: {:?}",
        win[0]
    );
    assert!(
        win[0].width() >= 600.0 && win[0].height() >= 600.0,
        "{:?}",
        win[0]
    );
    for name in [
        "usb_close",
        "usb_refresh",
        "usb_folder",
        "usb_prune",
        "usb_artwork",
    ] {
        let r = rects(&demo, name);
        assert_eq!(r.len(), 1, "{name}");
        assert!(win[0].contains_rect(r[0]), "{name} inside the window");
    }
    assert_eq!(rects(&demo, "usb_drive").len(), 2, "a tile per drive");
    assert_eq!(rects(&demo, "usb_format").len(), 2, "two library choices");

    // No stick chosen: Check stick does nothing.
    let p = rects(&demo, "usb_check")[0].center();
    click(&mut demo, p);
    assert!(matches!(
        demo.app.library().unwrap().usb.stage,
        Stage::Setup
    ));
    // A tile chooses the drive; the second choice, Device Library only.
    let p = rects(&demo, "usb_drive")[0].center();
    click(&mut demo, p);
    assert_eq!(demo.app.library().unwrap().usb.device, "Q:\\");
    let p = rects(&demo, "usb_format")[1].center();
    click(&mut demo, p);
    assert_eq!(
        demo.app.library().unwrap().usb.format,
        dj_export::usb::UsbFormat::DeviceLibrary
    );
    assert!(demo.app.library().unwrap().usb.prune);
    let p = rects(&demo, "usb_prune")[0].center();
    click(&mut demo, p);
    assert!(
        !demo.app.library().unwrap().usb.prune,
        "the switch turns it off"
    );

    // Cancel closes it; opened again, Escape closes it.
    let p = rects(&demo, "usb_cancel")[0].center();
    click(&mut demo, p);
    assert!(!demo.app.library().unwrap().usb.open);
    demo.run(2, SIZE, 1.0);
    assert!(
        rects(&demo, "usb_window").is_empty(),
        "gone from the screen"
    );
    demo.app.library().unwrap().usb.open_for(Vec::new());
    demo.run(2, SIZE, 1.0);
    demo.run_events(
        4,
        SIZE,
        1.0,
        |i| if i == 1 { key(Key::Escape) } else { vec![] },
    );
    assert!(!demo.app.library().unwrap().usb.open, "Escape closes it");
}

/// Several crates: Ctrl + click adds one (the list shows the one clicked
/// last), Shift + click selects a range, Ctrl+A every crate; Export… on a
/// selected crate exports them all; a click elsewhere goes back to the
/// open crate alone.
#[test]
fn several_crates_are_selected_and_exported_together() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    let ids = {
        let l = demo.app.library().unwrap();
        l.lib.create_crate("Second", None).unwrap();
        l.lib.create_crate("Third", None).unwrap();
        l.lib.create_crate("Fourth", None).unwrap();
        l.reload();
        l.crate_list().into_iter().map(|c| c.0).collect::<Vec<_>>()
    };
    assert_eq!(ids.len(), 4);
    demo.run(3, SIZE, 1.0);
    let row = |demo: &Demo, i: usize| rects(demo, "crate_row")[i].left_center() + vec2(30.0, 0.0);

    // A plain click opens the first; Ctrl + click adds the third.
    let p = row(&demo, 0);
    click(&mut demo, p);
    let p = row(&demo, 2);
    demo.hold_modifiers(Modifiers::CTRL);
    click(&mut demo, p);
    demo.hold_modifiers(Modifiers::NONE);
    let l = demo.app.library().unwrap();
    assert_eq!(l.selected_crates(), vec![ids[0], ids[2]]);
    assert_eq!(
        l.selection,
        Selection::Crate(ids[2]),
        "shows the one clicked last"
    );

    // Shift + click from the third to the fourth.
    let p = row(&demo, 3);
    demo.hold_modifiers(Modifiers::SHIFT);
    click(&mut demo, p);
    demo.hold_modifiers(Modifiers::NONE);
    assert_eq!(
        demo.app.library().unwrap().selected_crates(),
        vec![ids[2], ids[3]]
    );

    // Export… on a selected crate: all of them.
    let p = row(&demo, 2);
    right_click(&mut demo, p);
    let item = rects(&demo, "crate_export");
    assert_eq!(item.len(), 1);
    click(&mut demo, item[0].center());
    let l = demo.app.library().unwrap();
    assert!(l.usb.open);
    assert_eq!(l.usb.target, vec![ids[2], ids[3]], "both selected crates");
    l.usb.open = false;
    demo.run(2, SIZE, 1.0);

    // Export… on a crate outside the selection: that crate alone.
    let p = row(&demo, 1);
    right_click(&mut demo, p);
    let item = rects(&demo, "crate_export");
    click(&mut demo, item[0].center());
    let l = demo.app.library().unwrap();
    assert_eq!(l.usb.target, vec![ids[1]]);
    l.usb.open = false;
    demo.run(2, SIZE, 1.0);

    // Ctrl+A after a click on a crate: every crate; again: the open one.
    let p = row(&demo, 1);
    demo.run_events(9, SIZE, 1.0, |i| match i {
        0..=1 => vec![moved(p)],
        2 => vec![button(p, true)],
        3 => vec![button(p, false)],
        5 => ctrl_a(),
        _ => vec![],
    });
    assert_eq!(demo.app.library().unwrap().selected_crates(), ids);
    demo.run_events(4, SIZE, 1.0, |i| if i == 0 { ctrl_a() } else { vec![] });
    assert_eq!(
        demo.app.library().unwrap().selected_crates(),
        vec![ids[1]],
        "Ctrl+A again: the open crate alone"
    );

    // All selected, then a click on a track: back to the open crate.
    demo.run_events(4, SIZE, 1.0, |i| if i == 0 { ctrl_a() } else { vec![] });
    assert_eq!(demo.app.library().unwrap().selected_crates().len(), 4);
    let all = rects(&demo, "all_tracks")[0].center();
    click(&mut demo, all);
    let l = demo.app.library().unwrap();
    assert!(
        l.selected_crates().is_empty(),
        "All tracks: no crate selected"
    );
    assert_eq!(l.selection, Selection::All);
}

/// While the stick is written the window can be closed: the export goes on
/// and the top bar shows it next to the 2 / 4 deck buttons; a click there
/// opens the window again. The outcome stays in the top bar until the
/// window has shown it. Closing Kaldor meanwhile asks first.
#[test]
fn a_running_export_goes_on_in_the_top_bar() {
    use dj_ui::usb_export::{Counter, Stage};
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    {
        let l = demo.app.library().unwrap();
        let c = l.crate_list()[0].0;
        l.usb.open_for(vec![c]);
        // A running export (the sender stays, so it keeps running).
        let (tx, rx) = std::sync::mpsc::channel();
        std::mem::forget(tx);
        l.usb.stage = Stage::Exporting {
            rx,
            counter: std::sync::Arc::new(Counter::at(3, 10)),
            cancel: dj_export::usb::CancelToken::new(),
        };
        // As `start_export` does: its outcome is not seen yet.
        l.usb.result_seen = false;
    }
    demo.run(3, SIZE, 1.0);
    assert!(
        rects(&demo, "usb_progress").is_empty(),
        "not while the window is open"
    );
    let p = rects(&demo, "usb_hide")[0].center();
    click(&mut demo, p);
    let l = demo.app.library().unwrap();
    assert!(!l.usb.open, "Close closes the window");
    assert!(l.usb.exporting(), "the export goes on");
    demo.run(2, SIZE, 1.0);
    let pill = rects(&demo, "usb_progress");
    assert_eq!(pill.len(), 1, "the top bar shows the export");
    // Right of 2 / 4 and FX.
    let fx = rects(&demo, "fx_toggle")[0];
    assert!(
        pill[0].left() > fx.right() && pill[0].left() - fx.right() < 40.0,
        "next to the 2 / 4 and FX buttons: {:?} vs {fx:?}",
        pill[0]
    );
    assert!((pill[0].center().y - fx.center().y).abs() < 4.0);

    // Closing Kaldor while it writes: asked first.
    demo.app.request_close_for_test();
    assert!(demo.app.close_asked());
    demo.run(2, SIZE, 1.0);
    let k = rects(&demo, "close_cancel")[0].center();
    click(&mut demo, k);
    assert!(!demo.app.close_asked(), "Keep exporting");

    // A click on the pill opens the window on the same step; Escape
    // closes it again (the export goes on).
    let p = rects(&demo, "usb_progress")[0].center();
    click(&mut demo, p);
    assert!(demo.app.library().unwrap().usb.open);
    assert_eq!(rects(&demo, "usb_stop").len(), 1, "the writing step");
    demo.run_events(
        4,
        SIZE,
        1.0,
        |i| {
            if i == 1 { key(Key::Escape) } else { vec![] }
        },
    );
    assert!(!demo.app.library().unwrap().usb.open);

    // Done while closed: the outcome in the top bar until it is seen.
    demo.app.library().unwrap().usb.stage = Stage::Done(Ok(dj_export::usb::UsbReport {
        copied: 10,
        ..Default::default()
    }));
    demo.run(2, SIZE, 1.0);
    let pill = rects(&demo, "usb_progress");
    assert_eq!(pill.len(), 1, "the outcome waits in the top bar");
    click(&mut demo, pill[0].center());
    assert!(demo.app.library().unwrap().usb.open);
    let p = rects(&demo, "usb_done")[0].center();
    click(&mut demo, p);
    demo.run(2, SIZE, 1.0);
    assert!(rects(&demo, "usb_progress").is_empty(), "seen: gone");

    // Not clicked: "Export done" goes by itself after a while; a failure
    // stays until it is looked at.
    let long_ago = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(
        dj_ui::usb_export::DONE_PILL_SECS + 1,
    ));
    {
        let u = &mut demo.app.library().unwrap().usb;
        u.result_seen = false;
        u.finished_at = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(2));
    }
    demo.run(2, SIZE, 1.0);
    assert_eq!(rects(&demo, "usb_progress").len(), 1, "done, just now");
    demo.app.library().unwrap().usb.finished_at = long_ago;
    demo.run(2, SIZE, 1.0);
    assert!(
        rects(&demo, "usb_progress").is_empty(),
        "done, a while ago: gone by itself"
    );
    {
        let u = &mut demo.app.library().unwrap().usb;
        u.stage = Stage::Done(Err("the stick was unplugged".into()));
        u.result_seen = false;
        u.finished_at = long_ago;
    }
    demo.run(2, SIZE, 1.0);
    assert_eq!(
        rects(&demo, "usb_progress").len(),
        1,
        "a failure stays until it is seen"
    );
}

/// A message in the library's status line goes after a while (as in the
/// top bar); the same message set again shows again.
#[test]
fn library_status_messages_go_after_a_while() {
    let mut demo = Demo::new(AppConfig::default());
    demo.app.library().unwrap().status = "Added 2 tracks to Demo crate".into();
    demo.run(2, SIZE, 1.0);
    assert_eq!(rects(&demo, "library_status").len(), 1, "shown");
    demo.app
        .library()
        .unwrap()
        .age_status_for_test(dj_ui::library::STATUS_SECS + 1);
    demo.run(2, SIZE, 1.0);
    assert!(
        rects(&demo, "library_status").is_empty(),
        "gone after a while"
    );
    assert!(demo.app.library().unwrap().status.is_empty());
    demo.app.library().unwrap().status = "Added 2 tracks to Demo crate".into();
    demo.run(2, SIZE, 1.0);
    assert_eq!(
        rects(&demo, "library_status").len(),
        1,
        "the same again: shown"
    );
}

/// A track loaded on a deck is marked in the list: a badge per
/// deck at the row's right end (here decks 1 and 3, in that order), gone
/// when the deck loads something else.
#[test]
fn rows_loaded_on_a_deck_are_marked() {
    let dir = music_dir("dj-ui-row-decks");
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    let (a, b) = {
        let l = demo.app.library().unwrap();
        let a = l.add_file(&dir.join("a.wav")).expect("a.wav");
        let b = l.add_file(&dir.join("b.wav")).expect("b.wav");
        (a, b)
    };
    demo.run(2, SIZE, 1.0);
    assert!(
        rects(&demo, "row_deck").is_empty(),
        "nothing of the library loaded"
    );
    demo.app.load_library_track(DeckId::ALL[0], a);
    demo.app.load_library_track(DeckId::ALL[2], a);
    wait(&mut demo, "a.wav on decks 1 and 3", |d| {
        let s = d.app.snapshot();
        s.decks[0].loaded && s.decks[2].loaded && d.app.deck_title(DeckId::ALL[2]).contains('a')
    });
    demo.run(2, SIZE, 1.0);
    let chips = rects(&demo, "row_deck");
    assert_eq!(chips.len(), 2, "a badge for deck 1 and one for deck 3");
    assert!(
        (chips[0].center().y - chips[1].center().y).abs() < 0.5,
        "on one row"
    );
    let list = rects(&demo, "track_row")[0];
    assert!(
        chips.iter().all(|c| c.right() <= list.right()),
        "inside the row"
    );
    // Deck 3 loads b.wav: a.wav keeps deck 1's badge, b.wav gets deck 3's.
    demo.app.load_library_track(DeckId::ALL[2], b);
    wait(&mut demo, "b.wav on deck 3", |d| {
        d.app.snapshot().decks[2].loaded && d.app.deck_title(DeckId::ALL[2]).contains('b')
    });
    demo.run(2, SIZE, 1.0);
    let chips = rects(&demo, "row_deck");
    assert_eq!(chips.len(), 2);
    assert!(
        (chips[0].center().y - chips[1].center().y).abs() > 1.0,
        "now on two rows"
    );
}
