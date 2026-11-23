//! Decks on screen: the side columns (LAYER buttons and deck panels) and
//! the waveform strips in the centre.

use crate::app::{KaldorApp, region_ui};
use crate::layout::{Frame, PanelSize, PanelSlot, Slot};
use crate::loader::LoadJob;
use crate::theme::{self, Palette, deck_color};
use crate::uimap::Region;
use crate::waveform_view::{ZoomedParams, paint_overview, paint_zoomed};
use crate::widgets::{self, Icon, flat_button, is_held};
use dj_analysis::Waveform;
use dj_audio::{Command, DeckCommand, DeckSnapshot, PitchRange};
use dj_core::DeckId;
use dj_core::config::DeckLayout;
use dj_core::time::format_mm_ss_t;
use egui::{Align, Color32, Id, Layout, Rect, Sense, Stroke, UiBuilder, pos2, vec2};
use std::path::PathBuf;
use std::sync::Arc;

/// UI-side state for one deck.
pub(crate) struct DeckUi {
    pub title: String,
    pub path: Option<PathBuf>,
    pub job: Option<LoadJob>,
    /// The track's waveform (from the library, or built at load): the
    /// overviews, and the scrolling view until `detail` is there.
    pub waveform: Option<Arc<Waveform>>,
    /// The fine waveform for the scrolling view (about 1200 points per
    /// second), built from the decoded audio after each load; memory only.
    pub detail: Option<Arc<Waveform>>,
    pub status: Option<String>,
    pub path_input: String,
    pub scrub_anchor: Option<f64>,
    pub cue_held: bool,
    pub bend_held: i8,
    /// Library track on this deck, if it came from the library.
    pub track_id: Option<dj_library::TrackId>,
    pub bpm: Option<f64>,
    pub key: Option<dj_core::MusicalKey>,
    pub grid: Option<dj_core::Beatgrid>,
    /// The track's loudness (LUFS) and the auto gain stored with it, so a
    /// change of the auto gain settings reaches a loaded track.
    pub loudness: (Option<f64>, Option<f64>),
    /// A Ctrl-drag on the waveform that is moving the grid.
    pub grid_drag: Option<crate::perform::GridDrag>,
    /// Jog mode for the controller: touching the jog top scratches (true,
    /// vinyl) or the jog only nudges.
    pub jog_vinyl: bool,
    /// Play count already recorded for this load.
    pub play_recorded: bool,
    /// Smooth playhead for the scrolling waveform, and when it was last drawn.
    pub scroll: crate::waveform_view::ScrollClock,
    /// The frame the scroll position was last computed in, and that
    /// position (frames).
    pub scroll_frame: Option<u64>,
    pub scroll_pos: f64,
    /// Position (frames) to seek to once a reload starts (after the audio
    /// output changed).
    pub resume_at: Option<f64>,
    /// The load in progress was started by the user (not a reload after
    /// the audio output changed): load position, tempo reset and instant
    /// doubles apply.
    pub fresh_load: bool,
    /// Instant doubles: the deck whose position, tempo and loop the new
    /// load copies.
    pub double_of: Option<usize>,
    /// Pads, hot cues, saved loops, loop and jump sizes.
    pub perf: crate::perform::DeckPerf,
    /// Hot cue rows of the Extended panel held down (like pads, but
    /// whatever the pad mode).
    pub cue_list_held: [bool; dj_audio::HOT_CUES],
    /// What the Extended panel shows next to CUE and PLAY: the hot cues or
    /// the beatgrid controls.
    pub panel_tab: PanelTab,
}

/// The tabs of an Extended deck panel's hot cue area.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PanelTab {
    #[default]
    Cues,
    Grid,
}

impl DeckUi {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            path: None,
            job: None,
            waveform: None,
            detail: None,
            status: None,
            path_input: String::new(),
            scrub_anchor: None,
            cue_held: false,
            bend_held: 0,
            track_id: None,
            bpm: None,
            key: None,
            grid: None,
            loudness: (None, None),
            grid_drag: None,
            jog_vinyl: true,
            play_recorded: false,
            scroll: crate::waveform_view::ScrollClock::default(),
            scroll_frame: None,
            scroll_pos: 0.0,
            resume_at: None,
            fresh_load: false,
            double_of: None,
            perf: crate::perform::DeckPerf::default(),
            cue_list_held: [false; dj_audio::HOT_CUES],
            panel_tab: PanelTab::Cues,
        }
    }

    /// Forgets library information (before loading something new).
    pub fn clear_track_info(&mut self) {
        self.track_id = None;
        self.bpm = None;
        self.key = None;
        self.grid = None;
        self.loudness = (None, None);
        self.grid_drag = None;
        self.play_recorded = false;
        self.resume_at = None;
        self.perf.clear_markers();
    }
}

/// Pitch bend amount while a bend button is held.
const BEND: f64 = 0.04;
/// Narrowest KEY / SYNC / SLIP / VINYL button that keeps its label.
const VINYL_MIN_W: f32 = 44.0;
/// Tempo that turns the zoom (in beats) into the waveform window in seconds.
const DEFAULT_BPM: f64 = 120.0;
/// Scroll (points) that halves or doubles the waveform zoom: three wheel
/// notches of 40 points.
pub const ZOOM_SCROLL_PER_DOUBLING: f32 = 120.0;

/// Splits "Artist - Title" (a library track's display name) for two lines.
pub(crate) fn artist_title(title: &str) -> (&str, &str) {
    match title.split_once(" - ") {
        Some((a, t)) if !a.is_empty() && !t.is_empty() => (a, t),
        _ => ("", title),
    }
}

/// Loads a library track dropped on `rect`, and records `rect` for files
/// dropped from the OS and for the drop outline (`paint_drop_highlight`).
pub(crate) fn drop_target(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    rect: Rect,
    index: usize,
    salt: &str,
) {
    app.drop_rects.push((index, rect));
    let r = ui.interact(rect, Id::unique(("deck_drop", salt, index)), Sense::hover());
    if let Some(p) = r.dnd_release_payload::<crate::library_view::TrackDrag>() {
        app.load_library_track(DeckId::ALL[index], p.0);
    }
    // An audio file from the Files panel (the first one when several).
    if let Some(f) = r.dnd_release_payload::<crate::files_panel::FileDrag>()
        && let Some(p) = f.0.iter().find(|p| dj_library::is_audio_file(p))
    {
        app.load_path(DeckId::ALL[index], p.clone());
    }
}

