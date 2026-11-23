//! The library track and crate lists: drop feedback on crates, the # and
//! file-found columns, choosing the columns, and tracks whose file is gone.

use dj_core::{AppConfig, DeckId};
use dj_library::Library;
use dj_ui::DropTarget;
use dj_ui::file_check::FileState;
use dj_ui::headless::Demo;
use dj_ui::library::{LibraryUi, SortKey};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const SIZE: egui::Vec2 = vec2(1600.0, 950.0);

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

fn rects(demo: &Demo, name: &str) -> Vec<egui::Rect> {
    demo.app
        .ui_map()
        .find(name, None)
        .iter()
        .map(|c| c.rect)
        .collect()
}

/// Press on `from` and move to `to` in steps (the button stays down).
fn drag_to(demo: &mut Demo, from: Pos2, to: Pos2) {
    let mut seq = vec![vec![moved(from)], vec![button(from, true)]];
    for k in 1..=6 {
        seq.push(vec![moved(from + (to - from) * (k as f32 / 6.0))]);
    }
    seq.push(vec![moved(to)]);
    let n = seq.len() + 2;
    demo.run_events(n, SIZE, 1.0, |i| seq.get(i).cloned().unwrap_or_default());
}

fn release(demo: &mut Demo, at: Pos2) {
    demo.run_events(4, SIZE, 1.0, |i| {
        if i == 0 {
            vec![button(at, false)]
        } else {
            vec![]
        }
    });
}

#[test]
fn dragging_a_track_onto_a_crate_outlines_it_then_adds_the_track() {
    let mut demo = Demo::new(AppConfig::default());
    // The drag lives in egui's context: keep it from run to run.
    demo.keep_context();
    let peak = {
        let l = demo.app.library().unwrap();
        let id = l.lib.create_crate("Peak time", None).unwrap();
        l.reload();
        id
    };
    demo.run(3, SIZE, 1.0);
    let (track, k) = {
        let l = demo.app.library().unwrap();
        let k = l
            .crate_list()
            .iter()
            .position(|(id, _, _)| *id == peak)
            .unwrap();
        (l.view()[0], k)
    };
    let target = rects(&demo, "crate_row")[k].center();
    let row = rects(&demo, "track_row")[0].center();

    drag_to(&mut demo, row, target);
    assert_eq!(demo.app.drop_highlight(), Some(DropTarget::Crate(peak)));
    let drop = demo.app.library().unwrap().crate_drop.unwrap();
    assert!(!drop.already);
    assert!(drop.rect.contains(target));
    release(&mut demo, target);
    let l = demo.app.library().unwrap();
    assert_eq!(l.lib.crate_tracks(peak).unwrap(), vec![track]);
    assert!(l.status.starts_with("Added"), "{}", l.status);
    assert!(l.crate_flash.is_some_and(|(c, _)| c == peak));
    assert_eq!(demo.app.drop_highlight(), None, "gone after the drop");

    // Again onto the same crate: marked "already in", not added twice.
    drag_to(&mut demo, row, target);
    assert!(
        demo.app
            .library()
            .unwrap()
            .crate_drop
            .is_some_and(|d| d.already)
    );
    release(&mut demo, target);
    let l = demo.app.library().unwrap();
    assert_eq!(l.lib.crate_tracks(peak).unwrap(), vec![track]);
    assert!(l.status.contains("already in"), "{}", l.status);
}

