//! Deck panels of the Extended layout (four panels in a 2x2 grid over the
//! full-width waveforms): header with the LAYER badge, title, key and BPM;
//! the track overview; two tabs, the hot cue list and the beatgrid
//! controls; CUE and PLAY; the loop length, SYNC, KEY and SLIP; and on the
//! inner side a platter display (BPM, pitch, elapsed and remaining time, a
//! needle turning like a 33 1/3 rpm record, a progress ring) with the
//! channel meter. Our own design; every control calls the same functions as
//! the other layouts. Pitch, bend and loading are on the controller and the
//! keyboard; loop in / out / reloop in the PADS panel and on the
//! controller.

use crate::app::KaldorApp;
use crate::deck_view::{
    DeckView, PanelTab, bpm_row, key_button, layer_badge, overview, shown_tempo, slip_button,
    sync_button, time_row, title_block, transport,
};
use crate::perform::{
    GRID_ADJUST, GRID_ADJUST_LARGE, GRID_NUDGE, GRID_NUDGE_LARGE, GridEdit, PadMode, size_label,
};
use crate::theme::{self, Palette, rgb};
use crate::widgets::{self, Icon, flat_button, is_held};
use dj_audio::{DeckCommand, HOT_CUES, JOG_SECONDS_PER_TURN};
use dj_core::time::format_mm_ss_t;
use egui::{Align2, Color32, Id, Rect, Sense, Shape, Stroke, pos2, vec2};
use std::f32::consts::{PI, TAU};

/// Remaining time below which the platter's progress ring turns amber.
const END_WARNING_SECONDS: f64 = 30.0;
const PAD: f32 = 6.0;
const ROW_GAP: f32 = 3.0;

/// Splits `rect` into `n` equal parts left to right with `gap` between.
fn columns(rect: Rect, n: usize, gap: f32) -> Vec<Rect> {
    let w = ((rect.width() - gap * (n as f32 - 1.0)) / n as f32).floor();
    (0..n)
        .map(|i| {
            let x = rect.left() + i as f32 * (w + gap);
            let right = if i + 1 == n { rect.right() } else { x + w };
            Rect::from_min_max(pos2(x, rect.top()), pos2(right, rect.bottom()))
        })
        .collect()
}

/// Header: LAYER badge, title over artist (click: keyboard deck), key chip
/// and BPM.
fn header(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    rect: Rect,
    side: usize,
    bpm_w: f32,
) {
    let h = rect.height();
    let b = Rect::from_min_size(rect.min, vec2(h, h));
    layer_badge(app, ui, v, b, side);
    let bpm = Rect::from_min_max(
        pos2((rect.right() - bpm_w).max(b.right() + 40.0), rect.top()),
        rect.max,
    );
    bpm_row(app, ui, v, bpm, (h * 0.68).clamp(14.0, 20.0));
    let text = Rect::from_min_max(
        pos2(b.right() + 8.0, rect.top()),
        pos2(bpm.left() - 6.0, rect.bottom()),
    );
    let r = title_block(app, ui, v, text, text)
        .on_hover_text("Click: the keyboard's pad, loop and jump keys work on this deck (F1-F4)");
    if r.clicked() {
        app.set_keyboard_deck(v.index);
    }
}

