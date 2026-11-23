//! The play history: one session per app run (from the first track played),
//! every track played in it with the time and deck. Each play keeps a copy
//! of the artist, title, path, BPM and key at that moment, so a session
//! still reads right after the track was edited or removed from the
//! library.

use crate::db::{Library, Result, now};
use crate::interop::playlist::PlaylistEntry;
use crate::model::TrackId;
use dj_core::MusicalKey;
use rusqlite::params;
use std::path::PathBuf;

pub type SessionId = i64;

/// A session in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct HistorySession {
    pub id: SessionId,
    /// The first play, seconds since the Unix epoch.
    pub started: i64,
    /// The last play.
    pub ended: i64,
    pub plays: usize,
}

/// One track played.
#[derive(Clone, Debug, PartialEq)]
pub struct HistoryPlay {
    /// Seconds since the Unix epoch.
    pub played_at: i64,
    /// Deck 1-4 (0: not known).
    pub deck: u8,
    /// The library track (`None` once it was removed from the library).
    pub track: Option<TrackId>,
    pub artist: String,
    pub title: String,
    pub path: PathBuf,
    pub duration: f64,
    pub bpm: Option<f64>,
    pub key: Option<MusicalKey>,
}

impl HistoryPlay {
    /// "Artist - Title" (or the title, or the file name).
    pub fn display(&self) -> String {
        match (self.artist.is_empty(), self.title.is_empty()) {
            (false, false) => format!("{} - {}", self.artist, self.title),
            (true, false) => self.title.clone(),
            _ => self
                .path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default(),
        }
    }
}

