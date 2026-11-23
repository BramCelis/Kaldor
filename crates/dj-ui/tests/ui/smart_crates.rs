//! Smart crates: made with the button next to New crate in the smart
//! crate window, shown with the matching tracks, edited again from the
//! crate's right-click menu.

use dj_core::AppConfig;
use dj_library::smart::{Field, Op, Rule};
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

fn click_with(demo: &mut Demo, p: Pos2, b: PointerButton) {
    let press = move |pressed| Event::PointerButton {
        pos: p,
        button: b,
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
fn a_smart_crate_is_made_from_the_menu_and_fills_itself() {
    let mut demo = Demo::new(AppConfig::default());
    demo.keep_context();
    demo.run(3, SIZE, 1.0);
    let total = demo.app.library().unwrap().track_count();
    assert!(total >= 2, "the demo library has tracks");

    // Not in All tracks' right-click menu any more: a button of its own,
    // next to New crate.
    let all = rects(&demo, "all_tracks")[0].center();
    click_with(&mut demo, all, PointerButton::Secondary);
    let item = rects(&demo, "new_smart_crate");
    assert_eq!(item.len(), 1, "one New smart crate: the button");
    let new_crate = rects(&demo, "new_crate")[0];
    assert!(
        (item[0].center().y - new_crate.center().y).abs() < 1.0
            && item[0].left() > new_crate.right(),
        "next to New crate: {:?} {new_crate:?}",
        item[0]
    );
    // Close the menu, then the button.
    demo.run_events(3, SIZE, 1.0, |i| match i {
        0 => vec![Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::default(),
        }],
        _ => vec![],
    });
    click_with(&mut demo, item[0].center(), PointerButton::Primary);
    assert!(demo.app.library().unwrap().smart_edit.is_some());

    // The window's rule: one track's artist (as if typed).
    let artist = {
        let l = demo.app.library().unwrap();
        let id = l.view()[0];
        let artist = l.track(id).unwrap().meta.artist.clone();
        let e = l.smart_edit.as_mut().unwrap();
        e.name = "One artist".into();
        e.rules.rules = vec![Rule {
            field: Field::Artist,
            op: Op::Is,
            value: artist.clone(),
        }];
        artist
    };
    demo.run(3, SIZE, 1.0);
    let save = rects(&demo, "smart_save");
    assert_eq!(save.len(), 1, "the smart crate window is open");
    click_with(&mut demo, save[0].center(), PointerButton::Primary);

    let l = demo.app.library().unwrap();
    assert!(l.smart_edit.is_none(), "closed after Save");
    let (id, _, _) = l
        .crate_list()
        .into_iter()
        .find(|(_, n, _)| n == "One artist")
        .expect("the smart crate is in the tree");
    assert!(l.is_smart(id));
    assert_eq!(l.selection, Selection::Crate(id), "and opened");
    let shown = l.view().to_vec();
    assert!(!shown.is_empty());
    assert!(shown.iter().all(|t| {
        l.track(*t)
            .unwrap()
            .meta
            .artist
            .eq_ignore_ascii_case(&artist)
    }));
    assert!(l.reorder_blocked().is_some(), "no hand ordering");

    // Right-click the smart crate: Edit rules… opens the window with them.
    demo.run(2, SIZE, 1.0);
    let names: Vec<String> = demo
        .app
        .library()
        .unwrap()
        .crate_list()
        .into_iter()
        .map(|(_, n, _)| n)
        .collect();
    let at = names.iter().position(|n| n == "One artist").unwrap();
    let row = rects(&demo, "crate_row")[at].left_center() + vec2(30.0, 0.0);
    click_with(&mut demo, row, PointerButton::Secondary);
    let edit = rects(&demo, "smart_edit");
    assert_eq!(edit.len(), 1, "Edit rules… for a smart crate");
    click_with(&mut demo, edit[0].center(), PointerButton::Primary);
    let l = demo.app.library().unwrap();
    let e = l.smart_edit.as_ref().expect("the window is open");
    assert_eq!(e.id, Some(id));
    assert_eq!(e.rules.rules[0].value, artist);
}