#[test]
fn the_columns_follow_the_setting() {
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    for shown in ["#", "Found", "Artist", "Title", "BPM", "Key", "Time"] {
        assert!(!rects(&demo, shown).is_empty(), "{shown} by default");
    }
    assert!(rects(&demo, "Plays").is_empty());
    assert!(!rects(&demo, "columns_menu").is_empty());

    let mut cfg = AppConfig::default();
    cfg.ui.library_columns = ["number", "title", "plays", "location", "no-such-column"]
        .map(String::from)
        .to_vec();
    let mut demo = Demo::new(cfg);
    demo.run(3, SIZE, 1.0);
    for shown in ["#", "Title", "Plays", "Location"] {
        assert!(!rects(&demo, shown).is_empty(), "{shown}");
    }
    for hidden in ["Artist", "Found", "BPM"] {
        assert!(rects(&demo, hidden).is_empty(), "{hidden}");
    }
    // In the order of the setting.
    assert!(rects(&demo, "Title")[0].left() < rects(&demo, "Plays")[0].left());
    // # sorts by the list order, again reverses it.
    let p = rects(&demo, "#")[0].center();
    let click = |demo: &mut Demo| {
        demo.run_events(6, SIZE, 1.0, |i| match i {
            0 | 1 => vec![moved(p)],
            2 => vec![button(p, true)],
            3 => vec![button(p, false)],
            _ => vec![],
        })
    };
    click(&mut demo);
    let l = demo.app.library().unwrap();
    assert_eq!(l.sort.0, SortKey::Order);
    let first = l.view().to_vec();
    let up = l.sort.1;
    click(&mut demo);
    let l = demo.app.library().unwrap();
    assert_eq!(l.sort, (SortKey::Order, !up));
    let mut second = l.view().to_vec();
    second.reverse();
    assert_eq!(first, second);
}

#[test]
fn a_track_whose_file_is_gone_is_not_loaded() {
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    let id = demo.app.library().unwrap().view()[0];
    demo.app
        .library()
        .unwrap()
        .files
        .set(id, FileState::DriveMissing);
    let d = DeckId::ALL[0];
    let before = demo.app.deck_title(d).to_string();
    demo.app.load_library_track(d, id);
    demo.run(3, SIZE, 1.0);
    assert!(
        demo.app.status_line().starts_with("Not loaded"),
        "{}",
        demo.app.status_line()
    );
    assert_eq!(demo.app.deck_title(d), before, "the deck keeps its track");
}

#[test]
fn the_library_checks_its_files_in_the_background() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-file-check");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    std::fs::create_dir_all(&dir).unwrap();
    let here = dir.join("here.wav");
    std::fs::write(&here, b"not really audio").unwrap();
    let mut lib = Library::open_in_memory().unwrap();
    let meta = |p: &PathBuf| dj_library::TrackMeta {
        path: p.clone(),
        title: "t".into(),
        ..Default::default()
    };
    let (a, _) = lib.upsert_track(&meta(&here)).unwrap();
    let (b, _) = lib
        .upsert_track(&meta(&dir.join("moved away.wav")))
        .unwrap();
    let mut l = LibraryUi::with_settings(
        lib,
        dj_analysis::AnalysisOptions::default(),
        1,
        false,
        false,
    );
    let wait = |l: &mut LibraryUi, id, want: FileState| {
        let deadline = Instant::now() + Duration::from_secs(20);
        while l.file_state(id) != want {
            assert!(Instant::now() < deadline, "track {id}: not {want:?}");
            l.poll();
            std::thread::sleep(Duration::from_millis(10));
        }
    };
    wait(&mut l, a, FileState::Found);
    wait(&mut l, b, FileState::Missing);
    // The stick is pulled (the file goes): a row on screen shows it soon.
    l.visible_rows = vec![a];
    std::fs::remove_file(&here).unwrap();
    wait(&mut l, a, FileState::Missing);
    // Sorting by it puts the found ones first.
    std::fs::write(&here, b"back").unwrap();
    wait(&mut l, a, FileState::Found);
    l.sort = (SortKey::Found, true);
    l.mark_dirty();
    assert_eq!(l.view(), &[a, b]);
    std::fs::remove_dir_all(&dir).ok();
}

/// Press at `from`, move to `to` in steps and release, in one run.
fn drag_and_drop(demo: &mut Demo, from: Pos2, to: Pos2) {
    let mut seq = vec![vec![moved(from)], vec![button(from, true)]];
    for k in 1..=6 {
        seq.push(vec![moved(from + (to - from) * (k as f32 / 6.0))]);
    }
    seq.push(vec![moved(to)]);
    seq.push(vec![button(to, false)]);
    let n = seq.len() + 3;
    demo.run_events(n, SIZE, 1.0, |i| seq.get(i).cloned().unwrap_or_default());
}

