//! "Import from…": other libraries and playlist files read
//! into one [`ForeignLibrary`], then merged into the Kaldor library by
//! [`Library::import_foreign`].
//!
//! Sources: rekordbox XML ([`rekordbox`]), iTunes / Apple Music XML
//! ([`itunes`]), M3U / M3U8 and PLS playlists ([`playlist`], which also
//! writes them).
//!
//! Merging never throws away work done in Kaldor:
//! - a track already in the library (same file path) keeps its tags; its
//!   rating and colour are only filled in when it has none, its play count
//!   goes up to the imported one, its date added goes back to the earlier;
//! - cues and loops are taken when the track has none of its own (or with
//!   [`InteropOptions::replace_cues`]);
//! - a beatgrid is taken when the track has none or only an analysed one
//!   (never over a grid edited or locked in Kaldor, unless
//!   [`InteropOptions::replace_grids`]); an imported grid counts as edited,
//!   so re-analysis keeps it;
//! - an imported key is kept as the track's tag key: shown until Kaldor's
//!   own analysis has run;
//! - playlists and folders become crates under one root crate (named after
//!   the source); importing again reuses crates with the same name and
//!   adds only tracks that are not in them yet.
//!
//! Files that do not exist are still added (marked missing), so their cues
//! and crates survive until the files are found again (relocate).

pub mod itunes;
pub mod playlist;
mod plist;
pub mod rekordbox;
pub mod rekordbox_db;
pub mod sync;
mod xml;

use crate::db::{Library, Result};
use crate::import::read_meta;
use crate::model::{CrateId, Cue, CueKind, SavedLoop, TrackId, TrackMeta};
use dj_core::{Beatgrid, MusicalKey};
use std::path::PathBuf;

/// A track as another program knows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ForeignTrack {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub comment: String,
    pub year: Option<i32>,
    /// Seconds (0 = unknown).
    pub duration: f64,
    pub bpm: Option<f64>,
    pub key: Option<MusicalKey>,
    /// 0 to 5 stars.
    pub rating: u8,
    /// 0xRRGGBB.
    pub color: Option<u32>,
    pub play_count: u32,
    /// Seconds since the Unix epoch.
    pub date_added: Option<i64>,
    pub grid: Option<Beatgrid>,
    pub cues: Vec<Cue>,
    pub loops: Vec<SavedLoop>,
}

/// A playlist or a folder of them.
#[derive(Clone, Debug, PartialEq)]
pub enum ForeignNode {
    Folder {
        name: String,
        children: Vec<ForeignNode>,
    },
    /// Tracks as indices into [`ForeignLibrary::tracks`], in order.
    Playlist { name: String, tracks: Vec<usize> },
}

/// Everything read from another library or playlist file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ForeignLibrary {
    /// Shown to the user and the default root crate name ("rekordbox").
    pub source: String,
    pub tracks: Vec<ForeignTrack>,
    pub playlists: Vec<ForeignNode>,
    /// Things that could not be read or were left out.
    pub warnings: Vec<String>,
}

/// How to merge.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InteropOptions {
    /// The crate the playlists go under (created at the top level, or the
    /// existing one of that name). `None`: playlists at the top level.
    pub root_crate: Option<String>,
    /// Take the imported cues and loops even when the track has its own.
    pub replace_cues: bool,
    /// Take the imported grid even over one edited or locked in Kaldor.
    pub replace_grids: bool,
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InteropReport {
    pub tracks_added: usize,
    /// Tracks that were in the library already (matched by path).
    pub tracks_matched: usize,
    /// Tracks whose file is not there (added and marked missing).
    pub missing_files: usize,
    pub cues: usize,
    pub loops: usize,
    pub grids: usize,
    /// Crates made (not counting reused ones).
    pub crates: usize,
    /// Tracks put into crates.
    pub crate_entries: usize,
    /// The tracks added whose file is there (to analyse).
    pub to_analyse: Vec<TrackId>,
}

impl ForeignLibrary {
    /// Reads the file metadata of new tracks from disk (slow: one tag read
    /// per file). Call this on a background thread before
    /// [`Library::import_foreign`]; tracks already prepared are skipped.
    pub fn prepare(&self) -> Vec<Option<TrackMeta>> {
        self.tracks
            .iter()
            .map(|t| t.path.exists().then(|| read_meta(&t.path)))
            .collect()
    }
}

