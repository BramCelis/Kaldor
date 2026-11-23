//! The settings window (the settings button in the top bar): a window over
//! the app with a tab per section (Audio, Decks, Mixer, Display, Library,
//! Controller, Keyboard, Recording). Each section is a column of cards with one row
//! per setting (`settings_ui`).

use crate::app::KaldorApp;
use crate::audio::{DeviceChoice, ROUTINGS, fit_routing, routing_name};
use crate::settings_ui::{Rows, button, card, level_bar, segmented, switch};
use crate::theme::Palette;
use crate::uimap::UiMap;
use dj_audio::{Command, TestTone, ToneTarget};
use dj_core::AudioConfig;
use egui::{Color32, RichText};

/// The sections of the settings window, one tab each.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SetupTab {
    #[default]
    Audio,
    Decks,
    Mixer,
    Display,
    Library,
    Controller,
    Keyboard,
    Recording,
}

impl SetupTab {
    pub const ALL: [SetupTab; 8] = [
        SetupTab::Audio,
        SetupTab::Decks,
        SetupTab::Mixer,
        SetupTab::Display,
        SetupTab::Library,
        SetupTab::Controller,
        SetupTab::Keyboard,
        SetupTab::Recording,
    ];

    pub fn label(self) -> &'static str {
        match self {
            SetupTab::Audio => "Audio",
            SetupTab::Decks => "Decks",
            SetupTab::Mixer => "Mixer",
            SetupTab::Display => "Display",
            SetupTab::Library => "Library",
            SetupTab::Controller => "Controller",
            SetupTab::Keyboard => "Keyboard",
            SetupTab::Recording => "Recording",
        }
    }

    /// A line under the title describing the section.
    fn blurb(self) -> &'static str {
        match self {
            SetupTab::Audio => {
                "Where Kaldor plays: the audio device, its outputs, sample rate and buffer."
            }
            SetupTab::Decks => "How the decks play: sync, quantize, key lock, loop and jump sizes.",
            SetupTab::Mixer => "How the EQ, filter, faders, auto gain and master limiter behave.",
            SetupTab::Display => "How Kaldor looks: theme, waveforms and how keys are written.",
            SetupTab::Library => {
                "Track analysis, bringing music in and taking crates out to rekordbox."
            }
            SetupTab::Controller => "The connected DJ controller and your own mappings.",
            SetupTab::Keyboard => "Every keyboard action and its key. Change any of them.",
            SetupTab::Recording => "Record the master output to a file.",
        }
    }

    /// The tab's name in the UI map ("setup_tab_audio", ...).
    pub fn map_name(self) -> &'static str {
        match self {
            SetupTab::Audio => "setup_tab_audio",
            SetupTab::Decks => "setup_tab_decks",
            SetupTab::Mixer => "setup_tab_mixer",
            SetupTab::Display => "setup_tab_display",
            SetupTab::Library => "setup_tab_library",
            SetupTab::Controller => "setup_tab_controller",
            SetupTab::Keyboard => "setup_tab_keyboard",
            SetupTab::Recording => "setup_tab_recording",
        }
    }
}

/// Sample rates offered in Audio (the config file can hold any other).
const SAMPLE_RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

fn rate_name(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", f64::from(rate) / 1000.0)
    }
}

/// Beat sizes offered for loops, jumps and the slicer.
fn beats_name(b: f32) -> String {
    if b >= 1.0 {
        format!("{b:.0} {}", if b == 1.0 { "beat" } else { "beats" })
    } else {
        format!("1/{:.0} beat", 1.0 / b)
    }
}

/// Widest the column of cards gets (it is centred in the window).
const COLUMN_W: f32 = 760.0;

