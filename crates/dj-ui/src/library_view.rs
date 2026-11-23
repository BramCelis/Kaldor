//! Library browser: crate tree, search and the track list.

use crate::app::{KaldorApp, region_ui};
use crate::file_check::FileState;
use crate::layout::TOOLBAR_ROW_H;
use crate::library::{CrateDrop, CrateRename, LibColumn, LibraryUi, Selection, SortKey};
use crate::theme::{self, Palette};
use crate::uimap::Region;
use crate::widgets::{self, Icon, flat_button};
use dj_core::DeckId;
use dj_core::time::format_mm_ss_t;
use dj_library::{CrateId, CrateNode, TrackId};
use egui::{Align, Color32, FontId, Id, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};
use std::path::PathBuf;

/// Drag-and-drop payload: a track from the list.
#[derive(Clone, Copy, Debug)]
pub struct TrackDrag(pub TrackId);

/// Something the user asked for; applied after drawing.
enum Action {
    Load(DeckId, TrackId),
    LoadFree(TrackId),
    AddToCrate(CrateId, TrackId),
    RemoveFromCrate(CrateId, TrackId),
    NewCrate(Option<CrateId>),
    DeleteCrate(CrateId),
    StartRename(CrateId),
    CommitRename,
    CancelRename,
    Reanalyse(TrackId),
    /// Open the USB export window for crates (with their sub-crates); none
    /// for all crates.
    Export(Vec<CrateId>),
    /// A click on a crate, with Ctrl / Cmd and Shift.
    ClickCrate(CrateId, bool, bool),
    Select(Selection),
    Sort(SortKey),
    /// Switch a track list column on or off.
    ToggleColumn(LibColumn),
    /// Back to the default columns (and widths).
    DefaultColumns,
    /// Move a column to a gap between the shown columns (index in the
    /// column list, 0 = first).
    MoveColumn(LibColumn, usize),
    /// A column's width while its edge is dragged (saved at the end).
    ResizeColumn(LibColumn, f32),
    /// The column fits itself again (double-click on its edge).
    ResetColumnWidth(LibColumn),
    /// Save the config (end of a column resize).
    SaveConfig,
    /// Move a track to a gap in the crate's list as shown.
    MoveInCrate(TrackId, usize),
    /// Open the Locate window for a track whose file is not found.
    Locate(TrackId),
    /// Ask before taking the track (and the selection it is in) out of the
    /// library.
    AskRemove(TrackId),
    /// A new crate with the track (and the selection it is in), named for
    /// typing (tracks dropped under the crates).
    NewCrateWith(TrackId),
    /// Files and folders from the Files panel dropped on the library.
    ImportFiles(Vec<PathBuf>, crate::library::ImportTarget),
    /// Import from another program's library or a playlist file (asks for
    /// the file).
    ImportFrom(crate::interop_view::Source),
    /// Save a crate as an M3U8 or PLS playlist (asks where).
    SavePlaylist(CrateId, String),
    /// Open the smart crate window for a new smart crate (under a crate).
    NewSmartCrate(Option<CrateId>),
    /// Open the smart crate window for a smart crate.
    EditSmartCrate(CrateId, String),
    /// Open the History panel (closes Files).
    History,
    /// Close the History panel.
    CloseHistory,
    /// Show or hide a crate's sub-crates (the arrow in front of it).
    ToggleCollapsed(CrateId),
    /// Show the user's crates or rekordbox's playlists.
    CrateTab(crate::library::CrateTab),
    /// Read rekordbox's library now.
    RekordboxSyncNow,
    /// Turn the rekordbox sync on or off.
    RekordboxEnable(bool),
    /// rekordbox's folder (picked with Browse; `None`: the default).
    RekordboxFolder(Option<PathBuf>),
}

const ROW_H: f32 = 18.0;
/// Width of the column menu button at the right end of the header.
const MENU_W: f32 = 18.0;

/// Column widths fitted to `width`. A column with a width set by dragging
/// (`user`) keeps it; the room left goes to the wide text columns without
/// one (Artist and Title most). On a narrow list the optional columns are
/// hidden first (width 0; Location, Comment, ... Album last), then the text
/// columns shrink; #, the file icon, BPM, Key and Time keep their width.
/// Still too wide (columns made wide by hand): all shrink alike.
fn column_widths(
    cols: &[LibColumn],
    user: &std::collections::BTreeMap<String, f32>,
    width: f32,
) -> Vec<f32> {
    let set = |c: LibColumn| user.get(c.id()).copied().filter(|w| *w > 0.0);
    let flex_of = |c: LibColumn| if set(c).is_some() { 0.0 } else { c.flex() };
    let mut w: Vec<f32> = cols.iter().map(|c| set(*c).unwrap_or(c.width())).collect();
    let mut hide: Vec<usize> = (0..cols.len())
        .filter(|i| cols[*i].hide_rank().is_some())
        .collect();
    hide.sort_by_key(|i| std::cmp::Reverse(cols[*i].hide_rank()));
    for i in hide {
        if w.iter().sum::<f32>() <= width {
            break;
        }
        w[i] = 0.0;
    }
    let shown = |i: usize, w: &[f32]| w[i] > 0.0;
    let flex: f32 = (0..cols.len())
        .filter(|i| shown(*i, &w))
        .map(|i| flex_of(cols[i]))
        .sum();
    let sum: f32 = w.iter().sum();
    if width >= sum {
        let extra = width - sum;
        // No text column to take the room: the columns without a width of
        // their own share it (none: the room stays empty at the right).
        let free: f32 = (0..cols.len())
            .filter(|i| shown(*i, &w) && set(cols[*i]).is_none())
            .map(|i| w[i])
            .sum();
        for i in 0..cols.len() {
            if !shown(i, &w) {
                continue;
            }
            let share = if flex > 0.0 {
                flex_of(cols[i]) / flex
            } else if set(cols[i]).is_none() && free > 0.0 {
                w[i] / free
            } else {
                0.0
            };
            w[i] += extra * share;
        }
        return w;
    }
    if flex > 0.0 {
        let (fixed, flexible): (Vec<usize>, Vec<usize>) =
            (0..cols.len()).partition(|i| flex_of(cols[*i]) == 0.0);
        let fixed: f32 = fixed.iter().map(|i| w[*i]).sum();
        let flex_w: f32 = flexible.iter().map(|i| w[*i]).sum();
        let k = ((width - fixed) / flex_w.max(1.0)).max(0.0);
        for i in flexible {
            if shown(i, &w) {
                w[i] = (w[i] * k).max(40.0);
            }
        }
    }
    let sum: f32 = w.iter().sum();
    if sum > width && width > 0.0 {
        let k = width / sum;
        for x in w.iter_mut().filter(|x| **x > 0.0) {
            *x = (*x * k).max(16.0);
        }
    }
    w
}

/// Narrowest and widest a column can be dragged.
const COL_MIN: f32 = 22.0;
const COL_MAX: f32 = 900.0;

/// The bottom area: the library (toolbar, crate tree and track list). No
/// tabs; the settings are a window over the app
/// (`setup::setup_window`), and the SAMPLER panel is off screen for now.
pub(crate) fn bottom_area(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    let pal = Palette::get(app.config.ui.dark_theme);
    app.map.enter(Region::Bottom, rect);
    ui.painter().rect_filled(rect, 0.0, pal.panel);
    let inner = rect.shrink2(vec2(6.0, 4.0));
    // Files or History open: the panel at the top, above the New crate /
    // search bar; the bar, the crates and the tracks under it (so files and
    // played tracks can be dragged down onto them).
    let history = app.history_open();
    let below_files =
        if (app.files_panel.open || history) && !app.show_sampler && inner.height() > 240.0 {
            let room = inner.height() - TOOLBAR_ROW_H - 4.0;
            let files_h = (room * 0.5).clamp(140.0, room - 120.0);
            let files_r = Rect::from_min_size(inner.min, vec2(inner.width(), files_h));
            if history {
                crate::history_view::history_panel(app, ui, files_r);
            } else {
                crate::files_panel::files_panel(app, ui, files_r);
            }
            files_r.bottom() + 6.0
        } else {
            inner.top()
        };
    let bar = Rect::from_min_size(
        pos2(inner.left(), below_files),
        vec2(inner.width(), TOOLBAR_ROW_H),
    );
    let content = Rect::from_min_max(pos2(inner.left(), bar.bottom() + 4.0), inner.max);
    toolbar(app, ui, bar);
    if content.height() < 8.0 {
        return;
    }
    // Reachable only through `show_sampler` (tests) until it gets a new
    // place.
    if app.show_sampler {
        let mut ui = region_ui(ui, content, Layout::top_down(Align::Min), "sampler_tab");
        crate::sampler_view::sampler_panel(app, &mut ui, content);
        return;
    }
    library_content(app, ui, content);
}

/// The library row: the new-crate button at the left end (above the crate
/// list), import / analysis progress and status after it, the search field
/// at the right end.
fn toolbar(app: &mut KaldorApp, ui: &mut egui::Ui, bar: Rect) {
    let row = bar.shrink2(vec2(0.0, 2.0));
    let pal = Palette::get(app.config.ui.dark_theme);
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    let mut actions: Vec<Action> = Vec::new();
    let mut marks: Vec<(&'static str, Rect)> = Vec::new();

    // Right end: the search field (with ×).
    let search_w = (row.width() * 0.3).clamp(160.0, 320.0);
    let search_r = Rect::from_min_max(pos2(row.right() - search_w, row.top()), row.max);
    let mut su = region_ui(
        ui,
        search_r,
        Layout::right_to_left(Align::Center),
        "search_row",
    );
    let c = su.small_button("×").on_hover_text("Clear the search");
    if c.clicked() {
        lib.search.clear();
    }
    marks.push(("search_clear", c.rect));
    let s = su.add(
        egui::TextEdit::singleline(&mut lib.search)
            .hint_text("Search: words, bpm:120-128, key:8A")
            .desired_width(su.available_width()),
    );
    marks.push(("search", s.rect));
    // Next to the search field: Files (the computer's folders, over the
    // library).
    let files_r = Rect::from_min_size(
        pos2(search_r.left() - 78.0, row.top() - 1.0),
        vec2(70.0, row.height() + 2.0),
    );
    let files_on = app.files_panel.open;
    let f = flat_button(
        ui,
        files_r,
        Id::unique("files_button"),
        "Files",
        theme::prop(13.0),
        None,
        files_on,
        pal.accent,
        false,
    )
    .on_hover_text("Browse the folders on this computer and drag music into crates");
    let toggle_files = f.clicked();
    marks.push(("files_button", files_r));
    // Before Files: History, opened in the same place as Files (one of the
    // two at a time).
    let history_r = files_r.translate(vec2(-(files_r.width() + 8.0), 0.0));
    let history_on = lib.history.is_some();
    let h = flat_button(
        ui,
        history_r,
        Id::unique("history_button"),
        "History",
        theme::prop(13.0),
        None,
        history_on,
        pal.accent,
        false,
    )
    .on_hover_text("The tracks played, session by session");
    let toggle_history = h.clicked();
    marks.push(("history_button", history_r));
    // Before History: Import, a menu of what can be brought in from other
    // software (also in Settings > Library).
    let import_r = Rect::from_min_size(
        pos2(history_r.left() - 8.0 - 92.0, history_r.top()),
        vec2(92.0, history_r.height()),
    );
    let ir = flat_button(
        ui,
        import_r,
        Id::unique("import_button"),
        "Import",
        theme::prop(13.0),
        Some(Icon::Import),
        false,
        pal.accent,
        false,
    );
    let menu_open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&ir));
    let ir = if menu_open {
        ir
    } else {
        ir.on_hover_text("Bring in your library from rekordbox or Apple Music, or a playlist file")
    };
    marks.push(("import_button", import_r));
    let mut menu_marks: Vec<(&'static str, Rect)> = Vec::new();
    egui::Popup::menu(&ir)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| import_menu(ui, &pal, &mut actions, &mut menu_marks));
    let search_r = Rect::from_min_max(import_r.min, search_r.max);
    // Left end, above the crate list: two icon buttons, a new crate and a
    // new smart crate, in the style of the other buttons (grey with the
    // icon in the accent colour; lighter with an accent outline on hover,
    // darker while pressed).
    let add_r = Rect::from_min_size(
        pos2(row.left(), row.top() - 1.0),
        vec2(34.0, row.height() + 2.0),
    );
    let smart_r = add_r.translate(vec2(add_r.width() + 6.0, 0.0));
    let new_buttons = [
        (
            add_r,
            "new_crate",
            Icon::AddCrate,
            "New crate (type its name, Enter)",
            Action::NewCrate(None),
        ),
        (
            smart_r,
            "new_smart_crate",
            Icon::AddSmartCrate,
            "New smart crate: fills itself with the tracks matching its rules",
            Action::NewSmartCrate(None),
        ),
    ];
    // Not on the rekordbox tab: its playlists are made in rekordbox.
    let crates_tab = lib.crate_tab == crate::library::CrateTab::Crates;
    for (r, name, icon, tip, action) in new_buttons {
        if !crates_tab {
            break;
        }
        let b = flat_button(
            ui,
            r,
            Id::unique(name),
            "",
            theme::prop(13.0),
            None,
            false,
            pal.accent,
            false,
        )
        .on_hover_text(tip);
        // Drawn larger than a button's own icon, so the S of the smart
        // crate stays readable.
        let side = (r.height() - 6.0).min(r.width() - 8.0);
        widgets::paint_icon(
            ui.painter(),
            Rect::from_center_size(r.center(), vec2(side, side)),
            icon,
            pal.accent,
        );
        if b.clicked() {
            actions.push(action);
        }
        marks.push((name, r));
    }
    let add_r = if crates_tab {
        add_r.union(smart_r)
    } else {
        Rect::from_min_size(add_r.min, vec2(0.0, add_r.height()))
    };

    // Between: progress and status, cut to the space left.
    let mut status = Vec::new();
    if let Some((d, t)) = lib.import_progress() {
        status.push(format!("importing {d}/{t}"));
    }
    let (done, total) = lib.analysis_progress();
    if total > 0 {
        status.push(format!("analysing {done}/{total}"));
    }
    // A message shows for a while, then goes (progress stays while it
    // runs).
    if let Some(msg) = lib.fresh_status() {
        status.push(msg);
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(500));
    }
    let sr = Rect::from_min_max(
        pos2(add_r.right() + 12.0, row.top()),
        pos2(search_r.left() - 12.0, row.bottom()),
    );
    if !status.is_empty() && sr.width() > 30.0 {
        let text = status.join("  ·  ");
        widgets::text_in(
            &ui.painter_at(sr),
            sr,
            &text,
            theme::prop(12.0),
            pal.text_weak,
            false,
        );
        ui.interact(sr, Id::unique("library_status"), Sense::hover())
            .on_hover_text(text);
        marks.push(("library_status", sr));
    }
    for (name, r) in marks {
        app.map.add(ui, name, None, r);
    }
    for (name, r) in menu_marks {
        app.map.add_rect(name, r, r);
    }
    // Files and History share the place over the library: opening one
    // closes the other.
    if toggle_history {
        if history_on {
            actions.push(Action::CloseHistory);
        } else {
            actions.push(Action::History);
        }
    }
    if toggle_files && !files_on {
        actions.push(Action::CloseHistory);
    }
    if toggle_files {
        let start = app.config.ui.files_folder.clone();
        app.files_panel.toggle(start);
    }
    apply(app, actions);
}

