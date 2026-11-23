//! Layout checks headless at 1366x768, 1920x1080 and 3840x2160, at scale
//! factors 1.0, 1.5 and 2.0, in all four layouts: regions do not overlap,
//! nothing is drawn outside the window, every control has a size and stays
//! inside its region, controls of a region do not overlap each other, and
//! every control the layout should show is there.

use dj_core::AppConfig;
use dj_core::config::DeckLayout;
use dj_ui::headless::Demo;
use dj_ui::layout::{MixerState, PanelSize};
use dj_ui::uimap::{Region, UiMap};

const SIZES: [(f32, f32); 3] = [(1366.0, 768.0), (1920.0, 1080.0), (3840.0, 2160.0)];
const SCALES: [f32; 3] = [1.0, 1.5, 2.0];
const LAYOUTS: [DeckLayout; 4] = [
    DeckLayout::TwoDecks,
    DeckLayout::FourStacked,
    DeckLayout::FourGrid,
    DeckLayout::Extended,
];

/// Regions that tile the window and must never overlap.
const TILES: [Region; 8] = [
    Region::TopBar,
    Region::LeftDecks,
    Region::RightDecks,
    Region::Waveforms,
    Region::MixerBar,
    Region::Mixer,
    Region::Divider,
    Region::Bottom,
];

/// Containers that hold other controls, and overlays drawn on top of one
/// (the phase meter on the waveform): not checked for overlap.
const CONTAINERS: [&str; 3] = ["panel", "phase_meter", "fx_panel"];
/// Handles that sit on the edge between two controls on purpose (the
/// track list's column resize edges).
const EDGE_HANDLES: [&str; 1] = ["col_resize"];

fn overlap(a: egui::Rect, b: egui::Rect) -> bool {
    let o = a.intersect(b);
    o.width() > 0.5 && o.height() > 0.5
}

/// Geometry checks on one frame's map. Returns a list of problems.
fn problems(map: &UiMap) -> Vec<String> {
    let mut out = Vec::new();
    let screen = map.screen.expand(0.5);
    for (r, rect) in &map.regions {
        if rect.width() < 1.0 || rect.height() < 1.0 {
            out.push(format!("region {r:?} empty: {rect:?}"));
        }
        if !screen.contains_rect(*rect) {
            out.push(format!("region {r:?} outside the window: {rect:?}"));
        }
    }
    let tiles: Vec<_> = map
        .regions
        .iter()
        .filter(|(r, _)| TILES.contains(r))
        .collect();
    for (i, (ra, a)) in tiles.iter().enumerate() {
        for (rb, b) in &tiles[i + 1..] {
            if ra != rb && overlap(*a, *b) {
                out.push(format!("regions {ra:?} {a:?} and {rb:?} {b:?} overlap"));
            }
        }
    }
    for c in &map.controls {
        let what = format!("{} (deck {:?}, {:?})", c.name, c.deck, c.region);
        if c.rect.width() < 1.0 || c.rect.height() < 1.0 {
            out.push(format!("{what}: no size {:?}", c.rect));
            continue;
        }
        let Some(region) = map.region(c.region) else {
            out.push(format!("{what}: region not recorded"));
            continue;
        };
        let region = region.expand(0.5);
        let visible = if c.scrolled {
            c.rect.intersect(c.clip)
        } else {
            c.rect
        };
        if visible.width() <= 0.0 || visible.height() <= 0.0 {
            continue; // Scrolled out of view.
        }
        if !region.contains_rect(visible) {
            out.push(format!(
                "{what}: {:?} sticks out of its region {region:?}",
                c.rect
            ));
        }
        if !screen.contains_rect(visible) {
            out.push(format!("{what}: {:?} outside the window", c.rect));
        }
    }
    let fixed: Vec<_> = map
        .controls
        .iter()
        .filter(|c| !c.scrolled && !CONTAINERS.contains(&c.name) && !EDGE_HANDLES.contains(&c.name))
        .collect();
    for (i, a) in fixed.iter().enumerate() {
        for b in &fixed[i + 1..] {
            if a.region == b.region && overlap(a.rect, b.rect) {
                out.push(format!(
                    "{} (deck {:?}) {:?} overlaps {} (deck {:?}) {:?}",
                    a.name, a.deck, a.rect, b.name, b.deck, b.rect
                ));
            }
        }
    }
    out
}

