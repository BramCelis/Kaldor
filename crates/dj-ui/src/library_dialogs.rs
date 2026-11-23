//! Questions the library asks before it removes something: "Remove from
//! the library?" (Delete in All tracks, or the right-click menu) and
//! "Reset the library?" (Settings > Library). The files on disk are never
//! touched.

use crate::app::{AppEvent, KaldorApp};
use crate::settings_ui::button;
use crate::theme::Palette;
use dj_library::TrackId;
use egui::{Color32, RichText};

fn frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(if pal.dark { pal.raised } else { pal.bg })
        .stroke(egui::Stroke::new(1.0, pal.border))
        .corner_radius(14.0)
        .inner_margin(egui::Margin::same(22))
}

impl KaldorApp {
    /// Tracks that left the library: the decks that have one loaded keep
    /// playing it but no longer save cues or grid edits to it.
    fn forget_tracks(&mut self, ids: Option<&[TrackId]>) {
        for d in self.decks.iter_mut() {
            if d.track_id
                .is_some_and(|t| ids.is_none_or(|ids| ids.contains(&t)))
            {
                d.track_id = None;
            }
        }
    }

    /// Removes the tracks waiting in "Remove from library?" (tests and the
    /// dialog's button).
    pub fn confirm_remove_now(&mut self) {
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        let Some(ids) = lib.confirm_remove.take() else {
            return;
        };
        match lib.remove_from_library(&ids) {
            Ok(msg) => {
                lib.status = msg;
                self.forget_tracks(Some(&ids));
            }
            Err(e) => lib.status = e,
        }
    }

    /// Resets the library as "Reset the library?" asks (tests and the
    /// dialog's button).
    pub fn confirm_reset_now(&mut self) {
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        let Some(forget_watch) = lib.confirm_reset.take() else {
            return;
        };
        match lib.reset_library() {
            Ok(msg) => {
                lib.status = msg;
                self.forget_tracks(None);
                if forget_watch && !self.config.library.watch_folders.is_empty() {
                    self.config.library.watch_folders.clear();
                    self.events.push(AppEvent::ConfigChanged);
                }
            }
            Err(e) => lib.status = e,
        }
    }
}

/// Both questions, when one is waiting (a modal over the app).
pub(crate) fn dialogs(app: &mut KaldorApp, ctx: &egui::Context) {
    remove_dialog(app, ctx);
    reset_dialog(app, ctx);
}

fn remove_dialog(app: &mut KaldorApp, ctx: &egui::Context) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let Some(lib) = app.library.as_ref() else {
        return;
    };
    let Some(ids) = lib.confirm_remove.clone() else {
        return;
    };
    let title = if ids.len() == 1 {
        let name = lib
            .track(ids[0])
            .map(|t| t.display_name())
            .unwrap_or_default();
        format!("Remove \u{201c}{name}\u{201d} from the library?")
    } else {
        format!("Remove {} tracks from the library?", ids.len())
    };
    let mut yes = false;
    let mut no = false;
    let mut marks = Vec::new();
    egui::Modal::new(egui::Id::unique("remove_from_library"))
        .backdrop_color(Color32::from_black_alpha(160))
        .frame(frame(&pal))
        .show(ctx, |ui| {
            ui.set_max_width(440.0);
            ui.label(RichText::new(title).size(17.0).color(pal.text));
            ui.add_space(6.0);
            ui.label(
                RichText::new(
                    "They are taken out of every crate, with their cues, loops and \
                     beatgrids. The files stay on your computer: import them again to \
                     bring them back.",
                )
                .size(13.0)
                .color(pal.text_weak),
            );
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let b = button(ui, "Remove", true, Some(pal.error));
                marks.push(("remove_confirm", b.rect, ui.clip_rect()));
                yes = b.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter));
                let c = button(ui, "Cancel", false, None);
                marks.push(("remove_cancel", c.rect, ui.clip_rect()));
                no = c.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
    for (n, r, c) in marks {
        app.map.add_rect(n, r, c);
    }
    if yes {
        app.confirm_remove_now();
    } else if no && let Some(lib) = app.library.as_mut() {
        lib.confirm_remove = None;
    }
}

fn reset_dialog(app: &mut KaldorApp, ctx: &egui::Context) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let watch = app.config.library.watch_folders.clone();
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    let Some(mut forget_watch) = lib.confirm_reset else {
        return;
    };
    let tracks = lib.track_count();
    let crates = lib.crate_list().len();
    let file = lib.lib.file_path();
    let mut yes = false;
    let mut no = false;
    let mut marks = Vec::new();
    egui::Modal::new(egui::Id::unique("reset_library"))
        .backdrop_color(Color32::from_black_alpha(160))
        .frame(frame(&pal))
        .show(ctx, |ui| {
            ui.set_max_width(480.0);
            ui.label(
                RichText::new("Reset the library?")
                    .size(17.0)
                    .color(pal.text),
            );
            ui.add_space(6.0);
            let mut text = format!(
                "Removes all {tracks} tracks and {crates} crates from Kaldor's library, \
                 with their cues, loops and beatgrids. The files stay on your computer."
            );
            if let Some(f) = &file {
                text.push_str(&format!(
                    " A backup of the library is made first, in {}.",
                    f.parent()
                        .map(|d| d.join("backups").display().to_string())
                        .unwrap_or_default()
                ));
            }
            ui.label(RichText::new(text).size(13.0).color(pal.text_weak));
            if !watch.is_empty() {
                ui.add_space(8.0);
                let r = ui.checkbox(
                    &mut forget_watch,
                    "Also forget the watch folders (otherwise they are imported again at the next start):",
                );
                marks.push(("reset_watch", r.rect, ui.clip_rect()));
                for w in &watch {
                    ui.label(
                        RichText::new(format!("    {}", w.display()))
                            .size(12.0)
                            .color(pal.text_weak),
                    );
                }
            }
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                let b = button(ui, "Reset library", true, Some(pal.error));
                marks.push(("reset_confirm", b.rect, ui.clip_rect()));
                yes = b.clicked();
                let c = button(ui, "Cancel", false, None);
                marks.push(("reset_cancel", c.rect, ui.clip_rect()));
                no = c.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
    for (n, r, c) in marks {
        app.map.add_rect(n, r, c);
    }
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    if no {
        lib.confirm_reset = None;
        return;
    }
    lib.confirm_reset = Some(forget_watch);
    if yes {
        app.confirm_reset_now();
    }
}