/// The library record for a foreign track: the file's own tags when it
/// was read (`file`), the foreign fields filling whatever it lacks.
fn merged_meta(t: &ForeignTrack, file: Option<&TrackMeta>) -> TrackMeta {
    let mut m = file.cloned().unwrap_or_else(|| TrackMeta {
        path: t.path.clone(),
        ..TrackMeta::default()
    });
    let fill = |dst: &mut String, src: &str| {
        if dst.trim().is_empty() && !src.trim().is_empty() {
            *dst = src.trim().to_string();
        }
    };
    fill(&mut m.title, &t.title);
    fill(&mut m.artist, &t.artist);
    fill(&mut m.album, &t.album);
    fill(&mut m.genre, &t.genre);
    fill(&mut m.comment, &t.comment);
    if m.year.is_none() {
        m.year = t.year;
    }
    if m.duration <= 0.0 && t.duration > 0.0 {
        m.duration = t.duration;
    }
    if m.tag_bpm.is_none() {
        m.tag_bpm = t.bpm.filter(|b| *b > 0.0);
    }
    if m.tag_key.is_none() {
        m.tag_key = t.key;
    }
    m
}

impl Library {
    /// Merges another library into this one (see the module docs). `files`
    /// is [`ForeignLibrary::prepare`]'s result (or empty to read the files
    /// here).
    pub fn import_foreign(
        &mut self,
        f: &ForeignLibrary,
        files: &[Option<TrackMeta>],
        opts: &InteropOptions,
    ) -> Result<InteropReport> {
        let mut report = InteropReport::default();
        let mut ids: Vec<TrackId> = Vec::with_capacity(f.tracks.len());
        for (i, t) in f.tracks.iter().enumerate() {
            let id = self.merge_track(t, files.get(i), opts, &mut report)?;
            ids.push(id);
        }
        // Playlists into crates.
        let root = match &opts.root_crate {
            Some(name) => Some(self.find_or_create_crate(name, None, &mut report)?),
            None => None,
        };
        self.import_nodes(&f.playlists, root, &ids, &mut report)?;
        Ok(report)
    }

    /// Merges one foreign track (see the module docs) and returns its id.
    /// `file`: [`ForeignLibrary::prepare`]'s entry for it (`Some(None)`: the
    /// file is not there), or `None` to read the tags here.
    pub(crate) fn merge_track(
        &mut self,
        t: &ForeignTrack,
        file: Option<&Option<TrackMeta>>,
        opts: &InteropOptions,
        report: &mut InteropReport,
    ) -> Result<TrackId> {
        let id = match self.track_by_path(&t.path)? {
            Some(existing) => {
                report.tracks_matched += 1;
                existing.id
            }
            None => {
                let read;
                let file = match file {
                    Some(m) => m.as_ref(),
                    None => {
                        read = t.path.exists().then(|| read_meta(&t.path));
                        read.as_ref()
                    }
                };
                self.add_foreign(t, file, file.is_some(), report)?
            }
        };
        self.merge_extras(id, t, opts, report)?;
        Ok(id)
    }

    /// Like [`Self::merge_track`] for a source whose own tags are complete
    /// enough (rekordbox's database): a new track takes them without a tag
    /// read, which keeps a sync of a large library fast.
    pub(crate) fn merge_track_untagged(
        &mut self,
        t: &ForeignTrack,
        opts: &InteropOptions,
        report: &mut InteropReport,
    ) -> Result<TrackId> {
        let id = match self.track_by_path(&t.path)? {
            Some(existing) => {
                report.tracks_matched += 1;
                existing.id
            }
            None => self.add_foreign(t, None, t.path.exists(), report)?,
        };
        self.merge_extras(id, t, opts, report)?;
        Ok(id)
    }

    fn add_foreign(
        &mut self,
        t: &ForeignTrack,
        file: Option<&TrackMeta>,
        exists: bool,
        report: &mut InteropReport,
    ) -> Result<TrackId> {
        let meta = merged_meta(t, file);
        let (id, _) = self.upsert_track(&meta)?;
        report.tracks_added += 1;
        if exists {
            report.to_analyse.push(id);
        } else {
            report.missing_files += 1;
            self.set_missing(id, true)?;
        }
        Ok(id)
    }

