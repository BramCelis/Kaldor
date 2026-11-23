//! Pointing tracks whose file moved at the new place, keeping their cues,
//! loops, grid, crates and play count. One window, one field: a file
//! (browsed, dropped or pasted) relinks the track and then looks for the
//! other missing tracks next to it; a folder is searched for every missing
//! track (same file name and size).

use crate::app::KaldorApp;
use crate::file_check::FileState;
use crate::library::LibraryUi;
use crate::settings_ui::button;
use crate::theme::Palette;
use dj_library::TrackId;
use egui::{Color32, RichText};
use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;

/// The Locate window for one track.
#[derive(Clone, Debug, Default)]
pub struct Locate {
    pub track: TrackId,
    /// A file or folder as typed, pasted or dropped.
    pub path: String,
    /// The last result (or why it did not work), shown in the window.
    pub message: String,
    /// The track is linked to its file again: the window shows the result
    /// until Done.
    pub done: bool,
    /// What the search for the other missing tracks found (after a file).
    pub others: String,
}

/// What a search found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MovedResult {
    /// Tracks and their new files (one match each).
    pub found: Vec<(TrackId, PathBuf)>,
    /// Tracks whose file was not there when the search started.
    pub missing: usize,
    /// Tracks with more than one file of the same name and size (left
    /// alone: you pick those by hand).
    pub ambiguous: usize,
}

/// A search running in the background.
pub struct MovedSearch {
    rx: mpsc::Receiver<MovedResult>,
    pub folder: PathBuf,
    /// Set when the search follows a relinked file: the message about it.
    after_relink: Option<String>,
}

/// The tracks whose file is gone, matched against `files` by file name
/// (ignoring case) and, when the size is known, the size.
pub fn match_moved(missing: &[(TrackId, PathBuf, u64)], files: &[(PathBuf, u64)]) -> MovedResult {
    let key = |p: &Path| p.file_name().map(|n| n.to_string_lossy().to_lowercase());
    let mut by_name: HashMap<String, Vec<&(PathBuf, u64)>> = HashMap::new();
    for f in files {
        if let Some(k) = key(&f.0) {
            by_name.entry(k).or_default().push(f);
        }
    }
    let mut used: HashSet<&Path> = HashSet::new();
    let mut out = MovedResult {
        missing: missing.len(),
        ..Default::default()
    };
    for (id, old, size) in missing {
        let Some(cands) = key(old).and_then(|k| by_name.get(&k)) else {
            continue;
        };
        let cands: Vec<&&(PathBuf, u64)> = cands
            .iter()
            .filter(|(_, s)| *size == 0 || s == size)
            .filter(|(p, _)| !used.contains(p.as_path()))
            .collect();
        match cands.as_slice() {
            [one] => {
                used.insert(one.0.as_path());
                out.found.push((*id, one.0.clone()));
            }
            [] => {}
            _ => out.ambiguous += 1,
        }
    }
    out
}

/// What changed when a file moved from `old` to `new`: the folders before
/// the part both paths share (`F:\` and `E:\` for `F:\Music\a.mp3` and
/// `E:\Music\a.mp3`). `None` when the file names differ or nothing but
/// the name is shared at the start.
pub fn moved_prefix(old: &Path, new: &Path) -> Option<(PathBuf, PathBuf)> {
    let o: Vec<Component> = old.components().collect();
    let n: Vec<Component> = new.components().collect();
    // Only folder and file names count (never the drive or the root).
    let same = |a: &Component, b: &Component| match (a, b) {
        (Component::Normal(x), Component::Normal(y)) => {
            x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase()
        }
        _ => false,
    };
    let mut k = 0;
    while k < o.len() && k < n.len() && same(&o[o.len() - 1 - k], &n[n.len() - 1 - k]) {
        k += 1;
    }
    if k == 0 || k == o.len() || k == n.len() {
        return None;
    }
    let op: PathBuf = o[..o.len() - k].iter().collect();
    let np: PathBuf = n[..n.len() - k].iter().collect();
    (op != np).then_some((op, np))
}