/// One side column: the LAYER row and the deck panels.
pub(crate) fn side_column(app: &mut KaldorApp, ui: &mut egui::Ui, frame: &Frame, side: usize) {
    let col = if side == 0 { frame.left } else { frame.right };
    let region = if side == 0 {
        Region::LeftDecks
    } else {
        Region::RightDecks
    };
    app.map.enter(region, col);
    let mut ui = region_ui(ui, col, Layout::top_down(Align::Min), ("side", side));
    if let Some(rows) = frame.layer_rows {
        layer_row(app, &mut ui, rows[side], side);
    }
    for slot in frame.panels.iter().filter(|p| p.deck % 2 == side) {
        deck_panel(app, &mut ui, slot, side);
    }
}

/// "LAYER" and the two deck buttons of a side (1/3 left, 2/4 right), like
/// the NS4FX LAYER buttons.
fn layer_row(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect, side: usize) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let bw = 44.0;
    let label_w = 48.0;
    let (label_rect, b0) = if side == 0 {
        (
            Rect::from_min_size(rect.min, vec2(label_w, rect.height())),
            rect.left() + label_w,
        )
    } else {
        (
            Rect::from_min_max(pos2(rect.right() - label_w, rect.top()), rect.max),
            rect.right() - label_w - 2.0 * bw - 4.0,
        )
    };
    let painter = ui.painter();
    painter.text(
        if side == 0 {
            label_rect.left_center()
        } else {
            label_rect.right_center()
        },
        if side == 0 {
            egui::Align2::LEFT_CENTER
        } else {
            egui::Align2::RIGHT_CENTER
        },
        "LAYER",
        theme::prop(11.0),
        pal.text_weak,
    );
    for (k, deck) in [side, side + 2].into_iter().enumerate() {
        let id = DeckId::ALL[deck];
        let r = Rect::from_min_size(
            pos2(b0 + k as f32 * (bw + 4.0), rect.top() + 1.0),
            vec2(bw, rect.height() - 2.0),
        );
        let resp = flat_button(
            ui,
            r,
            Id::unique(("layer", deck)),
            &format!("{}", id.number()),
            theme::prop(13.0),
            None,
            app.layer[side] == deck,
            deck_color(id),
            false,
        )
        .on_hover_text(format!(
            "LAYER: deck {} on the {} side",
            id.number(),
            if side == 0 { "left" } else { "right" }
        ));
        if resp.clicked() {
            app.layer[side] = deck;
        }
        app.map.add(ui, "layer", Some(deck), r);
    }
}

/// Values shown on a deck panel, read once per frame.
pub(crate) struct DeckView {
    pub index: usize,
    pub id: DeckId,
    pub d: DeckSnapshot,
    pub pos: f64,
    pub sr: f64,
    pub color: Color32,
    pub pal: Palette,
}

impl DeckView {
    /// Widget id for one of this deck's controls.
    pub(crate) fn uid(&self, name: &str) -> Id {
        Id::unique((name, self.index))
    }
}

/// One deck panel at its slot size.
fn deck_panel(app: &mut KaldorApp, ui: &mut egui::Ui, slot: &PanelSlot, side: usize) {
    let index = slot.deck;
    let Some(id) = DeckId::from_index(index) else {
        return;
    };
    let d = app.engine.snapshot().decks[index];
    let sr = f64::from(app.engine.snapshot().sample_rate.max(1));
    let pos = app.display_position(index, &d);
    let v = DeckView {
        index,
        id,
        d,
        pos,
        sr,
        color: deck_color(id),
        pal: Palette::get(app.config.ui.dark_theme),
    };
    let rect = slot.rect;
    drop_target(app, ui, rect, index, "panel");
    let focused = app.layer[side] == index && !app.config.ui.deck_view().1;
    ui.painter().rect(
        rect,
        6.0,
        v.pal.raised,
        Stroke::new(
            if focused { 1.5 } else { 1.0 },
            if focused { v.color } else { v.pal.border },
        ),
        egui::StrokeKind::Inside,
    );
    app.map.add(ui, "panel", Some(index), rect);
    let mut ui = region_ui(
        ui,
        rect,
        Layout::top_down(Align::Min),
        ("deck_panel", index),
    );
    let extended = app.config.ui.layout == DeckLayout::Extended;
    match (slot.size, extended) {
        (PanelSize::Full, false) => full_panel(app, &mut ui, &v, rect, side),
        (PanelSize::Compact, false) => compact_panel(app, &mut ui, &v, rect, side),
        (PanelSize::Full, true) => crate::deck_ext::full(app, &mut ui, &v, rect, side),
        (PanelSize::Compact, true) => crate::deck_ext::compact(app, &mut ui, &v, rect, side),
        (PanelSize::Mini, _) => mini_panel(app, &mut ui, &v, rect, extended.then_some(side)),
    }
}

/// The deck number badge that also switches LAYER (Extended layout, which
/// has no LAYER rows): click to make this deck its side's LAYER deck.
pub(crate) fn layer_badge(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    rect: Rect,
    side: usize,
) {
    badge(ui, rect, v);
    if app.kb_deck == v.index {
        // The keyboard deck (F1-F4): a bar under the number.
        ui.painter().rect_filled(
            Rect::from_min_size(
                pos2(rect.left(), rect.bottom() + 1.5),
                vec2(rect.width(), 2.5),
            ),
            1.0,
            v.pal.text,
        );
    }
    // With 2 decks shown, the badge switches the side to its other deck.
    let two = app.config.ui.deck_view().1;
    let other = v.index ^ 2;
    let r = ui
        .interact(rect, v.uid("layer_badge"), Sense::click())
        .on_hover_text(if two {
            format!(
                "LAYER: click to show and control deck {} on this side",
                other + 1
            )
        } else {
            format!(
                "LAYER: click to control deck {} from the {} side of the controller (outlined)",
                v.id.number(),
                if side == 0 { "left" } else { "right" }
            )
        });
    if r.clicked() {
        app.layer[side] = if two { other } else { v.index };
    }
    app.map.add(ui, "layer", Some(v.index), rect);
}

/// Number badge in the deck colour.
pub(crate) fn badge(ui: &egui::Ui, rect: Rect, v: &DeckView) {
    ui.painter().rect_filled(rect, 4.0, v.color);
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        v.id.number().to_string(),
        theme::display(ui.ctx(), (rect.height() * 0.5).clamp(9.0, 16.0)),
        Palette::on(v.color),
    );
}

