//! M3U / M3U8 and PLS playlist files, read (as a one-playlist
//! [`ForeignLibrary`]) and written.
//!
//! - M3U / M3U8: one path or `file://` URL per line, `#EXTINF:<seconds>,
//!   <Artist - Title>` before it; other `#` lines are comments. Relative
//!   paths are relative to the playlist's folder. M3U8 is UTF-8; an `.m3u`
//!   that is not valid UTF-8 is read as Latin-1.
//! - PLS: `[playlist]`, `FileN=`, `TitleN=`, `LengthN=`, `NumberOfEntries`,
//!   `Version=2`.
//!
//! Written files use absolute paths, UTF-8, Windows line ends on Windows.

use super::{ForeignLibrary, ForeignNode, ForeignTrack, url_to_path};
use crate::db::{Library, Result};
use crate::model::TrackId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One entry to write.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaylistEntry {
    pub path: PathBuf,
    /// Seconds (0 = unknown).
    pub duration: f64,
    /// "Artist - Title" (or the file name).
    pub display: String,
}

/// The entries for `ids` (unknown ids are left out).
pub fn entries_for(lib: &Library, ids: &[TrackId]) -> Result<Vec<PlaylistEntry>> {
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(t) = lib.track(*id)? {
            out.push(PlaylistEntry {
                display: t.display_name(),
                duration: t.meta.duration,
                path: t.meta.path,
            });
        }
    }
    Ok(out)
}

fn newline() -> &'static str {
    if cfg!(windows) { "\r\n" } else { "\n" }
}

/// An M3U8 playlist.
pub fn to_m3u8(entries: &[PlaylistEntry]) -> String {
    let nl = newline();
    let mut s = format!("#EXTM3U{nl}");
    for e in entries {
        let secs = if e.duration > 0.0 {
            e.duration.round() as i64
        } else {
            -1
        };
        s.push_str(&format!(
            "#EXTINF:{secs},{}{nl}{}{nl}",
            e.display.replace(['\r', '\n'], " "),
            e.path.display()
        ));
    }
    s
}

/// A PLS playlist.
pub fn to_pls(entries: &[PlaylistEntry]) -> String {
    let nl = newline();
    let mut s = format!("[playlist]{nl}");
    for (i, e) in entries.iter().enumerate() {
        let n = i + 1;
        let secs = if e.duration > 0.0 {
            e.duration.round() as i64
        } else {
            -1
        };
        s.push_str(&format!(
            "File{n}={}{nl}Title{n}={}{nl}Length{n}={secs}{nl}",
            e.path.display(),
            e.display.replace(['\r', '\n'], " ")
        ));
    }
    s.push_str(&format!(
        "NumberOfEntries={}{nl}Version=2{nl}",
        entries.len()
    ));
    s
}

/// Writes `entries` as M3U8 (`.m3u8`, `.m3u`) or PLS (`.pls`) by the
/// file's extension.
pub fn write(path: &Path, entries: &[PlaylistEntry]) -> std::io::Result<()> {
    let pls = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pls"));
    let text = if pls {
        to_pls(entries)
    } else {
        to_m3u8(entries)
    };
    std::fs::write(path, text)
}

/// Text from bytes: UTF-8 (without a byte-order mark), else Latin-1.
fn decode(bytes: &[u8]) -> String {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(b) {
        Ok(s) => s.to_string(),
        Err(_) => b.iter().map(|&c| char::from(c)).collect(),
    }
}

/// A path from an entry line: a URL, an absolute path, or one relative to
/// the playlist's folder.
fn resolve(line: &str, base: &Path) -> Option<PathBuf> {
    let p = url_to_path(line)?;
    Some(
        if p.is_absolute() || line.starts_with('\\') || line.starts_with('/') {
            p
        } else {
            base.join(p)
        },
    )
}

/// "Artist - Title" split back.
fn split_display(s: &str) -> (String, String) {
    match s.split_once(" - ") {
        Some((a, t)) => (a.trim().to_string(), t.trim().to_string()),
        None => (String::new(), s.trim().to_string()),
    }
}

struct Builder {
    lib: ForeignLibrary,
    by_path: HashMap<PathBuf, usize>,
    list: Vec<usize>,
}

impl Builder {
    fn new() -> Self {
        Self {
            lib: ForeignLibrary {
                source: "Playlist".into(),
                ..ForeignLibrary::default()
            },
            by_path: HashMap::new(),
            list: Vec::new(),
        }
    }

    fn push(&mut self, path: PathBuf, display: Option<&str>, secs: Option<f64>) {
        let i = match self.by_path.get(&path) {
            Some(i) => *i,
            None => {
                let (artist, title) = display.map(split_display).unwrap_or_default();
                let i = self.lib.tracks.len();
                self.lib.tracks.push(ForeignTrack {
                    path: path.clone(),
                    artist,
                    title,
                    duration: secs.filter(|s| *s > 0.0).unwrap_or(0.0),
                    ..ForeignTrack::default()
                });
                self.by_path.insert(path, i);
                i
            }
        };
        self.list.push(i);
    }

    fn finish(mut self, name: &str) -> ForeignLibrary {
        self.lib.playlists = vec![ForeignNode::Playlist {
            name: name.to_string(),
            tracks: self.list,
        }];
        self.lib
    }
}