fn column_ids(demo: &Demo) -> Vec<String> {
    demo.app.config().ui.library_columns.clone()
}

#[test]
fn columns_move_by_dragging_their_header() {
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    let bpm = rects(&demo, "BPM")[0];
    let artist = rects(&demo, "Artist")[0];
    drag_and_drop(
        &mut demo,
        bpm.center(),
        egui::pos2(artist.left() + 3.0, artist.center().y),
    );
    let ids = column_ids(&demo);
    assert_eq!(&ids[..5], &["number", "found", "bpm", "artist", "title"]);
    assert_eq!(
        demo.app.library().unwrap().sort.0,
        SortKey::Added,
        "a drag does not sort"
    );
    // And to the end.
    let bpm = rects(&demo, "BPM")[0];
    let last = *rects(&demo, "col_resize").last().unwrap();
    drag_and_drop(&mut demo, bpm.center(), last.center() + vec2(-2.0, 0.0));
    assert_eq!(column_ids(&demo).last().map(String::as_str), Some("bpm"));
}

#[test]
fn columns_resize_by_dragging_their_edge_and_fit_again_on_double_click() {
    let mut demo = Demo::new(AppConfig::default());
    demo.run(3, SIZE, 1.0);
    // Title is the fourth column shown: its right edge.
    let title = rects(&demo, "Title")[0];
    let edge = rects(&demo, "col_resize")[3];
    assert!((edge.center().x - title.right()).abs() < 1.0);
    drag_and_drop(&mut demo, edge.center(), edge.center() + vec2(-80.0, 0.0));
    let w = demo.app.config().ui.library_column_widths["title"];
    assert!(
        (w - (title.width() - 80.0)).abs() < 3.0,
        "{w} vs {}",
        title.width()
    );
    demo.run(2, SIZE, 1.0);
    let now = rects(&demo, "Title")[0];
    assert!((now.width() - w).abs() < 1.0);
    // Double-click the edge: it fits itself again.
    let edge = rects(&demo, "col_resize")[3].center();
    demo.run_events(8, SIZE, 1.0, |i| match i {
        0 => vec![moved(edge)],
        1 | 3 => vec![button(edge, true)],
        2 | 4 => vec![button(edge, false)],
        _ => vec![],
    });
    assert!(demo.app.config().ui.library_column_widths.is_empty());
}

