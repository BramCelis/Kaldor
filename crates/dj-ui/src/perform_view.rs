//! The PADS panel under the waveforms (in place of the mixer): for each
//! LAYER deck its pad modes and 8 pads, loop controls, beat jump, slip, key
//! shift, grid editing and the sync master.

use crate::app::{KaldorApp, region_ui};
use crate::perform::{
    AUTO_LOOP_PADS, GRID_ADJUST, GRID_ADJUST_LARGE, GRID_NUDGE, GRID_NUDGE_LARGE, GridEdit,
    PITCH_PADS, PadMode, ROLL_PADS, SLICER_DOMAINS, size_label,
};
use crate::sampler_view::{BANKS, slot_label};
use crate::theme::{self, Palette, deck_color, rgb};
use crate::uimap::Region;
use crate::widgets::{self, flat_button, is_held};
use dj_audio::{DeckSnapshot, EngineSnapshot, HOT_CUES, SAMPLER_BANK};
use dj_core::DeckId;
use egui::{Align, Color32, Id, Layout, Rect, Sense, Stroke, pos2, vec2};

/// Width and height the panel needs.
pub const PADS_MIN: egui::Vec2 = egui::vec2(804.0, 218.0);

const ROW_H: f32 = 22.0;
const GAP: f32 = 4.0;
const LABEL_W: f32 = 38.0;

/// The panel in `rect` (scrolls when the area is smaller than it needs).
pub(crate) fn pads_area(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    app.map.enter(Region::Mixer, rect);
    let pal = Palette::get(app.config.ui.dark_theme);
    let fits = rect.width() >= PADS_MIN.x && rect.height() >= PADS_MIN.y;
    let panel = if fits {
        Rect::from_min_size(rect.min, vec2(rect.width(), PADS_MIN.y))
    } else {
        rect
    };
    ui.painter().rect(
        panel,
        6.0,
        pal.panel,
        Stroke::new(1.0, pal.border),
        egui::StrokeKind::Inside,
    );
    let mut ui = region_ui(ui, rect, Layout::top_down(Align::Min), "pads");
    if fits {
        pads(app, &mut ui, panel);
    } else {
        egui::ScrollArea::both()
            .id_salt("pads_scroll")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                app.map.scrolled = true;
                let (r, _) = ui.allocate_exact_size(PADS_MIN, Sense::hover());
                pads(app, ui, r);
                app.map.scrolled = false;
            });
    }
}

fn pads(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    let inner = rect.shrink(6.0);
    let half_w = ((inner.width() - 12.0) / 2.0).floor();
    let snap = *app.engine.snapshot();
    for side in 0..2 {
        let deck = app.layer()[side];
        let x = if side == 0 {
            inner.left()
        } else {
            inner.right() - half_w
        };
        let half = Rect::from_min_size(pos2(x, inner.top()), vec2(half_w, inner.height()));
        deck_half(app, ui, deck, side, half, &snap);
    }
}

/// A row of fixed-width cells, left to right.
struct Row {
    rect: Rect,
    x: f32,
}

impl Row {
    fn new(rect: Rect) -> Self {
        Self {
            rect,
            x: rect.left(),
        }
    }

    fn cell(&mut self, w: f32) -> Rect {
        let w = w.min((self.rect.right() - self.x).max(0.0));
        let r = Rect::from_min_size(pos2(self.x, self.rect.top()), vec2(w, self.rect.height()));
        self.x += w + 3.0;
        r
    }

    fn space(&mut self, w: f32) {
        self.x += w;
    }

    fn rest(&self) -> Rect {
        Rect::from_min_max(pos2(self.x, self.rect.top()), self.rect.max)
    }
}

/// Context for drawing one deck's controls.
struct Half {
    deck: usize,
    tag: Option<usize>,
    color: Color32,
    pal: Palette,
    d: DeckSnapshot,
}

impl Half {
    fn id(&self, name: &str) -> Id {
        Id::unique(("pads", name, self.deck))
    }
}