/// Controls every frame must show (given the layout's panel sizes and
/// mixer state).
fn missing(app: &dj_ui::KaldorApp, layout: DeckLayout) -> Vec<String> {
    let map = app.ui_map();
    let frame = app.layout_frame().expect("frame drawn");
    let mut out = Vec::new();
    let mut need = |name: &str, deck: Option<u8>, count: usize| {
        let n = map.find(name, deck).len();
        if n < count {
            out.push(format!("{name} deck {deck:?}: {n} of {count}"));
        }
    };
    // The MIXER / PADS / FX bar is gone in every layout.
    let stray_tabs = !map.find("tab_mixer", None).is_empty() || frame.mixer_bar.is_some();
    let _ = layout;
    let library_tab = !app.sampler_shown();
    // Top bar: clock, layout menu, 2 / 4 decks; logo; master volume,
    // master meter, CPU, settings.
    for n in [
        "clock",
        "layout_menu",
        "decks_2",
        "decks_4",
        "app_name",
        "master_volume",
        "master_meter",
        "cpu",
        "settings",
        "divider",
    ] {
        need(n, None, 1);
    }
    if library_tab {
        for n in [
            "search",
            "search_clear",
            "all_tracks",
            "new_crate",
            "crate_splitter",
            "Artist",
            "Title",
            "BPM",
            "Key",
            "Time",
        ] {
            need(n, None, 1);
        }
    }
    let (view, two) = app.config().ui.deck_view();
    let decks: Vec<usize> = if two {
        app.layer().to_vec()
    } else {
        (0..4).collect()
    };
    let extended = view == DeckLayout::Extended;
    // LAYER: the LAYER rows (all four decks), or in Extended the badges of
    // the decks shown.
    let layer_decks: Vec<usize> = if extended {
        decks.clone()
    } else {
        (0..4).collect()
    };
    for d in layer_decks {
        need("layer", Some(d as u8 + 1), 1);
    }
    for &d in &decks {
        let n = Some(d as u8 + 1);
        for name in ["panel", "title", "cue", "play"] {
            need(name, n, 1);
        }
        let size = frame
            .panels
            .iter()
            .find(|p| p.deck == d)
            .map(|p| p.size)
            .expect("panel");
        if frame.mixer_state != MixerState::Swapped {
            need("wave", n, 1);
        }
        // Extended: the overview is in the panel (not in one-line panels).
        if (!extended && frame.mixer_state != MixerState::Swapped)
            || (extended && size != PanelSize::Mini)
        {
            need("overview", n, 1);
        }
        match size {
            PanelSize::Full if extended => {
                for name in [
                    "bpm",
                    "key",
                    "platter",
                    "deck_meter",
                    "key_lock",
                    "sync",
                    "slip",
                    "loop_length",
                    "tab_cues",
                    "tab_grid",
                ] {
                    need(name, n, 1);
                }
                need("cue_row", n, 8);
            }
            PanelSize::Compact if extended => {
                for name in ["bpm", "time", "sync", "key_lock"] {
                    need(name, n, 1);
                }
            }
            PanelSize::Full => {
                for name in [
                    "bpm",
                    "key",
                    "time",
                    "pitch_fader",
                    "pitch_value",
                    "pitch_range",
                    "pitch_reset",
                    "bend_down",
                    "bend_up",
                    "key_lock",
                    "sync",
                    "slip",
                    "path",
                    "load",
                ] {
                    need(name, n, 1);
                }
            }
            PanelSize::Compact => {
                for name in ["bpm", "time"] {
                    need(name, n, 1);
                }
            }
            PanelSize::Mini => need("play", n, 1),
        }
    }
    let pads = app.config().ui.centre_panel == dj_core::config::CentrePanel::Pads;
    if pads && matches!(frame.mixer_state, MixerState::Open | MixerState::Swapped) {
        for side in 0..2 {
            let n = Some(app.layer()[side] as u8 + 1);
            need("pad", n, 8);
            need("pad_mode", n, 8);
            for name in [
                "loop_in",
                "loop_out",
                "reloop",
                "auto_loop",
                "jump_back",
                "jump_fwd",
                "pads_slip",
                "key_up",
                "grid_set1",
                "grid_tap",
                "grid_lock",
                "sync_master",
            ] {
                need(name, n, 1);
            }
        }
    }
    if !pads && matches!(frame.mixer_state, MixerState::Open | MixerState::Swapped) {
        for d in 0..4u8 {
            for name in [
                "channel",
                "trim",
                "eq_hi",
                "eq_mid",
                "eq_low",
                "kill_hi",
                "kill_mid",
                "kill_low",
                "filter",
                "pfl",
                "channel_meter",
                "fader",
            ] {
                need(name, Some(d + 1), 1);
            }
            need("xf_assign", Some(d + 1), 3);
        }
        for name in [
            "crossfader",
            "master_gain",
            "booth_gain",
            "phones_gain",
            "cue_mix",
            "limiter",
            "xf_curve",
            "xf_reverse",
            "fader_curve",
            "mixer_master_meter",
        ] {
            need(name, None, 1);
        }
        need("gain_value", None, 3);
    }
    if frame.mixer_state == MixerState::NoRoom {
        need("mixer_swap", None, 1);
    }
    if stray_tabs {
        out.push("the MIXER / PADS / FX bar is shown".into());
    }
    out
}

