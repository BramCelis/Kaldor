//! rekordbox XML (`DJ_PLAYLISTS`), as AlphaTheta publishes it and as
//! rekordbox and Kaldor (`dj-export`) write it:
//!
//! ```text
//! DJ_PLAYLISTS
//!   COLLECTION
//!     TRACK TrackID Name Artist Album Genre TotalTime Year AverageBpm
//!           DateAdded Comments PlayCount Rating Location Tonality Colour
//!       TEMPO Inizio Bpm Metro Battito          (one or more)
//!       POSITION_MARK Name Type Start [End] Num [Red Green Blue]
//!   PLAYLISTS
//!     NODE Type="0" Name="ROOT"
//!       NODE Type="0" Name (folder)
//!       NODE Type="1" Name KeyType Entries (playlist)
//!         TRACK Key               (KeyType 0: TrackID, 1: Location)
//! ```
//!
//! `POSITION_MARK` Type 0 is a cue (Num 0-7 a hot cue, -1 a memory cue),
//! 3 the load cue (Kaldor's main cue), 4 a loop (with End); 1 and 2 (fade
//! in / out) are left out. Rating is 0-255 (51 per star). Several `TEMPO`
//! points (a variable tempo) are read as one grid at the first point's
//! tempo, with a warning.

use super::xml::{El, parse};
use super::{
    ForeignLibrary, ForeignNode, ForeignTrack, parse_date, slot_loops, tidy_cues, url_to_path,
};
use crate::model::{Cue, CueKind, SavedLoop};
use dj_core::{Beatgrid, MusicalKey};
use std::collections::HashMap;

fn num<T: std::str::FromStr>(el: &El, k: &str) -> Option<T> {
    el.attr(k).and_then(|v| v.trim().parse().ok())
}

fn text(el: &El, k: &str) -> String {
    el.attr(k).unwrap_or_default().trim().to_string()
}

/// `0xRRGGBB` or `#RRGGBB`.
fn colour(s: &str) -> Option<u32> {
    let h = s.trim();
    let h = h
        .strip_prefix("0x")
        .or_else(|| h.strip_prefix("0X"))
        .or_else(|| h.strip_prefix('#'))?;
    u32::from_str_radix(h, 16).ok().map(|c| c & 0xFF_FFFF)
}

fn rgb(el: &El) -> Option<u32> {
    let c = |k| num::<u32>(el, k).map(|v| v.min(255));
    Some((c("Red")? << 16) | (c("Green")? << 8) | c("Blue")?)
}

/// A grid at `bpm` through a beat at `start` seconds that is beat
/// `beat_in_bar` (1-4) of its bar, as rekordbox gives a grid (XML `TEMPO`,
/// ANLZ `PQTZ`).
pub(crate) fn grid_at(start: f64, bpm: f64, beat_in_bar: i64) -> Beatgrid {
    let mut g = Beatgrid::new(bpm, start.max(0.0));
    // `new` may have moved the first beat earlier by whole beats, so count
    // back.
    let shift = ((start - g.first_beat) / g.period()).round() as i64;
    g.first_beat_in_bar = ((beat_in_bar.clamp(1, 4) - 1 - shift).rem_euclid(4) + 1) as u8;
    g.number_bars_from_start();
    g
}

fn track(el: &El, warnings: &mut Vec<String>) -> Option<ForeignTrack> {
    let location = el.attr("Location")?;
    let Some(path) = url_to_path(location) else {
        warnings.push(format!(
            "Left out \"{}\": not a file ({location})",
            text(el, "Name")
        ));
        return None;
    };
    let mut t = ForeignTrack {
        path,
        title: text(el, "Name"),
        artist: text(el, "Artist"),
        album: text(el, "Album"),
        genre: text(el, "Genre"),
        comment: text(el, "Comments"),
        year: num::<i32>(el, "Year").filter(|y| *y > 0),
        duration: num::<f64>(el, "TotalTime").unwrap_or(0.0),
        bpm: num::<f64>(el, "AverageBpm").filter(|b| *b > 0.0),
        key: el.attr("Tonality").and_then(MusicalKey::parse),
        rating: (num::<u32>(el, "Rating").unwrap_or(0) as f64 / 51.0).round() as u8,
        color: el.attr("Colour").and_then(colour),
        play_count: num(el, "PlayCount").unwrap_or(0),
        date_added: el.attr("DateAdded").and_then(parse_date),
        ..ForeignTrack::default()
    };
    t.rating = t.rating.min(5);

    let tempos: Vec<&El> = el.all("TEMPO").collect();
    if let Some(first) = tempos.first()
        && let (Some(start), Some(bpm)) = (num::<f64>(first, "Inizio"), num::<f64>(first, "Bpm"))
        && bpm > 0.0
    {
        let battito = num::<i64>(first, "Battito").unwrap_or(1);
        t.grid = Some(grid_at(start, bpm, battito));
        if tempos
            .iter()
            .skip(1)
            .any(|p| num::<f64>(p, "Bpm").is_some_and(|b| (b - bpm).abs() > 0.01))
        {
            warnings.push(format!(
                "\"{}\" has a changing tempo: read as one grid at {bpm:.2} BPM",
                t.title
            ));
        }
    }

    let mut cues = Vec::new();
    let mut loops = Vec::new();
    for m in el.all("POSITION_MARK") {
        let Some(start) = num::<f64>(m, "Start").filter(|s| *s >= 0.0) else {
            continue;
        };
        let slot = num::<i32>(m, "Num").unwrap_or(-1);
        let name = text(m, "Name");
        let color = rgb(m);
        match num::<i32>(m, "Type").unwrap_or(0) {
            0 => cues.push(Cue {
                kind: if (0..8).contains(&slot) {
                    CueKind::Hot(slot as u8)
                } else {
                    CueKind::Memory
                },
                position: start,
                name,
                color,
            }),
            3 => cues.push(Cue {
                kind: CueKind::Main,
                position: start,
                name,
                color,
            }),
            4 => {
                if let Some(end) = num::<f64>(m, "End").filter(|e| *e > start) {
                    let slot = (0..8).contains(&slot).then_some(slot as u8);
                    loops.push((
                        slot,
                        SavedLoop {
                            slot: 0,
                            start,
                            end,
                            name,
                            color,
                        },
                    ));
                }
            }
            _ => {}
        }
    }
    t.cues = tidy_cues(cues);
    let (loops, dropped) = slot_loops(loops);
    t.loops = loops;
    if dropped > 0 {
        warnings.push(format!(
            "\"{}\": {dropped} loop(s) beyond the 8 pads left out",
            t.title
        ));
    }
    Some(t)
}

