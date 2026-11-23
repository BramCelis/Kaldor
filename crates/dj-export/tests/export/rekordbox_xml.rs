//! Exports a small library to rekordbox XML and parses it back.

use dj_core::{Beatgrid, MusicalKey};
use dj_export::{XmlExportOptions, export_xml};
use dj_library::{Cue, CueKind, Library, SavedLoop, TrackMeta};
use quick_xml::Reader;
use quick_xml::events::Event;
use std::collections::HashMap;
use std::path::PathBuf;

/// A parsed element: name, attributes, children.
#[derive(Debug, Clone)]
struct El {
    name: String,
    attrs: HashMap<String, String>,
    children: Vec<El>,
}

impl El {
    fn attr(&self, k: &str) -> &str {
        self.attrs.get(k).map(String::as_str).unwrap_or("<missing>")
    }
    fn child(&self, name: &str) -> &El {
        self.children.iter().find(|c| c.name == name).unwrap()
    }
    fn all(&self, name: &str) -> Vec<&El> {
        self.children.iter().filter(|c| c.name == name).collect()
    }
}

fn parse(xml: &str) -> El {
    let mut r = Reader::from_str(xml);
    let mut stack: Vec<El> = vec![El {
        name: "#doc".into(),
        attrs: HashMap::new(),
        children: vec![],
    }];
    let make = |e: &quick_xml::events::BytesStart| El {
        name: e.name().as_ref().to_string(),
        attrs: e
            .attributes()
            .map(|a| {
                let a = a.unwrap();
                (
                    a.key.as_ref().to_string(),
                    a.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .unwrap()
                        .into_owned(),
                )
            })
            .collect(),
        children: vec![],
    };
    loop {
        match r.read_event().unwrap() {
            Event::Start(e) => stack.push(make(&e)),
            Event::Empty(e) => {
                let el = make(&e);
                stack.last_mut().unwrap().children.push(el);
            }
            Event::End(_) => {
                let el = stack.pop().unwrap();
                stack.last_mut().unwrap().children.push(el);
            }
            Event::Eof => break,
            _ => {}
        }
    }
    stack.pop().unwrap().children.remove(0)
}

fn meta(path: &str, artist: &str, title: &str) -> TrackMeta {
    TrackMeta {
        path: PathBuf::from(path),
        artist: artist.into(),
        title: title.into(),
        album: "Album".into(),
        genre: "House".into(),
        duration: 245.6,
        bitrate: Some(320),
        sample_rate: Some(44_100),
        file_size: 9_876_543,
        ..TrackMeta::default()
    }
}

fn library() -> (Library, [i64; 3], [i64; 4]) {
    let mut lib = Library::open_in_memory().unwrap();
    let mut m1 = meta(
        r"C:\Music\Café & Bar\One <Edit>.mp3",
        "A & B",
        "One \"Edit\"",
    );
    m1.tag_key = MusicalKey::parse("8A");
    m1.year = Some(2021);
    let (t1, _) = lib.upsert_track(&m1).unwrap();
    let (t2, _) = lib
        .upsert_track(&meta(r"C:\Music\two.flac", "C", "Two"))
        .unwrap();
    let (t3, _) = lib
        .upsert_track(&meta(r"C:\Music\three.wav", "D", "Three"))
        .unwrap();
    lib.set_rating(t1, 4).unwrap();
    lib.set_beatgrid(t1, &Beatgrid::new(127.996, 0.2316))
        .unwrap();
    lib.set_cues(
        t1,
        &[
            Cue {
                kind: CueKind::Main,
                position: 0.2316,
                name: String::new(),
                color: None,
            },
            Cue {
                kind: CueKind::Hot(0),
                position: 30.25,
                name: "Drop".into(),
                color: Some(0x28E214),
            },
            Cue {
                kind: CueKind::Hot(7),
                position: 60.0,
                name: "Out".into(),
                color: None,
            },
        ],
    )
    .unwrap();
    lib.set_loops(
        t1,
        &[SavedLoop {
            slot: 2,
            start: 45.0,
            end: 46.875,
            name: "4 beats".into(),
            color: Some(0xFF8800),
        }],
    )
    .unwrap();

    // Crates: Sets (folder) > [Sets, Friday, Saturday]; Warm-up at the top
    // level. Sets held a track before it got sub-crates: the track went
    // into a first sub-crate "Sets" (a crate with sub-crates is a folder).
    let sets = lib.create_crate("Sets", None).unwrap();
    lib.add_to_crate(sets, &[t2]).unwrap();
    let fri = lib.create_crate("Friday", Some(sets)).unwrap();
    let sat = lib.create_crate("Saturday", Some(sets)).unwrap();
    let warm = lib.create_crate("Warm-up", None).unwrap();
    lib.add_to_crate(fri, &[t2, t1]).unwrap();
    lib.add_to_crate(sat, &[t1]).unwrap();
    lib.add_to_crate(warm, &[]).unwrap();
    (lib, [t1, t2, t3], [sets, fri, sat, warm])
}