/// The Import button's menu: one card-like row per source (icon tile,
/// title, a line on what comes in), then a hint about music files.
fn import_menu(
    ui: &mut egui::Ui,
    pal: &Palette,
    actions: &mut Vec<Action>,
    marks: &mut Vec<(&'static str, Rect)>,
) {
    const W: f32 = 310.0;
    ui.set_width(W);
    ui.spacing_mut().item_spacing.y = 2.0;
    ui.add_space(2.0);
    ui.label(
        RichText::new("IMPORT FROM")
            .font(theme::prop(10.5))
            .color(pal.text_weak),
    );
    ui.add_space(4.0);
    for s in crate::interop_view::Source::ALL {
        let (r, resp) = ui.allocate_exact_size(vec2(W, 50.0), Sense::click());
        // No tooltip: it would cover the rows under it; the line under the
        // title says what comes in, the Settings card how to export.
        let p = ui.painter();
        let hovered = resp.hovered();
        if hovered {
            p.rect_filled(r, 6.0, ui.visuals().widgets.hovered.bg_fill);
        }
        // The icon in a rounded, lightly tinted tile.
        let tile = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 17.0), vec2(34.0, 34.0));
        p.rect_filled(
            tile,
            8.0,
            pal.accent.gamma_multiply(if hovered { 0.3 } else { 0.16 }),
        );
        widgets::paint_icon(
            p,
            Rect::from_center_size(tile.center(), vec2(18.0, 18.0)),
            s.icon(),
            pal.accent,
        );
        let x = tile.right() + 12.0;
        p.text(
            pos2(x, r.center().y - 9.0),
            egui::Align2::LEFT_CENTER,
            s.title(),
            theme::prop(13.5),
            pal.text,
        );
        p.text(
            pos2(x, r.center().y + 9.0),
            egui::Align2::LEFT_CENTER,
            s.blurb(),
            theme::prop(11.5),
            pal.text_weak,
        );
        if hovered {
            widgets::paint_icon(
                p,
                Rect::from_center_size(pos2(r.right() - 16.0, r.center().y), vec2(12.0, 12.0)),
                Icon::ChevronRight,
                pal.text_weak,
            );
        }
        marks.push((s.browse_id(), r));
        if resp.clicked() {
            actions.push(Action::ImportFrom(s));
            ui.close();
        }
    }
    ui.add_space(4.0);
    ui.separator();
    ui.add(
        egui::Label::new(
            RichText::new(
                "Music files and folders: drag them onto the library, from Files or Explorer.",
            )
            .font(theme::prop(11.0))
            .color(pal.text_weak),
        )
        .wrap(),
    );
}

/// The settings window's Library tab: the analysis settings, then the
/// cards of [`crate::library_settings::library_cards`]. (Importing is done
/// from the Files panel, exporting from a crate's right-click menu.)
pub(crate) fn library_setup(app: &mut KaldorApp, ui: &mut egui::Ui) {
    use crate::settings_ui::{button, card};
    let pal = Palette::get(app.config.ui.dark_theme);
    let Some(lib) = app.library.as_mut() else {
        card(ui, &pal, "Library", |rows| {
            rows.row(
                "No library open",
                "Kaldor was started with --no-library.",
                |_| {},
            );
        });
        return;
    };
    let mut marks: Vec<(&'static str, Rect)> = Vec::new();
    let mut changed = false;
    let cfg = &mut app.config.library;
    card(ui, &pal, "Analysis", |rows| {
        rows.row(
            "BPM range",
            "Detected tempos are folded into this range (half or double). Pick one that fits your genres. For tracks analysed after the next start.",
            |ui| {
                let (mut lo, mut hi) = (cfg.bpm_min, cfg.bpm_max);
                let b = ui.add(
                    egui::DragValue::new(&mut hi)
                        .range(100.0..=250.0)
                        .speed(1.0)
                        .suffix(" BPM"),
                );
                ui.label("to");
                let a = ui.add(
                    egui::DragValue::new(&mut lo)
                        .range(50.0..=140.0)
                        .speed(1.0)
                        .suffix(" BPM"),
                );
                // The range must span at least an octave so every tempo
                // has a place in it.
                if (a.changed() || b.changed()) && hi >= lo * 2.0 {
                    cfg.bpm_min = lo;
                    cfg.bpm_max = hi;
                    changed = true;
                }
                marks.push(("bpm_min", a.rect));
                marks.push(("bpm_max", b.rect));
            },
        );
        rows.row(
            "Analysis threads",
            "How many tracks are analysed at once. Automatic uses half the cores (at most 4). Takes effect at the next start.",
            |ui| {
                let auto = dj_library::AnalysisQueue::default_workers();
                let name = |n: usize| {
                    if n == 0 {
                        format!("Automatic ({auto})")
                    } else {
                        n.to_string()
                    }
                };
                let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
                let r = egui::ComboBox::from_id_salt("analysis_workers")
                    .selected_text(name(cfg.analysis_workers))
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        for n in 0..=cores.min(16) {
                            if ui
                                .selectable_label(cfg.analysis_workers == n, name(n))
                                .clicked()
                            {
                                cfg.analysis_workers = n;
                                changed = true;
                            }
                        }
                    });
                marks.push(("analysis_workers", r.response.rect));
            },
        );
        rows.row(
            "Analyse on import",
            "New tracks are analysed right after they are imported. Off: a track is analysed when it is first loaded (imports finish sooner).",
            |ui| {
                let r = crate::settings_ui::switch(ui, cfg.analyse_on_import);
                if r.clicked() {
                    cfg.analyse_on_import = !cfg.analyse_on_import;
                    lib.analyse_on_import = cfg.analyse_on_import;
                    changed = true;
                }
                marks.push(("analyse_on_import", r.rect));
            },
        );
        rows.row(
            "Re-analyse",
            "Analyses the selected crate again (all tracks when All tracks is selected), for example after changing the BPM range. Locked grids stay.",
            |ui| {
                let b = button(ui, "Re-analyse", false, None);
                if b.clicked() {
                    if lib.protect {
                        lib.status = "Library is protected: nothing re-analysed (Settings > Library)".into();
                    } else {
                        let n = lib.reanalyse_selection();
                        lib.status = format!("{n} tracks queued for analysis");
                    }
                }
                marks.push(("reanalyse_all", b.rect));
            },
        );
        if let Some(status) = lib.fresh_status() {
            rows.block(|ui| {
                ui.label(RichText::new(status).size(12.5).color(pal.text_weak));
            });
        }
    });
    if changed {
        app.events.push(crate::app::AppEvent::ConfigChanged);
    }
    for (name, r) in marks {
        app.map.add(ui, name, None, r);
    }
    crate::library_settings::library_cards(app, ui, &pal);
}

