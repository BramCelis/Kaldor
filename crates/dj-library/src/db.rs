//! The library database.

use crate::model::{CrateId, CrateNode, Cue, CueKind, SavedLoop, Track, TrackId, TrackMeta};
use crate::schema;
use crate::search::{Query, SearchIndex};
use crate::smart::SmartRules;
use dj_analysis::{Analysis, Waveform};
use dj_core::{Beatgrid, MusicalKey};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Library errors.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("no such crate {0}")]
    NoSuchCrate(CrateId),
    #[error("cannot move a crate into itself or one of its sub-crates")]
    CrateCycle,
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("a smart crate fills itself from its rules: edit the rules instead")]
    SmartCrate,
    #[error("a crate with sub-crates is a folder: put tracks in one of its crates")]
    ParentCrate,
    #[error("a smart crate cannot hold sub-crates")]
    SmartParent,
}

pub type Result<T> = std::result::Result<T, LibraryError>;

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// The music library.
pub struct Library {
    pub(crate) conn: Connection,
    index: Option<SearchIndex>,
}

const TRACK_COLUMNS: &str = "id, path, title, artist, album, genre, comment, year, duration, \
     bitrate, sample_rate, file_size, file_mtime, tag_bpm, tag_key, bpm, musical_key, lufs, \
     gain_db, rating, color, play_count, date_added, missing, analysis_version";

fn track_from_row(r: &Row<'_>) -> rusqlite::Result<Track> {
    let tag_bpm: Option<f64> = r.get(13)?;
    let tag_key: Option<String> = r.get(14)?;
    let bpm: Option<f64> = r.get(15)?;
    let key: Option<String> = r.get(16)?;
    let tag_key = tag_key.as_deref().and_then(MusicalKey::parse);
    Ok(Track {
        id: r.get(0)?,
        meta: TrackMeta {
            path: PathBuf::from(r.get::<_, String>(1)?),
            title: r.get(2)?,
            artist: r.get(3)?,
            album: r.get(4)?,
            genre: r.get(5)?,
            comment: r.get(6)?,
            year: r.get(7)?,
            duration: r.get(8)?,
            bitrate: r.get(9)?,
            sample_rate: r.get(10)?,
            file_size: r.get::<_, i64>(11)? as u64,
            file_mtime: r.get(12)?,
            tag_bpm,
            tag_key,
        },
        bpm: bpm.or(tag_bpm),
        key: key.as_deref().and_then(MusicalKey::parse).or(tag_key),
        lufs: r.get(17)?,
        gain_db: r.get(18)?,
        rating: r.get::<_, i64>(19)?.clamp(0, 5) as u8,
        color: r.get::<_, Option<i64>>(20)?.map(|c| c as u32),
        play_count: r.get::<_, i64>(21)?.max(0) as u32,
        date_added: r.get(22)?,
        missing: r.get::<_, i64>(23)? != 0,
        analysis_version: r.get::<_, Option<i64>>(24)?.map(|v| v as u32),
    })
}

