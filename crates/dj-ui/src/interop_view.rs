//! "Import from…": another program's library (rekordbox XML, the iTunes /
//! Music library XML) or a playlist file (M3U, M3U8, PLS) merged into
//! Kaldor's library; and "Save as playlist…" for a crate.
//!
//! The file and the tags of its tracks are read on a background thread;
//! a window then says what was found and asks how to merge. The merge
//! itself runs on the UI thread when Import is clicked (one database
//! connection), then a report lists what came in and what was left out.

use crate::app::KaldorApp;
use crate::browse::{self, Pick};
use crate::settings_ui::{button, switch};
use crate::theme::Palette;
use dj_library::interop::{
    self, ForeignLibrary, ForeignNode, InteropOptions, InteropReport, playlist,
};
use dj_library::{CrateId, TrackMeta};
use egui::{Color32, RichText};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

/// What is imported.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Rekordbox,
    Itunes,
    Playlist,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Rekordbox, Source::Itunes, Source::Playlist];

    /// The entry in the "Import from" menus.
    pub fn menu_label(self) -> &'static str {
        match self {
            Source::Rekordbox => "rekordbox library (XML)…",
            Source::Itunes => "iTunes / Music library (XML)…",
            Source::Playlist => "Playlist file (M3U, PLS)…",
        }
    }

    /// The title of the entry in the Import menu (library toolbar).
    pub fn title(self) -> &'static str {
        match self {
            Source::Rekordbox => "rekordbox",
            Source::Itunes => "Apple Music / iTunes",
            Source::Playlist => "Playlist file",
        }
    }

    /// The line under the title in the Import menu.
    pub fn blurb(self) -> &'static str {
        match self {
            Source::Rekordbox => "Tracks, cues, loops, grids and playlists (XML)",
            Source::Itunes => "Tracks, ratings and playlists (Library XML)",
            Source::Playlist => "M3U, M3U8 or PLS, as a new crate",
        }
    }

    /// The icon of the entry in the Import menu.
    pub fn icon(self) -> crate::widgets::Icon {
        match self {
            Source::Rekordbox => crate::widgets::Icon::Record,
            Source::Itunes => crate::widgets::Icon::Note,
            Source::Playlist => crate::widgets::Icon::List,
        }
    }

    /// The hover text of the menu entry.
    pub fn hint(self) -> &'static str {
        match self {
            Source::Rekordbox => {
                "In rekordbox: File > Export Collection in xml format. Brings in tracks, \
                 hot cues, memory cues, loops, beatgrids, ratings and playlists."
            }
            Source::Itunes => {
                "In Music or iTunes: File > Library > Export Library. Brings in tracks, \
                 ratings, play counts and playlists."
            }
            Source::Playlist => "One playlist: its tracks come in as a crate named after the file.",
        }
    }

    /// The name the windows use.
    pub fn name(self) -> &'static str {
        match self {
            Source::Rekordbox => "rekordbox",
            Source::Itunes => "iTunes / Music",
            Source::Playlist => "a playlist",
        }
    }

    /// The Browse id (one per source, so each answer finds its own).
    pub fn browse_id(self) -> &'static str {
        match self {
            Source::Rekordbox => "import_rekordbox",
            Source::Itunes => "import_itunes",
            Source::Playlist => "import_playlist",
        }
    }

    fn pick(self) -> Pick {
        match self {
            Source::Rekordbox => Pick::File("rekordbox XML", &["xml"]),
            Source::Itunes => Pick::File("Library XML", &["xml"]),
            Source::Playlist => Pick::File("Playlist", &["m3u8", "m3u", "pls"]),
        }
    }

    fn dialog_title(self) -> &'static str {
        match self {
            Source::Rekordbox => "Import which rekordbox XML file?",
            Source::Itunes => "Import which iTunes / Music library file?",
            Source::Playlist => "Import which playlist?",
        }
    }

    /// The crate the playlists go under, by default (none for one
    /// playlist: it becomes a crate of its own).
    fn root_crate(self) -> Option<&'static str> {
        match self {
            Source::Rekordbox => Some("rekordbox"),
            Source::Itunes => Some("iTunes"),
            Source::Playlist => None,
        }
    }

    /// Reads the file (any thread).
    pub fn read(self, path: &Path) -> Result<ForeignLibrary, String> {
        match self {
            Source::Rekordbox => interop::rekordbox::read_file(path),
            Source::Itunes => interop::itunes::read_file(path),
            Source::Playlist => playlist::read_file(path),
        }
    }
}

type Prepared = Result<(ForeignLibrary, Vec<Option<TrackMeta>>), String>;

