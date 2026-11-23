//! Custom controls drawn with egui shapes: knobs, faders, meters, big
//! transport buttons, hold buttons, chips and small icons. No icon font or
//! image is used.

use crate::theme::Palette;
use dj_audio::Meter;
use egui::epaint::text::{LayoutJob, TextWrapping};
use egui::{
    Align2, Color32, FontId, Id, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, pos2, vec2,
};
use std::f32::consts::PI;
use std::ops::RangeInclusive;

/// Lays out `text` on one line no wider than `max_width`, cut with "…".
pub fn elided(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = TextWrapping {
        max_width: max_width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

/// Paints `text` cut to `rect`'s width, aligned left or right and centred
/// vertically. Returns true if it had to be cut.
pub fn text_in(
    painter: &egui::Painter,
    rect: Rect,
    text: &str,
    font: FontId,
    color: Color32,
    right: bool,
) -> bool {
    let g = elided(painter, text, font, color, rect.width());
    let cut = g.elided;
    let x = if right {
        rect.right() - g.size().x
    } else {
        rect.left()
    };
    painter.galley(pos2(x, rect.center().y - g.size().y / 2.0), g, color);
    cut
}

/// Angle of a knob at `t` in 0..=1 (radians, 0 = pointing up).
fn knob_angle(t: f32) -> f32 {
    (-135.0 + 270.0 * t) * PI / 180.0
}

fn arc_points(c: Pos2, r: f32, a0: f32, a1: f32) -> Vec<Pos2> {
    let n = (((a1 - a0).abs() / (PI / 24.0)).ceil() as usize).max(1);
    (0..=n)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / n as f32;
            pos2(c.x + r * a.sin(), c.y - r * a.cos())
        })
        .collect()
}

/// A rotary knob in `rect` (the knob fills the largest centred circle).
/// Drag up/down to turn (hold Shift for fine steps), double-click to reset
/// to `default`. Returns the response and the new value if the user changed
/// it. `bipolar` draws the value arc from the centre (trim, filter).
#[allow(clippy::too_many_arguments)]
pub fn knob(
    ui: &mut egui::Ui,
    rect: Rect,
    id: Id,
    value: f32,
    range: RangeInclusive<f32>,
    default: f32,
    bipolar: bool,
    color: Color32,
    hover: &str,
) -> (Response, Option<f32>) {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let (lo, hi) = (*range.start(), *range.end());
    let span = (hi - lo).max(f32::EPSILON);
    let mut new = None;
    if resp.double_clicked() {
        new = Some(default);
    } else if resp.dragged() {
        let dy = resp.drag_delta().y;
        if dy != 0.0 {
            let fine = ui.input(|i| i.modifiers.shift);
            let per_px = span / if fine { 1500.0 } else { 150.0 };
            new = Some((value - dy * per_px).clamp(lo, hi));
        }
    }
    let shown = new.unwrap_or(value);
    let t = ((shown - lo) / span).clamp(0.0, 1.0);
    let p = Palette::get(ui.visuals().dark_mode);
    let painter = ui.painter();
    let c = rect.center();
    let r = (rect.width().min(rect.height()) / 2.0 - 1.0).max(2.0);
    let track_w = (r * 0.18).clamp(1.5, 3.0);
    painter.add(Shape::line(
        arc_points(c, r - track_w / 2.0, knob_angle(0.0), knob_angle(1.0)),
        Stroke::new(track_w, p.knob_track),
    ));
    let from = if bipolar { 0.5 } else { 0.0 };
    if (t - from).abs() > 0.004 {
        painter.add(Shape::line(
            arc_points(
                c,
                r - track_w / 2.0,
                knob_angle(from.min(t)),
                knob_angle(from.max(t)),
            ),
            Stroke::new(track_w, color),
        ));
    }
    let body = r - track_w - 1.5;
    let fill = if resp.hovered() || resp.dragged() {
        p.knob.gamma_multiply(1.15)
    } else {
        p.knob
    };
    painter.circle(c, body, fill, Stroke::new(1.0, p.border));
    let a = knob_angle(t);
    let dir = vec2(a.sin(), -a.cos());
    painter.line_segment(
        [c + dir * body * 0.25, c + dir * body * 0.95],
        Stroke::new(2.0, p.text),
    );
    let resp = resp.on_hover_text(hover);
    (resp, new)
}

