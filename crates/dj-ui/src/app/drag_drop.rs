//! Dragging library tracks and files: where a drop would land, the outline
//! and the label next to the pointer, and files dropped from the OS.

use super::{DropTarget, KaldorApp};
use crate::theme::Palette;
use dj_core::DeckId;
use egui::{Color32, Id, Stroke};
use std::path::PathBuf;

impl KaldorApp {
    /// What the drop outline marked in the last frame (for tests).
    pub fn drop_highlight(&self) -> Option<DropTarget> {
        self.drop_highlight
    }

    /// Where a dragged library track or file from the OS would land right
    /// now: a sampler slot, or a deck. `None` when nothing is dragged.
    pub fn drop_target_now(&self, ctx: &egui::Context) -> Option<DropTarget> {
        let library = egui::DragAndDrop::has_payload_of_type::<crate::library_view::TrackDrag>(ctx);
        let panel = egui::DragAndDrop::has_payload_of_type::<crate::files_panel::FileDrag>(ctx);
        let files = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !library && !files && !panel {
            return None;
        }
        // During a drag from the file manager egui's pointer is stale on
        // Windows; the OS cursor is not (as when the file is dropped).
        let pos = if files {
            crate::os_cursor::cursor_in_window(ctx)
                .or_else(|| ctx.input(|i| i.pointer.latest_pos()))
        } else {
            ctx.input(|i| i.pointer.hover_pos())
        }?;
        if let Some((s, _)) = self.sample_drop_rects().find(|(_, r)| r.contains(pos)) {
            return Some(DropTarget::Sample(s));
        }
        if let Some((d, _)) = self.drop_rects.iter().find(|(_, r)| r.contains(pos)) {
            return Some(DropTarget::Deck(*d));
        }
        let lib = self.library.as_ref()?;
        if library {
            // The crate list found the crate under the pointer this frame;
            // under the crates: a new crate with the track(s).
            if let Some(c) = lib.crate_drop {
                return Some(DropTarget::Crate(c.id));
            }
            return lib
                .drop_areas
                .new_crate
                .filter(|r| r.contains(pos))
                .map(|_| DropTarget::NewCrate);
        }
        // Files (Files panel or file manager): a crate, a new crate, or the
        // library (All tracks; the list: the crate it shows).
        use crate::library::ImportTarget;
        lib.drop_areas
            .target_at(pos, lib.selection)
            .map(|t| match t {
                ImportTarget::Crate(c) => DropTarget::Crate(c),
                ImportTarget::NewCrate(_) => DropTarget::NewCrate,
                ImportTarget::Library => DropTarget::Library,
            })
    }

