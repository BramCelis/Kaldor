//! Reads a rekordbox USB stick and checks it.
//!
//! None of this shares code with the writer (baken-export):
//!
//! - `export.pdb` is read with [rekordcrate] (MPL-2.0, used unmodified), a
//!   parser written from Deep Symmetry's *Export Structure Analysis*.
//! - The ANLZ files are walked here, section by section, with the layouts in
//!   that analysis (they match rekordcrate's structs). Every section is also
//!   handed to rekordcrate; what it rejects is noted, not failed: rekordcrate
//!   0.3.0 only knows cue type 0 and 2, while the analysis (and rekordbox)
//!   use 1 for a cue point.
//! - The expected counts come from the rekordbox XML the stick was exported
//!   from, read with quick-xml.
//!
//! Checks: every track's audio file and analysis files are there; the
//! analysis path (`PPTH`) names the track's audio file; counts of tracks,
//! playlists, folders, playlist entries and cues match the XML; file names
//! and sizes fit FAT32; a OneLibrary is not older than the Device Library.
//!
//! [rekordcrate]: https://github.com/Holzhaus/rekordcrate

pub mod compare;

use binrw::BinRead;
use rekordcrate::anlz::{Content, Section};
use rekordcrate::pdb::{Header, PageType, Row};
use std::collections::{BTreeMap, HashMap};
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// One beat of a `PQTZ` grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beat {
    /// 1-4, the beat in the bar.
    pub beat_number: u16,
    /// Centi-BPM.
    pub tempo: u16,
    /// Milliseconds.
    pub time: u32,
}

/// One entry of a `PCOB` (`PCPT`) or `PCO2` (`PCP2`) list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CuePoint {
    /// 0 = memory cue, 1 = A, 2 = B ...
    pub hot_cue: u32,
    /// 1 = point (0 in rekordcrate), 2 = loop.
    pub kind: u8,
    pub time: u32,
    /// Loop end in ms, or `0xFFFF_FFFF`.
    pub loop_time: u32,
}

/// A cue section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CueList {
    /// `PCOB` or `PCO2`.
    pub tag: String,
    /// Hot cues (list type 1) or memory cues (0).
    pub hot: bool,
    pub cues: Vec<CuePoint>,
}

/// What an analysis file holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnlzFile {
    /// Section tags in file order.
    pub tags: Vec<String>,
    /// `PPTH`: the audio file's path on the stick.
    pub path: Option<String>,
    /// `PQTZ`.
    pub beats: Vec<Beat>,
    pub cue_lists: Vec<CueList>,
    /// Section tags rekordcrate could not parse (noted, not failed).
    pub rekordcrate_rejects: Vec<String>,
}

impl AnlzFile {
    /// Every cue once: from the `PCO2` lists if there are any (the `.EXT`
    /// holds all cues there), else from the `PCOB` lists.
    pub fn cues(&self) -> Vec<(bool, CuePoint)> {
        let tag = if self.cue_lists.iter().any(|l| l.tag == "PCO2") {
            "PCO2"
        } else {
            "PCOB"
        };
        self.cue_lists
            .iter()
            .filter(|l| l.tag == tag)
            .flat_map(|l| l.cues.iter().map(|c| (l.hot, *c)))
            .collect()
    }
}

fn be32(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("read past the end at 0x{at:x}"))
}