/// Missing tracks under `old_prefix` whose file is at the same place under
/// `new_prefix` (same size when known): a drive letter that changed, a
/// music folder moved as a whole.
pub fn remap_moved(
    missing: &[(TrackId, PathBuf, u64)],
    old_prefix: &Path,
    new_prefix: &Path,
) -> Vec<(TrackId, PathBuf)> {
    missing
        .iter()
        .filter_map(|(id, p, size)| {
            let rest = p.strip_prefix(old_prefix).ok()?;
            let cand = new_prefix.join(rest);
            let len = std::fs::metadata(&cand).ok().filter(|m| m.is_file())?.len();
            (*size == 0 || len == *size).then_some((*id, cand))
        })
        .collect()
}

/// A path as pasted: surrounding spaces and quotes (Explorer's "Copy as
/// path") removed.
pub fn clean_path(s: &str) -> PathBuf {
    PathBuf::from(s.trim().trim_matches('"').trim())
}

impl LibraryUi {
    /// Points track `id` at `path`. Returns the message for the status line.
    pub fn relink(&mut self, id: TrackId, path: &Path) -> Result<String, String> {
        if self.protect {
            return Err(
                "Library is protected: tracks cannot be relinked (Settings > Library)".into(),
            );
        }
        if !path.is_file() {
            return Err(format!("No file at {}", path.display()));
        }
        if !dj_library::is_audio_file(path) {
            return Err(format!("Not an audio file: {}", path.display()));
        }
        let old = self.track(id).cloned().ok_or("That track is gone")?;
        if let Ok(Some(other)) = self.lib.track_by_path(path)
            && other.id != id
        {
            return Err(format!(
                "That file is already in the library as \u{201c}{}\u{201d}",
                other.display_name()
            ));
        }
        self.lib.relocate(id, path).map_err(|e| e.to_string())?;
        self.reload();
        self.files.set(id, FileState::Found);
        let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let mut msg = format!(
            "Relinked \u{201c}{}\u{201d} to {}",
            old.display_name(),
            path.display()
        );
        if old.meta.file_size > 0 && size != old.meta.file_size {
            msg.push_str(" (the file size differs: re-analyse it if the beatgrid is off)");
        }
        Ok(msg)
    }

    /// Searches `folder` (and its subfolders) in the background for every
    /// track whose file is gone.
    pub fn start_moved_search(&mut self, folder: PathBuf) -> Result<(), String> {
        self.start_search(folder, None, None)
    }

    /// The search behind [`Self::start_moved_search`]. `moved` (old and new
    /// path of a file just relinked) first tries the same move for every
    /// missing track; `after_relink` is the message about that file.
    fn start_search(
        &mut self,
        folder: PathBuf,
        moved: Option<(PathBuf, PathBuf)>,
        after_relink: Option<String>,
    ) -> Result<(), String> {
        if self.protect {
            return Err(
                "Library is protected: tracks cannot be relinked (Settings > Library)".into(),
            );
        }
        if self.moved_search.is_some() {
            return Err("A search is already running".into());
        }
        if !folder.is_dir() {
            return Err(format!("No folder at {}", folder.display()));
        }
        let tracks: Vec<(TrackId, PathBuf, u64)> = self
            .all_tracks()
            .map(|t| (t.id, t.meta.path.clone(), t.meta.file_size))
            .collect();
        let (tx, rx) = mpsc::channel();
        let root = folder.clone();
        std::thread::Builder::new()
            .name("kaldor-find-moved".into())
            .spawn(move || {
                let missing: Vec<_> = tracks.into_iter().filter(|t| !t.1.is_file()).collect();
                // The same move as the file just relinked (another drive
                // letter, a music folder moved as a whole).
                let mut found = moved
                    .and_then(|(o, n)| moved_prefix(&o, &n))
                    .map(|(op, np)| remap_moved(&missing, &op, &np))
                    .unwrap_or_default();
                let done: HashSet<TrackId> = found.iter().map(|f| f.0).collect();
                let rest: Vec<_> = missing
                    .iter()
                    .filter(|t| !done.contains(&t.0))
                    .cloned()
                    .collect();
                let names: HashSet<String> = rest
                    .iter()
                    .filter_map(|t| t.1.file_name())
                    .map(|n| n.to_string_lossy().to_lowercase())
                    .collect();
                let files: Vec<(PathBuf, u64)> = dj_library::scan_folder(&root, true)
                    .into_iter()
                    .filter(|p| {
                        p.file_name()
                            .is_some_and(|n| names.contains(&n.to_string_lossy().to_lowercase()))
                    })
                    .map(|p| {
                        let s = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
                        (p, s)
                    })
                    .collect();
                let mut res = match_moved(&rest, &files);
                found.append(&mut res.found);
                res.found = found;
                res.missing = missing.len();
                let _ = tx.send(res);
            })
            .map_err(|e| format!("cannot start the search: {e}"))?;
        self.moved_search = Some(MovedSearch {
            rx,
            folder,
            after_relink,
        });
        Ok(())
    }