/// A vertical fader in `rect`. `t` is the position from 0 (bottom) to 1
/// (top). Clicking or dragging moves the cap to the pointer, like egui's
/// slider. Returns the response and the new position if it changed.
pub fn vfader(
    ui: &mut egui::Ui,
    rect: Rect,
    id: Id,
    t: f32,
    color: Color32,
    centre_mark: bool,
) -> (Response, Option<f32>) {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let cap_h = (rect.width() * 0.45).clamp(6.0, 14.0);
    let travel = (rect.height() - cap_h).max(1.0);
    let mut new = None;
    if (resp.dragged() || resp.clicked() || resp.drag_started())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let v = ((rect.bottom() - cap_h / 2.0 - pos.y) / travel).clamp(0.0, 1.0);
        if (v - t).abs() > f32::EPSILON {
            new = Some(v);
        }
    }
    let shown = new.unwrap_or(t).clamp(0.0, 1.0);
    let p = Palette::get(ui.visuals().dark_mode);
    let painter = ui.painter();
    let cx = rect.center().x;
    let slot = Rect::from_center_size(
        pos2(cx, rect.center().y),
        vec2(4.0, rect.height() - cap_h * 0.5),
    );
    painter.rect_filled(slot, 2.0, p.well);
    if centre_mark {
        let y = rect.center().y;
        painter.line_segment(
            [pos2(rect.left(), y), pos2(rect.right(), y)],
            Stroke::new(1.0, p.text_weak),
        );
    }
    let cy = rect.bottom() - cap_h / 2.0 - shown * travel;
    let cap = Rect::from_center_size(pos2(cx, cy), vec2(rect.width(), cap_h));
    let fill = if resp.hovered() || resp.dragged() {
        p.knob.gamma_multiply(1.2)
    } else {
        p.knob
    };
    painter.rect(
        cap,
        2.0,
        fill,
        Stroke::new(1.0, p.border),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [pos2(cap.left() + 2.0, cy), pos2(cap.right() - 2.0, cy)],
        Stroke::new(2.0, color),
    );
    (resp, new)
}

/// A horizontal fader in `rect` (crossfader). `t` runs from 0 (left) to 1
/// (right); clicking or dragging moves the cap to the pointer.
pub fn hfader(
    ui: &mut egui::Ui,
    rect: Rect,
    id: Id,
    t: f32,
    color: Color32,
) -> (Response, Option<f32>) {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let cap_w = (rect.height() * 0.7).clamp(8.0, 18.0);
    let travel = (rect.width() - cap_w).max(1.0);
    let mut new = None;
    if (resp.dragged() || resp.clicked() || resp.drag_started())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let v = ((pos.x - rect.left() - cap_w / 2.0) / travel).clamp(0.0, 1.0);
        if (v - t).abs() > f32::EPSILON {
            new = Some(v);
        }
    }
    let shown = new.unwrap_or(t).clamp(0.0, 1.0);
    let p = Palette::get(ui.visuals().dark_mode);
    let painter = ui.painter();
    let cy = rect.center().y;
    let slot = Rect::from_center_size(rect.center(), vec2(rect.width() - cap_w * 0.5, 4.0));
    painter.rect_filled(slot, 2.0, p.well);
    painter.line_segment(
        [
            pos2(rect.center().x, rect.top() + 2.0),
            pos2(rect.center().x, rect.bottom() - 2.0),
        ],
        Stroke::new(1.0, p.text_weak),
    );
    let cx = rect.left() + cap_w / 2.0 + shown * travel;
    let cap = Rect::from_center_size(pos2(cx, cy), vec2(cap_w, rect.height()));
    let fill = if resp.hovered() || resp.dragged() {
        p.knob.gamma_multiply(1.2)
    } else {
        p.knob
    };
    painter.rect(
        cap,
        2.0,
        fill,
        Stroke::new(1.0, p.border),
        egui::StrokeKind::Inside,
    );
    painter.line_segment(
        [pos2(cx, cap.top() + 2.0), pos2(cx, cap.bottom() - 2.0)],
        Stroke::new(2.0, color),
    );
    (resp, new)
}

