//! The crate list: a crate with sub-crates has an arrow that hides and
//! shows them; the closed crates are remembered in the config.

use dj_core::AppConfig;
use dj_ui::headless::Demo;
use dj_ui::library::Selection;
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};

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

#[test]
fn the_arrow_hides_and_shows_the_sub_crates() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    let (parent, child) = {
        let l = demo.app.library().unwrap();
        let parent = l.lib.create_crate("Tree parent", None).unwrap();
        let child = l.lib.create_crate("Tree child", Some(parent)).unwrap();
        l.reload();
        (parent, child)
    };
    demo.run(3, SIZE, 1.0);
    // A crate with sub-crates starts closed.
    let closed_rows = rects(&demo, "crate_row").len();
    let l = demo.app.library().unwrap();
    assert!(l.collapsed.contains(&parent), "closed by default");
    assert!(!l.visible_crates().contains(&child));
    let arrows = rects(&demo, "crate_arrow");
    assert_eq!(arrows.len(), 1, "only the crate with a sub-crate has one");
    // The arrow is on the parent's row, before its name.
    let order = demo.app.library().unwrap().visible_crates();
    let at = order.iter().position(|c| *c == parent).unwrap();
    let row = rects(&demo, "crate_row")[at];
    assert!(row.contains(arrows[0].center()));

    // Opened: the sub-crate's row comes; the config remembers it.
    click(&mut demo, arrows[0].center());
    assert_eq!(rects(&demo, "crate_row").len(), closed_rows + 1);
    assert_eq!(demo.app.config().ui.open_crates, vec![parent]);

    // Closed again while the sub-crate is open: the parent takes the
    // selection; the config forgets it.
    demo.app.library().unwrap().click_crate(child, false, false);
    let arrow = rects(&demo, "crate_arrow")[0].center();
    click(&mut demo, arrow);
    assert_eq!(rects(&demo, "crate_row").len(), closed_rows);
    let l = demo.app.library().unwrap();
    assert!(l.collapsed.contains(&parent));
    assert_eq!(l.selection, Selection::Crate(parent));
    assert!(l.crate_list().iter().any(|c| c.0 == child), "still a crate");
    assert!(demo.app.config().ui.open_crates.is_empty());

    // The browse knob does not go into the hidden sub-crate: the parent is
    // the last crate shown, so it stays there.
    let l = demo.app.library().unwrap();
    assert_eq!(l.visible_crates().last(), Some(&parent));
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(parent));

    // Renaming a sub-crate of a closed crate opens it (the text field is
    // in its row).
    l.start_rename(child);
    assert!(!l.collapsed.contains(&parent));
}

#[test]
fn opened_crates_come_back_at_the_next_start() {
    // The demo library's first crate ("Demo crate") has id 1; 999 is no
    // crate and is dropped.
    let mut cfg = AppConfig::default();
    cfg.ui.open_crates = vec![1, 999];
    let mut demo = Demo::new(cfg);
    let l = demo.app.library().unwrap();
    assert_eq!(l.crate_list()[0].1, "Demo crate");
    assert_eq!(l.opened.iter().copied().collect::<Vec<_>>(), vec![1]);
    demo.run(2, SIZE, 1.0);
    assert_eq!(demo.app.config().ui.open_crates, vec![1]);
}

