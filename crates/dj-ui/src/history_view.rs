//! The History panel: over the library's top part, in the place of the
//! Files panel (one of the two at a time; the History button next to Files
//! opens it). Sessions on the left (one per app run with tracks played),
//! the tracks of the chosen one on the right in the order played, with the
//! time and deck. Played tracks drag onto a deck or a crate as from the
//! track list. A session can be saved as a playlist (M3U8 / PLS) or a CSV
//! file, or turned into a crate.

use crate::app::KaldorApp;
use crate::browse::{self, Pick};
use crate::library_view::TrackDrag;
use crate::theme::{self, Palette};
use crate::widgets::{self, flat_button};
use dj_core::DeckId;
use dj_library::TrackId;
use dj_library::history::{self, HistoryPlay, HistorySession, SessionId};
use dj_library::interop::playlist;
use egui::{Align, FontId, Id, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};

/// Browse ids of the save buttons.
const SAVE_PLAYLIST: &str = "history_playlist";
const SAVE_CSV: &str = "history_csv";

/// What the panel shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HistoryView {
    pub sessions: Vec<HistorySession>,
    pub selected: Option<SessionId>,
    pub plays: Vec<HistoryPlay>,
}

/// Seconds to add to UTC for the local time, from the clock the app shows
/// (rounded to 15 minutes).
pub fn local_offset() -> i64 {
    let (y, mo, d, h, mi, s) = crate::recorder::now_parts();
    let stamp = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}Z");
    let Some(local) = dj_library::interop::parse_date(&stamp) else {
        return 0;
    };
    let utc = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(local);
    ((local - utc) as f64 / 900.0).round() as i64 * 900
}

impl KaldorApp {
    /// Opens the History panel on the newest session (closes Files, which
    /// takes the same place).
    pub fn open_history(&mut self) {
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        let sessions = lib.lib.sessions().unwrap_or_default();
        let selected = sessions.first().map(|s| s.id);
        let plays = selected
            .and_then(|s| lib.lib.session_plays(s).ok())
            .unwrap_or_default();
        lib.history = Some(HistoryView {
            sessions,
            selected,
            plays,
        });
        self.files_panel.open = false;
    }

    /// The History panel is open.
    pub fn history_open(&self) -> bool {
        self.library.as_ref().is_some_and(|l| l.history.is_some())
    }

    /// Logs a track played on deck `deck` (0-3) in this run's session
    /// (started with the first play). Off when Settings > Library > Keep a
    /// history is off.
    pub(crate) fn history_add(&mut self, id: dj_library::TrackId, deck: usize) {
        if !self.config.library.history {
            return;
        }
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        let session = match self.lib_tasks.history_session {
            Some(s) => s,
            None => match lib.lib.start_session() {
                Ok(s) => {
                    self.lib_tasks.history_session = Some(s);
                    s
                }
                Err(e) => {
                    tracing::warn!("history: {e}");
                    return;
                }
            },
        };
        if let Err(e) = lib.lib.log_play(session, id, deck as u8 + 1) {
            tracing::warn!("history: {e}");
        }
    }

    /// The selected session as a crate "History <date>" (returns its name).
    pub fn history_to_crate(&mut self) -> Option<String> {
        let lib = self.library.as_mut()?;
        let h = lib.history.as_ref()?;
        let s = h.sessions.iter().find(|s| Some(s.id) == h.selected)?;
        let name = format!(
            "History {}",
            history::format_time(s.started, local_offset())
        );
        let ids: Vec<_> = h.plays.iter().filter_map(|p| p.track).collect();
        let r = lib
            .lib
            .create_crate(&name, None)
            .and_then(|c| lib.lib.add_to_crate(c, &ids).map(|()| c));
        match r {
            Ok(c) => {
                lib.reload();
                lib.crate_flash = Some((c, std::time::Instant::now()));
                lib.status = format!("Crate \u{201c}{name}\u{201d} made from the history");
                Some(name)
            }
            Err(e) => {
                lib.status = format!("{e}");
                None
            }
        }
    }
}

/// "2 h 05" or "35 min".
fn length(secs: i64) -> String {
    let m = secs.max(0) / 60;
    if m >= 60 {
        format!("{} h {:02}", m / 60, m % 60)
    } else {
        format!("{m} min")
    }
}