fn nodes(
    parent: &El,
    by_id: &HashMap<String, usize>,
    by_location: &HashMap<String, usize>,
    warnings: &mut Vec<String>,
) -> Vec<ForeignNode> {
    let mut out = Vec::new();
    for n in parent.all("NODE") {
        let name = text(n, "Name");
        match n.attr("Type") {
            Some("0") => out.push(ForeignNode::Folder {
                children: nodes(n, by_id, by_location, warnings),
                name,
            }),
            Some("1") => {
                let by_location_key = n.attr("KeyType") == Some("1");
                let mut tracks = Vec::new();
                let mut unknown = 0;
                for e in n.all("TRACK") {
                    let key = e.attr("Key").unwrap_or_default();
                    let found = if by_location_key {
                        url_to_path(key)
                            .and_then(|p| by_location.get(&p.to_string_lossy().to_lowercase()))
                    } else {
                        by_id.get(key.trim())
                    };
                    match found {
                        Some(i) => tracks.push(*i),
                        None => unknown += 1,
                    }
                }
                if unknown > 0 {
                    warnings.push(format!(
                        "Playlist \"{name}\": {unknown} entries not in the collection"
                    ));
                }
                out.push(ForeignNode::Playlist { name, tracks });
            }
            _ => {}
        }
    }
    out
}

/// Reads a rekordbox XML document.
pub fn read(xml: &str) -> Result<ForeignLibrary, String> {
    let root = parse(xml)?;
    if root.name != "DJ_PLAYLISTS" {
        return Err(format!(
            "not a rekordbox XML file (the root is <{}>, not <DJ_PLAYLISTS>)",
            root.name
        ));
    }
    let mut lib = ForeignLibrary {
        source: "rekordbox".into(),
        ..ForeignLibrary::default()
    };
    let mut by_id = HashMap::new();
    let mut by_location = HashMap::new();
    if let Some(coll) = root.child("COLLECTION") {
        for el in coll.all("TRACK") {
            if let Some(t) = track(el, &mut lib.warnings) {
                let i = lib.tracks.len();
                if let Some(id) = el.attr("TrackID") {
                    by_id.insert(id.trim().to_string(), i);
                }
                by_location.insert(t.path.to_string_lossy().to_lowercase(), i);
                lib.tracks.push(t);
            }
        }
    }
    if let Some(root_node) = root.child("PLAYLISTS").and_then(|p| p.child("NODE")) {
        lib.playlists = nodes(root_node, &by_id, &by_location, &mut lib.warnings);
    }
    Ok(lib)
}