/// Badge, title, and artist (or the deck's status message in red).
fn header(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) -> egui::Response {
    let b = Rect::from_min_size(rect.min, vec2(rect.height(), rect.height()));
    badge(ui, b, v);
    if app.kb_deck == v.index {
        // The keyboard deck (F1-F4): a bar under the number.
        ui.painter().rect_filled(
            Rect::from_min_size(pos2(b.left(), b.bottom() + 1.5), vec2(b.width(), 2.5)),
            1.0,
            v.pal.text,
        );
    }
    let text = Rect::from_min_max(pos2(b.right() + 8.0, rect.top()), rect.max);
    title_block(app, ui, v, text, rect)
}

/// Title over artist (or the deck's status message in red) in `text`; the
/// click area (and the "title" control) is `area`.
pub(crate) fn title_block(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    text: Rect,
    area: Rect,
) -> egui::Response {
    let deck = &app.decks[v.index];
    let (artist, title) = artist_title(&deck.title);
    let painter = ui.painter_at(text);
    let (top, bottom) = (
        Rect::from_min_max(text.min, pos2(text.right(), text.center().y + 1.0)),
        Rect::from_min_max(pos2(text.left(), text.center().y + 1.0), text.max),
    );
    let title_text = if deck.title.is_empty() {
        "No track loaded"
    } else {
        title
    };
    widgets::text_in(
        &painter,
        top,
        title_text,
        theme::prop(14.0),
        if deck.title.is_empty() {
            v.pal.text_weak
        } else {
            v.pal.text
        },
        false,
    );
    let mut hover = deck.title.clone();
    if let Some(s) = &deck.status {
        widgets::text_in(&painter, bottom, s, theme::prop(11.5), v.pal.error, false);
        hover = format!("{hover}\n{s}");
    } else {
        widgets::text_in(
            &painter,
            bottom,
            artist,
            theme::prop(12.0),
            v.pal.text_weak,
            false,
        );
    }
    let r = ui.interact(area, v.uid("deck_header"), Sense::click());
    let r = if hover.is_empty() {
        r
    } else {
        r.on_hover_text(hover)
    };
    app.map.add(ui, "title", Some(v.index), area);
    r
}

/// Current BPM in large digits, original BPM, key chip.
pub(crate) fn bpm_row(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect, big: f32) {
    let deck = &app.decks[v.index];
    let painter = ui.painter_at(rect);
    // Key chip on the right, if there is a key.
    // With a key shift the chip shows the key heard, e.g. "10A+2".
    let shift = if v.d.key_lock { v.d.key_shift } else { 0 };
    let chip = (v.d.loaded).then_some(deck.key).flatten().map(|k| {
        let text = k
            .transposed(i32::from(shift))
            .display(app.config.ui.key_notation);
        let text = match shift {
            0 => text,
            s if s > 0 => format!("{text}+{s}"),
            s => format!("{text}{s}"),
        };
        let g = painter.layout_no_wrap(text, theme::display(ui.ctx(), 10.5), Color32::WHITE);
        let r = Rect::from_min_size(
            pos2(rect.right() - g.size().x - 10.0, rect.center().y - 9.0),
            vec2(g.size().x + 10.0, 18.0),
        );
        (g, r)
    });
    let text_right = chip.as_ref().map_or(rect.right(), |(_, r)| r.left() - 6.0);
    let mut x = rect.left();
    if v.d.loaded
        && let Some(bpm) = deck.bpm
    {
        let g = painter.layout_no_wrap(
            format!("{:.2}", bpm * shown_tempo(&v.d)),
            theme::mono(big),
            v.pal.text,
        );
        let w = g.size().x;
        painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, v.pal.text);
        x += w + 6.0;
        let small = Rect::from_min_max(pos2(x, rect.top()), pos2(text_right, rect.bottom()));
        if small.width() > 12.0 {
            if rect.height() >= 24.0 {
                // "BPM" over the original tempo.
                let (top, bottom) = (
                    Rect::from_min_max(small.min, pos2(small.right(), small.center().y)),
                    Rect::from_min_max(pos2(small.left(), small.center().y), small.max),
                );
                // Small capitals, as the labels on the FX display.
                widgets::text_in(
                    &painter,
                    top,
                    "BPM",
                    theme::prop(10.0),
                    v.pal.text_weak,
                    false,
                );
                // Just the number where "ORIG" does not fit too.
                let full = format!("ORIG {bpm:.2}");
                let fits = painter
                    .layout_no_wrap(full.clone(), theme::prop(10.0), v.pal.text_weak)
                    .size()
                    .x
                    <= bottom.width();
                widgets::text_in(
                    &painter,
                    bottom,
                    &if fits { full } else { format!("{bpm:.2}") },
                    theme::prop(10.0),
                    v.pal.text_weak,
                    false,
                );
            } else {
                widgets::text_in(
                    &painter,
                    small,
                    "BPM",
                    theme::prop(10.5),
                    v.pal.text_weak,
                    false,
                );
            }
        }
    } else {
        let r = Rect::from_min_max(rect.min, pos2(text_right, rect.bottom()));
        widgets::text_in(
            &painter,
            r,
            "--.-- BPM",
            theme::mono(big * 0.8),
            v.pal.text_weak,
            false,
        );
    }
    app.map.add(
        ui,
        "bpm",
        Some(v.index),
        Rect::from_min_max(
            rect.min,
            pos2(text_right.max(rect.left() + 1.0), rect.bottom()),
        ),
    );
    // The chip only where it does not cover the tempo.
    if let Some((g, chip)) = chip
        && chip.left() >= x - 2.0
    {
        painter.rect_filled(chip, 4.0, v.pal.accent.gamma_multiply(0.85));
        painter.galley(chip.center() - g.size() / 2.0, g, Color32::WHITE);
        let r = ui
            .interact(chip, v.uid("key_chip"), Sense::hover())
            .on_hover_text("Key");
        app.map.add(ui, "key", Some(v.index), r.rect);
    }
}