/// Crate tree, splitter and track list.
fn library_content(app: &mut KaldorApp, ui: &mut egui::Ui, content: Rect) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let text_scale = app.config.ui.library_text_scale;
    let notation = app.config.ui.key_notation;
    if app.library.is_none() {
        let mut ui = region_ui(ui, content, Layout::top_down(Align::Min), "no_library");
        let r = ui.label("No library open (started with --no-library).");
        app.map.add(&ui, "no_library", None, r.rect);
        return;
    }
    let max_tree = (content.width() * 0.45).max(120.0);
    app.crate_tree_width = app.crate_tree_width.clamp(120.0, max_tree);
    let tree = Rect::from_min_size(content.min, vec2(app.crate_tree_width, content.height()));
    let split = Rect::from_min_size(
        pos2(tree.right(), content.top()),
        vec2(6.0, content.height()),
    );
    let list = Rect::from_min_max(pos2(split.right(), content.top()), content.max);

    // Splitter: drag to resize the crate tree.
    let s = ui
        .interact(split, Id::unique("crate_splitter"), Sense::drag())
        .on_hover_text("Drag to resize the crate list");
    if s.hovered() || s.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if s.dragged() {
        app.crate_tree_width = (app.crate_tree_width + s.drag_delta().x).clamp(120.0, max_tree);
    }
    ui.painter().line_segment(
        [split.center_top(), split.center_bottom()],
        Stroke::new(
            1.0,
            if s.hovered() {
                pal.accent_line
            } else {
                pal.border
            },
        ),
    );
    app.map.add(ui, "crate_splitter", None, split);

    let mut actions: Vec<Action> = Vec::new();
    let mut marks: Vec<(&'static str, Rect, bool)> = Vec::new();
    let columns = LibColumn::from_config(&app.config.ui.library_columns);
    let user_widths = app.config.ui.library_column_widths.clone();
    // Library tracks loaded on a deck (deck index), marked in the list.
    let on_decks: Vec<(TrackId, usize)> = {
        let snap = app.engine.snapshot();
        app.decks
            .iter()
            .enumerate()
            .filter(|(i, _)| snap.decks[*i].loaded)
            .filter_map(|(i, d)| d.track_id.map(|t| (t, i)))
            .collect()
    };
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    {
        // Two tabs over the crate list: the user's crates and rekordbox's
        // playlists (new crates: the buttons above, in the toolbar).
        let tabs_r = Rect::from_min_size(tree.min, vec2(tree.width() - 4.0, 28.0));
        crate_tabs(ui, tabs_r, lib, &pal, &mut actions, &mut marks);
        let scroll_r = Rect::from_min_max(
            pos2(tree.left(), tabs_r.bottom() + 6.0),
            pos2(tree.right() - 4.0, tree.bottom()),
        );
        let mut tu = region_ui(ui, scroll_r, Layout::top_down(Align::Min), "crate_tree");
        match lib.crate_tab {
            crate::library::CrateTab::Crates => {
                egui::ScrollArea::vertical()
                    .id_salt("crate_scroll")
                    .auto_shrink([false, false])
                    .show(&mut tu, |ui| crate_tree(ui, lib, &mut actions, &mut marks));
            }
            crate::library::CrateTab::Rekordbox => {
                egui::ScrollArea::vertical()
                    .id_salt("rekordbox_scroll")
                    .auto_shrink([false, false])
                    .show(&mut tu, |ui| {
                        rekordbox_tree(ui, lib, &pal, &mut actions, &mut marks)
                    });
            }
        }
    }
    {
        let mut lu = region_ui(ui, list, Layout::top_down(Align::Min), "track_list");
        track_table(
            &mut lu,
            lib,
            &columns,
            &user_widths,
            app.config.ui.waveform_colors,
            notation,
            &mut actions,
            &mut marks,
            pal,
            text_scale,
            &on_decks,
        );
    }
    // A click in the crate list or the track list (anywhere in it, also
    // under the last track) gives the list the keys (Ctrl+A, Delete); a
    // click anywhere else takes them away.
    if ui.input(|i| i.pointer.any_pressed())
        && let Some(p) = ui.input(|i| i.pointer.interact_pos())
    {
        lib.list_focus = tree.contains(p) || list.contains(p);
        // A click in the library but not on a track (the empty part of the
        // list, the crates) clears the selection. Clicks outside the
        // library (decks, waveforms, top bar) keep it, so the track you
        // loaded stays selected while you play and edit its grid, and the
        // next one is right below it (before, any click did).
        // Menus and windows over the app do not count (a right-click menu
        // acts on the selection).
        let on_app = ui
            .ctx()
            .layer_id_at(p)
            .is_none_or(|l| l.order == egui::Order::Background);
        let in_library = tree.contains(p) || list.contains(p);
        let on_tracks = list.contains(p) && p.y <= lib.rows_bottom;
        if on_app && in_library && !on_tracks {
            lib.deselect();
        }
        // The same for the crates: Ctrl+A acts on the crates after a click
        // among them, and a click elsewhere in the library goes back to the
        // open crate alone.
        if on_app {
            lib.crates_focus = tree.contains(p);
            let on_crate = lib.drop_areas.crates.iter().any(|(_, r)| r.contains(p));
            if in_library && !on_crate {
                lib.clear_picked_crates();
            }
        }
    }
    // Files from the Files panel let go over the library: into the crate,
    // a new crate (under the crates, named after the folder) or the
    // library (All tracks; the list: the crate it shows).
    let files_drag =
        egui::DragAndDrop::has_payload_of_type::<crate::files_panel::FileDrag>(ui.ctx());
    if files_drag
        && ui.input(|i| i.pointer.any_released())
        && let Some(p) = ui.input(|i| i.pointer.hover_pos())
        && let Some(target) = lib.drop_areas.target_at(p, lib.selection)
        && let Some(fd) = egui::DragAndDrop::take_payload::<crate::files_panel::FileDrag>(ui.ctx())
    {
        let target = target.named(&fd.0);
        actions.push(Action::ImportFiles(fd.0.clone(), target));
    }
    // Once the controller's browse knob is used: an outline around the list
    // it is in (crates or tracks).
    if lib.knob_used {
        let r = if lib.browse_crates { tree } else { list };
        ui.painter().rect_stroke(
            r.shrink(1.0),
            4.0,
            Stroke::new(2.0, pal.accent_line),
            egui::StrokeKind::Inside,
        );
    }
    for (name, r, scrolled) in marks {
        app.map.scrolled = scrolled;
        app.map.add(ui, name, None, r);
    }
    app.map.scrolled = false;
    apply(app, actions);
}

/// The colour of the rekordbox sync's state: green when up to date, the
/// warning colour while it reads, red when it failed, grey when off or
/// not found.
fn sync_color(state: &crate::rekordbox_sync::SyncState, pal: &Palette) -> Color32 {
    use crate::rekordbox_sync::SyncState as S;
    match state {
        S::Synced => pal.play,
        S::Reading | S::Waiting => pal.warn,
        S::Failed(_) => pal.error,
        S::Off | S::NotFound(_) => pal.text_weak,
    }
}

/// The two tabs over the crate list, as one segmented control: a sunken
/// track, the open tab a raised pill. The rekordbox tab has a dot in the
/// sync's colour.
fn crate_tabs(
    ui: &mut egui::Ui,
    r: Rect,
    lib: &LibraryUi,
    pal: &Palette,
    actions: &mut Vec<Action>,
    marks: &mut Vec<(&'static str, Rect, bool)>,
) {
    use crate::library::CrateTab;
    let p = ui.painter().clone();
    p.rect(
        r,
        7.0,
        pal.well,
        Stroke::new(1.0, pal.border),
        egui::StrokeKind::Inside,
    );
    let half = r.width() / 2.0;
    let tabs = [
        (
            CrateTab::Crates,
            "Crates",
            "crate_tab_crates",
            "Your crates",
        ),
        (
            CrateTab::Rekordbox,
            "rekordbox",
            "crate_tab_rekordbox",
            "Your rekordbox playlists, kept in step with rekordbox on this computer",
        ),
    ];
    for (k, (tab, label, name, tip)) in tabs.into_iter().enumerate() {
        let seg = Rect::from_min_size(
            pos2(r.left() + k as f32 * half, r.top()),
            vec2(half, r.height()),
        )
        .shrink(3.0);
        let resp = ui
            .interact(seg, Id::unique(name), Sense::click())
            .on_hover_text(tip);
        let on = lib.crate_tab == tab;
        if on {
            p.rect(
                seg,
                5.0,
                pal.raised,
                Stroke::new(1.0, pal.border),
                egui::StrokeKind::Inside,
            );
        } else if resp.hovered() {
            p.rect_filled(seg, 5.0, ui.visuals().widgets.hovered.weak_bg_fill);
        }
        let color = if on { pal.text } else { pal.text_weak };
        let font = theme::prop(12.5);
        let g = p.layout_no_wrap(label.to_owned(), font, color);
        let dot = tab == CrateTab::Rekordbox;
        let total = g.size().x + if dot { 12.0 } else { 0.0 };
        let x = seg.center().x - total / 2.0;
        if dot {
            p.circle_filled(
                pos2(x + 3.0, seg.center().y),
                3.0,
                sync_color(&lib.rekordbox.state, pal),
            );
        }
        let tx = if dot { x + 12.0 } else { x };
        p.galley(pos2(tx, seg.center().y - g.size().y / 2.0), g, color);
        if resp.clicked() {
            actions.push(Action::CrateTab(tab));
        }
        marks.push((name, seg, false));
    }
}

/// "3 min ago" for the time of the last sync.
fn ago(t: std::time::Instant) -> String {
    let s = t.elapsed().as_secs();
    match s {
        0..=9 => "just now".into(),
        10..=59 => format!("{s} s ago"),
        60..=3599 => format!("{} min ago", s / 60),
        _ => format!("{} h ago", s / 3600),
    }
}

/// The rekordbox tab: the sync's state with Sync now, rekordbox's whole
/// collection, then its folders and playlists (read-only: they follow
/// rekordbox).
fn rekordbox_tree(
    ui: &mut egui::Ui,
    lib: &mut LibraryUi,
    pal: &Palette,
    actions: &mut Vec<Action>,
    marks: &mut Vec<(&'static str, Rect, bool)>,
) {
    use crate::rekordbox_sync::SyncState as S;
    // The browse knob moved the selection: its row scrolls into view.
    let scroll_to = match lib.selection {
        Selection::Crate(c) if std::mem::take(&mut lib.crate_scroll) => Some(c),
        _ => None,
    };
    // Read-only: nothing is dropped here (the crate tab's targets go).
    lib.drop_areas.crates.clear();
    lib.drop_areas.new_crate = None;
    lib.drop_areas.all_tracks = None;
    lib.crate_drop = None;
    let sync = &lib.rekordbox;
    let small = |t: String| {
        RichText::new(t)
            .font(theme::prop(11.5))
            .color(pal.text_weak)
    };
    // The state line: a dot and a few words, Sync now at the right.
    let line = match &sync.state {
        S::Off => "Sync is off".to_string(),
        S::NotFound(_) => "rekordbox not found".to_string(),
        S::Waiting => "Waiting for rekordbox…".to_string(),
        S::Reading => "Reading rekordbox…".to_string(),
        S::Synced => match sync.last.at {
            Some(t) => format!("Synced {}", ago(t)),
            None => "Synced".into(),
        },
        S::Failed(_) => "Sync failed, trying again".to_string(),
    };
    ui.horizontal(|ui| {
        let (dot, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
        ui.painter()
            .circle_filled(dot.center(), 3.0, sync_color(&sync.state, pal));
        let l = ui.label(small(line));
        if let S::Failed(e) = &sync.state {
            l.on_hover_text(e.as_str());
        }
        if sync.enabled && !matches!(sync.state, S::NotFound(_)) {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = ui
                    .add_enabled(
                        !sync.busy(),
                        egui::Button::new(RichText::new("Sync now").font(theme::prop(11.5)))
                            .small(),
                    )
                    .on_hover_text(
                        "Read rekordbox's library again now (it is read by itself when it changes)",
                    );
                marks.push(("rekordbox_sync_now", b.rect, false));
                if b.clicked() {
                    actions.push(Action::RekordboxSyncNow);
                }
            });
        }
    });
    ui.add_space(4.0);

    // Off or not found: say so, with what to do.
    match &sync.state {
        S::Off => {
            ui.label(small(
                "Turn it on to see your rekordbox playlists here, kept in step with rekordbox."
                    .into(),
            ));
            let b = ui.button("Turn on");
            marks.push(("rekordbox_enable", b.rect, false));
            if b.clicked() {
                actions.push(Action::RekordboxEnable(true));
            }
            if lib.rb_root.is_none() {
                return;
            }
            ui.add_space(6.0);
        }
        S::NotFound(dir) => {
            let where_ = if dir.as_os_str().is_empty() {
                "No rekordbox library is known on this system.".to_string()
            } else {
                format!("No rekordbox library (master.db) in {}.", dir.display())
            };
            ui.add(
                egui::Label::new(small(format!(
                    "{where_} rekordbox 6 or 7 keeps it there once it has run. \
                     If yours is elsewhere, pick its folder."
                )))
                .wrap(),
            );
            let start = dir.display().to_string();
            let (b, picked) = crate::browse::button(
                ui,
                "rekordbox_folder",
                "rekordbox's folder (with master.db)",
                crate::browse::Pick::Folder,
                &start,
            );
            marks.push(("rekordbox_folder", b.rect, false));
            if let Some(p) = picked {
                actions.push(Action::RekordboxFolder(Some(p)));
            }
            if lib.rb_root.is_none() {
                return;
            }
            ui.add_space(6.0);
        }
        _ => {}
    }
    let Some(root) = lib.rb_root else {
        return;
    };
    let selected = lib.selected_crates();
    // The whole collection.
    let count = lib.lib.crate_tracks(root).map(|t| t.len()).unwrap_or(0);
    let r = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.add_space(ARROW_W + 3.0);
            ui.selectable_label(selected.contains(&root), format!("Collection ({count})"))
        })
        .inner
        .on_hover_text("All tracks in rekordbox");
    marks.push(("rekordbox_collection", r.rect, true));
    if r.clicked() {
        actions.push(Action::ClickCrate(root, false, false));
    }
    if scroll_to == Some(root) {
        r.scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
    }
    rb_nodes(
        ui,
        &lib.rb_crates,
        &selected,
        &lib.collapsed,
        scroll_to,
        0,
        actions,
        marks,
    );

    ui.add_space(6.0);
    let last = &lib.rekordbox.last;
    let mut note = String::from(
        "Follows rekordbox on this computer: changes there show up here by themselves. \
         Drag tracks onto decks or into your crates.",
    );
    if last.smart_left_out > 0 {
        note.push_str(&format!(
            " {} intelligent playlist{} not shown yet.",
            last.smart_left_out,
            if last.smart_left_out == 1 {
                " is"
            } else {
                "s are"
            }
        ));
    }
    ui.add(egui::Label::new(small(note)).wrap());
    ui.add_space(48.0);
}