/// Pushing the browse knob on a closed crate with sub-crates opens it, and
/// the knob then turns through its sub-crates; a push on any other crate
/// still switches to the tracks.
#[test]
fn pushing_the_browse_knob_opens_a_parent_crate() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    let l = demo.app.library().unwrap();
    let parent = l.lib.create_crate("Knob parent", None).unwrap();
    let a = l.lib.create_crate("Knob child A", Some(parent)).unwrap();
    let b = l.lib.create_crate("Knob child B", Some(parent)).unwrap();
    let after = l.lib.create_crate("Knob after", None).unwrap();
    l.reload();
    l.browse_crates = true;
    // All tracks, Demo crate, the parent, the crate after it.
    l.move_crate(2);
    assert_eq!(l.selection, Selection::Crate(parent));
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(after), "closed: passed by");
    l.move_crate(-1);
    // The push opens it; the knob goes into it.
    assert!(l.toggle_selected_parent());
    assert!(!l.collapsed.contains(&parent));
    assert!(l.browse_crates, "still in the crates");
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(a));
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(b));
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(after));
    // On a crate without sub-crates the push does what it did: to the
    // tracks.
    assert!(!l.toggle_selected_parent());
    // On the open parent a push closes it again.
    l.move_crate(-3);
    assert_eq!(l.selection, Selection::Crate(parent));
    assert!(l.toggle_selected_parent());
    assert!(l.collapsed.contains(&parent), "closed again");
    l.move_crate(1);
    assert_eq!(l.selection, Selection::Crate(after), "closed: passed by");
}

/// The browse knob in the crate list scrolls the list with it: the crate
/// it selects is on screen, also far down a long list (it was not).
#[test]
fn the_browse_knob_scrolls_the_crate_list() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    let ids: Vec<_> = {
        let l = demo.app.library().unwrap();
        let ids = (0..60)
            .map(|i| {
                l.lib
                    .create_crate(&format!("Knob crate {i:02}"), None)
                    .unwrap()
            })
            .collect();
        l.reload();
        ids
    };
    demo.run(3, SIZE, 1.0);
    // The crate list's visible part: under its tabs, down to the library's
    // bottom (where the track list ends too).
    let bottom = demo
        .app
        .library()
        .unwrap()
        .drop_areas
        .list
        .unwrap()
        .bottom();
    let top = rects(&demo, "crate_tab_crates")[0].bottom();
    let on_screen = |demo: &mut Demo, id| {
        let order = demo.app.library().unwrap().visible_crates();
        let at = order.iter().position(|c| *c == id).unwrap();
        let row = rects(demo, "crate_row")[at];
        row.top() >= top && row.bottom() <= bottom + 1.0
    };
    assert!(
        !on_screen(&mut demo, ids[59]),
        "far down: not on screen at first"
    );
    {
        let l = demo.app.library().unwrap();
        l.browse_crates = true;
        l.move_crate(1000);
    }
    demo.run(4, SIZE, 1.0);
    assert_eq!(
        demo.app.library().unwrap().selection,
        Selection::Crate(ids[59])
    );
    assert!(on_screen(&mut demo, ids[59]), "the knob's crate is shown");
    // And back up.
    demo.app.library().unwrap().move_crate(-58);
    demo.run(4, SIZE, 1.0);
    assert!(on_screen(&mut demo, ids[1]));
}

/// A crate with sub-crates is a folder: opened, it shows its crates'
/// tracks, can not be put in order, and files dropped on it go into the
/// library only.
#[test]
fn a_folder_shows_its_crates_tracks_and_takes_none() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    let l = demo.app.library().unwrap();
    let tracks: Vec<_> = l.view().to_vec();
    assert!(tracks.len() >= 2, "the demo library has tracks");
    let folder = l.lib.create_crate("Folder", None).unwrap();
    let a = l.lib.create_crate("A", Some(folder)).unwrap();
    let b = l.lib.create_crate("B", Some(folder)).unwrap();
    l.lib.add_to_crate(a, &[tracks[0]]).unwrap();
    l.lib.add_to_crate(b, &[tracks[1], tracks[0]]).unwrap();
    l.reload();
    l.open_crate(Selection::Crate(folder));
    assert_eq!(l.view().len(), 2, "both crates' tracks, each once");
    assert!(l.reorder_blocked().is_some_and(|m| m.contains("folder")));
    let target = l.import_into(&[], dj_ui::library::ImportTarget::Crate(folder));
    assert_eq!(target, None, "into the library only");
    assert!(l.status.contains("folder"));
}