/// The settings window: over the whole app (the app behind is dimmed and
/// does not react). A header with the title and a round close button,
/// underlined tabs, and the section's cards. Close with ×, Escape or a
/// click outside it.
pub(crate) fn setup_window(app: &mut KaldorApp, ui: &mut egui::Ui) {
    if !app.show_setup {
        return;
    }
    let ctx = ui.ctx().clone();
    let pal = Palette::get(app.config.ui.dark_theme);
    let screen = ctx.content_rect();
    let size = egui::vec2(
        (screen.width() * 0.78).clamp(320.0, 1080.0),
        (screen.height() * 0.86).clamp(240.0, 880.0),
    );
    // The window's rectangle (the modal is centred), for the UI map and the
    // click-outside check.
    let outer = egui::Rect::from_center_size(screen.center(), size);
    let frame = egui::Frame::new()
        // The cards are a step lighter (dark) or white (light).
        .fill(if pal.dark { pal.raised } else { pal.bg })
        .stroke(egui::Stroke::new(1.0, pal.border))
        .corner_radius(16.0)
        .shadow(egui::Shadow {
            offset: [0, 12],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(120),
        });
    let modal = egui::Modal::new(egui::Id::unique("settings_window"))
        .backdrop_color(Color32::from_black_alpha(170))
        .frame(frame)
        .show(&ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            app.map.enter(crate::uimap::Region::Settings, outer);
            let pad = 28.0;
            let top = ui.max_rect().top();
            let left = ui.max_rect().left() + pad;
            let right = ui.max_rect().right() - pad;

            // Header: title, section line, round close button.
            let painter = ui.painter().clone();
            painter.text(
                egui::pos2(left, top + 24.0),
                egui::Align2::LEFT_TOP,
                "SETTINGS",
                crate::theme::display(ui.ctx(), 19.0),
                pal.text,
            );
            painter.text(
                egui::pos2(left, top + 56.0),
                egui::Align2::LEFT_TOP,
                app.setup_tab.blurb(),
                crate::theme::prop(13.0),
                pal.text_weak,
            );
            let close = egui::Rect::from_center_size(
                egui::pos2(right - 16.0, top + 38.0),
                egui::vec2(34.0, 34.0),
            );
            let cr = ui
                .interact(close, egui::Id::unique("close_setup"), egui::Sense::click())
                .on_hover_text("Close (Escape)");
            if cr.hovered() {
                painter.circle_filled(close.center(), 17.0, pal.border);
            }
            let k = 6.0;
            let c = close.center();
            for d in [egui::vec2(k, k), egui::vec2(k, -k)] {
                painter.line_segment([c - d, c + d], egui::Stroke::new(2.0, pal.text));
            }
            app.map.add(ui, "close_setup", None, close);
            let mut closed = cr.clicked();

            // Tabs: text with an accent underline under the active one.
            let tabs_y = top + 92.0;
            let tab_h = 36.0;
            let mut x = left;
            // The tab names in the display face, in capitals.
            let font = crate::theme::display(ui.ctx(), 11.0);
            for t in SetupTab::ALL {
                let w = painter
                    .layout_no_wrap(t.label().to_uppercase(), font.clone(), pal.text)
                    .size()
                    .x
                    + 24.0;
                let r = egui::Rect::from_min_size(egui::pos2(x, tabs_y), egui::vec2(w, tab_h));
                if r.right() > right {
                    break;
                }
                let resp = ui.interact(r, egui::Id::unique(t.map_name()), egui::Sense::click());
                let on = app.setup_tab == t;
                painter.text(
                    r.center(),
                    egui::Align2::CENTER_CENTER,
                    t.label().to_uppercase(),
                    font.clone(),
                    if on || resp.hovered() {
                        pal.text
                    } else {
                        pal.text_weak
                    },
                );
                if on {
                    painter.rect_filled(
                        egui::Rect::from_min_max(
                            egui::pos2(r.left() + 8.0, r.bottom() - 3.0),
                            egui::pos2(r.right() - 8.0, r.bottom()),
                        ),
                        2.0,
                        pal.accent,
                    );
                }
                if resp.clicked() {
                    app.setup_tab = t;
                }
                app.map.add(ui, t.map_name(), None, r);
                x = r.right() + 4.0;
            }
            painter.hline(
                left..=right,
                tabs_y + tab_h,
                egui::Stroke::new(1.0, pal.border),
            );

            // The section: a centred column of cards that scrolls.
            let body = egui::Rect::from_min_max(
                egui::pos2(left, tabs_y + tab_h + 1.0),
                egui::pos2(right, ui.max_rect().bottom() - 8.0),
            );
            let mut bu = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(body)
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            // Escape cancels a key change instead of closing the window.
            let capturing = app.key_capture.is_some();
            egui::ScrollArea::vertical()
                .id_salt(("setup_scroll", app.setup_tab))
                .auto_shrink([false, false])
                .show(&mut bu, |ui| {
                    app.map.scrolled = true;
                    let col = ui.available_width().min(COLUMN_W);
                    let indent = ((ui.available_width() - col) / 2.0).max(0.0);
                    ui.horizontal_top(|ui| {
                        ui.add_space(indent);
                        ui.vertical(|ui| {
                            ui.set_width(col);
                            match app.setup_tab {
                                SetupTab::Audio => audio(app, ui, &pal),
                                SetupTab::Decks => decks(app, ui, &pal),
                                SetupTab::Mixer => crate::mixer_setup::mixer_setup(app, ui),
                                SetupTab::Display => display(app, ui, &pal),
                                SetupTab::Library => crate::library_view::library_setup(app, ui),
                                SetupTab::Controller => controller(app, ui, &pal),
                                SetupTab::Keyboard => {
                                    crate::keyboard_setup::keyboard_setup(app, ui)
                                }
                                SetupTab::Recording => crate::record_view::record_settings(app, ui),
                            }
                            ui.add_space(18.0);
                        });
                    });
                    app.map.scrolled = false;
                });
            // Escape, or a press outside the window, closes it (checked
            // against the window's own rectangle, which is known before
            // egui has laid the window out).
            closed |= ui.input(|i| {
                (i.key_pressed(egui::Key::Escape) && !capturing)
                    || (i.pointer.any_pressed()
                        && i.pointer.press_origin().is_some_and(|p| !outer.contains(p)))
            });
            closed
        });
    if modal.inner {
        app.show_setup = false;
        app.key_capture = None;
    }
}

/// Audio: the output (driver, device, outputs, buffer, Apply), what runs
/// now, and the test tone.
fn audio(app: &mut KaldorApp, ui: &mut egui::Ui, pal: &Palette) {
    card(ui, pal, "Audio output", |rows| output_picker(app, rows));
    card(ui, pal, "Running now", |rows| running_now(app, rows));
    card(ui, pal, "Test tone", |rows| test_tone(app, rows));
}