fn meter_frac(v: f32) -> f32 {
    if v <= 0.0 {
        0.0
    } else {
        ((20.0 * v.log10() + 48.0) / 48.0).clamp(0.0, 1.0)
    }
}

fn meter_color(peak_lin: f32, frac: f32) -> Color32 {
    if peak_lin >= 0.99 {
        Color32::from_rgb(230, 60, 60)
    } else if frac > 0.85 {
        Color32::from_rgb(230, 200, 60)
    } else {
        Color32::from_rgb(70, 200, 110)
    }
}

/// Stereo level meter in `rect`: peak-level bars plus peak-hold lines, -48
/// to 0 dBFS.
/// Vertical when taller than wide.
pub fn paint_meter(painter: &egui::Painter, rect: Rect, m: Meter, well: Color32) {
    painter.rect_filled(rect, 1.0, well);
    let vertical = rect.height() >= rect.width();
    for c in 0..2 {
        // The bar is the falling peak level (like a hardware DJ meter); the
        // line on top is the held peak, which drops back onto the bar.
        let rms = meter_frac(m.level[c]);
        let peak = meter_frac(m.hold[c]);
        let color = meter_color(m.peak[c], meter_frac(m.peak[c]));
        if vertical {
            let w = rect.width() / 2.0;
            let x0 = rect.left() + c as f32 * w;
            let top = rect.bottom() - rect.height() * rms;
            painter.rect_filled(
                Rect::from_min_max(pos2(x0 + 1.0, top), pos2(x0 + w - 1.0, rect.bottom())),
                0.0,
                color,
            );
            let py = rect.bottom() - rect.height() * peak;
            painter.line_segment(
                [pos2(x0 + 1.0, py), pos2(x0 + w - 1.0, py)],
                Stroke::new(1.5, Color32::WHITE),
            );
        } else {
            let h = rect.height() / 2.0;
            let y0 = rect.top() + c as f32 * h;
            let right = rect.left() + rect.width() * rms;
            painter.rect_filled(
                Rect::from_min_max(pos2(rect.left(), y0 + 1.0), pos2(right, y0 + h - 1.0)),
                0.0,
                color,
            );
            let px = rect.left() + rect.width() * peak;
            painter.line_segment(
                [pos2(px, y0 + 1.0), pos2(px, y0 + h - 1.0)],
                Stroke::new(1.5, Color32::WHITE),
            );
        }
    }
}

/// Icons drawn into a square-ish rect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Play,
    Pause,
    Cue,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    SortUp,
    SortDown,
    /// Settings: a ring with eight teeth (drawn, no icon font).
    Gear,
    /// New crate: a crate (box) outline with a plus.
    AddCrate,
    /// New smart crate: the crate outline with a four-point spark (it
    /// fills itself).
    AddSmartCrate,
    /// Choose columns: three bars of a table.
    Columns,
    /// Import: an arrow down into an open tray.
    Import,
    /// Another DJ program's library: a record (rings and a centre hole).
    Record,
    /// A music app's library: an eighth note.
    Note,
    /// A playlist file: three lines with dots in front.
    List,
}