#[allow(clippy::too_many_arguments)]
fn button(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    h: &Half,
    r: Rect,
    name: &'static str,
    label: &str,
    on: bool,
    hover: &str,
) -> bool {
    if r.width() < 4.0 {
        return false;
    }
    // Arrows are drawn (the bundled font has no arrow glyphs).
    let (label, icon) = match label {
        "◀" => ("", Some(widgets::Icon::ChevronLeft)),
        "▶" => ("", Some(widgets::Icon::ChevronRight)),
        l => (l, None),
    };
    let resp = flat_button(
        ui,
        r,
        h.id(name),
        label,
        theme::prop(11.0),
        icon,
        on,
        h.pal.accent,
        false,
    )
    .on_hover_text(hover);
    app.map.add(ui, name, h.tag, r);
    resp.clicked()
}

/// A button that fires when pressed and, while held, again and again (see
/// [`widgets::held_repeat`]; the grid SLIP and ADJ arrows). True in the
/// frames where it fires.
fn repeat_button(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    h: &Half,
    r: Rect,
    name: &'static str,
    label: &str,
    hover: &str,
) -> bool {
    if r.width() < 4.0 {
        return false;
    }
    let (label, icon) = match label {
        "◀" => ("", Some(widgets::Icon::ChevronLeft)),
        "▶" => ("", Some(widgets::Icon::ChevronRight)),
        l => (l, None),
    };
    let id = h.id(name);
    let resp = flat_button(
        ui,
        r,
        id,
        label,
        theme::prop(11.0),
        icon,
        false,
        h.pal.accent,
        true,
    )
    .on_hover_text(hover);
    app.map.add(ui, name, h.tag, r);
    widgets::held_repeat(ui, &resp, id)
}

fn row_label(ui: &egui::Ui, h: &Half, r: Rect, text: &str) {
    widgets::text_in(
        &ui.painter_at(r),
        r,
        text,
        theme::prop(10.5),
        h.pal.text_weak,
        false,
    );
}

fn deck_half(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    deck: usize,
    side: usize,
    rect: Rect,
    snap: &EngineSnapshot,
) {
    let id = DeckId::ALL[deck];
    let h = Half {
        deck,
        tag: Some(deck),
        color: deck_color(id),
        pal: Palette::get(app.config.ui.dark_theme),
        d: snap.decks[deck],
    };
    // Any press in this half makes it the keyboard deck.
    if ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.any_pressed()) {
        app.set_keyboard_deck(deck);
    }
    if app.keyboard_deck() == deck {
        ui.painter().rect_stroke(
            rect.expand(3.0),
            5.0,
            Stroke::new(1.0, h.color.gamma_multiply(0.6)),
            egui::StrokeKind::Inside,
        );
    }
    let mut y = rect.top();
    let mut take = |hgt: f32| {
        let r = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), hgt));
        y += hgt + GAP;
        r
    };
    let header = take(24.0);
    let fixed = 24.0 + ROW_H * 4.0 + GAP * 6.0;
    let pad_h = ((rect.height() - fixed) / 2.0).floor().max(16.0);
    let pads_r = take(pad_h * 2.0 + GAP);
    let options = take(ROW_H);
    let loop_r = take(ROW_H);
    let jump_r = take(ROW_H);
    let grid_r = take(ROW_H);

    header_row(app, ui, &h, side, header);
    pad_grid(app, ui, &h, pads_r, pad_h, snap);
    options_row(app, ui, &h, options);
    loop_row(app, ui, &h, loop_r);
    jump_row(app, ui, &h, jump_r);
    grid_row(app, ui, &h, grid_r, snap);
}