/// Output picker rows: driver, device (with Refresh), outputs, buffer, and
/// Apply / Undo.
fn output_picker(app: &mut KaldorApp, rows: &mut Rows<'_>) {
    let Some(backend) = app.audio_backend.as_ref() else {
        rows.row(
            "Audio output",
            "Set in the config file (this run has no device choice).",
            |_| {},
        );
        return;
    };
    let p = &mut app.picker;
    if !p.initialized {
        p.initialized = true;
        p.hosts = backend.hosts();
        p.reset_from(&app.config.audio);
    }
    let mut refresh = p.devices.as_ref().is_none_or(|(h, _)| *h != p.host);
    let mut marks: Vec<(&'static str, egui::Rect)> = Vec::new();

    rows.row(
        "Driver",
        "WASAPI exclusive gives the lowest latency.",
        |ui| {
            let r = egui::ComboBox::from_id_salt("audio_host")
                .selected_text(p.host.as_deref().unwrap_or("Default"))
                .width(240.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(p.host.is_none(), "Default").clicked() {
                        p.host = None;
                        p.device = None;
                    }
                    for h in p.hosts.clone() {
                        let label = if h == dj_audio::device::EXCLUSIVE_HOST {
                            format!("{h} (low latency)")
                        } else {
                            h.clone()
                        };
                        if ui
                            .selectable_label(p.host.as_deref() == Some(h.as_str()), label)
                            .clicked()
                        {
                            p.host = Some(h);
                            p.device = None;
                        }
                    }
                });
            marks.push(("driver", r.response.rect));
        },
    );
    rows.row("Device", "The sound card or controller to play on.", |ui| {
        let b = button(ui, "Refresh", false, None)
            .on_hover_text("Look for devices again (after plugging one in)");
        if b.clicked() {
            refresh = true;
        }
        marks.push(("refresh", b.rect));
        let devices: Vec<DeviceChoice> = match p.devices.as_ref() {
            Some((_, Ok(d))) => d.clone(),
            _ => Vec::new(),
        };
        let r = egui::ComboBox::from_id_salt("audio_device")
            .selected_text(p.device.as_deref().unwrap_or("Default output"))
            .width(200.0)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(p.device.is_none(), "Default output")
                    .clicked()
                {
                    p.device = None;
                }
                for d in &devices {
                    let label = match (d.channels, d.is_default) {
                        (0, false) => d.name.clone(),
                        (0, true) => format!("{} (default)", d.name),
                        (c, false) => format!("{} ({c} ch)", d.name),
                        (c, true) => format!("{} ({c} ch, default)", d.name),
                    };
                    if ui
                        .selectable_label(p.device.as_deref() == Some(d.name.as_str()), label)
                        .clicked()
                    {
                        p.device = Some(d.name.clone());
                        p.routing = fit_routing(p.routing, d.channels);
                    }
                }
            });
        marks.push(("device", r.response.rect));
    });
    rows.row(
        "Outputs",
        "Which outputs carry the master and the headphones.",
        |ui| {
            let r = egui::ComboBox::from_id_salt("audio_routing")
                .selected_text(routing_name(p.routing))
                .width(240.0)
                .show_ui(ui, |ui| {
                    for (r, name) in ROUTINGS {
                        let on = dj_core::OutputRouting {
                            booth: None,
                            ..p.routing
                        } == r;
                        if ui.selectable_label(on, name).clicked() {
                            // The booth output stays as set below.
                            p.routing = dj_core::OutputRouting {
                                booth: p.routing.booth,
                                ..r
                            };
                        }
                    }
                });
            marks.push(("outputs", r.response.rect));
        },
    );
    rows.row(
        "Booth output",
        "A copy of the master at the booth level, for a booth monitor. Needs a device with more outputs (not the NS4FX).",
        |ui| {
            let opts: [(Option<u16>, &str); 4] = [
                (None, "Off"),
                (Some(2), "3-4"),
                (Some(4), "5-6"),
                (Some(6), "7-8"),
            ];
            let now = p.routing.booth.map(|b| b.left);
            let sel = opts.iter().position(|(o, _)| *o == now);
            let labels: Vec<&str> = opts.iter().map(|(_, l)| *l).collect();
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (c, rects) = segmented(ui, &labels, sel);
                if let Some(i) = c {
                    p.routing.booth = opts[i]
                        .0
                        .map(|l| dj_core::routing::ChannelPair::new(l, l + 1));
                }
                if let Some(r) = rects.first() {
                    marks.push(("booth_output", *r));
                }
            });
        },
    );
    rows.row(
        "Sample rate",
        "48 kHz suits most devices. If the device cannot run this rate, Kaldor uses the device's own rate.",
        |ui| {
            let r = egui::ComboBox::from_id_salt("audio_sample_rate")
                .selected_text(rate_name(p.sample_rate))
                .width(120.0)
                .show_ui(ui, |ui| {
                    for rate in SAMPLE_RATES {
                        ui.selectable_value(&mut p.sample_rate, rate, rate_name(rate));
                    }
                });
            marks.push(("sample_rate", r.response.rect));
        },
    );
    rows.row(
        "Buffer",
        "Smaller is lower latency; raise it if you hear dropouts.",
        |ui| {
            ui.label(
                RichText::new(format!(
                    "{:.1} ms",
                    f64::from(p.buffer_frames) * 1000.0 / f64::from(p.sample_rate.max(1))
                ))
                .size(13.0),
            );
            let (lo, hi) = AudioConfig::BUFFER_RANGE;
            let r = ui.add(
                egui::DragValue::new(&mut p.buffer_frames)
                    .range(lo..=hi)
                    .suffix(" frames"),
            );
            marks.push(("buffer", r.rect));
        },
    );
    if refresh {
        let host = p.host.clone();
        let list = backend.devices(host.as_deref());
        p.devices = Some((host, list));
    }
    let list_error = match &p.devices {
        Some((_, Err(e))) => Some(format!("No device list: {e}")),
        _ => None,
    };
    let wanted = p.to_config();
    let message = app.picker.message.clone();
    let desc = match (&message, &list_error) {
        (Some(m), _) => m.clone(),
        (None, Some(e)) => e.clone(),
        (None, None) => "Applying stops the decks; loaded tracks reload at the same place.".into(),
    };
    let mut apply = false;
    let mut undo = false;
    rows.row("Apply the changes", &desc, |ui| {
        let a = button(ui, "Apply", true, None);
        marks.push(("apply", a.rect));
        apply = a.clicked();
        let u = button(ui, "Undo changes", false, None);
        marks.push(("undo", u.rect));
        undo = u.clicked();
    });
    if apply {
        let msg = match app.switch_audio(wanted) {
            Ok(()) => format!("Now playing on {}.", app.output_info.device),
            Err(e) => format!("Could not open that output: {e}"),
        };
        app.picker.message = Some(msg);
    }
    if undo {
        app.picker.reset_from(&app.config.audio);
    }
    for (name, r) in marks {
        app.map.add(rows.ui, name, None, r);
    }
}