/// The full panel: header, overview, hot cue list, controls, platter and
/// meter. Left decks (1, 3) have the platter on the right, next to the
/// centre; right decks (2, 4) on the left.
pub(crate) fn full(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect, side: usize) {
    let inner = rect.shrink(PAD);
    let disc = inner.height();
    let meter_w = 8.0;
    let (content, platter_r, meter_r) = if side == 0 {
        let meter = Rect::from_min_max(pos2(inner.right() - meter_w, inner.top()), inner.max);
        let platter = Rect::from_min_size(
            pos2(meter.left() - 6.0 - disc, inner.top()),
            vec2(disc, disc),
        );
        let content = Rect::from_min_max(inner.min, pos2(platter.left() - 8.0, inner.bottom()));
        (content, platter, meter)
    } else {
        let meter = Rect::from_min_size(inner.min, vec2(meter_w, inner.height()));
        let platter = Rect::from_min_size(pos2(meter.right() + 6.0, inner.top()), vec2(disc, disc));
        let content = Rect::from_min_max(pos2(platter.right() + 8.0, inner.top()), inner.max);
        (content, platter, meter)
    };
    platter(app, ui, v, platter_r);
    deck_meter(app, ui, v, meter_r);

    let head = Rect::from_min_size(content.min, vec2(content.width(), 28.0));
    header(
        app,
        ui,
        v,
        head,
        side,
        (content.width() * 0.3).clamp(110.0, 170.0),
    );
    let ov = Rect::from_min_size(
        pos2(content.left(), head.bottom() + 4.0),
        vec2(content.width(), 18.0),
    );
    overview(app, ui, v.index, ov);
    let body = Rect::from_min_max(pos2(content.left(), ov.bottom() + 5.0), content.max);
    // The tabs and the cue list (or grid controls) on the outer side, the
    // controls next to the platter.
    let cue_w = (body.width() * 0.48).clamp(196.0, 376.0).floor();
    let (cues, controls) = if side == 0 {
        (
            Rect::from_min_size(body.min, vec2(cue_w, body.height())),
            Rect::from_min_max(pos2(body.left() + cue_w + 8.0, body.top()), body.max),
        )
    } else {
        (
            Rect::from_min_max(pos2(body.right() - cue_w, body.top()), body.max),
            Rect::from_min_max(body.min, pos2(body.right() - cue_w - 8.0, body.bottom())),
        )
    };
    // The tabs on the outer edge, stacked.
    let (tabs, list) = if side == 0 {
        (
            Rect::from_min_size(cues.min, vec2(TAB_W, cues.height())),
            Rect::from_min_max(pos2(cues.left() + TAB_W + 4.0, cues.top()), cues.max),
        )
    } else {
        (
            Rect::from_min_max(pos2(cues.right() - TAB_W, cues.top()), cues.max),
            Rect::from_min_max(cues.min, pos2(cues.right() - TAB_W - 4.0, cues.bottom())),
        )
    };
    panel_tabs(app, ui, v, tabs);
    match app.decks[v.index].panel_tab {
        PanelTab::Cues => cue_list(app, ui, v, list),
        PanelTab::Grid => grid_tab(app, ui, v, list),
    }
    let rows = rows_in(controls, 2);
    transport(app, ui, v, rows[0], 14.0);
    let cols = columns(rows[1], 5, 3.0);
    loop_length(app, ui, v, cols[0].union(cols[1]));
    sync_button(app, ui, v, cols[2]);
    key_button(app, ui, v, cols[3]);
    slip_button(app, ui, v, cols[4]);
}

/// Width of the tab strip next to the hot cue list.
const TAB_W: f32 = 46.0;

/// The two tabs of the hot cue area, stacked: CUES (the hot cue list) and
/// GRID (the beatgrid controls).
fn panel_tabs(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) {
    let rows = rows_in(rect, 2);
    let locked = app.decks[v.index].grid.is_some_and(|g| g.locked);
    let tabs = [
        (
            PanelTab::Cues,
            "CUES",
            "tab_cues",
            "Hot cues: jump, set, name, colour and delete",
        ),
        (
            PanelTab::Grid,
            "GRID",
            "tab_grid",
            if locked {
                "Beatgrid controls (the grid is locked)"
            } else {
                "Beatgrid controls: beat one, slip, stretch, halve, double, tap tempo, lock"
            },
        ),
    ];
    for ((tab, label, name, tip), r) in tabs.into_iter().zip(rows) {
        let on = app.decks[v.index].panel_tab == tab;
        let resp = flat_button(
            ui,
            r,
            v.uid(name),
            label,
            theme::prop(11.0),
            None,
            on,
            v.color,
            false,
        )
        .on_hover_text(tip);
        if resp.clicked() {
            app.decks[v.index].panel_tab = tab;
        }
        app.map.add(ui, name, Some(v.index), r);
    }
}