    /// Where a file or library track can be dropped into a sampler slot:
    /// the SAMPLER tab's slots when it is shown, and the pads in SAMPLE
    /// mode.
    fn sample_drop_rects(&self) -> impl Iterator<Item = (usize, egui::Rect)> + '_ {
        let tab = self.sampler_shown();
        self.sampler
            .drop_rects
            .iter()
            .filter(move |_| tab)
            .chain(&self.pad_sample_drops)
            .copied()
    }

    /// Outlines the deck (its panel and its waveform strip) or sampler slot
    /// a dragged track or file would land on. Drawn last, on a top
    /// layer: drawn with the panels, their backgrounds covered it.
    pub(super) fn paint_drop_highlight(&mut self, ui: &egui::Ui) {
        let ctx = ui.ctx();
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) {
            // No pointer events arrive during an OS drag: keep redrawing so
            // the outline follows the cursor.
            ctx.request_repaint();
        }
        self.drop_highlight = self.drop_target_now(ctx);
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::unique("drop_highlight"),
        ));
        let pal = Palette::get(self.config.ui.dark_theme);
        self.paint_drag_label(ctx, &painter, pal);
        let Some(target) = self.drop_highlight else {
            return;
        };
        let (rects, color, radius): (Vec<egui::Rect>, Color32, f32) = match target {
            DropTarget::Deck(d) => (
                self.drop_rects
                    .iter()
                    .filter(|(k, _)| *k == d)
                    .map(|(_, r)| *r)
                    .collect(),
                crate::theme::deck_color(DeckId::ALL[d]),
                6.0,
            ),
            DropTarget::Sample(s) => (
                self.sample_drop_rects()
                    .filter(|(k, _)| *k == s)
                    .map(|(_, r)| r)
                    .collect(),
                pal.accent,
                6.0,
            ),
            DropTarget::Crate(_) | DropTarget::NewCrate | DropTarget::Library => {
                let lib = self.library.as_ref();
                let drop = lib.and_then(|l| l.crate_drop);
                // Grey when the drop would change nothing.
                let blocked = drop.is_some_and(|d| d.already) || lib.is_some_and(|l| l.protect);
                // The area under the pointer: the crate row, the space under
                // the crates, All tracks or the track list.
                let pos = ctx
                    .input(|i| i.pointer.hover_pos())
                    .or_else(|| crate::os_cursor::cursor_in_window(ctx));
                let rect = lib.and_then(|l| {
                    let a = &l.drop_areas;
                    let p = pos?;
                    a.crates
                        .iter()
                        .map(|(_, r)| *r)
                        .chain(a.new_crate)
                        .chain(a.all_tracks)
                        .chain(a.list)
                        .find(|r| r.contains(p))
                });
                (
                    rect.into_iter().collect(),
                    if blocked {
                        pal.text_weak
                    } else {
                        pal.accent_line
                    },
                    4.0,
                )
            }
        };
        let thin = matches!(
            target,
            DropTarget::Crate(_) | DropTarget::NewCrate | DropTarget::Library
        );
        for r in rects {
            painter.rect_filled(r, radius, color.gamma_multiply(0.18));
            painter.rect_stroke(
                r,
                radius,
                Stroke::new(if thin { 3.5 } else { 5.0 }, Color32::from_black_alpha(160)),
                egui::StrokeKind::Inside,
            );
            painter.rect_stroke(
                r,
                radius,
                Stroke::new(if thin { 2.0 } else { 3.0 }, color),
                egui::StrokeKind::Inside,
            );
        }
    }

    /// Next to the pointer while a library track is dragged: the track's
    /// name and what a drop there does ("Add to Warm-up", "Load on deck
    /// 2", ...).
    fn paint_drag_label(&self, ctx: &egui::Context, painter: &egui::Painter, pal: Palette) {
        if let Some(fd) = egui::DragAndDrop::payload::<crate::files_panel::FileDrag>(ctx) {
            self.paint_file_label(ctx, painter, pal, &fd.0);
            return;
        }
        let Some(drag) = egui::DragAndDrop::payload::<crate::library_view::TrackDrag>(ctx) else {
            return;
        };
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let Some(lib) = self.library.as_ref() else {
            return;
        };
        let Some(track) = lib.track(drag.0) else {
            return;
        };
        let missing = lib.file_state(drag.0).is_missing();
        let (hint, hint_color) = match self.drop_highlight {
            Some(DropTarget::Deck(d)) if missing => (
                format!("File not found: cannot load on deck {}", d + 1),
                pal.error,
            ),
            Some(DropTarget::Deck(d)) => {
                let playing = self.engine.snapshot().decks[d].playing;
                if self.config.performance.lock_playing_deck && playing {
                    (format!("Deck {} is playing: locked", d + 1), pal.warn)
                } else {
                    (format!("Load on deck {}", d + 1), pal.text)
                }
            }
            Some(DropTarget::Sample(s)) => (format!("Load into sample slot {}", s + 1), pal.text),
            Some(DropTarget::Crate(c)) => {
                let name = lib
                    .crate_list()
                    .into_iter()
                    .find(|(id, _, _)| *id == c)
                    .map(|(_, n, _)| n)
                    .unwrap_or_default();
                if lib.protect {
                    ("Library is protected".into(), pal.warn)
                } else if lib.has_sub_crates(c) {
                    ("Folder: drop on one of its crates".into(), pal.text_weak)
                } else if lib.crate_drop.is_some_and(|d| d.already) {
                    (format!("Already in {name}"), pal.text_weak)
                } else {
                    (format!("Add to {name}"), pal.text)
                }
            }
            Some(DropTarget::NewCrate) if lib.protect => ("Library is protected".into(), pal.warn),
            Some(DropTarget::NewCrate) => ("New crate with this".into(), pal.text),
            Some(DropTarget::Library) => (String::new(), pal.text),
            // Over the track list: a new place in the crate, or why not.
            None => match &lib.list_hint {
                Some(h) if lib.list_drop.is_some() => (h.clone(), pal.text),
                Some(h) => (h.clone(), pal.text_weak),
                None => (String::new(), pal.text),
            },
        };
        // Several selected tracks travel together (crates); a deck takes
        // the one under the pointer.
        let count = if lib.picked.contains(&drag.0) {
            lib.picked.len()
        } else {
            1
        };
        let label = if count > 1
            && matches!(
                self.drop_highlight,
                Some(DropTarget::Crate(_) | DropTarget::NewCrate) | None
            ) {
            format!("{count} tracks")
        } else {
            track.display_name()
        };
        draw_chip(ctx, painter, pal, pos, label, hint, hint_color);
    }

    /// The label next to the pointer while files from the Files panel are
    /// dragged.
    fn paint_file_label(
        &self,
        ctx: &egui::Context,
        painter: &egui::Painter,
        pal: Palette,
        paths: &[PathBuf],
    ) {
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let name = |p: &PathBuf| {
            p.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.display().to_string())
        };
        let folders = paths.iter().filter(|p| p.is_dir()).count();
        let label = match paths {
            [one] => name(one),
            _ if folders == 0 => format!("{} files", paths.len()),
            _ => format!("{} items", paths.len()),
        };
        let lib = self.library.as_ref();
        let crate_name = |c: dj_library::CrateId| {
            lib.and_then(|l| {
                l.crate_list()
                    .into_iter()
                    .find(|(id, _, _)| *id == c)
                    .map(|(_, n, _)| n)
            })
            .unwrap_or_default()
        };
        let protect = lib.is_some_and(|l| l.protect);
        let (hint, color) = match self.drop_highlight {
            Some(DropTarget::Deck(d)) => {
                match paths.iter().find(|p| dj_library::is_audio_file(p)) {
                    Some(_) => (format!("Load on deck {}", d + 1), pal.text),
                    None => ("Folders load into crates, not decks".into(), pal.text_weak),
                }
            }
            Some(DropTarget::Sample(s)) => (format!("Load into sample slot {}", s + 1), pal.text),
            Some(DropTarget::Crate(_) | DropTarget::NewCrate) if protect => {
                ("Library is protected".into(), pal.warn)
            }
            Some(DropTarget::Crate(c)) => (format!("Add to {}", crate_name(c)), pal.text),
            Some(DropTarget::NewCrate) => {
                let n = crate::library::ImportTarget::NewCrate(String::new()).named(paths);
                let n = match n {
                    crate::library::ImportTarget::NewCrate(n) => n,
                    _ => String::new(),
                };
                (format!("New crate \u{201c}{n}\u{201d}"), pal.text)
            }
            Some(DropTarget::Library) => ("Add to the library".into(), pal.text),
            None => (String::new(), pal.text),
        };
        draw_chip(ctx, painter, pal, pos, label, hint, color);
    }
}

