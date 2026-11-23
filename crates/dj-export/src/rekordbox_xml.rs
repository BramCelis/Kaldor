//! rekordbox XML writer.
//!
//! Structure (AlphaTheta's published rekordbox XML format):
//!
//! ```text
//! DJ_PLAYLISTS Version="1.0.0"
//!   PRODUCT Name Version Company
//!   COLLECTION Entries
//!     TRACK TrackID Name Artist ... Location
//!       TEMPO Inizio Bpm Metro Battito
//!       POSITION_MARK Name Type Start [End] Num [Red Green Blue]
//!   PLAYLISTS
//!     NODE Type="0" Name="ROOT" Count
//!       NODE Type="0" (folder) Name Count
//!       NODE Type="1" (playlist) Name KeyType="0" Entries
//!         TRACK Key=<TrackID>
//! ```
//!
//! Crates map to playlists. A crate with sub-crates becomes a folder; if it
//! also holds tracks itself, those go into a playlist of the same name inside
//! that folder.

use dj_core::Beatgrid;
use dj_library::{CrateId, CrateNode, Cue, CueKind, Library, SavedLoop, Track, TrackId};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use quick_xml::Writer;
use quick_xml::events::{BytesDecl, BytesEnd, BytesStart, Event};
use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

/// Export errors.
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("library error: {0}")]
    Library(#[from] dj_library::LibraryError),
    #[error("write error: {0}")]
    Io(#[from] std::io::Error),
}

/// What to export.
#[derive(Clone, Debug, Default)]
pub struct XmlExportOptions {
    /// Crates to export (with their sub-crates). Empty = all crates.
    pub crates: Vec<CrateId>,
    /// Put every library track in the COLLECTION, not only the tracks in
    /// the exported crates.
    pub all_tracks: bool,
    /// Give each saved loop a hot pad no hot cue uses (its own slot when
    /// free, else the first free one), or make it a memory loop when all
    /// eight are taken. For players, which hold one hot cue per pad; the
    /// plain XML keeps the loop on its slot.
    pub loops_to_free_pads: bool,
    /// Write the grid's `Inizio` and `Bpm` with 6 decimals instead of
    /// rekordbox's 3 and 2. For the USB export, which computes every beat
    /// from them: `Bpm="128.00"` for a 127.996 BPM grid puts the last beats
    /// of a 6-minute track about 10 ms off Kaldor's grid.
    pub precise_tempo: bool,
}

/// The pad (`Num`) of each saved loop: its slot, or with `free_pads` the
/// slot when no hot cue or earlier loop uses it, else the first free pad,
/// else -1 (memory loop).
fn loop_pads(cues: &[Cue], loops: &[SavedLoop], free_pads: bool) -> Vec<i32> {
    if !free_pads {
        return loops.iter().map(|l| i32::from(l.slot.min(7))).collect();
    }
    let mut used = [false; 8];
    for c in cues {
        if let CueKind::Hot(n) = c.kind {
            used[usize::from(n.min(7))] = true;
        }
    }
    loops
        .iter()
        .map(|l| {
            let own = usize::from(l.slot.min(7));
            let pad = if used[own] {
                used.iter().position(|u| !u)
            } else {
                Some(own)
            };
            match pad {
                Some(p) => {
                    used[p] = true;
                    p as i32
                }
                None => -1,
            }
        })
        .collect()
}

/// What was written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportReport {
    pub tracks: usize,
    pub playlists: usize,
    pub folders: usize,
    pub cues: usize,
    pub loops: usize,
    /// Tracks whose file was missing (still exported; rekordbox will show
    /// them as missing too).
    pub missing_files: usize,
}

/// Characters kept as-is in a location URL: unreserved characters plus the
/// path separator and the drive colon.
const PATH_KEEP: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/')
    .remove(b':');

/// `file://localhost/` URL for a path, percent-encoded as UTF-8, with
/// forward slashes (`C:\Music\a b.mp3` → `file://localhost/C:/Music/a%20b.mp3`).
pub fn file_location(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    let s = s.strip_prefix("//?/").unwrap_or(&s).to_string();
    let encoded = utf8_percent_encode(&s, PATH_KEEP).to_string();
    if encoded.starts_with('/') {
        format!("file://localhost{encoded}")
    } else {
        format!("file://localhost/{encoded}")
    }
}