/// One button of the grid controls; greyed out and inert when `enabled`
/// is false. `hold`: senses holding (for the repeating arrows).
#[allow(clippy::too_many_arguments)]
fn grid_button(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    r: Rect,
    name: &'static str,
    label: &str,
    icon: Option<Icon>,
    on: bool,
    enabled: bool,
    hold: bool,
    tip: &str,
) -> egui::Response {
    let mut bu = ui.new_child(egui::UiBuilder::new().max_rect(r));
    if !enabled {
        bu.disable();
        bu.multiply_opacity(0.4);
    }
    let resp = flat_button(
        &mut bu,
        r,
        v.uid(name),
        label,
        theme::prop(12.0),
        icon,
        on,
        v.color,
        hold,
    )
    .on_hover_text(tip)
    .on_disabled_hover_text(tip);
    app.map.add(ui, name, Some(v.index), r);
    resp
}

/// The beatgrid controls (GRID tab) in the four rows of the cue list:
/// BEAT 1 and LOCK; SLIP (move the whole grid); STRETCH (squeeze or
/// stretch it around the play position); ÷2, ×2 and TAP. The arrows repeat
/// while held, with larger steps while Shift is held. A locked grid takes
/// no edits but LOCK. Each calls the same grid edit as the keys and the
/// PADS panel.
fn grid_tab(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) {
    let grid = app.decks[v.index].grid;
    let has_grid = v.d.loaded && grid.is_some();
    let locked = grid.is_some_and(|g| g.locked);
    let can_edit = has_grid && !locked;
    let shift = ui.input(|i| i.modifiers.shift);
    let (slip, stretch) = if shift {
        (GRID_NUDGE_LARGE, GRID_ADJUST_LARGE)
    } else {
        (GRID_NUDGE, GRID_ADJUST)
    };
    let why = |tip: &str| -> String {
        if !v.d.loaded {
            "Load a track first".into()
        } else if grid.is_none() {
            "No beatgrid yet: analyse the track, or tap the tempo (TAP)".into()
        } else if locked {
            "The grid is locked (LOCK)".into()
        } else {
            tip.to_string()
        }
    };
    let rows = rows_in(rect, 4);

    // BEAT 1 and LOCK. Beat one also starts a grid from the track's BPM
    // when there is none.
    let lock_w = (rows[0].width() * 0.34).floor();
    let b1 = Rect::from_min_max(
        rows[0].min,
        pos2(rows[0].right() - lock_w - 4.0, rows[0].bottom()),
    );
    let lk = Rect::from_min_max(pos2(rows[0].right() - lock_w, rows[0].top()), rows[0].max);
    let start_ok = v.d.loaded && !locked && (grid.is_some() || app.decks[v.index].bpm.is_some());
    let tip = if start_ok {
        "Beat one (the start of a bar) at the play position: put the playhead on a downbeat first"
            .to_string()
    } else {
        why("")
    };
    if grid_button(
        app,
        ui,
        v,
        b1,
        "grid_beat1",
        "BEAT 1",
        None,
        false,
        start_ok,
        false,
        &tip,
    )
    .clicked()
    {
        app.grid_edit(v.index, GridEdit::SetBeatOne);
    }
    let lock_tip = if has_grid {
        if locked {
            "Unlock the grid"
        } else {
            "Lock the grid: no edits, and re-analysis keeps it"
        }
    } else {
        "No beatgrid to lock"
    };
    if grid_button(
        app,
        ui,
        v,
        lk,
        "grid_lock",
        "LOCK",
        None,
        locked,
        has_grid,
        false,
        lock_tip,
    )
    .clicked()
    {
        app.grid_edit(v.index, GridEdit::ToggleLock);
    }

    // SLIP and STRETCH: ◀ name and step ▶.
    let pairs = [
        (
            rows[1],
            "SLIP",
            format!("{:.0} ms", slip * 1e3),
            ("grid_slip_earlier", "grid_slip_later"),
            (GridEdit::Nudge(-slip), GridEdit::Nudge(slip)),
            format!(
                "Slip: move the whole grid earlier / later, {:.0} ms a step ({:.0} ms with Shift). \
                 Hold to repeat. Ctrl+drag on the waveform does the same",
                GRID_NUDGE * 1e3,
                GRID_NUDGE_LARGE * 1e3
            ),
        ),
        (
            rows[2],
            "STRETCH",
            format!("{:.1} ms", stretch * 1e3),
            ("grid_stretch_down", "grid_stretch_up"),
            (GridEdit::Adjust(-stretch), GridEdit::Adjust(stretch)),
            format!(
                "Stretch: squeeze / stretch the grid around the play position; the beat there \
                 stays, beats further away move more ({:.1} ms per beat a step, {:.0} ms with \
                 Shift). Hold to repeat. Ctrl+Shift+drag on the waveform does the same",
                GRID_ADJUST * 1e3,
                GRID_ADJUST_LARGE * 1e3
            ),
        ),
    ];
    for (row, label, step, (n_down, n_up), (e_down, e_up), tip) in pairs {
        let arrow_w = (row.height() * 1.5).min(row.width() / 4.0).floor();
        let down = Rect::from_min_size(row.min, vec2(arrow_w, row.height()));
        let up = Rect::from_min_max(pos2(row.right() - arrow_w, row.top()), row.max);
        let mid = Rect::from_min_max(
            pos2(down.right() + 4.0, row.top()),
            pos2(up.left() - 4.0, row.bottom()),
        );
        let tip = why(&tip);
        for (r, name, left, edit) in [(down, n_down, true, e_down), (up, n_up, false, e_up)] {
            let resp = grid_button(app, ui, v, r, name, "", None, false, can_edit, true, &tip);
            // An arrow about half the row high (the stock icons are drawn
            // for taller buttons).
            let (c, k) = (r.center(), (r.height() * 0.22).max(3.0));
            let s = if left { -1.0 } else { 1.0 };
            let ink = if can_edit {
                v.color
            } else {
                v.pal.text_weak.gamma_multiply(0.5)
            };
            ui.painter().add(Shape::convex_polygon(
                vec![
                    pos2(c.x - s * k * 0.6, c.y - k),
                    pos2(c.x + s * k * 0.8, c.y),
                    pos2(c.x - s * k * 0.6, c.y + k),
                ],
                ink,
                Stroke::NONE,
            ));
            if can_edit && widgets::held_repeat(ui, &resp, v.uid(name)) {
                app.grid_edit(v.index, edit);
            }
        }
        // The name and the step size between the arrows.
        let painter = ui.painter_at(mid);
        painter.rect_filled(mid, 4.0, v.pal.well);
        let ink = if can_edit {
            v.pal.text
        } else {
            v.pal.text_weak
        };
        painter.text(
            pos2(mid.left() + 8.0, mid.center().y),
            Align2::LEFT_CENTER,
            label,
            theme::prop(12.0),
            ink,
        );
        painter.text(
            pos2(mid.right() - 8.0, mid.center().y),
            Align2::RIGHT_CENTER,
            step,
            theme::mono(11.0),
            v.pal.text_weak,
        );
        ui.interact(mid, v.uid(label), Sense::hover())
            .on_hover_text(&tip);
    }

    // ÷2, ×2, TAP.
    let cols = columns(rows[3], 3, 4.0);
    let edits = [
        (
            cols[0],
            "grid_halve",
            "÷2",
            GridEdit::Halve,
            can_edit,
            why("Halve the grid tempo (half as many beats)"),
        ),
        (
            cols[1],
            "grid_double",
            "×2",
            GridEdit::Double,
            can_edit,
            why("Double the grid tempo (twice as many beats)"),
        ),
        (
            cols[2],
            "grid_tap",
            "TAP",
            GridEdit::Tap,
            v.d.loaded && !locked,
            if !v.d.loaded {
                "Load a track first".to_string()
            } else if locked {
                "The grid is locked (LOCK)".to_string()
            } else {
                "Tap tempo: tap on the beat 4 times or more; the last tap is a beat".to_string()
            },
        ),
    ];
    for (r, name, label, edit, enabled, tip) in edits {
        if grid_button(
            app, ui, v, r, name, label, None, false, enabled, false, &tip,
        )
        .clicked()
        {
            app.grid_edit(v.index, edit);
        }
    }
}