/// LAYER deck buttons and the pad mode tabs.
fn header_row(app: &mut KaldorApp, ui: &mut egui::Ui, h: &Half, side: usize, r: Rect) {
    let mut row = Row::new(r);
    for deck in [side, side + 2] {
        let did = DeckId::ALL[deck];
        let c = row.cell(26.0);
        let resp = flat_button(
            ui,
            c,
            Id::unique(("pads_layer", deck)),
            &did.number().to_string(),
            theme::prop(12.0),
            None,
            app.layer()[side] == deck,
            deck_color(did),
            false,
        )
        .on_hover_text(format!("Show deck {} here (LAYER)", did.number()));
        if resp.clicked() {
            app.set_layer(side, deck);
            app.set_keyboard_deck(deck);
        }
        app.map.add(ui, "pads_layer", Some(deck), c);
    }
    row.space(4.0);
    let rest = row.rest();
    let tab_w = ((rest.width() - 3.0 * 7.0) / 8.0).floor();
    let current = app.pad_mode(h.deck);
    for m in PadMode::ALL {
        let c = row.cell(tab_w);
        let resp = flat_button(
            ui,
            c,
            Id::unique(("pad_mode", h.deck, m.short())),
            m.short(),
            theme::prop(10.0),
            None,
            current == m,
            h.color,
            false,
        )
        .on_hover_text(m.hint());
        if resp.clicked() {
            app.set_pad_mode(h.deck, m);
        }
        app.map.add(ui, "pad_mode", h.tag, c);
    }
}

/// What one pad shows: label, fill colour, lit, usable.
fn pad_look(
    app: &KaldorApp,
    h: &Half,
    k: usize,
    snap: &EngineSnapshot,
) -> (String, Color32, bool, bool) {
    let perf = &app.decks[h.deck].perf;
    let sr = f64::from(snap.sample_rate.max(1));
    let d = &h.d;
    match perf.pad_mode {
        PadMode::Cue => {
            let set = d.hot_cues[k].is_some();
            let name = &perf.cue_meta[k].name;
            let label = if name.is_empty() {
                format!("{}", k + 1)
            } else {
                format!("{} {name}", k + 1)
            };
            (label, rgb(perf.cue_meta[k].color), set, d.loaded)
        }
        PadMode::SavedLoop => match &perf.saved_loops[k] {
            Some(l) => {
                let active = d.loop_info.filter(|x| x.active).is_some_and(|a| {
                    (a.start / sr - l.start).abs() < 1e-3 && (a.end / sr - l.end).abs() < 1e-3
                });
                let beats = d
                    .grid
                    .map(|g| size_label(((l.end - l.start) * sr / g.beat_len) as f32));
                let mut label = if l.meta.name.is_empty() {
                    format!("L{}", k + 1)
                } else {
                    l.meta.name.clone()
                };
                if let Some(b) = beats {
                    label = format!("{label} · {b}");
                }
                if active {
                    label = format!("» {label}");
                }
                (label, rgb(l.meta.color), true, d.loaded)
            }
            None => (format!("L{}", k + 1), h.color, false, d.loaded),
        },
        PadMode::AutoLoop => {
            let b = AUTO_LOOP_PADS[k];
            let on = d
                .loop_info
                .is_some_and(|l| l.active && (l.beats - b).abs() < 1e-4);
            (size_label(b), h.color, on, d.loaded && d.grid.is_some())
        }
        PadMode::BeatJump => {
            let b = crate::perform::BEAT_JUMP_PADS[k];
            let label = if b < 0.0 {
                format!("◀ {}", size_label(-b as f32))
            } else {
                format!("{} ▶", size_label(b as f32))
            };
            (label, h.color, perf.held[k], d.loaded && d.grid.is_some())
        }
        PadMode::Roll => (
            size_label(ROLL_PADS[k]),
            h.color,
            perf.held[k],
            d.loaded && d.grid.is_some(),
        ),
        PadMode::Sampler => {
            let slot = app.sampler_bank() * SAMPLER_BANK + k;
            let s = snap.sampler.slots[slot];
            let name = app.sample_name(slot);
            let label = if name.is_empty() {
                slot_label(slot)
            } else {
                format!("{} {name}", slot_label(slot))
            };
            (
                label,
                Color32::from_rgb(0x4A, 0x7C, 0xF0),
                s.playing,
                s.loaded,
            )
        }
        PadMode::Slicer => {
            let lit = match (d.slicer_held, d.grid) {
                (Some(s), _) => usize::from(s) == k,
                (None, Some(g)) if d.loaded => {
                    let domain = f64::from(perf.slicer_domain);
                    let b = g.bar_beat(d.slip_position.unwrap_or(d.position));
                    let into = b - (b / domain).floor() * domain;
                    ((into / (domain / 8.0)).floor() as usize).min(7) == k
                }
                _ => false,
            };
            (
                format!("{}", k + 1),
                h.color,
                lit,
                d.loaded && d.grid.is_some(),
            )
        }
        PadMode::PitchPlay => {
            let st = PITCH_PADS[k];
            let label = if st > 0 {
                format!("+{st}")
            } else {
                format!("{st}")
            };
            (label, h.color, d.key_shift == st && d.key_lock, d.loaded)
        }
        PadMode::Stems => ("—".into(), h.pal.border, false, false),
    }
}