/// Writes the session chosen in a save dialog (every frame: the dialog
/// answers later, from its own thread).
pub(crate) fn take_saves(app: &mut KaldorApp) {
    let picked = [SAVE_PLAYLIST, SAVE_CSV].map(browse::take);
    if picked.iter().all(Option::is_none) {
        return;
    }
    let offset = local_offset();
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    let Some(h) = lib.history.as_ref() else {
        return;
    };
    let write = |path: &std::path::Path, csv: bool| {
        if csv {
            std::fs::write(path, history::to_csv(&h.plays, offset))
        } else {
            playlist::write(path, &history::to_entries(&h.plays))
        }
    };
    for (i, p) in picked.iter().enumerate() {
        if let Some(p) = p {
            lib.status = match write(p, i == 1) {
                Ok(()) => format!("History saved: {}", p.display()),
                Err(e) => format!("Could not save the history: {e}"),
            };
        }
    }
}

/// Something the panel asks for; applied after drawing.
enum HistoryAction {
    Pick(SessionId),
    SavePlaylist(String),
    SaveCsv(String),
    ToCrate,
    Close,
    LoadFree(TrackId),
    Load(DeckId, TrackId),
}

/// Widths of the TIME, DECK, BPM and KEY columns (TRACK takes the rest).
const TIME_W: f32 = 70.0;
const DECK_W: f32 = 46.0;
const BPM_W: f32 = 58.0;
const KEY_W: f32 = 48.0;
const ROW_H: f32 = 20.0;