/// rekordbox's folders and playlists: a click opens one in the track list (a
/// folder shows all its playlists' tracks); a folder's arrow opens or closes it.
#[allow(clippy::too_many_arguments)]
fn rb_nodes(
    ui: &mut egui::Ui,
    nodes: &[CrateNode],
    selected: &[CrateId],
    collapsed: &std::collections::HashSet<CrateId>,
    scroll_to: Option<CrateId>,
    depth: usize,
    actions: &mut Vec<Action>,
    marks: &mut Vec<(&'static str, Rect, bool)>,
) {
    for n in nodes {
        let closed = collapsed.contains(&n.id);
        let folder = !n.children.is_empty();
        let r = ui
            .horizontal(|ui| {
                crate_arrow(ui, n, closed, depth, actions, marks);
                if folder {
                    ui.selectable_label(selected.contains(&n.id), RichText::new(&n.name).strong())
                } else {
                    ui.selectable_label(selected.contains(&n.id), &n.name)
                }
            })
            .inner;
        marks.push(("rekordbox_row", r.rect, true));
        if scroll_to == Some(n.id) {
            r.scroll_to_me_animation(None, egui::style::ScrollAnimation::none());
        }
        if r.clicked() {
            // As in the crates tab: a click opens it in the track list (a
            // folder shows all its playlists' tracks); the arrow opens or
            // closes a folder.
            let m = ui.input(|i| i.modifiers);
            actions.push(Action::ClickCrate(n.id, m.command || m.ctrl, m.shift));
        }
        if folder && !closed {
            rb_nodes(
                ui,
                &n.children,
                selected,
                collapsed,
                scroll_to,
                depth + 1,
                actions,
                marks,
            );
        }
    }
}

fn crate_tree(
    ui: &mut egui::Ui,
    lib: &mut LibraryUi,
    actions: &mut Vec<Action>,
    marks: &mut Vec<(&'static str, Rect, bool)>,
) {
    let all = format!("All tracks ({})", lib.track_count());
    let r = ui.selectable_label(lib.selection == Selection::All, all);
    if r.clicked() {
        actions.push(Action::Select(Selection::All));
    }
    r.context_menu(|ui| {
        let b = ui
            .button("Export…")
            .on_hover_text("Puts all crates on a USB stick for rekordbox players");
        marks.push(("crate_export", b.rect, false));
        if b.clicked() {
            actions.push(Action::Export(Vec::new()));
            ui.close();
        }
    });
    marks.push(("all_tracks", r.rect, true));
    let crates = lib.crates.clone();
    let selected = lib.selected_crates();
    let mut rows = Vec::new();
    let tree = Tree {
        selected: &selected,
        collapsed: &lib.collapsed,
    };
    crate_nodes(
        ui,
        &crates,
        &tree,
        &mut lib.renaming,
        0,
        actions,
        &mut rows,
        marks,
    );
    // The browse knob moved the selection: show its row.
    if std::mem::take(&mut lib.crate_scroll) {
        let target = match lib.selection {
            Selection::All => Some(r.rect),
            Selection::Crate(c) => rows.iter().find(|(id, _)| *id == c).map(|x| x.1),
        };
        if let Some(t) = target {
            ui.scroll_to_rect_animation(t, None, egui::style::ScrollAnimation::none());
        }
    }
    ui.add_space(4.0);
    ui.small("Drag tracks onto a crate to add them, or below the crates for a new crate. Right-click a crate for more, such as Export. Import (above the tracks) brings in rekordbox, Apple Music or a playlist.");
    // Room to drop under the last crate, also when the list is full.
    ui.add_space(48.0);

    // Where things can be dropped (this frame): every crate row, the
    // empty space under the crates (a new crate) and All tracks.
    let clip = ui.clip_rect();
    let full = ui.max_rect();
    let last = rows.last().map_or(r.rect.bottom(), |(_, rr)| rr.bottom());
    let empty = Rect::from_x_y_ranges(
        full.x_range(),
        (last + 2.0)..=clip.bottom().max(last + 40.0),
    )
    .intersect(clip);
    lib.drop_areas.crates = rows
        .iter()
        .map(|(c, rr)| (*c, rr.intersect(clip)))
        .collect();
    lib.drop_areas.new_crate = (empty.height() > 4.0).then_some(empty);
    lib.drop_areas.all_tracks =
        Some(Rect::from_x_y_ranges(full.x_range(), r.rect.y_range()).intersect(clip));
    if empty.height() > 4.0 {
        marks.push(("new_crate_area", empty, false));
    }

    // A track or file dragged over a crate: the whole row is the target;
    // the outline and the hint next to the pointer are drawn by the app
    // (`paint_drop_highlight`, as for the decks).
    lib.crate_drop = None;
    let dragged = egui::DragAndDrop::payload::<TrackDrag>(ui.ctx()).map(|p| p.0);
    let files = egui::DragAndDrop::has_payload_of_type::<crate::files_panel::FileDrag>(ui.ctx());
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let released = ui.input(|i| i.pointer.any_released());
    if let Some(p) = pointer.filter(|p| clip.contains(*p))
        && let Some((id, rect)) = rows.iter().copied().find(|(_, r)| r.contains(p))
    {
        if let Some(track) = dragged {
            let set = lib.drag_set(track);
            // A smart crate and a folder take no tracks by hand (shown
            // greyed, as "already there"; not worked out every frame).
            let folder = lib.has_sub_crates(id);
            let already = lib.is_smart(id) || folder || {
                let inside = lib.lib.crate_tracks(id).unwrap_or_default();
                set.iter().all(|t| inside.contains(t))
            };
            lib.crate_drop = Some(CrateDrop {
                id,
                rect: rect.intersect(clip),
                already,
            });
            if released {
                egui::DragAndDrop::take_payload::<TrackDrag>(ui.ctx());
                if folder {
                    lib.status =
                        "A folder holds crates, not tracks: drop on one of its crates".into();
                } else {
                    actions.push(Action::AddToCrate(id, track));
                }
            }
        } else if files {
            lib.crate_drop = Some(CrateDrop {
                id,
                rect: rect.intersect(clip),
                already: lib.has_sub_crates(id),
            });
        }
    }
    // Library tracks dropped under the crates: a new crate with them.
    if let (Some(track), Some(p)) = (dragged, pointer)
        && empty.contains(p)
        && released
    {
        egui::DragAndDrop::take_payload::<TrackDrag>(ui.ctx());
        actions.push(Action::NewCrateWith(track));
    }
    // The crate that just took a track flashes (fading out).
    if let Some((id, t0)) = lib.crate_flash {
        let age = t0.elapsed().as_secs_f32();
        if age > FLASH_SECONDS {
            lib.crate_flash = None;
        } else if let Some((_, rect)) = rows.iter().find(|(c, _)| *c == id) {
            let pal = Palette::get(ui.visuals().dark_mode);
            let a = 1.0 - age / FLASH_SECONDS;
            ui.painter()
                .rect_filled(*rect, 4.0, pal.play.gamma_multiply(0.35 * a));
            ui.ctx().request_repaint();
        }
    }
    for (_, rect) in rows {
        marks.push(("crate_row", rect, true));
    }
}

/// How long a crate flashes after a track was dropped on it.
const FLASH_SECONDS: f32 = 0.8;

/// What the crate rows are drawn with.
struct Tree<'a> {
    /// The selected crates (highlighted).
    selected: &'a [CrateId],
    /// The crates whose sub-crates are hidden.
    collapsed: &'a std::collections::HashSet<CrateId>,
}

/// Width of the arrow in front of a crate with sub-crates (crates without
/// get the same room, so the names line up).
const ARROW_W: f32 = 14.0;

/// The indent for `depth` and, for a crate with sub-crates, the small arrow
/// that shows or hides them (right: hidden, down: shown).
fn crate_arrow(
    ui: &mut egui::Ui,
    n: &CrateNode,
    closed: bool,
    depth: usize,
    actions: &mut Vec<Action>,
    marks: &mut Vec<(&'static str, Rect, bool)>,
) {
    ui.spacing_mut().item_spacing.x = 3.0;
    ui.add_space(depth as f32 * 12.0);
    let h = ui.spacing().interact_size.y.min(18.0);
    if n.children.is_empty() {
        // The arrow's room and the gap after it (`add_space` adds none).
        ui.add_space(ARROW_W + ui.spacing().item_spacing.x);
        return;
    }
    let (r, resp) = ui.allocate_exact_size(vec2(ARROW_W, h), Sense::click());
    let resp = resp.on_hover_text(if closed {
        "Show the sub-crates"
    } else {
        "Hide the sub-crates"
    });
    let color = if resp.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    widgets::paint_icon(
        ui.painter(),
        Rect::from_center_size(r.center(), vec2(12.0, 12.0)),
        if closed {
            Icon::ChevronRight
        } else {
            Icon::ChevronDown
        },
        color,
    );
    if resp.clicked() {
        actions.push(Action::ToggleCollapsed(n.id));
    }
    marks.push(("crate_arrow", r, true));
}

/// The crate rows; `rows` gets each crate's full-width row (drop target).
/// The sub-crates of a closed crate are not drawn.
#[allow(clippy::too_many_arguments)]
fn crate_nodes(
    ui: &mut egui::Ui,
    nodes: &[CrateNode],
    tree: &Tree,
    renaming: &mut Option<CrateRename>,
    depth: usize,
    actions: &mut Vec<Action>,
    rows: &mut Vec<(CrateId, Rect)>,
    marks: &mut Vec<(&'static str, Rect, bool)>,
) {
    let selected = tree.selected;
    for n in nodes {
        let closed = tree.collapsed.contains(&n.id);
        if let Some(rn) = renaming.as_mut().filter(|r| r.id == n.id) {
            // Renaming in place: Enter or clicking elsewhere saves, Escape
            // cancels.
            let r = ui
                .horizontal(|ui| {
                    crate_arrow(ui, n, closed, depth, actions, marks);
                    ui.add(egui::TextEdit::singleline(&mut rn.name).desired_width(140.0))
                })
                .inner;
            // Not in the frame of the click that started the rename (the new
            // crate button): a text field drops the focus on a click
            // elsewhere in the same frame.
            let clicking = ui.input(|i| i.pointer.any_click() || i.pointer.any_down());
            if rn.focus_pending && !clicking {
                r.request_focus();
                // The whole name selected, so typing replaces it.
                let mut st = egui::TextEdit::load_state(ui.ctx(), r.id).unwrap_or_default();
                st.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(rn.name.chars().count()),
                )));
                st.store(ui.ctx(), r.id);
                rn.focus_pending = false;
            } else if r.lost_focus() {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    actions.push(Action::CancelRename);
                } else {
                    actions.push(Action::CommitRename);
                }
            }
            if !closed {
                crate_nodes(
                    ui,
                    &n.children,
                    tree,
                    renaming,
                    depth + 1,
                    actions,
                    rows,
                    marks,
                );
            }
            continue;
        }
        let r = ui
            .horizontal(|ui| {
                crate_arrow(ui, n, closed, depth, actions, marks);
                if n.smart {
                    // Smart crates: italic, with a small accent dot after
                    // the name.
                    let r = ui
                        .selectable_label(
                            selected.contains(&n.id),
                            RichText::new(&n.name).italics(),
                        )
                        .on_hover_text(
                            "Smart crate: fills itself from its rules (right-click to edit them)",
                        );
                    let pal = Palette::get(ui.visuals().dark_mode);
                    ui.painter().circle_filled(
                        pos2(r.rect.right() + 5.0, r.rect.center().y),
                        2.5,
                        pal.accent,
                    );
                    r
                } else {
                    ui.selectable_label(selected.contains(&n.id), &n.name)
                }
            })
            .inner;
        let full = ui.max_rect();
        rows.push((
            n.id,
            Rect::from_x_y_ranges(
                full.x_range(),
                (r.rect.top() - 1.0)..=(r.rect.bottom() + 1.0),
            ),
        ));
        if r.clicked() {
            let m = ui.input(|i| i.modifiers);
            actions.push(Action::ClickCrate(n.id, m.command || m.ctrl, m.shift));
        }
        r.context_menu(|ui| {
            if n.smart {
                let b = ui.button("Edit rules…");
                marks.push(("smart_edit", b.rect, false));
                if b.clicked() {
                    actions.push(Action::EditSmartCrate(n.id, n.name.clone()));
                    ui.close();
                }
            }
            // A smart crate is no folder: no sub-crates in it.
            if !n.smart {
                if ui.button("New sub-crate").clicked() {
                    actions.push(Action::NewCrate(Some(n.id)));
                    ui.close();
                }
                if ui.button("New smart sub-crate…").clicked() {
                    actions.push(Action::NewSmartCrate(Some(n.id)));
                    ui.close();
                }
            }
            if ui.button("Rename").clicked() {
                actions.push(Action::StartRename(n.id));
                ui.close();
            }
            // On a selected crate it exports every selected crate.
            let set = if selected.len() > 1 && selected.contains(&n.id) {
                selected.to_vec()
            } else {
                vec![n.id]
            };
            let (label, tip) = if set.len() > 1 {
                (
                    format!("Export {} crates…", set.len()),
                    "Puts the selected crates and their sub-crates on a USB stick for rekordbox players",
                )
            } else {
                (
                    "Export…".to_string(),
                    "Puts this crate and its sub-crates on a USB stick for rekordbox players",
                )
            };
            let b = ui.button(label).on_hover_text(tip);
            marks.push(("crate_export", b.rect, false));
            if b.clicked() {
                actions.push(Action::Export(set));
                ui.close();
            }
            let b = ui
                .button("Save as playlist…")
                .on_hover_text("Writes this crate's tracks to an M3U8 or PLS playlist file");
            marks.push(("crate_save_playlist", b.rect, false));
            if b.clicked() {
                actions.push(Action::SavePlaylist(n.id, n.name.clone()));
                ui.close();
            }
            ui.separator();
            if ui.button("Delete crate").clicked() {
                actions.push(Action::DeleteCrate(n.id));
                ui.close();
            }
        });
        if r.double_clicked() {
            actions.push(Action::StartRename(n.id));
        }
        if !closed {
            crate_nodes(
                ui,
                &n.children,
                tree,
                renaming,
                depth + 1,
                actions,
                rows,
                marks,
            );
        }
    }
}