fn be16(b: &[u8], at: usize) -> Result<u16, String> {
    b.get(at..at + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or_else(|| format!("read past the end at 0x{at:x}"))
}

fn u8_at(b: &[u8], at: usize) -> Result<u8, String> {
    b.get(at)
        .copied()
        .ok_or_else(|| format!("read past the end at 0x{at:x}"))
}

fn tag_at(b: &[u8], at: usize) -> Result<String, String> {
    b.get(at..at + 4)
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .ok_or_else(|| format!("read past the end at 0x{at:x}"))
}

/// Walks an ANLZ file (`.DAT`, `.EXT`, `.2EX`).
pub fn read_anlz(b: &[u8]) -> Result<AnlzFile, String> {
    if tag_at(b, 0)? != "PMAI" {
        return Err("does not start with PMAI".into());
    }
    let header = be32(b, 4)? as usize;
    let total = be32(b, 8)? as usize;
    if total != b.len() {
        return Err(format!("PMAI says {total} bytes, the file has {}", b.len()));
    }
    let mut out = AnlzFile::default();
    let mut pos = header;
    while pos < b.len() {
        let tag = tag_at(b, pos)?;
        let hlen = be32(b, pos + 4)? as usize;
        let len = be32(b, pos + 8)? as usize;
        if len < 12 || hlen < 12 || hlen > len || pos + len > b.len() {
            return Err(format!(
                "{tag} at 0x{pos:x}: lengths {hlen}/{len} do not fit the file"
            ));
        }
        let s = &b[pos..pos + len];
        match tag.as_str() {
            "PPTH" => {
                let n = be32(s, 12)? as usize;
                let raw = s
                    .get(hlen..hlen + n)
                    .ok_or_else(|| "PPTH: path runs past the section".to_string())?;
                let units: Vec<u16> = raw
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes(*c))
                    .take_while(|u| *u != 0)
                    .collect();
                out.path = Some(String::from_utf16_lossy(&units));
            }
            "PQTZ" => {
                let n = be32(s, 0x14)? as usize;
                if hlen + n * 8 > len {
                    return Err(format!("PQTZ: {n} beats do not fit {len} bytes"));
                }
                for i in 0..n {
                    let e = hlen + i * 8;
                    out.beats.push(Beat {
                        beat_number: be16(s, e)?,
                        tempo: be16(s, e + 2)?,
                        time: be32(s, e + 4)?,
                    });
                }
            }
            "PCOB" | "PCO2" => {
                let pcob = tag == "PCOB";
                let hot = be32(s, 12)? == 1;
                let n = usize::from(be16(s, if pcob { 18 } else { 16 })?);
                let entry_tag = if pcob { "PCPT" } else { "PCP2" };
                let mut cues = Vec::with_capacity(n);
                let mut e = hlen;
                for _ in 0..n {
                    if tag_at(s, e)? != entry_tag {
                        return Err(format!("{tag}: entry at 0x{e:x} is not {entry_tag}"));
                    }
                    let elen = be32(s, e + 8)? as usize;
                    if elen < 12 || e + elen > len {
                        return Err(format!("{tag}: entry length {elen} does not fit"));
                    }
                    let (kind, time) = if pcob { (0x1c, 0x20) } else { (0x10, 0x14) };
                    cues.push(CuePoint {
                        hot_cue: be32(s, e + 12)?,
                        kind: u8_at(s, e + kind)?,
                        time: be32(s, e + time)?,
                        loop_time: be32(s, e + time + 4)?,
                    });
                    e += elen;
                }
                out.cue_lists.push(CueList {
                    tag: tag.clone(),
                    hot,
                    cues,
                });
            }
            _ => {}
        }
        // The independent cross-check.
        match Section::read_be(&mut Cursor::new(s)) {
            Ok(sec) => {
                if let Content::BeatGrid(g) = sec.content {
                    let theirs: Vec<(u16, u32)> =
                        g.beats.iter().map(|b| (b.tempo, b.time)).collect();
                    let ours: Vec<(u16, u32)> =
                        out.beats.iter().map(|b| (b.tempo, b.time)).collect();
                    if theirs != ours {
                        return Err("PQTZ: rekordcrate reads a different grid".into());
                    }
                }
            }
            Err(_) => out.rekordcrate_rejects.push(tag.clone()),
        }
        out.tags.push(tag);
        pos += len;
    }
    Ok(out)
}

/// A track row of `export.pdb`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PdbTrack {
    pub id: u32,
    pub title: String,
    /// `/Contents/...`
    pub file_path: String,
    /// `/PIONEER/USBANLZ/Pxxx/xxxxxxxx/ANLZ0000.DAT`
    pub analyze_path: String,
    /// Centi-BPM.
    pub tempo: u32,
}