    /// Play count, rating, colour, cues, loops and grid of a merged track.
    fn merge_extras(
        &mut self,
        id: TrackId,
        t: &ForeignTrack,
        opts: &InteropOptions,
        report: &mut InteropReport,
    ) -> Result<()> {
        self.merge_stats(id, t)?;
        if !t.cues.is_empty() && (opts.replace_cues || self.cues(id)?.is_empty()) {
            self.set_cues(id, &t.cues)?;
            report.cues += t.cues.len();
        }
        if !t.loops.is_empty() && (opts.replace_cues || self.loops(id)?.is_empty()) {
            self.set_loops(id, &t.loops)?;
            report.loops += t.loops.len();
        }
        if let Some(g) = t.grid.filter(|g| g.bpm.is_finite() && g.bpm > 0.0)
            && (opts.replace_grids || !self.grid_set_by_user(id)?)
        {
            self.set_beatgrid(id, &g)?;
            report.grids += 1;
        }
        Ok(())
    }

    fn import_nodes(
        &mut self,
        nodes: &[ForeignNode],
        parent: Option<CrateId>,
        ids: &[TrackId],
        report: &mut InteropReport,
    ) -> Result<()> {
        for n in nodes {
            match n {
                ForeignNode::Folder { name, children } => {
                    // A folder is a crate with sub-crates only (a crate with
                    // sub-crates holds no tracks): a playlist in it named
                    // like the folder stays a sub-crate.
                    let c = self.find_or_create_crate(name, parent, report)?;
                    self.import_nodes(children, Some(c), ids, report)?;
                }
                ForeignNode::Playlist { name, tracks } => {
                    let c = self.find_or_create_crate(name, parent, report)?;
                    self.fill_crate(c, tracks, ids, report)?;
                }
            }
        }
        Ok(())
    }

    /// Adds a playlist's tracks (indices into `ids`) to crate `c`.
    fn fill_crate(
        &mut self,
        c: CrateId,
        tracks: &[usize],
        ids: &[TrackId],
        report: &mut InteropReport,
    ) -> Result<()> {
        let before = self.crate_tracks(c)?.len();
        let list: Vec<TrackId> = tracks.iter().filter_map(|i| ids.get(*i).copied()).collect();
        self.add_to_crate(c, &list)?;
        report.crate_entries += self.crate_tracks(c)?.len() - before;
        Ok(())
    }

    /// The crate `name` directly under `parent`, made if there is none.
    fn find_or_create_crate(
        &mut self,
        name: &str,
        parent: Option<CrateId>,
        report: &mut InteropReport,
    ) -> Result<CrateId> {
        let name = if name.trim().is_empty() {
            "Untitled"
        } else {
            name.trim()
        };
        if let Some(id) = self.child_crate(parent, name)? {
            return Ok(id);
        }
        report.crates += 1;
        self.create_crate(name, parent)
    }
}

/// `file://` URL (or a plain path) to a path: percent-decoded, and on
/// Windows-style drive paths (`/C:/Music/a.mp3`) with backslashes.
pub fn url_to_path(url: &str) -> Option<PathBuf> {
    let s = url.trim();
    let rest = if let Some(r) = s.strip_prefix("file://localhost") {
        r
    } else if let Some(r) = s.strip_prefix("file://") {
        r
    } else if s.contains("://") {
        return None;
    } else {
        s
    };
    let decoded = percent_encoding::percent_decode_str(rest)
        .decode_utf8()
        .ok()?
        .into_owned();
    // "/C:/Music/x.mp3" → "C:\Music\x.mp3"; "//server/share" stays a UNC path.
    let b = decoded.as_bytes();
    let drive = b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':';
    let p = if drive {
        decoded[1..].replace('/', "\\")
    } else if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        decoded.replace('/', "\\")
    } else if let Some(unc) = decoded.strip_prefix("//") {
        format!("\\\\{}", unc.replace('/', "\\"))
    } else {
        decoded
    };
    (!p.is_empty()).then(|| PathBuf::from(p))
}