/// The History panel in `rect` (over the library's top part), drawn like
/// the Files panel.
pub(crate) fn history_panel(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let notation = app.config.ui.key_notation;
    let two_decks = app.files_panel.two_decks;
    let offset = local_offset();
    let mut actions: Vec<HistoryAction> = Vec::new();
    let mut marks: Vec<(&'static str, Rect, bool)> = Vec::new();
    let Some(h) = app.library.as_ref().and_then(|l| l.history.clone()) else {
        return;
    };
    ui.painter().rect_filled(rect, 6.0, pal.raised);
    ui.painter().rect_stroke(
        rect,
        6.0,
        Stroke::new(1.0, pal.border),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(6.0);
    let head = Rect::from_min_size(inner.min, vec2(inner.width(), 24.0));
    let body = Rect::from_min_max(pos2(inner.left(), head.bottom() + 4.0), inner.max);
    marks.push(("history_panel", rect, false));

    // Header, right to left: close, then the session's buttons.
    let mut right = head.right();
    let mut button = |ui: &mut egui::Ui, id: &'static str, label: &str, tip: &str, w: f32| {
        let r = Rect::from_min_size(pos2(right - w, head.top()), vec2(w, head.height()));
        // A wider gap after the close button.
        right -= w + if id == "close" { 12.0 } else { 4.0 };
        let font = if id == "close" {
            theme::prop(14.0)
        } else {
            theme::prop(12.0)
        };
        let b = flat_button(
            ui,
            r,
            Id::unique(("history_btn", id)),
            label,
            font,
            None,
            false,
            pal.accent,
            false,
        )
        .on_hover_text(tip);
        (b.clicked(), r, right)
    };
    let (close, r, mut left_end) = button(ui, "close", "×", "Close History", 26.0);
    marks.push(("history_close", r, false));
    if close {
        actions.push(HistoryAction::Close);
    }
    let session = h.sessions.iter().find(|s| Some(s.id) == h.selected);
    // The buttons only while there is a session to save.
    if let Some(s) = session.filter(|_| !h.plays.is_empty()) {
        let name = format!(
            "Kaldor history {}",
            history::format_time(s.started, offset).replace(':', "-")
        );
        let busy = browse::busy();
        let (c, r, _) = button(
            ui,
            "crate",
            "Make a crate",
            "A crate with the session's tracks, in the order played",
            96.0,
        );
        marks.push(("history_crate", r, false));
        if c {
            actions.push(HistoryAction::ToCrate);
        }
        let (c, r, _) = button(
            ui,
            "csv",
            "Save as CSV…",
            "Saves the session as a CSV file (time, deck, artist, title, BPM, key)",
            100.0,
        );
        marks.push(("history_save_csv", r, false));
        if c && !busy {
            actions.push(HistoryAction::SaveCsv(name.clone()));
        }
        let (c, r, l) = button(
            ui,
            "playlist",
            "Save as playlist…",
            "Saves the session as an M3U8 or PLS playlist",
            124.0,
        );
        left_end = l;
        marks.push(("history_save_playlist", r, false));
        if c && !busy {
            actions.push(HistoryAction::SavePlaylist(name));
        }
    }
    // Left: the title and, when there is room, the hint.
    let text_r = Rect::from_min_max(head.min, pos2(left_end - 4.0, head.bottom()));
    let painter = ui.painter_at(text_r);
    let title = painter.layout_no_wrap("History".into(), theme::prop(13.5), pal.text);
    let title_w = title.size().x;
    painter.galley(
        pos2(
            text_r.left() + 4.0,
            text_r.center().y - title.size().y / 2.0,
        ),
        title,
        pal.text,
    );
    let hint = if two_decks {
        "Drag a track onto a deck or a crate  ·  2 decks while History is open"
    } else {
        "Drag a track onto a deck or a crate"
    };
    let hint_r = Rect::from_min_max(
        pos2(text_r.left() + title_w + 18.0, text_r.top()),
        text_r.max,
    );
    if hint_r.width() > 60.0 {
        widgets::text_in(
            &ui.painter_at(hint_r),
            hint_r,
            hint,
            theme::prop(11.5),
            pal.text_weak,
            false,
        );
    }

    // Left: the sessions, newest first.
    let tree_w = (body.width() * 0.3).clamp(190.0, 320.0);
    let tree_r = Rect::from_min_size(body.min, vec2(tree_w, body.height()));
    let list_r = Rect::from_min_max(pos2(tree_r.right() + 6.0, body.top()), body.max);
    ui.painter().line_segment(
        [
            pos2(tree_r.right() + 3.0, body.top()),
            pos2(tree_r.right() + 3.0, body.bottom()),
        ],
        Stroke::new(1.0, pal.border),
    );
    let mut tu = crate::app::region_ui(ui, tree_r, Layout::top_down(Align::Min), "history_tree");
    if h.sessions.is_empty() {
        tu.add_space(8.0);
        tu.label(RichText::new("Nothing played yet").color(pal.text_weak));
    }
    egui::ScrollArea::vertical()
        .id_salt("history_sessions")
        .auto_shrink([false, false])
        .show(&mut tu, |ui| {
            for s in &h.sessions {
                let (r, resp) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let painter = ui.painter_at(r);
                if h.selected == Some(s.id) {
                    painter.rect_filled(r, 3.0, ui.visuals().selection.bg_fill);
                } else if resp.hovered() {
                    painter.rect_filled(r, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
                }
                let info = format!("{} tracks \u{b7} {}", s.plays, length(s.ended - s.started));
                let info_g =
                    painter.layout_no_wrap(info, FontId::proportional(11.5), pal.text_weak);
                let info_w = info_g.size().x;
                painter.galley(
                    pos2(
                        r.right() - 6.0 - info_w,
                        r.center().y - info_g.size().y / 2.0,
                    ),
                    info_g,
                    pal.text_weak,
                );
                let date_r = Rect::from_min_max(
                    pos2(r.left() + 6.0, r.top()),
                    pos2(r.right() - info_w - 12.0, r.bottom()),
                );
                widgets::text_in(
                    &painter,
                    date_r,
                    &history::format_time(s.started, offset),
                    FontId::proportional(13.0),
                    pal.text,
                    false,
                );
                marks.push(("history_session", r, true));
                if resp.clicked() {
                    actions.push(HistoryAction::Pick(s.id));
                }
            }
        });

    // Right: the column names, then the session's tracks in the order
    // played.
    let cols_r = Rect::from_min_size(list_r.min, vec2(list_r.width(), 18.0));
    let rows_r = Rect::from_min_max(pos2(list_r.left(), cols_r.bottom() + 2.0), list_r.max);
    let columns = |r: Rect| {
        let track_w = (r.width() - TIME_W - DECK_W - BPM_W - KEY_W - 8.0).max(40.0);
        let mut x = r.left();
        [TIME_W, DECK_W, track_w, BPM_W, KEY_W].map(|w| {
            let c = Rect::from_min_size(pos2(x, r.top()), vec2(w, r.height()));
            x += w;
            c
        })
    };
    if !h.plays.is_empty() {
        let painter = ui.painter_at(cols_r);
        for (c, name) in columns(cols_r)
            .iter()
            .zip(["TIME", "DECK", "TRACK", "BPM", "KEY"])
        {
            widgets::text_in(
                &painter,
                c.shrink2(vec2(6.0, 0.0)),
                name,
                theme::prop(10.5),
                pal.text_weak,
                false,
            );
        }
    }
    let mut lu = crate::app::region_ui(ui, rows_r, Layout::top_down(Align::Min), "history_list");
    if h.plays.is_empty() && !h.sessions.is_empty() {
        lu.add_space(8.0);
        lu.label(RichText::new("No tracks in this session").color(pal.text_weak));
    }
    let hover_bg = lu.visuals().widgets.hovered.weak_bg_fill;
    egui::ScrollArea::vertical()
        .id_salt("history_plays")
        .auto_shrink([false, false])
        .show_rows(&mut lu, ROW_H, h.plays.len(), |ui, range| {
            for i in range {
                let p = &h.plays[i];
                let sense = if p.track.is_some() {
                    Sense::click_and_drag()
                } else {
                    Sense::click()
                };
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), sense);
                if let Some(t) = p.track {
                    resp.dnd_set_drag_payload(TrackDrag(t));
                }
                let painter = ui.painter_at(r);
                if resp.hovered() {
                    painter.rect_filled(r, 0.0, hover_bg);
                } else if i % 2 == 1 {
                    painter.rect_filled(r, 0.0, pal.panel.gamma_multiply(0.5));
                }
                let [time_c, deck_c, track_c, bpm_c, key_c] = columns(r);
                let cell = |c: Rect| c.shrink2(vec2(6.0, 0.0));
                let t = history::format_time(p.played_at, offset);
                widgets::text_in(
                    &painter,
                    cell(time_c),
                    t.get(11..).unwrap_or(&t),
                    FontId::proportional(12.5),
                    pal.text_weak,
                    false,
                );
                // The deck as a small badge in its colour.
                if (1..=4).contains(&p.deck) {
                    let dc = theme::deck_color(DeckId::ALL[usize::from(p.deck) - 1]);
                    let chip = Rect::from_min_size(
                        pos2(deck_c.left() + 6.0, r.center().y - 7.0),
                        vec2(16.0, 14.0),
                    );
                    painter.rect_filled(chip, 3.0, dc);
                    painter.text(
                        chip.center(),
                        egui::Align2::CENTER_CENTER,
                        p.deck.to_string(),
                        theme::mono(11.0),
                        Palette::on(dc),
                    );
                }
                // A track no longer in the library: dimmed (it cannot be
                // dragged or loaded).
                let color = if p.track.is_some() {
                    pal.text
                } else {
                    pal.text_weak
                };
                widgets::text_in(
                    &painter,
                    cell(track_c),
                    &p.display(),
                    FontId::proportional(13.0),
                    color,
                    false,
                );
                if let Some(b) = p.bpm {
                    widgets::text_in(
                        &painter,
                        cell(bpm_c),
                        &format!("{b:.1}"),
                        FontId::proportional(12.5),
                        color,
                        false,
                    );
                }
                if let Some(k) = p.key {
                    widgets::text_in(
                        &painter,
                        cell(key_c),
                        &k.display(notation),
                        FontId::proportional(12.5),
                        color,
                        false,
                    );
                }
                if i == 0 {
                    marks.push(("history_play", r, true));
                }
                let Some(t) = p.track else {
                    resp.on_hover_text("No longer in the library");
                    continue;
                };
                if resp.double_clicked() {
                    actions.push(HistoryAction::LoadFree(t));
                }
                resp.context_menu(|ui| {
                    for d in DeckId::ALL {
                        if ui.button(format!("Load on deck {}", d.number())).clicked() {
                            actions.push(HistoryAction::Load(d, t));
                            ui.close();
                        }
                    }
                });
            }
        });

    for (name, r, scrolled) in marks {
        app.map.scrolled = scrolled;
        app.map.add(ui, name, None, r);
    }
    app.map.scrolled = false;
    apply(app, actions);
}