/// The value of `name: DeviceSQLString("...")` in a Debug string
/// (rekordcrate keeps the track fields private).
fn debug_string(dbg: &str, name: &str) -> Option<String> {
    let key = format!("{name}: DeviceSQLString(\"");
    let start = dbg.find(&key)? + key.len();
    let mut out = String::new();
    let mut chars = dbg[start..].chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '0' => out.push('\0'),
                'u' => {
                    let hex: String = chars.by_ref().skip(1).take_while(|c| *c != '}').collect();
                    out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// The number after `name: ` or `name: Type(` in a Debug string.
fn debug_number(dbg: &str, name: &str) -> Option<u32> {
    let key = format!(" {name}: ");
    let start = dbg.find(&key)? + key.len();
    let rest = &dbg[start..];
    let rest = rest
        .find('(')
        .filter(|i| *i < 24)
        .map_or(rest, |i| &rest[i + 1..]);
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// What `export.pdb` holds.
#[derive(Clone, Debug, Default)]
pub struct Pdb {
    pub tracks: Vec<PdbTrack>,
    /// `(id, parent id, name, is folder)`.
    pub nodes: Vec<(u32, u32, String, bool)>,
    /// `(playlist id, track id)`.
    pub entries: Vec<(u32, u32)>,
}

/// Reads the tables of `export.pdb` this tool checks.
pub fn read_pdb(path: &Path) -> Result<Pdb, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let header = Header::read(&mut f).map_err(|e| format!("export.pdb header: {e}"))?;
    let mut pdb = Pdb::default();
    for t in &header.tables {
        if !matches!(
            t.page_type,
            PageType::Tracks | PageType::PlaylistTree | PageType::PlaylistEntries
        ) {
            continue;
        }
        let pages = header
            .read_pages(&mut f, binrw::Endian::NATIVE, (&t.first_page, &t.last_page))
            .map_err(|e| format!("export.pdb {:?} pages: {e}", t.page_type))?;
        for page in pages {
            for g in &page.row_groups {
                for row in g.present_rows() {
                    match row {
                        Row::Track(t) => {
                            let d = format!("{t:?}");
                            pdb.tracks.push(PdbTrack {
                                id: debug_number(&d, "id").unwrap_or(0),
                                title: debug_string(&d, "title").unwrap_or_default(),
                                file_path: debug_string(&d, "file_path").unwrap_or_default(),
                                analyze_path: debug_string(&d, "analyze_path").unwrap_or_default(),
                                tempo: debug_number(&d, "tempo").unwrap_or(0),
                            });
                        }
                        Row::PlaylistTreeNode(n) => {
                            let folder = n.is_folder();
                            pdb.nodes.push((
                                n.id.0,
                                n.parent_id.0,
                                n.name.into_string().unwrap_or_default(),
                                folder,
                            ));
                        }
                        Row::PlaylistEntry(e) => {
                            let d = format!("{e:?}");
                            pdb.entries.push((
                                debug_number(&d, "playlist_id").unwrap_or(0),
                                debug_number(&d, "track_id").unwrap_or(0),
                            ));
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    Ok(pdb)
}

/// The counts of a rekordbox XML.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Expected {
    pub tracks: usize,
    pub playlists: usize,
    pub folders: usize,
    pub entries: usize,
    /// Hot cues and hot loops (`Num` 0-7), memory cues and loops (`Num` -1).
    pub hot: usize,
    pub memory: usize,
    /// Per track title: (hot, memory). Titles that occur twice are left out.
    pub by_title: BTreeMap<String, (usize, usize)>,
    /// Per track title: the first `TEMPO` (`Inizio` in s, `Bpm`).
    pub grids: BTreeMap<String, (f64, f64)>,
}

/// Counts what a rekordbox XML holds.
pub fn read_xml(path: &Path) -> Result<Expected, String> {
    use quick_xml::events::Event;
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut r = quick_xml::Reader::from_str(&text);
    let mut x = Expected::default();
    let mut in_collection = false;
    let mut in_playlists = false;
    let mut title: Option<String> = None;
    let mut marks = (0, 0);
    let mut seen: HashMap<String, usize> = HashMap::new();
    let attr = |e: &quick_xml::events::BytesStart, k: &str| -> Option<String> {
        e.attributes().flatten().find_map(|a| {
            (a.key.as_ref() == k).then(|| {
                a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map(|v| v.into_owned())
                    .unwrap_or_default()
            })
        })
    };
    loop {
        let ev = r.read_event().map_err(|e| format!("XML: {e}"))?;
        let (e, empty) = match &ev {
            Event::Start(e) => (e, false),
            Event::Empty(e) => (e, true),
            Event::End(e) => {
                match e.name().as_ref() {
                    "COLLECTION" => in_collection = false,
                    "PLAYLISTS" => in_playlists = false,
                    "TRACK" if in_collection => {
                        if let Some(t) = title.take() {
                            *seen.entry(t.clone()).or_default() += 1;
                            x.by_title.insert(t, marks);
                        }
                    }
                    _ => {}
                }
                continue;
            }
            Event::Eof => break,
            _ => continue,
        };
        match e.name().as_ref() {
            "COLLECTION" => in_collection = !empty,
            "PLAYLISTS" => in_playlists = !empty,
            "TRACK" if in_collection => {
                x.tracks += 1;
                marks = (0, 0);
                let t = attr(e, "Name").unwrap_or_default();
                if empty {
                    *seen.entry(t.clone()).or_default() += 1;
                    x.by_title.insert(t, marks);
                } else {
                    title = Some(t);
                }
            }
            "TEMPO" if in_collection => {
                let num = |k: &str| attr(e, k).and_then(|v| v.parse::<f64>().ok());
                if let (Some(t), Some(inizio), Some(bpm)) = (&title, num("Inizio"), num("Bpm")) {
                    x.grids.entry(t.clone()).or_insert((inizio, bpm));
                }
            }
            "POSITION_MARK" if in_collection => {
                let num: i32 = attr(e, "Num").and_then(|n| n.parse().ok()).unwrap_or(-1);
                if (0..8).contains(&num) {
                    x.hot += 1;
                    marks.0 += 1;
                } else {
                    x.memory += 1;
                    marks.1 += 1;
                }
            }
            "NODE" if in_playlists => match attr(e, "Type").as_deref() {
                Some("1") => x.playlists += 1,
                Some("0") if attr(e, "Name").as_deref() != Some("ROOT") => x.folders += 1,
                _ => {}
            },
            "TRACK" if in_playlists => x.entries += 1,
            _ => {}
        }
    }
    x.by_title.retain(|t, _| seen.get(t) == Some(&1));
    x.grids.retain(|t, _| seen.get(t) == Some(&1));
    Ok(x)
}

/// Largest stick-beat distance from the grid that is accepted (ms).
pub const GRID_TOLERANCE_MS: f64 = 1.0;

/// Largest distance in ms between `beats` and the grid `inizio + n * 60 /
/// bpm` (beat n = the stick's n-th beat, as the XML's grid starts at its
/// first beat).
pub fn grid_error_ms(beats: &[Beat], inizio: f64, bpm: f64) -> f64 {
    let period = 60_000.0 / bpm;
    beats
        .iter()
        .enumerate()
        .map(|(n, b)| (f64::from(b.time) - (inizio * 1000.0 + n as f64 * period)).abs())
        .fold(0.0, f64::max)
}

/// One track as found on the stick.
#[derive(Clone, Debug, Default)]
pub struct TrackCheck {
    pub pdb: PdbTrack,
    pub dat: Option<AnlzFile>,
    pub ext: Option<AnlzFile>,
    /// From the `.EXT` if there is one, else the `.DAT`: (hot, memory).
    pub cues: (usize, usize),
    /// Largest distance in ms between the stick's beats and the XML's grid
    /// (`Inizio + n * 60 / Bpm`), when the XML has one for this title.
    pub grid_error_ms: Option<f64>,
}

/// The outcome.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub tracks: Vec<TrackCheck>,
    pub playlists: usize,
    pub folders: usize,
    pub entries: usize,
    pub onelibrary: bool,
    pub expected: Option<Expected>,
    /// Things that are wrong.
    pub problems: Vec<String>,
    /// Things worth knowing that are not wrong.
    pub notes: Vec<String>,
}

impl Report {
    pub fn ok(&self) -> bool {
        self.problems.is_empty()
    }
}

/// FAT32 allows 4 GiB - 1 per file.
pub const FAT32_MAX_FILE: u64 = 0xFFFF_FFFF;
/// Long file names: 255 UTF-16 units per name.
pub const FAT_MAX_NAME: usize = 255;

/// The stick path `/Contents/A/b.mp3` under `device`.
fn on_device(device: &Path, stick_path: &str) -> PathBuf {
    let mut p = device.to_path_buf();
    p.extend(stick_path.split('/').filter(|s| !s.is_empty()));
    p
}

fn check_names(dir: &Path, problems: &mut Vec<String>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.encode_utf16().count() > FAT_MAX_NAME {
            problems.push(format!("{}: name longer than FAT allows", p.display()));
        }
        if name
            .chars()
            .any(|c| c.is_control() || "\"*/:<>?\\|".contains(c))
        {
            problems.push(format!("{}: character FAT does not allow", p.display()));
        }
        match e.metadata() {
            Ok(m) if m.is_dir() => check_names(&p, problems),
            Ok(m) if m.len() > FAT32_MAX_FILE => {
                problems.push(format!("{}: over 4 GB, too big for FAT32", p.display()));
            }
            _ => {}
        }
    }
}

fn read_anlz_file(path: &Path) -> Result<AnlzFile, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read_anlz(&b).map_err(|e| format!("{}: {e}", path.display()))
}

/// Checks the stick at `device` (a drive root or folder holding `PIONEER/`
/// or `.PIONEER/`), against the XML it was exported from if given.
pub fn verify(device: &Path, xml: Option<&Path>) -> Result<Report, String> {
    let root = [".PIONEER", "PIONEER"]
        .into_iter()
        .map(|r| device.join(r))
        .find(|r| r.join("rekordbox").join("export.pdb").is_file())
        .ok_or_else(|| format!("no PIONEER/rekordbox/export.pdb under {}", device.display()))?;
    let rb = root.join("rekordbox");
    let pdb = read_pdb(&rb.join("export.pdb"))?;
    let mut r = Report {
        playlists: pdb.nodes.iter().filter(|n| !n.3).count(),
        folders: pdb.nodes.iter().filter(|n| n.3).count(),
        entries: pdb.entries.len(),
        ..Report::default()
    };

    let ids: HashMap<u32, &PdbTrack> = pdb.tracks.iter().map(|t| (t.id, t)).collect();
    for (pl, t) in &pdb.entries {
        if !ids.contains_key(t) {
            r.problems.push(format!(
                "playlist {pl} lists track {t}, which is not in the tracks table"
            ));
        }
        if !pdb.nodes.iter().any(|n| n.0 == *pl && !n.3) {
            r.problems.push(format!(
                "playlist entry for playlist {pl}, which does not exist"
            ));
        }
    }
    for n in &pdb.nodes {
        if n.1 != 0 && !pdb.nodes.iter().any(|p| p.0 == n.1 && p.3) {
            r.problems.push(format!(
                "\"{}\" has parent {}, which is not a folder",
                n.2, n.1
            ));
        }
    }

    let mut rejects: BTreeMap<String, usize> = BTreeMap::new();
    for t in &pdb.tracks {
        let mut tc = TrackCheck {
            pdb: t.clone(),
            ..TrackCheck::default()
        };
        let audio = on_device(device, &t.file_path);
        if t.file_path.is_empty() || !audio.is_file() {
            r.problems.push(format!(
                "\"{}\": audio file {} is missing",
                t.title, t.file_path
            ));
        }
        if t.analyze_path.is_empty() {
            r.problems
                .push(format!("\"{}\": no analysis path", t.title));
        } else {
            let dat = on_device(device, &t.analyze_path);
            let ext = dat.with_extension("EXT");
            for (path, slot) in [(dat, &mut tc.dat), (ext, &mut tc.ext)] {
                match read_anlz_file(&path) {
                    Ok(a) => {
                        if a.path.as_deref() != Some(t.file_path.as_str()) {
                            r.problems.push(format!(
                                "{}: PPTH {:?} does not name the track's file {}",
                                path.display(),
                                a.path,
                                t.file_path
                            ));
                        }
                        for c in a.cues() {
                            if c.1.kind > 2 {
                                r.problems.push(format!(
                                    "{}: cue type {} unknown",
                                    path.display(),
                                    c.1.kind
                                ));
                            }
                        }
                        for tag in &a.rekordcrate_rejects {
                            *rejects.entry(tag.clone()).or_default() += 1;
                        }
                        *slot = Some(a);
                    }
                    Err(e) => r.problems.push(e),
                }
            }
        }
        let with_cues = tc.ext.as_ref().or(tc.dat.as_ref());
        if let Some(a) = with_cues {
            let all = a.cues();
            tc.cues = (
                all.iter().filter(|c| c.0).count(),
                all.iter().filter(|c| !c.0).count(),
            );
        }
        if t.tempo > 0 && tc.dat.as_ref().is_some_and(|a| a.beats.is_empty()) {
            r.notes
                .push(format!("\"{}\": a tempo but no beat grid", t.title));
        }
        r.tracks.push(tc);
    }
    for (tag, n) in rejects {
        r.notes.push(format!(
            "rekordcrate 0.3.0 cannot parse {tag} in {n} file(s) (read here instead)"
        ));
    }

    // OneLibrary: optional, but never older than the Device Library.
    let one = rb.join("exportLibrary.db");
    r.onelibrary = one.is_file();
    if r.onelibrary {
        let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        if let (Some(a), Some(b)) = (modified(&one), modified(&rb.join("export.pdb")))
            && b.duration_since(a)
                .is_ok_and(|d| d > std::time::Duration::from_secs(60))
        {
            r.problems.push(
                "exportLibrary.db is older than export.pdb: a stale OneLibrary (rekordbox reports a library inconsistency)"
                    .into(),
            );
        }
    }
    for leftover in ["exportLibrary.db-wal", "exportLibrary.db-shm"] {
        if rb.join(leftover).exists() {
            r.notes.push(format!("{leftover} is on the stick"));
        }
    }
    for dir in [device.join("Contents"), root.clone()] {
        check_names(&dir, &mut r.problems);
    }

    if let Some(xml) = xml {
        let x = read_xml(xml)?;
        let mut cmp = |what: &str, stick: usize, xml: usize| {
            if stick != xml {
                r.problems
                    .push(format!("{what}: {stick} on the stick, {xml} in the XML"));
            }
        };
        cmp("tracks", r.tracks.len(), x.tracks);
        cmp("playlists", r.playlists, x.playlists);
        cmp("folders", r.folders, x.folders);
        cmp("playlist entries", r.entries, x.entries);
        let hot: usize = r.tracks.iter().map(|t| t.cues.0).sum();
        let mem: usize = r.tracks.iter().map(|t| t.cues.1).sum();
        cmp("hot cues and loops", hot, x.hot);
        cmp("memory cues and loops", mem, x.memory);
        for t in &mut r.tracks {
            let beats = t
                .dat
                .as_ref()
                .map(|a| a.beats.as_slice())
                .unwrap_or_default();
            if let Some(&(inizio, bpm)) = x.grids.get(&t.pdb.title)
                && bpm > 0.0
                && !beats.is_empty()
            {
                let err = grid_error_ms(beats, inizio, bpm);
                t.grid_error_ms = Some(err);
                // Beat times are whole ms on the stick: up to 0.5 ms is
                // rounding.
                if err > GRID_TOLERANCE_MS {
                    r.problems.push(format!(
                        "\"{}\": beat grid up to {err:.1} ms off the XML's ({bpm} BPM from {inizio} s)",
                        t.pdb.title
                    ));
                }
            }
        }
        for t in &r.tracks {
            if let Some(want) = x.by_title.get(&t.pdb.title)
                && *want != t.cues
            {
                r.problems.push(format!(
                    "\"{}\": {} hot and {} memory cues on the stick, {} and {} in the XML",
                    t.pdb.title, t.cues.0, t.cues.1, want.0, want.1
                ));
            }
        }
        r.expected = Some(x);
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(tag: &[u8; 4], header: &[u8], body: &[u8]) -> Vec<u8> {
        let mut v = tag.to_vec();
        v.extend_from_slice(&(12 + header.len() as u32).to_be_bytes());
        v.extend_from_slice(&((12 + header.len() + body.len()) as u32).to_be_bytes());
        v.extend_from_slice(header);
        v.extend_from_slice(body);
        v
    }

    fn file(sections: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = sections.concat();
        let mut v = b"PMAI".to_vec();
        v.extend_from_slice(&0x1cu32.to_be_bytes());
        v.extend_from_slice(&((0x1c + body.len()) as u32).to_be_bytes());
        v.resize(0x1c, 0);
        v.extend_from_slice(&body);
        v
    }

    #[test]
    fn walks_path_grid_and_cues() {
        let path: Vec<u8> = "/Contents/a.wav"
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_be_bytes)
            .collect();
        let ppth = section(b"PPTH", &(path.len() as u32).to_be_bytes(), &path);
        let mut grid_head = vec![0; 8];
        grid_head[4..8].copy_from_slice(&0x0008_0000u32.to_be_bytes());
        grid_head.extend_from_slice(&2u32.to_be_bytes());
        let mut beats = Vec::new();
        for (n, t) in [(1u16, 0u32), (2, 500)] {
            beats.extend_from_slice(&n.to_be_bytes());
            beats.extend_from_slice(&12000u16.to_be_bytes());
            beats.extend_from_slice(&t.to_be_bytes());
        }
        let pqtz = section(b"PQTZ", &grid_head, &beats);
        let mut pcpt = b"PCPT".to_vec();
        pcpt.extend_from_slice(&0x1cu32.to_be_bytes());
        pcpt.extend_from_slice(&0x38u32.to_be_bytes());
        pcpt.extend_from_slice(&1u32.to_be_bytes()); // hot cue A
        pcpt.extend_from_slice(&[0; 8]);
        pcpt.extend_from_slice(&[0xff; 4]);
        pcpt.push(1); // point
        pcpt.extend_from_slice(&[0, 3, 0xe8]);
        pcpt.extend_from_slice(&2000u32.to_be_bytes());
        pcpt.extend_from_slice(&u32::MAX.to_be_bytes());
        pcpt.resize(0x38, 0);
        let mut head = 1u32.to_be_bytes().to_vec();
        head.extend_from_slice(&0u16.to_be_bytes());
        head.extend_from_slice(&1u16.to_be_bytes());
        head.extend_from_slice(&u32::MAX.to_be_bytes());
        let pcob = section(b"PCOB", &head, &pcpt);
        let a = read_anlz(&file(&[ppth, pqtz, pcob])).unwrap();
        assert_eq!(a.tags, ["PPTH", "PQTZ", "PCOB"]);
        assert_eq!(a.path.as_deref(), Some("/Contents/a.wav"));
        assert_eq!(a.beats.len(), 2);
        assert_eq!(a.beats[1].time, 500);
        let cues = a.cues();
        assert_eq!(cues.len(), 1);
        assert!(cues[0].0);
        assert_eq!(
            (cues[0].1.hot_cue, cues[0].1.kind, cues[0].1.time),
            (1, 1, 2000)
        );
        // rekordcrate parsed the grid and path, but not the type-1 cue.
        assert_eq!(a.rekordcrate_rejects, ["PCOB"]);
    }

    #[test]
    fn broken_files_are_errors() {
        assert!(read_anlz(b"NOPE").is_err());
        let mut f = file(&[section(b"PQTZ", &[0; 12], &[])]);
        let n = f.len();
        f[n - 4..].copy_from_slice(&[0, 0, 0, 9]); // claims 9 beats
        assert!(read_anlz(&f).is_err());
    }

    #[test]
    fn debug_fields() {
        let d = r#"Track { id: TrackId(7), tempo: 12000, title: DeviceSQLString("Say \"hi\" \u{301}"), file_path: DeviceSQLString("/Contents/a.wav") }"#;
        assert_eq!(debug_number(d, "id"), Some(7));
        assert_eq!(debug_number(d, "tempo"), Some(12000));
        assert_eq!(
            debug_string(d, "title").as_deref(),
            Some("Say \"hi\" \u{301}")
        );
        assert_eq!(
            debug_string(d, "file_path").as_deref(),
            Some("/Contents/a.wav")
        );
    }
}
