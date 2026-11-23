//! The Files panel: the computer's folders inside the library (no separate
//! window). Folders on the left (places and drives), the folder's
//! subfolders and audio files on the right. Drag files or folders onto a
//! crate (added to it), onto the empty space under the crates (a new crate
//! named after the folder), onto All tracks or the track list, or a file
//! onto a deck. While it is open the library gets more room; when the
//! screen is too small for four decks as well, two decks are shown until
//! it closes.

use crate::app::KaldorApp;
use crate::layout::{self, LayoutInput};
use crate::theme::{self, Palette};
use crate::widgets::{self, flat_button};
use dj_core::DeckId;
use egui::{Align, Color32, FontId, Id, Layout, Rect, RichText, Sense, Stroke, pos2, vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Drag-and-drop payload: files and folders from the Files panel.
#[derive(Clone, Debug)]
pub struct FileDrag(pub Vec<PathBuf>);

/// Height the library wants while the Files panel is open (points): the
/// toolbar, the panel and a few rows of the library under it.
const FILES_LIBRARY_H: f32 = 500.0;
/// Below this a waveform strip is too short: show two decks instead.
const MIN_STRIP_H: f32 = 44.0;
/// A folder's contents are read again after this long.
const REREAD: Duration = Duration::from_secs(3);

/// One folder's contents.
#[derive(Clone, Debug, Default)]
struct Listing {
    dirs: Vec<PathBuf>,
    /// Audio files and their sizes.
    files: Vec<(PathBuf, u64)>,
    error: Option<String>,
    at: Option<Instant>,
}

/// The Files panel's state.
#[derive(Default)]
pub struct FilesPanel {
    pub open: bool,
    /// The folder shown on the right.
    pub folder: Option<PathBuf>,
    expanded: HashSet<PathBuf>,
    cache: HashMap<PathBuf, Listing>,
    /// Selected entries on the right (Ctrl / Shift + click).
    pub selected: Vec<PathBuf>,
    anchor: Option<PathBuf>,
    roots: Vec<(String, PathBuf)>,
    /// Two decks are shown because Files needs the room.
    pub two_decks: bool,
}

/// Something the panel asks for; applied after drawing.
enum FileAction {
    Open(PathBuf),
    Toggle(PathBuf),
    Up,
    Refresh,
    Close,
    LoadFree(PathBuf),
    Load(DeckId, PathBuf),
    Import(Vec<PathBuf>),
    NewCrate(PathBuf),
    Click(PathBuf, bool, bool),
}

fn hidden(name: &str) -> bool {
    name.starts_with('.')
        || name.eq_ignore_ascii_case("$RECYCLE.BIN")
        || name.eq_ignore_ascii_case("System Volume Information")
}

/// Reads a folder: subfolders and audio files, by name (any case).
fn read_listing(dir: &Path) -> Listing {
    let mut out = Listing {
        at: Some(Instant::now()),
        ..Default::default()
    };
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => {
            out.error = Some(e.to_string());
            return out;
        }
    };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if hidden(&name) {
            continue;
        }
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => out.dirs.push(p),
            Ok(_) if dj_library::is_audio_file(&p) => {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.files.push((p, size));
            }
            _ => {}
        }
    }
    let key = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    out.dirs.sort_by_key(|p| key(p));
    out.files.sort_by_key(|(p, _)| key(p));
    out
}

/// Where to start: the music, home, desktop and downloads folders, then
/// every drive or mounted volume.
fn find_roots() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .filter(|h| h.is_dir());
    if let Some(h) = &home {
        for (label, sub) in [
            ("Music", "Music"),
            ("Desktop", "Desktop"),
            ("Downloads", "Downloads"),
        ] {
            let p = h.join(sub);
            if p.is_dir() {
                out.push((label.to_string(), p));
            }
        }
        out.push(("Home".to_string(), h.clone()));
    }
    if cfg!(windows) {
        for c in b'C'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", c as char));
            if p.is_dir() {
                // "Music SSD (D:)", or "USB drive (E:)" without a name.
                let name = dj_export::volume::display_name(&p)
                    .unwrap_or_else(|| format!("{}:", c as char));
                out.push((name, p));
            }
        }
    } else {
        out.push(("Computer".to_string(), PathBuf::from("/")));
        let user = std::env::var("USER").unwrap_or_default();
        let mounts = [
            PathBuf::from("/Volumes"),
            PathBuf::from("/media").join(&user),
            PathBuf::from("/run/media").join(&user),
            PathBuf::from("/mnt"),
        ];
        for m in mounts {
            if let Ok(rd) = std::fs::read_dir(&m) {
                for e in rd.flatten() {
                    let p = e.path();
                    if p.is_dir() {
                        let name = e.file_name().to_string_lossy().into_owned();
                        out.push((name, p));
                    }
                }
            }
        }
    }
    out
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.display().to_string())
}