fn apply(app: &mut KaldorApp, actions: Vec<HistoryAction>) {
    for a in actions {
        match a {
            HistoryAction::Pick(s) => {
                if let Some(lib) = app.library.as_mut()
                    && let Some(h) = lib.history.as_mut()
                {
                    h.selected = Some(s);
                    h.plays = lib.lib.session_plays(s).unwrap_or_default();
                }
            }
            HistoryAction::SavePlaylist(name) => browse::open(
                SAVE_PLAYLIST,
                "Save the session as a playlist",
                Pick::SaveAs(
                    &[("M3U8 playlist", "m3u8"), ("PLS playlist", "pls")],
                    format!("{name}.m3u8"),
                ),
                "",
            ),
            HistoryAction::SaveCsv(name) => browse::open(
                SAVE_CSV,
                "Save the session as a CSV file",
                Pick::Save("CSV file", "csv", format!("{name}.csv")),
                "",
            ),
            HistoryAction::ToCrate => {
                app.history_to_crate();
            }
            HistoryAction::Close => {
                if let Some(lib) = app.library.as_mut() {
                    lib.history = None;
                }
            }
            HistoryAction::LoadFree(t) => match app.free_deck_now() {
                Some(deck) => app.load_library_track(deck, t),
                None => {
                    if let Some(lib) = app.library.as_mut() {
                        lib.status = "all decks are playing: load with right-click".into();
                    }
                }
            },
            HistoryAction::Load(d, t) => app.load_library_track(d, t),
        }
    }
}
