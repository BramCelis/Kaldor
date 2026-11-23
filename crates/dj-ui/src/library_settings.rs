//! Library settings that run in the background of the app: database
//! backups, watch folders and the play history.
//!
//! The history is in the library database (see
//! `dj_library::history` and `history_view`). The CSV files the history
//! was kept in before (`history/` in the app's data folder) are imported
//! once at start; the files stay.

use crate::app::KaldorApp;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

#[derive(Default)]
pub(crate) struct LibraryTasks {
    started: bool,
    /// Watch folders waiting for their turn to import.
    pending_imports: Vec<PathBuf>,
    /// The app's data folder (older history files live there).
    pub(crate) data_dir: Option<PathBuf>,
    /// This run's history session (made with the first play).
    pub(crate) history_session: Option<dj_library::history::SessionId>,
    /// A background backup's result.
    backup_rx: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    /// What the last library task did (shown in Settings > Library).
    pub(crate) message: String,
    /// The watch folder field while typed in.
    pub(crate) watch_edit: String,
}

/// Backups go to `backups/` next to the library file.
pub(crate) fn backup_dir(db: &Path) -> PathBuf {
    db.parent()
        .map_or_else(|| PathBuf::from("backups"), |d| d.join("backups"))
}

/// Deletes the oldest `library-*.db` backups beyond `keep`.
pub(crate) fn prune_backups(dir: &Path, keep: usize) -> std::io::Result<usize> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("library-") && n.ends_with(".db"))
        })
        .collect();
    // Names carry the date and time, so they sort oldest first.
    files.sort();
    let extra = files.len().saturating_sub(keep);
    for f in &files[..extra] {
        std::fs::remove_file(f)?;
    }
    Ok(extra)
}

/// `2026-11-15 2130` for file names.
fn stamp() -> String {
    let (y, mo, d, h, mi, s) = crate::recorder::now_parts();
    format!("{y:04}-{mo:02}-{d:02} {h:02}{mi:02}{s:02}")
}

/// The list of older history files already imported (in the history
/// folder).
const IMPORTED_LIST: &str = "imported-into-library.txt";

/// Imports the older history CSV files in `dir` that are not imported yet
/// (one session each); returns the number of plays. The files stay; their
/// names go into [`IMPORTED_LIST`].
pub(crate) fn import_old_history(
    lib: &mut dj_library::Library,
    dir: &Path,
    offset: i64,
) -> std::io::Result<usize> {
    let list = dir.join(IMPORTED_LIST);
    let done = std::fs::read_to_string(&list).unwrap_or_default();
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "csv"))
        .collect();
    files.sort();
    let mut plays = 0;
    let mut names = Vec::new();
    for f in files {
        let name = f
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if done.lines().any(|l| l == name) {
            continue;
        }
        let text = std::fs::read_to_string(&f)?;
        plays += lib
            .import_history_csv(&text, offset)
            .map_err(std::io::Error::other)?;
        names.push(name);
    }
    if !names.is_empty() {
        use std::io::Write;
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&list)?;
        for n in names {
            writeln!(out, "{n}")?;
        }
    }
    Ok(plays)
}

impl KaldorApp {
    /// Library settings work, once a frame: the start-up backup and watch
    /// folder scan, queued imports, a finished backup.
    pub(crate) fn poll_library_tasks(&mut self) {
        if self.library.is_none() {
            return;
        }
        if !self.lib_tasks.started {
            self.lib_tasks.started = true;
            if self.config.library.backups_keep > 0 {
                self.start_backup();
            }
            // Older history files into the library, once.
            if let Some(dir) = self.history_dir().filter(|d| d.is_dir())
                && let Some(lib) = self.library.as_mut()
            {
                match import_old_history(&mut lib.lib, &dir, crate::history_view::local_offset()) {
                    Ok(0) => {}
                    Ok(n) => tracing::info!("history: {n} plays imported from {}", dir.display()),
                    Err(e) => tracing::warn!("history: older files not imported: {e}"),
                }
            }
            self.lib_tasks.pending_imports = self
                .config
                .library
                .watch_folders
                .iter()
                .filter(|p| p.is_dir())
                .cloned()
                .collect();
        }
        if let Some(lib) = self.library.as_mut()
            && lib.import_progress().is_none()
            && let Some(next) = self.lib_tasks.pending_imports.pop()
        {
            lib.start_import(next);
        }
        if let Some(rx) = &self.lib_tasks.backup_rx
            && let Ok(r) = rx.try_recv()
        {
            self.lib_tasks.backup_rx = None;
            self.lib_tasks.message = match r {
                Ok(p) => format!("Library backed up to {}", p.display()),
                Err(e) => format!("Backup failed: {e}"),
            };
        }
    }