/// The column menu: a checkbox per column (the menu stays open, so several
/// can be switched), and back to the defaults.
fn column_menu(ui: &mut egui::Ui, columns: &[LibColumn], actions: &mut Vec<Action>) {
    ui.label(RichText::new("Columns").strong());
    for c in LibColumn::ALL {
        let mut on = columns.contains(&c);
        let last = on && columns.len() == 1;
        let r = ui.add_enabled(!last, egui::Checkbox::new(&mut on, c.menu_label()));
        if r.clicked() {
            actions.push(Action::ToggleColumn(c));
        }
    }
    ui.separator();
    if ui.button("Default columns").clicked() {
        actions.push(Action::DefaultColumns);
        ui.close();
    }
}

#[allow(clippy::too_many_arguments)]
fn track_table(
    ui: &mut egui::Ui,
    lib: &mut LibraryUi,
    columns: &[LibColumn],
    user_widths: &std::collections::BTreeMap<String, f32>,
    wave_mode: dj_core::config::WaveformColorMode,
    notation: dj_core::KeyNotation,
    actions: &mut Vec<Action>,
    marks: &mut Vec<(&'static str, Rect, bool)>,
    pal: Palette,
    scale: f32,
    on_decks: &[(TrackId, usize)],
) {
    // Text size from the settings: rows and fonts grow together.
    let scale = scale.clamp(0.8, 1.6);
    let row_h = (ROW_H * scale).round();
    let view: Vec<TrackId> = lib.view().to_vec();
    // The small waveforms of the rows on screen (last frame), a few per
    // frame; the rest the next frames.
    if columns.contains(&LibColumn::Waveform) {
        let rows = lib.visible_rows.clone();
        if lib.minis.load(&lib.lib, &rows) {
            ui.ctx().request_repaint();
        }
    }
    // Room at the right end for two deck badges (a track on two decks), so
    // they never cover the last column's text.
    let badge_room = 2.0 * ((row_h - 6.0).max(12.0) + 2.0 + 3.0) + 4.0;
    let widths = column_widths(
        columns,
        user_widths,
        ui.available_width() - MENU_W - badge_room,
    );
    // Header: click to sort, again to reverse; the arrow is drawn. Drag a
    // header to move the column, its right edge to resize it (double-click
    // the edge: fit again). Right-click (or the button at the right end)
    // chooses the columns.
    let (hrect, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), row_h + 2.0), Sense::hover());
    ui.painter().rect_filled(hrect, 0.0, pal.raised);
    let menu_close = egui::PopupCloseBehavior::CloseOnClickOutside;
    // The shown columns: index in `columns` and header cell.
    let mut cells: Vec<(usize, Rect)> = Vec::new();
    let mut x = hrect.left();
    for (i, w) in widths.iter().copied().enumerate() {
        if w > 0.0 {
            cells.push((
                i,
                Rect::from_min_size(pos2(x, hrect.top()), vec2(w, hrect.height())),
            ));
            x += w;
        }
    }
    // A header being dragged: the gap nearest the pointer, as an index in
    // `columns` (the gap after the last shown column = the end).
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let gap_at = |px: f32| -> (usize, f32) {
        let mut best = (columns.len(), x);
        let mut dist = f32::INFINITY;
        for (k, (i, c)) in cells.iter().enumerate() {
            for (gx, gi) in [
                (c.left(), *i),
                (c.right(), cells.get(k + 1).map_or(columns.len(), |n| n.0)),
            ] {
                if (px - gx).abs() < dist {
                    dist = (px - gx).abs();
                    best = (gi, gx);
                }
            }
        }
        best
    };
    lib.column_drag = None;
    let mut gap_line: Option<f32> = None;
    for &(ci, cell) in &cells {
        let col = &columns[ci];
        let name = col.name();
        let key = col.sort_key();
        let hover = match col {
            LibColumn::Number => "Sort by list order (import order, or the crate's order)".into(),
            LibColumn::Found => "File found or not: sort by it".into(),
            LibColumn::Waveform => "The whole track with its hot cues".into(),
            _ => format!("Sort by {name}"),
        };
        let r = ui.interact(
            cell,
            Id::unique(("sort", col.id())),
            Sense::click_and_drag(),
        );
        let r = if r.dragged() {
            r
        } else {
            r.on_hover_text(hover)
        };
        // The waveform is not sorted by.
        if r.clicked() && *col != LibColumn::Waveform {
            actions.push(Action::Sort(key));
        }
        if (r.dragged() || r.drag_stopped())
            && let Some(p) = pointer.or(r.interact_pointer_pos())
        {
            let (gap, gx) = gap_at(p.x);
            lib.column_drag = Some((*col, gap));
            gap_line = Some(gx);
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            if r.drag_stopped() {
                lib.column_drag = None;
                actions.push(Action::MoveColumn(*col, gap));
            }
        }
        if r.dragged() {
            ui.painter()
                .rect_filled(cell, 0.0, pal.accent.gamma_multiply(0.25));
        }
        egui::Popup::context_menu(&r)
            .close_behavior(menu_close)
            .show(|ui| column_menu(ui, columns, actions));
        marks.push((name, cell, false));
        let painter = ui.painter_at(cell);
        let active = lib.sort.0 == key;
        let color = if active || r.hovered() {
            pal.text
        } else {
            pal.text_weak
        };
        if *col == LibColumn::Found {
            // The icon itself as the header (no room for a word).
            let ir =
                Rect::from_center_size(pos2(cell.left() + 11.0, cell.center().y), vec2(12.0, 12.0));
            paint_file_icon(&painter, ir, FileState::Found, pal, Some(color));
            continue;
        }
        // "#" stands right-aligned over its numbers, its sort arrow left.
        let number = *col == LibColumn::Number;
        let text_r = if number {
            Rect::from_min_max(
                pos2(cell.left() + 14.0, cell.top()),
                pos2(cell.right() - 6.0, cell.bottom()),
            )
        } else {
            Rect::from_min_max(
                pos2(cell.left() + 3.0, cell.top()),
                pos2(cell.right() - 14.0, cell.bottom()),
            )
        };
        // Small capitals, as the labels on the FX display.
        widgets::text_in(
            &painter,
            text_r,
            &name.to_uppercase(),
            theme::prop(10.5 * scale),
            color,
            number,
        );
        if active {
            let ax = if number {
                cell.left() + 8.0
            } else {
                cell.right() - 8.0
            };
            let ar = Rect::from_center_size(pos2(ax, cell.center().y), vec2(10.0, 10.0));
            widgets::paint_icon(
                &painter,
                ar,
                if lib.sort.1 {
                    Icon::SortUp
                } else {
                    Icon::SortDown
                },
                pal.accent,
            );
        }
    }
    // Where a dragged header would go.
    if let Some(gx) = gap_line {
        ui.painter().line_segment(
            [pos2(gx, hrect.top()), pos2(gx, hrect.bottom())],
            Stroke::new(3.0, pal.accent_line),
        );
    }
    // Resize handles on the right edge of every shown column.
    for &(ci, cell) in &cells {
        let col = columns[ci];
        let edge = Rect::from_center_size(
            pos2(cell.right(), cell.center().y),
            vec2(7.0, cell.height()),
        );
        let r = ui
            .interact(
                edge,
                Id::unique(("col_resize", col.id())),
                Sense::click_and_drag(),
            )
            .on_hover_text("Drag to resize, double-click to fit");
        if r.hovered() || r.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
            ui.painter().line_segment(
                [edge.center_top(), edge.center_bottom()],
                Stroke::new(2.0, pal.accent_line),
            );
        } else {
            ui.painter().line_segment(
                [
                    pos2(cell.right() - 0.5, cell.top() + 4.0),
                    pos2(cell.right() - 0.5, cell.bottom() - 4.0),
                ],
                Stroke::new(1.0, pal.border),
            );
        }
        if r.dragged() && r.drag_delta().x != 0.0 {
            let w = (cell.width() + r.drag_delta().x).clamp(COL_MIN, COL_MAX);
            actions.push(Action::ResizeColumn(col, w));
        }
        if r.drag_stopped() {
            actions.push(Action::SaveConfig);
        }
        if r.double_clicked() {
            actions.push(Action::ResetColumnWidth(col));
        }
        marks.push(("col_resize", edge, false));
    }
    // The column menu button at the right end of the header.
    let mrect = Rect::from_min_max(pos2(hrect.right() - MENU_W, hrect.top()), hrect.max);
    let mr = ui
        .interact(mrect, Id::unique("columns_menu"), Sense::click())
        .on_hover_text("Choose the columns (or right-click the header)");
    widgets::paint_icon(
        &ui.painter_at(mrect),
        Rect::from_center_size(mrect.center(), vec2(12.0, 12.0)),
        Icon::Columns,
        if mr.hovered() {
            pal.text
        } else {
            pal.text_weak
        },
    );
    egui::Popup::menu(&mr)
        .close_behavior(menu_close)
        .show(|ui| column_menu(ui, columns, actions));
    marks.push(("columns_menu", mrect, false));
    let text_color = ui.visuals().text_color();
    let weak = ui.visuals().weak_text_color();
    let sel_bg = ui.visuals().selection.bg_fill;
    let hover_bg = ui.visuals().widgets.hovered.weak_bg_fill;
    let crate_sel = match lib.selection {
        Selection::Crate(c) => Some(c),
        Selection::All => None,
    };
    // rekordbox's crates (read-only) and folders (crates with sub-crates):
    // their rows offer no Remove, and they are not in Add to crate.
    let mut synced = lib.rb_crate_ids();
    synced.extend(lib.rb_root);
    let folders: Vec<CrateId> = lib
        .crate_list()
        .into_iter()
        .map(|c| c.0)
        .filter(|c| lib.has_sub_crates(*c))
        .collect();
    synced.extend(folders.iter().copied());
    let crate_list = lib.crate_list();
    let mut area = egui::ScrollArea::vertical()
        .id_salt("track_scroll")
        .auto_shrink([false, false]);
    // The controller's browse knob scrolls the list to the selection.
    if let Some(y) = lib.scroll_to.take() {
        area = area.vertical_scroll_offset(y.max(0.0));
    }
    // The real distance between rows (row height plus spacing), for the
    // browse knob's scrolling.
    lib.row_pitch = row_h + ui.spacing().item_spacing.y;
    let pitch = lib.row_pitch;
    let spacing_y = ui.spacing().item_spacing.y;
    // A library track being dragged, and whether it can be put in a new
    // place in this list (a crate in its own order).
    let dragged = egui::DragAndDrop::payload::<TrackDrag>(ui.ctx()).map(|p| p.0);
    let reorder = dragged.is_some() && lib.reorder_blocked().is_none();
    let mut visible = Vec::new();
    let out = area.show_rows(ui, row_h, view.len(), |ui, range| {
        // Dragging near the top or bottom edge scrolls the list.
        if reorder && let Some(p) = pointer {
            let clip = ui.clip_rect();
            let band = 32.0;
            if clip.x_range().contains(p.x) {
                let speed = if p.y > clip.top() && p.y < clip.top() + band {
                    (clip.top() + band - p.y) / band
                } else if p.y < clip.bottom() && p.y > clip.bottom() - band {
                    -(p.y - (clip.bottom() - band)) / band
                } else {
                    0.0
                };
                if speed != 0.0 {
                    ui.scroll_with_delta(vec2(0.0, speed * 14.0));
                    ui.ctx().request_repaint();
                }
            }
        }
        // Room under the last row, so it never ends on the list's edge.
        let end = range.end;
        for i in range {
            let Some(&id) = view.get(i) else { continue };
            let Some(t) = lib.track(id) else { continue };
            visible.push(id);
            // The database's flag until the checker has looked at the file.
            let state = match lib.file_state(id) {
                FileState::Unknown if t.missing => FileState::Missing,
                s => s,
            };
            let width = ui.available_width();
            let (rect, mut resp) =
                ui.allocate_exact_size(vec2(width, row_h), Sense::click_and_drag());
            resp.dnd_set_drag_payload(TrackDrag(id));
            let painter = ui.painter_at(rect);
            let picked = lib.is_picked(id);
            if picked {
                painter.rect_filled(rect, 0.0, sel_bg);
            } else if resp.hovered() {
                painter.rect_filled(rect, 0.0, hover_bg);
            } else if i % 2 == 1 {
                painter.rect_filled(rect, 0.0, pal.raised.gamma_multiply(0.5));
            }
            // Loaded on a deck: a faint tint and a stripe on the left edge in
            // the deck's colour (the first deck when it is on two), badges
            // with the deck numbers at the right end (below).
            let loaded_on: Vec<usize> = on_decks
                .iter()
                .filter(|(t, _)| *t == id)
                .map(|(_, d)| *d)
                .collect();
            if let Some(&first) = loaded_on.first() {
                let dc = theme::deck_color(DeckId::ALL[first]);
                if !picked {
                    painter.rect_filled(rect, 0.0, dc.gamma_multiply(0.12));
                }
                painter.rect_filled(
                    Rect::from_min_size(rect.min, vec2(3.0, rect.height())),
                    0.0,
                    dc,
                );
            }
            // A track whose file is not there is dimmed (the icon says why).
            let color = if state.is_missing() { weak } else { text_color };
            let mut x = rect.left();
            for (col, w) in columns.iter().zip(widths.iter().copied()) {
                if w <= 0.0 {
                    continue;
                }
                let full = Rect::from_min_size(pos2(x, rect.top()), vec2(w, row_h));
                x += w;
                if *col == LibColumn::Waveform {
                    crate::library_wave::paint(
                        &painter,
                        full.shrink2(vec2(4.0, 2.0)),
                        lib.minis.get(id),
                        wave_mode,
                        weak,
                    );
                    continue;
                }
                if *col == LibColumn::Found {
                    let ir = Rect::from_center_size(
                        pos2(full.left() + 11.0, full.center().y),
                        vec2(12.0, 12.0) * scale,
                    );
                    paint_file_icon(&painter, ir, state, pal, None);
                    if resp.hover_pos().is_some_and(|p| full.contains(p)) {
                        resp = resp.on_hover_text(state.describe(&t.meta.path));
                    }
                    continue;
                }
                let text = cell_text(*col, t, lib.order_number(id).unwrap_or(i + 1), notation);
                let cell = Rect::from_min_size(
                    pos2(full.left() + 3.0, rect.top()),
                    vec2((w - 9.0).max(1.0), row_h),
                );
                let c = if *col == LibColumn::Number
                    || (t.analysis_version.is_none() && text.is_empty())
                {
                    weak
                } else {
                    color
                };
                widgets::text_in(
                    &ui.painter_at(cell),
                    cell,
                    &text,
                    FontId::proportional(13.0 * scale),
                    c,
                    *col == LibColumn::Number,
                );
            }
            // A badge per deck the track is loaded on, at the row's right
            // end (over the gap under the column button), deck 1 leftmost.
            let mut right = rect.right() - 4.0;
            for &d in loaded_on.iter().rev() {
                let dc = theme::deck_color(DeckId::ALL[d]);
                let h = (row_h - 6.0).max(12.0);
                let chip = Rect::from_min_max(
                    pos2(right - h - 2.0, rect.center().y - h / 2.0),
                    pos2(right, rect.center().y + h / 2.0),
                );
                painter.rect_filled(chip, 3.0, dc);
                painter.text(
                    chip.center(),
                    egui::Align2::CENTER_CENTER,
                    (d + 1).to_string(),
                    theme::mono(11.0 * scale),
                    Palette::on(dc),
                );
                if resp.hover_pos().is_some_and(|p| chip.contains(p)) {
                    resp = resp.on_hover_text(format!("Loaded on deck {}", d + 1));
                }
                marks.push(("row_deck", chip, true));
                right = chip.left() - 3.0;
            }
            if i == 0 {
                marks.push(("track_row", rect, true));
            }
            if i + 1 == view.len() {
                marks.push(("track_row_last", rect, true));
            }
            // Plain click: this track alone; Ctrl / Cmd: add or take away;
            // Shift: the rows from the last click. A right-click on a track
            // outside the selection selects it first (as in Serato).
            let mods = ui.input(|i| i.modifiers);
            if resp.clicked() {
                lib.click_track(id, mods.command || mods.ctrl, mods.shift);
            }
            if resp.secondary_clicked() && !picked {
                lib.click_track(id, false, false);
            }
            if resp.double_clicked() {
                actions.push(Action::LoadFree(id));
            }
            // Menu entries that act on the selection say how many tracks.
            let n = if lib.picked.contains(&id) {
                lib.picked.len().max(1)
            } else {
                1
            };
            let these = if n == 1 {
                String::new()
            } else {
                format!(" ({n} tracks)")
            };
            resp.context_menu(|ui| {
                if state.is_missing() {
                    if ui.button("Locate file…").clicked() {
                        actions.push(Action::Locate(id));
                        ui.close();
                    }
                    ui.separator();
                }
                for d in DeckId::ALL {
                    if ui.button(format!("Load on deck {}", d.number())).clicked() {
                        actions.push(Action::Load(d, id));
                        ui.close();
                    }
                }
                ui.separator();
                ui.menu_button(format!("Add to crate{these}"), |ui| {
                    for (cid, name, depth) in &crate_list {
                        // A folder is shown (the tree stays readable) but
                        // takes no tracks.
                        let label = format!("{}{name}", "  ".repeat(*depth));
                        if folders.contains(cid) {
                            ui.add_enabled(false, egui::Button::new(label))
                                .on_disabled_hover_text("A folder: pick one of its crates");
                            continue;
                        }
                        if ui.button(label).clicked() {
                            actions.push(Action::AddToCrate(*cid, id));
                            ui.close();
                        }
                    }
                });
                if ui.button(format!("Re-analyse{these}")).clicked() {
                    actions.push(Action::Reanalyse(id));
                    ui.close();
                }
                ui.separator();
                // In a crate: out of this crate (Delete); in All tracks: out
                // of the library (Delete, asks first). The files stay.
                if let Some(c) = crate_sel.filter(|c| !synced.contains(c))
                    && ui
                        .button(format!("Remove from this crate{these}"))
                        .on_hover_text("Delete key")
                        .clicked()
                {
                    actions.push(Action::RemoveFromCrate(c, id));
                    ui.close();
                }
                let lib_hint = if crate_sel.is_some() {
                    "Out of every crate and the library; the files stay on your computer"
                } else {
                    "Delete key. Out of every crate and the library; the files stay on your computer"
                };
                if ui
                    .button(format!("Remove from library…{these}"))
                    .on_hover_text(lib_hint)
                    .clicked()
                {
                    actions.push(Action::AskRemove(id));
                    ui.close();
                }
            });
        }
        if end >= view.len() {
            ui.add_space(crate::library::LIST_END_PAD);
        }
    });
    lib.list_scroll = (out.state.clamped_offset().y, out.inner_rect.height());
    lib.drop_areas.list = Some(out.inner_rect);
    lib.rows_bottom =
        out.inner_rect.top() + view.len() as f32 * pitch - out.state.clamped_offset().y;
    lib.visible_rows = visible;

    // Keys for the list, after a click in the crates or the list (also
    // under the last track; not while typing): Delete takes the selection
    // out of the crate (in All tracks: out of the library, after asking),
    // Ctrl+A selects every crate (after a click in the crates) or every
    // track of the crate shown (after a click in the tracks). Where the
    // focus is set: `library_content`.
    if lib.list_focus && !ui.ctx().egui_wants_keyboard_input() {
        let (del, all) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace),
                // Ctrl / Cmd as the key event says (also when the window
                // missed the Ctrl press).
                i.events.iter().any(|e| {
                    matches!(e, egui::Event::Key {
                        key: egui::Key::A,
                        pressed: true,
                        modifiers,
                        ..
                    } if modifiers.command || modifiers.ctrl)
                }),
            )
        });
        if all {
            if lib.crates_focus {
                lib.toggle_all_crates();
            } else {
                lib.toggle_all();
            }
        }
        if del && let Some(id) = lib.selected_track.or(lib.picked.first().copied()) {
            match crate_sel {
                // rekordbox's playlists follow rekordbox.
                Some(c) if lib.is_synced(c) => {
                    lib.status = "rekordbox's playlists follow rekordbox: change them there".into();
                }
                // A folder holds no tracks of its own.
                Some(c) if lib.has_sub_crates(c) => {
                    lib.status =
                        "This is a folder: remove the track from the crate it is in".into();
                }
                Some(c) => actions.push(Action::RemoveFromCrate(c, id)),
                None => actions.push(Action::AskRemove(id)),
            }
        }
    }

    // A track dragged over the list: in a crate in its own order, a line
    // shows where it goes and a release puts it there; otherwise the label
    // next to the pointer says why not.
    lib.list_drop = None;
    lib.list_hint = None;
    let list_r = out.inner_rect;
    if let (Some(track), Some(p)) = (dragged, pointer)
        && list_r.contains(p)
    {
        if let Some(why) = lib.reorder_blocked() {
            if matches!(lib.selection, Selection::Crate(_)) {
                lib.list_hint = Some(why.to_string());
            }
        } else {
            let offset = out.state.clamped_offset().y;
            let gap =
                (((p.y - list_r.top() + offset) / pitch).round().max(0.0) as usize).min(view.len());
            let from = view.iter().position(|t| *t == track);
            // The # it gets: its row after the move, counted from 1 (from
            // the end when the list is shown with # descending).
            let row = match from {
                Some(f) if gap > f => gap - 1,
                _ => gap,
            };
            let place = if lib.sort.1 {
                row + 1
            } else {
                view.len() - row
            };
            let unchanged = from.is_some_and(|f| gap == f || gap == f + 1);
            lib.list_drop = Some(gap);
            lib.list_hint = Some(if unchanged {
                "Stays where it is".into()
            } else {
                format!("Move to position {place}")
            });
            let y = list_r.top() + gap as f32 * pitch - offset - spacing_y / 2.0;
            let painter = ui.painter_at(list_r.expand2(vec2(0.0, 2.0)));
            let stroke = Stroke::new(2.5, pal.accent_line);
            painter.line_segment([pos2(list_r.left(), y), pos2(list_r.right(), y)], stroke);
            painter.circle_filled(pos2(list_r.left() + 3.0, y), 4.0, pal.accent_line);
            if ui.input(|i| i.pointer.any_released()) {
                egui::DragAndDrop::take_payload::<TrackDrag>(ui.ctx());
                if !unchanged {
                    actions.push(Action::MoveInCrate(track, gap));
                }
            }
        }
    }
}