fn pad_grid(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    h: &Half,
    rect: Rect,
    pad_h: f32,
    snap: &EngineSnapshot,
) {
    let w = ((rect.width() - 3.0 * GAP) / 4.0).floor();
    for k in 0..HOT_CUES {
        let r = Rect::from_min_size(
            pos2(
                rect.left() + (k % 4) as f32 * (w + GAP),
                rect.top() + (k / 4) as f32 * (pad_h + GAP),
            ),
            vec2(w, pad_h),
        );
        let (label, color, lit, usable) = pad_look(app, h, k, snap);
        let mode = app.pad_mode(h.deck);
        let resp = flat_button(
            ui,
            r,
            Id::unique(("pad", h.deck, k)),
            &label,
            theme::prop(12.0),
            None,
            lit,
            color,
            true,
        );
        if !lit {
            // Unlit pads keep a thin bar in their colour.
            ui.painter().rect_filled(
                Rect::from_min_size(r.min + vec2(3.0, 3.0), vec2(r.width() - 6.0, 3.0)),
                1.0,
                if usable {
                    color.gamma_multiply(0.7)
                } else {
                    h.pal.border
                },
            );
        }
        let resp = resp.on_hover_text(mode.hint());
        app.map.add(ui, "pad", h.tag, r);
        if mode == PadMode::Sampler {
            // A library track or a file dropped on the pad loads into its
            // sampler slot.
            let slot = app.sampler_bank() * SAMPLER_BANK + k;
            app.pad_sample_drops.push((slot, r));
            if let Some(p) = resp.dnd_release_payload::<crate::library_view::TrackDrag>() {
                app.load_library_sample(slot, p.0);
            }
        }
        if !usable && mode != PadMode::Cue {
            continue;
        }
        let held = is_held(&resp);
        if held != app.decks[h.deck].perf.held[k] {
            app.set_keyboard_deck(h.deck);
            app.pad(h.deck, k, held);
        }
        match mode {
            PadMode::Cue | PadMode::SavedLoop => marker_menu(app, &resp, h.deck, k, mode),
            _ => {
                if resp.secondary_clicked() {
                    app.pad_secondary(h.deck, k);
                }
            }
        }
    }
}