#[test]
fn full_export_structure_and_values() {
    let (lib, [t1, t2, _t3], _) = library();
    let mut buf = Vec::new();
    let report = export_xml(&lib, &XmlExportOptions::default(), &mut buf).unwrap();
    let xml = String::from_utf8(buf).unwrap();
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
    let root = parse(&xml);
    assert_eq!(root.name, "DJ_PLAYLISTS");
    assert_eq!(root.attr("Version"), "1.0.0");
    assert_eq!(root.child("PRODUCT").attr("Name"), "Kaldor");

    // Only tracks that are in a crate (t3 is not).
    let coll = root.child("COLLECTION");
    assert_eq!(coll.attr("Entries"), "2");
    let tracks = coll.all("TRACK");
    assert_eq!(tracks.len(), 2);
    let tr1 = tracks
        .iter()
        .find(|t| t.attr("TrackID") == t1.to_string())
        .unwrap();
    assert_eq!(tr1.attr("Name"), "One \"Edit\"");
    assert_eq!(tr1.attr("Artist"), "A & B");
    assert_eq!(tr1.attr("Kind"), "MP3 File");
    assert_eq!(tr1.attr("Size"), "9876543");
    assert_eq!(tr1.attr("TotalTime"), "245");
    assert_eq!(tr1.attr("AverageBpm"), "128.00");
    assert_eq!(tr1.attr("BitRate"), "320");
    assert_eq!(tr1.attr("SampleRate"), "44100");
    assert_eq!(tr1.attr("Rating"), "204");
    assert_eq!(tr1.attr("Tonality"), "Am");
    assert_eq!(tr1.attr("Year"), "2021");
    assert_eq!(
        tr1.attr("Location"),
        "file://localhost/C:/Music/Caf%C3%A9%20%26%20Bar/One%20%3CEdit%3E.mp3"
    );
    assert_eq!(tr1.attr("DateAdded").len(), 10);

    let tempo = tr1.child("TEMPO");
    assert_eq!(tempo.attr("Inizio"), "0.232");
    assert_eq!(tempo.attr("Bpm"), "128.00");
    assert_eq!(tempo.attr("Metro"), "4/4");
    assert_eq!(tempo.attr("Battito"), "1");

    let marks = tr1.all("POSITION_MARK");
    assert_eq!(marks.len(), 4);
    let memory = marks.iter().find(|m| m.attr("Num") == "-1").unwrap();
    assert_eq!((memory.attr("Type"), memory.attr("Start")), ("0", "0.232"));
    let hot_a = marks.iter().find(|m| m.attr("Num") == "0").unwrap();
    assert_eq!(hot_a.attr("Name"), "Drop");
    assert_eq!(
        (hot_a.attr("Red"), hot_a.attr("Green"), hot_a.attr("Blue")),
        ("40", "226", "20")
    );
    let hot_h = marks.iter().find(|m| m.attr("Num") == "7").unwrap();
    assert!(
        !hot_h.attrs.contains_key("Red"),
        "no colour when none is set"
    );
    let lp = marks.iter().find(|m| m.attr("Type") == "4").unwrap();
    assert_eq!(
        (lp.attr("Start"), lp.attr("End"), lp.attr("Num")),
        ("45.000", "46.875", "2")
    );
    // A track without a grid has no TEMPO element.
    let tr2 = tracks
        .iter()
        .find(|t| t.attr("TrackID") == t2.to_string())
        .unwrap();
    assert!(tr2.all("TEMPO").is_empty());
    assert_eq!(tr2.attr("AverageBpm"), "0.00");

    // Playlists.
    let root_node = root.child("PLAYLISTS").child("NODE");
    assert_eq!(
        (root_node.attr("Type"), root_node.attr("Name")),
        ("0", "ROOT")
    );
    assert_eq!(root_node.attr("Count"), "2");
    let sets = &root_node.children[0];
    assert_eq!((sets.attr("Type"), sets.attr("Name")), ("0", "Sets"));
    assert_eq!(sets.attr("Count"), "3");
    let names: Vec<&str> = sets.children.iter().map(|c| c.attr("Name")).collect();
    assert_eq!(names, ["Sets", "Friday", "Saturday"]);
    let friday = &sets.children[1];
    assert_eq!(
        (
            friday.attr("Type"),
            friday.attr("KeyType"),
            friday.attr("Entries")
        ),
        ("1", "0", "2")
    );
    let keys: Vec<&str> = friday.all("TRACK").iter().map(|t| t.attr("Key")).collect();
    assert_eq!(keys, [t2.to_string(), t1.to_string()]);
    let warm = &root_node.children[1];
    assert_eq!((warm.attr("Type"), warm.attr("Entries")), ("1", "0"));

    assert_eq!(
        report,
        dj_export::ExportReport {
            tracks: 2,
            playlists: 4,
            folders: 1,
            cues: 3,
            loops: 1,
            missing_files: 0,
        }
    );
}