#[test]
fn tracks_in_a_crate_move_by_dragging() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    let (c, before) = {
        let l = demo.app.library().unwrap();
        let c = l.crate_list()[0].0;
        l.selection = dj_ui::library::Selection::Crate(c);
        l.sort = (SortKey::Order, true);
        l.mark_dirty();
        (c, l.lib.crate_tracks(c).unwrap())
    };
    assert_eq!(before.len(), 4);
    demo.run(3, SIZE, 1.0);
    let row0 = rects(&demo, "track_row")[0];
    let pitch = demo.app.library().unwrap().row_pitch;
    // Row 1 down to between rows 3 and 4 (gap 3).
    let from = row0.center();
    let to = egui::pos2(from.x, row0.top() + 3.0 * pitch - 1.0);
    drag_to(&mut demo, from, to);
    let l = demo.app.library().unwrap();
    assert_eq!(l.list_drop, Some(3));
    assert_eq!(l.list_hint.as_deref(), Some("Move to position 3"));
    release(&mut demo, to);
    let after = demo.app.library().unwrap().lib.crate_tracks(c).unwrap();
    assert_eq!(after, vec![before[1], before[2], before[0], before[3]]);

    // Sorted by BPM: no new order, the label says why.
    demo.app.library().unwrap().sort = (SortKey::Bpm, true);
    demo.app.library().unwrap().mark_dirty();
    demo.run(2, SIZE, 1.0);
    drag_to(&mut demo, from, to);
    let l = demo.app.library().unwrap();
    assert_eq!(l.list_drop, None);
    assert_eq!(
        l.list_hint.as_deref(),
        Some("Sort by # to change the order")
    );
    release(&mut demo, to);
    assert_eq!(
        demo.app.library().unwrap().lib.crate_tracks(c).unwrap(),
        after
    );
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
fn a_moved_file_is_relinked_in_the_locate_window() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-locate");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("found again.wav");
    write_wav(&file, 0.2);
    let mut demo = Demo::new(AppConfig::default());
    let id = {
        let l = demo.app.library().unwrap();
        let id = l.view()[0];
        l.files.set(id, FileState::Missing);
        l.open_locate(id);
        id
    };
    demo.run(3, SIZE, 1.0);
    let field = rects(&demo, "locate_path")[0].center();
    let relink = rects(&demo, "locate_go")[0].center();
    // Explorer's "Copy as path" puts quotes around it.
    let pasted = format!("\"{}\"", file.display());
    demo.run_events(12, SIZE, 1.0, |i| match i {
        0..=2 => vec![moved(field)],
        3 => vec![button(field, true)],
        4 => vec![button(field, false)],
        5 => vec![Event::Text(pasted.clone())],
        6 => vec![moved(relink)],
        7 => vec![button(relink, true)],
        8 => vec![button(relink, false)],
        _ => vec![],
    });
    let l = demo.app.library().unwrap();
    assert!(
        l.locate.as_ref().is_some_and(|w| w.done),
        "the result stays on screen: {:?}",
        l.locate
    );
    assert_eq!(l.track(id).unwrap().meta.path, file);
    assert_eq!(l.file_state(id), FileState::Found);
    assert!(l.status.starts_with("Relinked"), "{}", l.status);
    // The window shows it until Done.
    demo.run(2, SIZE, 1.0);
    assert!(!rects(&demo, "locate_result").is_empty());
    assert!(rects(&demo, "locate_path").is_empty(), "no field any more");
    let done = rects(&demo, "locate_done")[0].center();
    demo.run_events(8, SIZE, 1.0, |i| match i {
        0..=2 => vec![moved(done)],
        3 => vec![button(done, true)],
        4 => vec![button(done, false)],
        _ => vec![],
    });
    assert!(demo.app.library().unwrap().locate.is_none());
}

#[test]
fn a_folder_search_relinks_every_moved_file_it_can_tell_apart() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-find-moved");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let old = dir.join("old");
    let new = dir.join("new");
    let sub = new.join("sub");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::create_dir_all(&sub).unwrap();
    let mut lib = Library::open_in_memory().unwrap();
    let mut ids = Vec::new();
    for (name, secs) in [("a.wav", 0.1), ("b.wav", 0.2), ("c.wav", 0.3)] {
        let p = old.join(name);
        write_wav(&p, secs);
        let meta = dj_library::read_meta(&p);
        ids.push(lib.upsert_track(&meta).unwrap().0);
    }
    // a and b moved; c moved and has a copy of the same size next to it,
    // so it is left alone.
    std::fs::rename(old.join("a.wav"), sub.join("a.wav")).unwrap();
    std::fs::rename(old.join("b.wav"), new.join("b.wav")).unwrap();
    std::fs::copy(old.join("c.wav"), sub.join("c.wav")).unwrap();
    std::fs::rename(old.join("c.wav"), new.join("c.wav")).unwrap();
    let mut l = LibraryUi::with_settings(
        lib,
        dj_analysis::AnalysisOptions::default(),
        1,
        false,
        false,
    );
    l.start_moved_search(new.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !l.status.starts_with("Found") {
        assert!(Instant::now() < deadline, "the search did not finish");
        l.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(l.track(ids[0]).unwrap().meta.path, sub.join("a.wav"));
    assert_eq!(l.track(ids[1]).unwrap().meta.path, new.join("b.wav"));
    assert_eq!(l.track(ids[2]).unwrap().meta.path, old.join("c.wav"));
    assert!(l.status.contains("Found 2 of 3"), "{}", l.status);
    assert!(
        l.status.contains("1 have more than one match"),
        "{}",
        l.status
    );
}

#[test]
fn a_file_picked_with_browse_relinks_at_once() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-browse");
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("picked.wav");
    write_wav(&file, 0.2);
    let mut demo = Demo::new(AppConfig::default());
    let id = {
        let l = demo.app.library().unwrap();
        let id = l.view()[1];
        l.files.set(id, FileState::DriveMissing);
        l.open_locate(id);
        id
    };
    demo.run(3, SIZE, 1.0);
    assert!(!rects(&demo, "locate_browse").is_empty());
    assert!(!rects(&demo, "locate_go").is_empty());
    // What the system's dialog would answer (tests never open one).
    dj_ui::browse::set_for_test("locate_file", file.clone());
    demo.run(3, SIZE, 1.0);
    let l = demo.app.library().unwrap();
    assert!(l.locate.as_ref().is_some_and(|w| w.done));
    assert_eq!(l.track(id).unwrap().meta.path, file);
}

