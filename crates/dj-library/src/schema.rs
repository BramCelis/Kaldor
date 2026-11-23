//! Database schema and migrations. `PRAGMA user_version` holds the schema
//! version; each migration runs once, in order, inside a transaction.

use rusqlite::Connection;

/// Migrations, oldest first. Never edit a migration that has shipped; add a
/// new one instead.
const MIGRATIONS: &[&str] = &[
    // 1: initial schema.
    r#"
    CREATE TABLE tracks (
        id              INTEGER PRIMARY KEY,
        path            TEXT NOT NULL UNIQUE,
        title           TEXT NOT NULL DEFAULT '',
        artist          TEXT NOT NULL DEFAULT '',
        album           TEXT NOT NULL DEFAULT '',
        genre           TEXT NOT NULL DEFAULT '',
        comment         TEXT NOT NULL DEFAULT '',
        year            INTEGER,
        duration        REAL NOT NULL DEFAULT 0,
        bitrate         INTEGER,
        sample_rate     INTEGER,
        file_size       INTEGER NOT NULL DEFAULT 0,
        file_mtime      INTEGER NOT NULL DEFAULT 0,
        tag_bpm         REAL,
        tag_key         TEXT,
        bpm             REAL,
        musical_key     TEXT,
        lufs            REAL,
        gain_db         REAL,
        rating          INTEGER NOT NULL DEFAULT 0,
        color           INTEGER,
        play_count      INTEGER NOT NULL DEFAULT 0,
        date_added      INTEGER NOT NULL,
        last_played     INTEGER,
        missing         INTEGER NOT NULL DEFAULT 0,
        analysis_version INTEGER,
        analyzed_at     INTEGER
    );
    CREATE TABLE beatgrids (
        track_id        INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
        bpm             REAL NOT NULL,
        first_beat      REAL NOT NULL,
        first_beat_in_bar INTEGER NOT NULL,
        locked          INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE cues (
        id              INTEGER PRIMARY KEY,
        track_id        INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        kind            TEXT NOT NULL,     -- 'main', 'hot', 'memory'
        slot            INTEGER,           -- 0..7 for hot cues
        position        REAL NOT NULL,     -- seconds
        name            TEXT NOT NULL DEFAULT '',
        color           INTEGER
    );
    CREATE INDEX cues_track ON cues(track_id);
    CREATE TABLE loops (
        id              INTEGER PRIMARY KEY,
        track_id        INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        slot            INTEGER NOT NULL,
        start           REAL NOT NULL,
        end             REAL NOT NULL,
        name            TEXT NOT NULL DEFAULT '',
        color           INTEGER
    );
    CREATE INDEX loops_track ON loops(track_id);
    CREATE TABLE waveforms (
        track_id        INTEGER PRIMARY KEY REFERENCES tracks(id) ON DELETE CASCADE,
        data            BLOB NOT NULL
    );
    CREATE TABLE crates (
        id              INTEGER PRIMARY KEY,
        parent_id       INTEGER REFERENCES crates(id) ON DELETE CASCADE,
        name            TEXT NOT NULL,
        position        INTEGER NOT NULL
    );
    CREATE INDEX crates_parent ON crates(parent_id);
    CREATE TABLE crate_tracks (
        crate_id        INTEGER NOT NULL REFERENCES crates(id) ON DELETE CASCADE,
        track_id        INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        position        INTEGER NOT NULL,
        PRIMARY KEY (crate_id, track_id)
    );
    "#,
    // 2: the bar numbering of a grid (`Beatgrid::bar_zero`,
    // the beat index of bar 0's downbeat). Existing grids count from the
    // first downbeat of the track, as `number_bars_from_start` does.
    r#"
    ALTER TABLE beatgrids ADD COLUMN bar_zero INTEGER NOT NULL DEFAULT 0;
    UPDATE beatgrids SET bar_zero = (5 - first_beat_in_bar) % 4;
    "#,
    // 3: migration 2 (and analysis that day) numbered grids
    // from the first downbeat after the track start, which made the bar
    // the music starts in bar -1 when the track opens on a later beat.
    // Bar 1 is the bar the track starts in: its downbeat is beat
    // 1 - first_beat_in_bar. Grids still holding the old default get it;
    // a bar 1 set with BEAT 1 further into the track keeps its place.
    r#"
    UPDATE beatgrids SET bar_zero = 1 - first_beat_in_bar
        WHERE bar_zero = (5 - first_beat_in_bar) % 4;
    "#,
    // 4: smart crates. A crate with rules (see `smart.rs`)
    // fills itself; NULL is a normal crate.
    r#"
    ALTER TABLE crates ADD COLUMN smart TEXT;
    "#,
    // 5: the play history (see `history.rs`). A play keeps a
    // copy of the track's tags, so it reads right after the track is gone.
    r#"
    CREATE TABLE history_sessions (
        id              INTEGER PRIMARY KEY,
        started         INTEGER NOT NULL,
        ended           INTEGER NOT NULL
    );
    CREATE TABLE history_plays (
        id              INTEGER PRIMARY KEY,
        session_id      INTEGER NOT NULL REFERENCES history_sessions(id) ON DELETE CASCADE,
        track_id        INTEGER REFERENCES tracks(id) ON DELETE SET NULL,
        played_at       INTEGER NOT NULL,
        deck            INTEGER NOT NULL DEFAULT 0,
        artist          TEXT NOT NULL DEFAULT '',
        title           TEXT NOT NULL DEFAULT '',
        path            TEXT NOT NULL DEFAULT '',
        duration        REAL NOT NULL DEFAULT 0,
        bpm             REAL,
        musical_key     TEXT
    );
    CREATE INDEX history_plays_session ON history_plays(session_id);
    "#,
    // 6: crates kept in step with another program (the
    // rekordbox tab): `synced` names the source, NULL is the user's own
    // crate. `synced_tracks` maps the source's track ids to ours, with a
    // fingerprint of what was last taken over, so a sync skips the tracks
    // that did not change.
    r#"
    ALTER TABLE crates ADD COLUMN synced TEXT;
    CREATE TABLE synced_tracks (
        source          TEXT NOT NULL,
        source_id       TEXT NOT NULL,
        track_id        INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
        fingerprint     INTEGER NOT NULL,
        PRIMARY KEY (source, source_id)
    );
    "#,
];

/// Latest schema version.
pub fn latest_version() -> u32 {
    MIGRATIONS.len() as u32
}

/// Applies pending migrations. Fails if the database is newer than this
/// build (so an old version never corrupts a newer library).
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<u32> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let current: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    if current > latest_version() {
        return Err(rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISMATCH),
            Some(format!(
                "library schema version {current} is newer than this app ({})",
                latest_version()
            )),
        ));
    }
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", i as u32 + 1)?;
        tx.commit()?;
    }
    Ok(latest_version())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library from before migration 2 gets the bar numbering of its
    /// grids (migrations 2 and 3): bar 1 is the bar the track starts in.
    #[test]
    fn old_grids_get_bar_1_where_the_track_starts() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO tracks (id, path, date_added) VALUES (1, 'a.mp3', 0), (2, 'b.mp3', 0)",
            [],
        )
        .unwrap();
        // Track 1 starts on a downbeat; track 2's first beat is the third of
        // its bar (so its first downbeat is beat 2).
        conn.execute(
            "INSERT INTO beatgrids (track_id, bpm, first_beat, first_beat_in_bar, locked)
             VALUES (1, 120, 0.1, 1, 0), (2, 120, 0.1, 3, 2)",
            [],
        )
        .unwrap();
        assert_eq!(migrate(&mut conn).unwrap(), latest_version());
        let zero = |id: i64| -> i64 {
            conn.query_row(
                "SELECT bar_zero FROM beatgrids WHERE track_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        // Track 2's bar 1 starts 2 beats before its first beat.
        assert_eq!((zero(1), zero(2)), (0, -2));
        // The same as a grid numbered by the app.
        let mut g = dj_core::Beatgrid::new(120.0, 0.1);
        g.first_beat_in_bar = 3;
        g.number_bars_from_start();
        assert_eq!(g.bar_zero, -2);
    }

    /// A library already at version 2 (this morning's numbering): grids
    /// with the old default move to bar 1 where the track starts; a bar 1
    /// set with BEAT 1 further in stays.
    #[test]
    fn migration_3_fixes_the_shifted_numbering() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(MIGRATIONS[0]).unwrap();
        conn.execute_batch(MIGRATIONS[1]).unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
        conn.execute(
            "INSERT INTO tracks (id, path, date_added) VALUES (1, 'a', 0), (2, 'b', 0), (3, 'c', 0)",
            [],
        )
        .unwrap();
        // 1: analysed, first beat is beat 2 (old default 3); 2: starts on a
        // downbeat (0 either way); 3: BEAT 1 set 10 bars in (41).
        conn.execute(
            "INSERT INTO beatgrids (track_id, bpm, first_beat, first_beat_in_bar, locked, bar_zero)
             VALUES (1, 163, 0.0002, 2, 0, 3), (2, 128, 0.05, 1, 2, 0), (3, 128, 0.1, 4, 2, 41)",
            [],
        )
        .unwrap();
        migrate(&mut conn).unwrap();
        let zero = |id: i64| -> i64 {
            conn.query_row(
                "SELECT bar_zero FROM beatgrids WHERE track_id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!((zero(1), zero(2), zero(3)), (-1, 0, 41));
    }
}