/// Days since 1970-01-01 of a calendar date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// "2024-05-01", "2024-05-01T10:20:30Z" or "2024-05-01 10:20:30" to
/// seconds since the Unix epoch (the time read as UTC).
pub fn parse_date(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, time) = s.split_once(['T', ' ']).unwrap_or((s, ""));
    let mut it = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, d) = (it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let mut secs = days_from_civil(y, m, d) * 86_400;
    let t = time.trim_end_matches('Z');
    let mut hms = t.split(':').map(|p| p.parse::<i64>().ok());
    if let (Some(Some(h)), Some(Some(mi))) = (hms.next(), hms.next()) {
        secs += h * 3600 + mi * 60 + hms.next().flatten().unwrap_or(0);
    }
    Some(secs)
}

/// Saved loops into pad slots: loops with a slot keep it, the rest take
/// the free slots in order; loops beyond eight are dropped (counted).
pub(crate) fn slot_loops(loops: Vec<(Option<u8>, SavedLoop)>) -> (Vec<SavedLoop>, usize) {
    let mut used = [false; 8];
    let mut out = Vec::new();
    let mut rest = Vec::new();
    for (slot, l) in loops {
        match slot.filter(|s| *s < 8 && !used[usize::from(*s)]) {
            Some(s) => {
                used[usize::from(s)] = true;
                out.push(SavedLoop { slot: s, ..l });
            }
            None => rest.push(l),
        }
    }
    let mut dropped = 0;
    for l in rest {
        match used.iter().position(|u| !u) {
            Some(s) => {
                used[s] = true;
                out.push(SavedLoop { slot: s as u8, ..l });
            }
            None => dropped += 1,
        }
    }
    out.sort_by_key(|l| l.slot);
    (out, dropped)
}

/// Hot cues on pads 0-7 keep their pad (a second one on a taken pad
/// becomes a memory cue); the main cue and memory cues stay as they are.
pub(crate) fn tidy_cues(cues: Vec<Cue>) -> Vec<Cue> {
    let mut used = [false; 8];
    let mut out: Vec<Cue> = cues
        .into_iter()
        .map(|mut c| {
            if let CueKind::Hot(s) = c.kind {
                let s = usize::from(s.min(7));
                if used[s] {
                    c.kind = CueKind::Memory;
                } else {
                    used[s] = true;
                }
            }
            c
        })
        .collect();
    out.sort_by(|a, b| a.position.total_cmp(&b.position));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_become_paths() {
        assert_eq!(
            url_to_path("file://localhost/C:/Music/a%20b.mp3"),
            Some(PathBuf::from("C:\\Music\\a b.mp3"))
        );
        assert_eq!(
            url_to_path("file:///D:/M%C3%BAsica/x.flac"),
            Some(PathBuf::from("D:\\Música\\x.flac"))
        );
        assert_eq!(
            url_to_path("file://localhost/Users/me/Music/x.mp3"),
            Some(PathBuf::from("/Users/me/Music/x.mp3"))
        );
        assert_eq!(
            url_to_path("file://localhost//NAS/share/x.mp3"),
            Some(PathBuf::from("\\\\NAS\\share\\x.mp3"))
        );
        assert_eq!(url_to_path("http://example.com/stream"), None);
        assert_eq!(
            url_to_path("C:/Music/x.mp3"),
            Some(PathBuf::from("C:\\Music\\x.mp3"))
        );
    }

    #[test]
    fn dates() {
        assert_eq!(parse_date("1970-01-01"), Some(0));
        assert_eq!(parse_date("2024-02-29"), Some(19_782 * 86_400));
        assert_eq!(
            parse_date("2024-02-29T01:02:03Z"),
            Some(19_782 * 86_400 + 3723)
        );
        assert_eq!(parse_date("2024-13-01"), None);
        assert_eq!(parse_date("yesterday"), None);
    }

    #[test]
    fn loops_fill_free_pads() {
        let l = |s: f64| SavedLoop {
            slot: 0,
            start: s,
            end: s + 1.0,
            name: String::new(),
            color: None,
        };
        let mut loops = vec![(Some(2), l(1.0)), (Some(2), l(2.0))];
        loops.extend((0..8).map(|i| (None, l(10.0 + f64::from(i)))));
        let (out, dropped) = slot_loops(loops);
        assert_eq!(out.len(), 8);
        assert_eq!(dropped, 2);
        assert_eq!(out[2].start, 1.0, "the one asking for pad 3 keeps it");
        let slots: Vec<u8> = out.iter().map(|l| l.slot).collect();
        assert_eq!(slots, (0..8).collect::<Vec<u8>>());
    }
}