/// Right-click menu of a hot cue or saved loop pad: name, colour, delete.
pub(crate) fn marker_menu(
    app: &mut KaldorApp,
    resp: &egui::Response,
    deck: usize,
    k: usize,
    mode: PadMode,
) {
    let exists = match mode {
        PadMode::Cue => app.decks[deck].perf.hot_cues[k].is_some(),
        _ => app.decks[deck].perf.saved_loops[k].is_some(),
    };
    if !exists {
        return;
    }
    // Egui's default menu closes on any click, also inside it, so clicking
    // the name field closed the menu. This one closes on a click
    // outside, on Escape, on Enter in the name field or on Delete.
    let just_opened = resp.secondary_clicked();
    egui::Popup::context_menu(resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            let perf = &mut app.decks[deck].perf;
            let meta = match mode {
                PadMode::Cue => Some(&mut perf.cue_meta[k]),
                _ => perf.saved_loops[k].as_mut().map(|l| &mut l.meta),
            };
            let mut changed = false;
            let mut delete = false;
            if let Some(meta) = meta {
                ui.label("Name");
                let name = ui.add(
                    egui::TextEdit::singleline(&mut meta.name)
                        .desired_width(140.0)
                        .id(Id::unique(("marker_name", deck, k))),
                );
                if just_opened {
                    name.request_focus();
                }
                app.map.add(ui, "marker_name", Some(deck), name.rect);
                changed |= name.changed();
                if name.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    ui.close();
                }
                ui.horizontal(|ui| {
                    for c in crate::perform::PAD_COLORS {
                        let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::click());
                        ui.painter().rect_filled(r, 3.0, rgb(c));
                        if meta.color == c {
                            ui.painter().rect_stroke(
                                r,
                                3.0,
                                Stroke::new(2.0, Color32::WHITE),
                                egui::StrokeKind::Outside,
                            );
                        }
                        if resp.clicked() {
                            meta.color = c;
                            changed = true;
                        }
                    }
                });
                delete = ui.button("Delete").clicked();
            }
            if changed {
                perf.dirty = true;
            }
            if delete {
                ui.close();
                match mode {
                    PadMode::Cue => app.delete_hot_cue(deck, k),
                    _ => app.delete_saved_loop(deck, k),
                }
            }
        });
}

/// Mode-specific options under the pads.
fn options_row(app: &mut KaldorApp, ui: &mut egui::Ui, h: &Half, r: Rect) {
    let mut row = Row::new(r);
    let mode = app.pad_mode(h.deck);
    match mode {
        PadMode::Cue => {
            let on = app.decks[h.deck].perf.cue_loop;
            let c = row.cell(76.0);
            if button(
                app,
                ui,
                h,
                c,
                "cue_loop",
                "CUE LOOP",
                on,
                "Cue loops: a hot cue also starts a loop of the auto loop size",
            ) {
                app.decks[h.deck].perf.cue_loop = !on;
            }
        }
        PadMode::Sampler => {
            for (b, name) in BANKS.iter().enumerate() {
                let c = row.cell(26.0);
                if button(
                    app,
                    ui,
                    h,
                    c,
                    "pads_bank",
                    name,
                    app.sampler_bank() == b,
                    &format!("Sampler bank {name}"),
                ) {
                    app.set_sampler_bank(b);
                }
            }
        }
        PadMode::Slicer => {
            row_label(ui, h, row.cell(44.0), "Domain");
            for d in SLICER_DOMAINS {
                let c = row.cell(28.0);
                let on = (app.decks[h.deck].perf.slicer_domain - d).abs() < 1e-3;
                if button(
                    app,
                    ui,
                    h,
                    c,
                    "slicer_domain",
                    &size_label(d),
                    on,
                    "Beats cut into the 8 slices",
                ) {
                    app.decks[h.deck].perf.slicer_domain = d;
                }
            }
        }
        PadMode::PitchPlay => {
            row_label(ui, h, row.cell(30.0), "Cue");
            for k in 0..HOT_CUES {
                let c = row.cell(20.0);
                let on = app.decks[h.deck].perf.pitch_cue == k;
                if button(
                    app,
                    ui,
                    h,
                    c,
                    "pitch_cue",
                    &format!("{}", k + 1),
                    on,
                    "Hot cue the pitch pads play from",
                ) {
                    app.decks[h.deck].perf.pitch_cue = k;
                }
            }
        }
        _ => {}
    }
    let rest = row.rest();
    if rest.width() > 20.0 {
        let hint = match mode {
            PadMode::Cue => "right-click a pad: name, colour, delete",
            PadMode::SavedLoop => "empty pad: save the current loop",
            PadMode::AutoLoop => "press again to leave the loop",
            PadMode::Roll => "the track carries on underneath",
            PadMode::Sampler => "right-click a pad: stop",
            PadMode::Slicer => "hold a pad to play its slice",
            PadMode::PitchPlay => "pads play the cue shifted in semitones",
            PadMode::BeatJump => "◀ back, ▶ forward by the beats shown",
            PadMode::Stems => "stems need stem separation (not available yet)",
        };
        row_label(ui, h, rest.shrink2(vec2(4.0, 0.0)), hint);
    }
}