#[test]
fn export_one_crate_or_everything() {
    let (lib, [_t1, _t2, t3], [_sets, fri, _sat, _warm]) = library();
    let mut buf = Vec::new();
    let r = export_xml(
        &lib,
        &XmlExportOptions {
            crates: vec![fri],
            all_tracks: false,
            loops_to_free_pads: false,
            precise_tempo: false,
        },
        &mut buf,
    )
    .unwrap();
    assert_eq!((r.tracks, r.playlists, r.folders), (2, 1, 0));
    let root = parse(&String::from_utf8(buf).unwrap());
    let n = root.child("PLAYLISTS").child("NODE");
    assert_eq!(n.attr("Count"), "1");
    assert_eq!(n.children[0].attr("Name"), "Friday");

    let mut buf = Vec::new();
    let r = export_xml(
        &lib,
        &XmlExportOptions {
            crates: vec![],
            all_tracks: true,
            loops_to_free_pads: false,
            precise_tempo: false,
        },
        &mut buf,
    )
    .unwrap();
    assert_eq!(r.tracks, 3);
    let root = parse(&String::from_utf8(buf).unwrap());
    assert!(
        root.child("COLLECTION")
            .all("TRACK")
            .iter()
            .any(|t| t.attr("TrackID") == t3.to_string())
    );
}

