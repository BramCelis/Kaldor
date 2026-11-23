//! "Import from…" merged into a library: nothing done in Kaldor is lost,
//! missing files still come in, importing twice adds nothing twice.

use dj_core::Beatgrid;
use dj_library::interop::{ForeignLibrary, ForeignNode, ForeignTrack, InteropOptions};
use dj_library::{Cue, CueKind, Library, SavedLoop, TrackMeta};
use std::path::PathBuf;

fn cue(kind: CueKind, at: f64) -> Cue {
    Cue {
        kind,
        position: at,
        name: String::new(),
        color: None,
    }
}

fn foreign() -> ForeignLibrary {
    let mut grid = Beatgrid::new(128.0, 0.05);
    grid.number_bars_from_start();
    ForeignLibrary {
        source: "rekordbox".into(),
        tracks: vec![
            ForeignTrack {
                path: PathBuf::from("Z:\\kaldor-test\\known.mp3"),
                title: "Known".into(),
                rating: 5,
                color: Some(0x00FF00),
                play_count: 40,
                date_added: Some(0),
                grid: Some(grid),
                cues: vec![cue(CueKind::Hot(3), 10.0)],
                ..ForeignTrack::default()
            },
            ForeignTrack {
                path: PathBuf::from("Z:\\kaldor-test\\missing.mp3"),
                title: "Elsewhere".into(),
                artist: "Someone".into(),
                duration: 200.0,
                bpm: Some(126.0),
                grid: Some(grid),
                cues: vec![cue(CueKind::Hot(0), 1.0), cue(CueKind::Memory, 2.0)],
                loops: vec![SavedLoop {
                    slot: 1,
                    start: 4.0,
                    end: 6.0,
                    name: "L".into(),
                    color: None,
                }],
                ..ForeignTrack::default()
            },
        ],
        playlists: vec![ForeignNode::Folder {
            name: "Gigs".into(),
            children: vec![ForeignNode::Playlist {
                name: "Friday".into(),
                tracks: vec![1, 0],
            }],
        }],
        warnings: vec![],
    }
}

#[test]
fn import_keeps_kaldor_work_adds_missing_files_and_is_repeatable() {
    let mut lib = Library::open_in_memory().unwrap();
    // A track already in Kaldor, with its own cue, an edited grid, a rating.
    let (known, _) = lib
        .upsert_track(&TrackMeta {
            path: PathBuf::from("Z:\\kaldor-test\\known.mp3"),
            title: "Known (Kaldor tags)".into(),
            ..TrackMeta::default()
        })
        .unwrap();
    lib.set_cues(known, &[cue(CueKind::Hot(0), 5.0)]).unwrap();
    lib.set_beatgrid(known, &Beatgrid::new(124.0, 0.1)).unwrap();
    lib.set_rating(known, 2).unwrap();

    let f = foreign();
    let opts = InteropOptions {
        root_crate: Some("From rekordbox".into()),
        ..InteropOptions::default()
    };
    let r = lib.import_foreign(&f, &[], &opts).unwrap();
    assert_eq!(
        (r.tracks_added, r.tracks_matched, r.missing_files),
        (1, 1, 1)
    );

    // The known track keeps its tags, cue, grid and rating; gets the colour,
    // the higher play count and the earlier date.
    let t = lib.track(known).unwrap().unwrap();
    assert_eq!(t.meta.title, "Known (Kaldor tags)");
    assert_eq!(t.rating, 2);
    assert_eq!(t.color, Some(0x00FF00));
    assert_eq!(t.play_count, 40);
    assert_eq!(t.date_added, 0);
    assert_eq!(lib.cues(known).unwrap(), vec![cue(CueKind::Hot(0), 5.0)]);
    assert!((lib.beatgrid(known).unwrap().unwrap().bpm - 124.0).abs() < 1e-9);

    // The missing file came in, marked missing, with its cues, loop, grid.
    let other = lib
        .track_by_path(&PathBuf::from("Z:\\kaldor-test\\missing.mp3"))
        .unwrap()
        .unwrap();
    assert!(other.missing);
    assert_eq!(
        (other.meta.title.as_str(), other.meta.artist.as_str()),
        ("Elsewhere", "Someone")
    );
    assert_eq!(other.bpm, Some(128.0), "the imported grid's tempo");
    assert_eq!(lib.cues(other.id).unwrap().len(), 2);
    assert_eq!(lib.loops(other.id).unwrap().len(), 1);
    assert_eq!((r.cues, r.loops, r.grids), (2, 1, 1));

    // Crates: From rekordbox > Gigs > Friday, in the playlist's order.
    let tree = lib.crate_tree().unwrap();
    assert_eq!(tree.len(), 1);
    assert_eq!(tree[0].name, "From rekordbox");
    let gigs = &tree[0].children[0];
    assert_eq!(gigs.name, "Gigs");
    let friday = gigs.children[0].id;
    assert_eq!(lib.crate_tracks(friday).unwrap(), vec![other.id, known]);
    assert_eq!((r.crates, r.crate_entries), (3, 2));

    // Again: nothing new.
    let again = lib.import_foreign(&f, &[], &opts).unwrap();
    assert_eq!((again.tracks_added, again.tracks_matched), (0, 2));
    assert_eq!((again.crates, again.crate_entries), (0, 0));
    assert_eq!(lib.crate_tree().unwrap().len(), 1);
    assert_eq!(lib.crate_tracks(friday).unwrap().len(), 2);
    assert_eq!(lib.track_count().unwrap(), 2);
}

#[test]
fn replace_options_take_the_imported_cues_and_grid() {
    let mut lib = Library::open_in_memory().unwrap();
    let (known, _) = lib
        .upsert_track(&TrackMeta {
            path: PathBuf::from("Z:\\kaldor-test\\known.mp3"),
            ..TrackMeta::default()
        })
        .unwrap();
    lib.set_cues(known, &[cue(CueKind::Hot(0), 5.0)]).unwrap();
    lib.set_beatgrid(known, &Beatgrid::new(124.0, 0.1)).unwrap();
    let opts = InteropOptions {
        replace_cues: true,
        replace_grids: true,
        ..InteropOptions::default()
    };
    lib.import_foreign(&foreign(), &[], &opts).unwrap();
    assert_eq!(lib.cues(known).unwrap(), vec![cue(CueKind::Hot(3), 10.0)]);
    assert!((lib.beatgrid(known).unwrap().unwrap().bpm - 128.0).abs() < 1e-9);
    // No root crate: the folder at the top level.
    assert_eq!(lib.crate_tree().unwrap()[0].name, "Gigs");
}
