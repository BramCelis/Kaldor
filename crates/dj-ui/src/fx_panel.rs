//! The FX as on the NS4FX: six effects (HPF, LPF,
//! Flanger, Echo, Reverb, Phaser), one chosen at a time for both sides;
//! the left side (FX unit 1) plays it on decks 1 and 3, the right side
//! (unit 2) on decks 2 and 4. WET/DRY is how much of it is heard (for HPF
//! and LPF: how far the filter closes), BEATS its timing, TAP the tempo.
//! SHIFT + a button chooses the color FX the channel filter knobs play
//! ([`COLOR`], DJM style).
//!
//! The controller uses the commands here and reads the chosen effect back
//! from the engine ([`lit`]). The FX display between the decks (FX in the
//! top bar) only shows what is on; the NS4FX operates it.

use crate::app::KaldorApp;
use crate::theme::{self, Palette};
use dj_audio::{ColorFx, EffectKind, FxCommand, FxMode, FxParams, UnitParams};
use egui::{Align2, Color32, FontId, Rect, Stroke, pos2, vec2};

/// The six effects, left to right and top to bottom as printed on the
/// NS4FX. HPF and LPF are the filter effect, high-pass and low-pass.
pub const BUTTONS: [EffectKind; 6] = [
    EffectKind::Filter,
    EffectKind::Filter,
    EffectKind::Flanger,
    EffectKind::Echo,
    EffectKind::Reverb,
    EffectKind::Phaser,
];

/// Their names on the display.
const NAMES: [&str; 6] = ["HPF", "LPF", "FLANGER", "ECHO", "REVERB", "PHASER"];

/// SHIFT + each button: the color FX on the filter knobs, as on the DJMs.
/// HPF is the plain filter.
pub const COLOR: [ColorFx; 6] = [
    ColorFx::Filter,
    ColorFx::Noise,
    ColorFx::Crush,
    ColorFx::DubEcho,
    ColorFx::Space,
    ColorFx::Sweep,
];

/// The color FX after SHIFT + button `k` with `now` chosen: that one, or
/// the plain filter when it was already chosen.
pub fn color_for(k: usize, now: ColorFx) -> ColorFx {
    let c = COLOR.get(k).copied().unwrap_or(ColorFx::Filter);
    if c == now { ColorFx::Filter } else { c }
}

/// The filter's LP / HP knob (0: low-pass, 1: high-pass) for HPF and LPF.
pub fn button_filter(k: usize) -> Option<f32> {
    match k {
        0 => Some(1.0),
        1 => Some(0.0),
        _ => None,
    }
}

/// The chosen effect (0-5): the one effect unit 1 plays (SINGLE, slot 1),
/// if it is one of the six.
pub fn lit(fx: &FxParams) -> Option<usize> {
    let p = &fx.units[0];
    let s = p.slots[0];
    if p.mode != FxMode::Single || !s.on {
        return None;
    }
    match s.kind {
        EffectKind::Filter if s.params[1] >= 0.5 => Some(0),
        EffectKind::Filter => Some(1),
        k => BUTTONS.iter().position(|b| *b == k),
    }
}

/// Unit `u` plays effect `pick` (one effect, slot 1, with its own
/// settings) or nothing, WET/DRY at `wet`.
pub fn pick_commands(u: usize, pick: Option<usize>, wet: f32, out: &mut Vec<FxCommand>) {
    let Some(k) = pick.filter(|k| *k < BUTTONS.len()) else {
        out.push(FxCommand::SlotOn(u, 0, false));
        return;
    };
    out.push(FxCommand::Mode(u, FxMode::Single));
    out.push(FxCommand::SlotKind(u, 0, BUTTONS[k]));
    if let Some(hp) = button_filter(k) {
        out.push(FxCommand::SlotParam(u, 0, 1, hp));
    }
    out.push(FxCommand::SlotOn(u, 0, true));
    wet_commands(u, pick, wet, out);
}