/// A small label next to the pointer: what is dragged and, under it, what
/// a drop there does.
fn draw_chip(
    ctx: &egui::Context,
    painter: &egui::Painter,
    pal: Palette,
    pos: egui::Pos2,
    label: String,
    hint: String,
    hint_color: Color32,
) {
    {
        let font = crate::theme::prop(12.5);
        let name = painter.layout_no_wrap(label, font.clone(), pal.text_weak);
        let hint_g = painter.layout_no_wrap(hint, font, hint_color);
        let pad = egui::vec2(8.0, 4.0);
        let w = name.size().x.max(hint_g.size().x).min(360.0) + pad.x * 2.0;
        let h = name.size().y
            + if hint_g.size().x > 0.0 {
                hint_g.size().y + 2.0
            } else {
                0.0
            }
            + pad.y * 2.0;
        let screen = ctx.content_rect();
        let mut at = pos + egui::vec2(16.0, 14.0);
        at.x = at.x.min(screen.right() - w - 4.0);
        at.y = at.y.min(screen.bottom() - h - 4.0);
        let r = egui::Rect::from_min_size(at, egui::vec2(w, h));
        painter.rect_filled(r, 4.0, pal.raised);
        painter.rect_stroke(
            r,
            4.0,
            Stroke::new(1.0, pal.border),
            egui::StrokeKind::Inside,
        );
        let clip = painter
            .clip_rect()
            .intersect(r.shrink2(egui::vec2(pad.x - 2.0, 0.0)));
        let p = painter.with_clip_rect(clip);
        let name_h = name.size().y;
        p.galley(r.min + pad, name, pal.text_weak);
        p.galley(
            r.min + pad + egui::vec2(0.0, name_h + 2.0),
            hint_g,
            hint_color,
        );
    }
}