/// A file read and ready to merge, with the choices of the window.
pub struct Ready {
    pub source: Source,
    pub path: PathBuf,
    pub foreign: ForeignLibrary,
    files: Vec<Option<TrackMeta>>,
    /// Put the playlists in a crate of their own (named `root`).
    pub use_root: bool,
    pub root: String,
    pub replace_cues: bool,
    pub replace_grids: bool,
}

impl Ready {
    /// Tracks whose file is not on this computer.
    pub fn not_found(&self) -> usize {
        self.files.iter().filter(|f| f.is_none()).count()
    }

    /// Playlists (not folders) in the file.
    pub fn playlists(&self) -> usize {
        fn count(nodes: &[ForeignNode]) -> usize {
            nodes
                .iter()
                .map(|n| match n {
                    ForeignNode::Folder { children, .. } => count(children),
                    ForeignNode::Playlist { .. } => 1,
                })
                .sum()
        }
        count(&self.foreign.playlists)
    }
}

/// Where an import is.
#[derive(Default)]
pub enum Stage {
    #[default]
    Idle,
    /// The file and the tags are being read.
    Reading {
        source: Source,
        path: PathBuf,
        rx: mpsc::Receiver<Prepared>,
    },
    /// Waiting for Import or Cancel.
    Ready(Box<Ready>),
    /// What the import did.
    Done {
        source: Source,
        report: InteropReport,
        warnings: Vec<String>,
    },
    /// The file could not be read (or merged).
    Failed(String),
}

/// The Import from… state of the library.
#[derive(Default)]
pub struct ForeignImport {
    pub stage: Stage,
    /// The crate waiting for its "Save as playlist" file name.
    pub save_crate: Option<CrateId>,
}

/// Browse id of "Save as playlist…".
pub const SAVE_ID: &str = "playlist_save";

impl ForeignImport {
    /// Asks for the file to import (the system's dialog).
    pub fn ask_file(&mut self, s: Source) {
        browse::open(s.browse_id(), s.dialog_title(), s.pick(), "");
    }

    /// Reads `path` on a background thread.
    pub fn start(&mut self, source: Source, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        let p = path.clone();
        let spawned = std::thread::Builder::new()
            .name("kaldor-import-from".into())
            .spawn(move || {
                let r = source.read(&p).map(|f| {
                    let files = f.prepare();
                    (f, files)
                });
                let _ = tx.send(r);
            });
        self.stage = match spawned {
            Ok(_) => Stage::Reading { source, path, rx },
            Err(e) => Stage::Failed(format!("Cannot read {}: {e}", path.display())),
        };
    }

    /// Takes a picked file and a finished read. True while reading.
    pub fn poll(&mut self) -> bool {
        for s in Source::ALL {
            if let Some(p) = browse::take(s.browse_id()) {
                self.start(s, p);
            }
        }
        let Stage::Reading { source, path, rx } = &self.stage else {
            return false;
        };
        match rx.try_recv() {
            Ok(Ok((foreign, files))) => {
                let source = *source;
                self.stage = Stage::Ready(Box::new(Ready {
                    source,
                    path: path.clone(),
                    foreign,
                    files,
                    use_root: source.root_crate().is_some(),
                    root: source.root_crate().unwrap_or("Imported").to_string(),
                    replace_cues: false,
                    replace_grids: false,
                }));
                false
            }
            Ok(Err(e)) => {
                self.stage = Stage::Failed(e);
                false
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.stage = Stage::Failed("The import stopped unexpectedly".into());
                false
            }
            Err(mpsc::TryRecvError::Empty) => true,
        }
    }

    /// Asks where to save crate `c` (named `name`) as a playlist.
    pub fn ask_save(&mut self, c: CrateId, name: &str) {
        self.save_crate = Some(c);
        let file: String = name
            .chars()
            .map(|ch| if r#"\/:*?"<>|"#.contains(ch) { '_' } else { ch })
            .collect();
        browse::open(
            SAVE_ID,
            "Save the crate as a playlist",
            Pick::SaveAs(
                &[("M3U8 playlist", "m3u8"), ("PLS playlist", "pls")],
                format!("{}.m3u8", file.trim()),
            ),
            "",
        );
    }
}

/// "Imported from rekordbox: 120 new tracks, …".
pub fn summary(source: Source, r: &InteropReport) -> String {
    let mut parts = vec![format!(
        "{} new track{}",
        r.tracks_added,
        if r.tracks_added == 1 { "" } else { "s" }
    )];
    if r.tracks_matched > 0 {
        parts.push(format!("{} already in the library", r.tracks_matched));
    }
    if r.crates > 0 {
        parts.push(format!(
            "{} crate{}",
            r.crates,
            if r.crates == 1 { "" } else { "s" }
        ));
    }
    if r.missing_files > 0 {
        parts.push(format!("{} files not found", r.missing_files));
    }
    format!("Imported from {}: {}", source.name(), parts.join(", "))
}

impl KaldorApp {
    /// Import from… and Save as playlist work, once a frame.
    pub fn poll_foreign(&mut self) -> bool {
        let Some(lib) = self.library.as_mut() else {
            return false;
        };
        if let Some(dest) = browse::take(SAVE_ID)
            && let Some(c) = lib.foreign.save_crate.take()
        {
            let r = lib
                .lib
                .shown_tracks(c)
                .and_then(|ids| playlist::entries_for(&lib.lib, &ids))
                .map_err(|e| e.to_string())
                .and_then(|entries| {
                    playlist::write(&dest, &entries)
                        .map(|()| entries.len())
                        .map_err(|e| e.to_string())
                });
            lib.status = match r {
                Ok(n) => format!("Playlist of {n} tracks saved: {}", dest.display()),
                Err(e) => format!("Could not save the playlist: {e}"),
            };
        }
        lib.foreign.poll()
    }