pub fn paint_icon(painter: &egui::Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) / 2.0;
    match icon {
        Icon::Import => {
            let w = (s * 0.17).max(1.5);
            let stroke = Stroke::new(w, color);
            // The tray: open at the top, rounded at the bottom corners.
            let (half, top, bottom) = (s * 0.85, c.y + s * 0.15, c.y + s * 0.85);
            let r = s * 0.22;
            let mut pts = vec![pos2(c.x + half, top)];
            for (cx, from) in [(c.x + half - r, 0.0_f32), (c.x - half + r, PI / 2.0)] {
                for k in 0..=6 {
                    let a = from + k as f32 / 6.0 * PI / 2.0;
                    pts.push(pos2(cx + r * a.cos(), bottom - r + r * a.sin()));
                }
            }
            pts.push(pos2(c.x - half, top));
            painter.add(Shape::line(pts, stroke));
            // The arrow: a shaft from the top into the tray, a chevron head.
            let tip = pos2(c.x, c.y + s * 0.4);
            painter.line_segment([pos2(c.x, c.y - s * 0.9), tip], stroke);
            let k = s * 0.38;
            painter.add(Shape::line(
                vec![pos2(c.x - k, tip.y - k), tip, pos2(c.x + k, tip.y - k)],
                stroke,
            ));
        }
        Icon::Record => {
            let w = (s * 0.15).max(1.3);
            painter.circle_stroke(c, s * 0.85, Stroke::new(w, color));
            painter.circle_stroke(c, s * 0.5, Stroke::new(w * 0.7, color));
            painter.circle_filled(c, s * 0.18, color);
        }
        Icon::Note => {
            let w = (s * 0.16).max(1.4);
            let head = pos2(c.x - s * 0.2, c.y + s * 0.55);
            let stem_x = head.x + s * 0.3;
            painter.circle_filled(head, s * 0.32, color);
            painter.line_segment(
                [pos2(stem_x, head.y), pos2(stem_x, c.y - s * 0.85)],
                Stroke::new(w, color),
            );
            painter.add(Shape::line(
                vec![
                    pos2(stem_x, c.y - s * 0.85),
                    pos2(stem_x + s * 0.5, c.y - s * 0.45),
                    pos2(stem_x + s * 0.45, c.y - s * 0.05),
                ],
                Stroke::new(w, color),
            ));
        }
        Icon::List => {
            let w = (s * 0.16).max(1.4);
            for k in [-1.0, 0.0, 1.0] {
                let y = c.y + k * s * 0.6;
                painter.circle_filled(pos2(c.x - s * 0.7, y), w * 0.75, color);
                painter.line_segment(
                    [pos2(c.x - s * 0.35, y), pos2(c.x + s * 0.85, y)],
                    Stroke::new(w, color),
                );
            }
        }
        Icon::Columns => {
            let w = s * 0.5;
            for k in [-1.0, 0.0, 1.0] {
                let x = c.x + k * s * 0.7;
                painter.rect_filled(
                    Rect::from_center_size(pos2(x, c.y), vec2(w * 0.7, s * 1.7)),
                    1.0,
                    color,
                );
            }
        }
        Icon::AddCrate | Icon::AddSmartCrate => {
            // A storage box: a lid with slanted top corners, a line under
            // it, a body with rounded bottom corners; a handle slot in the
            // body (a smart crate: an S there instead).
            let w = (s * 0.18).max(1.5);
            let stroke = Stroke::new(w, color);
            let (half, top_half) = (s * 0.86, s * 0.5);
            let top = c.y - s * 0.82;
            let lid = c.y - s * 0.28;
            let bottom = c.y + s * 0.86;
            let r = s * 0.26;
            let mut pts = vec![
                pos2(c.x - top_half, top),
                pos2(c.x + top_half, top),
                pos2(c.x + half, lid),
            ];
            // The rounded bottom corners, right then left.
            for (cx, from) in [(c.x + half - r, 0.0_f32), (c.x - half + r, PI / 2.0)] {
                for k in 0..=6 {
                    let a = from + k as f32 / 6.0 * PI / 2.0;
                    pts.push(pos2(cx + r * a.cos(), bottom - r + r * a.sin()));
                }
            }
            pts.push(pos2(c.x - half, lid));
            painter.add(Shape::closed_line(pts, stroke));
            painter.line_segment([pos2(c.x - half, lid), pos2(c.x + half, lid)], stroke);
            let m = pos2(c.x, (lid + bottom) / 2.0);
            if icon == Icon::AddCrate {
                painter.rect_filled(Rect::from_center_size(m, vec2(s * 0.62, w)), w / 2.0, color);
            } else {
                painter.text(
                    m + vec2(0.0, s * 0.03),
                    Align2::CENTER_CENTER,
                    "S",
                    crate::theme::display(painter.ctx(), (bottom - lid) * 0.95),
                    color,
                );
            }
        }
        Icon::Gear => {
            for i in 0..8 {
                let a = i as f32 * PI / 4.0;
                let d = vec2(a.cos(), a.sin());
                painter.line_segment(
                    [c + d * (s * 0.55), c + d * (s * 0.9)],
                    Stroke::new(s * 0.3, color),
                );
            }
            painter.circle_stroke(c, s * 0.5, Stroke::new(s * 0.3, color));
        }
        Icon::Play => {
            painter.add(Shape::convex_polygon(
                vec![
                    pos2(c.x - s * 0.55, c.y - s * 0.7),
                    pos2(c.x + s * 0.75, c.y),
                    pos2(c.x - s * 0.55, c.y + s * 0.7),
                ],
                color,
                Stroke::NONE,
            ));
        }
        Icon::Pause => {
            for dx in [-0.4, 0.4] {
                painter.rect_filled(
                    Rect::from_center_size(pos2(c.x + dx * s, c.y), vec2(s * 0.42, s * 1.4)),
                    1.0,
                    color,
                );
            }
        }
        Icon::Cue => {
            // A half disc pointing right with a bar: our own cue mark.
            painter.rect_filled(
                Rect::from_center_size(pos2(c.x - s * 0.55, c.y), vec2(s * 0.28, s * 1.4)),
                1.0,
                color,
            );
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = -PI / 2.0 + PI * i as f32 / 12.0;
                    pos2(c.x - s * 0.25 + s * 0.7 * a.cos(), c.y + s * 0.7 * a.sin())
                })
                .collect();
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
        Icon::ChevronDown
        | Icon::ChevronRight
        | Icon::ChevronLeft
        | Icon::SortUp
        | Icon::SortDown => {
            let k = s * 0.5;
            let pts = match icon {
                Icon::ChevronDown | Icon::SortDown => {
                    vec![
                        pos2(c.x - k, c.y - k * 0.5),
                        pos2(c.x + k, c.y - k * 0.5),
                        pos2(c.x, c.y + k * 0.6),
                    ]
                }
                Icon::SortUp => {
                    vec![
                        pos2(c.x - k, c.y + k * 0.5),
                        pos2(c.x, c.y - k * 0.6),
                        pos2(c.x + k, c.y + k * 0.5),
                    ]
                }
                Icon::ChevronLeft => vec![
                    pos2(c.x + k * 0.5, c.y - k),
                    pos2(c.x + k * 0.5, c.y + k),
                    pos2(c.x - k * 0.6, c.y),
                ],
                _ => vec![
                    pos2(c.x - k * 0.5, c.y - k),
                    pos2(c.x + k * 0.6, c.y),
                    pos2(c.x - k * 0.5, c.y + k),
                ],
            };
            painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
        }
    }
}

