//! The top bar: clock, layout menu and deck count on the left, the logo in
//! the middle, REC, master volume, meters, CPU and the settings button on
//! the right.

use super::{AppEvent, KaldorApp, region_ui};
use crate::theme::Palette;
use crate::uimap::Region;
use dj_audio::Command;
use dj_core::config::DeckLayout;
use egui::{Align, Color32, Id, Layout, Rect, Sense, pos2, vec2};
use std::sync::Arc;

/// Top bar logo: height of the mark (points; nearly the bar's 34;
/// barely visible at 22), size of the name and the space between them.
const LOGO_MARK_H: f32 = 30.0;
const LOGO_TEXT: f32 = 20.0;
const LOGO_GAP: f32 = 8.0;

/// The name "Kaldor" in the logo font with its tracking.
fn logo_name(ui: &egui::Ui, color: Color32) -> Arc<egui::Galley> {
    let job = egui::text::LayoutJob::single_section(
        crate::APP_NAME.to_string(),
        egui::TextFormat {
            font_id: crate::theme::logo_font(ui.ctx(), LOGO_TEXT),
            extra_letter_spacing: LOGO_TEXT * crate::theme::LOGO_TRACKING,
            color,
            ..Default::default()
        },
    );
    ui.painter().layout_job(job)
}

