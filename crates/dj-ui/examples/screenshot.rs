//! Renders the UI headless and writes PNG screenshots, without a window, GPU
//! or audio device (CPU rasteriser in `dj_ui::headless`). Exits by itself.
//!
//! ```sh
//! cargo run --release -p dj-ui --example screenshot -- [OUT_DIR] [WxH@SCALE ...]
//! ```
//!
//! Default: `target/screenshots`, every layout in dark theme at 1920x1080 and
//! 1366x768 (scale 1), and 4 stacked in light theme at 1920x1080. Sizes are
//! physical pixels; the scale is the HiDPI factor (1366x768@1.5 lays out at
//! 911x512 points). `SHOTS=usb` (any part of a shot's name) renders only
//! the matching shots.

use dj_core::AppConfig;
use dj_core::config::DeckLayout;
use std::path::PathBuf;

fn parse_size(s: &str) -> Option<(f32, f32, f32)> {
    let (wh, scale) = s.split_once('@').unwrap_or((s, "1"));
    let (w, h) = wh.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?, scale.parse().ok()?))
}

fn write_wav(path: &std::path::Path, samples: &[f32]) {
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 44_100,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    if let Ok(mut w) = hound::WavWriter::create(path, spec) {
        for s in samples {
            let _ = w.write_sample((s.clamp(-1.0, 1.0) * 32_000.0) as i16);
        }
        let _ = w.finalize();
    }
}