/// The compact panel (short or narrow windows): header, overview, and CUE,
/// PLAY, SYNC, KEY and the remaining time.
pub(crate) fn compact(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    v: &DeckView,
    rect: Rect,
    side: usize,
) {
    let c = rect.shrink(PAD);
    let head = Rect::from_min_size(c.min, vec2(c.width(), 28.0));
    header(
        app,
        ui,
        v,
        head,
        side,
        (c.width() * 0.32).clamp(80.0, 150.0),
    );
    let ov = Rect::from_min_size(pos2(c.left(), head.bottom() + 4.0), vec2(c.width(), 16.0));
    overview(app, ui, v.index, ov);
    let row = Rect::from_min_max(
        pos2(c.left(), ov.bottom() + 4.0),
        pos2(c.right(), c.bottom().min(ov.bottom() + 4.0 + 28.0)),
    );
    let tr_w = (row.width() * 0.44).floor();
    let tr = Rect::from_min_size(row.min, vec2(tr_w, row.height()));
    transport(app, ui, v, tr, 12.0);
    let rest = Rect::from_min_max(pos2(tr.right() + 4.0, row.top()), row.max);
    let [s, k, t] = columns(rest, 3, 4.0)[..] else {
        return;
    };
    sync_button(app, ui, v, s);
    key_button(app, ui, v, k);
    time_row(app, ui, v, t.shrink2(vec2(0.0, 4.0)), 12.0);
}

