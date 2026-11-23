//! Library data types.

use dj_core::MusicalKey;
use std::path::PathBuf;

pub type TrackId = i64;
pub type CrateId = i64;

/// What we know about a file from its tags and properties.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrackMeta {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub comment: String,
    pub year: Option<i32>,
    /// Seconds.
    pub duration: f64,
    /// kbit/s.
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub file_size: u64,
    /// Modification time, seconds since the Unix epoch.
    pub file_mtime: i64,
    /// BPM written in the file's tags (not analysed).
    pub tag_bpm: Option<f64>,
    /// Key written in the file's tags (not analysed).
    pub tag_key: Option<MusicalKey>,
}

/// A track in the library.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    pub id: TrackId,
    pub meta: TrackMeta,
    /// Analysed BPM (falls back to the tag BPM until analysed).
    pub bpm: Option<f64>,
    pub key: Option<MusicalKey>,
    pub lufs: Option<f64>,
    /// Auto-gain in dB.
    pub gain_db: Option<f64>,
    /// 0 to 5 stars.
    pub rating: u8,
    /// 0xRRGGBB.
    pub color: Option<u32>,
    pub play_count: u32,
    /// Seconds since the Unix epoch.
    pub date_added: i64,
    pub missing: bool,
    /// Analysis version, if analysed.
    pub analysis_version: Option<u32>,
}

impl Track {
    /// "Artist - Title", or the file name when the tags are empty.
    pub fn display_name(&self) -> String {
        match (self.meta.artist.is_empty(), self.meta.title.is_empty()) {
            (false, false) => format!("{} - {}", self.meta.artist, self.meta.title),
            (true, false) => self.meta.title.clone(),
            _ => self
                .meta
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

/// Kind of cue point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CueKind {
    /// The main (CUE button) cue.
    Main,
    /// Hot cue in pad slot 0..=7.
    Hot(u8),
    /// Memory cue (no pad).
    Memory,
}

/// A cue point.
#[derive(Clone, Debug, PartialEq)]
pub struct Cue {
    pub kind: CueKind,
    /// Seconds from the start of the track.
    pub position: f64,
    pub name: String,
    pub color: Option<u32>,
}

/// A saved loop.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedLoop {
    /// Pad slot 0..=7.
    pub slot: u8,
    pub start: f64,
    pub end: f64,
    pub name: String,
    pub color: Option<u32>,
}

/// One crate with its sub-crates.
#[derive(Clone, Debug, PartialEq)]
pub struct CrateNode {
    pub id: CrateId,
    pub name: String,
    pub parent: Option<CrateId>,
    /// A smart crate (fills itself from rules).
    pub smart: bool,
    /// Kept in step with another program (the rekordbox tab), not edited
    /// by hand.
    pub synced: bool,
    pub children: Vec<CrateNode>,
}