/// Elapsed time on the left, remaining on the right, load progress between.
pub(crate) fn time_row(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    rect: Rect,
    size: f32,
) {
    let painter = ui.painter_at(rect);
    if v.d.loaded {
        let remaining = (v.d.length - v.pos).max(0.0) / v.sr;
        let elapsed = format_mm_ss_t(v.pos / v.sr);
        let remaining = format!("-{}", format_mm_ss_t(remaining));
        let w = |s: &str| {
            painter
                .layout_no_wrap(s.to_owned(), theme::mono(size), v.pal.text)
                .size()
                .x
        };
        use dj_core::config::TimeDisplay;
        let shown = app.config.performance.time_display;
        // Elapsed only when both fit; remaining always (unless the
        // settings ask for one of them).
        let fits = w(&elapsed) + w(&remaining) + 8.0 <= rect.width();
        if shown == TimeDisplay::Elapsed || (shown == TimeDisplay::Both && fits) {
            widgets::text_in(
                &painter,
                rect,
                &elapsed,
                theme::mono(size),
                v.pal.text,
                false,
            );
        }
        if shown != TimeDisplay::Elapsed {
            widgets::text_in(
                &painter,
                rect,
                &remaining,
                theme::mono(size),
                v.pal.text_weak,
                true,
            );
        }
        if !v.d.load_complete && v.d.length > 0.0 {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                format!(
                    "loading {:.0}%",
                    v.d.ready_frames as f64 / v.d.length * 100.0
                ),
                theme::prop(11.0),
                v.pal.warn,
            );
        }
    } else {
        widgets::text_in(
            &painter,
            rect,
            "-:--.-",
            theme::mono(size),
            v.pal.text_weak,
            false,
        );
    }
    app.map.add(ui, "time", Some(v.index), rect);
}

/// CUE (hold to preview) and PLAY/PAUSE side by side in `rect`.
pub(crate) fn transport(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    rect: Rect,
    font: f32,
) {
    let half = (rect.width() - 6.0) / 2.0;
    let cue_r = Rect::from_min_size(rect.min, vec2(half, rect.height()));
    let play_r = Rect::from_min_max(pos2(rect.right() - half, rect.top()), rect.max);
    let cue = flat_button(
        ui,
        cue_r,
        v.uid("cue"),
        "CUE",
        theme::prop(font),
        Some(Icon::Cue),
        v.d.previewing || app.decks[v.index].cue_held,
        v.pal.cue,
        true,
    )
    .on_hover_text("CUE: press to set or return; hold to preview");
    let cue_down = is_held(&cue);
    if cue_down != app.decks[v.index].cue_held {
        app.decks[v.index].cue_held = cue_down;
        app.send(Command::Deck(v.id, DeckCommand::Cue(cue_down)));
    }
    app.map.add(ui, "cue", Some(v.index), cue_r);
    let playing = v.d.playing;
    let play = flat_button(
        ui,
        play_r,
        v.uid("play"),
        if playing { "PAUSE" } else { "PLAY" },
        theme::prop(font),
        Some(if playing { Icon::Pause } else { Icon::Play }),
        playing,
        v.pal.play,
        false,
    );
    if play.clicked() {
        app.send(Command::Deck(v.id, DeckCommand::TogglePlay));
    }
    app.map.add(ui, "play", Some(v.index), play_r);
}

/// Everything: header, BPM, key, times, pitch, bend, key lock, load field,
/// CUE and PLAY, with the pitch fader on the outer edge.
fn full_panel(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect, side: usize) {
    let pad = 6.0;
    let fader_w = 28.0;
    let inner = rect.shrink(pad);
    let (fader_col, content) = if side == 0 {
        (
            Rect::from_min_size(inner.min, vec2(fader_w, inner.height())),
            Rect::from_min_max(pos2(inner.left() + fader_w + 6.0, inner.top()), inner.max),
        )
    } else {
        (
            Rect::from_min_max(pos2(inner.right() - fader_w, inner.top()), inner.max),
            Rect::from_min_max(
                inner.min,
                pos2(inner.right() - fader_w - 6.0, inner.bottom()),
            ),
        )
    };
    pitch_fader(app, ui, v, fader_col);

    // Minimum: 32 + 26 + 20 + 3 x 22 + 6 x 3 + 38 = 200. Extra height (tall
    // windows, 2-deck layout) makes the numbers and buttons bigger.
    let s = ((content.height() - 204.0) / 200.0).clamp(0.0, 1.0);
    let gap = 3.0 + 5.0 * s;
    let mut y = content.top();
    let mut row = |h: f32| {
        let r = Rect::from_min_size(pos2(content.left(), y), vec2(content.width(), h));
        y += h + gap;
        r
    };
    let head = row(32.0);
    let bpm = row(26.0 + 14.0 * s);
    let time = row(20.0 + 8.0 * s);
    let pitch = row(22.0);
    let bend = row(22.0 + 4.0 * s);
    let load = row(22.0);
    let th = 38.0 + 26.0 * s;
    let transport_r = Rect::from_min_max(
        pos2(content.left(), (content.bottom() - th).max(y)),
        pos2(content.right(), content.bottom().max(y + 30.0)),
    );
    let h = header(app, ui, v, head)
        .on_hover_text("Click: the keyboard's pad, loop and jump keys work on this deck (F1-F4)");
    if h.clicked() {
        app.set_keyboard_deck(v.index);
    }
    bpm_row(app, ui, v, bpm, 22.0 + 12.0 * s);
    time_row(app, ui, v, time, 15.0 + 5.0 * s);
    pitch_row(app, ui, v, pitch);
    bend_row(app, ui, v, bend);
    load_row(app, ui, v, load);
    transport(app, ui, v, transport_r, 15.0 + 3.0 * s);
}

/// Title, BPM and time on one line, CUE and PLAY. Clicking the header makes
/// this deck the LAYER deck of its side (shown in full when there is room).
fn compact_panel(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect, side: usize) {
    let c = rect.shrink(6.0);
    let head = Rect::from_min_size(c.min, vec2(c.width(), 32.0));
    let info = Rect::from_min_size(pos2(c.left(), head.bottom() + 3.0), vec2(c.width(), 20.0));
    let tr = Rect::from_min_max(
        pos2(c.left(), info.bottom() + 4.0),
        pos2(c.right(), c.bottom().min(info.bottom() + 4.0 + 64.0)),
    );
    let h = header(app, ui, v, head).on_hover_text("Click to show this deck in full (LAYER)");
    if h.clicked() {
        app.layer[side] = v.index;
    }
    let (l, r) = (
        Rect::from_min_size(info.min, vec2(info.width() * 0.45, info.height())),
        Rect::from_min_max(
            pos2(info.left() + info.width() * 0.45, info.top()),
            info.max,
        ),
    );
    bpm_row(app, ui, v, l, 14.0);
    time_row(app, ui, v, r, 13.0);
    transport(app, ui, v, tr, 13.0);
}

