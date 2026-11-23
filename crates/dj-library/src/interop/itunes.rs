//! iTunes / Apple Music "Library.xml" (File > Library > Export Library):
//! a property list with `Tracks` (a dict of track dicts by id) and
//! `Playlists` (an array of playlist dicts).
//!
//! Track fields read: Name, Artist, Album, Genre, Comments, Year, Total
//! Time (ms), BPM, Rating (0-100, 20 per star; a "Rating Computed" album
//! rating is left out), Play Count, Date Added, Location (a `file://`
//! URL). Streams, podcasts and videos are left out.
//!
//! Playlists: the library itself ("Master") and the built-in lists
//! ("Distinguished Kind": Music, Movies, ...) are left out; folders
//! ("Folder") nest by "Parent Persistent ID"; smart playlists come in with
//! the tracks they hold at export time.

use super::plist::{Value, root};
use super::xml::parse;
use super::{ForeignLibrary, ForeignNode, ForeignTrack, parse_date, url_to_path};
use std::collections::HashMap;

fn track(d: &Value, warnings: &mut Vec<String>) -> Option<ForeignTrack> {
    let name = d.str("Name").unwrap_or_default().trim().to_string();
    if d.flag("Podcast") || d.flag("Has Video") || d.flag("Movie") || d.flag("TV Show") {
        return None;
    }
    let location = d.str("Location")?;
    let Some(path) = url_to_path(location) else {
        warnings.push(format!("Left out \"{name}\": not a file ({location})"));
        return None;
    };
    let rating = if d.flag("Rating Computed") {
        0
    } else {
        (d.int("Rating").unwrap_or(0).clamp(0, 100) as f64 / 20.0).round() as u8
    };
    Some(ForeignTrack {
        path,
        title: name,
        artist: d.str("Artist").unwrap_or_default().trim().to_string(),
        album: d.str("Album").unwrap_or_default().trim().to_string(),
        genre: d.str("Genre").unwrap_or_default().trim().to_string(),
        comment: d.str("Comments").unwrap_or_default().trim().to_string(),
        year: d.int("Year").filter(|y| *y > 0).map(|y| y as i32),
        duration: d.int("Total Time").unwrap_or(0).max(0) as f64 / 1000.0,
        bpm: d.int("BPM").filter(|b| *b > 0).map(|b| b as f64),
        rating,
        play_count: d
            .int("Play Count")
            .unwrap_or(0)
            .clamp(0, i64::from(u32::MAX)) as u32,
        date_added: d.str("Date Added").and_then(parse_date),
        ..ForeignTrack::default()
    })
}

/// A playlist before nesting.
struct Flat {
    name: String,
    id: Option<String>,
    parent: Option<String>,
    folder: bool,
    tracks: Vec<usize>,
}

fn nest(all: &[Flat], parent: Option<&str>, depth: usize) -> Vec<ForeignNode> {
    if depth > 32 {
        return Vec::new();
    }
    let ids: Vec<&str> = all.iter().filter_map(|p| p.id.as_deref()).collect();
    all.iter()
        .filter(|p| {
            let own = p.parent.as_deref().filter(|q| ids.contains(q));
            own == parent
        })
        .map(|p| {
            if p.folder {
                ForeignNode::Folder {
                    name: p.name.clone(),
                    children: p
                        .id
                        .as_deref()
                        .map(|id| nest(all, Some(id), depth + 1))
                        .unwrap_or_default(),
                }
            } else {
                ForeignNode::Playlist {
                    name: p.name.clone(),
                    tracks: p.tracks.clone(),
                }
            }
        })
        .collect()
}

/// Reads an iTunes / Music library XML document.
pub fn read(xml: &str) -> Result<ForeignLibrary, String> {
    let doc = parse(xml)?;
    let top = root(&doc).ok_or("not an iTunes / Music library file (no <plist>)")?;
    let Some(tracks) = top.get("Tracks") else {
        return Err("not an iTunes / Music library file (no Tracks)".into());
    };
    let mut lib = ForeignLibrary {
        source: "iTunes".into(),
        ..ForeignLibrary::default()
    };
    let mut by_id: HashMap<i64, usize> = HashMap::new();
    for (k, d) in tracks.entries() {
        if let Some(t) = track(d, &mut lib.warnings) {
            let id = d.int("Track ID").or_else(|| k.parse().ok());
            if let Some(id) = id {
                by_id.insert(id, lib.tracks.len());
            }
            lib.tracks.push(t);
        }
    }
    let mut flat = Vec::new();
    for p in top.get("Playlists").map(Value::items).unwrap_or_default() {
        if p.flag("Master")
            || p.get("Distinguished Kind").is_some()
            || p.get("Visible") == Some(&Value::Bool(false))
        {
            continue;
        }
        let tracks = p
            .get("Playlist Items")
            .map(Value::items)
            .unwrap_or_default()
            .iter()
            .filter_map(|i| i.int("Track ID"))
            .filter_map(|id| by_id.get(&id).copied())
            .collect();
        flat.push(Flat {
            name: p.str("Name").unwrap_or("Untitled").trim().to_string(),
            id: p.str("Playlist Persistent ID").map(str::to_string),
            parent: p.str("Parent Persistent ID").map(str::to_string),
            folder: p.flag("Folder"),
            tracks,
        });
    }
    lib.playlists = nest(&flat, None, 0);
    Ok(lib)
}