    /// Imports what the Import window holds (its Import button; tests).
    pub fn import_foreign_now(&mut self) {
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        let Stage::Ready(ready) = std::mem::take(&mut lib.foreign.stage) else {
            return;
        };
        if lib.protect {
            lib.status = "Library is protected: nothing imported (Settings > Library)".into();
            return;
        }
        let opts = InteropOptions {
            root_crate: (ready.use_root && !ready.root.trim().is_empty())
                .then(|| ready.root.trim().to_string()),
            replace_cues: ready.replace_cues,
            replace_grids: ready.replace_grids,
        };
        match lib.lib.import_foreign(&ready.foreign, &ready.files, &opts) {
            Ok(report) => {
                lib.reload();
                if lib.analyse_on_import {
                    for id in &report.to_analyse {
                        lib.analyse_if_needed(*id);
                    }
                }
                lib.status = summary(ready.source, &report);
                lib.foreign.stage = Stage::Done {
                    source: ready.source,
                    report,
                    warnings: ready.foreign.warnings,
                };
            }
            Err(e) => lib.foreign.stage = Stage::Failed(format!("The import failed: {e}")),
        }
    }
}

fn frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(if pal.dark { pal.raised } else { pal.bg })
        .stroke(egui::Stroke::new(1.0, pal.border))
        .corner_radius(14.0)
        .inner_margin(egui::Margin::same(22))
}

/// What the Import window was asked to do this frame.
enum Answer {
    None,
    Import,
    Close,
}

