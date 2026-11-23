//! Is a library track's file still there? A background thread checks the
//! files (a removed USB stick, a moved or renamed file) so the track list
//! can show it before a load is tried. The rows on screen are checked every
//! [`VISIBLE_EVERY`], the whole library every [`FULL_EVERY`]. Nothing is
//! written to the database: a stick that comes back makes the tracks
//! available again without any change to the library.

use dj_library::TrackId;
use std::collections::{HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often the rows on screen are checked again.
pub const VISIBLE_EVERY: Duration = Duration::from_millis(1500);
/// How often the whole library is checked again.
pub const FULL_EVERY: Duration = Duration::from_secs(30);
/// Files checked per step of a full pass (the rows on screen go first).
const STEP: usize = 256;

/// Whether a track's file can be found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum FileState {
    /// Not checked yet.
    #[default]
    Unknown,
    Found,
    /// The drive, stick or volume the file is on is not there.
    DriveMissing,
    /// The drive is there, the file is not (moved, renamed or deleted).
    Missing,
}

impl FileState {
    pub fn is_missing(self) -> bool {
        matches!(self, Self::Missing | Self::DriveMissing)
    }

    /// Tooltip text for the icon in the track list.
    pub fn describe(self, path: &Path) -> String {
        match self {
            Self::Unknown => "Not checked yet".into(),
            Self::Found => format!("File found\n{}", path.display()),
            Self::DriveMissing => format!(
                "Drive not connected: {}\n{}",
                drive_root(path)
                    .map(|r| r.display().to_string())
                    .unwrap_or_default(),
                path.display()
            ),
            Self::Missing => format!(
                "File not found (moved, renamed or deleted)\n{}",
                path.display()
            ),
        }
    }
}

enum Request {
    /// Check these first.
    Visible(Vec<(TrackId, PathBuf)>),
    /// Check all of these (replaces an unfinished full pass).
    Full(Vec<(TrackId, PathBuf)>),
}

/// The checker owned by the library UI: sends paths to the thread and keeps
/// the answers.
pub struct FileChecker {
    tx: Option<mpsc::Sender<Request>>,
    rx: Option<mpsc::Receiver<Vec<(TrackId, FileState)>>>,
    state: HashMap<TrackId, FileState>,
    last_visible: Option<Instant>,
    last_full: Option<Instant>,
    /// The track list changed: start a new full pass now.
    full_due: bool,
}

