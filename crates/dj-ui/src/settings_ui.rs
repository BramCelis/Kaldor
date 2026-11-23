//! Building blocks of the settings window: cards with a title, setting
//! rows (name and description on the left, the control on the right, thin
//! lines between rows), toggle switches, segmented choices and buttons.
//! Our own design; every block registers nothing by itself, the callers put
//! their controls in the UI map.

use crate::theme::{self, Palette};
use egui::{Align, Color32, Layout, Rect, Response, RichText, Sense, Stroke, Ui, pos2, vec2};

/// Width of the control side of a row (at most).
const CONTROL_W: f32 = 380.0;

/// A titled card. `body` gets a [`Rows`] to add setting rows to.
pub(crate) fn card<R>(
    ui: &mut Ui,
    pal: &Palette,
    title: &str,
    body: impl FnOnce(&mut Rows<'_>) -> R,
) -> R {
    ui.add_space(14.0);
    // The section's name in small spaced capitals, as the labels on the FX
    // display; the card has the same corners as the FX display and decks.
    ui.label(
        RichText::new(title.to_uppercase())
            .size(10.5)
            .extra_letter_spacing(1.2)
            .color(pal.text_weak),
    );
    ui.add_space(6.0);
    egui::Frame::new()
        .fill(card_fill(pal))
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(6.0)
        .inner_margin(egui::Margin::symmetric(16, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut rows = Rows {
                ui,
                n: 0,
                pal: *pal,
            };
            body(&mut rows)
        })
        .inner
}

/// The card background: a step above the window.
pub(crate) fn card_fill(pal: &Palette) -> Color32 {
    if pal.dark {
        Color32::from_rgb(0x1A, 0x1D, 0x23)
    } else {
        Color32::WHITE
    }
}

/// The rows of a card.
pub(crate) struct Rows<'a> {
    pub ui: &'a mut Ui,
    n: usize,
    pal: Palette,
}

impl Rows<'_> {
    fn separator(&mut self) {
        if self.n > 0 {
            let r = self.ui.available_rect_before_wrap();
            let y = self.ui.cursor().top();
            self.ui.painter().hline(
                r.left()..=r.right(),
                y,
                Stroke::new(1.0, self.pal.border.gamma_multiply(0.7)),
            );
        }
        self.n += 1;
    }

    /// One setting: `label` and `desc` on the left, the control on the
    /// right. The control's ui is laid out right to left: add the
    /// rightmost control first.
    pub fn row<R>(&mut self, label: &str, desc: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
        self.separator();
        let pal = self.pal;
        let ui = &mut *self.ui;
        ui.add_space(10.0);
        let w = ui.available_width();
        let ctl_w = (w * 0.5).min(CONTROL_W);
        let r = ui
            .horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(w - ctl_w - 12.0, 0.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_width(w - ctl_w - 12.0);
                        ui.label(RichText::new(label).size(14.0).color(pal.text));
                        if !desc.is_empty() {
                            ui.label(RichText::new(desc).size(12.0).color(pal.text_weak));
                        }
                    },
                );
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 0.0),
                    Layout::right_to_left(Align::Center),
                    control,
                )
                .inner
            })
            .inner;
        ui.add_space(10.0);
        r
    }

    /// Free content in a row of its own (lists, notes).
    pub fn block<R>(&mut self, content: impl FnOnce(&mut Ui) -> R) -> R {
        self.separator();
        let ui = &mut *self.ui;
        ui.add_space(10.0);
        let r = content(ui);
        ui.add_space(10.0);
        r
    }
}

