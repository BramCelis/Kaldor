//! Structure diff of two sticks: ours against one written by rekordbox from
//! the same tracks (golden tests).
//!
//! What can differ for good reasons (the analysis: grid, cues, waveforms
//! computed by different programs) is shown, not judged. What should match
//! is the structure: the files, the section tags and their order and header
//! sizes in every analysis file, the sizes of the waveform sections (they
//! follow the track's length), the paths, and the tables and track fields of
//! `export.pdb`.

use crate::{Beat, read_anlz};
use binrw::BinRead;
use rekordcrate::pdb::{Header, Row};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// One section of an ANLZ file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionInfo {
    pub tag: String,
    /// Header length (tag, lengths and the section's own header fields).
    pub header: u32,
    /// Whole section length.
    pub len: u32,
}

fn be32(b: &[u8], at: usize) -> Result<u32, String> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| format!("read past the end at 0x{at:x}"))
}

/// The sections of an ANLZ file, in order.
pub fn sections(b: &[u8]) -> Result<Vec<SectionInfo>, String> {
    if b.get(0..4) != Some(b"PMAI") {
        return Err("does not start with PMAI".into());
    }
    let mut pos = be32(b, 4)? as usize;
    let mut out = Vec::new();
    while pos + 12 <= b.len() {
        let tag = String::from_utf8_lossy(&b[pos..pos + 4]).into_owned();
        let header = be32(b, pos + 4)?;
        let len = be32(b, pos + 8)?;
        if len < 12 {
            return Err(format!("{tag} at 0x{pos:x}: length {len}"));
        }
        out.push(SectionInfo { tag, header, len });
        pos += len as usize;
    }
    Ok(out)
}

