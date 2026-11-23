//! UI-side library state: the open `Library`, the analysis worker pool, a
//! background folder importer and the current filtered/sorted view.

use dj_analysis::AnalysisOptions;
use dj_library::{
    AnalysisJob, AnalysisQueue, CrateId, CrateNode, Library, Track, TrackId, TrackMeta, read_meta,
    scan_folder,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;

/// Points of room under the last track in the list, so the last row never
/// ends on the list's bottom edge (where scaling and the window's border
/// cut it off).
pub const LIST_END_PAD: f32 = 10.0;

/// The two tabs of the crate list: the user's crates, and rekordbox's
/// playlists kept in step with rekordbox on this computer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrateTab {
    #[default]
    Crates,
    Rekordbox,
}

/// Which tracks the list shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    All,
    Crate(CrateId),
}

/// Sortable columns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortKey {
    Artist,
    Title,
    Bpm,
    Key,
    Time,
    Album,
    Genre,
    Added,
    /// Crate order inside a crate, import order in "All tracks" (the #
    /// column).
    Order,
    /// File found or not (missing ones last when ascending).
    Found,
    Year,
    Comment,
    Bitrate,
    Plays,
    FileName,
    Location,
}

/// A column of the track list. Which ones are shown, in which order, is a
/// setting (`ui.library_columns`, by [`LibColumn::id`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibColumn {
    /// Row number in the list as it is shown.
    Number,
    /// File found / not found icon.
    Found,
    Artist,
    Title,
    /// A small waveform of the whole track with its hot cues (as in
    /// rekordbox's track list).
    Waveform,
    Bpm,
    Key,
    Time,
    Album,
    Genre,
    Added,
    Year,
    Comment,
    Bitrate,
    Plays,
    FileName,
    Location,
}

impl LibColumn {
    /// Every column, in the order they take when switched on.
    pub const ALL: [LibColumn; 17] = [
        Self::Number,
        Self::Found,
        Self::Artist,
        Self::Title,
        Self::Waveform,
        Self::Bpm,
        Self::Key,
        Self::Time,
        Self::Album,
        Self::Genre,
        Self::Added,
        Self::Year,
        Self::Comment,
        Self::Bitrate,
        Self::Plays,
        Self::FileName,
        Self::Location,
    ];

    /// The columns shown when nothing is set.
    pub const DEFAULT: [LibColumn; 11] = [
        Self::Number,
        Self::Found,
        Self::Artist,
        Self::Title,
        Self::Waveform,
        Self::Bpm,
        Self::Key,
        Self::Time,
        Self::Album,
        Self::Genre,
        Self::Added,
    ];

    /// The name in the config file.
    pub fn id(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Found => "found",
            Self::Artist => "artist",
            Self::Title => "title",
            Self::Waveform => "waveform",
            Self::Bpm => "bpm",
            Self::Key => "key",
            Self::Time => "time",
            Self::Album => "album",
            Self::Genre => "genre",
            Self::Added => "added",
            Self::Year => "year",
            Self::Comment => "comment",
            Self::Bitrate => "bitrate",
            Self::Plays => "plays",
            Self::FileName => "file",
            Self::Location => "location",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }

    /// The name in the header and the column menu.
    pub fn name(self) -> &'static str {
        match self {
            Self::Number => "#",
            Self::Found => "Found",
            Self::Artist => "Artist",
            Self::Title => "Title",
            Self::Waveform => "Waveform",
            Self::Bpm => "BPM",
            Self::Key => "Key",
            Self::Time => "Time",
            Self::Album => "Album",
            Self::Genre => "Genre",
            Self::Added => "Added",
            Self::Year => "Year",
            Self::Comment => "Comment",
            Self::Bitrate => "Bitrate",
            Self::Plays => "Plays",
            Self::FileName => "File name",
            Self::Location => "Location",
        }
    }

    /// What the column menu says about it (when the name is not enough).
    pub fn menu_label(self) -> &'static str {
        match self {
            Self::Number => "# (row number)",
            Self::Found => "File found (icon)",
            Self::Location => "Location (folder)",
            Self::Waveform => "Waveform (with hot cues)",
            c => c.name(),
        }
    }

    /// Width before the list is fitted to its space (points).
    pub fn width(self) -> f32 {
        match self {
            Self::Number => 38.0,
            Self::Found => 22.0,
            Self::Artist => 170.0,
            Self::Title => 230.0,
            Self::Waveform => 170.0,
            Self::Bpm => 58.0,
            Self::Key => 44.0,
            Self::Time => 52.0,
            Self::Album => 160.0,
            Self::Genre => 100.0,
            Self::Added => 80.0,
            Self::Year => 46.0,
            Self::Comment => 160.0,
            Self::Bitrate => 70.0,
            Self::Plays => 46.0,
            Self::FileName => 180.0,
            Self::Location => 220.0,
        }
    }

    /// Share of the room left over (and of the shrinking on a narrow
    /// list).
    pub fn flex(self) -> f32 {
        match self {
            Self::Artist => 0.4,
            Self::Title => 0.6,
            Self::Comment => 0.15,
            Self::FileName => 0.2,
            Self::Location => 0.25,
            _ => 0.0,
        }
    }

    /// On a narrow list, columns with a higher rank are hidden first;
    /// `None` is never hidden.
    pub fn hide_rank(self) -> Option<u8> {
        match self {
            Self::Waveform => Some(1),
            Self::Album => Some(2),
            Self::Genre => Some(3),
            Self::Added => Some(4),
            Self::Year => Some(5),
            Self::Plays => Some(6),
            Self::Bitrate => Some(7),
            Self::FileName => Some(8),
            Self::Comment => Some(9),
            Self::Location => Some(10),
            _ => None,
        }
    }

    pub fn sort_key(self) -> SortKey {
        match self {
            Self::Number => SortKey::Order,
            Self::Found => SortKey::Found,
            Self::Artist => SortKey::Artist,
            Self::Title => SortKey::Title,
            // Not sortable (the header does nothing).
            Self::Waveform => SortKey::Order,
            Self::Bpm => SortKey::Bpm,
            Self::Key => SortKey::Key,
            Self::Time => SortKey::Time,
            Self::Album => SortKey::Album,
            Self::Genre => SortKey::Genre,
            Self::Added => SortKey::Added,
            Self::Year => SortKey::Year,
            Self::Comment => SortKey::Comment,
            Self::Bitrate => SortKey::Bitrate,
            Self::Plays => SortKey::Plays,
            Self::FileName => SortKey::FileName,
            Self::Location => SortKey::Location,
        }
    }

    /// The columns from the config: unknown names and repeats are skipped;
    /// nothing left gives the default columns.
    pub fn from_config(ids: &[String]) -> Vec<LibColumn> {
        let mut out: Vec<LibColumn> = Vec::new();
        for c in ids.iter().filter_map(|s| Self::from_id(s)) {
            if !out.contains(&c) {
                out.push(c);
            }
        }
        if out.is_empty() {
            out = Self::DEFAULT.to_vec();
        }
        out
    }

    pub fn to_config(cols: &[LibColumn]) -> Vec<String> {
        cols.iter().map(|c| c.id().to_string()).collect()
    }

    /// Switches `c` on or off. A column switched on takes its place from
    /// [`Self::ALL`] among the ones shown; the last column shown stays.
    pub fn toggle(cols: &mut Vec<LibColumn>, c: LibColumn) {
        if let Some(i) = cols.iter().position(|x| *x == c) {
            if cols.len() > 1 {
                cols.remove(i);
            }
            return;
        }
        let rank = |x: LibColumn| Self::ALL.iter().position(|a| *a == x).unwrap_or(0);
        let at = cols
            .iter()
            .position(|x| rank(*x) > rank(c))
            .unwrap_or(cols.len());
        cols.insert(at, c);
    }
}