/// WET/DRY for unit `u`: the wet/dry mix, or for HPF and LPF the filter's
/// cutoff (0: open, nothing filtered; 1: LPF 100 Hz, HPF 10 kHz) with the
/// filter fully in, as a filter knob sweeps.
pub fn wet_commands(u: usize, pick: Option<usize>, wet: f32, out: &mut Vec<FxCommand>) {
    if pick.and_then(button_filter).is_some() {
        out.push(FxCommand::SlotParam(u, 0, 0, wet));
        out.push(FxCommand::Mix(u, 1.0));
    } else {
        out.push(FxCommand::Mix(u, wet));
    }
}

/// Side `u` (0: decks 1 and 3, 1: decks 2 and 4) on or off: the unit on
/// its own side's decks, as the paddle.
pub fn side_commands(u: usize, on: bool, out: &mut Vec<FxCommand>) {
    for t in 0..dj_audio::FX_TARGETS {
        out.push(FxCommand::Assign(
            u,
            t,
            t < dj_core::NUM_DECKS && t % 2 == u,
        ));
    }
    out.push(FxCommand::UnitOn(u, on));
}

/// Where WET/DRY of unit `p` stands (the filter's cutoff for HPF / LPF).
pub fn wet_of(p: &UnitParams, pick: Option<usize>) -> f32 {
    if pick.and_then(button_filter).is_some() {
        p.slots[0].params[0]
    } else {
        p.mix
    }
}

/// What the display shows.
struct View {
    name: Option<&'static str>,
    sides: [bool; 2],
    beats: &'static str,
    wet: f32,
    color: ColorFx,
    tap: Option<f64>,
}

impl View {
    fn of(app: &KaldorApp) -> Self {
        let snap = app.engine.snapshot();
        let fx = snap.fx.params;
        let chosen = lit(&fx);
        Self {
            name: chosen.map(|k| NAMES[k]),
            sides: [fx.units[0].on, fx.units[1].on],
            beats: dj_audio::beats_label(fx.units[0].beats),
            wet: wet_of(&fx.units[0], chosen),
            color: snap.mixer.color_fx,
            tap: fx.tap_bpm,
        }
    }

    /// The effect is heard somewhere.
    fn live(&self) -> bool {
        self.name.is_some() && self.sides.iter().any(|s| *s)
    }
}

/// Small capitals label.
fn label(ui: &egui::Ui, at: egui::Pos2, align: Align2, text: &str, color: Color32) -> Rect {
    ui.painter().text(at, align, text, theme::prop(10.0), color)
}

/// A side chip ("1·3"): filled while that side plays the effect.
fn chip(ui: &egui::Ui, r: Rect, text: &str, on: bool, pal: &Palette) {
    let p = ui.painter();
    if on {
        p.rect_filled(r, r.height() / 2.0, pal.play);
    } else {
        p.rect_stroke(
            r,
            r.height() / 2.0,
            Stroke::new(1.0, pal.border),
            egui::StrokeKind::Inside,
        );
    }
    p.text(
        r.center(),
        Align2::CENTER_CENTER,
        text,
        theme::prop(10.0),
        if on {
            Palette::on(pal.play)
        } else {
            pal.text_weak
        },
    );
}