/// `n` equal rows in `rect`.
fn rows_in(rect: Rect, n: usize) -> Vec<Rect> {
    let h = ((rect.height() - ROW_GAP * (n as f32 - 1.0)) / n as f32).floor();
    (0..n)
        .map(|i| {
            Rect::from_min_size(
                pos2(rect.left(), rect.top() + i as f32 * (h + ROW_GAP)),
                vec2(rect.width(), h),
            )
        })
        .collect()
}

/// The loop length as one number: the running loop's length, else the auto
/// loop size (which the controller's loop knob and the keys change, so it
/// follows them). Click: auto loop of that length on / off; mouse wheel:
/// double (up) or halve (down), the running loop too.
fn loop_length(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, r: Rect) {
    let active = v.d.loop_info.filter(|l| l.active);
    let beats = active
        .map(|l| l.beats)
        .filter(|b| *b > 0.0)
        .unwrap_or(app.decks[v.index].perf.auto_loop_beats);
    let resp = flat_button(
        ui,
        r,
        v.uid("loop_length"),
        &format!("LOOP  {}", size_label(beats)),
        theme::prop(13.0),
        None,
        active.is_some(),
        v.color,
        false,
    )
    .on_hover_text(if v.d.grid.is_none() {
        "Loop length in beats (needs a beatgrid)"
    } else {
        "Loop length in beats. Click: loop on / off; mouse wheel: longer / shorter"
    });
    if resp.clicked() {
        app.auto_loop(v.index);
    }
    if resp.hovered() {
        // One step per wheel event (not the smoothed scroll).
        let wheel: f32 = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::MouseWheel { delta, .. } => Some(delta.y),
                    _ => None,
                })
                .sum()
        });
        if wheel != 0.0 {
            app.scale_loop(v.index, if wheel > 0.0 { 2.0 } else { 0.5 });
        }
    }
    app.map.add(ui, "loop_length", Some(v.index), r);
}

/// Hot cues 1-4 in the first column and 5-8 in the second: click to jump
/// (hold to play from the cue), empty rows set a cue, right-click to name,
/// colour or delete, × deletes.
fn cue_list(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) {
    let cols = columns(rect, 2, 4.0);
    for (c, col) in cols.iter().enumerate() {
        for (row, cell) in rows_in(*col, 4).into_iter().enumerate() {
            cue_row(app, ui, v, c * 4 + row, cell);
        }
    }
}