/// One line: badge, title, CUE, PLAY. With `layer_side` (Extended layout)
/// the badge switches LAYER.
fn mini_panel(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    rect: Rect,
    layer_side: Option<usize>,
) {
    let c = rect.shrink2(vec2(4.0, 2.0));
    let h = c.height();
    let b = Rect::from_min_size(c.min, vec2(h, h));
    match layer_side {
        Some(side) => layer_badge(app, ui, v, b, side),
        None => badge(ui, b, v),
    }
    let tr_w = (c.width() * 0.45).min(120.0);
    let tr = Rect::from_min_max(pos2(c.right() - tr_w, c.top()), c.max);
    let title = Rect::from_min_max(
        pos2(b.right() + 6.0, c.top()),
        pos2(tr.left() - 6.0, c.bottom()),
    );
    let deck = &app.decks[v.index];
    let t = if deck.title.is_empty() {
        "No track loaded".to_string()
    } else {
        deck.title.clone()
    };
    if title.width() > 10.0 {
        widgets::text_in(
            &ui.painter_at(title),
            title,
            &t,
            theme::prop(12.0),
            v.pal.text,
            false,
        );
        app.map.add(ui, "title", Some(v.index), title);
    }
    transport(app, ui, v, tr, 11.0);
}

/// Vertical pitch fader: "−" (slower) at the top, "+" at the bottom, centre
/// line at 0 %. Double-click resets to 0 %.
fn pitch_fader(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, col: Rect) {
    let painter = ui.painter();
    let lab = 14.0;
    painter.text(
        pos2(col.center().x, col.top() + lab / 2.0),
        egui::Align2::CENTER_CENTER,
        "−",
        theme::prop(14.0),
        v.pal.text_weak,
    );
    painter.text(
        pos2(col.center().x, col.bottom() - lab / 2.0),
        egui::Align2::CENTER_CENTER,
        "+",
        theme::prop(14.0),
        v.pal.text_weak,
    );
    let r = Rect::from_min_max(
        pos2(col.left() + 2.0, col.top() + lab + 2.0),
        pos2(col.right() - 2.0, col.bottom() - lab - 2.0),
    );
    let t = ((1.0 - v.d.pitch) / 2.0) as f32;
    let (resp, new) = widgets::vfader(ui, r, v.uid("pitch"), t, v.color, true);
    // Soft takeover: where the controller's pitch fader is while it waits
    // to pick the pitch up (as Serato's ghost fader).
    if let Some(g) = v.d.pitch_fader_ghost {
        let gt = ((1.0 - g) / 2.0) as f32;
        let cap_h = (r.width() * 0.45).clamp(6.0, 14.0);
        let travel = (r.height() - cap_h).max(1.0);
        let y = r.bottom() - cap_h / 2.0 - gt.clamp(0.0, 1.0) * travel;
        ui.painter().rect_stroke(
            Rect::from_center_size(pos2(r.center().x, y), vec2(r.width(), cap_h)),
            2.0,
            egui::Stroke::new(1.5, v.color.gamma_multiply(0.8)),
            egui::StrokeKind::Inside,
        );
    }
    let resp = resp.on_hover_text(format!(
        "Pitch {:+.2}% (range {}); double-click: 0 %{}",
        v.d.pitch * v.d.pitch_range.fraction() * 100.0,
        v.d.pitch_range.label(),
        if v.d.pitch_fader_ghost.is_some() {
            "\nThe outline is the controller's pitch fader: move it across the \
             pitch and it takes over"
        } else {
            ""
        }
    ));
    if resp.double_clicked() {
        app.send(Command::Deck(v.id, DeckCommand::SetPitch(0.0)));
    } else if let Some(t) = new {
        let pitch = (1.0 - 2.0 * f64::from(t)).clamp(-1.0, 1.0);
        app.send(Command::Deck(v.id, DeckCommand::SetPitch(pitch)));
    }
    app.map.add(ui, "pitch_fader", Some(v.index), r);
}

/// Pitch value (drag or type), range and reset.
pub(crate) fn pitch_row(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) {
    let w = rect.width();
    let a = Rect::from_min_size(rect.min, vec2((w * 0.42).floor(), rect.height()));
    let b = Rect::from_min_size(
        pos2(a.right() + 4.0, rect.top()),
        vec2((w * 0.36).floor(), rect.height()),
    );
    let c = Rect::from_min_max(pos2(b.right() + 4.0, rect.top()), rect.max);
    let range = v.d.pitch_range.fraction() * 100.0;
    let mut pct = v.d.pitch * range;
    let r = ui
        .put(
            a,
            egui::DragValue::new(&mut pct)
                .range(-range..=range)
                .fixed_decimals(2)
                .speed(0.01)
                .suffix("%"),
        )
        .on_hover_text("Pitch: drag or type a value");
    if r.changed() {
        app.send(Command::Deck(v.id, DeckCommand::SetPitch(pct / range)));
    }
    app.map.add(ui, "pitch_value", Some(v.index), a);
    let mut child = ui.new_child(UiBuilder::new().max_rect(b).id_salt(v.uid("range_ui")));
    let combo = egui::ComboBox::from_id_salt(v.uid("range"))
        .truncate()
        .width(b.width() - 8.0)
        .selected_text(v.d.pitch_range.label())
        .show_ui(&mut child, |ui| {
            for r in PitchRange::ALL {
                if ui
                    .selectable_label(v.d.pitch_range == r, r.label())
                    .clicked()
                {
                    app.send(Command::Deck(v.id, DeckCommand::SetPitchRange(r)));
                }
            }
        });
    let combo = combo.response.on_hover_text("Pitch range");
    app.map.add(ui, "pitch_range", Some(v.index), combo.rect);
    let reset = flat_button(
        ui,
        c,
        v.uid("pitch_reset"),
        "0",
        theme::prop(12.0),
        None,
        false,
        v.pal.accent,
        false,
    )
    .on_hover_text("Reset pitch");
    if reset.clicked() {
        app.send(Command::Deck(v.id, DeckCommand::SetPitch(0.0)));
    }
    app.map.add(ui, "pitch_reset", Some(v.index), c);
}