fn kind_for(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("mp3") => "MP3 File",
        Some("m4a" | "mp4" | "aac" | "alac") => "M4A File",
        Some("wav" | "wave") => "WAV File",
        Some("aif" | "aiff" | "aifc") => "AIFF File",
        Some("flac") => "FLAC File",
        Some("ogg") => "OGG File",
        _ => "Audio File",
    }
}

/// rekordbox rating scale: 0, 51, 102, 153, 204, 255 for 0 to 5 stars.
fn rating_value(stars: u8) -> u32 {
    u32::from(stars.min(5)) * 51
}

fn date(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

fn rgb(color: u32) -> [(&'static str, String); 3] {
    [
        ("Red", ((color >> 16) & 0xFF).to_string()),
        ("Green", ((color >> 8) & 0xFF).to_string()),
        ("Blue", (color & 0xFF).to_string()),
    ]
}

fn start(name: &str, attrs: &[(&str, String)]) -> BytesStart<'static> {
    let mut e = BytesStart::new(name.to_string());
    for (k, v) in attrs {
        e.push_attribute((*k, v.as_str()));
    }
    e
}

/// The playlist tree to write.
enum Node {
    Folder { name: String, children: Vec<Node> },
    Playlist { name: String, tracks: Vec<TrackId> },
}

fn build_nodes(lib: &Library, crates: &[CrateNode]) -> Result<Vec<Node>, ExportError> {
    let mut out = Vec::new();
    for c in crates {
        let tracks = lib.crate_tracks(c.id)?;
        if c.children.is_empty() {
            out.push(Node::Playlist {
                name: c.name.clone(),
                tracks,
            });
        } else {
            let mut children = Vec::new();
            if !tracks.is_empty() {
                children.push(Node::Playlist {
                    name: c.name.clone(),
                    tracks,
                });
            }
            children.extend(build_nodes(lib, &c.children)?);
            out.push(Node::Folder {
                name: c.name.clone(),
                children,
            });
        }
    }
    Ok(out)
}

fn collect_tracks(nodes: &[Node], out: &mut BTreeSet<TrackId>) {
    for n in nodes {
        match n {
            Node::Folder { children, .. } => collect_tracks(children, out),
            Node::Playlist { tracks, .. } => out.extend(tracks.iter().copied()),
        }
    }
}

fn find_crates(tree: &[CrateNode], wanted: &[CrateId], out: &mut Vec<CrateNode>) {
    for c in tree {
        if wanted.contains(&c.id) {
            out.push(c.clone());
        } else {
            find_crates(&c.children, wanted, out);
        }
    }
}

/// Writes a rekordbox XML file for the library (or the chosen crates).
pub fn export_xml<W: Write>(
    lib: &Library,
    opts: &XmlExportOptions,
    out: W,
) -> Result<ExportReport, ExportError> {
    let tree = lib.crate_tree()?;
    let selected = if opts.crates.is_empty() {
        tree
    } else {
        let mut v = Vec::new();
        find_crates(&tree, &opts.crates, &mut v);
        v
    };
    let nodes = build_nodes(lib, &selected)?;
    let mut ids = BTreeSet::new();
    collect_tracks(&nodes, &mut ids);
    if opts.all_tracks {
        ids.extend(lib.tracks()?.iter().map(|t| t.id));
    }
    let mut tracks: Vec<Track> = Vec::with_capacity(ids.len());
    for id in &ids {
        if let Some(t) = lib.track(*id)? {
            tracks.push(t);
        }
    }

    let mut report = ExportReport::default();
    let mut w = Writer::new_with_indent(out, b' ', 2);
    w.write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), None)))?;
    w.write_event(Event::Start(start(
        "DJ_PLAYLISTS",
        &[("Version", "1.0.0".into())],
    )))?;
    w.write_event(Event::Empty(start(
        "PRODUCT",
        &[
            ("Name", "Kaldor".into()),
            ("Version", env!("CARGO_PKG_VERSION").into()),
            ("Company", "Kaldor".into()),
        ],
    )))?;
    w.write_event(Event::Start(start(
        "COLLECTION",
        &[("Entries", tracks.len().to_string())],
    )))?;
    for t in &tracks {
        let grid = lib.beatgrid(t.id)?;
        let cues = lib.cues(t.id)?;
        let loops = lib.loops(t.id)?;
        if t.missing {
            report.missing_files += 1;
        }
        write_track(&mut w, t, grid, &cues, &loops, opts, &mut report)?;
        report.tracks += 1;
    }
    w.write_event(Event::End(BytesEnd::new("COLLECTION")))?;

    w.write_event(Event::Start(BytesStart::new("PLAYLISTS")))?;
    w.write_event(Event::Start(start(
        "NODE",
        &[
            ("Type", "0".into()),
            ("Name", "ROOT".into()),
            ("Count", nodes.len().to_string()),
        ],
    )))?;
    write_nodes(&mut w, &nodes, &mut report)?;
    w.write_event(Event::End(BytesEnd::new("NODE")))?;
    w.write_event(Event::End(BytesEnd::new("PLAYLISTS")))?;
    w.write_event(Event::End(BytesEnd::new("DJ_PLAYLISTS")))?;
    let mut inner = w.into_inner();
    inner.write_all(b"\n")?;
    inner.flush()?;
    Ok(report)
}