/// The Import window (a modal), when an import is under way.
pub(crate) fn window(app: &mut KaldorApp, ctx: &egui::Context) {
    if app.poll_foreign() {
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    let pal = Palette::get(app.config.ui.dark_theme);
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    if matches!(lib.foreign.stage, Stage::Idle) {
        return;
    }
    let mut answer = Answer::None;
    let mut marks = Vec::new();
    let title = |t: String| RichText::new(t).size(17.0).color(pal.text);
    let weak = |t: String| RichText::new(t).size(13.0).color(pal.text_weak);
    egui::Modal::new(egui::Id::unique("import_from"))
        .backdrop_color(Color32::from_black_alpha(160))
        .frame(frame(&pal))
        .show(ctx, |ui| {
            ui.set_max_width(500.0);
            match &mut lib.foreign.stage {
                Stage::Idle => {}
                Stage::Reading { source, path, .. } => {
                    ui.label(title(format!("Import from {}", source.name())));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(weak(format!(
                            "Reading {} and the tags of its tracks…",
                            path.display()
                        )));
                    });
                    ui.add_space(16.0);
                    let c = button(ui, "Cancel", false, None);
                    marks.push(("import_cancel", c.rect, ui.clip_rect()));
                    if c.clicked() {
                        answer = Answer::Close;
                    }
                }
                Stage::Ready(r) => {
                    ui.label(title(format!("Import from {}", r.source.name())));
                    ui.add_space(4.0);
                    ui.label(weak(r.path.display().to_string()));
                    ui.add_space(10.0);
                    let found = r.foreign.tracks.len();
                    let mut line = format!(
                        "{found} track{} and {} playlist{}",
                        if found == 1 { "" } else { "s" },
                        r.playlists(),
                        if r.playlists() == 1 { "" } else { "s" }
                    );
                    let missing = r.not_found();
                    if missing > 0 {
                        line.push_str(&format!(
                            "; {missing} of the files are not on this computer (they come in marked \u{201c}not found\u{201d}, Locate finds them later)"
                        ));
                    }
                    ui.label(RichText::new(line).size(14.0).color(pal.text));
                    ui.add_space(6.0);
                    ui.label(weak(
                        "Tracks already in the library keep their tags, cues, loops and the \
                         beatgrids you set; they take the rating, colour and play count when \
                         they have none."
                            .into(),
                    ));
                    ui.add_space(12.0);
                    if r.source.root_crate().is_some() {
                        ui.horizontal(|ui| {
                            let s = switch(ui, r.use_root);
                            marks.push(("import_use_root", s.rect, ui.clip_rect()));
                            if s.clicked() {
                                r.use_root = !r.use_root;
                            }
                            ui.label(RichText::new("Playlists go in the crate").color(pal.text));
                            let t = ui.add_enabled(
                                r.use_root,
                                egui::TextEdit::singleline(&mut r.root).desired_width(160.0),
                            );
                            marks.push(("import_root", t.rect, ui.clip_rect()));
                        });
                        ui.add_space(4.0);
                    }
                    ui.horizontal(|ui| {
                        let s = switch(ui, r.replace_cues);
                        marks.push(("import_replace_cues", s.rect, ui.clip_rect()));
                        if s.clicked() {
                            r.replace_cues = !r.replace_cues;
                        }
                        ui.label(
                            RichText::new("Replace the cues and loops of tracks that have some")
                                .color(pal.text),
                        );
                    });
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        let s = switch(ui, r.replace_grids);
                        marks.push(("import_replace_grids", s.rect, ui.clip_rect()));
                        if s.clicked() {
                            r.replace_grids = !r.replace_grids;
                        }
                        ui.label(
                            RichText::new("Replace the beatgrids you set or edited")
                                .color(pal.text),
                        );
                    });
                    if !r.foreign.warnings.is_empty() {
                        ui.add_space(8.0);
                        ui.label(weak(format!(
                            "{} note{} (listed after the import)",
                            r.foreign.warnings.len(),
                            if r.foreign.warnings.len() == 1 { "" } else { "s" }
                        )));
                    }
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        let b = button(ui, "Import", true, None);
                        marks.push(("import_confirm", b.rect, ui.clip_rect()));
                        if b.clicked() {
                            answer = Answer::Import;
                        }
                        let c = button(ui, "Cancel", false, None);
                        marks.push(("import_cancel", c.rect, ui.clip_rect()));
                        if c.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                            answer = Answer::Close;
                        }
                    });
                }
                Stage::Done {
                    source,
                    report,
                    warnings,
                } => {
                    ui.label(title(format!("Imported from {}", source.name())));
                    ui.add_space(8.0);
                    let rows = [
                        ("New tracks", report.tracks_added),
                        ("Already in the library", report.tracks_matched),
                        ("Files not found", report.missing_files),
                        ("Cues", report.cues),
                        ("Loops", report.loops),
                        ("Beatgrids", report.grids),
                        ("New crates", report.crates),
                        ("Tracks put in crates", report.crate_entries),
                    ];
                    egui::Grid::new("import_report")
                        .num_columns(2)
                        .spacing([24.0, 3.0])
                        .show(ui, |ui| {
                            for (k, v) in rows {
                                ui.label(RichText::new(k).color(pal.text_weak));
                                ui.label(RichText::new(v.to_string()).color(pal.text));
                                ui.end_row();
                            }
                        });
                    if !warnings.is_empty() {
                        ui.add_space(10.0);
                        ui.label(RichText::new("Notes").color(pal.text));
                        egui::ScrollArea::vertical()
                            .max_height(160.0)
                            .show(ui, |ui| {
                                for w in warnings.iter().take(500) {
                                    ui.label(weak(w.clone()));
                                }
                                if warnings.len() > 500 {
                                    ui.label(weak(format!("… and {} more", warnings.len() - 500)));
                                }
                            });
                    }
                    ui.add_space(16.0);
                    let b = button(ui, "Close", true, None);
                    marks.push(("import_close", b.rect, ui.clip_rect()));
                    if b.clicked()
                        || ui.input(|i| {
                            i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter)
                        })
                    {
                        answer = Answer::Close;
                    }
                }
                Stage::Failed(e) => {
                    ui.label(title("The import did not work".into()));
                    ui.add_space(6.0);
                    ui.label(weak(e.clone()));
                    ui.add_space(16.0);
                    let b = button(ui, "Close", true, None);
                    marks.push(("import_close", b.rect, ui.clip_rect()));
                    if b.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        answer = Answer::Close;
                    }
                }
            }
        });
    for (n, r, c) in marks {
        app.map.add_rect(n, r, c);
    }
    match answer {
        Answer::None => {}
        Answer::Import => app.import_foreign_now(),
        Answer::Close => {
            if let Some(lib) = app.library.as_mut() {
                // A read still running finishes on its own; its answer is
                // dropped with the receiver.
                lib.foreign.stage = Stage::Idle;
            }
        }
    }
}