/// The text of one cell (`number`: the track's #, see [`LibraryUi::order_number`]).
fn cell_text(
    col: LibColumn,
    t: &dj_library::Track,
    number: usize,
    notation: dj_core::KeyNotation,
) -> String {
    match col {
        LibColumn::Number => number.to_string(),
        LibColumn::Found | LibColumn::Waveform => String::new(),
        LibColumn::Artist => t.meta.artist.clone(),
        LibColumn::Title => {
            if t.meta.title.is_empty() {
                t.display_name()
            } else {
                t.meta.title.clone()
            }
        }
        LibColumn::Bpm => t.bpm.map(|b| format!("{b:.2}")).unwrap_or_default(),
        LibColumn::Key => t.key.map(|k| k.display(notation)).unwrap_or_default(),
        LibColumn::Time => format_mm_ss_t(t.meta.duration)
            .trim_end_matches(".0")
            .to_string(),
        LibColumn::Album => t.meta.album.clone(),
        LibColumn::Genre => t.meta.genre.clone(),
        LibColumn::Added => format_date(t.date_added),
        LibColumn::Year => t.meta.year.map(|y| y.to_string()).unwrap_or_default(),
        LibColumn::Comment => t.meta.comment.clone(),
        LibColumn::Bitrate => t
            .meta
            .bitrate
            .map(|b| format!("{b} kbps"))
            .unwrap_or_default(),
        LibColumn::Plays => {
            if t.play_count == 0 {
                String::new()
            } else {
                t.play_count.to_string()
            }
        }
        LibColumn::FileName => t
            .meta
            .path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default(),
        LibColumn::Location => t
            .meta
            .path
            .parent()
            .map(|s| s.display().to_string())
            .unwrap_or_default(),
    }
}