/// Pitch bend − / + (hold) and key lock.
fn bend_row(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) {
    let bw = 24.0;
    let minus_r = Rect::from_min_size(rect.min, vec2(bw, rect.height()));
    let plus_r = Rect::from_min_size(
        pos2(minus_r.right() + 3.0, rect.top()),
        vec2(bw, rect.height()),
    );
    // KEY LOCK, SYNC and SLIP share the rest (and VINYL, the jog mode,
    // when a controller is attached).
    let x0 = plus_r.right() + 5.0;
    // VINYL only where four buttons keep readable labels (on small panels
    // the controller's SCRATCH light shows the mode: bright = vinyl).
    let with_jog =
        app.controller.is_some() && ((rect.right() - x0 - 9.0) / 4.0).floor() >= VINYL_MIN_W;
    let n = if with_jog { 4.0 } else { 3.0 };
    let third = ((rect.right() - x0 - 3.0 * (n - 1.0)) / n).floor();
    let kl_r = Rect::from_min_size(pos2(x0, rect.top()), vec2(third, rect.height()));
    let sync_r = Rect::from_min_size(
        pos2(kl_r.right() + 3.0, rect.top()),
        vec2(third, rect.height()),
    );
    let (slip_r, vinyl_r) = if with_jog {
        let s = Rect::from_min_size(
            pos2(sync_r.right() + 3.0, rect.top()),
            vec2(third, rect.height()),
        );
        (
            s,
            Some(Rect::from_min_max(
                pos2(s.right() + 3.0, rect.top()),
                rect.max,
            )),
        )
    } else {
        (
            Rect::from_min_max(pos2(sync_r.right() + 3.0, rect.top()), rect.max),
            None,
        )
    };
    bend_buttons(app, ui, v, minus_r, plus_r);
    key_button(app, ui, v, kl_r);
    sync_button(app, ui, v, sync_r);
    slip_button(app, ui, v, slip_r);
    if let Some(r) = vinyl_r {
        let on = app.decks[v.index].jog_vinyl;
        let b = flat_button(
            ui,
            r,
            v.uid("vinyl"),
            "VINYL",
            theme::prop(11.5),
            None,
            on,
            v.pal.accent,
            false,
        )
        .on_hover_text(
            "Jog mode (SCRATCH on the controller): lit = touching the jog holds and scratches the track; off = the jog nudges the tempo (CDJ style)",
        );
        if b.clicked() {
            app.decks[v.index].jog_vinyl = !on;
        }
        app.map.add(ui, "vinyl", Some(v.index), r);
    }
}

/// Pitch bend − and + (hold).
pub(crate) fn bend_buttons(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    minus_r: Rect,
    plus_r: Rect,
) {
    let bend_now = app.decks[v.index].bend_held;
    let minus = flat_button(
        ui,
        minus_r,
        v.uid("bend_down"),
        "−",
        theme::prop(15.0),
        None,
        bend_now < 0,
        v.pal.accent,
        true,
    )
    .on_hover_text("Pitch bend down (hold)");
    let plus = flat_button(
        ui,
        plus_r,
        v.uid("bend_up"),
        "+",
        theme::prop(15.0),
        None,
        bend_now > 0,
        v.pal.accent,
        true,
    )
    .on_hover_text("Pitch bend up (hold)");
    let bend = i8::from(is_held(&plus)) - i8::from(is_held(&minus));
    if bend != app.decks[v.index].bend_held {
        app.decks[v.index].bend_held = bend;
        app.send(Command::Deck(
            v.id,
            DeckCommand::PitchBend(f64::from(bend) * BEND),
        ));
    }
    app.map.add(ui, "bend_down", Some(v.index), minus_r);
    app.map.add(ui, "bend_up", Some(v.index), plus_r);
}

/// KEY (key lock) toggle.
pub(crate) fn key_button(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, r: Rect) {
    let kl = flat_button(
        ui,
        r,
        v.uid("key_lock"),
        "KEY",
        theme::prop(11.5),
        None,
        v.d.key_lock,
        v.pal.accent,
        false,
    )
    .on_hover_text("Key lock: keep the key when the tempo changes");
    if kl.clicked() {
        app.send(Command::Deck(v.id, DeckCommand::SetKeyLock(!v.d.key_lock)));
    }
    app.map.add(ui, "key_lock", Some(v.index), r);
}

/// SYNC (right-click: make this deck the sync master).
pub(crate) fn sync_button(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, r: Rect) {
    let master = app.engine.snapshot().sync_master == Some(v.index);
    let sync = flat_button(
        ui,
        r,
        v.uid("sync"),
        if master && v.d.sync { "MSTR" } else { "SYNC" },
        theme::prop(11.5),
        None,
        v.d.sync,
        v.pal.accent,
        false,
    )
    .on_hover_text(if master {
        "SYNC: this deck is the sync master. Right-click: make master"
    } else {
        "SYNC: match the tempo (and with Smart sync the beat phase) of the master. Right-click: make this deck the master"
    });
    if sync.clicked() {
        app.toggle_sync(v.index);
    }
    if sync.secondary_clicked() {
        app.make_master(v.index);
    }
    app.map.add(ui, "sync", Some(v.index), r);
}

/// SLIP toggle.
pub(crate) fn slip_button(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, r: Rect) {
    let slip = flat_button(
        ui,
        r,
        v.uid("slip"),
        "SLIP",
        theme::prop(11.5),
        None,
        v.d.slip,
        v.pal.accent,
        false,
    )
    .on_hover_text(
        "Slip: loops, hot cues and scratching are momentary; the track carries on underneath",
    );
    if slip.clicked() {
        app.toggle_slip(v.index);
    }
    app.map.add(ui, "slip", Some(v.index), r);
}

/// Path field and Load (Enter in the field loads too).
fn load_row(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) {
    let bw = 46.0;
    let browse_w = 28.0;
    let field = Rect::from_min_max(
        rect.min,
        pos2(rect.right() - bw - browse_w - 8.0, rect.bottom()),
    );
    let btn = Rect::from_min_max(pos2(rect.right() - bw, rect.top()), rect.max);
    let browse_r = Rect::from_min_max(
        pos2(rect.right() - bw - browse_w - 4.0, rect.top()),
        pos2(rect.right() - bw - 4.0, rect.bottom()),
    );
    // Browse: the system's open dialog; the file picked loads at once.
    const IDS: [&str; 4] = [
        "deck_browse_1",
        "deck_browse_2",
        "deck_browse_3",
        "deck_browse_4",
    ];
    let id = IDS[v.index.min(3)];
    let browse = flat_button(
        ui,
        browse_r,
        v.uid("browse"),
        "…",
        theme::prop(12.5),
        None,
        false,
        v.pal.accent,
        false,
    )
    .on_hover_text("Browse for an audio file to load");
    app.map.add(ui, "browse", Some(v.index), browse_r);
    if browse.clicked() {
        crate::browse::open(
            id,
            &format!("Load on deck {}", v.index + 1),
            crate::browse::Pick::AudioFile,
            &app.decks[v.index].path_input,
        );
    }
    if let Some(p) = crate::browse::take(id) {
        app.decks[v.index].path_input = p.display().to_string();
        app.load_path(v.id, p);
    }
    let input = ui.put(
        field,
        egui::TextEdit::singleline(&mut app.decks[v.index].path_input)
            .hint_text("path to audio file"),
    );
    app.map.add(ui, "path", Some(v.index), field);
    let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
    let load = flat_button(
        ui,
        btn,
        v.uid("load"),
        "Load",
        theme::prop(12.5),
        None,
        false,
        v.pal.accent,
        false,
    )
    .on_hover_text("Load the file (you can also drop files or library tracks on the deck)");
    app.map.add(ui, "load", Some(v.index), btn);
    if load.clicked() || enter {
        let p = PathBuf::from(app.decks[v.index].path_input.trim());
        if !p.as_os_str().is_empty() {
            app.load_path(v.id, p);
        }
    }
}