/// The top-level `name: value` pairs of a derived Debug string such as
/// `Track { id: TrackId(1), title: DeviceSQLString("A, B") }`.
pub fn debug_fields(s: &str) -> Vec<(String, String)> {
    let Some(open) = s.find('{') else {
        return Vec::new();
    };
    let body = s[open + 1..].trim_end().trim_end_matches('}');
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    let mut start = 0;
    let push = |part: &str, out: &mut Vec<(String, String)>| {
        if let Some((k, v)) = part.split_once(':') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    };
    for (i, c) in body.char_indices() {
        if in_str {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ',' if depth == 0 => {
                push(&body[start..i], &mut out);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&body[start..], &mut out);
    out
}

/// Every row of every table of an `export.pdb`, as Debug strings, by table.
/// A table rekordcrate cannot read is reported as an error string.
pub fn pdb_tables(path: &Path) -> Result<BTreeMap<String, Result<Vec<String>, String>>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let header = Header::read(&mut f).map_err(|e| format!("export.pdb header: {e}"))?;
    let mut out = BTreeMap::new();
    for t in &header.tables {
        let name = format!("{:?}", t.page_type);
        let rows = header
            .read_pages(&mut f, binrw::Endian::NATIVE, (&t.first_page, &t.last_page))
            .map(|pages| {
                pages
                    .iter()
                    .flat_map(|p| p.row_groups.iter())
                    .flat_map(|g| g.present_rows())
                    .map(|r: Row| format!("{r:?}"))
                    .collect::<Vec<_>>()
            })
            .map_err(|e| e.to_string().lines().next().unwrap_or_default().to_string());
        out.insert(name, rows);
    }
    Ok(out)
}

/// Files under `dir`, as lower-case `/` paths relative to it.
fn files(dir: &Path) -> BTreeMap<String, PathBuf> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if let Ok(rel) = p.strip_prefix(root) {
                let key = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().to_string())
                    .collect::<Vec<_>>()
                    .join("/");
                out.insert(key, p);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// The outcome: a readable report, and how many things differ in what
/// should be the same (structure).
#[derive(Clone, Debug, Default)]
pub struct Comparison {
    pub lines: Vec<String>,
    /// Structure differences (files, section layout, table and field sets).
    pub structure: Vec<String>,
}

fn grid_summary(b: &[Beat]) -> String {
    match (b.first(), b.last()) {
        (Some(f), Some(l)) if b.len() > 1 => format!(
            "{} beats, first {} ms (bar beat {}), {:.2} BPM",
            b.len(),
            f.time,
            f.beat_number,
            60_000.0 * (b.len() - 1) as f64 / f64::from(l.time - f.time).max(1.0)
        ),
        _ => format!("{} beats", b.len()),
    }
}

/// Median of `theirs[i] - nearest beat of ours`, in ms: how far the two
/// grids' beats are apart in phase (whatever their BPMs).
pub fn beat_offset_ms(ours: &[Beat], theirs: &[Beat]) -> Option<f64> {
    if ours.is_empty() || theirs.is_empty() {
        return None;
    }
    let mut d: Vec<f64> = theirs
        .iter()
        .map(|b| {
            let i = ours.partition_point(|o| o.time < b.time);
            [i.checked_sub(1), Some(i)]
                .into_iter()
                .flatten()
                .filter_map(|j| ours.get(j))
                .map(|o| f64::from(b.time) - f64::from(o.time))
                .min_by(|x, y| x.abs().total_cmp(&y.abs()))
                .unwrap_or(0.0)
        })
        .collect();
    d.sort_by(f64::total_cmp);
    Some(d[d.len() / 2])
}

/// Fields of a track row that come from the analysis or the export run, so
/// they differ between two programs without being wrong.
const EXPECTED_TO_DIFFER: [&str; 6] = [
    "tempo",
    "date_added",
    "analyze_date",
    "key_id",
    "artwork_id",
    "id",
];

/// Compares the stick at `ours` with the one at `golden` (each a drive root
/// or folder holding `PIONEER/` or `.PIONEER/`).
pub fn compare(ours: &Path, golden: &Path) -> Result<Comparison, String> {
    let root = |d: &Path| {
        [".PIONEER", "PIONEER"]
            .into_iter()
            .map(|r| d.join(r))
            .find(|r| r.is_dir())
            .ok_or_else(|| format!("no PIONEER folder under {}", d.display()))
    };
    let (ro, rg) = (root(ours)?, root(golden)?);
    let mut c = Comparison::default();

    // Files.
    let (fo, fg) = (files(&ro), files(&rg));
    c.lines.push("== Files under PIONEER".into());
    for k in fo.keys().chain(fg.keys()).collect::<BTreeSet<_>>() {
        match (fo.contains_key(k), fg.contains_key(k)) {
            (true, false) => c.structure.push(format!("only on ours: PIONEER/{k}")),
            (false, true) => c
                .structure
                .push(format!("only on rekordbox's: PIONEER/{k}")),
            _ => {}
        }
    }
    let (co, cg) = (
        files(&ours.join("Contents")),
        files(&golden.join("Contents")),
    );
    if co.keys().ne(cg.keys()) {
        c.structure.push(format!(
            "Contents differ: ours {:?}, rekordbox's {:?}",
            co.keys().collect::<Vec<_>>(),
            cg.keys().collect::<Vec<_>>()
        ));
    } else {
        c.lines
            .push(format!("Contents: the same {} files", co.len()));
    }
    for (k, po) in &fo {
        let Some(pg) = fg.get(k) else { continue };
        let so = std::fs::metadata(po).map(|m| m.len()).unwrap_or(0);
        let sg = std::fs::metadata(pg).map(|m| m.len()).unwrap_or(0);
        c.lines.push(format!("  {k}: {so} / {sg} bytes"));
    }

    // Analysis files.
    for (k, po) in &fo {
        let up = k.to_ascii_uppercase();
        if !(up.ends_with(".DAT") || up.ends_with(".EXT") || up.ends_with(".2EX"))
            || !up.starts_with("USBANLZ/")
        {
            continue;
        }
        let Some(pg) = fg.get(k) else { continue };
        let (bo, bg) = (
            std::fs::read(po).map_err(|e| e.to_string())?,
            std::fs::read(pg).map_err(|e| e.to_string())?,
        );
        c.lines.push(format!("== {k} (ours / rekordbox's)"));
        let (so, sg) = match (sections(&bo), sections(&bg)) {
            (Ok(a), Ok(b)) => (a, b),
            (a, b) => {
                c.structure
                    .push(format!("{k}: cannot walk: {:?} / {:?}", a.err(), b.err()));
                continue;
            }
        };
        let tags = |s: &[SectionInfo]| s.iter().map(|x| x.tag.clone()).collect::<Vec<_>>();
        if tags(&so) != tags(&sg) {
            c.structure
                .push(format!("{k}: sections {:?} / {:?}", tags(&so), tags(&sg)));
        }
        for i in 0..so.len().max(sg.len()) {
            let show = |s: Option<&SectionInfo>| {
                s.map_or("-".to_string(), |s| {
                    format!("{} h{} {}", s.tag, s.header, s.len)
                })
            };
            let (a, b) = (so.get(i), sg.get(i));
            let mark = match (a, b) {
                (Some(a), Some(b)) if a == b => "=",
                (Some(a), Some(b)) if a.tag == b.tag && a.header == b.header => "~",
                _ => "!",
            };
            if let (Some(a), Some(b)) = (a, b)
                && a.tag == b.tag
                && a.header != b.header
            {
                c.structure
                    .push(format!("{k}: {} header {} / {}", a.tag, a.header, b.header));
            }
            c.lines
                .push(format!("  {mark} {:<22} {}", show(a), show(b)));
        }
        if let (Ok(a), Ok(b)) = (read_anlz(&bo), read_anlz(&bg)) {
            if a.path != b.path {
                c.structure
                    .push(format!("{k}: PPTH {:?} / {:?}", a.path, b.path));
            }
            if !a.beats.is_empty() || !b.beats.is_empty() {
                c.lines.push(format!(
                    "  grid: {} / {}",
                    grid_summary(&a.beats),
                    grid_summary(&b.beats)
                ));
            }
            if let Some(off) = beat_offset_ms(&a.beats, &b.beats) {
                c.lines.push(format!(
                    "  rekordbox's beats sit {off:+.0} ms from our nearest beat (median)"
                ));
            }
            let cues = |f: &crate::AnlzFile| {
                f.cue_lists
                    .iter()
                    .map(|l| {
                        format!(
                            "{}{}:{}",
                            l.tag,
                            if l.hot { "hot" } else { "mem" },
                            l.cues.len()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            };
            c.lines.push(format!("  cues: {} / {}", cues(&a), cues(&b)));
        }
    }

    // export.pdb.
    let (po, pg) = (
        ro.join("rekordbox").join("export.pdb"),
        rg.join("rekordbox").join("export.pdb"),
    );
    if po.is_file() && pg.is_file() {
        let (to, tg) = (pdb_tables(&po)?, pdb_tables(&pg)?);
        c.lines
            .push("== export.pdb tables (rows: ours / rekordbox's)".into());
        for name in to.keys().chain(tg.keys()).collect::<BTreeSet<_>>() {
            let count = |t: Option<&Result<Vec<String>, String>>| match t {
                None => "absent".to_string(),
                Some(Ok(r)) => r.len().to_string(),
                Some(Err(e)) => format!("unreadable ({e})"),
            };
            let (a, b) = (to.get(name), tg.get(name));
            c.lines
                .push(format!("  {name:<20} {} / {}", count(a), count(b)));
            if a.is_none() || b.is_none() {
                c.structure
                    .push(format!("table {name}: {} / {}", count(a), count(b)));
            }
            // Small tables in full when they differ.
            if let (Some(Ok(ra)), Some(Ok(rb))) = (a, b)
                && name != "Tracks"
                && ra != rb
                && ra.len().max(rb.len()) <= 12
            {
                for r in ra {
                    c.lines.push(format!("    ours: {r}"));
                }
                for r in rb {
                    c.lines.push(format!("    rekordbox: {r}"));
                }
            }
        }
        // Track rows matched by their audio path.
        let tracks = |t: &BTreeMap<String, Result<Vec<String>, String>>| -> BTreeMap<String, Vec<(String, String)>> {
            t.get("Tracks")
                .and_then(|r| r.as_ref().ok())
                .map(|rows| {
                    rows.iter()
                        .map(|r| {
                            let f = debug_fields(r.trim_start_matches("Track(").trim_end_matches(')'));
                            let path = f
                                .iter()
                                .find(|(k, _)| k == "file_path")
                                .map(|(_, v)| v.to_lowercase())
                                .unwrap_or_default();
                            (path, f)
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let (ta, tb) = (tracks(&to), tracks(&tg));
        for (path, fa) in &ta {
            let Some(fb) = tb.get(path) else {
                c.structure.push(format!("track {path} only on ours"));
                continue;
            };
            c.lines.push(format!("== track {path}"));
            let mb: BTreeMap<&str, &str> =
                fb.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            for (k, va) in fa {
                let vb = mb.get(k.as_str()).copied().unwrap_or("(absent)");
                if va != vb {
                    let why = if EXPECTED_TO_DIFFER.contains(&k.as_str()) {
                        "  (analysis/run)"
                    } else {
                        ""
                    };
                    c.lines.push(format!("  {k}: {va} / {vb}{why}"));
                }
            }
        }
        for path in tb.keys() {
            if !ta.contains_key(path) {
                c.structure
                    .push(format!("track {path} only on rekordbox's"));
            }
        }
    }
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_fields_split_at_the_top_level() {
        let f = debug_fields(
            r#"Track { id: TrackId(1), title: DeviceSQLString("A, \"B\" {c}"), xs: [1, 2], n: 5 }"#,
        );
        let keys: Vec<&str> = f.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["id", "title", "xs", "n"]);
        assert_eq!(f[1].1, r#"DeviceSQLString("A, \"B\" {c}")"#);
        assert_eq!(f[2].1, "[1, 2]");
    }

    #[test]
    fn sections_are_listed_in_order() {
        let mut f = b"PMAI".to_vec();
        f.extend_from_slice(&0x1cu32.to_be_bytes());
        f.extend_from_slice(&0u32.to_be_bytes());
        f.resize(0x1c, 0);
        for tag in [b"PPTH", b"PQTZ"] {
            f.extend_from_slice(tag);
            f.extend_from_slice(&0x10u32.to_be_bytes());
            f.extend_from_slice(&0x14u32.to_be_bytes());
            f.extend_from_slice(&[0; 8]);
        }
        let s = sections(&f).unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(
            (s[1].tag.as_str(), s[1].header, s[1].len),
            ("PQTZ", 0x10, 0x14)
        );
    }
}