/// Reads M3U / M3U8 text. `base` is the playlist's folder.
pub fn read_m3u(text: &str, base: &Path, name: &str) -> ForeignLibrary {
    let mut b = Builder::new();
    let mut info: Option<(Option<f64>, String)> = None;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let (secs, display) = rest.split_once(',').unwrap_or((rest, ""));
            info = Some((secs.trim().parse().ok(), display.trim().to_string()));
        } else if line.starts_with('#') {
            continue;
        } else {
            match resolve(line, base) {
                Some(p) => {
                    let (secs, display) = info.take().unwrap_or((None, String::new()));
                    let d = (!display.is_empty()).then_some(display.as_str());
                    b.push(p, d, secs);
                }
                None => b
                    .lib
                    .warnings
                    .push(format!("Left out \"{line}\": not a file")),
            }
        }
    }
    b.finish(name)
}

/// Reads PLS text. `base` is the playlist's folder.
pub fn read_pls(text: &str, base: &Path, name: &str) -> ForeignLibrary {
    let mut files: Vec<(u32, String)> = Vec::new();
    let mut titles: HashMap<u32, String> = HashMap::new();
    let mut lengths: HashMap<u32, f64> = HashMap::new();
    for line in text.lines() {
        let Some((k, v)) = line.trim().split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        let index = |prefix: &str| k.strip_prefix(prefix).and_then(|n| n.parse::<u32>().ok());
        if let Some(n) = index("file") {
            files.push((n, v.trim().to_string()));
        } else if let Some(n) = index("title") {
            titles.insert(n, v.trim().to_string());
        } else if let Some(n) = index("length")
            && let Ok(s) = v.trim().parse()
        {
            lengths.insert(n, s);
        }
    }
    files.sort_by_key(|(n, _)| *n);
    let mut b = Builder::new();
    for (n, f) in files {
        match resolve(&f, base) {
            Some(p) => b.push(
                p,
                titles.get(&n).map(String::as_str),
                lengths.get(&n).copied(),
            ),
            None => b.lib.warnings.push(format!("Left out \"{f}\": not a file")),
        }
    }
    b.finish(name)
}

/// Reads an `.m3u`, `.m3u8` or `.pls` file; the playlist is named after
/// the file.
pub fn read_file(path: &Path) -> std::result::Result<ForeignLibrary, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = decode(&bytes);
    let base = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Playlist".into());
    let pls = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("pls"))
        || text
            .trim_start()
            .to_ascii_lowercase()
            .starts_with("[playlist]");
    Ok(if pls {
        read_pls(&text, base, &name)
    } else {
        read_m3u(&text, base, &name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries() -> Vec<PlaylistEntry> {
        vec![
            PlaylistEntry {
                path: PathBuf::from(if cfg!(windows) {
                    "C:\\Music\\a b.mp3"
                } else {
                    "/m/a b.mp3"
                }),
                duration: 245.6,
                display: "Artist One - First".into(),
            },
            PlaylistEntry {
                path: PathBuf::from(if cfg!(windows) {
                    "C:\\Music\\ü.flac"
                } else {
                    "/m/ü.flac"
                }),
                duration: 0.0,
                display: "Second".into(),
            },
        ]
    }

    #[test]
    fn m3u8_and_pls_round_trip() {
        let base = Path::new(if cfg!(windows) { "C:\\Lists" } else { "/lists" });
        for (text, pls) in [(to_m3u8(&entries()), false), (to_pls(&entries()), true)] {
            let lib = if pls {
                read_pls(&text, base, "Set")
            } else {
                read_m3u(&text, base, "Set")
            };
            assert_eq!(lib.tracks.len(), 2, "{text}");
            assert_eq!(lib.tracks[0].path, entries()[0].path);
            assert_eq!(lib.tracks[0].artist, "Artist One");
            assert_eq!(lib.tracks[0].title, "First");
            assert_eq!(lib.tracks[0].duration, 246.0);
            assert_eq!(lib.tracks[1].path, entries()[1].path);
            assert_eq!(lib.tracks[1].title, "Second");
            assert_eq!(
                lib.playlists,
                vec![ForeignNode::Playlist {
                    name: "Set".into(),
                    tracks: vec![0, 1]
                }]
            );
        }
    }

    #[test]
    fn relative_paths_urls_and_latin1() {
        let base = Path::new(if cfg!(windows) { "C:\\Lists" } else { "/lists" });
        let text = "#EXTM3U\nsub/x.mp3\nfile:///C:/Music/y.mp3\nsub/x.mp3\nhttp://radio/x\n";
        let lib = read_m3u(text, base, "Mix");
        assert_eq!(lib.tracks.len(), 2, "the same file twice is one track");
        // (Paths compare by component, so "/" and "\" match on Windows.)
        assert_eq!(lib.tracks[0].path, base.join("sub").join("x.mp3"));
        assert_eq!(
            lib.playlists,
            vec![ForeignNode::Playlist {
                name: "Mix".into(),
                tracks: vec![0, 1, 0]
            }]
        );
        assert_eq!(lib.warnings.len(), 1);
        // "café.mp3" in Latin-1.
        assert_eq!(decode(b"caf\xe9.mp3"), "café.mp3");
        assert_eq!(decode("\u{feff}ok".as_bytes()), "ok");
    }
}