/// Two short library tracks (files in `out`): A on decks 1 and 3, B on
/// deck 2, so the list shows their deck badges.
fn decks_from_library(
    demo: &mut dj_ui::headless::Demo,
    out: &std::path::Path,
    size: egui::Vec2,
    scale: f32,
) {
    let dir = std::path::absolute(out.join("library-decks")).unwrap_or_default();
    let _ = std::fs::create_dir_all(&dir);
    let mut ids = Vec::new();
    for (name, bpm) in [("Warm Up Tool", 122.0), ("Peak Time Roller", 128.0)] {
        let path = dir.join(format!("{name}.wav"));
        write_wav(
            &path,
            &dj_analysis::synth::ClickTrack::new(bpm, 6.0).render(),
        );
        if let Some(id) = demo.app.library().and_then(|l| l.add_file(&path)) {
            ids.push(id);
        }
    }
    let [a, b] = ids[..] else {
        return;
    };
    use dj_core::DeckId;
    demo.app.load_library_track(DeckId::ALL[0], a);
    demo.app.load_library_track(DeckId::ALL[2], a);
    demo.app.load_library_track(DeckId::ALL[1], b);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        demo.run(1, size, scale);
        let s = demo.app.snapshot();
        if [0, 1, 2]
            .iter()
            .all(|&d| s.decks[d].loaded && s.decks[d].load_complete)
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The USB export window for the demo crate, in the step `name` names:
/// made-up drives for the setup, a real check into a folder under `out`
/// (the demo tracks have no files, so they are left out), a made-up
/// progress and result.
fn usb_state(
    demo: &mut dj_ui::headless::Demo,
    name: &str,
    out: &std::path::Path,
    size: egui::Vec2,
    scale: f32,
) {
    use dj_export::volume::Drive;
    use dj_ui::usb_export::{Counter, DriveText, Stage, UsbExport};
    let Some(l) = demo.app.library() else {
        return;
    };
    let target = l.crate_list().first().map(|c| c.0);
    let u = &mut l.usb;
    u.open = true;
    u.target = target.into_iter().collect();
    let drive = |root: &str, label: &str, removable: bool| Drive {
        root: PathBuf::from(root),
        label: label.into(),
        removable,
    };
    u.drives = vec![
        drive("E:\\", "KALDOR", true),
        drive("F:\\", "", true),
        drive("D:\\", "Music SSD", false),
    ];
    let text = |name: &str, detail: &str| DriveText {
        name: name.into(),
        detail: detail.into(),
    };
    u.drive_text = vec![
        text("KALDOR (E:)", "28.7 GB free"),
        text("USB drive (F:)", "7.4 GB free"),
        text("Music SSD (D:)", "Fixed drive"),
    ];
    u.device = "E:\\".into();
    match name {
        "usb_planned" => {
            // Three short click tracks with files and grids in the crate,
            // so the check has something to put on the stick.
            let stick = out.join("usb-stick");
            let music = std::path::absolute(out.join("usb-music")).unwrap_or_default();
            let _ = std::fs::create_dir_all(&stick);
            let _ = std::fs::create_dir_all(&music);
            if let Some(c) = target {
                for (k, title) in ["Harbour Lights", "Night Orbit", "Concrete"]
                    .into_iter()
                    .enumerate()
                {
                    let bpm = 120.0 + 4.0 * k as f64;
                    let wav = music.join(format!("{title}.wav"));
                    write_wav(
                        &wav,
                        &dj_analysis::synth::ClickTrack::new(bpm, 4.0).render(),
                    );
                    let meta = dj_library::TrackMeta {
                        path: wav.clone(),
                        title: title.into(),
                        artist: "Kaldor Test".into(),
                        duration: 4.0,
                        file_size: std::fs::metadata(&wav).map_or(0, |m| m.len()),
                        ..dj_library::TrackMeta::default()
                    };
                    if let Ok((t, _)) = l.lib.upsert_track(&meta) {
                        let _ = l.lib.set_beatgrid(t, &dj_core::Beatgrid::new(bpm, 0.0));
                        let _ = l.lib.add_to_crate(c, &[t]);
                    }
                }
                l.reload();
            }
            l.usb.device = stick.display().to_string();
            UsbExport::start_plan(l);
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while std::time::Instant::now() < deadline {
                demo.run(1, size, scale);
                let planning = demo
                    .app
                    .library()
                    .is_some_and(|l| matches!(l.usb.stage, Stage::Planning(_)));
                if !planning {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        }
        "usb_exporting" | "usb_bar" => {
            // The sender stays alive, so the window keeps "writing".
            let (tx, rx) = std::sync::mpsc::channel();
            std::mem::forget(tx);
            u.stage = Stage::Exporting {
                rx,
                counter: std::sync::Arc::new(Counter::at(17, 42)),
                cancel: dj_export::usb::CancelToken::new(),
            };
        }
        "usb_done" | "usb_bar_done" => {
            u.stage = Stage::Done(Ok(dj_export::usb::UsbReport {
                copied: 38,
                kept: 4,
                anlz_files: 84,
                artwork: 31,
                pruned: 2,
                onelibrary_written: true,
                ..Default::default()
            }));
        }
        _ => {}
    }
    // The window closed: the top bar shows the export (and then its
    // outcome, not yet seen).
    if name.starts_with("usb_bar")
        && let Some(l) = demo.app.library()
    {
        l.usb.open = false;
        l.usb.result_seen = false;
    }
}

/// The demo crate in its own order with its first track being dragged
/// below the third.
fn order_shot(
    demo: &mut dj_ui::headless::Demo,
    size: egui::Vec2,
    scale: f32,
) -> dj_ui::headless::Screenshot {
    if let Some(l) = demo.app.library() {
        let c = l.crate_list()[0].0;
        l.selection = dj_ui::library::Selection::Crate(c);
        l.sort = (dj_ui::library::SortKey::Order, true);
        l.mark_dirty();
    }
    demo.run(3, size, scale);
    let pitch = demo.app.library().map_or(22.0, |l| l.row_pitch);
    let Some(row) = demo
        .app
        .ui_map()
        .find("track_row", None)
        .first()
        .map(|c| c.rect)
    else {
        return demo.screenshot(40, size, scale);
    };
    let from = row.center();
    let to = egui::pos2(from.x + 40.0, row.top() + 3.0 * pitch - 1.0);
    let mut seq = vec![
        vec![egui::Event::PointerMoved(from)],
        vec![egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        }],
    ];
    for k in 1..=8 {
        seq.push(vec![egui::Event::PointerMoved(
            from + (to - from) * (k as f32 / 8.0),
        )]);
    }
    demo.screenshot_events(40, size, scale, |i| seq.get(i).cloned().unwrap_or_default())
}

/// The library with a track being dragged over a second crate, one track
/// on a drive that is not connected and one whose file moved.
fn library_shot(
    demo: &mut dj_ui::headless::Demo,
    size: egui::Vec2,
    scale: f32,
) -> dj_ui::headless::Screenshot {
    use dj_ui::file_check::FileState;
    if let Some(l) = demo.app.library() {
        let _ = l.lib.create_crate("Peak time", None);
        l.reload();
        let ids = l.view().to_vec();
        l.files.set(ids[1], FileState::DriveMissing);
        l.files.set(ids[2], FileState::Missing);
    }
    demo.run(3, size, scale);
    let find = |demo: &dj_ui::headless::Demo, name: &str, k: usize| {
        demo.app
            .ui_map()
            .find(name, None)
            .get(k)
            .map(|c| c.rect.center())
    };
    let (Some(row), Some(target)) = (find(demo, "track_row", 0), find(demo, "crate_row", 1)) else {
        return demo.screenshot(40, size, scale);
    };
    let button = |p: egui::Pos2, pressed: bool| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let mut seq = vec![
        vec![egui::Event::PointerMoved(row)],
        vec![button(row, true)],
    ];
    for k in 1..=8 {
        let p = row + (target - row) * (k as f32 / 8.0);
        seq.push(vec![egui::Event::PointerMoved(p)]);
    }
    demo.screenshot_events(40, size, scale, |i| seq.get(i).cloned().unwrap_or_default())
}

/// The Import from… window: a small rekordbox XML read (one file not on
/// this computer, one note), then for `import_done` imported.
fn import_state(
    demo: &mut dj_ui::headless::Demo,
    name: &str,
    out: &std::path::Path,
    size: egui::Vec2,
    scale: f32,
) {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<DJ_PLAYLISTS Version="1.0.0"><PRODUCT Name="rekordbox"/>
<COLLECTION Entries="3">
<TRACK TrackID="1" Name="Night Drive" Artist="Example" Location="file://localhost/X:/Music/night.mp3" AverageBpm="124.00">
<TEMPO Inizio="0.100" Bpm="124.00" Metro="4/4" Battito="1"/>
<POSITION_MARK Name="Drop" Type="0" Start="64.000" Num="0"/></TRACK>
<TRACK TrackID="2" Name="Radio" Location="http://radio.example/stream"/>
<TRACK TrackID="3" Name="Sunrise" Artist="Example" Location="file://localhost/X:/Music/sunrise.mp3"/>
</COLLECTION>
<PLAYLISTS><NODE Type="0" Name="ROOT" Count="1"><NODE Type="0" Name="Gigs" Count="1">
<NODE Name="Friday" Type="1" KeyType="0" Entries="2"><TRACK Key="1"/><TRACK Key="3"/></NODE>
</NODE></NODE></PLAYLISTS></DJ_PLAYLISTS>"#;
    let path = std::path::absolute(out.join("rekordbox-example.xml")).unwrap_or_default();
    let _ = std::fs::write(&path, xml);
    if let Some(l) = demo.app.library() {
        l.foreign
            .start(dj_ui::interop_view::Source::Rekordbox, path);
    }
    for _ in 0..200 {
        demo.run(1, size, scale);
        let ready = demo
            .app
            .library()
            .is_some_and(|l| matches!(l.foreign.stage, dj_ui::interop_view::Stage::Ready(_)));
        if ready {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if name == "import_done" {
        demo.app.import_foreign_now();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args
        .first()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/screenshots"));
    let mut sizes: Vec<(f32, f32, f32)> =
        args.iter().skip(1).filter_map(|s| parse_size(s)).collect();
    if sizes.is_empty() {
        sizes = vec![(1920.0, 1080.0, 1.0), (1366.0, 768.0, 1.0)];
    }
    std::fs::create_dir_all(&out).expect("create output folder");
    let layouts = [
        (DeckLayout::Extended, "extended"),
        (DeckLayout::TwoDecks, "2decks"),
        (DeckLayout::FourStacked, "4stacked"),
        (DeckLayout::FourGrid, "4grid"),
    ];
    type Job<'a> = (DeckLayout, &'a str, bool, (f32, f32, f32));
    let mut jobs: Vec<Job> = Vec::new();
    for &s in &sizes {
        for &(l, name) in &layouts {
            jobs.push((l, name, true, s));
        }
    }
    jobs.push((DeckLayout::FourStacked, "4stacked", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "extended", false, sizes[0]));
    jobs.push((DeckLayout::FourStacked, "setup", true, sizes[0]));
    for tab in [
        "setup_decks",
        "setup_mixer",
        "setup_display",
        "setup_library",
        "setup_controller",
        "setup_keyboard",
        "setup_recording",
    ] {
        jobs.push((DeckLayout::Extended, tab, true, sizes[0]));
    }
    jobs.push((DeckLayout::FourStacked, "pads", true, sizes[0]));
    jobs.push((DeckLayout::TwoDecks, "pads", true, sizes[0]));
    jobs.push((DeckLayout::FourStacked, "sampler", true, sizes[0]));
    jobs.push((DeckLayout::FourStacked, "fx", true, sizes[0]));
    jobs.push((DeckLayout::FourStacked, "fx", false, sizes[0]));
    jobs.push((DeckLayout::FourStacked, "record", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "library", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "library", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "library_order", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "locate", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "locate_done", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "files", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "library_wave", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "library_wave", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "files", true, (1366.0, 768.0, 1.0)));
    for name in [
        "usb_setup",
        "usb_planned",
        "usb_exporting",
        "usb_done",
        "usb_bar",
        "usb_bar_done",
    ] {
        jobs.push((DeckLayout::Extended, name, true, sizes[0]));
    }
    jobs.push((DeckLayout::Extended, "usb_setup", false, sizes[0]));
    jobs.push((
        DeckLayout::Extended,
        "usb_setup",
        true,
        (1366.0, 768.0, 1.0),
    ));
    // The scrolling waveforms zoomed in all the way, to 4 beats and out all
    // the way.
    jobs.push((DeckLayout::Extended, "zoom_2beats", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "zoom_128beats", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "zoom_4beats", true, sizes[0]));
    // The GRID tab on decks 1 and 4 (dark and light), and on a small screen.
    jobs.push((DeckLayout::Extended, "grid_tab", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "library_decks", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "library_decks", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "grid_tab", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "grid_tab", true, (1366.0, 768.0, 1.0)));
    if sizes.len() > 1 {
        jobs.push((DeckLayout::FourStacked, "pads", true, sizes[1]));
        jobs.push((DeckLayout::FourStacked, "sampler", false, sizes[1]));
        jobs.push((DeckLayout::FourStacked, "fx", true, sizes[1]));
    }
    // The FX display between the decks (Echo on decks 1 / 3, dub echo on
    // the filter knobs), dark and light, a small screen and one line.
    jobs.push((DeckLayout::Extended, "fxpanel", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "fxpanel", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "fxpanel", true, (1366.0, 768.0, 1.0)));
    jobs.push((DeckLayout::FourStacked, "fxpanel", true, sizes[0]));
    // Import from… (asking, the report) and the smart crate window.
    jobs.push((DeckLayout::Extended, "import_ready", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "import_done", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "import_ready", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "smart", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "smart", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "history", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "history", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "crates", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "importmenu", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "rekordboxtab", true, sizes[0]));
    jobs.push((DeckLayout::Extended, "rekordboxtab", false, sizes[0]));
    jobs.push((DeckLayout::Extended, "importmenu", false, sizes[0]));
    // `SHOTS=usb` renders only the shots whose name contains "usb".
    if let Ok(only) = std::env::var("SHOTS") {
        jobs.retain(|j| j.1.contains(only.as_str()));
    }
    for (layout, name, dark, (w, h, scale)) in jobs {
        let mut cfg = AppConfig::default();
        cfg.ui.layout = layout;
        cfg.ui.dark_theme = dark;
        match name {
            "zoom_2beats" => cfg.ui.zoom_beats = 2.0,
            "zoom_128beats" => cfg.ui.zoom_beats = 128.0,
            "zoom_4beats" => cfg.ui.zoom_beats = 4.0,
            "fxpanel" => cfg.ui.fx_panel = true,
            _ => {}
        }
        let mut demo = dj_ui::headless::Demo::new(cfg);
        if name == "fxpanel" {
            demo.add_performance_state();
            let mut cmds = Vec::new();
            for u in 0..2 {
                dj_ui::fx_panel::pick_commands(u, Some(3), 0.62, &mut cmds);
            }
            dj_ui::fx_panel::side_commands(0, true, &mut cmds);
            for c in cmds {
                demo.app.send(dj_audio::Command::Fx(c));
            }
            demo.app
                .send(dj_audio::Command::Mixer(dj_audio::MixerCommand::ColorFx(
                    dj_audio::ColorFx::DubEcho,
                )));
        }
        demo.app.show_setup(name == "setup");
        let tab = match name {
            "setup_decks" => Some(dj_ui::SetupTab::Decks),
            "setup_mixer" => Some(dj_ui::SetupTab::Mixer),
            "setup_display" => Some(dj_ui::SetupTab::Display),
            "setup_library" => Some(dj_ui::SetupTab::Library),
            "setup_controller" => Some(dj_ui::SetupTab::Controller),
            "setup_keyboard" => Some(dj_ui::SetupTab::Keyboard),
            "setup_recording" => Some(dj_ui::SetupTab::Recording),
            _ => None,
        };
        if let Some(t) = tab {
            demo.app.show_setup_tab(t);
        }
        if name == "pads" || name == "sampler" || name == "4stacked" || name == "extended" {
            demo.add_performance_state();
        }
        // The panels under the waveforms start closed; open the one shown.
        if name == "pads" || name == "mixer" || layout != DeckLayout::Extended {
            demo.app.show_pads(name == "pads");
        }
        demo.app.show_sampler(name == "sampler");
        if name == "fx" {
            demo.app.show_fx();
            demo.add_fx_state();
        }
        demo.app.show_record(name == "record");
        let size = egui::vec2(w / scale, h / scale);
        if name == "library_wave"
            && let Some(l) = demo.app.library()
        {
            // Hot cues on the demo tracks for the small waveforms.
            for (k, id) in l.view().to_vec().into_iter().enumerate() {
                let cues: Vec<dj_library::Cue> = (0..=k as u8 + 1)
                    .map(|s| dj_library::Cue {
                        kind: dj_library::CueKind::Hot(s),
                        position: 2.0 + f64::from(s) * 6.5,
                        name: String::new(),
                        color: None,
                    })
                    .collect();
                let _ = l.lib.set_cues(id, &cues);
            }
        }
        if name == "files" {
            // The project folder: a few folders and the test music.
            let dir = std::env::current_dir().unwrap_or_default();
            demo.app.open_files_at(dir);
        }
        if (name == "locate" || name == "locate_done")
            && let Some(l) = demo.app.library()
        {
            let id = l.view()[0];
            l.files.set(id, dj_ui::file_check::FileState::Missing);
            l.open_locate(id);
            // The result after a relink, as the window shows it.
            if name == "locate_done"
                && let Some(w) = l.locate.as_mut()
            {
                w.done = true;
                w.others = "3 more missing tracks found and relinked (2 still not found).".into();
            }
        }
        if name.starts_with("usb_") {
            usb_state(&mut demo, name, &out, size, scale);
        }
        if name == "grid_tab" {
            for d in [dj_core::DeckId::ALL[0], dj_core::DeckId::ALL[3]] {
                demo.app.show_grid_tab(d, true);
            }
        }
        if name == "library_decks" {
            decks_from_library(&mut demo, &out, size, scale);
        }
        if name == "history"
            && let Some(l) = demo.app.library()
        {
            // Two sessions of the demo tracks, an hour apart.
            let ids = l.view().to_vec();
            for (n, start) in [(0usize, 1_794_940_000i64), (1, 1_794_990_000)] {
                if let Ok(s) = l.lib.start_session() {
                    for (k, id) in ids.iter().enumerate().skip(n) {
                        let _ =
                            l.lib
                                .log_play_at(s, *id, (k % 4) as u8 + 1, start + k as i64 * 330);
                    }
                }
            }
        }
        if name == "rekordboxtab" {
            rekordbox_tab(&mut demo);
        }
        if name == "history" {
            demo.app.open_history();
        }
        // Nested crates (one closed) and a smart crate, for the arrows and
        // the New crate / New smart crate buttons.
        if name == "crates"
            && let Some(l) = demo.app.library()
        {
            let sets = l.lib.create_crate("Sets", None).ok();
            if let Some(sets) = sets {
                let _ = l.lib.create_crate("Friday", Some(sets));
                if let Ok(sat) = l.lib.create_crate("Saturday", Some(sets)) {
                    let _ = l.lib.create_crate("Warm-up", Some(sat));
                }
            }
            if let Ok(genres) = l.lib.create_crate("Genres", None) {
                let _ = l.lib.create_crate("House", Some(genres));
                let _ = l.lib.create_crate("Techno", Some(genres));
                l.toggle_collapsed(genres);
            }
            let _ = l.lib.create_smart_crate(
                "Fast ones",
                None,
                &dj_library::smart::SmartRules::default(),
            );
            l.reload();
        }
        if name.starts_with("import_") {
            import_state(&mut demo, name, &out, size, scale);
        }
        if name == "smart"
            && let Some(l) = demo.app.library()
        {
            use dj_library::smart::{Field, Op, Rule, SmartRules};
            let rules = SmartRules {
                match_all: true,
                rules: vec![
                    Rule {
                        field: Field::Location,
                        op: Op::Contains,
                        value: ".wav".into(),
                    },
                    Rule {
                        field: Field::Bpm,
                        op: Op::Between,
                        value: "120-130".into(),
                    },
                ],
            };
            let _ = l.lib.create_smart_crate("House 120-130", None, &rules);
            l.reload();
            let mut e = dj_ui::smart_view::SmartEdit::new(None);
            e.name = "Warm-up".into();
            e.rules = rules;
            l.smart_edit = Some(e);
        }
        let shot = match name {
            "library" => library_shot(&mut demo, size, scale),
            "library_order" => order_shot(&mut demo, size, scale),
            "importmenu" => import_menu_shot(&mut demo, size, scale),
            _ => demo.screenshot(40, size, scale),
        };
        let theme = if dark { "dark" } else { "light" };
        let file = out.join(format!("{name}_{layout:?}_{w}x{h}@{scale}_{theme}.png"));
        let f = std::fs::File::create(&file).expect("create png");
        let mut enc = png::Encoder::new(
            std::io::BufWriter::new(f),
            shot.width as u32,
            shot.height as u32,
        );
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().expect("png header");
        writer.write_image_data(&shot.rgba).expect("png data");
        println!("{}", file.display());
    }
}

/// The Import button's menu, open, the pointer then moved down onto its
/// first entry.
fn import_menu_shot(
    demo: &mut dj_ui::headless::Demo,
    size: egui::Vec2,
    scale: f32,
) -> dj_ui::headless::Screenshot {
    demo.run(3, size, scale);
    let Some(b) = demo
        .app
        .ui_map()
        .find("import_button", None)
        .first()
        .map(|c| c.rect.center())
    else {
        return demo.screenshot(40, size, scale);
    };
    let button = |p: egui::Pos2, pressed: bool| egui::Event::PointerButton {
        pos: p,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::default(),
    };
    let seq = [
        vec![egui::Event::PointerMoved(b)],
        vec![button(b, true)],
        vec![button(b, false)],
        vec![egui::Event::PointerMoved(b + egui::vec2(40.0, 62.0))],
    ];
    demo.screenshot_events(40, size, scale, |i| seq.get(i).cloned().unwrap_or_default())
}

/// The crate list's rekordbox tab: the demo tracks as rekordbox's
/// collection, in a folder with two playlists and a playlist, the
/// collection open.
fn rekordbox_tab(demo: &mut dj_ui::headless::Demo) {
    use dj_library::interop::rekordbox_db::RekordboxDb;
    use dj_library::interop::{ForeignLibrary, ForeignNode, ForeignTrack};
    let Some(l) = demo.app.library() else {
        return;
    };
    let tracks: Vec<ForeignTrack> = l
        .lib
        .tracks()
        .unwrap_or_default()
        .into_iter()
        .map(|t| ForeignTrack {
            path: t.meta.path.clone(),
            title: t.meta.title.clone(),
            artist: t.meta.artist.clone(),
            ..ForeignTrack::default()
        })
        .collect();
    let n = tracks.len();
    let db = RekordboxDb {
        ids: (0..n).map(|i| format!("{i}")).collect(),
        fingerprints: (0..n as u64).collect(),
        library: ForeignLibrary {
            source: "rekordbox".into(),
            tracks,
            playlists: vec![
                ForeignNode::Folder {
                    name: "Gigs".into(),
                    children: vec![
                        ForeignNode::Playlist {
                            name: "Friday main room".into(),
                            tracks: (0..n).rev().collect(),
                        },
                        ForeignNode::Playlist {
                            name: "Saturday".into(),
                            tracks: vec![0],
                        },
                    ],
                },
                ForeignNode::Playlist {
                    name: "Warm-up".into(),
                    tracks: (0..n.min(2)).collect(),
                },
            ],
            warnings: Vec::new(),
        },
        smart_playlists: 0,
        misplaced_cues: Vec::new(),
    };
    l.apply_rekordbox(db);
    l.rekordbox.show_as_synced();
    l.crate_tab = dj_ui::library::CrateTab::Rekordbox;
    if let Some(root) = l.rb_root {
        l.open_crate(dj_ui::library::Selection::Crate(root));
    }
}