/// A toggle switch (accent when on). Toggle the value when it is clicked.
pub(crate) fn switch(ui: &mut Ui, on: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(42.0, 24.0), Sense::click());
    let pal = Palette::get(ui.visuals().dark_mode);
    let t = ui.ctx().animate_bool_responsive(resp.id, on);
    let off = pal.knob_track;
    let fill = lerp_color(off, pal.accent, t);
    let painter = ui.painter();
    painter.rect_filled(rect, 12.0, fill);
    if resp.hovered() {
        painter.rect_stroke(
            rect,
            12.0,
            Stroke::new(1.0, pal.accent_line),
            egui::StrokeKind::Inside,
        );
    }
    let r = rect.height() / 2.0 - 3.0;
    let x = egui::lerp((rect.left() + r + 3.0)..=(rect.right() - r - 3.0), t);
    painter.circle_filled(pos2(x, rect.center().y), r, Color32::WHITE);
    resp
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A segmented choice: the options side by side in a pill, the selected
/// one filled. Returns the option clicked (if any) and each option's
/// rectangle (left to right) for the UI map. Lays out left to right
/// inside the caller's ui.
pub(crate) fn segmented(
    ui: &mut Ui,
    options: &[&str],
    selected: Option<usize>,
) -> (Option<usize>, Vec<Rect>) {
    let pal = Palette::get(ui.visuals().dark_mode);
    let font = theme::prop(13.0);
    let widths: Vec<f32> = options
        .iter()
        .map(|o| {
            ui.painter()
                .layout_no_wrap((*o).to_string(), font.clone(), pal.text)
                .size()
                .x
                + 26.0
        })
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (outer, _) = ui.allocate_exact_size(vec2(total, 28.0), Sense::hover());
    ui.painter().rect_filled(outer, 8.0, pal.well);
    let mut clicked = None;
    let mut rects = Vec::new();
    let mut x = outer.left() + 2.0;
    for (i, (o, w)) in options.iter().zip(widths).enumerate() {
        let r = Rect::from_min_size(pos2(x, outer.top() + 2.0), vec2(w, outer.height() - 4.0));
        let resp = ui.interact(r, ui.scope_id().with(("segment", o, i)), Sense::click());
        let on = selected == Some(i);
        if on {
            ui.painter().rect_filled(r, 6.0, pal.accent);
        } else if resp.hovered() {
            ui.painter()
                .rect_filled(r, 6.0, pal.accent.gamma_multiply(0.18));
        }
        ui.painter().text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            *o,
            font.clone(),
            if on { Color32::WHITE } else { pal.text },
        );
        if resp.clicked() {
            clicked = Some(i);
        }
        rects.push(r);
        x = r.right();
    }
    (clicked, rects)
}

/// A button sized to its label: `primary` is filled with `color` (the
/// accent unless given), otherwise a quiet outlined button.
pub(crate) fn button(ui: &mut Ui, label: &str, primary: bool, color: Option<Color32>) -> Response {
    let pal = Palette::get(ui.visuals().dark_mode);
    let font = theme::prop(13.5);
    let w = ui
        .painter()
        .layout_no_wrap(label.to_string(), font.clone(), pal.text)
        .size()
        .x
        + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w.max(72.0), 30.0), Sense::click());
    let fill = color.unwrap_or(pal.accent);
    let painter = ui.painter();
    if primary {
        let f = if resp.hovered() {
            fill.gamma_multiply(1.15)
        } else {
            fill
        };
        painter.rect_filled(rect, 8.0, f);
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font,
            Color32::WHITE,
        );
    } else {
        let bg = if resp.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };
        painter.rect(
            rect,
            8.0,
            bg,
            Stroke::new(1.0, pal.border),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font,
            pal.text,
        );
    }
    resp
}

/// A slim level bar (0..1) for meters in the settings.
pub(crate) fn level_bar(ui: &mut Ui, level: f32) -> Response {
    let pal = Palette::get(ui.visuals().dark_mode);
    let (rect, resp) = ui.allocate_exact_size(vec2(150.0, 6.0), Sense::hover());
    ui.painter().rect_filled(rect, 3.0, pal.well);
    let w = rect.width() * level.clamp(0.0, 1.0);
    if w > 0.5 {
        ui.painter().rect_filled(
            Rect::from_min_size(rect.min, vec2(w, rect.height())),
            3.0,
            if level >= 0.99 { pal.error } else { pal.play },
        );
    }
    resp
}