/// What the audio runs with now, as label / value rows.
fn running_now(app: &mut KaldorApp, rows: &mut Rows<'_>) {
    let info = app.output_info.clone();
    let mut items: Vec<(&str, String)> = vec![
        ("Driver", info.host.clone()),
        ("Device", info.device.clone()),
        (
            "Sample rate",
            format!(
                "{} Hz, {} channels, {}",
                info.sample_rate, info.channels, info.sample_format
            ),
        ),
        (
            "Buffer",
            match info.buffer_frames {
                Some(b) => format!(
                    "{b} frames ({:.1} ms)",
                    f64::from(b) * 1000.0 / f64::from(info.sample_rate.max(1))
                ),
                None => "driver default".into(),
            },
        ),
    ];
    if let Some(out) = &app.output {
        items.push((
            "Output latency",
            match out.output_latency() {
                Some(l) => format!(
                    "{:.2} ms (reported by the driver)",
                    l.as_secs_f64() * 1000.0
                ),
                None => "not reported yet".into(),
            },
        ));
    }
    for (k, v) in items {
        rows.row(k, "", |ui| {
            ui.label(RichText::new(v).size(13.5));
        });
    }
    if let Some(total) = app.output.as_ref().map(|o| o.xruns()) {
        // A new output starts counting from zero again.
        if total < app.xrun_base {
            app.xrun_base = 0;
        }
        let shown = total - app.xrun_base;
        let clipped = app.engine.snapshot().clipped;
        rows.row(
            "Underruns",
            "Moments the audio could not keep up (heard as clicks). Reset before a set to count only that set.",
            |ui| {
                let b = button(ui, "Reset", false, None);
                if b.clicked() {
                    app.xrun_base = total;
                    app.clip_base = clipped;
                    if let Some(o) = &app.output {
                        o.reset_timing();
                    }
                }
                app.map.add(ui, "xrun_reset", None, b.rect);
                ui.label(RichText::new(shown.to_string()).size(13.5));
            },
        );
        // Samples cut at full scale per output (each is a click); with the
        // limiters on they stay 0.
        if clipped.iter().zip(app.clip_base.iter()).any(|(c, b)| c < b) {
            app.clip_base = [0; 3];
        }
        let c: Vec<u64> = clipped
            .iter()
            .zip(app.clip_base.iter())
            .map(|(c, b)| c - b)
            .collect();
        rows.row(
            "Clipped samples",
            "Samples cut at full scale on the way out (heard as crackle). Should stay 0.",
            |ui| {
                let r = ui.label(
                    RichText::new(format!(
                        "master {} · headphones {} · booth {}",
                        c[0], c[1], c[2]
                    ))
                    .size(13.5),
                );
                app.map.add(ui, "clipped", None, r.rect);
            },
        );
        if let Some(t) = app.output.as_ref().and_then(|o| o.timing()) {
            let ms = |us: u32| f64::from(us) / 1000.0;
            let queued = if t.queued_min_us == u32::MAX {
                "-".to_string()
            } else {
                format!("{:.2} ms", ms(t.queued_min_us))
            };
            rows.row(
                "Timing (worst)",
                "Since the output started. A dropout with a slow engine write: the engine was too slow; with a long gap: Windows kept the audio thread from running.",
                |ui| {
                    let r = ui.label(
                        RichText::new(format!(
                            "engine write {:.2} ms · gap between checks {:.2} ms · least queued {queued}",
                            ms(t.process_max_us),
                            ms(t.gap_max_us),
                        ))
                        .size(13.5),
                    );
                    app.map.add(ui, "output_timing", None, r.rect);
                },
            );
        }
    }
}

/// The test tone: Master / Headphones / Off, and the two levels.
fn test_tone(app: &mut KaldorApp, rows: &mut Rows<'_>) {
    let current = app.engine.snapshot().test_tone;
    let sel = match current {
        Some(ToneTarget::Master) => 0,
        Some(ToneTarget::Cue) => 1,
        None => 2,
    };
    let mut pick = None;
    let mut rects = Vec::new();
    rows.row(
        "Test tone",
        "440 Hz at -18 dBFS. On the NS4FX the master is outputs 1-2, the headphones 3-4.",
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (c, r) = segmented(ui, &["Master", "Headphones", "Off"], Some(sel));
                pick = c;
                rects = r;
            });
        },
    );
    for (name, r) in ["tone_master", "tone_cue", "tone_off"]
        .into_iter()
        .zip(&rects)
    {
        app.map.add(rows.ui, name, None, *r);
    }
    match pick {
        Some(0) => app.send(Command::SetTestTone(Some(TestTone::new(
            ToneTarget::Master,
        )))),
        Some(1) => app.send(Command::SetTestTone(Some(TestTone::new(ToneTarget::Cue)))),
        Some(_) => app.send(Command::SetTestTone(None)),
        None => {}
    }
    let snap = *app.engine.snapshot();
    for (label, peak) in [
        ("Master level", snap.master_peak),
        ("Headphones level", snap.cue_peak),
    ] {
        rows.row(label, "", |ui| {
            ui.with_layout(egui::Layout::top_down(egui::Align::Max), |ui| {
                for p in peak {
                    level_bar(ui, p);
                    ui.add_space(3.0);
                }
            });
        });
    }
}

/// A switch row; true when it was clicked. The switch is in the UI map as
/// `name`.
fn toggle(
    rows: &mut Rows<'_>,
    map: &mut UiMap,
    label: &str,
    desc: &str,
    name: &'static str,
    on: bool,
) -> bool {
    rows.row(label, desc, |ui| {
        let r = switch(ui, on);
        map.add(ui, name, None, r.rect);
        r.clicked()
    })
}

/// A row with a segmented choice; returns the option clicked. Each option
/// is in the UI map as `name` with its index as the deck number.
fn choice<T: PartialEq + Copy>(
    rows: &mut Rows<'_>,
    map: &mut UiMap,
    label: &str,
    desc: &str,
    name: &'static str,
    options: &[(T, &str)],
    now: T,
) -> Option<T> {
    rows.row(label, desc, |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let labels: Vec<&str> = options.iter().map(|(_, l)| *l).collect();
            let sel = options.iter().position(|(v, _)| *v == now);
            let (c, rects) = segmented(ui, &labels, sel);
            for (i, r) in rects.into_iter().enumerate() {
                map.add(ui, name, Some(i), r);
            }
            c.map(|i| options[i].0)
        })
        .inner
    })
}

/// A row with a drop-down; returns the option picked.
fn dropdown<T: PartialEq + Copy>(
    rows: &mut Rows<'_>,
    map: &mut UiMap,
    label: &str,
    desc: &str,
    name: &'static str,
    options: &[(T, String)],
    now: T,
) -> Option<T> {
    rows.row(label, desc, |ui| {
        let current = options
            .iter()
            .find(|(v, _)| *v == now)
            .map_or_else(|| "Custom".to_string(), |(_, l)| l.clone());
        let mut picked = None;
        let r = egui::ComboBox::from_id_salt(name)
            .selected_text(current)
            .width(160.0)
            .height(360.0)
            .show_ui(ui, |ui| {
                for (v, l) in options {
                    if ui.selectable_label(*v == now, l.as_str()).clicked() {
                        picked = Some(*v);
                    }
                }
            });
        map.add(ui, name, None, r.response.rect);
        picked
    })
}

