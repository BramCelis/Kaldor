//! The rekordbox tab's sync: watches rekordbox's library on this computer
//! (`master.db` and its `-wal`) and, a moment after it changes, reads it on
//! a background thread; the UI thread then merges it into the library
//! ([`dj_library::Library::sync_foreign`]).
//!
//! Checks the files' size and time every [`CHECK`] (a few `stat` calls).
//! rekordbox writes in bursts, so a change is read once the files have
//! stayed the same for one check; the first read and "Sync now" go at
//! once. A read that fails (rekordbox writing a page right then) is tried
//! again after [`RETRY`].

use dj_library::Library;
use dj_library::interop::rekordbox_db::{self, RekordboxDb};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The source name kept in the library for rekordbox's crates and ids.
pub const SOURCE: &str = "rekordbox";
const CHECK: Duration = Duration::from_secs(2);
const RETRY: Duration = Duration::from_secs(10);

type Stamp = Vec<(u64, Option<std::time::SystemTime>)>;

/// Where the sync is.
#[derive(Clone, Debug, PartialEq)]
pub enum SyncState {
    /// Turned off (Settings > Library).
    Off,
    /// No rekordbox library in the folder.
    NotFound(PathBuf),
    /// Not read yet.
    Waiting,
    /// Reading rekordbox's library.
    Reading,
    /// Up to date.
    Synced,
    /// The last read failed (tried again).
    Failed(String),
}

/// What the last sync brought (for the tab's header).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LastSync {
    pub at: Option<Instant>,
    pub tracks: usize,
    pub playlists: usize,
    pub smart_left_out: usize,
    pub notes: Vec<String>,
}

pub struct RekordboxSync {
    /// Sync at all (Settings > Library).
    pub enabled: bool,
    /// rekordbox's folder; `None` is where rekordbox keeps it by default
    /// (only for a library in a file: tests never look there).
    pub folder: Option<PathBuf>,
    /// Use rekordbox's default folder when `folder` is `None`.
    pub use_default: bool,
    pub state: SyncState,
    pub last: LastSync,
    applied: Option<Stamp>,
    seen: Option<Stamp>,
    next_check: Option<Instant>,
    force: bool,
    job: Option<mpsc::Receiver<(Result<RekordboxDb, String>, Stamp)>>,
}

impl Default for RekordboxSync {
    fn default() -> Self {
        Self {
            enabled: false,
            folder: None,
            use_default: false,
            state: SyncState::Off,
            last: LastSync::default(),
            applied: None,
            seen: None,
            next_check: None,
            force: false,
            job: None,
        }
    }
}

impl RekordboxSync {
    /// The rekordbox folder watched.
    pub fn dir(&self) -> Option<PathBuf> {
        self.folder
            .clone()
            .or_else(|| self.use_default.then(rekordbox_db::default_dir).flatten())
    }

    /// Reads rekordbox's library now (also when it did not change).
    pub fn sync_now(&mut self) {
        self.force = true;
        self.next_check = None;
    }

    /// Settings changed: start over.
    pub fn configure(&mut self, enabled: bool, folder: Option<PathBuf>) {
        if self.enabled != enabled || self.folder != folder {
            self.enabled = enabled;
            self.folder = folder;
            self.applied = None;
            self.seen = None;
            self.sync_now();
        }
    }

    /// Shows the tab as up to date for an hour without reading anything
    /// (screenshots: no rekordbox there).
    #[doc(hidden)]
    pub fn show_as_synced(&mut self) {
        self.enabled = true;
        self.state = SyncState::Synced;
        self.last.at = Some(Instant::now());
        self.next_check = Some(Instant::now() + Duration::from_secs(3600));
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }

    /// Once a frame: returns rekordbox's library when a read finished.
    pub fn tick(&mut self, lib: &Library) -> Option<RekordboxDb> {
        if !self.enabled {
            self.state = SyncState::Off;
            self.job = None;
            return None;
        }
        if let Some(rx) = &self.job {
            match rx.try_recv() {
                Ok((Ok(db), stamp)) => {
                    self.job = None;
                    self.applied = Some(stamp);
                    self.seen = None;
                    self.state = SyncState::Synced;
                    return Some(db);
                }
                Ok((Err(e), _)) => {
                    self.job = None;
                    self.state = SyncState::Failed(e);
                    self.next_check = Some(Instant::now() + RETRY);
                }
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.job = None;
                    self.state = SyncState::Failed("the read stopped".into());
                    self.next_check = Some(Instant::now() + RETRY);
                }
            }
            return None;
        }
        let now = Instant::now();
        if self.next_check.is_some_and(|t| now < t) {
            return None;
        }
        self.next_check = Some(now + CHECK);
        let Some(dir) = self.dir() else {
            self.state = SyncState::NotFound(PathBuf::new());
            return None;
        };
        let Some(stamp) = rekordbox_db::stamp(&dir) else {
            self.state = SyncState::NotFound(dir);
            self.applied = None;
            return None;
        };
        if matches!(self.state, SyncState::Off | SyncState::NotFound(_)) {
            self.state = SyncState::Waiting;
        }
        let changed = self.applied.as_ref() != Some(&stamp);
        if !changed && !self.force {
            return None;
        }
        // Read once the files stopped changing (at once the first time and
        // on Sync now).
        let settled = self.seen.as_ref() == Some(&stamp);
        if !(self.force || self.applied.is_none() || settled) {
            self.seen = Some(stamp);
            return None;
        }
        self.force = false;
        let known = lib.synced_fingerprints(SOURCE).unwrap_or_default();
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("rekordbox-sync".into())
            .spawn(move || {
                let _ = tx.send((rekordbox_db::read_dir(&dir, &known), stamp));
            });
        match spawned {
            Ok(_) => {
                self.job = Some(rx);
                self.state = SyncState::Reading;
            }
            Err(e) => self.state = SyncState::Failed(e.to_string()),
        }
        None
    }
}