/// A flat button with a fill colour when `on`, drawn in `rect`. Used for
/// transport (CUE, PLAY), toggles (KEY LOCK, PFL, kills) and chips.
/// `hold` makes it sense drags too, so a long press stays held (see
/// [`is_held`]).
#[allow(clippy::too_many_arguments)]
pub fn flat_button(
    ui: &mut egui::Ui,
    rect: Rect,
    id: Id,
    label: &str,
    font: FontId,
    icon: Option<Icon>,
    on: bool,
    on_fill: Color32,
    hold: bool,
) -> Response {
    let sense = if hold {
        Sense::click_and_drag()
    } else {
        Sense::click()
    };
    let resp = ui.interact(rect, id, sense);
    let p = Palette::get(ui.visuals().dark_mode);
    let pressed = resp.is_pointer_button_down_on();
    let fill = if on {
        on_fill
    } else if pressed {
        ui.visuals().widgets.active.bg_fill
    } else if resp.hovered() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.inactive.bg_fill
    };
    let stroke = if on {
        Stroke::new(1.0, on_fill)
    } else {
        Stroke::new(1.0, if resp.hovered() { on_fill } else { p.border })
    };
    let painter = ui.painter();
    painter.rect(rect, 4.0, fill, stroke, egui::StrokeKind::Inside);
    let fg = if on { Palette::on(on_fill) } else { p.text };
    let has_text = !label.is_empty();
    // Captions in capitals take the display face (the signature look);
    // smaller steps when one does not fit, before it is cut.
    let font = if crate::theme::is_caption(label) {
        let room = rect.width() - 10.0;
        let mut size = font.size * crate::theme::DISPLAY_SCALE;
        let min = size * 0.75;
        loop {
            let f = crate::theme::display(ui.ctx(), size);
            let w = painter
                .layout_no_wrap(label.to_owned(), f.clone(), fg)
                .size()
                .x;
            if w <= room || size <= min {
                break f;
            }
            size -= 0.5;
        }
    } else {
        font
    };
    match icon {
        Some(icon) => {
            let ih = (rect.height() * 0.55).min(22.0).min(rect.width() - 6.0);
            // A smaller font when the text does not fit, then the icon only.
            let text_w = |f: &FontId| {
                painter
                    .layout_no_wrap(label.to_owned(), f.clone(), fg)
                    .size()
                    .x
            };
            let room = rect.width() - 8.0 - ih - 6.0;
            let mut font = font;
            if has_text && text_w(&font) > room {
                font = FontId::new(font.size * 0.8, font.family.clone());
            }
            let has_text = has_text && text_w(&font) <= room;
            let total = if has_text {
                ih + 6.0 + text_w(&font)
            } else {
                ih
            };
            let x0 = rect.center().x - total / 2.0;
            let ir = Rect::from_min_size(pos2(x0, rect.center().y - ih / 2.0), vec2(ih, ih));
            paint_icon(painter, ir, icon, if on { fg } else { on_fill });
            if has_text {
                painter.text(
                    pos2(ir.right() + 6.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    font,
                    fg,
                );
            }
        }
        None => {
            let g = elided(painter, label, font, fg, rect.width() - 4.0);
            let pos = rect.center() - g.size() / 2.0;
            painter.galley(pos, g, fg);
        }
    }
    resp
}