impl Default for FileChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl FileChecker {
    /// Starts the checker thread (no thread if it cannot be started; the
    /// states then stay unknown).
    pub fn new() -> Self {
        let (tx, req_rx) = mpsc::channel();
        let (res_tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("kaldor-file-check".into())
            .spawn(move || worker(req_rx, res_tx));
        match spawned {
            Ok(_) => Self {
                tx: Some(tx),
                rx: Some(rx),
                ..Self::disabled()
            },
            Err(e) => {
                tracing::warn!("cannot start the file checker: {e}");
                Self::disabled()
            }
        }
    }

    /// A checker that never touches the disk (the demo, whose tracks have
    /// no files); states only come from [`Self::set`].
    pub fn disabled() -> Self {
        Self {
            tx: None,
            rx: None,
            state: HashMap::new(),
            last_visible: None,
            last_full: None,
            full_due: true,
        }
    }

    pub fn get(&self, id: TrackId) -> FileState {
        self.state.get(&id).copied().unwrap_or_default()
    }

    /// Sets a state by hand (demo and tests).
    pub fn set(&mut self, id: TrackId, s: FileState) {
        self.state.insert(id, s);
    }

    /// The library's tracks changed: check them all again soon.
    pub fn tracks_changed(&mut self) {
        self.full_due = true;
    }

    /// Takes the answers in and sends new requests when they are due.
    /// `visible` are the rows on screen; `all` gives every track's path
    /// (only called for a full pass). Returns true when a state changed.
    pub fn tick(
        &mut self,
        now: Instant,
        visible: impl FnOnce() -> Vec<(TrackId, PathBuf)>,
        all: impl FnOnce() -> Vec<(TrackId, PathBuf)>,
    ) -> bool {
        let mut changed = false;
        if let Some(rx) = &self.rx {
            for batch in rx.try_iter() {
                for (id, s) in batch {
                    changed |= self.state.insert(id, s) != Some(s);
                }
            }
        }
        let Some(tx) = &self.tx else {
            return changed;
        };
        let due = |last: Option<Instant>, every: Duration| {
            last.is_none_or(|t| now.saturating_duration_since(t) >= every)
        };
        if self.full_due || due(self.last_full, FULL_EVERY) {
            self.full_due = false;
            self.last_full = Some(now);
            let _ = tx.send(Request::Full(all()));
        }
        if due(self.last_visible, VISIBLE_EVERY) {
            self.last_visible = Some(now);
            let v = visible();
            if !v.is_empty() {
                let _ = tx.send(Request::Visible(v));
            }
        }
        changed
    }
}

fn worker(rx: mpsc::Receiver<Request>, tx: mpsc::Sender<Vec<(TrackId, FileState)>>) {
    let mut full: VecDeque<(TrackId, PathBuf)> = VecDeque::new();
    loop {
        // Idle: wait for work. Busy with a full pass: take new requests
        // between steps.
        let first = if full.is_empty() {
            match rx.recv() {
                Ok(r) => Some(r),
                Err(_) => return,
            }
        } else {
            None
        };
        let mut visible = Vec::new();
        for r in first.into_iter().chain(rx.try_iter()) {
            match r {
                Request::Visible(v) => visible.extend(v),
                Request::Full(v) => full = v.into(),
            }
        }
        // A drive's state is looked up once per step: a missing stick costs
        // one check, not one per track on it.
        let mut drives = HashMap::new();
        if !visible.is_empty() {
            let out = visible
                .into_iter()
                .map(|(id, p)| (id, check(&p, &mut drives)))
                .collect();
            if tx.send(out).is_err() {
                return;
            }
        }
        let n = full.len().min(STEP);
        if n > 0 {
            let out = full
                .drain(..n)
                .map(|(id, p)| (id, check(&p, &mut drives)))
                .collect();
            if tx.send(out).is_err() {
                return;
            }
        }
    }
}

/// The state of one file. `drives` caches whether each drive is there.
pub fn check(path: &Path, drives: &mut HashMap<PathBuf, bool>) -> FileState {
    if let Some(root) = drive_root(path) {
        let there = *drives.entry(root.clone()).or_insert_with(|| root.is_dir());
        if !there {
            return FileState::DriveMissing;
        }
    }
    if path.is_file() {
        FileState::Found
    } else {
        FileState::Missing
    }
}

/// The drive or removable volume a path is on: `E:\` or `\\server\share`
/// on Windows, `/Volumes/<name>` on macOS, `/media/<user>/<name>`,
/// `/run/media/<user>/<name>` and `/mnt/<name>` on Linux. `None` for a
/// path on the system disk (or a relative path).
pub fn drive_root(path: &Path) -> Option<PathBuf> {
    let comps: Vec<Component> = path.components().collect();
    if let Some(Component::Prefix(_)) = comps.first() {
        let mut root = PathBuf::new();
        root.push(comps[0].as_os_str());
        if let Some(Component::RootDir) = comps.get(1) {
            root.push(Component::RootDir.as_os_str());
        }
        return Some(root);
    }
    if comps.first() != Some(&Component::RootDir) {
        return None;
    }
    let names: Vec<&std::ffi::OsStr> = comps[1..].iter().map(|c| c.as_os_str()).collect::<Vec<_>>();
    let take = match names.as_slice() {
        [a, _, _, ..] if *a == "Volumes" => 2,
        [a, _, _, ..] if *a == "mnt" => 2,
        [a, _, _, _, ..] if *a == "media" => 3,
        [a, b, _, _, _, ..] if *a == "run" && *b == "media" => 4,
        _ => return None,
    };
    let mut root = PathBuf::from(Component::RootDir.as_os_str());
    for n in &names[..take] {
        root.push(n);
    }
    Some(root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_roots() {
        let r = |s: &str| drive_root(Path::new(s));
        if cfg!(windows) {
            assert_eq!(r(r"E:\Music\a.mp3"), Some(PathBuf::from(r"E:\")));
            assert_eq!(
                r(r"\\nas\music\a.mp3"),
                Some(PathBuf::from(r"\\nas\music\"))
            );
        }
        assert_eq!(
            r("/Volumes/STICK/Music/a.mp3"),
            Some(PathBuf::from("/Volumes/STICK"))
        );
        assert_eq!(
            r("/media/dj/STICK/a.mp3"),
            Some(PathBuf::from("/media/dj/STICK"))
        );
        assert_eq!(
            r("/run/media/dj/STICK/a.mp3"),
            Some(PathBuf::from("/run/media/dj/STICK"))
        );
        assert_eq!(r("/mnt/usb/a.mp3"), Some(PathBuf::from("/mnt/usb")));
        assert_eq!(r("/home/dj/Music/a.mp3"), None);
        assert_eq!(r("music/a.mp3"), None);
    }

    #[test]
    fn found_missing_and_drive_missing() {
        let dir = crate::test_dir("kaldor-file-check");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.wav");
        std::fs::write(&file, b"x").unwrap();
        let mut drives = HashMap::new();
        assert_eq!(check(&file, &mut drives), FileState::Found);
        assert_eq!(
            check(&dir.join("moved.wav"), &mut drives),
            FileState::Missing
        );
        // A drive that is not there (known from the cache, so the test
        // looks at nothing outside the project folder).
        let gone = if cfg!(windows) {
            PathBuf::from(r"Q:\Music\a.wav")
        } else {
            PathBuf::from("/Volumes/kaldor-no-such-stick/a.wav")
        };
        drives.insert(drive_root(&gone).unwrap(), false);
        assert_eq!(check(&gone, &mut drives), FileState::DriveMissing);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_thread_reports_files_on_screen_and_the_full_pass() {
        let dir = crate::test_dir("kaldor-file-thread");
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.wav");
        std::fs::write(&a, b"x").unwrap();
        let b = dir.join("b.wav");
        let mut c = FileChecker::new();
        let t0 = Instant::now();
        let all = || vec![(1, a.clone()), (2, b.clone())];
        c.tick(t0, || vec![(1, a.clone())], all);
        let deadline = Instant::now() + Duration::from_secs(10);
        while c.get(2) == FileState::Unknown {
            assert!(Instant::now() < deadline, "no answer from the checker");
            std::thread::sleep(Duration::from_millis(5));
            c.tick(t0, Vec::new, Vec::new);
        }
        assert_eq!(c.get(1), FileState::Found);
        assert_eq!(c.get(2), FileState::Missing);
        // The file appears: the next visible check (1.5 s later) sees it.
        std::fs::write(&b, b"x").unwrap();
        let t1 = t0 + VISIBLE_EVERY;
        c.tick(t1, || vec![(2, b.clone())], Vec::new);
        while c.get(2) != FileState::Found {
            assert!(Instant::now() < deadline, "the new file was not seen");
            std::thread::sleep(Duration::from_millis(5));
            c.tick(t1, Vec::new, Vec::new);
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