impl KaldorApp {
    /// Top bar: on the left the clock, the layout menu and 2 / 4 decks (and
    /// a new status message for a few seconds); the Kaldor mark and name in
    /// the middle; to the right of it REC, the master volume, the master meter
    /// and the CPU meter, and the settings button at the right end. On
    /// narrow windows the right side moves to a second row.
    pub(super) fn top_bar(&mut self, ui: &mut egui::Ui, rect: Rect, two_rows: bool) {
        let pal = Palette::get(self.config.ui.dark_theme);
        ui.painter().rect_filled(rect, 0.0, pal.panel);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, pal.border),
        );
        self.map.enter(Region::TopBar, rect);
        let inner = rect.shrink2(vec2(10.0, 4.0));
        let (row1, row2) = if two_rows {
            let h = inner.height() / 2.0;
            (
                Rect::from_min_size(inner.min, vec2(inner.width(), h)),
                Rect::from_min_size(pos2(inner.left(), inner.top() + h), vec2(inner.width(), h)),
            )
        } else {
            (inner, inner)
        };
        // The mark and the name together centred; on two rows the right
        // end of the first row, as tall as the row allows.
        let mark_h = if two_rows {
            LOGO_MARK_H.min(row1.height())
        } else {
            LOGO_MARK_H.min(rect.height() - 2.0)
        };
        let logo_size = self.logo_size(ui, mark_h);
        let logo = if two_rows {
            Rect::from_min_size(
                pos2(
                    row1.right() - logo_size.x,
                    row1.center().y - logo_size.y / 2.0,
                ),
                logo_size,
            )
        } else {
            Rect::from_center_size(rect.center(), logo_size)
        };
        self.top_logo(ui, logo);
        let left = Rect::from_min_max(row1.min, pos2(logo.left() - 16.0, row1.bottom()));
        let mut left_ui = region_ui(ui, left, Layout::left_to_right(Align::Center), "top_left");
        self.top_left(&mut left_ui);
        let right = if two_rows {
            row2
        } else {
            Rect::from_min_max(pos2(logo.right() + 20.0, row1.top()), row1.max)
        };
        self.top_right(ui, right);
    }

    /// Size of the mark (`mark_h` high) and the name.
    fn logo_size(&self, ui: &egui::Ui, mark_h: f32) -> egui::Vec2 {
        let name_w =
            logo_name(ui, Color32::WHITE).size().x - LOGO_TEXT * crate::theme::LOGO_TRACKING;
        vec2(
            mark_h * crate::brand::mark_aspect() + LOGO_GAP + name_w,
            mark_h,
        )
    }

    /// The Kaldor mark (rendered from its SVG at the screen's pixel size,
    /// on whole pixels, so it is sharp) with the name in Archivo next to it.
    fn top_logo(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let dark = self.config.ui.dark_theme;
        let ink = Palette::get(dark).text;
        let ppp = ui.ctx().pixels_per_point();
        let h_px = (rect.height() * ppp).round().max(1.0);
        let w_px = (h_px * crate::brand::mark_aspect()).ceil();
        let min = pos2(
            (rect.left() * ppp).round() / ppp,
            (rect.top() * ppp).round() / ppp,
        );
        let mark = Rect::from_min_size(min, vec2(w_px / ppp, h_px / ppp));
        if let Some(tex) = crate::brand::mark_texture(ui.ctx(), dark, ink, h_px as u32) {
            ui.painter().image(
                tex.id(),
                mark,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let name = logo_name(ui, ink);
        let at = pos2(
            mark.right() + LOGO_GAP,
            rect.center().y - name.size().y / 2.0,
        );
        ui.painter().galley(at, name, ink);
        self.map.add(ui, "app_name", None, rect);
    }

    /// Clock, layout menu, 2 / 4 decks, FX, and a new status message.
    fn top_left(&mut self, ui: &mut egui::Ui) {
        let pal = Palette::get(self.config.ui.dark_theme);
        let (y, mo, d, h, mi, _) = crate::recorder::now_parts();
        let clock = ui
            .label(
                egui::RichText::new(if self.config.ui.clock_24h {
                    format!("{h:02}:{mi:02}")
                } else {
                    let (h12, ap) = (
                        if h % 12 == 0 { 12 } else { h % 12 },
                        if h < 12 { "am" } else { "pm" },
                    );
                    format!("{h12}:{mi:02} {ap}")
                })
                .monospace()
                .size(16.0)
                .color(pal.text),
            )
            .on_hover_text(format!("{y:04}-{mo:02}-{d:02}"));
        self.map.add(ui, "clock", None, clock.rect);
        ui.add_space(12.0);
        let (layout, two) = self.config.ui.deck_view();
        let name = |l: DeckLayout| match l {
            DeckLayout::Extended => "Extended",
            DeckLayout::FourGrid => "Grid",
            _ => "Stacked",
        };
        let mut pick = None;
        let combo = egui::ComboBox::from_id_salt("layout_menu")
            .selected_text(name(layout))
            .width(96.0)
            .show_ui(ui, |ui| {
                for l in [
                    DeckLayout::Extended,
                    DeckLayout::FourStacked,
                    DeckLayout::FourGrid,
                ] {
                    if ui.selectable_label(layout == l, name(l)).clicked() {
                        pick = Some(l);
                    }
                }
            });
        let combo = combo.response.on_hover_text(
            "Screen layout. Extended: deck panels on top, waveforms under them. \
             Stacked: waveforms on top of each other between the deck panels. \
             Grid: waveforms side by side.",
        );
        self.map.add(ui, "layout_menu", None, combo.rect);
        if let Some(l) = pick {
            self.config.ui.layout = l;
            self.config.ui.two_decks = two;
            self.events.push(AppEvent::ConfigChanged);
        }
        ui.add_space(6.0);
        for (label, want_two, id, hover) in [
            ("2", true, "decks_2", "Show 2 decks (the LAYER decks)"),
            ("4", false, "decks_4", "Show all 4 decks"),
        ] {
            let r = ui
                .selectable_label(
                    two == want_two,
                    egui::RichText::new(label).font(crate::theme::display(ui.ctx(), 11.5)),
                )
                .on_hover_text(hover);
            if r.clicked() && two != want_two {
                self.config.ui.layout = layout;
                self.config.ui.two_decks = want_two;
                self.events.push(AppEvent::ConfigChanged);
            }
            self.map.add(ui, id, None, r.rect);
        }
        // FX: the FX panel between the decks.
        ui.add_space(6.0);
        let open = self.config.ui.fx_panel;
        let r = ui
            .selectable_label(
                open,
                egui::RichText::new("FX").font(crate::theme::display(ui.ctx(), 11.5)),
            )
            .on_hover_text("Show / hide the FX between the decks");
        if r.clicked() {
            self.config.ui.fx_panel = !open;
            self.events.push(AppEvent::ConfigChanged);
        }
        self.map.add(ui, "fx_toggle", None, r.rect);
        // A USB export going on with its window closed (or its outcome).
        if let Some(lib) = self.library.as_mut()
            && let Some(r) = crate::usb_export::top_bar_pill(ui, &mut lib.usb, &pal)
        {
            self.map.add(ui, "usb_progress", None, r);
        }
        if let Some(msg) = self.fresh_status() {
            ui.add_space(14.0);
            let w = ui.available_width();
            if w > 40.0 {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 18.0), Sense::hover());
                crate::widgets::text_in(
                    &ui.painter_at(r),
                    r,
                    &msg,
                    crate::theme::prop(12.5),
                    pal.warn,
                    false,
                );
                resp.on_hover_text(msg);
                self.map.add(ui, "status_info", None, r);
            }
        }
    }

    /// The status message while it is new (10 s).
    fn fresh_status(&mut self) -> Option<String> {
        if self.status_line.is_empty() {
            return None;
        }
        let now = std::time::Instant::now();
        match &self.status_since {
            Some((m, _)) if *m == self.status_line => {}
            _ => self.status_since = Some((self.status_line.clone(), now)),
        }
        self.status_since
            .as_ref()
            .filter(|(_, t)| now.duration_since(*t) < std::time::Duration::from_secs(10))
            .map(|(m, _)| m.clone())
    }

    /// Right end of the top bar, right to left: settings, the CPU box, the
    /// master meter, the master volume knob and REC.
    fn top_right(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let snap = *self.engine.snapshot();
        let pal = Palette::get(self.config.ui.dark_theme);
        let cy = rect.center().y;
        let h = rect.height();
        // Settings (for now it opens the Setup tab).
        let gear = Rect::from_min_size(pos2(rect.right() - h - 4.0, rect.top()), vec2(h + 4.0, h));
        let b = crate::widgets::flat_button(
            ui,
            gear,
            Id::unique("top_settings"),
            "",
            crate::theme::prop(12.0),
            Some(crate::widgets::Icon::Gear),
            self.show_setup,
            pal.accent,
            false,
        )
        .on_hover_text("Settings (opens the Setup tab for now)");
        if b.clicked() {
            self.show_setup = !self.show_setup;
        }
        self.map.add(ui, "settings", None, gear);

        // CPU: a box with the load as a number, filled from the left as far
        // as the load (full box = the whole buffer period; the budget is a
        // quarter). Smoothed so it does not flicker.
        let cpu = Rect::from_min_size(pos2(gear.left() - 12.0 - 64.0, cy - 10.0), vec2(64.0, 20.0));
        if cpu.left() < rect.left() {
            return;
        }
        let (load, label) = self.cpu_display(snap.callback_load);
        ui.painter().rect_filled(cpu, 3.0, pal.well);
        let fill = if load < 0.25 { pal.cue } else { pal.error };
        let w = (cpu.width() * load.clamp(0.0, 1.0)).max(2.0);
        ui.painter().rect_filled(
            Rect::from_min_size(cpu.min, vec2(w, cpu.height())),
            3.0,
            fill.gamma_multiply(0.85),
        );
        ui.painter().rect_stroke(
            cpu,
            3.0,
            egui::Stroke::new(1.0, pal.border),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            cpu.center(),
            egui::Align2::CENTER_CENTER,
            label,
            crate::theme::mono(13.0),
            pal.text,
        );
        let mut hover = format!(
            "Audio CPU load {:.1} % (peak {:.1} %) of the buffer period; budget 25 %\n{} | {} | {} Hz | {} ch",
            snap.callback_load * 100.0,
            snap.callback_load_peak * 100.0,
            self.output_info.host,
            self.output_info.device,
            self.output_info.sample_rate,
            self.output_info.channels
        );
        if let Some(out) = &self.output {
            hover.push_str(&format!("\nxruns {}", out.xruns()));
            if let Some(l) = out.output_latency() {
                hover.push_str(&format!(", latency {:.1} ms", l.as_secs_f64() * 1000.0));
            }
        }
        let r = ui
            .interact(cpu, Id::unique("top_cpu"), Sense::hover())
            .on_hover_text(hover);
        self.map.add(ui, "cpu", None, r.rect);

        // Master meter (amber outline while the limiter works).
        let mr = Rect::from_min_size(pos2(cpu.left() - 16.0 - 96.0, cy - 6.0), vec2(96.0, 12.0));
        if mr.left() < rect.left() {
            return;
        }
        crate::widgets::paint_meter(ui.painter(), mr, snap.master_meter, pal.well);
        if snap.limiter_gain < 0.999 {
            ui.painter().rect_stroke(
                mr.expand(1.5),
                2.0,
                egui::Stroke::new(1.5, pal.warn),
                egui::StrokeKind::Outside,
            );
        }
        let r = ui
            .interact(mr, Id::unique("top_master_meter"), Sense::hover())
            .on_hover_text("Master level (amber outline: the limiter is working)");
        self.map.add(ui, "master_meter", None, r.rect);

        // Master volume, left of the meter.
        let kd = h.min(26.0);
        let kr = Rect::from_center_size(pos2(mr.left() - 8.0 - kd / 2.0, cy), vec2(kd, kd));
        if kr.left() < rect.left() {
            return;
        }
        // 12 o'clock is MASTER_CENTRE_DB (where it starts), not 0 dB.
        use crate::mixer_view::{MASTER_CENTRE_DB, master_db, master_pos};
        let gain = snap.mixer.master_gain_db;
        let (_, new) = crate::widgets::knob(
            ui,
            kr,
            Id::unique("top_master_volume"),
            master_pos(gain),
            0.0..=1.0,
            0.5,
            false,
            pal.accent,
            &format!(
                "Master volume {gain:.1} dB (drag up / down, Shift: fine; double-click: \
                 12 o'clock, {MASTER_CENTRE_DB:.0} dB)"
            ),
        );
        if let Some(v) = new {
            self.send(Command::Mixer(dj_audio::MixerCommand::MasterGain(
                master_db(v),
            )));
        }
        self.map.add(ui, "master_volume", None, kr);

        // REC, left of the master volume.
        self.top_record(ui, &snap, &pal, kr.left() - 12.0, rect.left(), cy);
    }

    /// The REC button ending at `right`: starts and stops the master
    /// recording (the settings are in Settings > Recording). Red with the
    /// recorded time while recording; an amber outline when frames were
    /// lost.
    fn top_record(
        &mut self,
        ui: &mut egui::Ui,
        snap: &dj_audio::EngineSnapshot,
        pal: &Palette,
        right: f32,
        min_left: f32,
        cy: f32,
    ) {
        let on = snap.recording;
        let w = if on { 92.0 } else { 60.0 };
        let r = Rect::from_min_size(pos2(right - w, cy - 12.0), vec2(w, 24.0));
        if r.left() < min_left {
            return;
        }
        let resp = ui.interact(r, Id::unique("top_record"), Sense::click());
        let red = pal.error;
        let fill = if on {
            red
        } else if resp.hovered() {
            ui.visuals().widgets.hovered.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };
        let edge = if on || resp.hovered() {
            red
        } else {
            pal.border
        };
        let painter = ui.painter();
        painter.rect(
            r,
            4.0,
            fill,
            egui::Stroke::new(1.0, edge),
            egui::StrokeKind::Inside,
        );
        if snap.record_dropped > 0 {
            painter.rect_stroke(
                r.expand(1.5),
                5.0,
                egui::Stroke::new(1.5, pal.warn),
                egui::StrokeKind::Outside,
            );
        }
        let fg = if on { Palette::on(red) } else { pal.text };
        let dot = pos2(r.left() + 14.0, cy);
        painter.circle_filled(dot, 4.5, if on { fg } else { red });
        let (text, font) = if on {
            let secs = snap.record_frames as f64 / f64::from(snap.sample_rate.max(1));
            (crate::record_view::clock(secs), crate::theme::mono(13.0))
        } else {
            ("REC".to_string(), crate::theme::display(ui.ctx(), 10.5))
        };
        painter.text(
            pos2(dot.x + 11.0, cy),
            egui::Align2::LEFT_CENTER,
            text,
            font,
            fg,
        );
        let mut hover = match (&self.rec.current, on) {
            (Some(p), true) => format!(
                "Recording the master output to {}\nClick to stop (Shift+R)",
                p.display()
            ),
            _ => "Record the master output (Shift+R). Format, folder and file name: \
                  Settings > Recording"
                .to_string(),
        };
        if snap.record_dropped > 0 {
            hover.push_str(&format!(
                "\n{} frames were lost: the disk did not keep up",
                snap.record_dropped
            ));
        }
        if !self.rec.status.is_empty() {
            hover.push('\n');
            hover.push_str(&self.rec.status);
        }
        let resp = resp.on_hover_text(hover);
        if resp.clicked() {
            self.toggle_recording();
        }
        self.map.add(ui, "top_record", None, r);
    }

    /// The CPU load for the top bar: smoothed over about 0.6 s, and the
    /// number changes at most 4 times a second, so it does not flicker.
    fn cpu_display(&mut self, load: f32) -> (f32, String) {
        let now = std::time::Instant::now();
        let (v, t) = self.cpu_smooth.unwrap_or((load, now));
        let dt = now.duration_since(t).as_secs_f32().min(1.0);
        let v = v + (load - v) * (1.0 - (-dt / 0.6).exp());
        self.cpu_smooth = Some((v, now));
        let stale = self
            .cpu_label
            .as_ref()
            .is_none_or(|(_, at)| now.duration_since(*at) >= std::time::Duration::from_millis(250));
        if stale {
            self.cpu_label = Some((format!("{:.0} %", v * 100.0), now));
        }
        let label = self
            .cpu_label
            .as_ref()
            .map_or_else(String::new, |(s, _)| s.clone());
        (v, label)
    }
}