impl Library {
    /// Starts a session (when the first track of an app run is played).
    pub fn start_session(&mut self) -> Result<SessionId> {
        let t = now();
        self.conn.execute(
            "INSERT INTO history_sessions (started, ended) VALUES (?1, ?1)",
            [t],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Logs track `id` played on `deck` (1-4) now, in `session`.
    pub fn log_play(&mut self, session: SessionId, id: TrackId, deck: u8) -> Result<()> {
        self.log_play_at(session, id, deck, now())
    }

    /// [`Self::log_play`] at a given time (tests, imports).
    pub fn log_play_at(
        &mut self,
        session: SessionId,
        id: TrackId,
        deck: u8,
        at: i64,
    ) -> Result<()> {
        let Some(t) = self.track(id)? else {
            return Ok(());
        };
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO history_plays
                (session_id, track_id, played_at, deck, artist, title, path, duration, bpm, musical_key)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                session,
                id,
                at,
                deck,
                t.meta.artist,
                t.meta.title,
                t.meta.path.to_string_lossy(),
                t.meta.duration,
                t.bpm,
                t.key.map(|k| k.camelot()),
            ],
        )?;
        tx.execute(
            "UPDATE history_sessions SET ended = MAX(ended, ?1) WHERE id = ?2",
            params![at, session],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Every session with at least one play, newest first.
    pub fn sessions(&self) -> Result<Vec<HistorySession>> {
        let mut st = self.conn.prepare(
            "SELECT s.id, MIN(p.played_at), MAX(p.played_at), COUNT(p.id) FROM history_sessions s
             JOIN history_plays p ON p.session_id = s.id
             GROUP BY s.id ORDER BY MIN(p.played_at) DESC, s.id DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok(HistorySession {
                id: r.get(0)?,
                started: r.get(1)?,
                ended: r.get(2)?,
                plays: r.get::<_, i64>(3)? as usize,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The plays of a session, in the order played.
    pub fn session_plays(&self, session: SessionId) -> Result<Vec<HistoryPlay>> {
        let mut st = self.conn.prepare(
            "SELECT played_at, deck, track_id, artist, title, path, duration, bpm, musical_key
             FROM history_plays WHERE session_id = ?1 ORDER BY played_at, id",
        )?;
        let rows = st.query_map([session], |r| {
            let key: Option<String> = r.get(8)?;
            let path: String = r.get(5)?;
            Ok(HistoryPlay {
                played_at: r.get(0)?,
                deck: r.get(1)?,
                track: r.get(2)?,
                artist: r.get(3)?,
                title: r.get(4)?,
                path: PathBuf::from(path),
                duration: r.get(6)?,
                bpm: r.get(7)?,
                key: key.as_deref().and_then(MusicalKey::parse),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Imports one session from a CSV file of the older history
    /// (`played_at,artist,title,bpm,key,path`, local time; `offset`: seconds
    /// local time is ahead of UTC). Tracks are matched by path. Returns the
    /// number of plays (0: nothing imported, no session made).
    pub fn import_history_csv(&mut self, text: &str, offset: i64) -> Result<usize> {
        let mut plays = Vec::new();
        for line in text.lines().skip(1) {
            let f = split_csv(line);
            if f.len() < 6 {
                continue;
            }
            let Some(at) = crate::interop::parse_date(&f[0]) else {
                continue;
            };
            plays.push((at - offset, f));
        }
        if plays.is_empty() {
            return Ok(0);
        }
        let started = plays.iter().map(|p| p.0).min().unwrap_or(0);
        let ended = plays.iter().map(|p| p.0).max().unwrap_or(started);
        let mut ids = Vec::with_capacity(plays.len());
        for (_, f) in &plays {
            ids.push(
                self.track_by_path(std::path::Path::new(&f[5]))?
                    .map(|t| t.id),
            );
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO history_sessions (started, ended) VALUES (?1, ?2)",
            params![started, ended],
        )?;
        let session = tx.last_insert_rowid();
        for ((at, f), id) in plays.iter().zip(&ids) {
            tx.execute(
                "INSERT INTO history_plays
                    (session_id, track_id, played_at, deck, artist, title, path, bpm, musical_key)
                 VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7, ?8)",
                params![
                    session,
                    id,
                    at,
                    f[1],
                    f[2],
                    f[5],
                    f[3].parse::<f64>().ok(),
                    MusicalKey::parse(&f[4]).map(|k| k.camelot()),
                ],
            )?;
        }
        tx.commit()?;
        Ok(plays.len())
    }

    /// Deletes a session and its plays (the tracks stay).
    pub fn delete_session(&mut self, session: SessionId) -> Result<()> {
        self.conn
            .execute("DELETE FROM history_sessions WHERE id = ?1", [session])?;
        Ok(())
    }
}

/// "2026-11-17 21:30" in UTC from seconds since the Unix epoch (`offset`:
/// seconds to add for the local time).
pub fn format_time(t: i64, offset: i64) -> String {
    let t = t + offset;
    let days = t.div_euclid(86_400);
    let secs = t.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60
    )
}

/// Year, month, day from days since 1970-01-01 (Howard Hinnant's
/// algorithm, public domain).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// One CSV field (quoted when needed).
fn csv(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Splits one CSV line (quoted fields with `""` for a quote).
pub fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => out.push(std::mem::take(&mut cur)),
            (c, _) => cur.push(c),
        }
    }
    out.push(cur);
    out
}

/// A session as CSV:`played_at,deck,artist,title,bpm,key,path`, times
/// shifted by `offset` seconds (local time).
pub fn to_csv(plays: &[HistoryPlay], offset: i64) -> String {
    let nl = if cfg!(windows) { "\r\n" } else { "\n" };
    let mut s = format!("played_at,deck,artist,title,bpm,key,path{nl}");
    for p in plays {
        s.push_str(&format!(
            "{},{},{},{},{},{},{}{nl}",
            format_time(p.played_at, offset),
            p.deck,
            csv(&p.artist),
            csv(&p.title),
            p.bpm.map(|b| format!("{b:.2}")).unwrap_or_default(),
            p.key.map(|k| k.camelot()).unwrap_or_default(),
            csv(&p.path.display().to_string()),
        ));
    }
    s
}

/// A session's plays as playlist entries (for M3U8 / PLS).
pub fn to_entries(plays: &[HistoryPlay]) -> Vec<PlaylistEntry> {
    plays
        .iter()
        .map(|p| PlaylistEntry {
            path: p.path.clone(),
            duration: p.duration,
            display: p.display(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_csv() {
        assert_eq!(format_time(0, 0), "1970-01-01 00:00");
        // 2026-11-17 21:30 UTC.
        assert_eq!(format_time(1_794_951_000, 0), "2026-11-17 21:30");
        assert_eq!(format_time(1_794_951_000, 3600), "2026-11-17 22:30");
        assert_eq!(format_time(951_782_400, 0), "2000-02-29 00:00");
        let p = HistoryPlay {
            played_at: 0,
            deck: 2,
            track: None,
            artist: "A, B".into(),
            title: "Say \"hi\"".into(),
            path: PathBuf::from("x.mp3"),
            duration: 0.0,
            bpm: Some(128.0),
            key: MusicalKey::parse("8A"),
        };
        let text = to_csv(&[p], 0);
        assert!(
            text.contains("1970-01-01 00:00,2,\"A, B\",\"Say \"\"hi\"\"\",128.00,8A,x.mp3"),
            "{text}"
        );
    }
}