/// A crate under a track being dragged (for the drop outline and the hint
/// next to the pointer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrateDrop {
    pub id: CrateId,
    /// The crate's row in the crate list.
    pub rect: egui::Rect,
    /// The dragged track is in this crate already.
    pub already: bool,
}

enum ImportMsg {
    Progress(usize, usize),
    Batch(Vec<TrackMeta>),
    /// Every audio file found, in folder order (new and known ones).
    Files(Vec<PathBuf>),
    Done,
}

/// A folder import running on a background thread. The thread reads files
/// and tags; the UI thread writes them to the database in batches.
struct ImportJob {
    rx: mpsc::Receiver<ImportMsg>,
    added: Vec<TrackId>,
    progress: (usize, usize),
    /// The crate the imported tracks go into (folder dropped on a crate).
    target: Option<CrateId>,
    /// Every audio file of the folder (from the thread, at the end).
    files: Vec<PathBuf>,
}

impl ImportJob {
    fn start(
        root: PathBuf,
        known: HashMap<PathBuf, (u64, i64)>,
        target: Option<CrateId>,
    ) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("kaldor-import".into())
            .spawn(move || {
                let files = scan_folder(&root, true);
                let total = files.len();
                let _ = tx.send(ImportMsg::Files(files.clone()));
                let mut batch = Vec::new();
                for (i, p) in files.into_iter().enumerate() {
                    let m = read_meta(&p);
                    if known.get(&p) != Some(&(m.file_size, m.file_mtime)) {
                        batch.push(m);
                    }
                    if batch.len() >= 200 {
                        if tx
                            .send(ImportMsg::Batch(std::mem::take(&mut batch)))
                            .is_err()
                        {
                            return;
                        }
                        let _ = tx.send(ImportMsg::Progress(i + 1, total));
                    }
                }
                let _ = tx.send(ImportMsg::Batch(batch));
                let _ = tx.send(ImportMsg::Progress(total, total));
                let _ = tx.send(ImportMsg::Done);
            })?;
        Ok(Self {
            rx,
            added: Vec::new(),
            progress: (0, 0),
            target,
            files: Vec::new(),
        })
    }
}

/// Where dropped files or folders go.
#[derive(Clone, Debug, PartialEq)]
pub enum ImportTarget {
    /// The library only.
    Library,
    Crate(CrateId),
    /// A new crate with this name (the folder's name).
    NewCrate(String),
}

impl ImportTarget {
    /// A new crate gets the name of the first folder dropped ("New crate"
    /// for files).
    pub fn named(self, paths: &[PathBuf]) -> Self {
        match self {
            Self::NewCrate(n) if n.is_empty() => Self::NewCrate(
                paths
                    .iter()
                    .find(|p| p.is_dir())
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "New crate".into()),
            ),
            t => t,
        }
    }
}

/// A crate being renamed in the crate tree.
#[derive(Clone, Debug, PartialEq)]
pub struct CrateRename {
    pub id: CrateId,
    pub name: String,
    /// The text field still has to take the keyboard focus.
    pub focus_pending: bool,
}

/// Library state owned by the UI.
pub struct LibraryUi {
    pub lib: Library,
    queue: AnalysisQueue,
    tracks: HashMap<TrackId, Track>,
    pub crates: Vec<CrateNode>,
    pub selection: Selection,
    pub search: String,
    last_search: String,
    view: Vec<TrackId>,
    /// Each track's # in the selected crate's order (see `rebuild_view`).
    order_no: HashMap<TrackId, usize>,
    view_dirty: bool,
    pub sort: (SortKey, bool),
    import: Option<ImportJob>,
    /// The message of the status line (shown for [`STATUS_SECS`], see
    /// [`Self::fresh_status`]).
    pub status: String,
    /// The status message shown and since when.
    status_since: Option<(String, std::time::Instant)>,
    pub new_crate: String,
    /// Crate being renamed in place, with the name being typed.
    pub renaming: Option<CrateRename>,
    pub selected_track: Option<TrackId>,
    analysis_total: usize,
    /// The controller's browse knob moves the crate selection (true) or
    /// the track selection.
    pub browse_crates: bool,
    /// Where the track list is scrolled and how tall it is (points), from
    /// the last frame; and a scroll the list should make (browse knob).
    pub list_scroll: (f32, f32),
    pub scroll_to: Option<f32>,
    /// Height of one track row including the spacing between rows.
    pub row_pitch: f32,
    /// The browse knob has been used: show which list it is in.
    pub knob_used: bool,
    /// The rekordbox USB export dialog.
    pub usb: crate::usb_export::UsbExport,
    /// Analyse new tracks when they are imported (off: when first loaded).
    pub analyse_on_import: bool,
    /// No edits to cues, grids or crates (Settings > Library).
    pub protect: bool,
    /// Whether each track's file is there (checked in the background).
    pub files: crate::file_check::FileChecker,
    /// The tracks in the rows on screen (last frame), checked first.
    pub visible_rows: Vec<TrackId>,
    /// The crate under a dragged track (this frame).
    pub crate_drop: Option<CrateDrop>,
    /// A crate that just took a dropped track: it flashes.
    pub crate_flash: Option<(CrateId, std::time::Instant)>,
    /// The Locate window (a moved file), when open.
    pub locate: Option<crate::relink::Locate>,
    pub(crate) moved_search: Option<crate::relink::MovedSearch>,
    /// A track dragged inside a crate's list: where it would go (index in
    /// the list as shown, 0 = before the first row).
    pub list_drop: Option<usize>,
    /// A track dragged over the list where it cannot be moved: why.
    pub list_hint: Option<String>,
    /// A column header being dragged: the column and where it would go
    /// (index in the shown columns).
    pub column_drag: Option<(LibColumn, usize)>,
    /// More tracks selected with Ctrl / Shift + click or Ctrl+A (besides
    /// `selected_track`, the one clicked last).
    pub picked: Vec<TrackId>,
    /// Where a Shift + click range starts.
    anchor: Option<TrackId>,
    /// The track list was clicked last (Delete and Ctrl+A act on it).
    pub list_focus: bool,
    /// Crates selected with Ctrl / Shift + click or Ctrl+A. Empty: the
    /// open crate (`selection`) alone. See [`Self::selected_crates`].
    pub picked_crates: Vec<CrateId>,
    /// Where a Shift + click range of crates starts.
    crate_anchor: Option<CrateId>,
    /// The crate list was clicked last: Ctrl+A selects the crates there,
    /// not the tracks.
    pub crates_focus: bool,
    /// Where the last track row ends on screen (last frame): a click below
    /// it, in the empty part of the list, clears the selection.
    pub rows_bottom: f32,
    /// Tracks waiting for "Remove from library?" to be answered.
    pub confirm_remove: Option<Vec<TrackId>>,
    /// "Reset the library?" is asked; the bool: also forget the watch
    /// folders.
    pub confirm_reset: Option<bool>,
    /// Folders waiting to be imported (and the crate they go into).
    import_queue: std::collections::VecDeque<(PathBuf, Option<CrateId>)>,
    /// Library tracks by file path (the Files panel marks them).
    by_path: HashMap<PathBuf, TrackId>,
    /// The small waveforms of the Waveform column.
    pub minis: crate::library_wave::MiniWaves,
    /// Where things can be dropped, from the last frame (also for files
    /// dropped from the file manager): every crate row, the empty space
    /// under the crates (a new crate), "All tracks" and the track list.
    pub drop_areas: DropAreas,
    /// Import from… (another program's library or a playlist file) and
    /// Save as playlist.
    pub foreign: crate::interop_view::ForeignImport,
    /// The smart crate window, while a smart crate is made or edited.
    pub smart_edit: Option<crate::smart_view::SmartEdit>,
    /// The History panel (over the library's top part), while it is open.
    pub history: Option<crate::history_view::HistoryView>,
    /// Crates whose sub-crates are hidden in the crate list (the arrow in
    /// front of the name; saved in the config by the app).
    pub collapsed: std::collections::HashSet<CrateId>,
    /// Crates with sub-crates that were opened (saved in the config); every
    /// other one is closed ([`Self::refold`]).
    pub opened: std::collections::HashSet<CrateId>,
    /// The tab the crate list shows.
    pub crate_tab: CrateTab,
    /// The browse knob moved the crate selection: the crate list scrolls
    /// to it (this frame).
    pub crate_scroll: bool,
    /// rekordbox's crate (its whole collection) and its playlists under it,
    /// kept by the sync; not in [`Self::crates`].
    pub rb_root: Option<CrateId>,
    pub rb_crates: Vec<CrateNode>,
    /// Watches and reads rekordbox's library.
    pub rekordbox: crate::rekordbox_sync::RekordboxSync,
}