/// Display: theme, scale and full screen, the waveforms, the library and
/// track information, the clock and closing.
fn display(app: &mut KaldorApp, ui: &mut egui::Ui, pal: &Palette) {
    use dj_core::KeyNotation;
    use dj_core::config::WaveformColorMode;
    let label = |m: WaveformColorMode| match m {
        WaveformColorMode::ThreeBand => "3-band",
        WaveformColorMode::Monochrome => "Mono",
        WaveformColorMode::Blue => "Blue",
    };
    let mut changed = false;
    card(ui, pal, "Appearance", |rows| {
        if toggle(
            rows,
            &mut app.map,
            "Dark theme",
            "Off: the light theme.",
            "theme",
            app.config.ui.dark_theme,
        ) {
            app.config.ui.dark_theme = !app.config.ui.dark_theme;
            changed = true;
        }
        let on = app.config.ui.fullscreen;
        if toggle(
            rows,
            &mut app.map,
            "Full screen",
            "Covers the whole screen, taskbar included (Shift+F11).",
            "fullscreen",
            on,
        ) {
            app.set_fullscreen(!on);
        }
        let scales: Vec<(u32, String)> = [0u32, 80, 90, 100, 110, 125, 150, 175, 200]
            .into_iter()
            .map(|p| {
                (
                    p,
                    if p == 0 {
                        "Like the display".into()
                    } else {
                        format!("{p} %")
                    },
                )
            })
            .collect();
        let now = (app.config.ui.ui_scale * 100.0).round() as u32;
        if let Some(p) = dropdown(
            rows,
            &mut app.map,
            "Size of everything",
            "Makes the whole window larger or smaller, on top of the display's scaling.",
            "ui_scale",
            &scales,
            now,
        ) {
            app.config.ui.ui_scale = p as f32 / 100.0;
            changed = true;
        }
        let fps: Vec<(u32, String)> = [0u32, 30, 60, 120]
            .into_iter()
            .map(|f| {
                (
                    f,
                    if f == 0 {
                        "Every display frame".into()
                    } else {
                        format!("{f} per second")
                    },
                )
            })
            .collect();
        if let Some(f) = dropdown(
            rows,
            &mut app.map,
            "Frame rate",
            "A lower rate saves battery; the waveforms scroll less smoothly.",
            "max_fps",
            &fps,
            app.config.ui.max_fps,
        ) {
            app.config.ui.max_fps = f;
            changed = true;
        }
    });
    card(ui, pal, "Waveforms", |rows| {
        rows.row(
            "Colours",
            "3-band shows lows, mids and highs in their own colours.",
            |ui| {
                let combo = egui::ComboBox::from_id_salt("wf_colors")
                    .selected_text(label(app.config.ui.waveform_colors))
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for m in [
                            WaveformColorMode::ThreeBand,
                            WaveformColorMode::Monochrome,
                            WaveformColorMode::Blue,
                        ] {
                            if ui
                                .selectable_label(app.config.ui.waveform_colors == m, label(m))
                                .clicked()
                            {
                                app.config.ui.waveform_colors = m;
                                changed = true;
                            }
                        }
                    });
                app.map
                    .add(ui, "waveform_colors", None, combo.response.rect);
            },
        );
        rows.row(
            "Zoom",
            "Beats shown at 120 BPM, the same for every deck. The + and - keys and the mouse wheel change it too.",
            |ui| {
                use dj_core::config::{ZOOM_MAX_BEATS, ZOOM_MIN_BEATS, clamp_zoom};
                let mut z = clamp_zoom(app.config.ui.zoom_beats);
                let r = ui.add(
                    egui::Slider::new(&mut z, ZOOM_MIN_BEATS..=ZOOM_MAX_BEATS)
                        .logarithmic(true)
                        .fixed_decimals(0)
                        .suffix(" beats"),
                );
                if r.changed() {
                    app.config.ui.zoom_beats = clamp_zoom(z);
                    changed = true;
                }
                app.map.add(ui, "waveform_zoom", None, r.rect);
            },
        );
        if let Some(at) = choice(
            rows,
            &mut app.map,
            "Playhead",
            "Where the play position sits across the waveforms. Left third shows more of what is coming.",
            "playhead_at",
            &[(50u32, "Middle"), (33, "Left third")],
            if app.config.ui.playhead_at < 0.45 {
                33
            } else {
                50
            },
        ) {
            app.config.ui.playhead_at = at as f32 / 100.0;
            changed = true;
        }
        if toggle(
            rows,
            &mut app.map,
            "Beat and bar lines",
            "The beatgrid on the waveforms.",
            "grid_lines",
            app.config.ui.grid_lines,
        ) {
            app.config.ui.grid_lines = !app.config.ui.grid_lines;
            changed = true;
        }
        if toggle(
            rows,
            &mut app.map,
            "Bar numbers",
            "Numbers next to the bar lines (Extended layout).",
            "bar_numbers",
            app.config.ui.bar_numbers,
        ) {
            app.config.ui.bar_numbers = !app.config.ui.bar_numbers;
            changed = true;
        }
    });
    card(ui, pal, "Library and track information", |rows| {
        if let Some(k) = choice(
            rows,
            &mut app.map,
            "Key notation",
            "How keys are written on the decks and in the library: Camelot (8A), Open Key (1m) or musical (Am).",
            "key_notation",
            &[
                (KeyNotation::Camelot, "Camelot"),
                (KeyNotation::OpenKey, "Open Key"),
                (KeyNotation::Musical, "Musical"),
            ],
            app.config.ui.key_notation,
        ) && k != app.config.ui.key_notation
        {
            app.config.ui.key_notation = k;
            changed = true;
        }
        let now = (app.config.ui.library_text_scale * 100.0).round() as u32;
        if let Some(s) = choice(
            rows,
            &mut app.map,
            "Library text size",
            "Bigger rows are easier to read in a dark booth.",
            "library_text",
            &[
                (90u32, "Small"),
                (100, "Normal"),
                (120, "Large"),
                (140, "Larger"),
            ],
            now,
        ) {
            app.config.ui.library_text_scale = s as f32 / 100.0;
            changed = true;
        }
        let mut colors: Vec<(Option<u32>, String)> = vec![(None, "Each pad its own".into())];
        let names = [
            "Red", "Orange", "Yellow", "Green", "Cyan", "Blue", "Purple", "Pink",
        ];
        colors.extend(
            crate::perform::PAD_COLORS
                .iter()
                .zip(names)
                .map(|(c, n)| (Some(*c), n.to_string())),
        );
        if let Some(c) = dropdown(
            rows,
            &mut app.map,
            "New hot cue colour",
            "The colour a new hot cue gets. Cues you already set keep theirs.",
            "hot_cue_color",
            &colors,
            app.config.ui.hot_cue_color,
        ) {
            app.config.ui.hot_cue_color = c;
            changed = true;
        }
    });
    card(ui, pal, "General", |rows| {
        if toggle(
            rows,
            &mut app.map,
            "24-hour clock",
            "The clock in the top bar. Off: 9:30 pm.",
            "clock_24h",
            app.config.ui.clock_24h,
        ) {
            app.config.ui.clock_24h = !app.config.ui.clock_24h;
            changed = true;
        }
        if toggle(
            rows,
            &mut app.map,
            "Ask before closing",
            "When a deck is playing, closing Kaldor asks first.",
            "confirm_close",
            app.config.ui.confirm_close,
        ) {
            app.config.ui.confirm_close = !app.config.ui.confirm_close;
            changed = true;
        }
    });
    if changed {
        app.events.push(crate::app::AppEvent::ConfigChanged);
    }
}