fn cue_row(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, k: usize, cell: Rect) {
    if k >= HOT_CUES {
        return;
    }
    let set = v.d.hot_cues[k].filter(|_| v.d.loaded);
    let meta = app.decks[v.index].perf.cue_meta[k].clone();
    let color = rgb(meta.color);
    let del_w = cell.height().min(20.0);
    let (trig, del) = if set.is_some() {
        (
            Rect::from_min_max(cell.min, pos2(cell.right() - del_w - 2.0, cell.bottom())),
            Some(Rect::from_min_max(
                pos2(cell.right() - del_w, cell.top()),
                cell.max,
            )),
        )
    } else {
        (cell, None)
    };
    // A set cue is a box in its colour with its time on it; an empty one a
    // plain box. Both show the cue number at the start.
    let held_now = app.decks[v.index].cue_list_held[k];
    let resp = flat_button(
        ui,
        trig,
        Id::unique(("cue_row", v.index, k)),
        "",
        theme::mono(12.5),
        None,
        set.is_some(),
        color,
        true,
    );
    let ink = if set.is_some() {
        Palette::on(color)
    } else {
        v.pal.text_weak
    };
    let painter = ui.painter_at(trig);
    painter.text(
        pos2(trig.left() + 7.0, trig.center().y),
        Align2::LEFT_CENTER,
        (k + 1).to_string(),
        theme::mono(12.5),
        ink,
    );
    if let Some(f) = set {
        painter.text(
            trig.center(),
            Align2::CENTER_CENTER,
            format_mm_ss_t(f / v.sr),
            theme::mono(12.5),
            ink,
        );
    }
    if held_now {
        ui.painter().rect_stroke(
            trig,
            4.0,
            Stroke::new(2.0, Color32::WHITE),
            egui::StrokeKind::Inside,
        );
    }
    let resp = resp.on_hover_text(match set {
        Some(_) => format!(
            "Hot cue {}{}: click to jump, hold to play from it; right-click: name, colour, delete",
            k + 1,
            if meta.name.is_empty() {
                String::new()
            } else {
                format!(" \"{}\"", meta.name)
            }
        ),
        None => format!("Set hot cue {} at the play position", k + 1),
    });
    app.map.add(ui, "cue_row", Some(v.index), trig);
    let held = is_held(&resp) && v.d.loaded;
    if held != held_now {
        app.decks[v.index].cue_list_held[k] = held;
        let perf = &app.decks[v.index].perf;
        let loop_beats = if perf.cue_loop {
            perf.auto_loop_beats
        } else {
            0.0
        };
        app.deck_command(
            v.index,
            DeckCommand::HotCue {
                slot: k as u8,
                pressed: held,
                loop_beats,
            },
        );
    }
    if set.is_some() {
        crate::perform_view::marker_menu(app, &resp, v.index, k, PadMode::Cue);
    }
    if let Some(d) = del {
        let b = flat_button(
            ui,
            d,
            Id::unique(("cue_delete", v.index, k)),
            "×",
            theme::prop(13.0),
            None,
            false,
            v.pal.error,
            false,
        )
        .on_hover_text(format!("Delete hot cue {}", k + 1));
        if b.clicked() {
            app.delete_hot_cue(v.index, k);
        }
        app.map.add(ui, "cue_delete", Some(v.index), d);
    }
}

/// The deck's channel level (as on the mixer).
fn deck_meter(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) {
    let m = app.engine.snapshot().channel_meters[v.index];
    widgets::paint_meter(ui.painter(), rect, m, v.pal.well);
    let r = ui
        .interact(rect, v.uid("deck_meter"), Sense::hover())
        .on_hover_text("Channel level");
    app.map.add(ui, "deck_meter", Some(v.index), r.rect);
}

