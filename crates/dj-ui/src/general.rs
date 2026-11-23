//! General settings that act on the whole app: the UI scale and asking
//! before closing while a deck plays (or a USB export writes a stick).

use crate::app::KaldorApp;
use crate::settings_ui::button;
use crate::theme::Palette;
use egui::{Color32, RichText};

impl KaldorApp {
    /// The UI scale from the settings (0: the display's own scaling). Only
    /// applied when the setting changes, so egui's own Ctrl +/- zoom still
    /// works until then.
    pub(crate) fn apply_ui_scale(&mut self, ctx: &egui::Context) {
        let want = self.config.ui.ui_scale;
        if self.ui_scale_applied == Some(want) {
            return;
        }
        self.ui_scale_applied = Some(want);
        let zoom = if want > 0.0 {
            want.clamp(0.75, 2.0)
        } else {
            1.0
        };
        if (ctx.zoom_factor() - zoom).abs() > 1e-3 {
            ctx.set_zoom_factor(zoom);
        }
    }

    /// Closing while a deck plays (when set) or while a USB export writes
    /// the stick (always): cancel and ask first.
    pub(crate) fn close_guard(&mut self, ctx: &egui::Context) {
        if !ctx.input(|i| i.viewport().close_requested()) || self.close_ok {
            return;
        }
        if self.close_needs_asking() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.close_ask = true;
        }
    }

    /// A USB export is writing the stick.
    fn usb_exporting(&self) -> bool {
        self.library.as_ref().is_some_and(|l| l.usb.exporting())
    }

    fn close_needs_asking(&self) -> bool {
        let playing = self.engine.snapshot().decks.iter().any(|d| d.playing);
        (self.config.ui.confirm_close && playing) || self.usb_exporting()
    }

    /// The question "a deck is playing: close?" while it is open.
    pub(crate) fn close_question(&mut self, ctx: &egui::Context) {
        if !self.close_ask {
            return;
        }
        let pal = Palette::get(self.config.ui.dark_theme);
        let frame = egui::Frame::new()
            .fill(if pal.dark { pal.raised } else { pal.bg })
            .stroke(egui::Stroke::new(1.0, pal.border))
            .corner_radius(14.0)
            .inner_margin(egui::Margin::same(24));
        let mut close = false;
        let mut keep = false;
        // The export is the bigger loss: it is named first.
        let (title, text, stay) = if self.usb_exporting() {
            (
                "A USB export is still running",
                "Close Kaldor anyway? The export stops and the stick may be left half-written.",
                "Keep exporting",
            )
        } else {
            (
                "A deck is still playing",
                "Close Kaldor anyway? The music stops.",
                "Keep playing",
            )
        };
        egui::Modal::new(egui::Id::unique("close_question"))
            .backdrop_color(Color32::from_black_alpha(170))
            .frame(frame)
            .show(ctx, |ui| {
                ui.set_max_width(380.0);
                ui.label(RichText::new(title).size(18.0).color(pal.text));
                ui.add_space(6.0);
                ui.label(RichText::new(text).size(13.0).color(pal.text_weak));
                ui.add_space(16.0);
                ui.horizontal(|ui| {
                    let b = button(ui, "Close Kaldor", true, Some(pal.error));
                    close = b.clicked();
                    self.map.add(ui, "close_confirm", None, b.rect);
                    let k = button(ui, stay, false, None);
                    keep = k.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
                    self.map.add(ui, "close_cancel", None, k.rect);
                });
            });
        if close {
            self.close_ask = false;
            self.close_ok = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if keep {
            self.close_ask = false;
        }
    }

    /// Asks to close as the window's close button does (tests).
    pub fn request_close_for_test(&mut self) {
        if self.close_needs_asking() && !self.close_ok {
            self.close_ask = true;
        }
    }

    /// The close question is shown.
    pub fn close_asked(&self) -> bool {
        self.close_ask
    }
}