/// A button that reports "held" for as long as the mouse button stays down
/// on it (CUE, pitch bend), laid out by egui.
///
/// It senses drags as well as clicks: egui turns a press held longer than
/// its click window (0.8 s) into a drag, and a click-only button stops
/// reporting the press at that moment (CUE preview stopped by itself).
pub fn hold_button(ui: &mut egui::Ui, label: RichText, min_width: f32) -> Response {
    ui.add(
        egui::Button::new(label)
            .min_size(vec2(min_width, 24.0))
            .sense(Sense::click_and_drag()),
    )
}

/// True while the primary (left) mouse button that went down on `r` is still
/// held. Other buttons do not count: a right-click opens a menu and must not
/// also fire the pad or button.
pub fn is_held(r: &Response) -> bool {
    r.is_pointer_button_down_on() && r.ctx.input(|i| i.pointer.primary_down())
}

/// For a button that repeats while held (the grid SLIP and STRETCH
/// arrows): true when it is pressed, and while it stays held again after
/// 0.4 s every 0.05 s. `id` is the button's id.
pub fn held_repeat(ui: &egui::Ui, resp: &Response, id: egui::Id) -> bool {
    const REPEAT_DELAY: f64 = 0.4;
    const REPEAT_EVERY: f64 = 0.05;
    let next_id = id.with("next_fire");
    if !is_held(resp) {
        ui.data_mut(|d| d.remove::<f64>(next_id));
        return false;
    }
    let now = ui.input(|i| i.time);
    ui.ctx().request_repaint();
    match ui.data(|d| d.get_temp::<f64>(next_id)) {
        None => {
            ui.data_mut(|d| d.insert_temp(next_id, now + REPEAT_DELAY));
            true
        }
        Some(next) if now >= next => {
            ui.data_mut(|d| d.insert_temp(next_id, (next + REPEAT_EVERY).max(now)));
            true
        }
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, PointerButton};

    /// Runs one UI frame with a hold button; returns (held, button rect).
    fn frame(ctx: &egui::Context, time: f64, events: Vec<Event>, flat: bool) -> (bool, Rect) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(400.0, 200.0))),
            time: Some(time),
            events,
            ..Default::default()
        };
        let mut out = (false, Rect::NOTHING);
        let mut output = ctx.run_ui(input, |ui| {
            let r = if flat {
                let rect = Rect::from_min_size(pos2(10.0, 10.0), vec2(80.0, 40.0));
                flat_button(
                    ui,
                    rect,
                    Id::unique("cue"),
                    "CUE",
                    FontId::proportional(14.0),
                    Some(Icon::Cue),
                    false,
                    Color32::YELLOW,
                    true,
                )
            } else {
                hold_button(ui, RichText::new("CUE"), 48.0)
            };
            out = (is_held(&r), r.rect);
        });
        // No GPU in tests: drop the font texture upload on purpose.
        output.textures_delta.clear();
        out
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// Regression: egui turns a press held longer than its click window
    /// (0.8 s) into a drag; the CUE button must stay held until release.
    /// Checked for the egui-laid-out hold button and the drawn one the deck
    /// panels use.
    #[test]
    fn hold_button_stays_held_until_release() {
        for flat in [false, true] {
            let ctx = egui::Context::default();
            let (_, rect) = frame(&ctx, 0.0, vec![], flat);
            let pos = rect.center();

            let (held, _) = frame(
                &ctx,
                1.0 / 60.0,
                vec![Event::PointerMoved(pos), button(pos, true)],
                flat,
            );
            assert!(held, "held on the press frame");
            // Three seconds of holding still.
            for i in 2..180 {
                let (held, _) = frame(&ctx, f64::from(i) / 60.0, vec![], flat);
                assert!(
                    held,
                    "released by itself after {:.2} s",
                    f64::from(i) / 60.0
                );
            }
            let (held, _) = frame(&ctx, 3.0, vec![button(pos, false)], flat);
            assert!(!held, "released on the release frame");
            let (held, _) = frame(&ctx, 3.1, vec![], flat);
            assert!(!held, "stays released");
        }
    }

    #[test]
    fn knob_drag_and_double_click() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(30.0, 30.0));
        let run = |time: f64, events: Vec<Event>, value: f32| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(200.0, 200.0))),
                time: Some(time),
                events,
                ..Default::default()
            };
            let mut got = None;
            let mut out = ctx.run_ui(input, |ui| {
                got = knob(
                    ui,
                    rect,
                    Id::unique("k"),
                    value,
                    0.0..=1.0,
                    0.5,
                    false,
                    Color32::RED,
                    "test",
                )
                .1;
            });
            out.textures_delta.clear();
            got
        };
        let c = rect.center();
        run(0.0, vec![], 0.5);
        run(0.1, vec![Event::PointerMoved(c), button(c, true)], 0.5);
        // Drag 30 px up: +0.2.
        let mut v = 0.5;
        for (i, y) in [-10.0, -20.0, -30.0].into_iter().enumerate() {
            if let Some(n) = run(
                0.2 + i as f64 * 0.02,
                vec![Event::PointerMoved(c + vec2(0.0, y))],
                v,
            ) {
                v = n;
            }
        }
        assert!((v - 0.7).abs() < 0.02, "dragged to {v}");
        run(0.4, vec![button(c + vec2(0.0, -30.0), false)], v);
        // Double-click resets to the default.
        let mut reset = None;
        for (t, pressed) in [(1.0, true), (1.05, false), (1.1, true), (1.15, false)] {
            let mut ev = vec![Event::PointerMoved(c)];
            ev.push(button(c, pressed));
            if let Some(n) = run(t, ev, v) {
                reset = Some(n);
            }
        }
        assert_eq!(reset, Some(0.5));
    }
}