/// Points on a circle around `c` from angle `a0` to `a1` (radians, 0 = up,
/// clockwise).
fn arc(c: egui::Pos2, r: f32, a0: f32, a1: f32) -> Vec<egui::Pos2> {
    let steps = ((a1 - a0).abs() / TAU * 96.0).ceil().max(2.0) as usize;
    (0..=steps)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / steps as f32 - PI / 2.0;
            pos2(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect()
}

/// The platter display: current BPM, pitch and range, elapsed and
/// remaining time; a ring that fills with the track's progress (amber in
/// the last 30 s) and a needle turning with the play position like a
/// 33 1/3 rpm record (one turn per 1.8 s of audio, the jog's scale).
fn platter(app: &mut KaldorApp, ui: &mut egui::Ui, v: &DeckView, rect: Rect) {
    let pal: Palette = v.pal;
    let painter = ui.painter_at(rect.expand(2.0));
    let c = rect.center();
    let r = rect.width().min(rect.height()) / 2.0 - 1.0;
    let ring = 6.0;
    let d = &v.d;
    // Progress ring.
    painter.circle_stroke(c, r - ring / 2.0, Stroke::new(ring, pal.well));
    let remaining = (d.length - v.pos).max(0.0) / v.sr;
    if d.loaded && d.length > 0.0 {
        let frac = (v.pos / d.length).clamp(0.0, 1.0) as f32;
        let warn = remaining < END_WARNING_SECONDS && d.load_complete;
        if frac > 0.0 {
            painter.add(Shape::line(
                arc(c, r - ring / 2.0, 0.0, frac * TAU),
                Stroke::new(ring, if warn { pal.warn } else { v.color }),
            ));
        }
    }
    // Disc.
    let disc_r = r - ring - 3.0;
    painter.circle(c, disc_r, pal.raised, Stroke::new(1.0, pal.border));
    // Needle (under the numbers).
    if d.loaded {
        let turns = (v.pos / v.sr / JOG_SECONDS_PER_TURN).fract() as f32;
        let a = turns * TAU - PI / 2.0;
        let dir = vec2(a.cos(), a.sin());
        painter.line_segment(
            [c + dir * (disc_r * 0.18), c + dir * (disc_r - 3.0)],
            Stroke::new(3.0, v.color.gamma_multiply(0.75)),
        );
    }
    painter.circle_filled(c, 3.0, v.color);
    // Numbers.
    let deck = &app.decks[v.index];
    let bpm = match (d.loaded, deck.bpm) {
        (true, Some(b)) => format!("{:.1}", b * shown_tempo(d)),
        _ => "--.-".into(),
    };
    let big = (disc_r * 0.36).clamp(12.0, 34.0);
    let small = (disc_r * 0.19).clamp(9.0, 14.0);
    let mid = (disc_r * 0.2).clamp(9.0, 17.0);
    painter.text(
        pos2(c.x, c.y - disc_r * 0.42),
        Align2::CENTER_CENTER,
        bpm,
        theme::mono(big),
        if d.loaded { pal.text } else { pal.text_weak },
    );
    let pitch = (shown_tempo(d) - 1.0) * 100.0;
    painter.text(
        pos2(c.x - disc_r * 0.08, c.y - disc_r * 0.1),
        Align2::RIGHT_CENTER,
        format!("{pitch:+.1}%"),
        theme::mono(small),
        pal.text_weak,
    );
    painter.text(
        pos2(c.x + disc_r * 0.08, c.y - disc_r * 0.1),
        Align2::LEFT_CENTER,
        d.pitch_range.label(),
        theme::mono(small),
        pal.text_weak,
    );
    use dj_core::config::TimeDisplay;
    let shown = app.config.performance.time_display;
    let (elapsed, rest) = if d.loaded {
        let loading = !d.load_complete && d.length > 0.0;
        let left = format!("-{}", format_mm_ss_t(remaining));
        let rest = if loading {
            format!("{:.0}%", d.ready_frames as f64 / d.length * 100.0)
        } else if shown == TimeDisplay::Both {
            left.clone()
        } else {
            String::new()
        };
        let main = if shown == TimeDisplay::Remaining {
            left
        } else {
            format_mm_ss_t(v.pos / v.sr)
        };
        (main, rest)
    } else {
        ("-:--.-".to_string(), String::new())
    };
    painter.text(
        pos2(c.x, c.y + disc_r * 0.24),
        Align2::CENTER_CENTER,
        elapsed,
        theme::mono(mid),
        pal.text,
    );
    painter.text(
        pos2(c.x, c.y + disc_r * 0.52),
        Align2::CENTER_CENTER,
        rest,
        theme::mono(mid),
        if remaining < END_WARNING_SECONDS && d.loaded {
            pal.warn
        } else {
            pal.text_weak
        },
    );
    let resp = ui
        .interact(rect, v.uid("platter"), Sense::hover())
        .on_hover_text(
            "BPM, pitch and range, elapsed and remaining time; the ring is the track's progress",
        );
    app.map.add(ui, "platter", Some(v.index), resp.rect);
}