impl KaldorApp {
    /// Files dropped from the OS load onto the deck under the pointer (or
    /// the first deck in the layout).
    pub(super) fn handle_dropped_files(&mut self, ui: &egui::Ui) {
        let (dropped, pos): (Vec<PathBuf>, Option<egui::Pos2>) = ui.ctx().input(|i| {
            (
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect(),
                i.pointer.latest_pos(),
            )
        });
        let Some(p) = dropped.first().cloned() else {
            return;
        };
        // The Locate window is open: the file (or folder) is for it.
        if self.locate_took_drop(&p) {
            return;
        }
        // Over the library (crates as in the Files panel): every dropped
        // file and folder goes into the crate, a new crate or the library.
        // A folder dropped anywhere else goes into the library.
        let at = crate::os_cursor::cursor_in_window(ui.ctx()).or(pos);
        if let Some(lib) = self.library.as_mut() {
            let target = at.and_then(|c| lib.drop_areas.target_at(c, lib.selection));
            let over_deck = at.is_some_and(|c| self.drop_rects.iter().any(|(_, r)| r.contains(c)));
            let target = match target {
                Some(t) => Some(t),
                None if p.is_dir() && !over_deck => Some(crate::library::ImportTarget::Library),
                None => None,
            };
            if let Some(t) = target {
                let t = t.named(&dropped);
                lib.import_into(&dropped, t);
                return;
            }
        }
        // The OS cursor first: egui's pointer is stale during a drag from
        // the file manager on Windows (see `os_cursor`).
        let deck_at = |pos: egui::Pos2| {
            self.drop_rects
                .iter()
                .find(|(_, r)| r.contains(pos))
                .map(|(d, _)| *d)
        };
        let cursor = crate::os_cursor::cursor_in_window(ui.ctx()).or(pos);
        // A sampler slot under the pointer takes the file.
        if let Some(slot) = cursor.and_then(|c| {
            self.sample_drop_rects()
                .find(|(_, r)| r.contains(c))
                .map(|(s, _)| s)
        }) {
            self.load_sample(slot, p, None, String::new());
            return;
        }
        let target = crate::os_cursor::cursor_in_window(ui.ctx())
            .and_then(deck_at)
            .or_else(|| pos.and_then(deck_at))
            .unwrap_or(self.layer[0]);
        self.load_path(DeckId::ALL[target], p);
    }
}