    /// Copies the library file to `backups/library-<date time>.db` on a
    /// background thread, then keeps the newest `backups_keep` copies.
    pub(crate) fn start_backup(&mut self) {
        let Some(db) = self.library.as_ref().and_then(|l| l.lib.file_path()) else {
            self.lib_tasks.message = "No library file to back up (in-memory library)".into();
            return;
        };
        if self.lib_tasks.backup_rx.is_some() {
            return;
        }
        let keep = self.config.library.backups_keep.max(1);
        let dir = backup_dir(&db);
        let dest = dir.join(format!("library-{}.db", stamp().replace(' ', "-")));
        let (tx, rx) = mpsc::channel();
        self.lib_tasks.backup_rx = Some(rx);
        self.lib_tasks.message = "Backing up the library…".into();
        let spawned = std::thread::Builder::new()
            .name("kaldor-backup".into())
            .spawn(move || {
                let r = dj_library::Library::backup_file(&db, &dest)
                    .map_err(|e| e.to_string())
                    .and_then(|()| prune_backups(&dir, keep).map_err(|e| e.to_string()))
                    .map(|_| dest);
                let _ = tx.send(r);
            });
        if let Err(e) = spawned {
            self.lib_tasks.backup_rx = None;
            self.lib_tasks.message = format!("Backup failed: {e}");
        }
    }

    /// Imports every watch folder now (Scan now).
    pub(crate) fn scan_watch_folders(&mut self) {
        self.lib_tasks.pending_imports = self
            .config
            .library
            .watch_folders
            .iter()
            .filter(|p| p.is_dir())
            .cloned()
            .collect();
        self.lib_tasks.message = format!(
            "Scanning {} folder(s) for new music",
            self.lib_tasks.pending_imports.len()
        );
    }

    /// The folder the older history files are in.
    pub(crate) fn history_dir(&self) -> Option<PathBuf> {
        self.lib_tasks.data_dir.as_ref().map(|d| d.join("history"))
    }
}