fn size_text(bytes: u64) -> String {
    let mb = bytes as f64 / 1_048_576.0;
    if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

impl FilesPanel {
    /// Opens or closes the panel (opening reads the drives again).
    pub fn toggle(&mut self, start: Option<PathBuf>) {
        self.open = !self.open;
        if self.open {
            self.roots = find_roots();
            if self.folder.is_none() {
                self.folder = start
                    .filter(|p| p.is_dir())
                    .or_else(|| self.roots.first().map(|r| r.1.clone()));
            }
            if let Some(f) = self.folder.clone() {
                self.reveal(&f);
            }
        }
    }

    /// Expands the tree down to `p`.
    fn reveal(&mut self, p: &Path) {
        for a in p.ancestors().skip(1) {
            self.expanded.insert(a.to_path_buf());
        }
    }

    fn listing(&mut self, dir: &Path) -> &Listing {
        let stale = self
            .cache
            .get(dir)
            .and_then(|l| l.at)
            .is_none_or(|t| t.elapsed() > REREAD);
        if stale {
            self.cache.insert(dir.to_path_buf(), read_listing(dir));
        }
        &self.cache[dir]
    }

    /// The entries on the right, folders first.
    fn entries(&mut self) -> Vec<(PathBuf, Option<u64>)> {
        let Some(dir) = self.folder.clone() else {
            return Vec::new();
        };
        let l = self.listing(&dir);
        l.dirs
            .iter()
            .map(|d| (d.clone(), None))
            .chain(l.files.iter().map(|(f, s)| (f.clone(), Some(*s))))
            .collect()
    }

    /// A click on an entry: plain, Ctrl / Cmd (add or take away) or Shift
    /// (a range from the last click).
    pub fn click(&mut self, p: PathBuf, ctrl: bool, shift: bool) {
        if shift && let Some(a) = self.anchor.clone() {
            let all: Vec<PathBuf> = self.entries().into_iter().map(|e| e.0).collect();
            if let (Some(i), Some(j)) = (
                all.iter().position(|x| *x == a),
                all.iter().position(|x| *x == p),
            ) {
                self.selected = all[i.min(j)..=i.max(j)].to_vec();
                return;
            }
        }
        if ctrl {
            if let Some(k) = self.selected.iter().position(|x| *x == p) {
                self.selected.remove(k);
            } else {
                self.selected.push(p.clone());
            }
        } else {
            self.selected = vec![p.clone()];
        }
        self.anchor = Some(p);
    }

    /// What a drag of `p` carries: the selection when `p` is in it.
    fn drag_set(&self, p: &Path) -> Vec<PathBuf> {
        if self.selected.iter().any(|s| s == p) {
            self.selected.clone()
        } else {
            vec![p.to_path_buf()]
        }
    }

    /// Shows `dir` on the right.
    pub fn open_folder(&mut self, dir: PathBuf) {
        self.reveal(&dir);
        self.expanded.insert(dir.clone());
        self.cache.remove(&dir);
        self.folder = Some(dir);
        self.selected.clear();
        self.anchor = None;
    }
}

/// The layout while the Files panel is open: the library gets at least
/// [`FILES_LIBRARY_H`]; when four decks then no longer fit (waveform strips
/// too short), two decks are shown. Returns true when it switched to two.
pub(crate) fn make_room(input: &mut LayoutInput) -> bool {
    let h = input.size.y.max(1.0);
    input.library_fraction = input.library_fraction.max((FILES_LIBRARY_H / h).min(0.65));
    if input.two_decks || input.layout == dj_core::config::DeckLayout::TwoDecks {
        return false;
    }
    let f = layout::compute(input);
    let cramped = f.strips.is_empty()
        || f.strips.iter().any(|s| s.rect.height() < MIN_STRIP_H)
        || f.panels.iter().any(|p| p.size == layout::PanelSize::Mini);
    if cramped {
        input.two_decks = true;
    }
    cramped
}

/// A folder (`open`) or a note (a file) in front of a name.
fn paint_kind(painter: &egui::Painter, r: Rect, folder: bool, color: Color32) {
    let c = r.center();
    if folder {
        let body =
            Rect::from_center_size(c + vec2(0.0, 1.0), vec2(r.width() * 0.9, r.height() * 0.62));
        let tab = Rect::from_min_size(
            pos2(body.left(), body.top() - r.height() * 0.14),
            vec2(body.width() * 0.42, r.height() * 0.18),
        );
        painter.rect_filled(tab, 1.0, color);
        painter.rect_filled(body, 1.5, color);
    } else {
        let s = r.height() * 0.5;
        painter.circle_filled(c + vec2(-s * 0.35, s * 0.45), s * 0.32, color);
        painter.line_segment(
            [c + vec2(-s * 0.05, s * 0.45), c + vec2(-s * 0.05, -s * 0.7)],
            Stroke::new(1.5, color),
        );
        painter.line_segment(
            [c + vec2(-s * 0.05, -s * 0.7), c + vec2(s * 0.55, -s * 0.45)],
            Stroke::new(1.5, color),
        );
    }
}

/// The Files panel in `rect` (over the library's top part).
pub(crate) fn files_panel(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let mut actions: Vec<FileAction> = Vec::new();
    let mut marks: Vec<(&'static str, Rect, bool)> = Vec::new();
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
    marks.push(("files_panel", rect, false));

    // Header: up, the folder, the hint, refresh and close.
    let fp = &mut app.files_panel;
    let mut x = head.left();
    let mut small = |ui: &mut egui::Ui, id: &str, label: &str, tip: &str, w: f32| {
        let r = Rect::from_min_size(pos2(x, head.top()), vec2(w, head.height()));
        x += w + 4.0;
        let b = flat_button(
            ui,
            r,
            Id::unique(("files_btn", id)),
            label,
            theme::prop(12.5),
            None,
            false,
            pal.accent,
            false,
        )
        .on_hover_text(tip);
        (b, r)
    };
    let (up, up_r) = small(ui, "up", "Up", "Up one folder", 34.0);
    if up.clicked() {
        actions.push(FileAction::Up);
    }
    marks.push(("files_up", up_r, false));
    let close_r = Rect::from_min_size(
        pos2(head.right() - 26.0, head.top()),
        vec2(26.0, head.height()),
    );
    let close = flat_button(
        ui,
        close_r,
        Id::unique("files_close"),
        "×",
        theme::prop(14.0),
        None,
        false,
        pal.accent,
        false,
    )
    .on_hover_text("Close Files");
    if close.clicked() {
        actions.push(FileAction::Close);
    }
    marks.push(("files_close", close_r, false));
    let refresh_r = Rect::from_min_size(
        pos2(close_r.left() - 70.0, head.top()),
        vec2(66.0, head.height()),
    );
    let refresh = flat_button(
        ui,
        refresh_r,
        Id::unique("files_refresh"),
        "Refresh",
        theme::prop(12.0),
        None,
        false,
        pal.accent,
        false,
    )
    .on_hover_text("Read the drives and this folder again");
    if refresh.clicked() {
        actions.push(FileAction::Refresh);
    }
    let path_r = Rect::from_min_max(
        pos2(x + 4.0, head.top()),
        pos2(refresh_r.left() - 8.0, head.bottom()),
    );
    let hint = if fp.two_decks {
        "Drag onto a crate, or under the crates for a new crate  ·  2 decks while Files is open"
    } else {
        "Drag onto a crate, or under the crates for a new crate"
    };
    let folder_text = fp
        .folder
        .as_ref()
        .map(|f| f.display().to_string())
        .unwrap_or_default();
    let painter = ui.painter_at(path_r);
    let hint_g = painter.layout_no_wrap(hint.to_string(), theme::prop(11.5), pal.text_weak);
    let hint_w = hint_g.size().x;
    let show_hint = path_r.width() > hint_w + 160.0;
    let text_r = if show_hint {
        Rect::from_min_max(
            path_r.min,
            pos2(path_r.right() - hint_w - 12.0, path_r.bottom()),
        )
    } else {
        path_r
    };
    widgets::text_in(
        &painter,
        text_r,
        &folder_text,
        theme::prop(13.0),
        pal.text,
        false,
    );
    if show_hint {
        painter.galley(
            pos2(
                path_r.right() - hint_w,
                path_r.center().y - hint_g.size().y / 2.0,
            ),
            hint_g,
            pal.text_weak,
        );
    }
    marks.push(("files_path", path_r, false));

    // Left: places and drives, with the folders that are open.
    let tree_w = (body.width() * 0.3).clamp(170.0, 320.0);
    let tree_r = Rect::from_min_size(body.min, vec2(tree_w, body.height()));
    let list_r = Rect::from_min_max(pos2(tree_r.right() + 6.0, body.top()), body.max);
    ui.painter().line_segment(
        [
            pos2(tree_r.right() + 3.0, body.top()),
            pos2(tree_r.right() + 3.0, body.bottom()),
        ],
        Stroke::new(1.0, pal.border),
    );
    let roots = fp.roots.clone();
    let mut tu = crate::app::region_ui(ui, tree_r, Layout::top_down(Align::Min), "files_tree");
    egui::ScrollArea::vertical()
        .id_salt("files_tree_scroll")
        .auto_shrink([false, false])
        .show(&mut tu, |ui| {
            for (label, p) in &roots {
                tree_node(ui, fp, p, Some(label), 0, pal, &mut actions, &mut marks);
            }
        });

    // Right: the folder's subfolders and audio files.
    let entries = fp.entries();
    let error = fp.folder.clone().and_then(|d| fp.listing(&d).error.clone());
    let mut lu = crate::app::region_ui(ui, list_r, Layout::top_down(Align::Min), "files_list");
    let row_h = 20.0;
    let in_lib: Vec<bool> = {
        let lib = app.library.as_ref();
        entries
            .iter()
            .map(|(p, _)| lib.is_some_and(|l| l.track_of_path(p).is_some()))
            .collect()
    };
    let fp = &mut app.files_panel;
    if entries.is_empty() {
        let msg = match &error {
            Some(e) => format!("Cannot open this folder: {e}"),
            None if fp.folder.is_none() => "Pick a folder on the left".into(),
            None => "No folders or audio files here".into(),
        };
        lu.add_space(8.0);
        lu.label(RichText::new(msg).color(pal.text_weak));
    }
    let sel_bg = lu.visuals().selection.bg_fill;
    let hover_bg = lu.visuals().widgets.hovered.weak_bg_fill;
    egui::ScrollArea::vertical()
        .id_salt("files_list_scroll")
        .auto_shrink([false, false])
        .show_rows(&mut lu, row_h, entries.len(), |ui, range| {
            for i in range {
                let (p, size) = &entries[i];
                let folder = size.is_none();
                let (r, resp) = ui.allocate_exact_size(
                    vec2(ui.available_width(), row_h),
                    Sense::click_and_drag(),
                );
                if resp.drag_started() || resp.dragged() {
                    resp.dnd_set_drag_payload(FileDrag(fp.drag_set(p)));
                }
                let painter = ui.painter_at(r);
                let picked = fp.selected.contains(p);
                if picked {
                    painter.rect_filled(r, 0.0, sel_bg);
                } else if resp.hovered() {
                    painter.rect_filled(r, 0.0, hover_bg);
                } else if i % 2 == 1 {
                    painter.rect_filled(r, 0.0, pal.panel.gamma_multiply(0.5));
                }
                let icon =
                    Rect::from_min_size(pos2(r.left() + 4.0, r.top() + 3.0), vec2(14.0, 14.0));
                paint_kind(
                    &painter,
                    icon,
                    folder,
                    if folder { pal.cue } else { pal.text_weak },
                );
                let size_w = 80.0;
                let name_r = Rect::from_min_max(
                    pos2(icon.right() + 6.0, r.top()),
                    pos2(r.right() - size_w - 24.0, r.bottom()),
                );
                widgets::text_in(
                    &painter,
                    name_r,
                    &name_of(p),
                    FontId::proportional(13.0),
                    pal.text,
                    false,
                );
                if let Some(s) = size {
                    let sr = Rect::from_min_max(
                        pos2(r.right() - size_w - 20.0, r.top()),
                        pos2(r.right() - 20.0, r.bottom()),
                    );
                    widgets::text_in(
                        &painter,
                        sr,
                        &size_text(*s),
                        FontId::proportional(12.0),
                        pal.text_weak,
                        true,
                    );
                }
                // A green dot: this file is in the library.
                if in_lib[i] {
                    painter.circle_filled(pos2(r.right() - 9.0, r.center().y), 3.0, pal.play);
                }
                let resp = if in_lib[i] {
                    resp.on_hover_text("In the library")
                } else {
                    resp
                };
                if i == 0 {
                    marks.push(("files_row", r, true));
                }
                let mods = ui.input(|i| i.modifiers);
                if resp.clicked() {
                    actions.push(FileAction::Click(
                        p.clone(),
                        mods.command || mods.ctrl,
                        mods.shift,
                    ));
                }
                if resp.secondary_clicked() && !picked {
                    actions.push(FileAction::Click(p.clone(), false, false));
                }
                if resp.double_clicked() {
                    actions.push(if folder {
                        FileAction::Open(p.clone())
                    } else {
                        FileAction::LoadFree(p.clone())
                    });
                }
                let set = fp.drag_set(p);
                resp.context_menu(|ui| {
                    if folder {
                        if ui.button("Open").clicked() {
                            actions.push(FileAction::Open(p.clone()));
                            ui.close();
                        }
                        if ui.button("New crate from this folder").clicked() {
                            actions.push(FileAction::NewCrate(p.clone()));
                            ui.close();
                        }
                    } else {
                        for d in DeckId::ALL {
                            if ui.button(format!("Load on deck {}", d.number())).clicked() {
                                actions.push(FileAction::Load(d, p.clone()));
                                ui.close();
                            }
                        }
                        ui.separator();
                    }
                    let what = if set.len() > 1 {
                        format!("Add {} to the library", set.len())
                    } else {
                        "Add to the library".to_string()
                    };
                    if ui.button(what).clicked() {
                        actions.push(FileAction::Import(set.clone()));
                        ui.close();
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

/// One folder in the tree, and its open subfolders.
#[allow(clippy::too_many_arguments)]
fn tree_node(
    ui: &mut egui::Ui,
    fp: &mut FilesPanel,
    p: &Path,
    label: Option<&str>,
    depth: usize,
    pal: Palette,
    actions: &mut Vec<FileAction>,
    marks: &mut Vec<(&'static str, Rect, bool)>,
) {
    let open = fp.expanded.contains(p);
    let current = fp.folder.as_deref() == Some(p);
    let row_h = 20.0;
    let (r, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), row_h), Sense::click_and_drag());
    if resp.drag_started() || resp.dragged() {
        resp.dnd_set_drag_payload(FileDrag(vec![p.to_path_buf()]));
    }
    let painter = ui.painter_at(r);
    if current {
        painter.rect_filled(r, 3.0, ui.visuals().selection.bg_fill);
    } else if resp.hovered() {
        painter.rect_filled(r, 3.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    let x0 = r.left() + 4.0 + depth as f32 * 14.0;
    let chev = Rect::from_min_size(pos2(x0, r.top() + 4.0), vec2(12.0, 12.0));
    widgets::paint_icon(
        &painter,
        chev,
        if open {
            widgets::Icon::ChevronDown
        } else {
            widgets::Icon::ChevronRight
        },
        pal.text_weak,
    );
    let icon = Rect::from_min_size(pos2(chev.right() + 4.0, r.top() + 3.0), vec2(14.0, 14.0));
    paint_kind(&painter, icon, true, pal.cue);
    let name = label.map(str::to_string).unwrap_or_else(|| name_of(p));
    let text_r = Rect::from_min_max(pos2(icon.right() + 6.0, r.top()), r.max);
    widgets::text_in(
        &painter,
        text_r,
        &name,
        FontId::proportional(13.0),
        pal.text,
        false,
    );
    if depth == 0 {
        marks.push(("files_place", r, true));
    }
    // The arrow opens or closes the folder in the tree; the name shows it
    // on the right (and opens it in the tree).
    if resp.clicked() {
        let on_chevron = resp
            .interact_pointer_pos()
            .is_some_and(|q| q.x < chev.right() + 2.0);
        actions.push(if on_chevron {
            FileAction::Toggle(p.to_path_buf())
        } else {
            FileAction::Open(p.to_path_buf())
        });
    }
    if open {
        let dirs = fp.listing(p).dirs.clone();
        for d in dirs {
            tree_node(ui, fp, &d, None, depth + 1, pal, actions, marks);
        }
    }
}

fn apply(app: &mut KaldorApp, actions: Vec<FileAction>) {
    for a in actions {
        match a {
            FileAction::Open(p) => {
                app.files_panel.open_folder(p.clone());
                app.config.ui.files_folder = Some(p);
                app.events.push(crate::app::AppEvent::ConfigChanged);
            }
            FileAction::Toggle(p) => {
                let fp = &mut app.files_panel;
                if !fp.expanded.remove(&p) {
                    fp.expanded.insert(p.clone());
                    fp.cache.remove(&p);
                }
            }
            FileAction::Up => {
                if let Some(parent) = app
                    .files_panel
                    .folder
                    .as_ref()
                    .and_then(|f| f.parent())
                    .map(Path::to_path_buf)
                {
                    app.files_panel.open_folder(parent);
                }
            }
            FileAction::Refresh => {
                let fp = &mut app.files_panel;
                fp.roots = find_roots();
                fp.cache.clear();
            }
            FileAction::Close => app.files_panel.open = false,
            FileAction::Click(p, ctrl, shift) => app.files_panel.click(p, ctrl, shift),
            FileAction::LoadFree(p) => app.load_file_on_free_deck(p),
            FileAction::Load(d, p) => app.load_path(d, p),
            FileAction::Import(paths) => {
                if let Some(lib) = app.library.as_mut() {
                    lib.import_into(&paths, crate::library::ImportTarget::Library);
                }
            }
            FileAction::NewCrate(dir) => {
                if let Some(lib) = app.library.as_mut() {
                    lib.import_into(
                        std::slice::from_ref(&dir),
                        crate::library::ImportTarget::NewCrate(name_of(&dir)),
                    );
                }
            }
        }
    }
}

impl KaldorApp {
    /// Opens the Files panel on `dir` with `dir` as its only place (tests
    /// and screenshots: nothing outside it is listed). The toolbar's Files
    /// button opens it on the last folder, with every place and drive.
    pub fn open_files_at(&mut self, dir: PathBuf) {
        let fp = &mut self.files_panel;
        fp.open = true;
        fp.roots = vec![(name_of(&dir), dir.clone())];
        fp.open_folder(dir);
    }

    /// The Files panel is open, and whether it switched to two decks.
    pub fn files_state(&self) -> (bool, bool) {
        (self.files_panel.open, self.files_panel.two_decks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_lists_subfolders_and_audio_files_only() {
        let dir = crate::test_dir("kaldor-files");
        std::fs::create_dir_all(dir.join("B sub")).unwrap();
        std::fs::create_dir_all(dir.join("a sub")).unwrap();
        std::fs::create_dir_all(dir.join(".hidden")).unwrap();
        std::fs::write(dir.join("Track.MP3"), b"x").unwrap();
        std::fs::write(dir.join("cover.jpg"), b"x").unwrap();
        std::fs::write(dir.join("b.wav"), b"xx").unwrap();
        let l = read_listing(&dir);
        let names = |v: &[PathBuf]| v.iter().map(|p| name_of(p)).collect::<Vec<_>>();
        assert_eq!(names(&l.dirs), ["a sub", "B sub"]);
        let files: Vec<PathBuf> = l.files.iter().map(|f| f.0.clone()).collect();
        assert_eq!(names(&files), ["b.wav", "Track.MP3"]);
        assert_eq!(l.files[0].1, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_small_screen_gets_two_decks_while_files_is_open() {
        use dj_core::config::DeckLayout;
        let mut big = LayoutInput {
            size: vec2(1920.0, 1080.0),
            layout: DeckLayout::Extended,
            two_decks: false,
            layer: [0, 1],
            mixer_open: false,
            mixer_swap: false,
            library_fraction: 0.3,
            fx_panel: false,
        };
        let mut small = LayoutInput {
            size: vec2(1366.0, 700.0),
            ..big
        };
        let switched_big = make_room(&mut big);
        let switched_small = make_room(&mut small);
        assert!(big.library_fraction * 1080.0 >= FILES_LIBRARY_H - 1.0);
        assert!(!switched_big, "1080p keeps four decks");
        assert!(
            switched_small && small.two_decks,
            "a small screen shows two"
        );
    }
}