impl KaldorApp {
    /// Real time shown across a zoomed waveform strip, the same for every
    /// deck: `zoom_beats` beats at 120 BPM. It depends only on the zoom, so
    /// changing one deck's tempo squeezes or stretches that deck's waveform
    /// alone (each strip shows `seconds * tempo` of its track) and leaves
    /// the others as they are. It used to follow the first playing deck's
    /// tempo, which rescaled every strip when that deck's pitch moved.
    pub fn waveform_window_seconds(&self) -> f64 {
        // Within the range also for a config saved before it changed.
        f64::from(dj_core::config::clamp_zoom(self.config.ui.zoom_beats)) * 60.0 / DEFAULT_BPM
    }
}

/// The waveform strips (or nothing when the mixer is shown in their place).
pub(crate) fn waveforms(app: &mut KaldorApp, ui: &mut egui::Ui, frame: &Frame) {
    let Some(first) = frame.strips.first() else {
        return;
    };
    let area = frame.strips.iter().fold(first.rect, |a, s| a.union(s.rect));
    app.map.enter(Region::Waveforms, area);
    let seconds_visible = app.waveform_window_seconds();
    let mut ui = region_ui(ui, area, Layout::top_down(Align::Min), "waveforms");
    for s in &frame.strips {
        strip(app, &mut ui, s, seconds_visible);
    }
}

/// One deck's strip: colour tab with the number, zoomed waveform (scrub,
/// wheel zoom) and overview (click to seek).
fn strip(app: &mut KaldorApp, ui: &mut egui::Ui, slot: &Slot, seconds_visible: f64) {
    let index = slot.deck;
    let Some(id) = DeckId::from_index(index) else {
        return;
    };
    let rect = slot.rect;
    drop_target(app, ui, rect, index, "strip");
    let d = app.engine.snapshot().decks[index];
    let sr = f64::from(app.engine.snapshot().sample_rate.max(1));
    let pos = app.display_position(index, &d);
    let pal = Palette::get(app.config.ui.dark_theme);
    let color = deck_color(id);

    let tab = Rect::from_min_size(rect.min, vec2(20.0, rect.height()));
    ui.painter().rect_filled(tab, 3.0, color);
    ui.painter().text(
        tab.center(),
        egui::Align2::CENTER_CENTER,
        id.number().to_string(),
        theme::display(ui.ctx(), 11.0),
        Palette::on(color),
    );
    let body = Rect::from_min_max(pos2(tab.right() + 2.0, rect.top()), rect.max);
    // Extended: the overview is in the deck panel, the strip is all
    // zoomed waveform, with bar numbers.
    let extended = app.config.ui.layout == DeckLayout::Extended;
    let ov_h = if extended {
        0.0
    } else {
        (rect.height() * 0.22).clamp(10.0, 24.0).floor()
    };
    let zoom_r = if extended {
        body
    } else {
        Rect::from_min_max(body.min, pos2(body.right(), body.bottom() - ov_h - 2.0))
    };
    let ov_r = Rect::from_min_max(pos2(body.left(), body.bottom() - ov_h), body.max);

    // The scale follows the pitch fader (or sync), not the bend: a bend only
    // makes the waveform scroll faster or slower.
    let frames_visible = (seconds_visible * sr * d.pitch_tempo.max(0.05)).max(1.0);
    // The fine detail waveform once it is built (a moment after the load).
    let wf = app.decks[index]
        .detail
        .clone()
        .or_else(|| app.decks[index].waveform.clone());
    let markers = deck_markers(app, index, &d);
    let resp = ui.interact(zoom_r, Id::unique(("wave", index)), Sense::click_and_drag());
    paint_zoomed(
        ui.painter(),
        zoom_r,
        &ZoomedParams {
            waveform: wf.as_deref(),
            position: pos,
            frames_visible,
            length: d.length,
            cue_point: d.loaded.then_some(d.cue_point),
            colors: app.config.ui.waveform_colors,
            background: pal.well,
            playhead: pal.playhead,
            grid: app.decks[index].grid,
            device_rate: sr,
            markers,
            bar_numbers: extended && app.config.ui.bar_numbers,
            grid_lines: app.config.ui.grid_lines,
            playhead_at: app.config.ui.playhead_at,
        },
    );
    phase_meter(app, ui, index, &d, zoom_r);
    app.map.add(ui, "wave", Some(index), zoom_r);
    if d.loaded {
        let fpp = frames_visible / f64::from(zoom_r.width().max(1.0));
        // Ctrl+drag moves the grid (slip), Ctrl+Shift+drag stretches it
        // (adjust) around the play position; the beat under the mouse follows it.
        let mods = ui.input(|i| i.modifiers);
        if resp.hovered() && mods.command && app.decks[index].grid.is_some_and(|g| !g.locked) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if resp.drag_started() && mods.command {
            let at = ui.input(|i| i.pointer.press_origin()).map_or(pos, |o| {
                let head =
                    zoom_r.left() + zoom_r.width() * app.config.ui.playhead_at.clamp(0.1, 0.9);
                pos + f64::from(o.x - head) * fpp
            }) / sr;
            app.grid_drag_begin(index, at, pos / sr, mods.shift);
        } else if resp.drag_started() {
            app.decks[index].scrub_anchor = Some(pos);
        }
        if app.decks[index].grid_drag.is_some() {
            if resp.dragged() {
                let dx = resp
                    .interact_pointer_pos()
                    .zip(ui.input(|i| i.pointer.press_origin()))
                    .map_or(0.0, |(p, o)| p.x - o.x);
                app.grid_drag_to(index, f64::from(dx) * fpp / sr);
            }
            if resp.drag_stopped() {
                app.grid_drag_end(index);
            }
        } else if resp.dragged()
            && let Some(anchor) = app.decks[index].scrub_anchor
        {
            let dx = f64::from(
                resp.interact_pointer_pos()
                    .zip(ui.input(|i| i.pointer.press_origin()))
                    .map_or(0.0, |(p, o)| p.x - o.x),
            );
            app.send(Command::Deck(
                id,
                DeckCommand::Scrub {
                    active: true,
                    target: anchor - dx * fpp,
                },
            ));
        }
        if resp.drag_stopped() && app.decks[index].scrub_anchor.is_some() {
            app.decks[index].scrub_anchor = None;
            app.send(Command::Deck(
                id,
                DeckCommand::Scrub {
                    active: false,
                    target: 0.0,
                },
            ));
        }
        if resp.hovered() {
            // The zoom follows how far the wheel turned (egui spreads one
            // notch, 40 points, over several frames): a third of a doubling
            // per notch, so 32 to 1 beat takes 15 notches. Up zooms in.
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let z = app.config.ui.zoom_beats * 2f32.powf(-scroll / ZOOM_SCROLL_PER_DOUBLING);
                app.config.ui.zoom_beats = dj_core::config::clamp_zoom(z);
            }
        }
    }

    if !extended {
        overview(app, ui, index, ov_r);
    }
}