/// LOOP: in, out, reloop/exit, halve, auto loop of the size, double, move.
fn loop_row(app: &mut KaldorApp, ui: &mut egui::Ui, h: &Half, r: Rect) {
    let mut row = Row::new(r);
    row_label(ui, h, row.cell(LABEL_W), "LOOP");
    let d = h.d;
    let active = d.loop_info.is_some_and(|l| l.active);
    let size = app.decks[h.deck].perf.auto_loop_beats;
    let c = row.cell(32.0);
    if button(
        app,
        ui,
        h,
        c,
        "loop_in",
        "IN",
        d.loop_in.is_some(),
        "Loop in point",
    ) {
        app.deck_command(h.deck, dj_audio::DeckCommand::LoopIn);
    }
    let c = row.cell(36.0);
    if button(
        app,
        ui,
        h,
        c,
        "loop_out",
        "OUT",
        false,
        "Loop out point: starts the loop",
    ) {
        app.deck_command(h.deck, dj_audio::DeckCommand::LoopOut);
    }
    let c = row.cell(52.0);
    if button(
        app,
        ui,
        h,
        c,
        "reloop",
        if active { "EXIT" } else { "RELOOP" },
        active,
        "Leave the loop, or start the last loop again",
    ) {
        app.deck_command(h.deck, dj_audio::DeckCommand::Reloop);
    }
    row.space(4.0);
    let c = row.cell(24.0);
    if button(
        app,
        ui,
        h,
        c,
        "loop_halve",
        "½",
        false,
        "Halve the loop size",
    ) {
        app.scale_loop(h.deck, 0.5);
    }
    let c = row.cell(46.0);
    let auto_on = d
        .loop_info
        .is_some_and(|l| l.active && (l.beats - size).abs() < 1e-4);
    if button(
        app,
        ui,
        h,
        c,
        "auto_loop",
        &size_label(size),
        auto_on,
        "Auto loop of this many beats (again: leave)",
    ) {
        app.auto_loop(h.deck);
    }
    let c = row.cell(24.0);
    if button(
        app,
        ui,
        h,
        c,
        "loop_double",
        "×2",
        false,
        "Double the loop size",
    ) {
        app.scale_loop(h.deck, 2.0);
    }
    row.space(4.0);
    let c = row.cell(24.0);
    if button(
        app,
        ui,
        h,
        c,
        "loop_back",
        "◀",
        false,
        "Move the loop back by its length",
    ) {
        app.deck_command(h.deck, dj_audio::DeckCommand::LoopMove(-1));
    }
    let c = row.cell(24.0);
    if button(
        app,
        ui,
        h,
        c,
        "loop_fwd",
        "▶",
        false,
        "Move the loop forward by its length",
    ) {
        app.deck_command(h.deck, dj_audio::DeckCommand::LoopMove(1));
    }
}