fn write_track<W: Write>(
    w: &mut Writer<W>,
    t: &Track,
    grid: Option<Beatgrid>,
    cues: &[Cue],
    loops: &[SavedLoop],
    opts: &XmlExportOptions,
    report: &mut ExportReport,
) -> Result<(), ExportError> {
    let m = &t.meta;
    let bpm = grid.map(|g| g.bpm).or(t.bpm);
    let mut attrs: Vec<(&str, String)> = vec![
        ("TrackID", t.id.to_string()),
        (
            "Name",
            if m.title.is_empty() {
                t.display_name()
            } else {
                m.title.clone()
            },
        ),
        ("Artist", m.artist.clone()),
        ("Album", m.album.clone()),
        ("Genre", m.genre.clone()),
        ("Kind", kind_for(&m.path).into()),
        ("Size", m.file_size.to_string()),
        // Whole seconds, truncated, as rekordbox writes it (and reads it).
        (
            "TotalTime",
            (m.duration.max(0.0).floor() as i64).to_string(),
        ),
        ("Year", m.year.map(|y| y.to_string()).unwrap_or_default()),
        (
            "AverageBpm",
            bpm.map(|b| format!("{b:.2}"))
                .unwrap_or_else(|| "0.00".into()),
        ),
        ("DateAdded", date(t.date_added)),
        ("BitRate", m.bitrate.unwrap_or(0).to_string()),
        ("SampleRate", m.sample_rate.unwrap_or(0).to_string()),
        ("Comments", m.comment.clone()),
        ("PlayCount", t.play_count.to_string()),
        ("Rating", rating_value(t.rating).to_string()),
        ("Location", file_location(&m.path)),
        ("Tonality", t.key.map(|k| k.musical()).unwrap_or_default()),
    ];
    if let Some(c) = t.color {
        attrs.push(("Colour", format!("0x{c:06X}")));
    }
    w.write_event(Event::Start(start("TRACK", &attrs)))?;

    if let Some(g) = grid {
        w.write_event(Event::Empty(start(
            "TEMPO",
            &[
                (
                    "Inizio",
                    if opts.precise_tempo {
                        format!("{:.6}", g.first_beat)
                    } else {
                        format!("{:.3}", g.first_beat)
                    },
                ),
                (
                    "Bpm",
                    if opts.precise_tempo {
                        format!("{:.6}", g.bpm)
                    } else {
                        format!("{:.2}", g.bpm)
                    },
                ),
                ("Metro", "4/4".into()),
                ("Battito", g.first_beat_in_bar.clamp(1, 4).to_string()),
            ],
        )))?;
    }
    for c in cues {
        // The main cue is written as a memory cue; rekordbox has no
        // separate "main cue" in XML.
        let num = match c.kind {
            CueKind::Hot(n) => i32::from(n.min(7)),
            CueKind::Main | CueKind::Memory => -1,
        };
        let mut a: Vec<(&str, String)> = vec![
            ("Name", c.name.clone()),
            ("Type", "0".into()),
            ("Start", format!("{:.3}", c.position)),
            ("Num", num.to_string()),
        ];
        if let Some(col) = c.color {
            a.extend(rgb(col));
        }
        w.write_event(Event::Empty(start("POSITION_MARK", &a)))?;
        report.cues += 1;
    }
    let pads = loop_pads(cues, loops, opts.loops_to_free_pads);
    for (l, pad) in loops.iter().zip(pads) {
        // Saved loops become hot loops on pads 0-7 (see `loop_pads`).
        let mut a: Vec<(&str, String)> = vec![
            ("Name", l.name.clone()),
            ("Type", "4".into()),
            ("Start", format!("{:.3}", l.start)),
            ("End", format!("{:.3}", l.end)),
            ("Num", pad.to_string()),
        ];
        if let Some(col) = l.color {
            a.extend(rgb(col));
        }
        w.write_event(Event::Empty(start("POSITION_MARK", &a)))?;
        report.loops += 1;
    }
    w.write_event(Event::End(BytesEnd::new("TRACK")))?;
    Ok(())
}