/// Decks: sync and quantize, tempo, transport, loading, the sizes loops,
/// jumps and the slicer start at, and the sampler's recording length.
fn decks(app: &mut KaldorApp, ui: &mut egui::Ui, pal: &Palette) {
    use crate::perform::SLICER_DOMAINS;
    use dj_core::config::{CueMode, KeyLockEngine, LoadPosition, TimeDisplay};
    let mut changed = false;
    let mut engine = false;
    card(ui, pal, "Sync and quantize", |rows| {
        let on = app.config.performance.quantize;
        if toggle(
            rows,
            &mut app.map,
            "Quantize",
            "Cues, loops and jumps snap to the beatgrid (all decks).",
            "quantize",
            on,
        ) {
            app.set_quantize(!on);
        }
        let on = app.config.performance.smart_sync;
        if toggle(
            rows,
            &mut app.map,
            "Smart sync",
            "SYNC matches the beat phase too; off: tempo only.",
            "smart_sync",
            on,
        ) {
            app.set_smart_sync(!on);
        }
        if toggle(
            rows,
            &mut app.map,
            "Master follows the music",
            "When the sync master stops, a playing synced deck becomes the master. Off: the master stays until you choose another.",
            "auto_sync_master",
            app.config.performance.auto_sync_master,
        ) {
            app.config.performance.auto_sync_master = !app.config.performance.auto_sync_master;
            engine = true;
        }
    });
    card(ui, pal, "Tempo", |rows| {
        if toggle(
            rows,
            &mut app.map,
            "Key lock at start",
            "Every deck starts with key lock on, so tempo changes keep the pitch. KEY on a deck still turns it off.",
            "key_lock_default",
            app.config.performance.key_lock_default,
        ) {
            app.config.performance.key_lock_default = !app.config.performance.key_lock_default;
            changed = true;
        }
        if let Some(r) = choice(
            rows,
            &mut app.map,
            "Pitch range",
            "How far the pitch fader goes, on every deck (the range button changes it per deck).",
            "pitch_range",
            &[(8u8, "±8 %"), (16, "±16 %"), (50, "±50 %")],
            app.config.performance.pitch_range_percent,
        ) {
            app.config.performance.pitch_range_percent = r;
            app.send_pitch_range();
            changed = true;
        }
        if let Some(k) = choice(
            rows,
            &mut app.map,
            "Key lock sound",
            "Best: Signalsmith Stretch. Light: WSOLA, less CPU, can wobble on long notes; key shift needs Best.",
            "key_lock_engine",
            &[
                (KeyLockEngine::Signalsmith, "Best"),
                (KeyLockEngine::Wsola, "Light"),
            ],
            app.config.performance.key_lock_engine,
        ) {
            app.config.performance.key_lock_engine = k;
            engine = true;
        }
    });
    card(ui, pal, "Transport", |rows| {
        if let Some(c) = choice(
            rows,
            &mut app.map,
            "CUE while playing",
            "Pause: back to the cue point and stop (CDJ). Keep playing: back to the cue point and play on (stutter).",
            "cue_mode",
            &[(CueMode::Cdj, "Pause"), (CueMode::Stutter, "Keep playing")],
            app.config.performance.cue_mode,
        ) {
            app.config.performance.cue_mode = c;
            engine = true;
        }
        rows.row(
            "Stop brake",
            "Pausing slows the deck down to a stop like a turntable, over this time. 0: stops at once.",
            |ui| {
                let mut s = app.config.performance.stop_brake_seconds;
                let r = ui.add(
                    egui::Slider::new(&mut s, 0.0..=3.0)
                        .step_by(0.1)
                        .fixed_decimals(1)
                        .suffix(" s"),
                );
                if r.changed() {
                    app.config.performance.stop_brake_seconds = s;
                    engine = true;
                }
                app.map.add(ui, "stop_brake", None, r.rect);
            },
        );
        if let Some(v) = choice(
            rows,
            &mut app.map,
            "Jog mode",
            "Vinyl: touching the jog top scratches. Nudge: the jog only bends the tempo. For every deck (SCRATCH changes it per deck).",
            "jog_mode",
            &[(true, "Vinyl"), (false, "Nudge")],
            app.config.performance.jog_vinyl,
        ) {
            app.config.performance.jog_vinyl = v;
            for d in &mut app.decks {
                d.jog_vinyl = v;
            }
            changed = true;
        }
        if let Some(t) = choice(
            rows,
            &mut app.map,
            "Time shown",
            "On the deck panels.",
            "time_display",
            &[
                (TimeDisplay::Both, "Both"),
                (TimeDisplay::Elapsed, "Elapsed"),
                (TimeDisplay::Remaining, "Remaining"),
            ],
            app.config.performance.time_display,
        ) {
            app.config.performance.time_display = t;
            changed = true;
        }
    });
    card(ui, pal, "Loading", |rows| {
        if let Some(p) = choice(
            rows,
            &mut app.map,
            "Start at",
            "Where a newly loaded track waits: the start, its main cue, or its bar 1 marker (where you set BEAT 1, else the first downbeat).",
            "load_position",
            &[
                (LoadPosition::Start, "Start"),
                (LoadPosition::MainCue, "Main cue"),
                (LoadPosition::FirstBeat, "Bar 1"),
            ],
            app.config.performance.load_position,
        ) {
            app.config.performance.load_position = p;
            changed = true;
        }
        if toggle(
            rows,
            &mut app.map,
            "Reset the tempo",
            "A newly loaded track starts at 0 % pitch. Off: the deck keeps its tempo.",
            "reset_tempo_on_load",
            app.config.performance.reset_tempo_on_load,
        ) {
            app.config.performance.reset_tempo_on_load =
                !app.config.performance.reset_tempo_on_load;
            changed = true;
        }
        if toggle(
            rows,
            &mut app.map,
            "Instant doubles",
            "Loading the track that is on another deck copies its position, tempo, key lock, loop and play state.",
            "instant_doubles",
            app.config.performance.instant_doubles,
        ) {
            app.config.performance.instant_doubles = !app.config.performance.instant_doubles;
            changed = true;
        }
        if toggle(
            rows,
            &mut app.map,
            "Lock playing decks",
            "A deck that is playing does not take a new track, so a slip of the mouse cannot stop the music.",
            "lock_playing_deck",
            app.config.performance.lock_playing_deck,
        ) {
            app.config.performance.lock_playing_deck = !app.config.performance.lock_playing_deck;
            changed = true;
        }
    });
    // The sizes each deck starts with; a change here also goes to every
    // deck now (the deck's own buttons and keys change it per deck).
    let jumps: [f32; 9] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0];
    let loops: [f32; 11] = [
        1.0 / 32.0,
        1.0 / 16.0,
        0.125,
        0.25,
        0.5,
        1.0,
        2.0,
        4.0,
        8.0,
        16.0,
        32.0,
    ];
    let named = |v: &[f32]| -> Vec<(u32, String)> {
        v.iter()
            .map(|b| ((b * 1000.0) as u32, beats_name(*b)))
            .collect()
    };
    let key = |b: f32| (b * 1000.0) as u32;
    card(ui, pal, "Loops and jumps", |rows| {
        let p = app.config.performance.clone();
        if let Some(b) = dropdown(
            rows,
            &mut app.map,
            "Beat jump size",
            "How far a beat jump moves. Applies to every deck now; the jump size keys change it per deck.",
            "jump_size",
            &named(&jumps),
            key(p.beat_jump_beats),
        ) {
            let b = b as f32 / 1000.0;
            app.config.performance.beat_jump_beats = b;
            app.decks.iter_mut().for_each(|d| d.perf.jump_beats = b);
            changed = true;
        }
        if let Some(b) = dropdown(
            rows,
            &mut app.map,
            "Auto loop size",
            "The length of an auto loop. Applies to every deck now; halve and double change it per deck.",
            "auto_loop_size",
            &named(&loops),
            key(p.auto_loop_beats),
        ) {
            let b = b as f32 / 1000.0;
            app.config.performance.auto_loop_beats = b;
            app.decks
                .iter_mut()
                .for_each(|d| d.perf.auto_loop_beats = b);
            changed = true;
        }
        if let Some(b) = dropdown(
            rows,
            &mut app.map,
            "Slicer length",
            "The beats the slicer cuts into its 8 slices.",
            "slicer_size",
            &named(&SLICER_DOMAINS),
            key(p.slicer_domain_beats),
        ) {
            let b = b as f32 / 1000.0;
            app.config.performance.slicer_domain_beats = b;
            app.decks.iter_mut().for_each(|d| d.perf.slicer_domain = b);
            changed = true;
        }
    });
    card(ui, pal, "Sampler", |rows| {
        if let Some(b) = dropdown(
            rows,
            &mut app.map,
            "Recording length",
            "How long a sampler slot records from the master, in beats of the master tempo (8 seconds when there is none).",
            "sampler_record_beats",
            &named(&[4.0, 8.0, 16.0, 32.0, 64.0]),
            key(app.config.performance.sampler_record_beats),
        ) {
            app.config.performance.sampler_record_beats = b as f32 / 1000.0;
            changed = true;
        }
    });
    if engine {
        app.send_performance_settings();
        changed = true;
    }
    if changed {
        app.events.push(crate::app::AppEvent::ConfigChanged);
    }
}