    /// A search that finished: relinks what it found. Returns the message,
    /// once.
    pub(crate) fn poll_moved_search(&mut self) -> Option<String> {
        let res = self.moved_search.as_ref()?.rx.try_recv();
        let res = match res {
            Ok(r) => r,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => MovedResult::default(),
        };
        let search = self.moved_search.take()?;
        let mut done = 0;
        for (id, p) in &res.found {
            if self.lib.relocate(*id, p).is_ok() {
                self.files.set(*id, FileState::Found);
                done += 1;
            }
        }
        if done > 0 {
            self.reload();
        }
        let left = res.missing.saturating_sub(done);
        let mut msg = match &search.after_relink {
            Some(first) if done == 0 => first.clone(),
            Some(first) => {
                format!("{first}; {done} more found next to it ({left} still not found)")
            }
            None => format!(
                "Found {done} of {} missing tracks in {}",
                res.missing,
                search.folder.display()
            ),
        };
        if res.ambiguous > 0 {
            msg.push_str(&format!(
                "; {} have more than one match there: locate those one by one",
                res.ambiguous
            ));
        }
        self.status = msg.clone();
        if let Some(l) = self.locate.as_mut() {
            if search.after_relink.is_some() {
                // The window already shows the relinked file; this is the
                // line about the others.
                let s = if done == 1 { "" } else { "s" };
                l.others = if done == 0 {
                    "No other missing tracks were found next to it.".into()
                } else {
                    format!(
                        "{done} more missing track{s} found and relinked ({left} still not found)."
                    )
                };
                if res.ambiguous > 0 {
                    l.others.push_str(&format!(
                        " {} have more than one match there: locate those one by one.",
                        res.ambiguous
                    ));
                }
            } else {
                l.message = msg.clone();
                l.done = self.files.get(l.track) == FileState::Found;
                if l.done {
                    // A folder search found this track: the count goes on
                    // the result's second line.
                    l.others = format!("{msg}.");
                }
            }
        }
        Some(msg)
    }

    /// Opens the Locate window for a track.
    pub fn open_locate(&mut self, track: TrackId) {
        self.locate = Some(Locate {
            track,
            ..Default::default()
        });
    }

    /// The window's file or folder: a file relinks the track (and then
    /// looks for the other missing tracks next to it), a folder is
    /// searched for every missing track.
    pub fn locate_with(&mut self, path: &Path) {
        let Some(id) = self.locate.as_ref().map(|l| l.track) else {
            return;
        };
        let r = if path.is_dir() {
            self.start_moved_search(path.to_path_buf())
        } else {
            let old = self.track(id).map(|t| t.meta.path.clone());
            self.relink(id, path).map(|msg| {
                self.status = msg.clone();
                // Others that moved the same way, or into the same folder.
                let others = if self.missing_count() == 0 {
                    "No other tracks are missing.".to_string()
                } else {
                    match (old, path.parent()) {
                        (Some(old), Some(dir)) => {
                            let moved = Some((old, path.to_path_buf()));
                            match self.start_search(dir.to_path_buf(), moved, Some(msg.clone())) {
                                Ok(()) => String::new(),
                                Err(e) => e,
                            }
                        }
                        _ => String::new(),
                    }
                };
                if let Some(l) = self.locate.as_mut() {
                    l.done = true;
                    l.message = msg;
                    l.others = others;
                }
            })
        };
        if let (Err(e), Some(l)) = (r, self.locate.as_mut()) {
            l.message = e;
        }
    }