/// The FX display in `rect` (display only; the NS4FX operates it): the
/// chosen effect, where it plays, its timing and amount, the color FX on
/// the filter knobs and the tapped tempo. A card when there is height for
/// it, else one line.
pub(crate) fn fx_panel(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let v = View::of(app);
    ui.painter().rect(
        rect,
        6.0,
        pal.raised,
        Stroke::new(
            1.0,
            if v.live() {
                pal.accent_line
            } else {
                pal.border
            },
        ),
        egui::StrokeKind::Inside,
    );
    app.map.enter(crate::uimap::Region::Fx, rect);
    app.map.add(ui, "fx_panel", None, rect);
    let name_font = |size| theme::logo_font(ui.ctx(), size);
    let name_color = if v.live() {
        pal.accent_line
    } else if v.name.is_some() {
        pal.text
    } else {
        pal.text_weak
    };
    let color_color = if v.color == ColorFx::Filter {
        pal.text_weak
    } else {
        pal.cue
    };
    if rect.height() >= 110.0 {
        card(
            app,
            ui,
            rect,
            &v,
            &pal,
            name_font(17.0),
            name_color,
            color_color,
        );
    } else {
        line(
            app,
            ui,
            rect,
            &v,
            &pal,
            name_font(13.0),
            name_color,
            color_color,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn card(
    app: &mut KaldorApp,
    ui: &egui::Ui,
    rect: Rect,
    v: &View,
    pal: &Palette,
    name_font: FontId,
    name_color: Color32,
    color_color: Color32,
) {
    let inner = rect.shrink2(vec2(12.0, 10.0));
    let p = ui.painter();
    // Content height (see below), centred in the panel.
    let h = 14.0 + 8.0 + 22.0 + 6.0 + 14.0 + 5.0 + 3.0 + 10.0 + 1.0 + 9.0 + 14.0;
    let mut y = inner.top() + ((inner.height() - h) / 2.0).max(0.0);
    let (x0, x1) = (inner.left(), inner.right());

    // FX, and where it plays.
    label(
        ui,
        pos2(x0, y + 7.0),
        Align2::LEFT_CENTER,
        "FX",
        pal.text_weak,
    );
    let cw = 30.0;
    for (u, text) in ["2·4", "1·3"].into_iter().enumerate() {
        let side = 1 - u;
        let r = Rect::from_min_size(pos2(x1 - cw - u as f32 * (cw + 4.0), y), vec2(cw, 14.0));
        chip(ui, r, text, v.sides[side], pal);
        app.map.add(ui, "fx_side", Some(side), r);
    }
    y += 14.0 + 8.0;

    // The effect.
    let r = p.text(
        pos2(x0, y + 11.0),
        Align2::LEFT_CENTER,
        v.name.unwrap_or("NO FX"),
        name_font,
        name_color,
    );
    app.map.add(ui, "fx_name", None, r);
    y += 22.0 + 6.0;

    // Timing and amount.
    let mono = theme::mono(11.0);
    p.text(
        pos2(x0, y + 7.0),
        Align2::LEFT_CENTER,
        format!("{} BEAT", v.beats),
        mono.clone(),
        pal.text,
    );
    p.text(
        pos2(x1, y + 7.0),
        Align2::RIGHT_CENTER,
        format!("{:.0} %", v.wet * 100.0),
        mono.clone(),
        pal.text,
    );
    y += 14.0 + 5.0;
    let bar = Rect::from_min_size(pos2(x0, y), vec2(x1 - x0, 3.0));
    p.rect_filled(bar, 1.5, pal.knob_track);
    p.rect_filled(
        Rect::from_min_size(bar.min, vec2(bar.width() * v.wet.clamp(0.0, 1.0), 3.0)),
        1.5,
        if v.live() {
            pal.accent_line
        } else {
            pal.text_weak
        },
    );
    app.map.add(ui, "fx_wet", None, bar);
    y += 3.0 + 10.0;
    p.line_segment([pos2(x0, y), pos2(x1, y)], Stroke::new(1.0, pal.border));
    y += 1.0 + 9.0;

    // The color FX on the filter knobs, and a tapped tempo.
    label(
        ui,
        pos2(x0, y + 7.0),
        Align2::LEFT_CENTER,
        "COLOR",
        pal.text_weak,
    );
    let r = p.text(
        pos2(x0 + 42.0, y + 7.0),
        Align2::LEFT_CENTER,
        v.color.name(),
        theme::logo_font(ui.ctx(), 10.5),
        color_color,
    );
    app.map.add(ui, "fx_color", None, r);
    if let Some(bpm) = v.tap {
        p.text(
            pos2(x1, y + 7.0),
            Align2::RIGHT_CENTER,
            format!("TAP {bpm:.1}"),
            mono,
            pal.cue,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn line(
    app: &mut KaldorApp,
    ui: &egui::Ui,
    rect: Rect,
    v: &View,
    pal: &Palette,
    name_font: FontId,
    name_color: Color32,
    color_color: Color32,
) {
    let inner = rect.shrink2(vec2(12.0, 0.0));
    let cy = rect.center().y;
    let p = ui.painter();
    let right = inner.right();
    let mut x = inner.left();
    let fits = |x: f32, w: f32| x + w <= right;

    let r = label(ui, pos2(x, cy), Align2::LEFT_CENTER, "FX", pal.text_weak);
    x = r.right() + 10.0;
    let r = p.text(
        pos2(x, cy),
        Align2::LEFT_CENTER,
        v.name.unwrap_or("NO FX"),
        name_font,
        name_color,
    );
    app.map.add(ui, "fx_name", None, r);
    x = r.right().max(x + 70.0) + 12.0;
    let cw = 30.0;
    if fits(x, 2.0 * cw + 4.0) {
        for (side, text) in ["1·3", "2·4"].into_iter().enumerate() {
            let r = Rect::from_center_size(
                pos2(x + cw / 2.0 + side as f32 * (cw + 4.0), cy),
                vec2(cw, 14.0),
            );
            chip(ui, r, text, v.sides[side], pal);
            app.map.add(ui, "fx_side", Some(side), r);
        }
        x += 2.0 * cw + 4.0 + 14.0;
    }
    let mono = theme::mono(11.0);
    let text = format!("{} BEAT  {:.0} %", v.beats, v.wet * 100.0);
    if fits(x, 110.0) {
        let r = p.text(pos2(x, cy), Align2::LEFT_CENTER, text, mono, pal.text);
        app.map.add(ui, "fx_wet", None, r);
        x = r.right() + 16.0;
    }
    if fits(x, 110.0) {
        let r = label(ui, pos2(x, cy), Align2::LEFT_CENTER, "COLOR", pal.text_weak);
        let r = p.text(
            pos2(r.right() + 6.0, cy),
            Align2::LEFT_CENTER,
            v.color.name(),
            theme::logo_font(ui.ctx(), 10.5),
            color_color,
        );
        app.map.add(ui, "fx_color", None, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chosen_effect_reads_back() {
        let mut p = FxParams::default();
        assert_eq!(lit(&p), None, "nothing chosen");
        for k in 0..6 {
            let mut out = Vec::new();
            pick_commands(0, Some(k), 0.5, &mut out);
            // As the engine applies them.
            let u = &mut p.units[0];
            for c in out {
                match c {
                    FxCommand::Mode(_, m) => u.mode = m,
                    FxCommand::SlotKind(_, s, kind) => {
                        u.slots[s] = dj_audio::SlotParams::new(kind);
                    }
                    FxCommand::SlotParam(_, s, i, v) => u.slots[s].params[i] = v,
                    FxCommand::SlotOn(_, s, on) => u.slots[s].on = on,
                    FxCommand::Mix(_, m) => u.mix = m,
                    _ => {}
                }
            }
            assert_eq!(lit(&p), Some(k), "{k}");
        }
        p.units[0].slots[0].on = false;
        assert_eq!(lit(&p), None, "deselected");
    }

    #[test]
    fn wet_dry_is_the_cutoff_on_the_filters() {
        let mut out = Vec::new();
        wet_commands(1, Some(0), 0.3, &mut out);
        assert_eq!(
            out,
            [FxCommand::SlotParam(1, 0, 0, 0.3), FxCommand::Mix(1, 1.0)]
        );
        out.clear();
        wet_commands(1, Some(3), 0.3, &mut out);
        assert_eq!(out, [FxCommand::Mix(1, 0.3)]);
    }

    #[test]
    fn shift_buttons_choose_the_djm_color_fx() {
        // Expected: HPF the standard filter, LPF noise, Flanger crush, Echo dub
        // echo, Reverb space, Phaser sweep.
        let want = [
            ColorFx::Filter,
            ColorFx::Noise,
            ColorFx::Crush,
            ColorFx::DubEcho,
            ColorFx::Space,
            ColorFx::Sweep,
        ];
        for (k, c) in want.into_iter().enumerate() {
            assert_eq!(color_for(k, ColorFx::Filter), c, "button {k}");
        }
        // The chosen one again: the plain filter.
        assert_eq!(color_for(3, ColorFx::DubEcho), ColorFx::Filter);
        assert_eq!(color_for(4, ColorFx::DubEcho), ColorFx::Space);
    }
}