/// The DJ controller: which one, its connection, and MIDI learn.
fn controller(app: &mut KaldorApp, ui: &mut egui::Ui, pal: &Palette) {
    let s = app.controller_status();
    let attached = app.controller.is_some();
    card(ui, pal, "Controller", |rows| {
        rows.row(
            "Status",
            &if s.is_empty() {
                "No controller (started with --no-controller, or turned off below).".to_string()
            } else {
                s
            },
            |_| {},
        );
        rows.row(
            "Use a controller",
            "Look for a controller when Kaldor starts. Takes effect at the next start.",
            |ui| {
                let r = switch(ui, app.config.controller.enabled);
                if r.clicked() {
                    app.config.controller.enabled = !app.config.controller.enabled;
                    app.events.push(crate::app::AppEvent::ConfigChanged);
                }
                app.map.add(ui, "controller_enabled", None, r.rect);
            },
        );
        // Which controller: any supported one that is plugged in, or one.
        const MODELS: [(&str, &str); 3] = [
            ("auto", "Automatic"),
            ("ns4fx", "Numark NS4FX"),
            ("ddj-flx6", "Pioneer DJ DDJ-FLX6"),
        ];
        rows.row(
            "Controller",
            "Automatic uses whichever supported controller is plugged in (also when you plug it in later). Takes effect at the next start. A mapping file below replaces this.",
            |ui| {
                let labels: Vec<&str> = MODELS.iter().map(|m| m.1).collect();
                let sel = MODELS
                    .iter()
                    .position(|m| m.0 == app.config.controller.model)
                    .unwrap_or(0);
                let (c, rects) = segmented(ui, &labels, Some(sel));
                for (r, name) in rects.iter().zip(["model_auto", "model_ns4fx", "model_ddj_flx6"]) {
                    app.map.add(ui, name, None, *r);
                }
                if let Some(c) = c.filter(|c| *c != sel) {
                    app.config.controller.model = MODELS[c].0.into();
                    app.events.push(crate::app::AppEvent::ConfigChanged);
                }
            },
        );
        let unknown = app.controller_unknown();
        let mut changed = false;
        rows.row(
            "Jog sensitivity",
            "How far the music moves per turn of the jog wheel when scratching, bending and searching. 1.0: one turn is one turn of a record.",
            |ui| {
                let mut s = app.config.controller.jog_sensitivity;
                let r = ui.add(
                    egui::Slider::new(&mut s, 0.25..=4.0)
                        .logarithmic(true)
                        .fixed_decimals(2)
                        .suffix("×"),
                );
                if r.changed() {
                    app.config.controller.jog_sensitivity = s;
                    changed = true;
                }
                app.map.add(ui, "jog_sensitivity", None, r.rect);
            },
        );
        if toggle(
            rows,
            &mut app.map,
            "Soft takeover",
            "Knobs, faders and the pitch fader do nothing until they reach the value on screen (after a LAYER switch or a change with the mouse). Off: they jump to where the control is.",
            "soft_takeover",
            app.config.controller.soft_takeover,
        ) {
            app.config.controller.soft_takeover = !app.config.controller.soft_takeover;
            changed = true;
        }
        let profile = app
            .config
            .controller
            .profile
            .as_ref()
            .map(|p| p.display().to_string());
        rows.row(
            "Mapping file",
            "A controller profile (TOML) to use instead of the built-in mappings. Takes effect at the next start.",
            |ui| {
                let d = button(ui, "Built-in", false, None);
                if d.clicked() && app.config.controller.profile.is_some() {
                    app.config.controller.profile = None;
                    app.profile_edit = None;
                    changed = true;
                }
                app.map.add(ui, "profile_default", None, d.rect);
                let edit = app
                    .profile_edit
                    .get_or_insert_with(|| profile.clone().unwrap_or_default());
                let r = ui.add(
                    egui::TextEdit::singleline(edit)
                        .hint_text("Built-in mapping")
                        .desired_width(220.0),
                );
                app.map.add(ui, "profile_path", None, r.rect);
                let start = app
                    .profile_edit
                    .clone()
                    .or_else(|| profile.clone())
                    .unwrap_or_default();
                let (br, picked) = crate::browse::button(
                    ui,
                    "controller_profile",
                    "Controller profile",
                    crate::browse::Pick::File("Controller profile", &["toml"]),
                    &start,
                );
                app.map.add(ui, "profile_browse", None, br.rect);
                if let Some(p) = picked {
                    app.profile_edit = None;
                    if Some(&p) != app.config.controller.profile.as_ref() {
                        app.config.controller.profile = Some(p);
                        changed = true;
                    }
                } else if r.lost_focus() {
                    let text = app.profile_edit.take().unwrap_or_default();
                    let new = (!text.trim().is_empty()).then(|| std::path::PathBuf::from(text.trim()));
                    if new != app.config.controller.profile {
                        app.config.controller.profile = new;
                        changed = true;
                    }
                }
            },
        );
        if changed {
            app.events.push(crate::app::AppEvent::ConfigChanged);
        }
        if !unknown.is_empty() {
            rows.block(|ui| {
                ui.label(
                    RichText::new("Messages the profile does not know (newest last)")
                        .size(12.0)
                        .color(pal.text_weak),
                );
                for m in unknown {
                    ui.monospace(m);
                }
            });
        }
    });
    if attached {
        midi_learn(app, ui, pal);
    }
}