/// The file icon: a small filled dot when the file is there, a red ring
/// with "!" when it is not, a dashed ring for "not checked yet". `tint`
/// overrides the colour (the header).
pub(crate) fn paint_file_icon(
    painter: &egui::Painter,
    rect: Rect,
    state: FileState,
    pal: Palette,
    tint: Option<Color32>,
) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) / 2.0;
    match state {
        FileState::Unknown => {
            let col = tint.unwrap_or(pal.text_weak).gamma_multiply(0.6);
            for k in 0..8 {
                let a0 = k as f32 * std::f32::consts::TAU / 8.0;
                let a1 = a0 + std::f32::consts::TAU / 16.0;
                let p = |a: f32| c + vec2(a.cos(), a.sin()) * (r * 0.55);
                painter.line_segment([p(a0), p(a1)], Stroke::new(1.0, col));
            }
        }
        FileState::Found => {
            painter.circle_filled(c, r * 0.42, tint.unwrap_or(pal.play));
        }
        FileState::Missing | FileState::DriveMissing => {
            // Amber for a drive that is not connected (plug it in and the
            // track is back), red for a file that is gone from its place.
            let col = tint.unwrap_or(if state == FileState::DriveMissing {
                pal.warn
            } else {
                pal.error
            });
            painter.circle_stroke(c, r * 0.85, Stroke::new(1.5, col));
            painter.line_segment(
                [c - vec2(0.0, r * 0.45), c + vec2(0.0, r * 0.12)],
                Stroke::new(1.6, col),
            );
            painter.circle_filled(c + vec2(0.0, r * 0.42), 1.0, col);
        }
    }
}

/// Deck for a double-clicked track: the first empty deck, else the first
/// paused one. Never a playing deck (double-click always replaced
/// the paused track on deck 1).
impl KaldorApp {
    /// The deck a double-click loads onto (see [`free_deck`]).
    pub(crate) fn free_deck_now(&self) -> Option<DeckId> {
        let snap = *self.engine.snapshot();
        // A deck whose load has started counts as loaded.
        let loaded = DeckId::ALL
            .map(|d| snap.decks[d.index()].loaded || self.decks[d.index()].job.is_some());
        let playing = DeckId::ALL.map(|d| snap.decks[d.index()].playing);
        free_deck(loaded, playing)
    }

    /// A file double-clicked in the Files panel: onto the first free deck.
    pub(crate) fn load_file_on_free_deck(&mut self, p: PathBuf) {
        match self.free_deck_now() {
            Some(deck) => self.load_path(deck, p),
            None => {
                if let Some(lib) = self.library.as_mut() {
                    lib.status = "all decks are playing: load with right-click".into();
                }
            }
        }
    }
}

fn free_deck(loaded: [bool; 4], playing: [bool; 4]) -> Option<DeckId> {
    let pick = |ok: &dyn Fn(usize) -> bool| DeckId::ALL.into_iter().find(|d| ok(d.index()));
    pick(&|i| !loaded[i]).or_else(|| pick(&|i| !playing[i]))
}