    /// The number of tracks known to be missing right now.
    pub fn missing_count(&self) -> usize {
        self.all_tracks()
            .filter(|t| self.files.get(t.id).is_missing())
            .count()
    }
}

impl KaldorApp {
    /// A file or folder dropped from the file manager while the Locate
    /// window is open. Returns true when the window took it.
    pub(crate) fn locate_took_drop(&mut self, path: &Path) -> bool {
        let Some(lib) = self.library.as_mut() else {
            return false;
        };
        let Some(loc) = lib.locate.as_mut() else {
            return false;
        };
        loc.path = path.display().to_string();
        lib.locate_with(path);
        true
    }
}

/// The Locate window (a modal over the app).
pub(crate) fn locate_window(app: &mut KaldorApp, ctx: &egui::Context) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    let Some(mut loc) = lib.locate.clone() else {
        return;
    };
    let Some(track) = lib.track(loc.track).cloned() else {
        lib.locate = None;
        return;
    };
    let searching = lib.moved_search.is_some();
    let missing = lib.missing_count();
    let frame = egui::Frame::new()
        .fill(if pal.dark { pal.raised } else { pal.bg })
        .stroke(egui::Stroke::new(1.0, pal.border))
        .corner_radius(14.0)
        .inner_margin(egui::Margin::same(20));
    let mut go: Option<PathBuf> = None;
    let mut close = false;
    let mut marks: Vec<(&'static str, egui::Rect, egui::Rect)> = Vec::new();
    egui::Modal::new(egui::Id::unique("locate_window"))
        .backdrop_color(Color32::from_black_alpha(150))
        .frame(frame)
        .show(ctx, |ui| {
            ui.set_width(520.0);
            ui.label(RichText::new("Locate file").size(18.0).color(pal.text));
            ui.add_space(8.0);
            ui.label(RichText::new(track.display_name()).strong().color(pal.text));
            if loc.done {
                // The result stays until Done: what was relinked, and what
                // the search for the other missing tracks found.
                ui.label(
                    RichText::new(format!("Now at {}", track.meta.path.display()))
                        .size(12.0)
                        .color(pal.text_weak),
                );
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
                    let c = r.center();
                    ui.painter().circle_filled(c, 9.0, pal.play);
                    let ink = egui::Stroke::new(2.2, Color32::WHITE);
                    ui.painter().line_segment([c + egui::vec2(-4.5, 0.0), c + egui::vec2(-1.2, 3.5)], ink);
                    ui.painter().line_segment([c + egui::vec2(-1.2, 3.5), c + egui::vec2(4.8, -3.5)], ink);
                    let t = ui.label(RichText::new("Relinked: the track plays from its new place").color(pal.text));
                    marks.push(("locate_result", t.rect, ui.clip_rect()));
                });
                if loc.message.contains("size differs") {
                    ui.label(
                        RichText::new("The file size differs from before: re-analyse it if the beatgrid is off.")
                            .size(12.5)
                            .color(pal.warn),
                    );
                }
                ui.add_space(6.0);
                if searching {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Looking for the other missing tracks…").color(pal.text_weak),
                        );
                    });
                } else if !loc.others.is_empty() {
                    let t = ui.label(RichText::new(&loc.others).color(pal.text));
                    marks.push(("locate_others", t.rect, ui.clip_rect()));
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let b = button(ui, "Done", true, None);
                    marks.push(("locate_done", b.rect, ui.clip_rect()));
                    close = b.clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter));
                });
                return;
            }
            ui.label(
                RichText::new(format!("Was at {}", track.meta.path.display()))
                    .size(12.0)
                    .color(pal.text_weak),
            );
            ui.add_space(8.0);
            let others = if missing > 1 {
                format!(" ({} other tracks are not found either)", missing - 1)
            } else {
                String::new()
            };
            ui.label(
                RichText::new(format!(
                    "Browse to the file, drag it here or paste its path: the track is \
                     relinked, and other missing tracks next to it are found too. A \
                     folder dragged or pasted here is searched for every missing \
                     track{others}."
                ))
                .color(pal.text_weak),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                let r = ui.add(
                    egui::TextEdit::singleline(&mut loc.path)
                        .desired_width(320.0)
                        .hint_text("File or folder"),
                );
                marks.push(("locate_path", r.rect, ui.clip_rect()));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                // Starts where the file used to be.
                let start = if loc.path.trim().is_empty() {
                    track
                        .meta
                        .path
                        .parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                } else {
                    loc.path.clone()
                };
                let (br, picked) = crate::browse::button(
                    ui,
                    "locate_file",
                    "Where is the file now?",
                    crate::browse::Pick::AudioFile,
                    &start,
                );
                marks.push(("locate_browse", br.rect, ui.clip_rect()));
                let b = ui.add_enabled_ui(!searching, |ui| button(ui, "Locate", true, None));
                marks.push(("locate_go", b.inner.rect, ui.clip_rect()));
                if (b.inner.clicked() || enter) && !loc.path.trim().is_empty() {
                    go = Some(clean_path(&loc.path));
                }
                if let Some(p) = picked {
                    loc.path = p.display().to_string();
                    go = Some(p);
                }
            });
            if searching {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Searching…").color(pal.text_weak));
                });
            }
            if !loc.message.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new(&loc.message).color(pal.text));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let b = button(ui, "Close", false, None);
                marks.push(("locate_close", b.rect, ui.clip_rect()));
                close = b.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
    for (name, r, clip) in marks {
        app.map.add_rect(name, r, clip);
    }
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    if close {
        lib.locate = None;
        return;
    }
    if let Some(l) = lib.locate.as_mut() {
        l.path = loc.path.clone();
    }
    if let Some(p) = go {
        lib.locate_with(&p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_files_match_by_name_and_size() {
        let p = PathBuf::from;
        let missing = vec![
            (1, p("F:/old/A.mp3"), 100),
            (2, p("F:/old/b.mp3"), 200),
            (3, p("F:/old/c.mp3"), 300),
            (4, p("F:/old/d.mp3"), 0),
        ];
        let files = vec![
            (p("E:/new/a.MP3"), 100),
            (p("E:/new/b.mp3"), 999),
            (p("E:/x/c.mp3"), 300),
            (p("E:/y/c.mp3"), 300),
            (p("E:/new/d.mp3"), 5),
        ];
        let r = match_moved(&missing, &files);
        assert_eq!(
            r.found,
            vec![(1, p("E:/new/a.MP3")), (4, p("E:/new/d.mp3"))]
        );
        assert_eq!(r.ambiguous, 1, "two c.mp3 of the same size");
        assert_eq!(r.missing, 4);
    }

    #[test]
    fn the_move_of_one_file_is_found_from_its_two_paths() {
        let p = PathBuf::from;
        assert_eq!(
            moved_prefix(&p("F:/Music/House/a.mp3"), &p("E:/Music/House/a.mp3")),
            Some((p("F:/"), p("E:/")))
        );
        assert_eq!(
            moved_prefix(&p("/old/Music/a.mp3"), &p("/new/stuff/Music/a.mp3")),
            Some((p("/old"), p("/new/stuff")))
        );
        assert_eq!(moved_prefix(&p("F:/a.mp3"), &p("E:/b.mp3")), None);
        assert_eq!(moved_prefix(&p("F:/a.mp3"), &p("F:/a.mp3")), None);
    }

    #[test]
    fn pasted_paths_lose_their_quotes() {
        assert_eq!(
            clean_path("  \"C:\\Music\\a b.mp3\"\n"),
            PathBuf::from("C:\\Music\\a b.mp3")
        );
    }
}