/// JUMP: back, size, forward; SLIP; KEY shift.
fn jump_row(app: &mut KaldorApp, ui: &mut egui::Ui, h: &Half, r: Rect) {
    let mut row = Row::new(r);
    row_label(ui, h, row.cell(LABEL_W), "JUMP");
    let size = app.decks[h.deck].perf.jump_beats;
    let c = row.cell(24.0);
    if button(app, ui, h, c, "jump_back", "◀", false, "Beat jump back") {
        app.beat_jump(h.deck, -1.0);
    }
    let c = row.cell(18.0);
    if button(app, ui, h, c, "jump_smaller", "−", false, "Smaller jump") {
        app.scale_jump(h.deck, 0.5);
    }
    let sr = row.cell(34.0);
    widgets::text_in(
        &ui.painter_at(sr),
        sr,
        &size_label(size),
        theme::mono(12.0),
        h.pal.text,
        false,
    );
    ui.interact(sr, h.id("jump_size"), Sense::hover())
        .on_hover_text("Beat jump size (beats)");
    app.map.add(ui, "jump_size", h.tag, sr);
    let c = row.cell(18.0);
    if button(app, ui, h, c, "jump_larger", "+", false, "Larger jump") {
        app.scale_jump(h.deck, 2.0);
    }
    let c = row.cell(24.0);
    if button(app, ui, h, c, "jump_fwd", "▶", false, "Beat jump forward") {
        app.beat_jump(h.deck, 1.0);
    }
    row.space(8.0);
    let c = row.cell(40.0);
    if button(
        app,
        ui,
        h,
        c,
        "pads_slip",
        "SLIP",
        h.d.slip,
        "Slip: loops, hot cues and scratching are momentary; the track carries on underneath",
    ) {
        app.toggle_slip(h.deck);
    }
    row.space(8.0);
    row_label(ui, h, row.cell(28.0), "KEY");
    let c = row.cell(18.0);
    if button(
        app,
        ui,
        h,
        c,
        "key_down",
        "−",
        false,
        "Key shift down a semitone",
    ) {
        app.key_shift_by(h.deck, -1);
    }
    let ks = h.d.key_shift;
    let c = row.cell(32.0);
    let label = if ks > 0 {
        format!("+{ks}")
    } else {
        format!("{ks}")
    };
    if button(
        app,
        ui,
        h,
        c,
        "key_reset",
        &label,
        ks != 0,
        "Key shift in semitones (key lock on); click: back to 0",
    ) {
        app.key_shift_by(h.deck, 0);
    }
    let c = row.cell(18.0);
    if button(
        app,
        ui,
        h,
        c,
        "key_up",
        "+",
        false,
        "Key shift up a semitone",
    ) {
        app.key_shift_by(h.deck, 1);
    }
}