/// The # stays with the track (its place in the crate's order) whatever the
/// list is sorted or searched by, as in Serato.
#[test]
fn the_number_stays_with_the_track_when_sorting_and_searching() {
    let mut demo = Demo::new(AppConfig::default());
    let l = demo.app.library().unwrap();
    let c = l.crate_list()[0].0;
    l.selection = dj_ui::library::Selection::Crate(c);
    l.sort = (SortKey::Order, true);
    l.mark_dirty();
    let order = l.view().to_vec();
    // A setlist order: the last track first.
    l.move_in_view(order[3], 0).unwrap();
    let set = l.view().to_vec();
    assert_eq!(set[0], order[3]);
    let numbers: Vec<Option<usize>> = set.iter().map(|id| l.order_number(*id)).collect();
    assert_eq!(numbers, vec![Some(1), Some(2), Some(3), Some(4)]);
    // Sorted by BPM: other rows, same numbers per track.
    l.sort = (SortKey::Bpm, false);
    l.mark_dirty();
    let by_bpm = l.view().to_vec();
    assert_ne!(by_bpm, set);
    for (k, id) in set.iter().enumerate() {
        assert_eq!(l.order_number(*id), Some(k + 1));
    }
    // Searched: the one hit keeps its number.
    l.search = "Concrete".into();
    let hits = l.view().to_vec();
    assert_eq!(hits.len(), 1);
    let k = set.iter().position(|id| *id == hits[0]).unwrap();
    assert_eq!(l.order_number(hits[0]), Some(k + 1));
    // Back to # (search cleared): the setlist order is still there.
    l.search.clear();
    l.sort = (SortKey::Order, true);
    l.mark_dirty();
    assert_eq!(l.view(), &set[..]);
}