fn config(layout: DeckLayout) -> AppConfig {
    let mut c = AppConfig::default();
    c.ui.layout = layout;
    c
}

/// The 2 / 4 switch: 2 decks (the LAYER decks) in every layout, on a large
/// and a small window.
#[test]
fn two_decks_in_every_layout() {
    let mut demo = Demo::new(config(DeckLayout::Extended));
    demo.app.set_two_decks(true);
    let mut report = Vec::new();
    for (pw, ph, scale) in [(1920.0, 1080.0, 1.0), (1366.0, 768.0, 1.5)] {
        for layout in [
            DeckLayout::Extended,
            DeckLayout::FourStacked,
            DeckLayout::FourGrid,
        ] {
            demo.app.set_layout(layout);
            let size = egui::vec2(pw / scale, ph / scale);
            demo.run(3, size, scale);
            let what = format!("{pw}x{ph} @{scale} {layout:?} 2 decks");
            let frame = demo.app.layout_frame().expect("frame").clone();
            let shown: Vec<usize> = frame.panels.iter().map(|p| p.deck).collect();
            let strips: Vec<usize> = frame.strips.iter().map(|s| s.deck).collect();
            if shown.len() != 2 || strips.len() != 2 {
                report.push(format!("{what}: panels {shown:?}, strips {strips:?}"));
            }
            for p in problems(demo.app.ui_map()) {
                report.push(format!("{what}: {p}"));
            }
            for m in missing(&demo.app, layout) {
                report.push(format!("{what}: missing {m}"));
            }
        }
    }
    assert!(
        report.is_empty(),
        "{} problems:\n{}",
        report.len(),
        report.join("\n")
    );
}

#[test]
fn every_size_scale_and_layout() {
    let mut demo = Demo::new(config(DeckLayout::FourStacked));
    let mut report = Vec::new();
    for (pw, ph) in SIZES {
        for scale in SCALES {
            for layout in LAYOUTS {
                demo.app.set_layout(layout);
                let size = egui::vec2(pw / scale, ph / scale);
                demo.run(3, size, scale);
                let what = format!("{pw}x{ph} @{scale} {layout:?}");
                let frame = demo.app.layout_frame().expect("frame").clone();
                for p in problems(demo.app.ui_map()) {
                    report.push(format!("{what}: {p}"));
                }
                for m in missing(&demo.app, layout) {
                    report.push(format!("{what}: missing {m}"));
                }
                println!(
                    "{what}: mixer {:?}, panels {:?}, {} controls",
                    frame.mixer_state,
                    frame.panels.iter().map(|p| p.size).collect::<Vec<_>>(),
                    demo.app.ui_map().controls.len()
                );
            }
        }
    }
    assert!(
        report.is_empty(),
        "{} problems:\n{}",
        report.len(),
        report.join("\n")
    );
}

/// The FX panel (FX in the top bar) at every size, scale and layout: its
/// controls all there, inside it, nothing overlapping.
#[test]
fn fx_panel_at_every_size() {
    let mut cfg = config(DeckLayout::Extended);
    cfg.ui.fx_panel = true;
    let mut demo = Demo::new(cfg);
    let mut report = Vec::new();
    for (pw, ph) in SIZES {
        for scale in SCALES {
            for layout in LAYOUTS {
                demo.app.set_layout(layout);
                let size = egui::vec2(pw / scale, ph / scale);
                demo.run(3, size, scale);
                let what = format!("{pw}x{ph} @{scale} {layout:?} FX");
                for p in problems(demo.app.ui_map()) {
                    report.push(format!("{what}: {p}"));
                }
                let map = demo.app.ui_map();
                // The display and the effect's name always; the rest as it
                // fits (one line in a narrow gap leaves some out).
                for (name, n) in [("fx_panel", 1), ("fx_name", 1)] {
                    let found = map.find(name, None).len();
                    if found != n {
                        report.push(format!("{what}: {name} {found} of {n}"));
                    }
                }
            }
        }
    }
    assert!(
        report.is_empty(),
        "{} problems:\n{}",
        report.len(),
        report.join("\n")
    );
}