/// Seconds a message stays in the library's status line (as in the top
/// bar).
pub const STATUS_SECS: u64 = 10;

/// The library's drop areas in the last frame.
#[derive(Clone, Debug, Default)]
pub struct DropAreas {
    pub crates: Vec<(CrateId, egui::Rect)>,
    pub new_crate: Option<egui::Rect>,
    pub all_tracks: Option<egui::Rect>,
    pub list: Option<egui::Rect>,
}

impl DropAreas {
    /// What a drop at `p` would do.
    pub fn target_at(&self, p: egui::Pos2, selection: Selection) -> Option<ImportTarget> {
        if let Some((c, _)) = self.crates.iter().find(|(_, r)| r.contains(p)) {
            return Some(ImportTarget::Crate(*c));
        }
        if self.new_crate.is_some_and(|r| r.contains(p)) {
            return Some(ImportTarget::NewCrate(String::new()));
        }
        if self.all_tracks.is_some_and(|r| r.contains(p)) {
            return Some(ImportTarget::Library);
        }
        if self.list.is_some_and(|r| r.contains(p)) {
            return Some(match selection {
                Selection::Crate(c) => ImportTarget::Crate(c),
                Selection::All => ImportTarget::Library,
            });
        }
        None
    }
}

impl LibraryUi {
    /// Wraps an open library and queues analysis for tracks that need it.
    pub fn new(lib: Library, opts: AnalysisOptions, workers: usize) -> Self {
        Self::with_settings(lib, opts, workers, true, false)
    }

    /// The library state with nothing loaded or queued yet (`workers` > 0).
    fn new_unqueued(lib: Library, opts: AnalysisOptions, workers: usize) -> Self {
        Self {
            lib,
            queue: AnalysisQueue::new(workers, opts),
            tracks: HashMap::new(),
            crates: Vec::new(),
            selection: Selection::All,
            search: String::new(),
            last_search: String::new(),
            view: Vec::new(),
            order_no: HashMap::new(),
            view_dirty: true,
            sort: (SortKey::Added, false),
            import: None,
            status: String::new(),
            status_since: None,
            new_crate: String::new(),
            renaming: None,
            selected_track: None,
            analysis_total: 0,
            browse_crates: false,
            list_scroll: (0.0, 0.0),
            scroll_to: None,
            row_pitch: 22.0,
            knob_used: false,
            usb: crate::usb_export::UsbExport::default(),
            analyse_on_import: true,
            protect: false,
            files: crate::file_check::FileChecker::new(),
            visible_rows: Vec::new(),
            crate_drop: None,
            crate_flash: None,
            locate: None,
            moved_search: None,
            list_drop: None,
            list_hint: None,
            column_drag: None,
            picked: Vec::new(),
            anchor: None,
            list_focus: false,
            picked_crates: Vec::new(),
            crate_anchor: None,
            crates_focus: false,
            rows_bottom: 0.0,
            confirm_remove: None,
            confirm_reset: None,
            import_queue: Default::default(),
            by_path: HashMap::new(),
            minis: Default::default(),
            drop_areas: DropAreas::default(),
            foreign: Default::default(),
            smart_edit: None,
            history: None,
            collapsed: Default::default(),
            opened: Default::default(),
            crate_tab: CrateTab::default(),
            crate_scroll: false,
            rb_root: None,
            rb_crates: Vec::new(),
            rekordbox: Default::default(),
        }
    }

    /// Takes a finished read of rekordbox's library into the library (the
    /// rekordbox tab), see [`Library::sync_foreign`].
    pub fn apply_rekordbox(&mut self, db: dj_library::interop::rekordbox_db::RekordboxDb) {
        use crate::rekordbox_sync::SOURCE;
        let mut playlists = 0;
        fn count(nodes: &[dj_library::interop::ForeignNode], n: &mut usize) {
            for x in nodes {
                match x {
                    dj_library::interop::ForeignNode::Folder { children, .. } => count(children, n),
                    dj_library::interop::ForeignNode::Playlist { .. } => *n += 1,
                }
            }
        }
        count(&db.library.playlists, &mut playlists);
        match self
            .lib
            .sync_foreign(SOURCE, "rekordbox", &db.library, &db.ids, &db.fingerprints)
        {
            Ok(report) => {
                // Hot cues D-H an earlier version put on the wrong pad,
                // where you have not changed them since.
                match self.lib.fix_misplaced_cues(&db.library, &db.misplaced_cues) {
                    Ok(n) if n > 0 => {
                        self.status = format!("rekordbox: hot cues D-H put right on {n} tracks");
                    }
                    Ok(_) => {}
                    Err(e) => self.status = format!("library error: {e}"),
                }
                if report.changed() {
                    self.reload();
                    if self.analyse_on_import {
                        for id in &report.to_analyse {
                            self.analyse_if_needed(*id);
                        }
                    }
                    if report.tracks_added > 0 {
                        self.status = format!(
                            "rekordbox: {} new tracks{}",
                            report.tracks_added,
                            if report.missing_files > 0 {
                                format!(", {} not found on this computer", report.missing_files)
                            } else {
                                String::new()
                            }
                        );
                    }
                }
                self.rekordbox.last = crate::rekordbox_sync::LastSync {
                    at: Some(std::time::Instant::now()),
                    tracks: report.tracks,
                    playlists,
                    smart_left_out: db.smart_playlists,
                    notes: db.library.warnings,
                };
            }
            Err(e) => {
                self.rekordbox.state =
                    crate::rekordbox_sync::SyncState::Failed(format!("library error: {e}"));
            }
        }
    }

    /// The status message while it is new ([`STATUS_SECS`]). After that it
    /// is cleared, so the same message set again shows again.
    pub fn fresh_status(&mut self) -> Option<String> {
        if self.status.is_empty() {
            self.status_since = None;
            return None;
        }
        let now = std::time::Instant::now();
        match &self.status_since {
            Some((m, _)) if *m == self.status => {}
            _ => self.status_since = Some((self.status.clone(), now)),
        }
        let since = self.status_since.as_ref().map_or(now, |(_, t)| *t);
        if now.duration_since(since) >= std::time::Duration::from_secs(STATUS_SECS) {
            self.status.clear();
            self.status_since = None;
            return None;
        }
        Some(self.status.clone())
    }

