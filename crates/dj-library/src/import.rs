//! Folder import: find audio files, read their tags with `lofty`, and add
//! them to the library.

use crate::db::{Library, Result};
use crate::model::{TrackId, TrackMeta};
use dj_core::MusicalKey;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::ItemKey;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// File extensions treated as audio (case-insensitive).
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "wav", "wave", "aif", "aiff", "aifc", "m4a", "mp4", "aac", "alac", "ogg",
];

pub fn is_audio_file(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Lists audio files under `root`, sorted. Unreadable directories are
/// skipped.
pub fn scan_folder(root: &Path, recursive: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            match entry.file_type() {
                Ok(t) if t.is_dir() => {
                    if recursive {
                        stack.push(p);
                    }
                }
                Ok(t) if t.is_file() && is_audio_file(&p) => out.push(p),
                _ => {}
            }
        }
    }
    out.sort();
    out
}

fn first_year(s: &str) -> Option<i32> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).take(4).collect();
    (digits.len() == 4).then(|| digits.parse().ok()).flatten()
}

/// Reads tags and properties. Never fails: unreadable tags give a record
/// with only the file name, size and time (analysis fills in the rest).
pub fn read_meta(path: &Path) -> TrackMeta {
    let mut m = TrackMeta {
        path: path.to_path_buf(),
        ..TrackMeta::default()
    };
    if let Ok(md) = std::fs::metadata(path) {
        m.file_size = md.len();
        m.file_mtime = md
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
    }
    let Ok(tagged) = lofty::read_from_path(path) else {
        return m;
    };
    let props = tagged.properties();
    m.duration = props.duration().as_secs_f64();
    m.bitrate = props.audio_bitrate();
    m.sample_rate = props.sample_rate();
    if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
        let get = |k: ItemKey| tag.get_string(k).map(|s| s.trim().to_string());
        m.title = get(ItemKey::TrackTitle).unwrap_or_default();
        m.artist = get(ItemKey::TrackArtist).unwrap_or_default();
        m.album = get(ItemKey::AlbumTitle).unwrap_or_default();
        m.genre = get(ItemKey::Genre).unwrap_or_default();
        m.comment = get(ItemKey::Comment).unwrap_or_default();
        m.year = get(ItemKey::Year)
            .or_else(|| get(ItemKey::RecordingDate))
            .as_deref()
            .and_then(first_year);
        m.tag_bpm = get(ItemKey::Bpm)
            .or_else(|| get(ItemKey::IntegerBpm))
            .and_then(|s| s.replace(',', ".").parse::<f64>().ok())
            .filter(|b| *b > 20.0 && *b < 400.0);
        m.tag_key = get(ItemKey::InitialKey)
            .as_deref()
            .and_then(MusicalKey::parse);
    }
    m
}

/// Result of an import.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImportReport {
    pub scanned: usize,
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub added_ids: Vec<TrackId>,
}

impl Library {
    /// Imports files. Files already in the library with the same size and
    /// modification time are left alone. Runs in batches of 500 per
    /// transaction; `progress(done, total)` is called after each batch.
    pub fn import_files(
        &mut self,
        paths: &[PathBuf],
        mut progress: impl FnMut(usize, usize),
    ) -> Result<ImportReport> {
        let mut report = ImportReport {
            scanned: paths.len(),
            ..ImportReport::default()
        };
        let known: std::collections::HashMap<PathBuf, (u64, i64)> = self
            .tracks()?
            .into_iter()
            .map(|t| (t.meta.path, (t.meta.file_size, t.meta.file_mtime)))
            .collect();
        for (i, chunk) in paths.chunks(500).enumerate() {
            let mut metas = Vec::with_capacity(chunk.len());
            for p in chunk {
                let m = read_meta(p);
                if known.get(p) == Some(&(m.file_size, m.file_mtime)) {
                    report.unchanged += 1;
                } else {
                    metas.push(m);
                }
            }
            for (id, added) in self.upsert_tracks(&metas)? {
                if added {
                    report.added += 1;
                    report.added_ids.push(id);
                } else {
                    report.updated += 1;
                }
            }
            progress(((i + 1) * 500).min(paths.len()), paths.len());
        }
        Ok(report)
    }

    /// Scans a folder and imports every audio file in it.
    pub fn import_folder(
        &mut self,
        root: &Path,
        recursive: bool,
        progress: impl FnMut(usize, usize),
    ) -> Result<ImportReport> {
        let files = scan_folder(root, recursive);
        self.import_files(&files, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_and_years() {
        assert!(is_audio_file(Path::new("a/b/Song.MP3")));
        assert!(is_audio_file(Path::new("x.aiff")));
        assert!(!is_audio_file(Path::new("cover.jpg")));
        assert!(!is_audio_file(Path::new("noext")));
        assert_eq!(first_year("2019-04-01"), Some(2019));
        assert_eq!(first_year("19"), None);
    }
}