/// Small windows: the mixer shown in place of the waveforms, the Setup tab,
/// the light theme and a deck popup also lay out cleanly.
#[test]
fn small_window_states() {
    let mut demo = Demo::new(config(DeckLayout::FourStacked));
    let mut report = Vec::new();
    for (w, h, scale) in [
        (1366.0, 768.0, 2.0),
        (1920.0, 1080.0, 2.0),
        (1366.0, 768.0, 1.5),
    ] {
        let size = egui::vec2(w / scale, h / scale);
        let what = format!("{w}x{h} @{scale}");
        // Mixer in place of the waveforms (when there is no room).
        demo.run(2, size, scale);
        let no_room = demo.app.layout_frame().unwrap().mixer_state == MixerState::NoRoom;
        if no_room {
            let r = demo.app.ui_map().find("mixer_swap", None)[0].rect;
            demo.run_events(6, size, scale, |i| click_at(r.center(), i));
            assert_eq!(
                demo.app.layout_frame().unwrap().mixer_state,
                MixerState::Swapped,
                "{what}"
            );
            for p in problems(demo.app.ui_map()) {
                report.push(format!("{what} swapped: {p}"));
            }
            for m in missing(&demo.app, DeckLayout::FourStacked) {
                report.push(format!("{what} swapped: missing {m}"));
            }
            let r = demo.app.ui_map().find("mixer_swap", None)[0].rect;
            demo.run_events(6, size, scale, |i| click_at(r.center(), i));
            assert_eq!(
                demo.app.layout_frame().unwrap().mixer_state,
                MixerState::NoRoom,
                "{what}: back"
            );
        }
        // Setup tab.
        demo.app.show_setup(true);
        demo.run(3, size, scale);
        for p in problems(demo.app.ui_map()) {
            report.push(format!("{what} setup: {p}"));
        }
        assert!(!demo.app.ui_map().find("tone_off", None).is_empty() || h / scale < 500.0);
        demo.app.show_setup(false);
        // No "…" (deck popup) on any panel since 2026-11-16.
        demo.run(2, size, scale);
        assert!(
            demo.app.ui_map().find("deck_more", None).is_empty(),
            "{what}: no \u{2026} button"
        );
    }
    assert!(
        report.is_empty(),
        "{} problems:\n{}",
        report.len(),
        report.join("\n")
    );
}

/// A left click at `p` spread over frames: move, press, release.
fn click_at(p: egui::Pos2, frame: usize) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Default::default(),
    };
    match frame {
        0 => vec![egui::Event::PointerMoved(p)],
        1 => vec![button(true)],
        2 => vec![button(false)],
        _ => vec![],
    }
}

/// The PADS panel (in place of the mixer) and the SAMPLER tab at
/// every size, scale and layout.
#[test]
fn pads_panel_and_sampler_tab_at_every_size() {
    let mut demo = Demo::new(config(DeckLayout::FourStacked));
    demo.add_performance_state();
    let mut report = Vec::new();
    for (pw, ph) in SIZES {
        for scale in SCALES {
            for layout in LAYOUTS {
                demo.app.set_layout(layout);
                demo.app.show_pads(true);
                demo.app.show_sampler(true);
                let size = egui::vec2(pw / scale, ph / scale);
                demo.run(3, size, scale);
                let what = format!("{pw}x{ph} @{scale} {layout:?} pads+sampler");
                for p in problems(demo.app.ui_map()) {
                    report.push(format!("{what}: {p}"));
                }
                for m in missing(&demo.app, layout) {
                    report.push(format!("{what}: missing {m}"));
                }
                // The sampler tiles when the bottom area has room for them.
                let bottom_h = demo.app.layout_frame().unwrap().bottom.height();
                if bottom_h >= 150.0 {
                    let n = demo.app.ui_map().find("sample_pad", None).len();
                    if n != 8 {
                        report.push(format!("{what}: {n} sample pads"));
                    }
                }
                // Small windows: the pads in place of the waveforms.
                if demo.app.layout_frame().unwrap().mixer_state == MixerState::NoRoom {
                    let r = demo.app.ui_map().find("mixer_swap", None)[0].rect;
                    demo.run_events(6, size, scale, |i| click_at(r.center(), i));
                    for p in problems(demo.app.ui_map()) {
                        report.push(format!("{what} swapped: {p}"));
                    }
                    for m in missing(&demo.app, layout) {
                        report.push(format!("{what} swapped: missing {m}"));
                    }
                    let r = demo.app.ui_map().find("mixer_swap", None)[0].rect;
                    demo.run_events(6, size, scale, |i| click_at(r.center(), i));
                }
            }
        }
    }
    demo.app.show_pads(false);
    demo.app.show_sampler(false);
    assert!(
        report.is_empty(),
        "{} problems:\n{}",
        report.len(),
        report.join("\n")
    );
}