    /// Makes the status message `secs` older (tests).
    pub fn age_status_for_test(&mut self, secs: u64) {
        if let Some((_, t)) = self.status_since.as_mut()
            && let Some(old) = t.checked_sub(std::time::Duration::from_secs(secs))
        {
            *t = old;
        }
    }

    /// The library track of a file, if it is in the library.
    pub fn track_of_path(&self, p: &std::path::Path) -> Option<TrackId> {
        self.by_path.get(p).copied()
    }

    /// A click on a track row: plain selects it alone, Ctrl / Cmd adds or
    /// takes it away, Shift selects the rows from the last plain click.
    pub fn click_track(&mut self, id: TrackId, ctrl: bool, shift: bool) {
        self.list_focus = true;
        self.crates_focus = false;
        if shift && let Some(a) = self.anchor {
            let view = self.view().to_vec();
            if let (Some(i), Some(j)) = (
                view.iter().position(|t| *t == a),
                view.iter().position(|t| *t == id),
            ) {
                let (lo, hi) = (i.min(j), i.max(j));
                self.picked = view[lo..=hi].to_vec();
                self.selected_track = Some(id);
                return;
            }
        }
        if ctrl {
            // The single selection joins the set first.
            if self.picked.is_empty()
                && let Some(s) = self.selected_track
            {
                self.picked.push(s);
            }
            if let Some(k) = self.picked.iter().position(|t| *t == id) {
                self.picked.remove(k);
                if self.selected_track == Some(id) {
                    self.selected_track = self.picked.last().copied();
                }
            } else {
                self.picked.push(id);
                self.selected_track = Some(id);
            }
            self.anchor = Some(id);
            return;
        }
        self.picked.clear();
        self.selected_track = Some(id);
        self.anchor = Some(id);
    }

    /// The track is selected (alone or with others).
    pub fn is_picked(&self, id: TrackId) -> bool {
        self.selected_track == Some(id) || self.picked.contains(&id)
    }

    /// The selected tracks in list order.
    pub fn picked_in_view(&mut self) -> Vec<TrackId> {
        let sel = self.selected_track;
        let picked = self.picked.clone();
        self.view()
            .iter()
            .copied()
            .filter(|t| Some(*t) == sel || picked.contains(t))
            .collect()
    }

    /// What a drag of `id` carries: the whole selection when `id` is in
    /// it, else `id` alone.
    pub fn drag_set(&mut self, id: TrackId) -> Vec<TrackId> {
        if self.picked.contains(&id) {
            self.picked_in_view()
        } else {
            vec![id]
        }
    }

    /// Ctrl+A: every track in the list.
    pub fn select_all(&mut self) {
        self.picked = self.view().to_vec();
        if self.selected_track.is_none() {
            self.selected_track = self.picked.first().copied();
        }
    }

    /// Forgets the selection (another crate, a search, ...).
    pub fn clear_picked(&mut self) {
        self.picked.clear();
        self.anchor = None;
    }

    /// No track selected (a click anywhere but on a track).
    pub fn deselect(&mut self) {
        self.clear_picked();
        self.selected_track = None;
    }

    /// Ctrl+A: every track in the list, or none when they all are already.
    pub fn toggle_all(&mut self) {
        let view = self.view().to_vec();
        if !view.is_empty() && view.iter().all(|t| self.is_picked(*t)) {
            self.deselect();
        } else {
            self.select_all();
        }
    }

    /// A click on a crate. Plain: it alone is selected and opened. Ctrl /
    /// Cmd: it joins the selected crates (or leaves them). Shift: the crates
    /// from the last click to this one, in the list's order. The track list
    /// shows the crate clicked last (when one taken away was open, the last
    /// one still selected).
    pub fn click_crate(&mut self, id: CrateId, ctrl: bool, shift: bool) {
        self.crates_focus = true;
        let mut open = Some(id);
        if shift && let Some(a) = self.crate_anchor {
            // The rows on screen: not the sub-crates of closed crates.
            let order: Vec<CrateId> = self.visible_crates();
            if let (Some(i), Some(j)) = (
                order.iter().position(|c| *c == a),
                order.iter().position(|c| *c == id),
            ) {
                let (lo, hi) = (i.min(j), i.max(j));
                self.picked_crates = order[lo..=hi].to_vec();
            }
        } else if ctrl {
            // The open crate joins the set first.
            if self.picked_crates.is_empty()
                && let Selection::Crate(o) = self.selection
            {
                self.picked_crates.push(o);
            }
            if let Some(k) = self.picked_crates.iter().position(|c| *c == id) {
                self.picked_crates.remove(k);
                open = if self.selection == Selection::Crate(id) {
                    self.picked_crates.last().copied()
                } else {
                    None
                };
            } else {
                self.picked_crates.push(id);
            }
            self.crate_anchor = Some(id);
        } else {
            self.picked_crates.clear();
            self.crate_anchor = Some(id);
        }
        if let Some(o) = open {
            self.open_crate(Selection::Crate(o));
        }
    }

    /// Shows `s` in the track list (no tracks selected, a crate in its own
    /// order).
    pub fn open_crate(&mut self, s: Selection) {
        if self.selection != s {
            self.deselect();
            self.view_dirty = true;
        }
        self.selection = s;
        if matches!(s, Selection::Crate(_)) {
            self.sort = (SortKey::Order, true);
        }
    }

    /// The crates selected, in the list's order: the ones picked with Ctrl,
    /// Shift or Ctrl+A, else the open crate (none when All tracks is open).
    pub fn selected_crates(&self) -> Vec<CrateId> {
        if self.picked_crates.is_empty() {
            match self.selection {
                Selection::Crate(c) => vec![c],
                Selection::All => Vec::new(),
            }
        } else {
            self.crate_list()
                .into_iter()
                .map(|c| c.0)
                .filter(|c| self.picked_crates.contains(c))
                .collect()
        }
    }

    /// Ctrl+A in the crate list: every crate, or back to the open one
    /// alone when they all are already.
    pub fn toggle_all_crates(&mut self) {
        let all: Vec<CrateId> = self.crate_list().into_iter().map(|c| c.0).collect();
        if !all.is_empty() && all.iter().all(|c| self.picked_crates.contains(c)) {
            self.picked_crates.clear();
        } else {
            self.picked_crates = all;
        }
    }

    /// Back to the open crate alone.
    pub fn clear_picked_crates(&mut self) {
        self.picked_crates.clear();
        self.crate_anchor = None;
    }

    /// Removes tracks from the library and every crate (the files stay).
    /// Returns the message for the status line.
    pub fn remove_from_library(&mut self, ids: &[TrackId]) -> Result<String, String> {
        if self.protect {
            return Err(
                "Library is protected: tracks cannot be removed (Settings > Library)".into(),
            );
        }
        let one = (ids.len() == 1)
            .then(|| self.track(ids[0]).map(|t| t.display_name()))
            .flatten();
        self.lib.remove_tracks(ids).map_err(|e| e.to_string())?;
        self.picked.retain(|t| !ids.contains(t));
        if self.selected_track.is_some_and(|t| ids.contains(&t)) {
            self.selected_track = None;
        }
        self.reload();
        Ok(match one {
            Some(name) => {
                format!("Removed \u{201c}{name}\u{201d} from the library (the file stays)")
            }
            None => format!(
                "Removed {} tracks from the library (the files stay)",
                ids.len()
            ),
        })
    }