/// The grid editor: SET 1 (beat one), SLIP (move the grid), ADJ (stretch
/// it from the last beat one), halve, double, tap, lock; sync MASTER.
/// SLIP and ADJ take small steps, large ones with Shift held. Ctrl+drag on
/// the waveform slips the grid, Ctrl+Shift+drag adjusts it.
fn grid_row(app: &mut KaldorApp, ui: &mut egui::Ui, h: &Half, r: Rect, snap: &EngineSnapshot) {
    let mut row = Row::new(r);
    let locked = app.decks[h.deck].grid.is_some_and(|g| g.locked);
    let shift = ui.input(|i| i.modifiers.shift);
    let (slip, adjust) = if shift {
        (GRID_NUDGE_LARGE, GRID_ADJUST_LARGE)
    } else {
        (GRID_NUDGE, GRID_ADJUST)
    };
    let c = row.cell(36.0);
    if button(
        app,
        ui,
        h,
        c,
        "grid_set1",
        "SET 1",
        false,
        if locked {
            "The grid is locked"
        } else {
            "Beat one (bar start) at the play position"
        },
    ) && !locked
    {
        app.grid_edit(h.deck, GridEdit::SetBeatOne);
    }
    let pairs: [(&str, &str, &str, f32, GridEdit, GridEdit, String); 2] = [
        (
            "SLIP",
            "grid_earlier",
            "grid_later",
            26.0,
            GridEdit::Nudge(-slip),
            GridEdit::Nudge(slip),
            format!(
                "Slip: move the whole grid earlier / later, {:.0} ms a step ({:.0} ms with Shift). \
                 Hold to repeat. Ctrl+drag on the waveform does the same",
                GRID_NUDGE * 1e3,
                GRID_NUDGE_LARGE * 1e3
            ),
        ),
        (
            "ADJ",
            "grid_adjust_down",
            "grid_adjust_up",
            24.0,
            GridEdit::Adjust(-adjust),
            GridEdit::Adjust(adjust),
            format!(
                "Adjust: squeeze / stretch the grid around the play position. The beat at the \
                 play position stays, beats further away move more ({:.1} ms per beat a step, \
                 {:.0} ms with Shift). Hold to repeat. Ctrl+Shift+drag on the waveform does the same",
                GRID_ADJUST * 1e3,
                GRID_ADJUST_LARGE * 1e3
            ),
        ),
    ];
    for (label, down, up, label_w, e_down, e_up, hover) in pairs {
        let hover = if locked {
            "The grid is locked".to_string()
        } else {
            hover
        };
        let c = row.cell(18.0);
        if repeat_button(app, ui, h, c, down, "◀", &hover) && !locked {
            app.grid_edit(h.deck, e_down);
        }
        row_label(ui, h, row.cell(label_w), label);
        let c = row.cell(18.0);
        if repeat_button(app, ui, h, c, up, "▶", &hover) && !locked {
            app.grid_edit(h.deck, e_up);
        }
    }
    let edits: [(&'static str, &str, GridEdit, f32, &str); 3] = [
        (
            "grid_halve",
            "÷2",
            GridEdit::Halve,
            22.0,
            "Halve the grid tempo",
        ),
        (
            "grid_double",
            "×2",
            GridEdit::Double,
            22.0,
            "Double the grid tempo",
        ),
        (
            "grid_tap",
            "TAP",
            GridEdit::Tap,
            30.0,
            "Tap tempo: tap on the beat 4 times or more",
        ),
    ];
    for (name, label, edit, w, hover) in edits {
        let c = row.cell(w);
        let hover = if locked { "The grid is locked" } else { hover };
        if button(app, ui, h, c, name, label, false, hover) && !locked {
            app.grid_edit(h.deck, edit);
        }
    }
    let c = row.cell(36.0);
    if button(
        app,
        ui,
        h,
        c,
        "grid_lock",
        "LOCK",
        locked,
        "Lock the grid: no edits, and re-analysis keeps it",
    ) {
        app.grid_edit(h.deck, GridEdit::ToggleLock);
    }
    row.space(6.0);
    let c = row.rest();
    let c = Rect::from_min_max(pos2(c.right() - 62.0, c.top()), c.max);
    if c.left() > row.x - 3.0
        && button(
            app,
            ui,
            h,
            c,
            "sync_master",
            "MASTER",
            snap.sync_master == Some(h.deck),
            "Make this deck the sync master",
        )
    {
        app.make_master(h.deck);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_fit_in_the_panel_height() {
        // Header, two pad rows of at least 30 px, four control rows.
        let need = 24.0 + 2.0 * 30.0 + GAP + ROW_H * 4.0 + GAP * 6.0 + 12.0;
        assert!(need <= PADS_MIN.y, "{need}");
    }

    #[test]
    fn colours_unpack() {
        assert_eq!(rgb(0x12_34_56), Color32::from_rgb(0x12, 0x34, 0x56));
    }

    #[test]
    fn half_width_holds_every_row() {
        let half = (PADS_MIN.x - 12.0 - 12.0) / 2.0;
        let loop_row =
            LABEL_W + 32.0 + 36.0 + 52.0 + 4.0 + 24.0 + 46.0 + 24.0 + 4.0 + 48.0 + 3.0 * 9.0;
        let jump_row = LABEL_W
            + 24.0
            + 18.0
            + 34.0
            + 18.0
            + 24.0
            + 8.0
            + 40.0
            + 8.0
            + 28.0
            + 18.0
            + 32.0
            + 18.0
            + 3.0 * 12.0;
        let grid_row = 36.0
            + (18.0 + 26.0 + 18.0)
            + (18.0 + 24.0 + 18.0)
            + 22.0
            + 22.0
            + 30.0
            + 36.0
            + 6.0
            + 62.0
            + 3.0 * 12.0;
        for (name, w) in [("loop", loop_row), ("jump", jump_row), ("grid", grid_row)] {
            assert!(w <= half, "{name} row {w} > {half}");
        }
    }
}
