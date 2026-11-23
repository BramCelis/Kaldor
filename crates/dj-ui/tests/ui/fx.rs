//! FX and recording in the headless UI: the FX panel, FX keys, the config keeping
//! the FX setup, and the RECORD tab writing a real file under `target/`.
//! Synthetic pointer and key events, no window, no audio device.

use dj_core::AppConfig;
use dj_core::config::{DeckLayout, RecordFormat};
use dj_ui::headless::Demo;
use egui::{Event, Key, Modifiers, PointerButton, Pos2, Rect, vec2};
use std::time::{Duration, Instant};

struct T {
    demo: Demo,
    size: egui::Vec2,
}

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

fn key_event(key: Key, pressed: bool, shift: bool) -> Event {
    Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers: if shift {
            Modifiers::SHIFT
        } else {
            Modifiers::default()
        },
    }
}

impl T {
    fn new(cfg: AppConfig) -> Self {
        let mut t = Self {
            demo: Demo::new(cfg),
            size: vec2(1920.0, 1080.0),
        };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        self.demo.run(n, self.size, 1.0);
    }

    fn input(&mut self, seq: Vec<Vec<Event>>) {
        let n = seq.len() + 3;
        self.demo.run_events(n, self.size, 1.0, |i| {
            seq.get(i).cloned().unwrap_or_default()
        });
    }

    fn rect(&self, name: &str, index: Option<u8>) -> Rect {
        self.demo
            .app
            .ui_map()
            .find(name, index)
            .first()
            .map(|p| p.rect)
            .unwrap_or_else(|| panic!("control {name} ({index:?}) not on screen"))
    }

    fn click(&mut self, name: &str, index: Option<u8>) {
        let p = self.rect(name, index).center();
        // Two frames before the press: a window that appears in this run
        // (the settings window) is laid out invisibly in its first frame.
        self.input(vec![
            vec![moved(p)],
            vec![moved(p)],
            vec![button(p, true)],
            vec![button(p, false)],
        ]);
    }

    fn key(&mut self, k: Key, shift: bool) {
        self.input(vec![
            vec![key_event(k, true, shift)],
            vec![key_event(k, false, shift)],
        ]);
    }

    fn fx(&self) -> dj_audio::FxParams {
        self.demo.app.snapshot().fx.params
    }

    /// Runs frames until `done` holds (real time passes: the writer thread
    /// and the config check run on their own clocks).
    fn until(&mut self, what: &str, mut done: impl FnMut(&Self) -> bool) {
        let start = Instant::now();
        while !done(self) {
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "timed out: {what}"
            );
            self.frames(5);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

/// UI map index of FX item `i` of unit `u` (unit * 10 + item, 1-based).
fn ix(u: u8, i: u8) -> Option<u8> {
    Some(u * 10 + i + 1)
}

#[test]
#[ignore = "the PADS / FX / mixer panel is off screen in every layout; re-enable when it has a new place"]
fn fx_panel_controls_reach_the_engine_and_the_config_keeps_them() {
    let mut cfg = AppConfig::default();
    cfg.ui.layout = DeckLayout::FourStacked;
    let mut t = T::new(cfg);
    t.click("tab_fx", None);
    for u in 0..2 {
        for n in [
            "fx_on",
            "fx_mode",
            "fx_beats_down",
            "fx_beats_up",
            "fx_tap",
            "fx_mix",
            "fx_presets",
        ] {
            t.rect(n, ix(u, 0));
        }
        for s in 0..3 {
            t.rect("fx_kind", ix(u, s));
            t.rect("fx_slot_on", ix(u, s));
            t.rect("fx_depth", ix(u, s));
        }
        for target in 0..5 {
            t.rect("fx_assign", ix(u, target));
        }
    }
    t.click("fx_on", ix(0, 0));
    assert!(t.fx().units[0].on);
    t.click("fx_slot_on", ix(1, 2));
    assert!(t.fx().units[1].slots[2].on);
    t.click("fx_assign", ix(0, 4));
    assert!(t.fx().units[0].assign[4], "unit 1 on the master");
    let beats = t.fx().units[0].beats;
    t.click("fx_beats_up", ix(0, 0));
    assert_eq!(t.fx().units[0].beats, beats + 1);
    // Single mode: the first effect's three knobs.
    t.click("fx_mode", ix(1, 0));
    assert_eq!(t.fx().units[1].mode, dj_audio::FxMode::Single);
    t.frames(2);
    for i in 0..3 {
        t.rect("fx_param", ix(1, i));
    }

    // Keys: Shift+Z unit 1, Shift+Q its first effect, Home / End beats.
    t.key(Key::Z, true);
    assert!(!t.fx().units[0].on);
    t.key(Key::Q, true);
    assert!(t.fx().units[0].slots[0].on);
    t.key(Key::End, false);
    assert_eq!(t.fx().units[0].beats, beats + 2);

    // The setup (not on/off) lands in the config.
    t.until("config has the FX setup", |t| {
        let c = &t.demo.app.config().fx.units;
        c.len() == 2 && c[1].mode == "single" && c[0].assign[4]
    });
}

#[test]
fn top_bar_rec_writes_a_file() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("phase6_ui_rec");
    let _ = std::fs::remove_dir_all(&dir);
    let mut cfg = AppConfig::default();
    cfg.recording.folder = Some(dir.clone());
    cfg.recording.format = RecordFormat::Flac;
    let mut t = T::new(cfg);
    t.frames(3);
    // REC sits left of the master volume knob.
    let (rec, vol) = (t.rect("top_record", None), t.rect("master_volume", None));
    assert!(rec.right() <= vol.left(), "{rec:?} {vol:?}");
    t.click("top_record", None);
    assert!(t.demo.app.snapshot().recording);
    t.frames(30);
    // The same button stops it.
    t.click("top_record", None);
    assert!(!t.demo.app.snapshot().recording);
    t.until("the file is saved", |t| {
        !t.demo.app.recorded_files().is_empty()
    });
    let files = t.demo.app.recorded_files();
    assert_eq!(files.len(), 1);
    let f = &files[0];
    assert!(
        f.starts_with(&dir) && f.extension().is_some_and(|e| e == "flac"),
        "{f:?}"
    );
    let name = f.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("Kaldor 20"), "{name}");
    let (_, audio) = dj_decode::decode_file(f, 0).expect("a readable FLAC");
    assert!(audio.len_frames() > 1000);
}