/// Kaldor → rekordbox XML → "Import from…" into an empty library: the same
/// tracks, cues, loop, grid and crates come back.
#[test]
fn round_trip_through_import() {
    use dj_library::interop::{InteropOptions, rekordbox};
    let (lib, [t1, t2, _t3], _) = library();
    let mut buf = Vec::new();
    export_xml(
        &lib,
        &XmlExportOptions {
            precise_tempo: true,
            ..XmlExportOptions::default()
        },
        &mut buf,
    )
    .unwrap();
    let foreign = rekordbox::read(&String::from_utf8(buf).unwrap()).unwrap();
    assert!(foreign.warnings.is_empty(), "{:?}", foreign.warnings);

    let mut back = Library::open_in_memory().unwrap();
    let opts = InteropOptions {
        root_crate: Some("rekordbox".into()),
        ..InteropOptions::default()
    };
    let r = back.import_foreign(&foreign, &[], &opts).unwrap();
    assert_eq!((r.tracks_added, r.missing_files), (2, 2));
    assert_eq!(back.track_count().unwrap(), 2);

    let orig1 = lib.track(t1).unwrap().unwrap();
    let b1 = back.track_by_path(&orig1.meta.path).unwrap().unwrap();
    assert_eq!(b1.meta.title, orig1.meta.title);
    assert_eq!(b1.meta.artist, orig1.meta.artist);
    assert_eq!(b1.meta.album, orig1.meta.album);
    assert_eq!(b1.meta.genre, orig1.meta.genre);
    assert_eq!(b1.meta.year, Some(2021));
    assert_eq!(b1.rating, 4);

    let g = back.beatgrid(b1.id).unwrap().unwrap();
    let og = lib.beatgrid(t1).unwrap().unwrap();
    assert!((g.bpm - og.bpm).abs() < 1e-6, "{} vs {}", g.bpm, og.bpm);
    assert!((g.first_beat - og.first_beat).abs() < 1e-6);
    assert_eq!(g.first_beat_in_bar, og.first_beat_in_bar);

    // Cues: positions to rekordbox's millisecond; the main cue comes back
    // as a memory cue (rekordbox XML has no main cue of its own).
    let cues = back.cues(b1.id).unwrap();
    assert_eq!(cues.len(), 3);
    let find = |k: CueKind| cues.iter().find(|c| c.kind == k).unwrap();
    assert!((find(CueKind::Memory).position - 0.2316).abs() < 1e-3);
    let drop = find(CueKind::Hot(0));
    assert!((drop.position - 30.25).abs() < 1e-3);
    assert_eq!((drop.name.as_str(), drop.color), ("Drop", Some(0x28E214)));
    assert_eq!(find(CueKind::Hot(7)).name, "Out");

    let loops = back.loops(b1.id).unwrap();
    assert_eq!(loops.len(), 1);
    let l = &loops[0];
    assert_eq!(
        (l.slot, l.name.as_str(), l.color),
        (2, "4 beats", Some(0xFF8800))
    );
    assert!((l.start - 45.0).abs() < 1e-3 && (l.end - 46.875).abs() < 1e-3);

    // Crates: rekordbox > Sets > Sets, Friday, Saturday; Warm-up.
    let b2 = back
        .track_by_path(&lib.track(t2).unwrap().unwrap().meta.path)
        .unwrap()
        .unwrap();
    let tree = back.crate_tree().unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].name, "rekordbox");
    let top: Vec<&str> = tree[0].children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(top, ["Sets", "Warm-up"]);
    let sets = &tree[0].children[0];
    let subs: Vec<&str> = sets.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(subs, ["Sets", "Friday", "Saturday"]);
    assert!(back.crate_tracks(sets.id).unwrap().is_empty(), "a folder");
    assert_eq!(back.crate_tracks(sets.children[0].id).unwrap(), vec![b2.id]);
    assert_eq!(
        back.crate_tracks(sets.children[1].id).unwrap(),
        vec![b2.id, b1.id]
    );
    assert_eq!(back.crate_tracks(sets.children[2].id).unwrap(), vec![b1.id]);
    assert!(
        back.crate_tracks(tree[0].children[1].id)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn empty_library_is_valid() {
    let lib = Library::open_in_memory().unwrap();
    let mut buf = Vec::new();
    export_xml(&lib, &XmlExportOptions::default(), &mut buf).unwrap();
    let root = parse(&String::from_utf8(buf).unwrap());
    assert_eq!(root.child("COLLECTION").attr("Entries"), "0");
    assert_eq!(root.child("PLAYLISTS").child("NODE").attr("Count"), "0");
}