/// Settings > Library, after Analysis: watch folders, the database
/// (backups, protect, reset) and the history.
pub(crate) fn library_cards(app: &mut KaldorApp, ui: &mut egui::Ui, pal: &crate::theme::Palette) {
    use crate::settings_ui::{button, card, switch};
    use egui::RichText;
    if app.library.is_none() {
        return;
    }
    let mut changed = false;
    let mut scan = false;
    let mut backup = false;
    let mut remove: Option<usize> = None;
    card(ui, pal, "Watch folders", |rows| {
        let folders = app.config.library.watch_folders.clone();
        if folders.is_empty() {
            rows.row(
                "No watch folders",
                "Add the folders you keep music in: Kaldor looks for new files there at every start.",
                |_| {},
            );
        }
        for (i, f) in folders.iter().enumerate() {
            let desc = if f.is_dir() { "" } else { "Not found" };
            rows.row(&f.display().to_string(), desc, |ui| {
                let b = button(ui, "Remove", false, None);
                if b.clicked() {
                    remove = Some(i);
                }
                app.map.add(ui, "watch_remove", Some(i), b.rect);
            });
        }
        rows.row("Add a folder", "", |ui| {
            let b = button(ui, "Add", true, None);
            let add = b.clicked() && !app.lib_tasks.watch_edit.trim().is_empty();
            app.map.add(ui, "watch_add", None, b.rect);
            let r = ui.add(
                egui::TextEdit::singleline(&mut app.lib_tasks.watch_edit)
                    .hint_text("Folder")
                    .desired_width(240.0),
            );
            app.map.add(ui, "watch_path", None, r.rect);
            // A folder picked in the dialog is added at once.
            let (br, picked) = crate::browse::button(
                ui,
                "watch_folder",
                "Watch which folder?",
                crate::browse::Pick::Folder,
                &app.lib_tasks.watch_edit,
            );
            app.map.add(ui, "watch_browse", None, br.rect);
            if let Some(p) = &picked {
                app.lib_tasks.watch_edit = p.display().to_string();
            }
            if add || picked.is_some() {
                let p = PathBuf::from(app.lib_tasks.watch_edit.trim());
                app.lib_tasks.watch_edit.clear();
                if !app.config.library.watch_folders.contains(&p) {
                    app.config.library.watch_folders.push(p);
                    changed = true;
                }
            }
        });
        rows.row(
            "Scan now",
            "Imports new files from every watch folder (known files are skipped).",
            |ui| {
                let b = button(ui, "Scan now", false, None);
                scan = b.clicked();
                app.map.add(ui, "watch_scan", None, b.rect);
            },
        );
    });
    if let Some(i) = remove {
        app.config.library.watch_folders.remove(i);
        changed = true;
    }
    if scan {
        app.scan_watch_folders();
    }
    let db = app.library.as_ref().and_then(|l| l.lib.file_path());
    card(ui, pal, "Database", |rows| {
        rows.row(
            "Library file",
            &db.as_ref().map_or_else(
                || "In memory (not saved)".to_string(),
                |p| p.display().to_string(),
            ),
            |_| {},
        );
        let keeps: Vec<(usize, String)> = [0usize, 3, 5, 10, 20]
            .into_iter()
            .map(|k| {
                (
                    k,
                    if k == 0 {
                        "Off".to_string()
                    } else {
                        format!("Keep {k}")
                    },
                )
            })
            .collect();
        let where_to = db.as_deref().map_or_else(
            || "backups".to_string(),
            |p| backup_dir(p).display().to_string(),
        );
        rows.row(
            "Backups",
            &format!(
                "A copy at every start, in {where_to}. Older copies beyond this number are deleted."
            ),
            |ui| {
                let b = button(ui, "Back up now", false, None);
                backup = b.clicked();
                app.map.add(ui, "backup_now", None, b.rect);
                let now = app.config.library.backups_keep;
                let r = egui::ComboBox::from_id_salt("backups_keep")
                    .selected_text(
                        keeps
                            .iter()
                            .find(|(k, _)| *k == now)
                            .map_or_else(|| format!("Keep {now}"), |(_, l)| l.clone()),
                    )
                    .width(110.0)
                    .show_ui(ui, |ui| {
                        for (k, l) in &keeps {
                            if ui.selectable_label(*k == now, l.as_str()).clicked() {
                                app.config.library.backups_keep = *k;
                                changed = true;
                            }
                        }
                    });
                app.map.add(ui, "backups_keep", None, r.response.rect);
            },
        );
        rows.row(
            "Protect the library",
            "No changes to crates, cues, loops or beatgrids (for a gig or a borrowed laptop). Playing and loading still work.",
            |ui| {
                let on = app.config.library.protect;
                let r = switch(ui, on);
                if r.clicked() {
                    app.config.library.protect = !on;
                    if let Some(l) = app.library.as_mut() {
                        l.protect = !on;
                    }
                    changed = true;
                }
                app.map.add(ui, "protect_library", None, r.rect);
            },
        );
        let count = app.library.as_ref().map_or(0, |l| l.track_count());
        rows.row(
            "Reset the library",
            &format!(
                "Starts over with an empty library ({count} tracks now): every track and crate is removed, \
                 the files stay on your computer. Asks first, and makes a backup."
            ),
            |ui| {
                let b = button(ui, "Reset library…", false, Some(pal.error));
                if b.clicked()
                    && let Some(l) = app.library.as_mut()
                {
                    l.confirm_reset = Some(true);
                }
                app.map.add(ui, "reset_library", None, b.rect);
            },
        );
    });
    if backup {
        app.start_backup();
    }
    // The rekordbox tab's sync.
    let mut rb_changed = false;
    card(ui, pal, "rekordbox sync", |rows| {
        let cfg = &mut app.config.library;
        rows.row(
            "Keep the rekordbox tab in step",
            "Shows your rekordbox playlists in the crate list's rekordbox tab and follows rekordbox on this computer: new tracks, playlists and changes show up by themselves. rekordbox's library is only read, never changed.",
            |ui| {
                let r = switch(ui, cfg.rekordbox_sync);
                if r.clicked() {
                    cfg.rekordbox_sync = !cfg.rekordbox_sync;
                    rb_changed = true;
                }
                app.map.add(ui, "rekordbox_sync_switch", None, r.rect);
            },
        );
        let default = dj_library::interop::rekordbox_db::default_dir();
        let shown = cfg
            .rekordbox_folder
            .clone()
            .or(default.clone())
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let desc = format!(
            "The folder with rekordbox's library (master.db). {}",
            if cfg.rekordbox_folder.is_some() {
                format!("Chosen by you: {shown}")
            } else if shown.is_empty() {
                "No default on this system: pick it.".to_string()
            } else {
                format!("Where rekordbox keeps it: {shown}")
            }
        );
        rows.row("rekordbox folder", &desc, |ui| {
            ui.horizontal(|ui| {
                let (b, picked) = crate::browse::button(
                    ui,
                    "rekordbox_folder_settings",
                    "rekordbox's folder (with master.db)",
                    crate::browse::Pick::Folder,
                    &shown,
                );
                app.map.add(ui, "rekordbox_folder_settings", None, b.rect);
                if let Some(p) = picked {
                    cfg.rekordbox_folder = Some(p);
                    rb_changed = true;
                }
                if cfg.rekordbox_folder.is_some() {
                    let d = button(ui, "Default", false, None)
                        .on_hover_text("Use the folder where rekordbox keeps its library");
                    if d.clicked() {
                        cfg.rekordbox_folder = None;
                        rb_changed = true;
                    }
                }
            });
        });
    });
    if rb_changed {
        changed = true;
        let (on, folder) = (
            app.config.library.rekordbox_sync,
            app.config.library.rekordbox_folder.clone(),
        );
        if let Some(l) = app.library.as_mut() {
            l.rekordbox.configure(on, folder);
        }
    }
    let mut import_from = None;
    card(ui, pal, "Import from other software", |rows| {
        for s in crate::interop_view::Source::ALL {
            let label = s.menu_label().trim_end_matches('…');
            rows.row(label, s.hint(), |ui| {
                let b = button(ui, "Import…", false, None);
                if b.clicked() {
                    import_from = Some(s);
                }
                app.map.add(ui, s.browse_id(), None, b.rect);
            });
        }
    });
    if let Some(s) = import_from
        && let Some(l) = app.library.as_mut()
    {
        l.foreign.ask_file(s);
    }
    let mut open_history = false;
    card(ui, pal, "History", |rows| {
        let on = app.config.library.history;
        rows.row(
            "Keep a history",
            "Every track you play, one session each time Kaldor runs, in the library.",
            |ui| {
                let r = switch(ui, on);
                if r.clicked() {
                    app.config.library.history = !on;
                    changed = true;
                }
                app.map.add(ui, "history", None, r.rect);
            },
        );
        rows.row(
            "Sessions",
            "The tracks of each session in the order played; save one as a playlist or CSV file, or make it a crate.",
            |ui| {
                let b = button(ui, "History…", false, None);
                open_history = b.clicked();
                app.map.add(ui, "history_open", None, b.rect);
            },
        );
    });
    if open_history {
        // The panel is over the library, behind this window: close it.
        app.open_history();
        app.show_setup = false;
    }
    let msg = app.lib_tasks.message.clone();
    if !msg.is_empty() {
        card(ui, pal, "Last library task", |rows| {
            rows.block(|ui| {
                ui.label(RichText::new(msg).size(12.5).color(pal.text_weak));
            });
        });
    }
    if changed {
        app.events.push(crate::app::AppEvent::ConfigChanged);
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_the_newest_backups() {
        let dir = crate::test_dir("kaldor-prune");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for n in [
            "library-2026-11-01-0900.db",
            "library-2026-11-03-0900.db",
            "library-2026-11-02-0900.db",
            "other.db",
        ] {
            std::fs::write(dir.join(n), b"x").unwrap();
        }
        assert_eq!(prune_backups(&dir, 2).unwrap(), 1);
        assert!(!dir.join("library-2026-11-01-0900.db").exists());
        assert!(dir.join("library-2026-11-02-0900.db").exists());
        assert!(dir.join("library-2026-11-03-0900.db").exists());
        assert!(dir.join("other.db").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The older history files come into the library once; the files stay.
    #[test]
    fn older_history_files_are_imported_once() {
        let dir = crate::test_dir("kaldor-hist");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("2026-11-15 213000.csv");
        std::fs::write(
            &f,
            "played_at,artist,title,bpm,key,path\n\
             2026-11-15 21:30:00,Artist,\"Title, Two\",128.00,8A,C:\\m\\a.mp3\n",
        )
        .unwrap();
        let mut lib = dj_library::Library::open_in_memory().unwrap();
        assert_eq!(import_old_history(&mut lib, &dir, 0).unwrap(), 1);
        assert_eq!(import_old_history(&mut lib, &dir, 0).unwrap(), 0, "once");
        assert!(f.exists());
        let s = lib.sessions().unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(lib.session_plays(s[0].id).unwrap()[0].title, "Title, Two");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