fn path_str(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

impl Library {
    /// Opens (or creates) a library file and applies migrations.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        schema::migrate(&mut conn)?;
        let mut lib = Self { conn, index: None };
        // Crates with sub-crates are folders: tracks still in
        // one from before go into a sub-crate of the same name.
        lib.fold_parent_tracks()?;
        Ok(lib)
    }

    /// An in-memory library (tests, smoke test).
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn, index: None })
    }

    /// The library file, `None` for an in-memory library.
    pub fn file_path(&self) -> Option<PathBuf> {
        self.conn
            .path()
            .filter(|p| !p.is_empty())
            .map(PathBuf::from)
    }

    pub fn schema_version(&self) -> Result<u32> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?)
    }

    fn invalidate(&mut self) {
        self.index = None;
    }

    // ----- tracks -------------------------------------------------------

    /// Inserts a track or updates its file metadata (matched by path).
    /// Returns the id and whether it was newly added.
    pub fn upsert_track(&mut self, m: &TrackMeta) -> Result<(TrackId, bool)> {
        let r = upsert(&self.conn, m)?;
        self.invalidate();
        Ok(r)
    }

    /// Inserts or updates many tracks in one transaction.
    pub fn upsert_tracks(&mut self, metas: &[TrackMeta]) -> Result<Vec<(TrackId, bool)>> {
        let tx = self.conn.transaction()?;
        let mut out = Vec::with_capacity(metas.len());
        for m in metas {
            out.push(upsert(&tx, m)?);
        }
        tx.commit()?;
        self.invalidate();
        Ok(out)
    }

    pub fn track(&self, id: TrackId) -> Result<Option<Track>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE id = ?1"),
                [id],
                track_from_row,
            )
            .optional()?)
    }

    pub fn track_by_path(&self, path: &Path) -> Result<Option<Track>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TRACK_COLUMNS} FROM tracks WHERE path = ?1"),
                [path_str(path)],
                track_from_row,
            )
            .optional()?)
    }

    /// All tracks, ordered by id.
    pub fn tracks(&self) -> Result<Vec<Track>> {
        let mut st = self
            .conn
            .prepare(&format!("SELECT {TRACK_COLUMNS} FROM tracks ORDER BY id"))?;
        let rows = st.query_map([], track_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn track_count(&self) -> Result<usize> {
        Ok(self
            .conn
            .query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0))?
            as usize)
    }

    pub fn remove_track(&mut self, id: TrackId) -> Result<()> {
        self.conn
            .execute("DELETE FROM tracks WHERE id = ?1", [id])?;
        self.invalidate();
        Ok(())
    }

    /// Removes tracks from the library and from every crate, with their
    /// cues, loops, grids and waveforms. The files stay on disk.
    pub fn remove_tracks(&mut self, ids: &[TrackId]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for id in ids {
            tx.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        self.invalidate();
        Ok(())
    }

    /// Empties the library: every track and crate (with cues, loops, grids
    /// and waveforms). The files stay on disk.
    pub fn clear(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM crate_tracks", [])?;
        tx.execute("DELETE FROM crates", [])?;
        tx.execute("DELETE FROM tracks", [])?;
        tx.commit()?;
        self.invalidate();
        Ok(())
    }

    pub fn set_rating(&mut self, id: TrackId, stars: u8) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET rating = ?1 WHERE id = ?2",
            params![i64::from(stars.min(5)), id],
        )?;
        Ok(())
    }

    pub fn set_color(&mut self, id: TrackId, color: Option<u32>) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET color = ?1 WHERE id = ?2",
            params![color.map(i64::from), id],
        )?;
        Ok(())
    }

    /// Marks a track's file as missing (or found).
    pub fn set_missing(&mut self, id: TrackId, missing: bool) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET missing = ?1 WHERE id = ?2",
            params![i64::from(missing), id],
        )?;
        Ok(())
    }

    /// Merges what another program knows about a track without losing
    /// anything of Kaldor's: rating and colour only where there is none,
    /// the higher play count, the earlier date added.
    pub(crate) fn merge_stats(
        &mut self,
        id: TrackId,
        t: &crate::interop::ForeignTrack,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET
                 rating = CASE WHEN rating = 0 THEN ?1 ELSE rating END,
                 color = COALESCE(color, ?2),
                 play_count = MAX(play_count, ?3),
                 date_added = MIN(date_added, COALESCE(?4, date_added))
             WHERE id = ?5",
            params![
                i64::from(t.rating.min(5)),
                t.color.map(i64::from),
                i64::from(t.play_count),
                t.date_added,
                id
            ],
        )?;
        self.invalidate();
        Ok(())
    }

    /// True when the track's grid was edited or locked in Kaldor (an
    /// analysed grid is not).
    pub(crate) fn grid_set_by_user(&self, id: TrackId) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT locked FROM beatgrids WHERE track_id = ?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some_and(|l| l != 0))
    }

    /// Counts a play (deck loaded and played).
    pub fn record_play(&mut self, id: TrackId) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET play_count = play_count + 1, last_played = ?1 WHERE id = ?2",
            params![now(), id],
        )?;
        Ok(())
    }

    /// Re-checks every file on disk and updates the `missing` flag. Returns
    /// the number of missing files.
    pub fn check_missing(&mut self) -> Result<usize> {
        let paths: Vec<(TrackId, String)> = {
            let mut st = self.conn.prepare("SELECT id, path FROM tracks")?;
            st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let tx = self.conn.transaction()?;
        let mut missing = 0;
        for (id, p) in paths {
            let m = !Path::new(&p).exists();
            missing += usize::from(m);
            tx.execute(
                "UPDATE tracks SET missing = ?1 WHERE id = ?2",
                params![i64::from(m), id],
            )?;
        }
        tx.commit()?;
        Ok(missing)
    }

    /// Points a track at a new file (after the user found a moved file).
    pub fn relocate(&mut self, id: TrackId, new_path: &Path) -> Result<()> {
        self.conn.execute(
            "UPDATE tracks SET path = ?1, missing = 0 WHERE id = ?2",
            params![path_str(new_path), id],
        )?;
        self.invalidate();
        Ok(())
    }

    // ----- analysis -----------------------------------------------------

    /// Stores analysis results. A locked or user-edited beatgrid is kept.
    pub fn store_analysis(&mut self, id: TrackId, a: &Analysis) -> Result<()> {
        let tx = self.conn.transaction()?;
        let keep_grid: bool = tx
            .query_row(
                "SELECT locked FROM beatgrids WHERE track_id = ?1",
                [id],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .is_some_and(|l| l != 0);
        let bpm = if keep_grid {
            None
        } else {
            a.beatgrid.map(|g| g.bpm)
        };
        tx.execute(
            "UPDATE tracks SET bpm = COALESCE(?1, bpm), musical_key = ?2, lufs = ?3, gain_db = ?4,
                 duration = CASE WHEN ?5 > 0 THEN ?5 ELSE duration END,
                 analysis_version = ?6, analyzed_at = ?7 WHERE id = ?8",
            params![
                bpm,
                a.key.map(|k| k.camelot()),
                a.lufs,
                a.gain_db,
                a.duration,
                i64::from(a.version),
                now(),
                id
            ],
        )?;
        if let (Some(g), false) = (a.beatgrid, keep_grid) {
            tx.execute(
                "INSERT INTO beatgrids (track_id, bpm, first_beat, first_beat_in_bar, locked, bar_zero)
                 VALUES (?1, ?2, ?3, ?4, 0, ?5)
                 ON CONFLICT(track_id) DO UPDATE SET bpm = excluded.bpm,
                     first_beat = excluded.first_beat, first_beat_in_bar = excluded.first_beat_in_bar,
                     bar_zero = excluded.bar_zero",
                params![id, g.bpm, g.first_beat, i64::from(g.first_beat_in_bar), g.bar_zero],
            )?;
        }
        tx.execute(
            "INSERT INTO waveforms (track_id, data) VALUES (?1, ?2)
             ON CONFLICT(track_id) DO UPDATE SET data = excluded.data",
            params![id, a.waveform.to_bytes()],
        )?;
        tx.commit()?;
        self.invalidate();
        Ok(())
    }

    /// Forgets that the user edited a track's grid (not a locked one), so
    /// the next analysis replaces it. Used by an explicit re-analysis.
    pub fn forget_grid_edit(&mut self, id: TrackId) -> Result<()> {
        self.conn.execute(
            "UPDATE beatgrids SET locked = 0 WHERE track_id = ?1 AND locked = 2",
            [id],
        )?;
        Ok(())
    }

    /// Tracks never analysed, or analysed by an older algorithm version.
    pub fn tracks_needing_analysis(&self) -> Result<Vec<(TrackId, PathBuf)>> {
        let mut st = self.conn.prepare(
            "SELECT id, path FROM tracks WHERE missing = 0 AND
             (analysis_version IS NULL OR analysis_version < ?1) ORDER BY id",
        )?;
        let rows = st.query_map([i64::from(dj_analysis::ANALYSIS_VERSION)], |r| {
            Ok((r.get(0)?, PathBuf::from(r.get::<_, String>(1)?)))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn beatgrid(&self, id: TrackId) -> Result<Option<Beatgrid>> {
        Ok(self
            .conn
            .query_row(
                "SELECT bpm, first_beat, first_beat_in_bar, locked, bar_zero FROM beatgrids
                 WHERE track_id = ?1",
                [id],
                |r| {
                    Ok(Beatgrid {
                        bpm: r.get(0)?,
                        first_beat: r.get(1)?,
                        first_beat_in_bar: r.get::<_, i64>(2)?.clamp(1, 4) as u8,
                        // 1 = locked; 2 = edited by the user (kept by
                        // re-analysis like a locked grid, but editable).
                        locked: r.get::<_, i64>(3)? == 1,
                        bar_zero: r.get(4)?,
                    })
                },
            )
            .optional()?)
    }

    /// Saves a beatgrid edited by the user (also updates the track BPM). It
    /// is stored as edited (`locked` = 2), or locked (1): re-analysis keeps
    /// both, so an edit survives a changed file or a new analysis version.
    pub fn set_beatgrid(&mut self, id: TrackId, g: &Beatgrid) -> Result<()> {
        self.conn.execute(
            "INSERT INTO beatgrids (track_id, bpm, first_beat, first_beat_in_bar, locked, bar_zero)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(track_id) DO UPDATE SET bpm = excluded.bpm, first_beat = excluded.first_beat,
                 first_beat_in_bar = excluded.first_beat_in_bar, locked = excluded.locked,
                 bar_zero = excluded.bar_zero",
            params![
                id,
                g.bpm,
                g.first_beat,
                i64::from(g.first_beat_in_bar),
                if g.locked { 1i64 } else { 2 },
                g.bar_zero
            ],
        )?;
        self.conn.execute(
            "UPDATE tracks SET bpm = ?1 WHERE id = ?2",
            params![g.bpm, id],
        )?;
        self.invalidate();
        Ok(())
    }

    pub fn waveform(&self, id: TrackId) -> Result<Option<Waveform>> {
        let data: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT data FROM waveforms WHERE track_id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?;
        Ok(data.and_then(|d| Waveform::from_bytes(&d)))
    }

    // ----- cues and loops -----------------------------------------------

    /// Replaces all cues of a track.
    pub fn set_cues(&mut self, id: TrackId, cues: &[Cue]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM cues WHERE track_id = ?1", [id])?;
        for c in cues {
            let (kind, slot) = match c.kind {
                CueKind::Main => ("main", None),
                CueKind::Hot(s) => ("hot", Some(i64::from(s.min(7)))),
                CueKind::Memory => ("memory", None),
            };
            tx.execute(
                "INSERT INTO cues (track_id, kind, slot, position, name, color) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, kind, slot, c.position, c.name, c.color.map(i64::from)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn cues(&self, id: TrackId) -> Result<Vec<Cue>> {
        let mut st = self.conn.prepare(
            "SELECT kind, slot, position, name, color FROM cues WHERE track_id = ?1 ORDER BY position",
        )?;
        let rows = st.query_map([id], |r| {
            let kind: String = r.get(0)?;
            let slot: Option<i64> = r.get(1)?;
            let kind = match (kind.as_str(), slot) {
                ("hot", Some(s)) => CueKind::Hot(s.clamp(0, 7) as u8),
                ("main", _) => CueKind::Main,
                _ => CueKind::Memory,
            };
            Ok(Cue {
                kind,
                position: r.get(2)?,
                name: r.get(3)?,
                color: r.get::<_, Option<i64>>(4)?.map(|c| c as u32),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Replaces all saved loops of a track.
    pub fn set_loops(&mut self, id: TrackId, loops: &[SavedLoop]) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM loops WHERE track_id = ?1", [id])?;
        for l in loops {
            tx.execute(
                "INSERT INTO loops (track_id, slot, start, end, name, color) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, i64::from(l.slot.min(7)), l.start, l.end, l.name, l.color.map(i64::from)],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn loops(&self, id: TrackId) -> Result<Vec<SavedLoop>> {
        let mut st = self.conn.prepare(
            "SELECT slot, start, end, name, color FROM loops WHERE track_id = ?1 ORDER BY slot",
        )?;
        let rows = st.query_map([id], |r| {
            Ok(SavedLoop {
                slot: r.get::<_, i64>(0)?.clamp(0, 7) as u8,
                start: r.get(1)?,
                end: r.get(2)?,
                name: r.get(3)?,
                color: r.get::<_, Option<i64>>(4)?.map(|c| c as u32),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ----- crates -------------------------------------------------------

    fn next_position(&self, parent: Option<CrateId>) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM crates WHERE parent_id IS ?1",
            [parent],
            |r| r.get(0),
        )?)
    }

    fn crate_exists(&self, id: CrateId) -> Result<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM crates WHERE id = ?1", [id], |_| Ok(()))
            .optional()?
            .is_some())
    }

    /// Whether a crate has sub-crates.
    pub fn has_sub_crates(&self, id: CrateId) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM crates WHERE parent_id = ?1 LIMIT 1",
                [id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Whether a crate is kept by a sync (the rekordbox tab's).
    fn is_synced(&self, id: CrateId) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT synced IS NOT NULL FROM crates WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Makes `parent` ready to hold a sub-crate: a crate with sub-crates is
    /// a folder, so tracks it holds go into a new first sub-crate of the
    /// same name. Refuses a smart crate. Synced crates (rekordbox's
    /// collection) are left as they are. Returns the crate the tracks went
    /// into, if any.
    fn make_folder(&mut self, parent: CrateId) -> Result<Option<CrateId>> {
        if !self.crate_exists(parent)? {
            return Err(LibraryError::NoSuchCrate(parent));
        }
        if self.is_synced(parent)? {
            return Ok(None);
        }
        if self.smart_rules(parent)?.is_some() {
            return Err(LibraryError::SmartParent);
        }
        let held: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM crate_tracks WHERE crate_id = ?1",
            [parent],
            |r| r.get(0),
        )?;
        if held == 0 {
            return Ok(None);
        }
        let name: String =
            self.conn
                .query_row("SELECT name FROM crates WHERE id = ?1", [parent], |r| {
                    r.get(0)
                })?;
        let tx = self.conn.transaction()?;
        let first: i64 = tx.query_row(
            "SELECT COALESCE(MIN(position), 0) - 1 FROM crates WHERE parent_id = ?1",
            [parent],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO crates (parent_id, name, position) VALUES (?1, ?2, ?3)",
            params![parent, name, first],
        )?;
        let child = tx.last_insert_rowid();
        tx.execute(
            "UPDATE crate_tracks SET crate_id = ?1 WHERE crate_id = ?2",
            params![child, parent],
        )?;
        tx.commit()?;
        Ok(Some(child))
    }

    /// Moves the tracks of every crate that has sub-crates into a sub-crate
    /// (see [`Self::make_folder`]); a library from before the rule.
    /// Returns how many crates were changed.
    pub fn fold_parent_tracks(&mut self) -> Result<usize> {
        let parents: Vec<CrateId> = {
            let mut st = self.conn.prepare(
                "SELECT DISTINCT p.id FROM crates p \
                 JOIN crates c ON c.parent_id = p.id \
                 JOIN crate_tracks t ON t.crate_id = p.id \
                 WHERE p.synced IS NULL AND p.smart IS NULL",
            )?;
            st.query_map([], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for p in &parents {
            self.make_folder(*p)?;
        }
        Ok(parents.len())
    }

    /// The tracks a crate shows: its own; for a folder (a crate with
    /// sub-crates and no tracks of its own) those of all its sub-crates, in
    /// tree order, each once.
    pub fn shown_tracks(&self, id: CrateId) -> Result<Vec<TrackId>> {
        let own = self.crate_tracks(id)?;
        if !own.is_empty() || !self.has_sub_crates(id)? {
            return Ok(own);
        }
        fn walk(lib: &Library, id: CrateId, depth: usize, out: &mut Vec<TrackId>) -> Result<()> {
            if depth > 64 {
                return Ok(());
            }
            let children: Vec<CrateId> = {
                let mut st = lib
                    .conn
                    .prepare("SELECT id FROM crates WHERE parent_id = ?1 ORDER BY position, id")?;
                st.query_map([id], |r| r.get(0))?
                    .collect::<rusqlite::Result<_>>()?
            };
            for c in children {
                out.extend(lib.crate_tracks(c)?);
                walk(lib, c, depth + 1, out)?;
            }
            Ok(())
        }
        let mut all = Vec::new();
        walk(self, id, 0, &mut all)?;
        let mut seen = std::collections::HashSet::new();
        all.retain(|t| seen.insert(*t));
        Ok(all)
    }

    /// Creates a crate at the end of `parent` (or at the top level). A
    /// parent holding tracks becomes a folder first (its tracks go into a
    /// sub-crate of its name, [`Self::make_folder`]); a smart crate takes
    /// no sub-crates.
    pub fn create_crate(&mut self, name: &str, parent: Option<CrateId>) -> Result<CrateId> {
        if let Some(p) = parent {
            self.make_folder(p)?;
        }
        let pos = self.next_position(parent)?;
        self.conn.execute(
            "INSERT INTO crates (parent_id, name, position) VALUES (?1, ?2, ?3)",
            params![parent, name, pos],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Creates a smart crate (it fills itself from `rules`).
    pub fn create_smart_crate(
        &mut self,
        name: &str,
        parent: Option<CrateId>,
        rules: &SmartRules,
    ) -> Result<CrateId> {
        let id = self.create_crate(name, parent)?;
        self.set_smart_rules(id, rules)?;
        Ok(id)
    }

    /// A smart crate's rules (`None`: a normal crate).
    pub fn smart_rules(&self, id: CrateId) -> Result<Option<SmartRules>> {
        let s: Option<Option<String>> = self
            .conn
            .query_row("SELECT smart FROM crates WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        match s {
            None => Err(LibraryError::NoSuchCrate(id)),
            Some(s) => Ok(s.map(|s| SmartRules::decode(&s))),
        }
    }

    /// Changes a smart crate's rules (or makes a crate without tracks a
    /// smart crate).
    pub fn set_smart_rules(&mut self, id: CrateId, rules: &SmartRules) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE crates SET smart = ?1 WHERE id = ?2",
            params![rules.encode(), id],
        )?;
        if n == 0 {
            return Err(LibraryError::NoSuchCrate(id));
        }
        Ok(())
    }

    /// Refuses track edits on a smart crate.
    fn not_smart(&self, id: CrateId) -> Result<()> {
        if self.smart_rules(id)?.is_some() {
            return Err(LibraryError::SmartCrate);
        }
        Ok(())
    }

    pub fn rename_crate(&mut self, id: CrateId, name: &str) -> Result<()> {
        let n = self.conn.execute(
            "UPDATE crates SET name = ?1 WHERE id = ?2",
            params![name, id],
        )?;
        if n == 0 {
            return Err(LibraryError::NoSuchCrate(id));
        }
        Ok(())
    }

    /// Deletes a crate and its sub-crates (tracks stay in the library).
    pub fn delete_crate(&mut self, id: CrateId) -> Result<()> {
        self.conn
            .execute("DELETE FROM crates WHERE id = ?1", [id])?;
        Ok(())
    }

    fn parent_of(&self, id: CrateId) -> Result<Option<CrateId>> {
        Ok(self
            .conn
            .query_row("SELECT parent_id FROM crates WHERE id = ?1", [id], |r| {
                r.get(0)
            })?)
    }

    /// Moves a crate under `new_parent` at `index` among its siblings.
    pub fn move_crate(
        &mut self,
        id: CrateId,
        new_parent: Option<CrateId>,
        index: usize,
    ) -> Result<()> {
        if !self.crate_exists(id)? {
            return Err(LibraryError::NoSuchCrate(id));
        }
        // Refuse cycles: walk up from the new parent.
        let mut p = new_parent;
        while let Some(c) = p {
            if c == id {
                return Err(LibraryError::CrateCycle);
            }
            p = self.parent_of(c)?;
        }
        // The new parent becomes a folder.
        if let Some(np) = new_parent {
            self.make_folder(np)?;
        }
        let tx = self.conn.transaction()?;
        let mut siblings: Vec<CrateId> = {
            let mut st = tx.prepare(
                "SELECT id FROM crates WHERE parent_id IS ?1 AND id != ?2 ORDER BY position",
            )?;
            st.query_map(params![new_parent, id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        siblings.insert(index.min(siblings.len()), id);
        tx.execute(
            "UPDATE crates SET parent_id = ?1 WHERE id = ?2",
            params![new_parent, id],
        )?;
        for (pos, c) in siblings.iter().enumerate() {
            tx.execute(
                "UPDATE crates SET position = ?1 WHERE id = ?2",
                params![pos as i64, c],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The crate tree, siblings in their stored order.
    pub fn crate_tree(&self) -> Result<Vec<CrateNode>> {
        let mut st = self.conn.prepare(
            "SELECT id, parent_id, name, smart IS NOT NULL, synced IS NOT NULL FROM crates \
             ORDER BY position, id",
        )?;
        type CrateRow = (CrateId, Option<CrateId>, String, bool, bool);
        let all: Vec<CrateRow> = st
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        fn build(parent: Option<CrateId>, all: &[CrateRow], depth: usize) -> Vec<CrateNode> {
            if depth > 64 {
                return Vec::new();
            }
            all.iter()
                .filter(|c| c.1 == parent)
                .map(|c| CrateNode {
                    id: c.0,
                    name: c.2.clone(),
                    parent: c.1,
                    smart: c.3,
                    synced: c.4,
                    children: build(Some(c.0), all, depth + 1),
                })
                .collect()
        }
        Ok(build(None, &all, 0))
    }

    /// The crate called `name` directly under `parent` (case-sensitive),
    /// if there is one.
    pub fn child_crate(&self, parent: Option<CrateId>, name: &str) -> Result<Option<CrateId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM crates WHERE parent_id IS ?1 AND name = ?2 AND smart IS NULL \
                 AND synced IS NULL ORDER BY position LIMIT 1",
                params![parent, name],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Appends tracks to a crate (tracks already in it are skipped).
    pub fn add_to_crate(&mut self, crate_id: CrateId, tracks: &[TrackId]) -> Result<()> {
        self.not_smart(crate_id)?;
        if self.has_sub_crates(crate_id)? && !self.is_synced(crate_id)? {
            return Err(LibraryError::ParentCrate);
        }
        let tx = self.conn.transaction()?;
        let mut pos: i64 = tx.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM crate_tracks WHERE crate_id = ?1",
            [crate_id],
            |r| r.get(0),
        )?;
        for t in tracks {
            let n = tx.execute(
                "INSERT OR IGNORE INTO crate_tracks (crate_id, track_id, position) VALUES (?1, ?2, ?3)",
                params![crate_id, t, pos],
            )?;
            pos += n as i64;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn remove_from_crate(&mut self, crate_id: CrateId, tracks: &[TrackId]) -> Result<()> {
        self.not_smart(crate_id)?;
        let tx = self.conn.transaction()?;
        for t in tracks {
            tx.execute(
                "DELETE FROM crate_tracks WHERE crate_id = ?1 AND track_id = ?2",
                params![crate_id, t],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Track ids in a crate, in crate order. A smart crate's are worked out
    /// now from its rules, in the order the tracks were added.
    pub fn crate_tracks(&self, crate_id: CrateId) -> Result<Vec<TrackId>> {
        if let Some(rules) = self.smart_rules(crate_id).ok().flatten() {
            return Ok(rules.filter(&self.tracks()?, now()));
        }
        let mut st = self
            .conn
            .prepare("SELECT track_id FROM crate_tracks WHERE crate_id = ?1 ORDER BY position")?;
        let rows = st.query_map([crate_id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Moves a track to `index` within its crate.
    pub fn move_in_crate(&mut self, crate_id: CrateId, track: TrackId, index: usize) -> Result<()> {
        self.not_smart(crate_id)?;
        let mut ids = self.crate_tracks(crate_id)?;
        let Some(from) = ids.iter().position(|t| *t == track) else {
            return Ok(());
        };
        let t = ids.remove(from);
        ids.insert(index.min(ids.len()), t);
        let tx = self.conn.transaction()?;
        for (pos, t) in ids.iter().enumerate() {
            tx.execute(
                "UPDATE crate_tracks SET position = ?1 WHERE crate_id = ?2 AND track_id = ?3",
                params![pos as i64, crate_id, t],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ----- search and maintenance ---------------------------------------

    /// Searches the whole library (see [`Query::parse`] for the syntax).
    pub fn search(&mut self, query: &str) -> Result<Vec<TrackId>> {
        if self.index.is_none() {
            self.index = Some(SearchIndex::build(&self.tracks()?));
        }
        let q = Query::parse(query);
        Ok(self
            .index
            .as_ref()
            .map(|i| i.search(&q))
            .unwrap_or_default())
    }

    /// Copies the library file `src` to `dest` through a connection of its
    /// own, so it can run on a background thread while the library is open
    /// (SQLite gives it a consistent snapshot).
    pub fn backup_file(src: &Path, dest: &Path) -> Result<()> {
        let conn = Connection::open_with_flags(
            src,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        if let Some(dir) = dest.parent() {
            std::fs::create_dir_all(dir)?;
        }
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        conn.execute("VACUUM INTO ?1", [path_str(dest)])?;
        Ok(())
    }

    /// Writes a consistent copy of the database to `dest` (for backups).
    pub fn backup_to(&self, dest: &Path) -> Result<()> {
        if dest.exists() {
            std::fs::remove_file(dest)?;
        }
        self.conn.execute("VACUUM INTO ?1", [path_str(dest)])?;
        Ok(())
    }
}

fn upsert(conn: &Connection, m: &TrackMeta) -> Result<(TrackId, bool)> {
    let existing: Option<TrackId> = conn
        .query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            [path_str(&m.path)],
            |r| r.get(0),
        )
        .optional()?;
    let key = m.tag_key.map(|k| k.camelot());
    match existing {
        Some(id) => {
            conn.execute(
                "UPDATE tracks SET title = ?1, artist = ?2, album = ?3, genre = ?4, comment = ?5,
                    year = ?6, duration = ?7, bitrate = ?8, sample_rate = ?9, file_size = ?10,
                    file_mtime = ?11, tag_bpm = ?12, tag_key = ?13, missing = 0 WHERE id = ?14",
                params![
                    m.title,
                    m.artist,
                    m.album,
                    m.genre,
                    m.comment,
                    m.year,
                    m.duration,
                    m.bitrate,
                    m.sample_rate,
                    m.file_size as i64,
                    m.file_mtime,
                    m.tag_bpm,
                    key,
                    id
                ],
            )?;
            Ok((id, false))
        }
        None => {
            conn.execute(
                "INSERT INTO tracks (path, title, artist, album, genre, comment, year, duration,
                    bitrate, sample_rate, file_size, file_mtime, tag_bpm, tag_key, date_added)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
                params![
                    path_str(&m.path),
                    m.title,
                    m.artist,
                    m.album,
                    m.genre,
                    m.comment,
                    m.year,
                    m.duration,
                    m.bitrate,
                    m.sample_rate,
                    m.file_size as i64,
                    m.file_mtime,
                    m.tag_bpm,
                    key,
                    now()
                ],
            )?;
            Ok((conn.last_insert_rowid(), true))
        }
    }
}