fn write_nodes<W: Write>(
    w: &mut Writer<W>,
    nodes: &[Node],
    report: &mut ExportReport,
) -> Result<(), ExportError> {
    for n in nodes {
        match n {
            Node::Folder { name, children } => {
                w.write_event(Event::Start(start(
                    "NODE",
                    &[
                        ("Type", "0".into()),
                        ("Name", name.clone()),
                        ("Count", children.len().to_string()),
                    ],
                )))?;
                write_nodes(w, children, report)?;
                w.write_event(Event::End(BytesEnd::new("NODE")))?;
                report.folders += 1;
            }
            Node::Playlist { name, tracks } => {
                w.write_event(Event::Start(start(
                    "NODE",
                    &[
                        ("Name", name.clone()),
                        ("Type", "1".into()),
                        ("KeyType", "0".into()),
                        ("Entries", tracks.len().to_string()),
                    ],
                )))?;
                for t in tracks {
                    w.write_event(Event::Empty(start("TRACK", &[("Key", t.to_string())])))?;
                }
                w.write_event(Event::End(BytesEnd::new("NODE")))?;
                report.playlists += 1;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations() {
        assert_eq!(
            file_location(Path::new(r"C:\Music\My Track #1 (Remix).mp3")),
            "file://localhost/C:/Music/My%20Track%20%231%20%28Remix%29.mp3"
        );
        assert_eq!(
            file_location(Path::new("/home/dj/Música/café & co.flac")),
            "file://localhost/home/dj/M%C3%BAsica/caf%C3%A9%20%26%20co.flac"
        );
        assert_eq!(
            file_location(Path::new(r"\\?\D:\a.wav")),
            "file://localhost/D:/a.wav"
        );
    }

    #[test]
    fn small_helpers() {
        assert_eq!(rating_value(0), 0);
        assert_eq!(rating_value(3), 153);
        assert_eq!(rating_value(9), 255);
        assert_eq!(date(0), "1970-01-01");
        assert_eq!(date(951_782_400), "2000-02-29");
        assert_eq!(kind_for(Path::new("a.FLAC")), "FLAC File");
        assert_eq!(kind_for(Path::new("a.m4a")), "M4A File");
    }

    #[test]
    fn saved_loops_move_off_taken_pads() {
        let hot = |n| Cue {
            kind: CueKind::Hot(n),
            position: 1.0,
            name: String::new(),
            color: None,
        };
        let lp = |slot| SavedLoop {
            slot,
            start: 1.0,
            end: 2.0,
            name: String::new(),
            color: None,
        };
        let cues = [hot(0), hot(1)];
        let loops = [lp(0), lp(3), lp(3)];
        assert_eq!(loop_pads(&cues, &loops, false), [0, 3, 3]);
        assert_eq!(loop_pads(&cues, &loops, true), [2, 3, 4]);
        let all: Vec<Cue> = (0..8).map(hot).collect();
        assert_eq!(loop_pads(&all, &[lp(5)], true), [-1]);
    }
}