/// The whole-track overview of a deck in `rect`: playhead, cues, played
/// part dimmed, load progress; click or drag to seek.
pub(crate) fn overview(app: &mut KaldorApp, ui: &mut egui::Ui, index: usize, rect: Rect) {
    let Some(id) = DeckId::from_index(index) else {
        return;
    };
    let d = app.engine.snapshot().decks[index];
    let sr = f64::from(app.engine.snapshot().sample_rate.max(1));
    let pos = app.display_position(index, &d);
    let pal = Palette::get(app.config.ui.dark_theme);
    let wf = app.decks[index].waveform.clone();
    let markers = deck_markers(app, index, &d);
    let oresp = ui.interact(
        rect,
        Id::unique(("overview", index)),
        Sense::click_and_drag(),
    );
    paint_overview(
        ui.painter(),
        rect,
        wf.as_deref(),
        d.length,
        d.ready_frames as f64,
        pos,
        d.loaded.then_some(d.cue_point),
        app.config.ui.waveform_colors,
        pal.well,
        pal.playhead,
        sr,
        &markers,
        app.config.ui.layout == DeckLayout::Extended,
    );
    app.map.add(ui, "overview", Some(index), rect);
    if d.loaded
        && (oresp.clicked() || oresp.dragged())
        && let Some(p) = oresp.interact_pointer_pos()
    {
        let frac = f64::from(((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
        app.send(Command::Deck(id, DeckCommand::Seek(frac * d.length)));
    }
}

/// Hot cues (in their pad colours), loop and slip position for the strips.
fn deck_markers(app: &KaldorApp, index: usize, d: &DeckSnapshot) -> crate::waveform_view::Markers {
    let perf = &app.decks[index].perf;
    let mut m = crate::waveform_view::Markers::default();
    if !d.loaded {
        return m;
    }
    for (k, c) in d.hot_cues.iter().enumerate() {
        if let Some(f) = c {
            let rgb = perf.cue_meta[k].color;
            m.hot_cues[k] = Some((*f, crate::theme::rgb(rgb)));
        }
    }
    m.loop_region = d.loop_info.map(|l| (l.start, l.end, l.active));
    m.loop_in = d.loop_in;
    m.slip = d.slip_position;
    m
}

/// Beat phase against the sync master: a small bar in the top left corner of
/// the zoomed waveform. The marker sits in the middle when the beats line up
/// and moves left (behind) or right (ahead) by up to half a beat.
fn phase_meter(app: &mut KaldorApp, ui: &mut egui::Ui, index: usize, d: &DeckSnapshot, rect: Rect) {
    let snap = app.engine.snapshot();
    let Some(m) = snap.sync_master.filter(|m| *m != index) else {
        return;
    };
    let master = snap.decks[m];
    let (Some(g), Some(mg)) = (d.grid, master.grid) else {
        return;
    };
    if !(d.loaded && master.loaded) || rect.width() < 120.0 || rect.height() < 20.0 {
        return;
    }
    let a = g.beat_at(d.position);
    let b = mg.beat_at(master.position);
    // Compare in this deck's beats (half or double time like sync does).
    let own = g.bpm(snap.sample_rate) * d.tempo;
    let theirs = mg.bpm(snap.sample_rate) * master.tempo;
    let k = if own > 0.0 && theirs > 0.0 {
        (own / theirs).log2().round()
    } else {
        0.0
    };
    let err = ((a - b * 2f64.powf(k) + 0.5).rem_euclid(1.0) - 0.5) as f32;
    let pal = Palette::get(app.config.ui.dark_theme);
    let r = Rect::from_min_size(rect.min + vec2(6.0, 5.0), vec2(64.0, 6.0));
    ui.painter()
        .rect_filled(r, 2.0, pal.panel.gamma_multiply(0.85));
    ui.painter().line_segment(
        [
            pos2(r.center().x, r.top() - 1.0),
            pos2(r.center().x, r.bottom() + 1.0),
        ],
        Stroke::new(1.0, pal.text_weak),
    );
    let x = r.center().x + err * r.width();
    let color = if err.abs() < 0.02 { pal.play } else { pal.warn };
    ui.painter().rect_filled(
        Rect::from_center_size(pos2(x, r.center().y), vec2(4.0, 8.0)),
        1.0,
        color,
    );
    let resp = ui
        .interact(r, Id::unique(("phase_meter", index)), Sense::hover())
        .on_hover_text(format!(
            "Beat phase against the master (deck {}): {:+.2} beats",
            m + 1,
            err
        ));
    app.map.add(ui, "phase_meter", Some(index), resp.rect);
}

/// Tempo for the BPM display: a synced deck shows the master's tempo, not
/// the small phase corrections on top of it, and a jog nudge is not shown
/// (beatmatching on the jog made the BPM go up and down).
pub(crate) fn shown_tempo(d: &DeckSnapshot) -> f64 {
    match d.sync_rate {
        Some(r) if d.sync && d.bend == 0.0 && !d.scrubbing => r,
        _ => d.set_tempo,
    }
}
