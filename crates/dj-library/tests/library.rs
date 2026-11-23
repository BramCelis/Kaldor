//! Library tests. Files are written under cargo's target directory only.

use dj_analysis::synth::ClickTrack;
use dj_analysis::{AnalysisOptions, analyze_audio};
use dj_core::{Beatgrid, MusicalKey};
use dj_decode::TrackAudio;
use dj_library::search::{Query, SearchIndex};
use dj_library::{
    AnalysisJob, AnalysisQueue, Cue, CueKind, Library, LibraryError, SavedLoop, Track, TrackMeta,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("dj-library")
        .join(name);
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn meta(path: &str, artist: &str, title: &str) -> TrackMeta {
    TrackMeta {
        path: PathBuf::from(path),
        artist: artist.into(),
        title: title.into(),
        duration: 200.0,
        ..TrackMeta::default()
    }
}

fn write_wav(path: &Path, samples: &[f32], sr: u32, channels: u16) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: sr,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for s in samples {
        w.write_sample((s.clamp(-1.0, 1.0) * 32_000.0) as i16)
            .unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn migrations_create_and_reopen() {
    let dir = tmp("migrate");
    let db = dir.join("lib.db");
    {
        let mut lib = Library::open(&db).unwrap();
        assert_eq!(lib.schema_version().unwrap(), dj_library::schema_version());
        lib.upsert_track(&meta("/a.mp3", "A", "One")).unwrap();
    }
    let lib = Library::open(&db).unwrap();
    assert_eq!(lib.track_count().unwrap(), 1);
    drop(lib);
    // A database from a newer app version is refused, not modified.
    {
        let c = rusqlite::Connection::open(&db).unwrap();
        c.pragma_update(None, "user_version", 999).unwrap();
    }
    assert!(Library::open(&db).is_err());
}

#[test]
fn tracks_analysis_cues_and_loops() {
    let mut lib = Library::open_in_memory().unwrap();
    let (id, added) = lib
        .upsert_track(&meta("/m/one.mp3", "Artist", "One"))
        .unwrap();
    assert!(added);
    let (id2, added2) = lib
        .upsert_track(&meta("/m/one.mp3", "Artist", "One (edit)"))
        .unwrap();
    assert_eq!((id2, added2), (id, false));
    assert_eq!(lib.track(id).unwrap().unwrap().meta.title, "One (edit)");
    assert_eq!(lib.tracks_needing_analysis().unwrap().len(), 1);

    // Analysis results.
    let click = ClickTrack {
        sample_rate: 44_100,
        ..ClickTrack::new(125.0, 20.0)
    };
    let a = analyze_audio(
        &TrackAudio::from_interleaved(44_100, &click.render()),
        &AnalysisOptions::default(),
    );
    lib.store_analysis(id, &a).unwrap();
    let t: Track = lib.track(id).unwrap().unwrap();
    assert!((t.bpm.unwrap() - 125.0).abs() < 0.01);
    assert!(t.lufs.is_some() && t.gain_db.is_some());
    assert_eq!(t.analysis_version, Some(dj_analysis::ANALYSIS_VERSION));
    assert!(lib.tracks_needing_analysis().unwrap().is_empty());
    let g = lib.beatgrid(id).unwrap().unwrap();
    assert!((g.bpm - 125.0).abs() < 0.01);
    let wf = lib.waveform(id).unwrap().unwrap();
    assert_eq!(wf.levels[0], a.waveform.levels[0]);

    // A locked grid survives re-analysis.
    let mut locked = Beatgrid::new(124.0, 0.5);
    locked.locked = true;
    lib.set_beatgrid(id, &locked).unwrap();
    lib.store_analysis(id, &a).unwrap();
    assert_eq!(lib.beatgrid(id).unwrap().unwrap(), locked);
    assert_eq!(lib.track(id).unwrap().unwrap().bpm, Some(124.0));

    // An edited (not locked) grid survives re-analysis too, and stays
    // editable; an explicit re-analysis (forget_grid_edit) replaces it.
    let edited = Beatgrid::new(124.5, 0.3);
    lib.set_beatgrid(id, &edited).unwrap();
    lib.store_analysis(id, &a).unwrap();
    let g = lib.beatgrid(id).unwrap().unwrap();
    assert_eq!(g, edited);
    assert!(!g.locked, "an edited grid is not locked");
    lib.forget_grid_edit(id).unwrap();
    lib.store_analysis(id, &a).unwrap();
    assert!((lib.beatgrid(id).unwrap().unwrap().bpm - 125.0).abs() < 0.01);
    // forget_grid_edit leaves a locked grid alone.
    lib.set_beatgrid(id, &locked).unwrap();
    lib.forget_grid_edit(id).unwrap();
    lib.store_analysis(id, &a).unwrap();
    assert_eq!(lib.beatgrid(id).unwrap().unwrap(), locked);

    let cues = vec![
        Cue {
            kind: CueKind::Main,
            position: 0.5,
            name: String::new(),
            color: None,
        },
        Cue {
            kind: CueKind::Hot(3),
            position: 32.0,
            name: "Drop".into(),
            color: Some(0xFF0000),
        },
        Cue {
            kind: CueKind::Memory,
            position: 64.0,
            name: "Break".into(),
            color: None,
        },
    ];
    lib.set_cues(id, &cues).unwrap();
    assert_eq!(lib.cues(id).unwrap(), cues);
    let loops = vec![SavedLoop {
        slot: 1,
        start: 10.0,
        end: 12.0,
        name: "4 beats".into(),
        color: Some(0x00FF00),
    }];
    lib.set_loops(id, &loops).unwrap();
    assert_eq!(lib.loops(id).unwrap(), loops);

    lib.set_rating(id, 9).unwrap();
    lib.record_play(id).unwrap();
    let t = lib.track(id).unwrap().unwrap();
    assert_eq!((t.rating, t.play_count), (5, 1));

    // Deleting the track removes its cues (foreign keys are on).
    lib.remove_track(id).unwrap();
    assert!(lib.cues(id).unwrap().is_empty());
}

#[test]
fn tag_bpm_and_key_are_used_until_analysis() {
    let mut lib = Library::open_in_memory().unwrap();
    let mut m = meta("/x.mp3", "A", "B");
    m.tag_bpm = Some(122.0);
    m.tag_key = MusicalKey::parse("Am");
    let (id, _) = lib.upsert_track(&m).unwrap();
    let t = lib.track(id).unwrap().unwrap();
    assert_eq!(t.bpm, Some(122.0));
    assert_eq!(t.key, Some(MusicalKey::new(9, true)));
}

#[test]
fn nested_crates_ordering_and_membership() {
    let mut lib = Library::open_in_memory().unwrap();
    let t: Vec<i64> = (0..5)
        .map(|i| {
            lib.upsert_track(&meta(&format!("/t{i}.mp3"), "A", &format!("T{i}")))
                .unwrap()
                .0
        })
        .collect();
    let house = lib.create_crate("House", None).unwrap();
    let techno = lib.create_crate("Techno", None).unwrap();
    let deep = lib.create_crate("Deep", Some(house)).unwrap();
    let tech = lib.create_crate("Tech", Some(house)).unwrap();

    let tree = lib.crate_tree().unwrap();
    assert_eq!(tree.len(), 2);
    assert_eq!(tree[0].name, "House");
    let kids: Vec<&str> = tree[0].children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(kids, ["Deep", "Tech"]);

    // Reorder siblings, move between parents, refuse cycles.
    lib.move_crate(tech, Some(house), 0).unwrap();
    let kids: Vec<String> = lib.crate_tree().unwrap()[0]
        .children
        .iter()
        .map(|c| c.name.clone())
        .collect();
    assert_eq!(kids, ["Tech", "Deep"]);
    lib.move_crate(deep, Some(techno), 0).unwrap();
    assert_eq!(lib.crate_tree().unwrap()[1].children[0].name, "Deep");
    assert!(matches!(
        lib.move_crate(house, Some(tech), 0),
        Err(LibraryError::CrateCycle)
    ));
    lib.move_crate(techno, None, 0).unwrap();
    assert_eq!(lib.crate_tree().unwrap()[0].name, "Techno");
    lib.rename_crate(techno, "Techno 2026").unwrap();

    // Membership and order.
    lib.add_to_crate(deep, &[t[2], t[0], t[1]]).unwrap();
    lib.add_to_crate(deep, &[t[0], t[3]]).unwrap(); // t0 already there
    assert_eq!(
        lib.crate_tracks(deep).unwrap(),
        vec![t[2], t[0], t[1], t[3]]
    );
    lib.move_in_crate(deep, t[3], 0).unwrap();
    assert_eq!(
        lib.crate_tracks(deep).unwrap(),
        vec![t[3], t[2], t[0], t[1]]
    );
    lib.remove_from_crate(deep, &[t[2]]).unwrap();
    assert_eq!(lib.crate_tracks(deep).unwrap(), vec![t[3], t[0], t[1]]);

    // Deleting a parent deletes sub-crates; tracks stay in the library.
    lib.delete_crate(techno).unwrap();
    assert!(lib.crate_tracks(deep).unwrap().is_empty());
    assert_eq!(lib.track_count().unwrap(), 5);
    assert!(matches!(
        lib.create_crate("x", Some(9999)),
        Err(LibraryError::NoSuchCrate(9999))
    ));
}

#[test]
fn search_text_bpm_and_key() {
    let mut lib = Library::open_in_memory().unwrap();
    fn add(lib: &mut Library, p: &str, a: &str, t: &str, bpm: f64, key: &str) -> i64 {
        let mut m = meta(p, a, t);
        m.tag_bpm = Some(bpm);
        m.tag_key = MusicalKey::parse(key);
        lib.upsert_track(&m).unwrap().0
    }
    let a = add(
        &mut lib,
        "/1.mp3",
        "Daft Punk",
        "Around the World",
        121.0,
        "8A",
    );
    let b = add(&mut lib, "/2.mp3", "Daftly", "Punked Up", 128.0, "9A");
    let c = add(&mut lib, "/3.mp3", "Other", "Song", 128.2, "8A");
    assert_eq!(lib.search("daft punk").unwrap(), vec![a, b]);
    assert_eq!(lib.search("bpm:128").unwrap(), vec![b, c]);
    assert_eq!(lib.search("bpm:120-125").unwrap(), vec![a]);
    assert_eq!(lib.search("key:Am").unwrap(), vec![a, c]);
    assert_eq!(lib.search("key:8A bpm:126-130").unwrap(), vec![c]);
    assert_eq!(lib.search("").unwrap(), vec![a, b, c]);
    assert!(lib.search("nothing-matches").unwrap().is_empty());
    // The index is rebuilt after changes.
    let d = add(&mut lib, "/4.mp3", "Daft Punk", "One More Time", 123.0, "");
    assert_eq!(lib.search("daft punk").unwrap(), vec![a, b, d]);
}

#[test]
fn folder_import_is_incremental() {
    let dir = tmp("import");
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    let s = dj_analysis::synth::tones(44_100, &[440.0], 0.5, 0.3);
    write_wav(&dir.join("a.wav"), &s, 44_100, 2);
    write_wav(&dir.join("sub").join("b.WAV"), &s, 44_100, 2);
    std::fs::write(dir.join("cover.jpg"), b"not audio").unwrap();

    let mut lib = Library::open_in_memory().unwrap();
    let flat = lib.import_folder(&dir, false, |_, _| {}).unwrap();
    assert_eq!((flat.scanned, flat.added), (1, 1));
    let r = lib.import_folder(&dir, true, |_, _| {}).unwrap();
    assert_eq!((r.scanned, r.added, r.unchanged), (2, 1, 1));
    let t = lib.track_by_path(&dir.join("a.wav")).unwrap().unwrap();
    assert!((t.meta.duration - 0.5).abs() < 0.01, "{}", t.meta.duration);
    assert_eq!(t.meta.sample_rate, Some(44_100));
    assert_eq!(t.display_name(), "a");

    // Missing-file detection.
    std::fs::remove_file(dir.join("a.wav")).unwrap();
    assert_eq!(lib.check_missing().unwrap(), 1);
    assert!(
        lib.track_by_path(&dir.join("a.wav"))
            .unwrap()
            .unwrap()
            .missing
    );
}

#[test]
fn import_5000_files() {
    let dir = tmp("import5000");
    // Tiny valid WAV files: 20 ms of 8 kHz mono silence.
    let s = vec![0.0f32; 160];
    for i in 0..5000 {
        let sub = dir.join(format!("folder{:02}", i / 100));
        if i % 100 == 0 {
            std::fs::create_dir_all(&sub).unwrap();
        }
        write_wav(&sub.join(format!("track{i:05}.wav")), &s, 8_000, 1);
    }
    let mut lib = Library::open(&dir.join("lib.db")).unwrap();
    let start = Instant::now();
    let mut calls = 0;
    let r = lib.import_folder(&dir, true, |_, _| calls += 1).unwrap();
    let took = start.elapsed();
    println!("imported 5000 files in {:.2} s", took.as_secs_f64());
    assert_eq!(r.added, 5000);
    assert_eq!(calls, 10);
    assert_eq!(lib.track_count().unwrap(), 5000);
    assert_eq!(lib.search("track04321").unwrap().len(), 1);
    // A second import changes nothing.
    let r2 = lib.import_folder(&dir, true, |_, _| {}).unwrap();
    assert_eq!(r2.unchanged, 5000);
}

#[test]
fn analysis_queue_end_to_end() {
    let dir = tmp("queue");
    let mut lib = Library::open_in_memory().unwrap();
    for (i, bpm) in [118.0, 126.0, 140.0].iter().enumerate() {
        let c = ClickTrack {
            sample_rate: 44_100,
            seed: i as u64 + 1,
            noise: 0.03,
            ..ClickTrack::new(*bpm, 20.0)
        };
        write_wav(&dir.join(format!("{bpm}.wav")), &c.render(), 44_100, 2);
    }
    lib.import_folder(&dir, false, |_, _| {}).unwrap();
    let queue = AnalysisQueue::new(2, AnalysisOptions::default());
    for (track, path) in lib.tracks_needing_analysis().unwrap() {
        queue.submit(AnalysisJob { track, path });
    }
    assert_eq!(queue.pending(), 3);
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut done = 0;
    while done < 3 && Instant::now() < deadline {
        if let Some(d) = queue.recv_timeout(Duration::from_millis(200)) {
            lib.store_analysis(d.track, &d.result.unwrap()).unwrap();
            done += 1;
        }
    }
    assert_eq!(done, 3);
    for t in lib.tracks().unwrap() {
        let want: f64 = t
            .meta
            .path
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .parse()
            .unwrap();
        assert!((t.bpm.unwrap() - want).abs() < 0.01, "{:?}", t.bpm);
    }
    assert!(lib.tracks_needing_analysis().unwrap().is_empty());
}

#[test]
fn backup_writes_a_working_copy() {
    let dir = tmp("backup");
    let mut lib = Library::open(&dir.join("lib.db")).unwrap();
    lib.upsert_track(&meta("/x.mp3", "A", "B")).unwrap();
    let copy = dir.join("backup.db");
    lib.backup_to(&copy).unwrap();
    let restored = Library::open(&copy).unwrap();
    assert_eq!(restored.track_count().unwrap(), 1);
}

#[test]
fn backup_file_copies_an_open_library_from_another_connection() {
    let dir = tmp("backup_file");
    let src = dir.join("lib.db");
    let mut lib = Library::open(&src).unwrap();
    lib.upsert_track(&meta("/x.mp3", "A", "B")).unwrap();
    lib.upsert_track(&meta("/y.mp3", "C", "D")).unwrap();
    let copy = dir.join("backups").join("copy.db");
    // On another thread, while `lib` stays open.
    let (s, c) = (src.clone(), copy.clone());
    std::thread::spawn(move || Library::backup_file(&s, &c))
        .join()
        .unwrap()
        .unwrap();
    assert_eq!(Library::open(&copy).unwrap().track_count().unwrap(), 2);
    drop(lib);
}

fn synthetic_tracks(n: usize) -> Vec<Track> {
    let words = [
        "deep", "night", "acid", "love", "sun", "bass", "dream", "city", "fire", "ocean", "drive",
        "soul",
    ];
    (0..n)
        .map(|i| Track {
            id: i as i64,
            meta: TrackMeta {
                path: PathBuf::from(format!("/music/{i}.mp3")),
                artist: format!("Artist {} {}", words[i % 12], i % 997),
                title: format!("{} {} {}", words[(i / 12) % 12], words[(i / 7) % 12], i),
                album: format!("Album {}", i % 503),
                genre: ["House", "Techno", "Drum & Bass", "Disco"][i % 4].to_string(),
                comment: String::new(),
                ..TrackMeta::default()
            },
            bpm: Some(80.0 + (i % 100) as f64),
            key: Some(MusicalKey::from_index((i % 24) as u8)),
            lufs: None,
            gain_db: None,
            rating: 0,
            color: None,
            play_count: 0,
            date_added: 0,
            missing: false,
            analysis_version: None,
        })
        .collect()
}

#[test]
fn search_100k_correctness() {
    let idx = SearchIndex::build(&synthetic_tracks(100_000));
    assert_eq!(idx.len(), 100_000);
    let r = idx.search(&Query::parse("acid techno bpm:120-125"));
    assert!(!r.is_empty());
    assert!(r.iter().all(|id| {
        let i = *id as usize;
        i % 4 == 1 && (40..=45).contains(&(i % 100))
    }));
}

/// Spec target: under 50 ms on 100,000 tracks. Timing is only meaningful in
/// release builds: `cargo test --release -p dj-library -- --ignored`.
#[test]
#[ignore = "timing; run in release"]
fn search_100k_timing() {
    let idx = SearchIndex::build(&synthetic_tracks(100_000));
    for q in [
        "deep night",
        "artist love 42",
        "bpm:124-126 key:8A",
        "house",
        "zzzz",
    ] {
        let q = Query::parse(q);
        let t = Instant::now();
        let n = idx.search(&q).len();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!("{q:?}: {n} hits in {ms:.2} ms");
        assert!(ms < 50.0);
    }
}

#[test]
fn removing_tracks_takes_them_out_of_every_crate_and_clear_empties_all() {
    let mut lib = Library::open_in_memory().unwrap();
    let a = lib.upsert_track(&meta("/a.mp3", "A", "a")).unwrap().0;
    let b = lib.upsert_track(&meta("/b.mp3", "B", "b")).unwrap().0;
    let c = lib.upsert_track(&meta("/c.mp3", "C", "c")).unwrap().0;
    let warm = lib.create_crate("Warm-up", None).unwrap();
    let peak = lib.create_crate("Peak", None).unwrap();
    lib.add_to_crate(warm, &[a, b, c]).unwrap();
    lib.add_to_crate(peak, &[b]).unwrap();
    lib.remove_tracks(&[b, c]).unwrap();
    assert_eq!(lib.crate_tracks(warm).unwrap(), vec![a]);
    assert!(lib.crate_tracks(peak).unwrap().is_empty());
    assert_eq!(lib.tracks().unwrap().len(), 1);
    assert_eq!(lib.search("b").unwrap(), Vec::<i64>::new());

    lib.clear().unwrap();
    assert!(lib.tracks().unwrap().is_empty());
    assert!(lib.crate_tree().unwrap().is_empty());
    // Usable afterwards.
    let d = lib.upsert_track(&meta("/d.mp3", "D", "d")).unwrap().0;
    assert_eq!(lib.search("d").unwrap(), vec![d]);
}

/// A smart crate lists the tracks matching its rules, live (a track edited
/// or added later shows up), and refuses tracks added by hand.
#[test]
fn smart_crates_fill_themselves() {
    use dj_library::smart::{Field, Op, Rule, SmartRules};
    let mut lib = Library::open_in_memory().unwrap();
    let mut m = meta("Z:/s/a.mp3", "A", "One");
    m.genre = "Deep House".into();
    let (a, _) = lib.upsert_track(&m).unwrap();
    let mut m = meta("Z:/s/b.mp3", "B", "Two");
    m.genre = "Techno".into();
    let (b, _) = lib.upsert_track(&m).unwrap();
    let rules = SmartRules {
        match_all: true,
        rules: vec![Rule {
            field: Field::Genre,
            op: Op::Contains,
            value: "house".into(),
        }],
    };
    let c = lib.create_smart_crate("House", None, &rules).unwrap();
    assert_eq!(lib.crate_tracks(c).unwrap(), vec![a]);
    assert!(lib.crate_tree().unwrap()[0].smart);
    assert_eq!(lib.smart_rules(c).unwrap(), Some(rules.clone()));

    // A new house track comes in by itself.
    let mut m = meta("Z:/s/c.mp3", "C", "Three");
    m.genre = "House".into();
    let (c3, _) = lib.upsert_track(&m).unwrap();
    assert_eq!(lib.crate_tracks(c).unwrap(), vec![a, c3]);

    // Rules changed: any of two.
    let mut r2 = rules.clone();
    r2.match_all = false;
    r2.rules.push(Rule {
        field: Field::Artist,
        op: Op::Is,
        value: "b".into(),
    });
    lib.set_smart_rules(c, &r2).unwrap();
    assert_eq!(lib.crate_tracks(c).unwrap(), vec![a, b, c3]);

    // Hand edits are refused; a normal crate is not smart.
    assert!(matches!(
        lib.add_to_crate(c, &[b]),
        Err(LibraryError::SmartCrate)
    ));
    assert!(lib.remove_from_crate(c, &[a]).is_err());
    let plain = lib.create_crate("Plain", None).unwrap();
    assert_eq!(lib.smart_rules(plain).unwrap(), None);
    lib.add_to_crate(plain, &[b]).unwrap();
    assert_eq!(lib.crate_tracks(plain).unwrap(), vec![b]);
}

/// The history: sessions with their plays in order; a play survives its
/// track leaving the library; older CSV sessions come in.
#[test]
fn history_sessions_and_plays() {
    use dj_library::history;
    let mut lib = Library::open_in_memory().unwrap();
    let (a, _) = lib.upsert_track(&meta("Z:/h/a.mp3", "A", "One")).unwrap();
    let (b, _) = lib.upsert_track(&meta("Z:/h/b.mp3", "B", "Two")).unwrap();
    let s1 = lib.start_session().unwrap();
    lib.log_play_at(s1, a, 1, 1_000).unwrap();
    lib.log_play_at(s1, b, 2, 1_300).unwrap();
    let s2 = lib.start_session().unwrap();
    assert_eq!(
        lib.sessions().unwrap().len(),
        1,
        "an empty session is not listed"
    );
    lib.log_play_at(s2, b, 3, 9_000).unwrap();

    let sessions = lib.sessions().unwrap();
    assert_eq!(sessions.len(), 2);
    let first = sessions.iter().find(|s| s.id == s1).unwrap();
    assert_eq!((first.plays, first.ended), (2, 1_300));
    let plays = lib.session_plays(s1).unwrap();
    assert_eq!(
        plays.iter().map(|p| (p.track, p.deck)).collect::<Vec<_>>(),
        vec![(Some(a), 1), (Some(b), 2)]
    );
    assert_eq!(plays[0].display(), "A - One");
    let entries = history::to_entries(&plays);
    assert_eq!(entries[1].path, PathBuf::from("Z:/h/b.mp3"));
    assert!(history::to_csv(&plays, 0).lines().count() == 3);

    // Track A leaves the library: its play stays, without the link.
    lib.remove_tracks(&[a]).unwrap();
    let plays = lib.session_plays(s1).unwrap();
    assert_eq!(plays[0].track, None);
    assert_eq!(plays[0].title, "One");

    lib.delete_session(s2).unwrap();
    assert_eq!(lib.sessions().unwrap().len(), 1);

    // An older CSV session (local time one hour ahead of UTC).
    let csv = "played_at,artist,title,bpm,key,path\n\
               2026-11-15 21:30:00,B,\"Two, live\",128.00,8A,Z:/h/b.mp3\n\
               2026-11-15 21:34:00,X,Gone,,,Z:/h/gone.mp3\n";
    assert_eq!(lib.import_history_csv(csv, 3600).unwrap(), 2);
    let newest = &lib.sessions().unwrap()[0];
    assert_eq!(newest.plays, 2);
    let plays = lib.session_plays(newest.id).unwrap();
    assert_eq!(plays[0].track, Some(b));
    assert_eq!(plays[0].title, "Two, live");
    assert_eq!(plays[0].key, MusicalKey::parse("8A"));
    assert_eq!(plays[1].track, None);
    assert_eq!(
        history::format_time(plays[0].played_at, 3600),
        "2026-11-15 21:30"
    );
}

/// A crate with sub-crates is a folder: it holds no tracks of its own and
/// shows those of its sub-crates. Tracks in a crate that gets a sub-crate
/// go into a first sub-crate of the same name; a smart crate takes no
/// sub-crates.
#[test]
fn a_crate_with_sub_crates_is_a_folder() {
    let mut lib = Library::open_in_memory().unwrap();
    let ids: Vec<_> = (0..3)
        .map(|i| {
            lib.upsert_track(&meta(&format!("/m/{i}.mp3"), "A", &format!("T{i}")))
                .unwrap()
                .0
        })
        .collect();
    let house = lib.create_crate("House", None).unwrap();
    lib.add_to_crate(house, &[ids[0], ids[1]]).unwrap();
    // A sub-crate: the tracks move into "House" inside House, first.
    let deep = lib.create_crate("Deep", Some(house)).unwrap();
    let tree = lib.crate_tree().unwrap();
    let h = tree.iter().find(|c| c.id == house).unwrap();
    let names: Vec<&str> = h.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["House", "Deep"]);
    assert!(lib.crate_tracks(house).unwrap().is_empty());
    assert_eq!(
        lib.crate_tracks(h.children[0].id).unwrap(),
        vec![ids[0], ids[1]]
    );
    // No tracks into a folder; into its crates.
    assert!(matches!(
        lib.add_to_crate(house, &[ids[2]]),
        Err(LibraryError::ParentCrate)
    ));
    lib.add_to_crate(deep, &[ids[2], ids[0]]).unwrap();
    // The folder shows its crates' tracks, each once, in tree order.
    assert_eq!(
        lib.shown_tracks(house).unwrap(),
        vec![ids[0], ids[1], ids[2]]
    );
    assert_eq!(lib.shown_tracks(deep).unwrap(), vec![ids[2], ids[0]]);
    // Moving a crate into one with tracks makes it a folder too.
    let techno = lib.create_crate("Techno", None).unwrap();
    lib.add_to_crate(techno, &[ids[1]]).unwrap();
    let peak = lib.create_crate("Peak", None).unwrap();
    lib.move_crate(peak, Some(techno), 0).unwrap();
    assert!(lib.crate_tracks(techno).unwrap().is_empty());
    assert_eq!(lib.shown_tracks(techno).unwrap(), vec![ids[1]]);
    // A smart crate takes no sub-crates.
    let smart = lib
        .create_smart_crate("Smart", None, &dj_library::SmartRules::default())
        .unwrap();
    assert!(matches!(
        lib.create_crate("Inside", Some(smart)),
        Err(LibraryError::SmartParent)
    ));
}

/// A library from before the rule: a crate that has both tracks and
/// sub-crates gets its tracks moved into a sub-crate when it is opened.
#[test]
fn old_parents_with_tracks_become_folders_when_opened() {
    let dir = tmp("folders");
    let path = dir.join("library.db");
    let (sets, t) = {
        let mut lib = Library::open(&path).unwrap();
        let t = lib.upsert_track(&meta("/m/x.mp3", "A", "X")).unwrap().0;
        let sets = lib.create_crate("Sets", None).unwrap();
        lib.add_to_crate(sets, &[t]).unwrap();
        (sets, t)
    };
    // A sub-crate made the old way (straight into the database).
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute(
            "INSERT INTO crates (parent_id, name, position) VALUES (?1, 'Friday', 0)",
            [sets],
        )
        .unwrap();
    }
    let lib = Library::open(&path).unwrap();
    assert!(lib.crate_tracks(sets).unwrap().is_empty());
    let tree = lib.crate_tree().unwrap();
    let s = tree.iter().find(|c| c.id == sets).unwrap();
    let names: Vec<&str> = s.children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Sets", "Friday"]);
    assert_eq!(lib.crate_tracks(s.children[0].id).unwrap(), vec![t]);
}