/// MIDI learn: pick a function (and deck), press Learn, use the control.
fn midi_learn(app: &mut KaldorApp, ui: &mut egui::Ui, pal: &Palette) {
    use dj_core::config::MidiFunction;
    let all = MidiFunction::all();
    let (mut pick, mut deck) = app.learn_pick;
    pick = pick.min(all.len() - 1);
    let function = all[pick];
    let mut marks: Vec<(&'static str, egui::Rect)> = Vec::new();
    let learning = app.learning().is_some();
    let mut start = false;
    let learned: Vec<String> = app
        .learned()
        .iter()
        .map(crate::controller::learned_text)
        .collect();
    let status = app.learn_status.clone();
    let mut remove = None;
    let mut clear = false;
    card(ui, pal, "MIDI learn", |rows| {
        rows.row(
            "Learn a control",
            if status.is_empty() {
                "Pick a function, press Learn, then move the control. It replaces what the control did."
            } else {
                &status
            },
            |ui| {
                let b = button(ui, if learning { "Cancel" } else { "Learn" }, true, None);
                marks.push(("learn_start", b.rect));
                start = b.clicked();
                if all[pick].needs_deck() {
                    let r = egui::ComboBox::from_id_salt("learn_deck")
                        .selected_text(format!("Deck {deck}"))
                        .width(80.0)
                        .show_ui(ui, |ui| {
                            for d in 1..=4u8 {
                                ui.selectable_value(&mut deck, d, format!("Deck {d}"));
                            }
                        });
                    marks.push(("learn_deck", r.response.rect));
                }
                let r = egui::ComboBox::from_id_salt("learn_function")
                    .selected_text(function.label())
                    .width(170.0)
                    .height(400.0)
                    .show_ui(ui, |ui| {
                        for (i, f) in all.iter().enumerate() {
                            ui.selectable_value(&mut pick, i, f.label());
                        }
                    });
                marks.push(("learn_function", r.response.rect));
            },
        );
        if learned.is_empty() {
            rows.row(
                "Learned controls",
                "None: the controller works as its profile says.",
                |_| {},
            );
        } else {
            for (i, t) in learned.iter().enumerate() {
                rows.row(t, "", |ui| {
                    let b = button(ui, "Remove", false, None);
                    if b.clicked() {
                        remove = Some(i);
                    }
                    marks.push(("learn_remove", b.rect));
                });
            }
            rows.row(
                "Back to the profile",
                "Removes every learned control.",
                |ui| {
                    let b = button(ui, "Remove all", false, None);
                    clear = b.clicked();
                    marks.push(("learn_clear", b.rect));
                },
            );
        }
        for (name, r) in marks.drain(..) {
            app.map.add(rows.ui, name, None, r);
        }
    });
    app.learn_pick = (pick, deck);
    if start {
        if learning {
            app.cancel_learn();
        } else {
            app.start_learn(all[pick], Some(deck));
        }
    }
    if let Some(i) = remove {
        app.remove_learned(i);
    }
    if clear {
        app.clear_learned();
    }
}