/// Reads a rekordbox XML file.
pub fn read_file(path: &std::path::Path) -> Result<ForeignLibrary, String> {
    let xml = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read(&xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<DJ_PLAYLISTS Version="1.0.0">
  <PRODUCT Name="rekordbox" Version="7.0.0" Company="AlphaTheta"/>
  <COLLECTION Entries="3">
    <TRACK TrackID="11" Name="Night Drive" Artist="Lumen &amp; Co" Album="Alpha" Genre="House"
           TotalTime="245" Year="2021" AverageBpm="124.00" DateAdded="2024-05-01"
           Comments="peak" PlayCount="7" Rating="204" Location="file://localhost/C:/Music/Night%20Drive.mp3"
           Tonality="Am" Colour="0xFF0000">
      <TEMPO Inizio="0.120" Bpm="124.00" Metro="4/4" Battito="1"/>
      <POSITION_MARK Name="Drop" Type="0" Start="32.000" Num="0" Red="40" Green="226" Blue="20"/>
      <POSITION_MARK Name="" Type="0" Start="64.000" Num="-1"/>
      <POSITION_MARK Name="Load" Type="3" Start="0.120" Num="-1"/>
      <POSITION_MARK Name="Loop" Type="4" Start="96.000" End="103.742" Num="2"/>
      <POSITION_MARK Name="Mem loop" Type="4" Start="120.000" End="124.000" Num="-1"/>
      <POSITION_MARK Name="Fade" Type="1" Start="5.000" Num="-1"/>
    </TRACK>
    <TRACK TrackID="12" Name="Varied" Artist="B" Location="file://localhost/C:/Music/b.flac" AverageBpm="90">
      <TEMPO Inizio="1.0" Bpm="90" Metro="4/4" Battito="3"/>
      <TEMPO Inizio="60.0" Bpm="92" Metro="4/4" Battito="1"/>
    </TRACK>
    <TRACK TrackID="13" Name="Stream" Location="https://example.com/x"/>
  </COLLECTION>
  <PLAYLISTS>
    <NODE Type="0" Name="ROOT" Count="2">
      <NODE Type="0" Name="Gigs" Count="1">
        <NODE Type="1" Name="Friday" KeyType="0" Entries="3">
          <TRACK Key="12"/><TRACK Key="11"/><TRACK Key="99"/>
        </NODE>
      </NODE>
      <NODE Type="1" Name="By location" KeyType="1" Entries="1">
        <TRACK Key="file://localhost/C:/Music/Night%20Drive.mp3"/>
      </NODE>
    </NODE>
  </PLAYLISTS>
</DJ_PLAYLISTS>"#;

    #[test]
    fn reads_tracks_cues_loops_grids_and_playlists() {
        let lib = read(XML).unwrap();
        assert_eq!(lib.tracks.len(), 2, "the stream is left out");
        let t = &lib.tracks[0];
        assert_eq!(
            t.path,
            std::path::PathBuf::from("C:\\Music\\Night Drive.mp3")
        );
        assert_eq!(
            (t.title.as_str(), t.artist.as_str()),
            ("Night Drive", "Lumen & Co")
        );
        assert_eq!(t.rating, 4);
        assert_eq!(t.color, Some(0xFF0000));
        assert_eq!(t.play_count, 7);
        assert_eq!(t.key, MusicalKey::parse("Am"));
        assert_eq!(t.year, Some(2021));
        assert_eq!(t.date_added, Some(1_714_521_600));
        let g = t.grid.unwrap();
        assert!((g.bpm - 124.0).abs() < 1e-9 && (g.first_beat - 0.12).abs() < 1e-9);
        assert_eq!(g.first_beat_in_bar, 1);
        // Hot cue on pad 1 with its colour, a memory cue, the main cue.
        assert!(
            t.cues.iter().any(|c| c.kind == CueKind::Hot(0)
                && c.name == "Drop"
                && c.color == Some(0x28E214))
        );
        assert!(
            t.cues
                .iter()
                .any(|c| c.kind == CueKind::Memory && c.position == 64.0)
        );
        assert!(t.cues.iter().any(|c| c.kind == CueKind::Main));
        assert_eq!(t.cues.len(), 3, "the fade-in mark is left out");
        // The hot loop keeps pad 3, the memory loop takes pad 1.
        assert_eq!(t.loops.len(), 2);
        assert!(t.loops.iter().any(|l| l.slot == 2 && l.name == "Loop"));
        assert!(t.loops.iter().any(|l| l.slot == 0 && l.name == "Mem loop"));
        // A changing tempo: one grid, a warning. The point at 1.0 s is beat 3
        // of its bar; at 90 BPM the grid's first beat is one beat earlier
        // (0.33 s), beat 2.
        let v = lib.tracks[1].grid.unwrap();
        assert!((v.first_beat - 1.0 / 3.0).abs() < 1e-9, "{}", v.first_beat);
        assert_eq!(v.first_beat_in_bar, 2);
        assert!(lib.warnings.iter().any(|w| w.contains("changing tempo")));
        assert!(lib.warnings.iter().any(|w| w.contains("not a file")));
        // Playlists: a folder with one playlist (the unknown entry warned),
        // and a playlist by location.
        assert_eq!(
            lib.playlists,
            vec![
                ForeignNode::Folder {
                    name: "Gigs".into(),
                    children: vec![ForeignNode::Playlist {
                        name: "Friday".into(),
                        tracks: vec![1, 0]
                    }],
                },
                ForeignNode::Playlist {
                    name: "By location".into(),
                    tracks: vec![0]
                },
            ]
        );
        assert!(lib.warnings.iter().any(|w| w.contains("1 entries not in")));
    }

    #[test]
    fn refuses_other_xml() {
        assert!(read("<plist><dict/></plist>").is_err());
        assert!(read("not xml at all <").is_err());
    }
}