/// Reads an iTunes / Music library XML file.
pub fn read_file(path: &std::path::Path) -> Result<ForeignLibrary, String> {
    let xml = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    read(&xml)
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Major Version</key><integer>1</integer>
  <key>Tracks</key>
  <dict>
    <key>101</key>
    <dict>
      <key>Track ID</key><integer>101</integer>
      <key>Name</key><string>Sunrise</string>
      <key>Artist</key><string>Dawn &amp; Dusk</string>
      <key>Album</key><string>Days</string>
      <key>Genre</key><string>Deep House</string>
      <key>Total Time</key><integer>312500</integer>
      <key>Year</key><integer>2019</integer>
      <key>BPM</key><integer>122</integer>
      <key>Rating</key><integer>80</integer>
      <key>Play Count</key><integer>12</integer>
      <key>Date Added</key><date>2020-03-04T05:06:07Z</date>
      <key>Location</key><string>file://localhost/C:/Users/me/Music/Sunrise.m4a</string>
    </dict>
    <key>102</key>
    <dict>
      <key>Track ID</key><integer>102</integer>
      <key>Name</key><string>Album rated</string>
      <key>Rating</key><integer>100</integer>
      <key>Rating Computed</key><true/>
      <key>Location</key><string>file:///C:/Users/me/Music/b.mp3</string>
    </dict>
    <key>103</key>
    <dict>
      <key>Track ID</key><integer>103</integer>
      <key>Name</key><string>Radio</string>
      <key>Track Type</key><string>URL</string>
      <key>Location</key><string>http://radio.example/stream</string>
    </dict>
    <key>104</key>
    <dict>
      <key>Track ID</key><integer>104</integer>
      <key>Name</key><string>Episode</string>
      <key>Podcast</key><true/>
      <key>Location</key><string>file:///C:/p.mp3</string>
    </dict>
  </dict>
  <key>Playlists</key>
  <array>
    <dict><key>Name</key><string>Library</string><key>Master</key><true/>
      <key>Playlist Items</key><array><dict><key>Track ID</key><integer>101</integer></dict></array></dict>
    <dict><key>Name</key><string>Music</string><key>Distinguished Kind</key><integer>4</integer></dict>
    <dict><key>Name</key><string>Sets</string><key>Folder</key><true/>
      <key>Playlist Persistent ID</key><string>AAA</string></dict>
    <dict><key>Name</key><string>Warm-up</string>
      <key>Playlist Persistent ID</key><string>BBB</string>
      <key>Parent Persistent ID</key><string>AAA</string>
      <key>Playlist Items</key><array>
        <dict><key>Track ID</key><integer>102</integer></dict>
        <dict><key>Track ID</key><integer>101</integer></dict>
        <dict><key>Track ID</key><integer>103</integer></dict>
      </array></dict>
  </array>
</dict>
</plist>"#;

    #[test]
    fn reads_tracks_and_nested_playlists() {
        let lib = read(XML).unwrap();
        assert_eq!(lib.tracks.len(), 2, "stream and podcast left out");
        let t = &lib.tracks[0];
        assert_eq!(t.title, "Sunrise");
        assert_eq!(t.artist, "Dawn & Dusk");
        assert_eq!(
            t.path,
            std::path::PathBuf::from("C:\\Users\\me\\Music\\Sunrise.m4a")
        );
        assert!((t.duration - 312.5).abs() < 1e-9);
        assert_eq!((t.bpm, t.rating, t.play_count), (Some(122.0), 4, 12));
        assert_eq!(t.year, Some(2019));
        assert_eq!(
            t.date_added,
            super::super::parse_date("2020-03-04T05:06:07Z")
        );
        assert_eq!(
            lib.tracks[1].rating, 0,
            "a computed album rating is not the track's"
        );
        assert_eq!(
            lib.playlists,
            vec![ForeignNode::Folder {
                name: "Sets".into(),
                children: vec![ForeignNode::Playlist {
                    name: "Warm-up".into(),
                    tracks: vec![1, 0],
                }],
            }]
        );
        assert!(lib.warnings.iter().any(|w| w.contains("Radio")));
    }

    #[test]
    fn refuses_other_files() {
        assert!(read("<DJ_PLAYLISTS/>").is_err());
        assert!(read("<plist><dict></dict></plist>").is_err());
    }
}