/// Relinking one file of a music folder that moved as a whole (another
/// drive letter, another parent folder) finds the others too, also in
/// other subfolders.
#[test]
fn relinking_one_file_finds_the_others_that_moved_the_same_way() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("dj-ui-moved-together");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).unwrap();
    }
    let old = dir.join("old").join("Music");
    for sub in ["House", "Techno"] {
        std::fs::create_dir_all(old.join(sub)).unwrap();
    }
    let mut lib = Library::open_in_memory().unwrap();
    let mut ids = Vec::new();
    for (sub, name, secs) in [("House", "a.wav", 0.1), ("Techno", "b.wav", 0.2)] {
        let p = old.join(sub).join(name);
        write_wav(&p, secs);
        ids.push(lib.upsert_track(&dj_library::read_meta(&p)).unwrap().0);
    }
    // The whole music folder moves somewhere else.
    let new = dir.join("stick").join("Music");
    std::fs::create_dir_all(dir.join("stick")).unwrap();
    std::fs::rename(&old, &new).unwrap();
    let mut l = LibraryUi::with_settings(
        lib,
        dj_analysis::AnalysisOptions::default(),
        1,
        false,
        false,
    );
    let deadline = Instant::now() + Duration::from_secs(20);
    while l.missing_count() < 2 {
        assert!(Instant::now() < deadline, "the files were not checked");
        l.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    l.open_locate(ids[0]);
    l.locate_with(&new.join("House").join("a.wav"));
    assert!(
        l.locate.as_ref().is_some_and(|w| w.done),
        "the result is shown"
    );
    while !l.status.contains("more found") {
        assert!(
            Instant::now() < deadline,
            "no search after the relink: {}",
            l.status
        );
        l.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        l.track(ids[1]).unwrap().meta.path,
        new.join("Techno").join("b.wav")
    );
    assert!(l.status.contains("1 more found"), "{}", l.status);
    // The window's second line says it.
    assert_eq!(
        l.locate.as_ref().map(|w| w.others.as_str()),
        Some("1 more missing track found and relinked (0 still not found).")
    );

    // A folder given to the window is searched.
    let other = dir.join("elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    std::fs::rename(new.join("Techno").join("b.wav"), other.join("b.wav")).unwrap();
    l.files.set(ids[1], FileState::Missing);
    l.open_locate(ids[1]);
    l.locate_with(&other);
    while l.track(ids[1]).unwrap().meta.path != other.join("b.wav") {
        assert!(
            Instant::now() < deadline,
            "the folder was not searched: {}",
            l.status
        );
        l.poll();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        l.locate
            .as_ref()
            .is_some_and(|w| w.done && w.others.starts_with("Found 1")),
        "{:?}",
        l.locate
    );
}

/// Scrolled to the end, the last track's row is whole: its bottom edge is
/// inside the list's visible area (it was cut off by a few pixels).
#[test]
fn the_last_row_is_not_cut_off() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    {
        let l = demo.app.library().unwrap();
        let metas: Vec<dj_library::TrackMeta> = (0..80)
            .map(|i| dj_library::TrackMeta {
                path: PathBuf::from(format!("Z:\nowhere\row {i:02}.mp3")),
                title: format!("Row {i:02}"),
                ..Default::default()
            })
            .collect();
        l.lib.upsert_tracks(&metas).unwrap();
        l.reload();
    }
    let check = |demo: &mut Demo, size: egui::Vec2, how: &str| {
        let found = demo.app.ui_map().find("track_row_last", None);
        assert!(
            !found.is_empty(),
            "{size:?} {how}: the last row is not shown"
        );
        let last = found[0].rect;
        let list = demo.app.library().unwrap().drop_areas.list.unwrap();
        eprintln!("{size:?} {how}: last row {last:?}, list {list:?}");
        assert!(
            last.bottom() <= list.bottom() - dj_ui::library::LIST_END_PAD + 0.5,
            "{size:?} {how}: the last row ends at {} (the list ends at {}, with room under it)",
            last.bottom(),
            list.bottom()
        );
    };
    for (size, ppp) in [
        (vec2(1600.0, 950.0), 1.0),
        (vec2(1366.0, 768.0), 1.0),
        (vec2(1920.0, 1080.0), 1.25),
        (vec2(1920.0, 1080.0), 1.5),
    ] {
        demo.run(3, size, ppp);
        // The mouse wheel, far down.
        let at = demo
            .app
            .library()
            .unwrap()
            .drop_areas
            .list
            .unwrap()
            .center();
        demo.run_events(30, size, ppp, |i| match i {
            0 | 1 => vec![moved(at)],
            2 => vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -100_000.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Modifiers::default(),
            }],
            _ => vec![],
        });
        check(&mut demo, size, "wheel");
        // Back up, then to the last track as the browse knob goes there.
        demo.run_events(30, size, ppp, |i| match i {
            0 => vec![Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, 100_000.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Wheel,
                modifiers: Modifiers::default(),
            }],
            _ => vec![],
        });
        demo.app.library().unwrap().move_selection(-1000);
        demo.run(3, size, ppp);
        demo.app.library().unwrap().move_selection(1000);
        demo.run(4, size, ppp);
        check(&mut demo, size, "knob");
    }
}