fn format_date(unix: i64) -> String {
    // Days since epoch to a civil date (proleptic Gregorian).
    let days = unix.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

fn apply(app: &mut KaldorApp, actions: Vec<Action>) {
    for a in actions {
        match a {
            Action::Load(d, id) => app.load_library_track(d, id),
            Action::LoadFree(id) => match app.free_deck_now() {
                Some(deck) => app.load_library_track(deck, id),
                None => {
                    if let Some(lib) = app.library.as_mut() {
                        lib.status = "all decks are playing: load with right-click".into();
                    }
                }
            },
            Action::ToggleColumn(c) => {
                let mut cols = LibColumn::from_config(&app.config.ui.library_columns);
                LibColumn::toggle(&mut cols, c);
                app.config.ui.library_columns = LibColumn::to_config(&cols);
                app.events.push(crate::app::AppEvent::ConfigChanged);
            }
            Action::DefaultColumns => {
                app.config.ui.library_columns = LibColumn::to_config(&LibColumn::DEFAULT);
                app.config.ui.library_column_widths.clear();
                app.events.push(crate::app::AppEvent::ConfigChanged);
            }
            Action::MoveColumn(c, to) => {
                let mut cols = LibColumn::from_config(&app.config.ui.library_columns);
                if let Some(from) = cols.iter().position(|x| *x == c) {
                    let to = if to > from { to - 1 } else { to }.min(cols.len() - 1);
                    if to != from {
                        let c = cols.remove(from);
                        cols.insert(to, c);
                        app.config.ui.library_columns = LibColumn::to_config(&cols);
                        app.events.push(crate::app::AppEvent::ConfigChanged);
                    }
                }
            }
            Action::ResizeColumn(c, w) => {
                // Saved when the drag ends (not every frame).
                app.config
                    .ui
                    .library_column_widths
                    .insert(c.id().to_string(), w.round());
            }
            Action::ResetColumnWidth(c) => {
                app.config.ui.library_column_widths.remove(c.id());
                app.events.push(crate::app::AppEvent::ConfigChanged);
            }
            Action::SaveConfig => app.events.push(crate::app::AppEvent::ConfigChanged),
            Action::Locate(t) => {
                if let Some(lib) = app.library.as_mut() {
                    lib.open_locate(t);
                }
            }
            Action::History => app.open_history(),
            Action::CloseHistory => {
                if let Some(lib) = app.library.as_mut() {
                    lib.history = None;
                }
            }
            Action::ToggleCollapsed(c) => {
                if let Some(lib) = app.library.as_mut() {
                    lib.toggle_collapsed(c);
                }
            }
            Action::CrateTab(t) => {
                if let Some(lib) = app.library.as_mut()
                    && lib.crate_tab != t
                {
                    lib.crate_tab = t;
                    lib.clear_picked_crates();
                    // The list shows the tab's first entry.
                    let first = match t {
                        crate::library::CrateTab::Crates => Selection::All,
                        crate::library::CrateTab::Rekordbox => {
                            lib.rb_root.map_or(Selection::All, Selection::Crate)
                        }
                    };
                    lib.open_crate(first);
                }
            }
            Action::RekordboxSyncNow => {
                if let Some(lib) = app.library.as_mut() {
                    lib.rekordbox.sync_now();
                }
            }
            Action::RekordboxEnable(on) => {
                app.config.library.rekordbox_sync = on;
                if let Some(lib) = app.library.as_mut() {
                    lib.rekordbox
                        .configure(on, app.config.library.rekordbox_folder.clone());
                }
                app.events.push(crate::app::AppEvent::ConfigChanged);
            }
            Action::RekordboxFolder(f) => {
                app.config.library.rekordbox_folder = f.clone();
                if let Some(lib) = app.library.as_mut() {
                    lib.rekordbox
                        .configure(app.config.library.rekordbox_sync, f);
                }
                app.events.push(crate::app::AppEvent::ConfigChanged);
            }
            Action::MoveInCrate(t, to) => {
                if let Some(lib) = app.library.as_mut()
                    && let Err(e) = lib.move_in_view(t, to)
                {
                    lib.status = e;
                }
            }
            other => {
                let Some(lib) = app.library.as_mut() else {
                    return;
                };
                let edits = matches!(
                    other,
                    Action::AddToCrate(..)
                        | Action::RemoveFromCrate(..)
                        | Action::NewCrate(_)
                        | Action::DeleteCrate(_)
                        | Action::StartRename(_)
                        | Action::CommitRename
                        | Action::Reanalyse(_)
                        | Action::AskRemove(_)
                        | Action::NewCrateWith(_)
                );
                if edits && lib.protect {
                    lib.renaming = None;
                    lib.status = "Library is protected: crates, cues and grids cannot be changed (Settings > Library)".into();
                    continue;
                }
                let result = match other {
                    Action::AddToCrate(c, t) => {
                        // The whole selection when the track is in it.
                        let ids = lib.drag_set(t);
                        let inside = lib.lib.crate_tracks(c).unwrap_or_default();
                        let new: Vec<TrackId> = ids
                            .iter()
                            .copied()
                            .filter(|x| !inside.contains(x))
                            .collect();
                        let crate_name = lib
                            .crate_list()
                            .into_iter()
                            .find(|(id, _, _)| *id == c)
                            .map(|(_, n, _)| n)
                            .unwrap_or_default();
                        let what = if ids.len() == 1 {
                            let name = lib.track(t).map(|x| x.display_name()).unwrap_or_default();
                            format!("\u{201c}{name}\u{201d}")
                        } else {
                            format!("{} tracks", ids.len())
                        };
                        lib.crate_flash = Some((c, std::time::Instant::now()));
                        if new.is_empty() {
                            lib.status = if ids.len() == 1 {
                                format!("{what} is already in {crate_name}")
                            } else {
                                format!("{what} are already in {crate_name}")
                            };
                            Ok(())
                        } else {
                            let r = lib.lib.add_to_crate(c, &new);
                            if r.is_ok() {
                                let skipped = ids.len() - new.len();
                                lib.status = if skipped == 0 {
                                    format!("Added {what} to {crate_name}")
                                } else {
                                    format!(
                                        "Added {} of {what} to {crate_name} ({skipped} were already in it)",
                                        new.len()
                                    )
                                };
                            }
                            r
                        }
                    }
                    Action::RemoveFromCrate(c, t) => {
                        let ids = lib.drag_set(t);
                        lib.clear_picked();
                        lib.selected_track = None;
                        lib.lib.remove_from_crate(c, &ids)
                    }
                    Action::AskRemove(t) => {
                        lib.confirm_remove = Some(lib.drag_set(t));
                        Ok(())
                    }
                    Action::NewCrateWith(t) => {
                        let ids = lib.drag_set(t);
                        let names: Vec<String> =
                            lib.crate_list().into_iter().map(|(_, n, _)| n).collect();
                        let name = (1..)
                            .map(|k| {
                                if k == 1 {
                                    "New crate".to_string()
                                } else {
                                    format!("New crate {k}")
                                }
                            })
                            .find(|n| !names.contains(n))
                            .unwrap_or_else(|| "New crate".into());
                        lib.lib.create_crate(&name, None).and_then(|c| {
                            lib.lib.add_to_crate(c, &ids)?;
                            lib.reload();
                            lib.crate_flash = Some((c, std::time::Instant::now()));
                            lib.status = format!(
                                "New crate with {} track{}: type its name",
                                ids.len(),
                                if ids.len() == 1 { "" } else { "s" }
                            );
                            lib.start_rename(c);
                            Ok(())
                        })
                    }
                    Action::ImportFiles(paths, target) => {
                        lib.import_into(&paths, target);
                        Ok(())
                    }
                    Action::NewCrate(parent) => {
                        // "New crate" (or "New crate 2", ...), then rename it
                        // in place: the name is selected for typing.
                        let names: Vec<String> =
                            lib.crate_list().into_iter().map(|(_, n, _)| n).collect();
                        let name = (1..)
                            .map(|k| {
                                if k == 1 {
                                    "New crate".to_string()
                                } else {
                                    format!("New crate {k}")
                                }
                            })
                            .find(|n| !names.contains(n))
                            .unwrap_or_else(|| "New crate".into());
                        // A parent with tracks becomes a folder: its tracks
                        // go into a sub-crate of its name (said below).
                        let moved = parent.filter(|p| {
                            !lib.has_sub_crates(*p)
                                && !lib.lib.crate_tracks(*p).unwrap_or_default().is_empty()
                        });
                        lib.lib.create_crate(&name, parent).map(|id| {
                            lib.reload();
                            lib.selection = Selection::Crate(id);
                            lib.start_rename(id);
                            if let Some(p) = moved {
                                let pname = lib
                                    .crate_list()
                                    .into_iter()
                                    .find(|c| c.0 == p)
                                    .map(|c| c.1)
                                    .unwrap_or_default();
                                lib.status = format!(
                                    "{pname} is a folder now: its tracks went into the sub-crate \"{pname}\""
                                );
                            }
                        })
                    }
                    Action::DeleteCrate(c) => {
                        if lib.selection == Selection::Crate(c) {
                            lib.selection = Selection::All;
                        }
                        lib.lib.delete_crate(c)
                    }
                    Action::StartRename(c) => {
                        lib.start_rename(c);
                        Ok(())
                    }
                    Action::CommitRename => {
                        lib.commit_rename();
                        Ok(())
                    }
                    Action::CancelRename => {
                        lib.renaming = None;
                        Ok(())
                    }
                    Action::Reanalyse(t) => {
                        for id in lib.drag_set(t) {
                            lib.reanalyse(id);
                        }
                        Ok(())
                    }
                    Action::Export(target) => {
                        lib.usb.open_for(target);
                        Ok(())
                    }
                    Action::ImportFrom(s) => {
                        lib.foreign.ask_file(s);
                        Ok(())
                    }
                    Action::SavePlaylist(c, name) => {
                        lib.foreign.ask_save(c, &name);
                        Ok(())
                    }
                    Action::NewSmartCrate(parent) => {
                        lib.smart_edit = Some(crate::smart_view::SmartEdit::new(parent));
                        Ok(())
                    }
                    Action::EditSmartCrate(c, name) => {
                        lib.smart_edit = crate::smart_view::SmartEdit::open(&lib.lib, c, &name);
                        Ok(())
                    }
                    Action::Select(s) => {
                        lib.clear_picked_crates();
                        lib.open_crate(s);
                        Ok(())
                    }
                    Action::ClickCrate(c, ctrl, shift) => {
                        lib.click_crate(c, ctrl, shift);
                        Ok(())
                    }
                    Action::Sort(k) => {
                        lib.sort = if lib.sort.0 == k {
                            (k, !lib.sort.1)
                        } else {
                            (k, true)
                        };
                        Ok(())
                    }
                    Action::Load(..)
                    | Action::LoadFree(_)
                    | Action::ToggleColumn(_)
                    | Action::DefaultColumns
                    | Action::MoveColumn(..)
                    | Action::ResizeColumn(..)
                    | Action::ResetColumnWidth(_)
                    | Action::SaveConfig
                    | Action::Locate(_)
                    | Action::History
                    | Action::CloseHistory
                    | Action::ToggleCollapsed(_)
                    | Action::CrateTab(_)
                    | Action::RekordboxSyncNow
                    | Action::RekordboxEnable(_)
                    | Action::RekordboxFolder(_)
                    | Action::MoveInCrate(..) => Ok(()),
                };
                if let Err(e) = result {
                    lib.status = format!("{e}");
                }
                lib.reload();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use dj_core::DeckId;

    #[test]
    fn double_click_picks_an_empty_deck_then_a_paused_one_never_a_playing_one() {
        use super::free_deck;
        let d = DeckId::ALL;
        // Deck 1 loaded and paused, deck 2 empty: deck 2.
        assert_eq!(
            free_deck([true, false, false, false], [false; 4]),
            Some(d[1])
        );
        // All loaded, deck 1 and 2 playing: deck 3.
        assert_eq!(free_deck([true; 4], [true, true, false, false]), Some(d[2]));
        // All playing: none.
        assert_eq!(free_deck([true; 4], [true; 4]), None);
        // Nothing loaded: deck 1.
        assert_eq!(free_deck([false; 4], [false; 4]), Some(d[0]));
    }

    #[test]
    fn default_columns_match_the_config_default() {
        use crate::library::LibColumn;
        let cfg = dj_core::config::UiConfig::default();
        assert_eq!(
            LibColumn::from_config(&cfg.library_columns),
            LibColumn::DEFAULT.to_vec()
        );
        assert_eq!(
            LibColumn::to_config(&LibColumn::DEFAULT),
            cfg.library_columns
        );
    }

    #[test]
    fn switching_columns_keeps_their_place_and_one_column() {
        use crate::library::LibColumn as C;
        let mut cols = vec![C::Artist, C::Title];
        C::toggle(&mut cols, C::Number);
        C::toggle(&mut cols, C::Bpm);
        C::toggle(&mut cols, C::Location);
        assert_eq!(cols, [C::Number, C::Artist, C::Title, C::Bpm, C::Location]);
        for c in [C::Number, C::Artist, C::Bpm, C::Location, C::Title] {
            C::toggle(&mut cols, c);
        }
        assert_eq!(cols, [C::Title], "the last column stays");
        // Unknown names and repeats are skipped; nothing left: defaults.
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            C::from_config(&ids(&["title", "x", "title", "bpm"])),
            [C::Title, C::Bpm]
        );
        assert_eq!(C::from_config(&ids(&["x"])), C::DEFAULT.to_vec());
    }

    #[test]
    fn column_widths_fill_the_list_and_hide_optional_columns_first() {
        use super::column_widths;
        use crate::library::LibColumn as C;
        let none = std::collections::BTreeMap::new();
        let cols = C::DEFAULT.to_vec();
        let at = |c: C| cols.iter().position(|x| *x == c).unwrap();
        // Wide: every column, the room goes to Artist and Title.
        let w = column_widths(&cols, &none, 1600.0);
        assert!((w.iter().sum::<f32>() - 1600.0).abs() < 0.5);
        assert!(w.iter().all(|x| *x > 0.0));
        assert!(w[at(C::Title)] > C::Title.width() && w[at(C::Artist)] > C::Artist.width());
        assert_eq!(w[at(C::Number)], C::Number.width());
        assert_eq!(w[at(C::Waveform)], C::Waveform.width());
        // Narrow: Added, Genre, Album, then the waveform go (in that
        // order); #, the icon, BPM, Key and Time keep their width.
        let w = column_widths(&cols, &none, 700.0);
        for c in [C::Added, C::Genre, C::Album, C::Waveform] {
            assert_eq!(w[at(c)], 0.0, "{c:?} hidden");
        }
        for c in [C::Number, C::Found, C::Bpm, C::Key, C::Time] {
            assert_eq!(w[at(c)], c.width(), "{c:?} keeps its width");
        }
        let w = column_widths(&cols, &none, 950.0);
        assert!(w[at(C::Album)] > 0.0 && w[at(C::Waveform)] > 0.0, "{w:?}");
        assert!(w[at(C::Genre)] == 0.0 && w[at(C::Added)] == 0.0, "{w:?}");
        // No text column: the room is shared.
        let w = column_widths(&[C::Number, C::Bpm], &none, 400.0);
        assert!((w.iter().sum::<f32>() - 400.0).abs() < 0.5);
    }

    #[test]
    fn dragged_widths_are_kept_and_the_rest_fits_around_them() {
        use super::column_widths;
        use crate::library::LibColumn as C;
        let cols = C::DEFAULT.to_vec();
        let mut set = std::collections::BTreeMap::new();
        set.insert("title".to_string(), 300.0);
        set.insert("bpm".to_string(), 90.0);
        let at = |c: C| cols.iter().position(|x| *x == c).unwrap();
        let w = column_widths(&cols, &set, 1600.0);
        assert_eq!(w[at(C::Title)], 300.0, "Title as dragged");
        assert_eq!(w[at(C::Bpm)], 90.0, "BPM as dragged");
        // Artist takes all the room left.
        assert!((w.iter().sum::<f32>() - 1600.0).abs() < 0.5);
        assert!(w[at(C::Artist)] > 450.0, "{w:?}");
        // Every column set by hand: the room stays empty at the right.
        let all: std::collections::BTreeMap<String, f32> =
            cols.iter().map(|c| (c.id().to_string(), 50.0)).collect();
        let w = column_widths(&cols, &all, 1600.0);
        assert!(w.iter().all(|x| *x == 50.0));
        // Too wide by hand: everything shrinks to fit.
        let mut wide = std::collections::BTreeMap::new();
        wide.insert("title".to_string(), 2000.0);
        let w = column_widths(&cols, &wide, 900.0);
        // (A few points over at most: no column goes under 16.)
        assert!(w.iter().sum::<f32>() <= 920.0, "{w:?}");
        assert!(w[at(C::Title)] > 500.0, "{w:?}");
    }

    #[test]
    fn cells() {
        use super::cell_text;
        use crate::library::LibColumn as C;
        let t = dj_library::Track {
            id: 7,
            meta: dj_library::TrackMeta {
                path: std::path::PathBuf::from("music").join("set").join("a.mp3"),
                artist: "A".into(),
                year: Some(1999),
                bitrate: Some(320),
                ..Default::default()
            },
            bpm: Some(128.0),
            key: None,
            lufs: None,
            gain_db: None,
            rating: 0,
            color: None,
            play_count: 3,
            date_added: 0,
            missing: false,
            analysis_version: None,
        };
        let n = dj_core::KeyNotation::default();
        assert_eq!(cell_text(C::Number, &t, 42, n), "42");
        assert_eq!(
            cell_text(C::Title, &t, 0, n),
            "a",
            "file name when untagged"
        );
        assert_eq!(cell_text(C::Bpm, &t, 0, n), "128.00");
        assert_eq!(cell_text(C::Year, &t, 0, n), "1999");
        assert_eq!(cell_text(C::Bitrate, &t, 0, n), "320 kbps");
        assert_eq!(cell_text(C::Plays, &t, 0, n), "3");
        assert_eq!(cell_text(C::FileName, &t, 0, n), "a.mp3");
        assert_eq!(
            cell_text(C::Location, &t, 0, n),
            std::path::Path::new("music")
                .join("set")
                .display()
                .to_string()
        );
    }

    #[test]
    fn dates() {
        assert_eq!(super::format_date(0), "1970-01-01");
        assert_eq!(super::format_date(1_762_819_200), "2025-11-11");
    }
}