    /// Empties the library (a backup is made first, next to the library
    /// file). Returns the message for the status line.
    pub fn reset_library(&mut self) -> Result<String, String> {
        if self.protect {
            return Err("Library is protected: it cannot be reset (Settings > Library)".into());
        }
        let backup = match self.lib.file_path() {
            Some(db) => {
                let dir = db
                    .parent()
                    .map(|d| d.join("backups"))
                    .unwrap_or_else(|| PathBuf::from("backups"));
                std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                let stamp = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs());
                let dest = dir.join(format!("library-before-reset-{stamp}.db"));
                dj_library::Library::backup_file(&db, &dest).map_err(|e| e.to_string())?;
                Some(dest)
            }
            None => None,
        };
        let count = self.track_count();
        self.lib.clear().map_err(|e| e.to_string())?;
        self.selection = Selection::All;
        self.selected_track = None;
        self.clear_picked();
        self.reload();
        Ok(match backup {
            Some(b) => format!(
                "Library reset: {count} tracks removed (the files stay). Backup: {}",
                b.display()
            ),
            None => format!("Library reset: {count} tracks removed (the files stay)"),
        })
    }

    /// Why the tracks in the list cannot be dragged into a new order now
    /// (`None`: they can). Only a crate's whole list, in its own order.
    pub fn reorder_blocked(&self) -> Option<&'static str> {
        let Selection::Crate(c) = self.selection else {
            return Some("Open a crate to put its tracks in order");
        };
        if self.is_smart(c) {
            return Some("A smart crate keeps the library's order");
        }
        if self.is_synced(c) {
            return Some("rekordbox's playlists keep rekordbox's order (change it there)");
        }
        if self.has_sub_crates(c) {
            return Some("A folder shows its crates' tracks: put them in order in each crate");
        }
        if self.protect {
            return Some("Library is protected");
        }
        if self.sort.0 != SortKey::Order {
            return Some("Sort by # to change the order");
        }
        if !self.search.trim().is_empty() {
            return Some("Clear the search to change the order");
        }
        None
    }

    /// A track's # (its place in the crate's own order, from 1), as of the
    /// last time the list was built.
    pub fn order_number(&self, id: TrackId) -> Option<usize> {
        self.order_no.get(&id).copied()
    }

    /// Every track in the library (no order).
    pub fn all_tracks(&self) -> impl Iterator<Item = &Track> {
        self.tracks.values()
    }

    /// Crate `id` is a smart crate (as of the last reload).
    /// rekordbox's collection or one of its playlists (read-only here).
    pub fn is_synced(&self, id: CrateId) -> bool {
        Some(id) == self.rb_root || self.rb_crate_ids().contains(&id)
    }

    pub fn is_smart(&self, id: CrateId) -> bool {
        fn find(nodes: &[CrateNode], id: CrateId) -> Option<bool> {
            nodes.iter().find_map(|n| {
                (n.id == id)
                    .then_some(n.smart)
                    .or_else(|| find(&n.children, id))
            })
        }
        find(&self.crates, id).unwrap_or(false)
    }

    /// Moves `track` to `to` in the list as shown (a crate sorted by #):
    /// `to` counts the gaps between rows, 0 = before the first row.
    pub fn move_in_view(&mut self, track: TrackId, to: usize) -> Result<(), String> {
        let Selection::Crate(c) = self.selection else {
            return Err("Tracks can only be moved inside a crate".into());
        };
        if self.protect {
            return Err(
                "Library is protected: crates cannot be changed (Settings > Library)".into(),
            );
        }
        if let Some(why) = self.reorder_blocked() {
            return Err(why.into());
        }
        let mut shown = self.view().to_vec();
        let Some(from) = shown.iter().position(|t| *t == track) else {
            return Ok(());
        };
        let t = shown.remove(from);
        let to = if to > from { to - 1 } else { to }.min(shown.len());
        shown.insert(to, t);
        // Shown descending: the crate's own order is the other way round.
        let asc = if self.sort.1 {
            to
        } else {
            shown.len() - 1 - to
        };
        self.lib
            .move_in_crate(c, track, asc)
            .map_err(|e| e.to_string())?;
        self.view_dirty = true;
        Ok(())
    }

    /// The file states of the tracks: answers from the checker, and new
    /// checks when they are due. With the list sorted by "Found", a
    /// change re-sorts it.
    pub fn check_files(&mut self) {
        let tracks = &self.tracks;
        let visible = || {
            self.visible_rows
                .iter()
                .filter_map(|id| tracks.get(id).map(|t| (*id, t.meta.path.clone())))
                .collect()
        };
        let all = || {
            tracks
                .values()
                .map(|t| (t.id, t.meta.path.clone()))
                .collect()
        };
        let changed = self.files.tick(std::time::Instant::now(), visible, all);
        if changed && self.sort.0 == SortKey::Found {
            self.view_dirty = true;
        }
    }

    /// The file state of a track.
    pub fn file_state(&self, id: TrackId) -> crate::file_check::FileState {
        self.files.get(id)
    }

    /// Like [`Self::new`], with the library settings; with analysis on
    /// import off nothing is queued now.
    pub fn with_settings(
        lib: Library,
        opts: AnalysisOptions,
        workers: usize,
        analyse_on_import: bool,
        protect: bool,
    ) -> Self {
        let workers = if workers == 0 {
            AnalysisQueue::default_workers()
        } else {
            workers
        };
        let mut s = Self::new_unqueued(lib, opts, workers);
        s.analyse_on_import = analyse_on_import;
        s.protect = protect;
        s.reload();
        if analyse_on_import {
            s.queue_missing_analysis();
        }
        s
    }

    /// Analyses a track now if it was never analysed (or by an older
    /// version): loading a track with analysis on import off.
    pub fn analyse_if_needed(&mut self, id: TrackId) {
        let needs = self.tracks.get(&id).is_some_and(|t| {
            t.analysis_version
                .is_none_or(|v| v < dj_analysis::ANALYSIS_VERSION)
        });
        if needs && let Some(t) = self.tracks.get(&id) {
            self.queue.submit(AnalysisJob {
                track: id,
                path: t.meta.path.clone(),
            });
            self.analysis_total += 1;
        }
    }

    /// Re-analyses the tracks of the selected crate (or every track).
    pub fn reanalyse_selection(&mut self) -> usize {
        let ids: Vec<TrackId> = match self.selection {
            Selection::All => self.tracks.keys().copied().collect(),
            Selection::Crate(c) => self.lib.shown_tracks(c).unwrap_or_default(),
        };
        for id in &ids {
            self.reanalyse(*id);
        }
        ids.len()
    }

    /// Re-reads tracks and crates from the database.
    pub fn reload(&mut self) {
        match self.lib.tracks() {
            Ok(ts) => {
                self.tracks = ts.into_iter().map(|t| (t.id, t)).collect();
                self.by_path = self
                    .tracks
                    .values()
                    .map(|t| (t.meta.path.clone(), t.id))
                    .collect();
            }
            Err(e) => self.status = format!("library error: {e}"),
        }
        match self.lib.crate_tree() {
            Ok(c) => {
                // Synced crates (rekordbox's) go to their own tab.
                let (synced, own): (Vec<CrateNode>, Vec<CrateNode>) =
                    c.into_iter().partition(|n| n.synced);
                self.crates = own;
                let root = synced.into_iter().next();
                self.rb_root = root.as_ref().map(|r| r.id);
                self.rb_crates = root.map(|r| r.children).unwrap_or_default();
            }
            Err(e) => self.status = format!("library error: {e}"),
        }
        // A deleted crate (a playlist removed in rekordbox) leaves the
        // track list.
        if let Selection::Crate(c) = self.selection
            && Some(c) != self.rb_root
            && !self.crate_list().iter().any(|x| x.0 == c)
            && !self.rb_crate_ids().contains(&c)
        {
            self.selection = Selection::All;
        }
        // Deleted crates leave the selection and the opened crates.
        if !self.picked_crates.is_empty() || !self.opened.is_empty() {
            let mut ids: Vec<CrateId> = self.crate_list().into_iter().map(|c| c.0).collect();
            ids.extend(self.rb_crate_ids());
            self.picked_crates.retain(|c| ids.contains(c));
            self.opened.retain(|c| ids.contains(c));
        }
        self.refold();
        self.view_dirty = true;
        // New tracks: check the files now, not at the next full pass.
        if self
            .tracks
            .keys()
            .any(|id| self.files.get(*id) == crate::file_check::FileState::Unknown)
        {
            self.files.tracks_changed();
        }
    }

    fn queue_missing_analysis(&mut self) {
        if let Ok(list) = self.lib.tracks_needing_analysis() {
            for (track, path) in list {
                self.queue.submit(AnalysisJob { track, path });
                self.analysis_total += 1;
            }
        }
    }

    /// Starts renaming a crate in place, starting from its current name.
    pub fn start_rename(&mut self, id: CrateId) {
        // The text field is in the crate's row: open the crates above it.
        self.reveal_crate(id);
        let name = self
            .crate_list()
            .into_iter()
            .find(|(c, _, _)| *c == id)
            .map(|(_, n, _)| n)
            .unwrap_or_default();
        self.renaming = Some(CrateRename {
            id,
            name,
            focus_pending: true,
        });
    }

    /// Saves the name being typed (an empty name keeps the old one).
    pub fn commit_rename(&mut self) {
        let Some(r) = self.renaming.take() else {
            return;
        };
        let name = r.name.trim();
        if name.is_empty() {
            return;
        }
        if let Err(e) = self.lib.rename_crate(r.id, name) {
            self.status = format!("{e}");
        }
        self.reload();
    }

    /// The library track for an audio file, added to the library (and queued
    /// for analysis) when it is not in it yet. `None` for a path that is not
    /// an existing audio file. Files loaded from the file manager go through
    /// here, so their cues, loops and grid edits are saved like any other
    /// track's.
    pub fn add_file(&mut self, path: &std::path::Path) -> Option<TrackId> {
        if let Ok(Some(t)) = self.lib.track_by_path(path) {
            return Some(t.id);
        }
        if !(path.is_file() && dj_library::is_audio_file(path)) {
            return None;
        }
        let meta = dj_library::read_meta(path);
        match self.lib.upsert_track(&meta) {
            Ok((id, _)) => {
                self.reload();
                if self.analyse_on_import {
                    self.queue_missing_analysis();
                }
                Some(id)
            }
            Err(e) => {
                self.status = format!("cannot add {}: {e}", path.display());
                None
            }
        }
    }

    /// Queues one track for (re-)analysis. An explicit re-analysis replaces
    /// a grid the user edited (not a locked one).
    pub fn reanalyse(&mut self, id: TrackId) {
        if let Err(e) = self.lib.forget_grid_edit(id) {
            tracing::warn!("cannot reset the grid edit of track {id}: {e}");
        }
        if let Some(t) = self.tracks.get(&id) {
            self.queue.submit(AnalysisJob {
                track: id,
                path: t.meta.path.clone(),
            });
            self.analysis_total += 1;
        }
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.get(&id)
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
    }

    /// (done, total) of the current analysis batch.
    pub fn analysis_progress(&self) -> (usize, usize) {
        let pending = self.queue.pending();
        (
            self.analysis_total.saturating_sub(pending),
            self.analysis_total,
        )
    }

    pub fn import_progress(&self) -> Option<(usize, usize)> {
        self.import.as_ref().map(|j| j.progress)
    }

    /// Starts importing a folder (recursively) in the background.
    pub fn start_import(&mut self, root: PathBuf) {
        self.start_import_into(root, None);
    }

    /// Imports a folder in the background; its tracks then go into
    /// `target` (in folder order). Waits in a queue while another import
    /// runs.
    pub fn start_import_into(&mut self, root: PathBuf, target: Option<CrateId>) {
        if self.import.is_some() {
            self.import_queue.push_back((root, target));
            return;
        }
        let known = self
            .tracks
            .values()
            .map(|t| (t.meta.path.clone(), (t.meta.file_size, t.meta.file_mtime)))
            .collect();
        match ImportJob::start(root.clone(), known, target) {
            Ok(j) => {
                self.status = format!("importing {}", root.display());
                self.import = Some(j);
            }
            Err(e) => self.status = format!("cannot start import: {e}"),
        }
    }

    /// Files and folders dropped on the library (from the Files panel or
    /// the file manager): files are added at once, folders (with their
    /// subfolders) in the background; then everything goes into `target`.
    /// Returns the crate the tracks go into (a new one for `NewCrate`).
    pub fn import_into(&mut self, paths: &[PathBuf], target: ImportTarget) -> Option<CrateId> {
        // A folder and rekordbox's playlists take no tracks by hand: the
        // files come into the library only.
        let target = match target {
            ImportTarget::Crate(c) if self.has_sub_crates(c) || self.is_synced(c) => {
                self.status = if self.is_synced(c) {
                    "rekordbox's playlists follow rekordbox: the files go into the library only"
                } else {
                    "A folder holds crates, not tracks: the files go into the library only"
                }
                .into();
                ImportTarget::Library
            }
            t => t,
        };
        if self.protect && target != ImportTarget::Library {
            self.status =
                "Library is protected: crates cannot be changed (Settings > Library)".into();
            return None;
        }
        let crate_id = match &target {
            ImportTarget::Library => None,
            ImportTarget::Crate(c) => Some(*c),
            ImportTarget::NewCrate(name) => {
                let names: Vec<String> = self.crate_list().into_iter().map(|(_, n, _)| n).collect();
                let name = (1..)
                    .map(|k| {
                        if k == 1 {
                            name.clone()
                        } else {
                            format!("{name} {k}")
                        }
                    })
                    .find(|n| !names.contains(n))
                    .unwrap_or_else(|| name.clone());
                match self.lib.create_crate(&name, None) {
                    Ok(id) => {
                        self.reload();
                        Some(id)
                    }
                    Err(e) => {
                        self.status = format!("{e}");
                        return None;
                    }
                }
            }
        };
        let mut ids = Vec::new();
        let mut skipped = 0;
        for p in paths {
            if p.is_dir() {
                self.start_import_into(p.clone(), crate_id);
            } else if let Some(id) = self.add_file(p) {
                ids.push(id);
            } else {
                skipped += 1;
            }
        }
        if let Some(c) = crate_id {
            self.crate_flash = Some((c, std::time::Instant::now()));
            if !ids.is_empty()
                && let Err(e) = self.lib.add_to_crate(c, &ids)
            {
                self.status = format!("{e}");
            }
        }
        if !ids.is_empty() {
            let into = crate_id
                .and_then(|c| {
                    self.crate_list()
                        .into_iter()
                        .find(|(id, _, _)| *id == c)
                        .map(|(_, n, _)| format!(" to {n}"))
                })
                .unwrap_or_else(|| " to the library".into());
            self.status = format!(
                "Added {} track{}{into}",
                ids.len(),
                if ids.len() == 1 { "" } else { "s" }
            );
        }
        if skipped > 0 && ids.is_empty() && self.import.is_none() {
            self.status = "Not an audio file Kaldor can play".into();
        }
        self.view_dirty = true;
        crate_id
    }

    /// Handles import batches and finished analyses. Returns ids of tracks
    /// whose analysis just finished (so decks can refresh).
    pub fn poll(&mut self) -> Vec<TrackId> {
        if let Some(db) = self.rekordbox.tick(&self.lib) {
            self.apply_rekordbox(db);
        }
        let mut changed = false;
        if let Some(job) = &mut self.import {
            let mut finished = false;
            while let Ok(msg) = job.rx.try_recv() {
                match msg {
                    ImportMsg::Progress(d, t) => job.progress = (d, t),
                    ImportMsg::Batch(metas) => match self.lib.upsert_tracks(&metas) {
                        Ok(ids) => {
                            job.added
                                .extend(ids.into_iter().filter(|(_, a)| *a).map(|(id, _)| id));
                            changed = true;
                        }
                        Err(e) => self.status = format!("import error: {e}"),
                    },
                    ImportMsg::Files(f) => job.files = f,
                    ImportMsg::Done => finished = true,
                }
            }
            if finished {
                let added = job.added.len();
                let target = job.target;
                let files = std::mem::take(&mut job.files);
                self.import = None;
                self.status = format!("import finished: {added} new tracks");
                changed = true;
                self.reload();
                // A folder dropped on a crate: its tracks go in, in folder
                // order (new and known ones).
                if let Some(c) = target {
                    let by_path: HashMap<&std::path::Path, TrackId> = self
                        .tracks
                        .values()
                        .map(|t| (t.meta.path.as_path(), t.id))
                        .collect();
                    let ids: Vec<TrackId> = files
                        .iter()
                        .filter_map(|p| by_path.get(p.as_path()).copied())
                        .collect();
                    match self.lib.add_to_crate(c, &ids) {
                        Ok(()) => {
                            let name = self
                                .crate_list()
                                .into_iter()
                                .find(|(id, _, _)| *id == c)
                                .map(|(_, n, _)| n)
                                .unwrap_or_default();
                            self.status = format!(
                                "Added {} tracks to {name} ({added} new in the library)",
                                ids.len()
                            );
                            self.crate_flash = Some((c, std::time::Instant::now()));
                        }
                        Err(e) => self.status = format!("{e}"),
                    }
                }
                if self.analyse_on_import {
                    self.queue_missing_analysis();
                }
                // The next folder waiting.
                if let Some((root, target)) = self.import_queue.pop_front() {
                    self.start_import_into(root, target);
                }
            }
        }
        let mut done = Vec::new();
        while let Some(d) = self.queue.try_recv() {
            match d.result {
                Ok(a) => {
                    if let Err(e) = self.lib.store_analysis(d.track, &a) {
                        self.status = format!("cannot store analysis: {e}");
                    }
                }
                Err(e) => {
                    tracing::warn!("analysis failed for track {}: {e}", d.track);
                }
            }
            done.push(d.track);
        }
        if !done.is_empty() {
            for id in &done {
                // New analysis: a new small waveform.
                self.minis.forget(*id);
                if let Ok(Some(t)) = self.lib.track(*id) {
                    self.tracks.insert(*id, t);
                }
            }
            self.view_dirty = true;
        }
        if self.queue.pending() == 0 {
            self.analysis_total = 0;
        }
        if changed {
            self.reload();
        }
        self.check_files();
        if let Some(msg) = self.poll_moved_search() {
            tracing::info!("{msg}");
        }
        done
    }

    fn rebuild_view(&mut self) {
        let base: Vec<TrackId> = match self.selection {
            Selection::All => {
                let mut v: Vec<TrackId> = self.tracks.keys().copied().collect();
                v.sort_unstable();
                v
            }
            Selection::Crate(c) => self.lib.shown_tracks(c).unwrap_or_default(),
        };
        let hits: Option<std::collections::HashSet<TrackId>> = if self.search.trim().is_empty() {
            None
        } else {
            self.lib
                .search(&self.search)
                .ok()
                .map(|v| v.into_iter().collect())
        };
        let base: Vec<TrackId> = base
            .into_iter()
            .filter(|id| self.tracks.contains_key(id))
            .collect();
        // The # of every track: its place in the crate's own order (in All
        // tracks: the import order). It stays with the track whatever the
        // list is sorted or searched by, as in Serato.
        self.order_no = base
            .iter()
            .enumerate()
            .map(|(i, id)| (*id, i + 1))
            .collect();
        let mut view: Vec<TrackId> = base
            .into_iter()
            .filter(|id| hits.as_ref().is_none_or(|h| h.contains(id)))
            .collect();
        let (key, asc) = self.sort;
        if key == SortKey::Order && !asc {
            view.reverse();
        }
        if key != SortKey::Order {
            let t = &self.tracks;
            let files = &self.files;
            let file_name = |x: &Track| {
                x.meta
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            };
            let folder = |x: &Track| {
                x.meta
                    .path
                    .parent()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            };
            let cmp_str = |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase());
            view.sort_by(|a, b| {
                let (Some(x), Some(y)) = (t.get(a), t.get(b)) else {
                    return std::cmp::Ordering::Equal;
                };
                let o = match key {
                    SortKey::Artist => cmp_str(&x.meta.artist, &y.meta.artist),
                    SortKey::Title => cmp_str(&x.display_name(), &y.display_name()),
                    SortKey::Bpm => x.bpm.unwrap_or(0.0).total_cmp(&y.bpm.unwrap_or(0.0)),
                    SortKey::Key => x
                        .key
                        .map(|k| k.camelot_sort_key())
                        .cmp(&y.key.map(|k| k.camelot_sort_key())),
                    SortKey::Time => x.meta.duration.total_cmp(&y.meta.duration),
                    SortKey::Album => cmp_str(&x.meta.album, &y.meta.album),
                    SortKey::Genre => cmp_str(&x.meta.genre, &y.meta.genre),
                    SortKey::Added => x.date_added.cmp(&y.date_added).then(x.id.cmp(&y.id)),
                    SortKey::Order => std::cmp::Ordering::Equal,
                    SortKey::Found => files.get(x.id).cmp(&files.get(y.id)),
                    SortKey::Year => x.meta.year.cmp(&y.meta.year),
                    SortKey::Comment => cmp_str(&x.meta.comment, &y.meta.comment),
                    SortKey::Bitrate => x.meta.bitrate.cmp(&y.meta.bitrate),
                    SortKey::Plays => x.play_count.cmp(&y.play_count),
                    SortKey::FileName => file_name(x).cmp(&file_name(y)),
                    SortKey::Location => folder(x)
                        .cmp(&folder(y))
                        .then_with(|| file_name(x).cmp(&file_name(y))),
                };
                if asc { o } else { o.reverse() }
            });
        }
        self.view = view;
    }

    /// The visible track ids, rebuilt when the search, selection, sort or
    /// data changed.
    pub fn view(&mut self) -> &[TrackId] {
        if self.search != self.last_search {
            self.last_search = self.search.clone();
            self.view_dirty = true;
        }
        if self.view_dirty {
            self.rebuild_view();
            self.view_dirty = false;
        }
        &self.view
    }

    /// Moves the selected track `delta` rows in the list (browse knob) and
    /// scrolls the list so it stays visible.
    pub fn move_selection(&mut self, delta: i32) {
        let row_h = self.row_pitch.max(1.0);
        let view = self.view().to_vec();
        if view.is_empty() {
            return;
        }
        let cur = self
            .selected_track
            .and_then(|id| view.iter().position(|x| *x == id));
        let next = match cur {
            Some(i) => (i as i64 + i64::from(delta)).clamp(0, view.len() as i64 - 1) as usize,
            None if delta < 0 => view.len() - 1,
            None => 0,
        };
        self.selected_track = Some(view[next]);
        self.clear_picked();
        let (offset, height) = self.list_scroll;
        let top = next as f32 * row_h;
        // The last row comes with the room under it.
        let bottom = top
            + row_h
            + if next + 1 == view.len() {
                LIST_END_PAD
            } else {
                0.0
            };
        if top < offset {
            self.scroll_to = Some(top);
        } else if height > row_h && bottom > offset + height {
            self.scroll_to = Some(bottom - height);
        }
    }

    /// The browse knob was pushed: switch between the crate list and the
    /// track list. Into the tracks, the first track is selected (so it is
    /// clear where the knob is); into the crates, the track selection goes.
    pub fn toggle_browse_focus(&mut self) {
        self.knob_used = true;
        self.browse_crates = !self.browse_crates;
        if self.browse_crates {
            self.selected_track = None;
        } else {
            self.selected_track = self.view().first().copied();
            self.scroll_to = Some(0.0);
        }
    }

    /// Moves the crate selection `delta` steps through "All tracks" and the
    /// crates in tree order (browse knob in crate mode), skipping the
    /// sub-crates of closed crates.
    pub fn move_crate(&mut self, delta: i32) {
        // The rekordbox tab: its collection, then its playlists.
        let first = match self.crate_tab {
            CrateTab::Crates => Selection::All,
            CrateTab::Rekordbox => match self.rb_root {
                Some(r) => Selection::Crate(r),
                None => return,
            },
        };
        // The rows on screen: a closed crate's (or folder's) sub-crates are
        // reached by pushing the knob on it (`toggle_selected_parent`).
        let mut order = vec![first];
        order.extend(self.visible_crates().into_iter().map(Selection::Crate));
        let cur = order.iter().position(|s| *s == self.selection).unwrap_or(0);
        let next = (cur as i64 + i64::from(delta)).clamp(0, order.len() as i64 - 1) as usize;
        if order[next] != self.selection {
            self.selection = order[next];
            self.view_dirty = true;
            self.selected_track = None;
            self.clear_picked();
            self.clear_picked_crates();
            self.scroll_to = Some(0.0);
            // The crate list follows the knob (opening the folders above).
            if let Selection::Crate(c) = self.selection {
                self.reveal_crate(c);
            }
            self.crate_scroll = true;
        }
    }

    pub fn mark_dirty(&mut self) {
        self.view_dirty = true;
    }

    /// The ids of rekordbox's playlists and folders (not its root).
    pub fn rb_crate_ids(&self) -> Vec<CrateId> {
        fn walk(nodes: &[CrateNode], out: &mut Vec<CrateId>) {
            for n in nodes {
                out.push(n.id);
                walk(&n.children, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.rb_crates, &mut out);
        out
    }

    /// Flat list of crates with their depth, for menus.
    pub fn crate_list(&self) -> Vec<(CrateId, String, usize)> {
        fn walk(nodes: &[CrateNode], depth: usize, out: &mut Vec<(CrateId, String, usize)>) {
            for n in nodes {
                out.push((n.id, n.name.clone(), depth));
                walk(&n.children, depth + 1, out);
            }
        }
        let mut out = Vec::new();
        walk(&self.crates, 0, &mut out);
        out
    }

    /// The crates shown in the crate list, in tree order: the sub-crates
    /// of closed crates are left out.
    pub fn visible_crates(&self) -> Vec<CrateId> {
        fn walk(
            nodes: &[CrateNode],
            closed: &std::collections::HashSet<CrateId>,
            out: &mut Vec<CrateId>,
        ) {
            for n in nodes {
                out.push(n.id);
                if !closed.contains(&n.id) {
                    walk(&n.children, closed, out);
                }
            }
        }
        let mut out = Vec::new();
        let nodes = match self.crate_tab {
            CrateTab::Crates => &self.crates,
            CrateTab::Rekordbox => &self.rb_crates,
        };
        walk(nodes, &self.collapsed, &mut out);
        out
    }

    /// The crates above `id` in the tree, the top one first (empty for a
    /// top-level or unknown crate).
    fn crate_parents(&self, id: CrateId) -> Vec<CrateId> {
        fn find(nodes: &[CrateNode], id: CrateId, path: &mut Vec<CrateId>) -> bool {
            for n in nodes {
                if n.id == id {
                    return true;
                }
                path.push(n.id);
                if find(&n.children, id, path) {
                    return true;
                }
                path.pop();
            }
            false
        }
        let mut path = Vec::new();
        if !find(&self.crates, id, &mut path) {
            path.clear();
            find(&self.rb_crates, id, &mut path);
        }
        path
    }

    /// Opens or closes a crate's sub-crates in the list. Closing it while
    /// one of its sub-crates is open shows this crate instead (the open one
    /// would be hidden).
    pub fn toggle_collapsed(&mut self, id: CrateId) {
        if self.collapsed.remove(&id) {
            self.opened.insert(id);
            return;
        }
        self.collapsed.insert(id);
        self.opened.remove(&id);
        if let Selection::Crate(open) = self.selection
            && self.crate_parents(open).contains(&id)
        {
            self.picked_crates.clear();
            self.crate_anchor = Some(id);
            self.open_crate(Selection::Crate(id));
        }
        // Hidden crates leave a Ctrl / Shift selection.
        let shown = self.visible_crates();
        self.picked_crates.retain(|c| shown.contains(c));
    }

    /// Opens every crate above `id`, so its row is shown.
    pub fn reveal_crate(&mut self, id: CrateId) {
        for p in self.crate_parents(id) {
            self.collapsed.remove(&p);
            self.opened.insert(p);
        }
    }

    /// Closes every crate with sub-crates that was not opened (`opened`):
    /// crates with sub-crates and rekordbox folders start closed.
    pub fn refold(&mut self) {
        fn parents(nodes: &[CrateNode], out: &mut std::collections::HashSet<CrateId>) {
            for n in nodes {
                if !n.children.is_empty() {
                    out.insert(n.id);
                    parents(&n.children, out);
                }
            }
        }
        let mut closed = std::collections::HashSet::new();
        parents(&self.crates, &mut closed);
        parents(&self.rb_crates, &mut closed);
        closed.retain(|c| !self.opened.contains(c));
        self.collapsed = closed;
    }

    /// Whether a crate (or rekordbox folder) has sub-crates.
    pub fn has_sub_crates(&self, id: CrateId) -> bool {
        fn find(nodes: &[CrateNode], id: CrateId) -> Option<bool> {
            nodes.iter().find_map(|n| {
                (n.id == id)
                    .then_some(!n.children.is_empty())
                    .or_else(|| find(&n.children, id))
            })
        }
        find(&self.crates, id)
            .or_else(|| find(&self.rb_crates, id))
            .unwrap_or(false)
    }

    /// The browse knob was pushed on a crate with sub-crates (in the crate
    /// list): a closed one opens (the knob then turns through its
    /// sub-crates), an open one closes. False when the push does something
    /// else (see [`Self::toggle_browse_focus`]).
    pub fn toggle_selected_parent(&mut self) -> bool {
        let Selection::Crate(c) = self.selection else {
            return false;
        };
        if !self.browse_crates || !self.has_sub_crates(c) {
            return false;
        }
        self.toggle_collapsed(c);
        self.crate_scroll = true;
        true
    }
}

/// Camelot order helper for sorting keys (1A, 1B, 2A, ...).
trait CamelotSort {
    fn camelot_sort_key(self) -> (u8, bool);
}

impl CamelotSort for dj_core::MusicalKey {
    fn camelot_sort_key(self) -> (u8, bool) {
        let c = self.camelot();
        let n: u8 = c.trim_end_matches(['A', 'B']).parse().unwrap_or(0);
        (n, !self.minor)
    }
}
