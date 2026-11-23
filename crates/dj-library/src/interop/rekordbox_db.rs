//! rekordbox 6 / 7's own library (`master.db`), read directly, for the
//! rekordbox tab (kept in step by [`crate::interop::sync`]).
//!
//! `master.db` is an SQLite database encrypted with SQLCipher 4 and its
//! default settings: 4096-byte pages, each ending in an 80-byte tail (a
//! 16-byte IV, then a 64-byte HMAC-SHA512 over the page's ciphertext, the IV
//! and the page number little-endian), AES-256-CBC, keys from
//! PBKDF2-HMAC-SHA512 (256 000 rounds over the 16-byte salt that replaces
//! the "SQLite format 3" header; the HMAC key from 2 rounds over the cipher
//! key with the salt XOR 0x3a). Every rekordbox installation uses the same
//! passphrase; it is kept here obfuscated the way the open-source tools
//! that read this database keep it. Facts from those
//! tools' documentation, checked by the HMAC of every page; no code taken.
//!
//! The file is only read: copied into memory with its `-wal`, decrypted
//! there (committed WAL frames applied) and opened as an in-memory,
//! read-only SQLite database. rekordbox may stay open.
//!
//! Tables (as documented by pyrekordbox): `djmdContent` (tracks; `BPM` in
//! centi-BPM, `Length` in seconds), `djmdArtist` / `djmdAlbum` /
//! `djmdGenre` / `djmdKey` / `djmdColor` (names), `djmdCue` (`InMsec`,
//! `OutMsec` -1 when not a loop, `Kind` 0 a memory cue, otherwise the hot
//! cue's number), `djmdPlaylist` (`Attribute` 0 playlist, 1 folder, 4
//! smart playlist; `ParentID` "root" at the top; `Seq` the order) and
//! `djmdSongPlaylist` (`TrackNo` the order). Rows with `rb_local_deleted`
//! set are in rekordbox's trash. Beat grids are in the track's ANLZ file
//! (`AnalysisDataPath`, under `share` in the rekordbox folder), section
//! `PQTZ`.

use super::rekordbox::grid_at;
use super::{
    ForeignLibrary, ForeignNode, ForeignTrack, parse_date, slot_loops, tidy_cues, url_to_path,
};
use crate::model::{Cue, CueKind, SavedLoop};
use dj_core::MusicalKey;
use rusqlite::Connection;
use rusqlite::types::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// The file name of rekordbox's library in its folder.
pub const DB_FILE: &str = "master.db";

/// rekordbox's folder on this computer, where it is by default (whether or
/// not it is there).
pub fn default_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Pioneer").join("rekordbox"))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| {
            PathBuf::from(h)
                .join("Library")
                .join("Pioneer")
                .join("rekordbox")
        })
    } else {
        None
    }
}

/// What changes when rekordbox writes its library: the size and time of
/// `master.db` and of its `-wal`. `None` when there is no `master.db`.
pub fn stamp(dir: &Path) -> Option<Vec<(u64, Option<std::time::SystemTime>)>> {
    let one = |p: PathBuf| {
        std::fs::metadata(p)
            .ok()
            .map(|m| (m.len(), m.modified().ok()))
    };
    let db = one(dir.join(DB_FILE))?;
    let wal = one(dir.join(format!("{DB_FILE}-wal"))).unwrap_or((0, None));
    Some(vec![db, wal])
}

// ----- the passphrase ------------------------------------------------------

/// The passphrase, obfuscated: base85 (RFC 1924 alphabet), XOR with
/// [`BLOB_KEY`] (repeated), zlib.
const BLOB: &str = "PN_Pq^*N>(JYe*u^8;Yg76HuZ<mR13S?=>)b9;DpoTXV(6ItkU`}8*m6tx_I{Solh_N#dfe{v=";
const BLOB_KEY: &[u8] = b"657f48f84c437cc1";
const B85: &[u8] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!#$%&()*+-;<=>?@^_`{|}~";

/// Base85 as Python's `base64.b85decode` reads it (a short last group is
/// padded with the highest digit, and as many bytes dropped).
fn b85decode(s: &str) -> Option<Vec<u8>> {
    let mut digits: Vec<u8> = s
        .bytes()
        .map(|c| B85.iter().position(|d| *d == c).map(|p| p as u8))
        .collect::<Option<_>>()?;
    let pad = (5 - digits.len() % 5) % 5;
    digits.extend(std::iter::repeat_n(84u8, pad));
    let mut out = Vec::with_capacity(digits.len() / 5 * 4);
    for g in digits.chunks(5) {
        let v = g.iter().try_fold(0u32, |acc, d| {
            acc.checked_mul(85)?.checked_add(u32::from(*d))
        })?;
        out.extend_from_slice(&v.to_be_bytes());
    }
    out.truncate(out.len() - pad);
    Some(out)
}

/// The passphrase of every rekordbox 6 / 7 `master.db`. The environment
/// variable `KALDOR_REKORDBOX_KEY` replaces it (should a later rekordbox
/// change it).
pub fn passphrase() -> Result<String, String> {
    if let Ok(k) = std::env::var("KALDOR_REKORDBOX_KEY")
        && !k.trim().is_empty()
    {
        return Ok(k.trim().to_string());
    }
    let mut b = b85decode(BLOB).ok_or("the rekordbox key does not decode")?;
    for (i, x) in b.iter_mut().enumerate() {
        *x ^= BLOB_KEY[i % BLOB_KEY.len()];
    }
    let plain = miniz_oxide::inflate::decompress_to_vec_zlib(&b)
        .map_err(|_| "the rekordbox key does not decode")?;
    String::from_utf8(plain).map_err(|_| "the rekordbox key does not decode".into())
}

// ----- SQLCipher 4 ---------------------------------------------------------

const PAGE: usize = 4096;
const IV: usize = 16;
const MAC: usize = 64;
const RESERVE: usize = IV + MAC;
const SALT: usize = 16;
const KDF_ROUNDS: u32 = 256_000;

struct Keys {
    cipher: [u8; 32],
    mac: [u8; 32],
}

impl Keys {
    fn new(passphrase: &[u8], salt: &[u8]) -> Self {
        use sha2::Sha512;
        let cipher = pbkdf2::pbkdf2_hmac_array::<Sha512, 32>(passphrase, salt, KDF_ROUNDS);
        let mac_salt: Vec<u8> = salt.iter().map(|b| b ^ 0x3a).collect();
        let mac = pbkdf2::pbkdf2_hmac_array::<Sha512, 32>(&cipher, &mac_salt, 2);
        Keys { cipher, mac }
    }

    /// The HMAC of page `pgno` whose ciphertext and IV are `data`.
    fn mac(&self, data: &[u8], pgno: u32) -> [u8; MAC] {
        use hmac::{Hmac, KeyInit, Mac};
        let mut m = <Hmac<sha2::Sha512> as KeyInit>::new_from_slice(&self.mac)
            .expect("HMAC takes any key length");
        m.update(data);
        m.update(&pgno.to_le_bytes());
        m.finalize().into_bytes().into()
    }

    /// Checks and decrypts one page in place. False when its HMAC does not
    /// match (a wrong key, or a page being written).
    fn open(&self, page: &mut [u8], pgno: u32) -> bool {
        use cbc::cipher::block_padding::NoPadding;
        use cbc::cipher::{BlockModeDecrypt, KeyIvInit};
        let start = if pgno == 1 { SALT } else { 0 };
        if page.len() != PAGE || self.mac(&page[start..PAGE - MAC], pgno) != page[PAGE - MAC..] {
            return false;
        }
        let (data, tail) = page[start..].split_at_mut(PAGE - start - RESERVE);
        cbc::Decryptor::<aes::Aes256>::new_from_slices(&self.cipher, &tail[..IV])
            .expect("key and IV lengths")
            .decrypt_padded::<NoPadding>(data)
            .is_ok()
    }
}

/// The plain database for an encrypted `db` and its `-wal` (may be empty):
/// every page checked and decrypted, the WAL's committed frames applied.
/// The result opens with SQLite (as a rollback-journal database).
pub fn decrypt(db: &[u8], wal: &[u8], passphrase: &[u8]) -> Result<Vec<u8>, String> {
    if db.len() < PAGE || !db.len().is_multiple_of(PAGE) {
        return Err(format!(
            "{DB_FILE} is {} bytes, not whole 4096-byte pages",
            db.len()
        ));
    }
    let keys = Keys::new(passphrase, &db[..SALT]);
    let mut plain = db.to_vec();
    for (i, page) in plain.chunks_mut(PAGE).enumerate() {
        let pgno = i as u32 + 1;
        if !keys.open(page, pgno) {
            return Err(if pgno == 1 {
                "rekordbox's library does not open with the known key (a rekordbox version \
                 this build does not know?)"
                    .into()
            } else {
                format!("page {pgno} of {DB_FILE} does not check out (rekordbox was writing?)")
            });
        }
    }
    // The WAL: a 32-byte header (big-endian; page size at 8, salt at 16),
    // then frames of a 24-byte header (page number, database size in pages
    // after a commit, else 0; the salt at 8) and a page. Frames belong to
    // the log while they carry the header's salt; the ones after the last
    // commit are not part of the database yet.
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    if wal.len() >= 32 && be(&wal[8..]) as usize == PAGE {
        let mut pending: Vec<(u32, Vec<u8>)> = Vec::new();
        for frame in wal[32..].as_chunks::<{ 24 + PAGE }>().0 {
            if frame[8..16] != wal[16..24] {
                break;
            }
            let pgno = be(frame);
            let mut page = frame[24..].to_vec();
            if pgno == 0 || !keys.open(&mut page, pgno) {
                break;
            }
            pending.push((pgno, page));
            let pages = be(&frame[4..]) as usize;
            if pages > 0 {
                plain.resize(pages * PAGE, 0);
                for (pgno, page) in pending.drain(..) {
                    let at = (pgno as usize - 1) * PAGE;
                    if at + PAGE <= plain.len() {
                        plain[at..at + PAGE].copy_from_slice(&page);
                    }
                }
            }
        }
    }
    plain[..SALT].copy_from_slice(b"SQLite format 3\0");
    // Read and write versions 1 (rollback journal): an in-memory database
    // cannot be in WAL mode.
    plain[18] = 1;
    plain[19] = 1;
    Ok(plain)
}

// ----- reading -------------------------------------------------------------

/// rekordbox's library as read.
#[derive(Clone, Debug, Default)]
pub struct RekordboxDb {
    /// Tracks (the whole collection, in rekordbox's order) and playlists.
    pub library: ForeignLibrary,
    /// rekordbox's id of each of `library.tracks`.
    pub ids: Vec<String>,
    /// A fingerprint of each track's data in rekordbox (tags, cues, grid
    /// file): unchanged means nothing to take over.
    pub fingerprints: Vec<u64>,
    /// Smart playlists (left out: rekordbox works out their tracks).
    pub smart_playlists: usize,
    /// Tracks (index into `library.tracks`) whose hot cues D-H the first
    /// version of this reader put on the wrong pad, with the cues as it
    /// gave them: where the library still has exactly those, they are
    /// replaced (see `fix_misplaced_cues`).
    pub misplaced_cues: Vec<(usize, Vec<Cue>)>,
}

/// Reads rekordbox's library in `dir` (its folder). Tracks whose
/// fingerprint is in `known` unchanged skip their grid file.
pub fn read_dir(dir: &Path, known: &HashMap<String, u64>) -> Result<RekordboxDb, String> {
    read_conn(&open_dir(dir)?, Some(dir), known)
}

/// rekordbox's library in `dir`, decrypted into memory and opened
/// read-only.
pub fn open_dir(dir: &Path) -> Result<Connection, String> {
    let path = dir.join(DB_FILE);
    let db = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let wal = std::fs::read(dir.join(format!("{DB_FILE}-wal"))).unwrap_or_default();
    let plain = decrypt(&db, &wal, passphrase()?.as_bytes())?;
    let mut conn = Connection::open_in_memory().map_err(|e| e.to_string())?;
    conn.deserialize_read_exact("main", plain.as_slice(), plain.len(), true)
        .map_err(|e| format!("rekordbox's library does not open: {e}"))?;
    Ok(conn)
}

/// FNV-1a: a fingerprint that stays the same across builds (it is kept in
/// the library).
struct Fnv(u64);

impl Fnv {
    fn new() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn add(&mut self, b: &[u8]) {
        for x in b.iter().chain(&[0x1f]) {
            self.0 ^= u64::from(*x);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

fn text(v: &Value) -> String {
    match v {
        Value::Text(s) => s.trim().to_string(),
        Value::Integer(i) => i.to_string(),
        Value::Real(f) => f.to_string(),
        _ => String::new(),
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Integer(i) => Some(*i as f64),
        Value::Real(f) => Some(*f),
        Value::Text(s) => s.trim().parse().ok(),
        _ => None,
    }
}

/// The colours rekordbox gives tracks, by name, in the values its XML
/// export writes for them.
fn track_colour(name: &str) -> Option<u32> {
    Some(match name.trim().to_ascii_lowercase().as_str() {
        "pink" => 0xFF007F,
        "red" => 0xFF0000,
        "orange" => 0xFFA500,
        "yellow" => 0xFFFF00,
        "green" => 0x00FF00,
        "aqua" => 0x25FDE9,
        "blue" => 0x0000FF,
        "purple" => 0x660099,
        _ => return None,
    })
}

/// An absolute file path (`C:/…`, `/Users/…`, `\\server\…`), not a
/// streaming service's track (`tidal:tracks:…`).
fn is_file_path(p: &str) -> bool {
    let b = p.as_bytes();
    p.starts_with('/')
        || p.starts_with("\\\\")
        || (b.len() > 2
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'/' | b'\\'))
}

/// The tables and their columns.
struct Schema(HashMap<String, HashSet<String>>);

impl Schema {
    fn read(conn: &Connection) -> rusqlite::Result<Self> {
        let mut st = conn.prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?;
        let tables: Vec<String> = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut out = HashMap::new();
        for t in tables {
            let mut st = conn.prepare(&format!("PRAGMA table_info(\"{t}\")"))?;
            let cols: HashSet<String> = st
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<_>>()?;
            out.insert(t, cols);
        }
        Ok(Schema(out))
    }

    fn has(&self, table: &str, col: &str) -> bool {
        self.0.get(table).is_some_and(|c| c.contains(col))
    }

    /// `alias.col`, or NULL when the column is not there.
    fn col(&self, table: &str, alias: &str, col: &str) -> String {
        if self.has(table, col) {
            format!("{alias}.\"{col}\"")
        } else {
            "NULL".into()
        }
    }

    /// The condition for rows not in rekordbox's trash.
    fn live(&self, table: &str, alias: &str) -> String {
        if self.has(table, "rb_local_deleted") {
            format!("COALESCE({alias}.rb_local_deleted, 0) = 0")
        } else {
            "1".into()
        }
    }

    /// `LEFT JOIN table alias ON alias.ID = c.idcol` and `alias.name`, or
    /// nothing and NULL when either is missing.
    fn join(&self, table: &str, alias: &str, idcol: &str, name: &str) -> (String, String) {
        if self.has(table, "ID") && self.has(table, name) && self.has("djmdContent", idcol) {
            (
                format!(" LEFT JOIN {table} {alias} ON {alias}.ID = c.\"{idcol}\""),
                format!("{alias}.\"{name}\""),
            )
        } else {
            (String::new(), "NULL".into())
        }
    }
}

fn rows(conn: &Connection, sql: &str, n: usize) -> Result<Vec<Vec<Value>>, String> {
    let mut st = conn.prepare(sql).map_err(|e| format!("{e} ({sql})"))?;
    let out = st
        .query_map([], |r| (0..n).map(|i| r.get::<_, Value>(i)).collect())
        .map_err(|e| e.to_string())?
        .collect::<rusqlite::Result<Vec<Vec<Value>>>>()
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Reads an opened (plain) rekordbox database. `dir`: rekordbox's folder,
/// for the grid files (`None`: no grids).
pub fn read_conn(
    conn: &Connection,
    dir: Option<&Path>,
    known: &HashMap<String, u64>,
) -> Result<RekordboxDb, String> {
    let s = Schema::read(conn).map_err(|e| e.to_string())?;
    for (t, c) in [("djmdContent", "FolderPath"), ("djmdContent", "ID")] {
        if !s.has(t, c) {
            return Err(format!("not a rekordbox library: no {t}.{c}"));
        }
    }
    let mut out = RekordboxDb {
        library: ForeignLibrary {
            source: "rekordbox".into(),
            ..ForeignLibrary::default()
        },
        ..RekordboxDb::default()
    };

    // Cues, by track.
    let mut cues: HashMap<String, Vec<Vec<Value>>> = HashMap::new();
    if s.has("djmdCue", "ContentID") && s.has("djmdCue", "InMsec") {
        let sql = format!(
            "SELECT q.ContentID, q.InMsec, {}, {}, {} FROM djmdCue q WHERE {} ORDER BY q.InMsec",
            s.col("djmdCue", "q", "OutMsec"),
            s.col("djmdCue", "q", "Kind"),
            s.col("djmdCue", "q", "Comment"),
            s.live("djmdCue", "q"),
        );
        for r in rows(conn, &sql, 5)? {
            cues.entry(text(&r[0])).or_default().push(r);
        }
    }

    // Tracks.
    let (j_artist, artist) = s.join("djmdArtist", "ar", "ArtistID", "Name");
    let (j_album, album) = s.join("djmdAlbum", "al", "AlbumID", "Name");
    let (j_genre, genre) = s.join("djmdGenre", "ge", "GenreID", "Name");
    let (j_key, key) = s.join("djmdKey", "k", "KeyID", "ScaleName");
    let (j_colour, colour) = s.join("djmdColor", "co", "ColorID", "Commnt");
    let c = |col| s.col("djmdContent", "c", col);
    let sql = format!(
        "SELECT c.ID, c.FolderPath, {}, {artist}, {album}, {genre}, {}, {}, {}, {}, {key}, {}, \
         {colour}, {}, {}, {}, {} FROM djmdContent c{j_artist}{j_album}{j_genre}{j_key}{j_colour} \
         WHERE {}",
        c("Title"),
        c("Commnt"),
        c("ReleaseYear"),
        c("Length"),
        c("BPM"),
        c("Rating"),
        c("DJPlayCount"),
        c("DateCreated"),
        c("AnalysisDataPath"),
        c("updated_at"),
        s.live("djmdContent", "c"),
    );
    let mut by_id: HashMap<String, usize> = HashMap::new();
    let mut not_files = 0;
    let mut tempo_changes = 0;
    for r in rows(conn, &sql, 17)? {
        let id = text(&r[0]);
        let Some(path) = Some(text(&r[1]))
            .filter(|p| is_file_path(p))
            .and_then(|p| url_to_path(&p))
        else {
            not_files += 1;
            continue;
        };
        let mut fp = Fnv::new();
        // The reader's version: a new one reads every track again once
        // (2: hot cues D-H on the right pads).
        fp.add(b"reader 2");
        for v in &r {
            fp.add(text(v).as_bytes());
        }
        let track_cues = cues.get(&id).map(Vec::as_slice).unwrap_or_default();
        for q in track_cues {
            for v in q {
                fp.add(text(v).as_bytes());
            }
        }
        // The grid file (its time and size count in the fingerprint).
        let anlz = dir
            .zip(Some(text(&r[15])).filter(|p| !p.is_empty()))
            .map(|(d, p)| d.join("share").join(p.trim_start_matches(['/', '\\'])));
        if let Some(m) = anlz.as_ref().and_then(|p| std::fs::metadata(p).ok()) {
            fp.add(&m.len().to_le_bytes());
            if let Some(t) = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            {
                fp.add(&t.as_nanos().to_le_bytes());
            }
        }
        let fingerprint = fp.0;
        let unchanged = known.get(&id) == Some(&fingerprint);

        let bpm = number(&r[9])
            .filter(|b| *b > 0.0)
            .map(|b| if b > 1000.0 { b / 100.0 } else { b });
        let rating = number(&r[11]).unwrap_or(0.0);
        let rating = if rating > 5.0 {
            (rating / 51.0).round()
        } else {
            rating
        };
        let mut t = ForeignTrack {
            path,
            title: text(&r[2]),
            artist: text(&r[3]),
            album: text(&r[4]),
            genre: text(&r[5]),
            comment: text(&r[6]),
            year: number(&r[7]).map(|y| y as i32).filter(|y| *y > 0),
            duration: number(&r[8]).unwrap_or(0.0).max(0.0),
            bpm,
            key: MusicalKey::parse(&text(&r[10])),
            rating: rating.clamp(0.0, 5.0) as u8,
            color: track_colour(&text(&r[12])),
            play_count: number(&r[13]).unwrap_or(0.0).max(0.0) as u32,
            date_added: parse_date(&text(&r[14])),
            ..ForeignTrack::default()
        };
        // Unchanged tracks are skipped by the sync: no need for their cues
        // and grid.
        if !unchanged {
            read_cues(&mut t, track_cues);
            let first = cues_with(track_cues, first_version_pad);
            if first != t.cues {
                out.misplaced_cues.push((out.library.tracks.len(), first));
            }
            if let Some(p) = &anlz
                && let Ok(b) = std::fs::read(p)
                && let Some((g, changes)) = grid_from_anlz(&b)
            {
                t.grid = Some(g);
                tempo_changes += usize::from(changes);
            }
        }
        by_id.insert(id.clone(), out.library.tracks.len());
        out.library.tracks.push(t);
        out.ids.push(id);
        out.fingerprints.push(fingerprint);
    }
    if not_files > 0 {
        out.library.warnings.push(format!(
            "{not_files} tracks left out: not files (streaming)"
        ));
    }
    if tempo_changes > 0 {
        out.library.warnings.push(format!(
            "{tempo_changes} tracks have a changing tempo: read as one grid at the first tempo"
        ));
    }

    // Playlists.
    if s.has("djmdPlaylist", "ID") && s.has("djmdPlaylist", "Name") {
        let sql = format!(
            "SELECT p.ID, p.Name, {}, {}, {} FROM djmdPlaylist p WHERE {}",
            s.col("djmdPlaylist", "p", "Attribute"),
            s.col("djmdPlaylist", "p", "ParentID"),
            s.col("djmdPlaylist", "p", "Seq"),
            s.live("djmdPlaylist", "p"),
        );
        let lists = rows(conn, &sql, 5)?;
        let mut entries: HashMap<String, Vec<usize>> = HashMap::new();
        if s.has("djmdSongPlaylist", "PlaylistID") && s.has("djmdSongPlaylist", "ContentID") {
            let sql = format!(
                "SELECT e.PlaylistID, e.ContentID FROM djmdSongPlaylist e WHERE {} \
                 ORDER BY e.PlaylistID, {}",
                s.live("djmdSongPlaylist", "e"),
                s.col("djmdSongPlaylist", "e", "TrackNo"),
            );
            for r in rows(conn, &sql, 2)? {
                if let Some(i) = by_id.get(&text(&r[1])) {
                    entries.entry(text(&r[0])).or_default().push(*i);
                }
            }
        }
        // Children of each parent, in rekordbox's order.
        let mut children: HashMap<String, Vec<(f64, String, String, i64)>> = HashMap::new();
        let ids: HashSet<String> = lists.iter().map(|r| text(&r[0])).collect();
        for r in &lists {
            let parent = text(&r[3]);
            let parent = if ids.contains(&parent) {
                parent
            } else {
                "root".to_string()
            };
            children.entry(parent).or_default().push((
                number(&r[4]).unwrap_or(0.0),
                text(&r[0]),
                text(&r[1]),
                number(&r[2]).unwrap_or(0.0) as i64,
            ));
        }
        for c in children.values_mut() {
            c.sort_by(|a, b| a.0.total_cmp(&b.0));
        }
        fn build(
            parent: &str,
            children: &HashMap<String, Vec<(f64, String, String, i64)>>,
            entries: &mut HashMap<String, Vec<usize>>,
            smart: &mut usize,
            depth: usize,
        ) -> Vec<ForeignNode> {
            let Some(list) = children.get(parent).filter(|_| depth < 32) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for (_, id, name, attribute) in list {
                match attribute {
                    1 => out.push(ForeignNode::Folder {
                        name: name.clone(),
                        children: build(id, children, entries, smart, depth + 1),
                    }),
                    0 => out.push(ForeignNode::Playlist {
                        name: name.clone(),
                        tracks: entries.remove(id).unwrap_or_default(),
                    }),
                    4 => *smart += 1,
                    _ => {}
                }
            }
            out
        }
        out.library.playlists = build("root", &children, &mut entries, &mut out.smart_playlists, 0);
    }
    Ok(out)
}

/// `djmdCue` rows (ContentID, InMsec, OutMsec, Kind, Comment) to cues and
/// loops.
/// The pad of a hot cue `Kind`: rekordbox numbers A-C 1-3 and D-H 5-9 (no
/// 4). 0 and anything else: a memory cue.
fn hot_cue_pad(kind: i64) -> Option<u8> {
    match kind {
        1..=3 => Some((kind - 1) as u8),
        5..=9 => Some((kind - 2) as u8),
        _ => None,
    }
}

/// The pad the first version of this reader gave a `Kind` (Kind - 1 for
/// 1-8), to recognise cues it put on the wrong pad.
fn first_version_pad(kind: i64) -> Option<u8> {
    (1..=8).contains(&kind).then(|| (kind - 1) as u8)
}

/// Cues read with `pad` (see [`read_cues`]).
fn cues_with(rows: &[Vec<Value>], pad: fn(i64) -> Option<u8>) -> Vec<Cue> {
    let mut t = ForeignTrack::default();
    read_cues_with(&mut t, rows, pad);
    t.cues
}

fn read_cues(t: &mut ForeignTrack, rows: &[Vec<Value>]) {
    read_cues_with(t, rows, hot_cue_pad);
}

fn read_cues_with(t: &mut ForeignTrack, rows: &[Vec<Value>], pad: fn(i64) -> Option<u8>) {
    let mut cues = Vec::new();
    let mut loops = Vec::new();
    for r in rows {
        let Some(start) = number(&r[1]).filter(|s| *s >= 0.0).map(|ms| ms / 1000.0) else {
            continue;
        };
        let end = number(&r[2]).filter(|e| *e >= 0.0).map(|ms| ms / 1000.0);
        let kind = number(&r[3]).unwrap_or(0.0) as i64;
        let slot = pad(kind);
        let name = text(&r[4]);
        match end.filter(|e| *e > start) {
            Some(end) => loops.push((
                slot,
                SavedLoop {
                    slot: 0,
                    start,
                    end,
                    name,
                    color: None,
                },
            )),
            None => cues.push(Cue {
                kind: slot.map_or(CueKind::Memory, CueKind::Hot),
                position: start,
                name,
                color: None,
            }),
        }
    }
    t.cues = tidy_cues(cues);
    t.loops = slot_loops(loops).0;
}

/// The beat grid in an ANLZ file's `PQTZ` section (each beat: its number
/// in the bar, the tempo in centi-BPM, its time in ms; big-endian), and
/// whether the tempo changes. A steady tempo is measured over the whole
/// track (the stored tempo is rounded to 0.01 BPM).
fn grid_from_anlz(b: &[u8]) -> Option<(dj_core::Beatgrid, bool)> {
    let be32 = |at: usize| {
        b.get(at..at + 4)
            .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    };
    let be16 = |at: usize| b.get(at..at + 2).map(|s| u16::from_be_bytes([s[0], s[1]]));
    if b.get(..4) != Some(b"PMAI") {
        return None;
    }
    let mut pos = be32(4)? as usize;
    while pos + 12 <= b.len() {
        let tag = &b[pos..pos + 4];
        let hlen = be32(pos + 4)? as usize;
        let len = be32(pos + 8)? as usize;
        if len < 12 || hlen < 12 || hlen > len || pos + len > b.len() {
            return None;
        }
        if tag == b"PQTZ" {
            let n = be32(pos + 0x14)? as usize;
            if n == 0 || hlen + n * 8 > len {
                return None;
            }
            let beat = |i: usize| {
                let e = pos + hlen + i * 8;
                Some((
                    be16(e)?,
                    f64::from(be16(e + 2)?) / 100.0,
                    f64::from(be32(e + 4)?),
                ))
            };
            let (number0, tempo0, time0) = beat(0)?;
            if tempo0 <= 0.0 {
                return None;
            }
            let (_, _, time_last) = beat(n - 1)?;
            let changes =
                (1..n).any(|i| beat(i).is_some_and(|(_, t, _)| (t - tempo0).abs() > 0.01));
            let bpm = if !changes && n > 1 && time_last > time0 {
                let measured = 60_000.0 * (n - 1) as f64 / (time_last - time0);
                if (measured - tempo0).abs() < 0.01 {
                    tempo0
                } else {
                    measured
                }
            } else {
                tempo0
            };
            return Some((grid_at(time0 / 1000.0, bpm, i64::from(number0)), changes));
        }
        pos += len;
    }
    None
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_key_decodes_to_64_hex_digits() {
        let k = passphrase().unwrap();
        assert_eq!(k.len(), 64, "{k}");
        assert!(k.bytes().all(|b| b.is_ascii_hexdigit()));
    }

    #[test]
    fn base85_like_python() {
        // base64.b85encode(b"hello") == b"Xk~0{Zv"
        assert_eq!(b85decode("Xk~0{Zv").unwrap(), b"hello");
        assert_eq!(b85decode("").unwrap(), b"");
        assert!(b85decode("\"").is_none());
    }

    /// SQLCipher 4 encryption of one page (the inverse of `Keys::open`).
    fn seal(keys: &Keys, page: &mut [u8], pgno: u32, iv: u8) {
        use cbc::cipher::block_padding::NoPadding;
        use cbc::cipher::{BlockModeEncrypt, KeyIvInit};
        let start = if pgno == 1 { SALT } else { 0 };
        let len = PAGE - start - RESERVE;
        page[PAGE - RESERVE..PAGE - MAC].fill(iv);
        let ivb: Vec<u8> = page[PAGE - RESERVE..PAGE - MAC].to_vec();
        cbc::Encryptor::<aes::Aes256>::new_from_slices(&keys.cipher, &ivb)
            .unwrap()
            .encrypt_padded::<NoPadding>(&mut page[start..start + len], len)
            .unwrap();
        let mac = keys.mac(&page[start..PAGE - MAC], pgno);
        page[PAGE - MAC..].copy_from_slice(&mac);
    }

    #[test]
    fn decrypts_pages_and_applies_committed_wal_frames() {
        let pass = b"test passphrase";
        let salt = [7u8; SALT];
        let keys = Keys::new(pass, &salt);
        // Two plain pages (page 1 keeps its salt in place of the header).
        let mut plain = vec![0u8; 2 * PAGE];
        for (i, b) in plain.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let mut db = plain.clone();
        db[..SALT].copy_from_slice(&salt);
        seal(&keys, &mut db[..PAGE], 1, 1);
        seal(&keys, &mut db[PAGE..], 2, 2);
        // A WAL: page 2 replaced and committed (2 pages), then page 1
        // replaced without a commit (ignored), then a frame with another
        // salt (an old log, ignored).
        let mut wal = vec![0u8; 32];
        wal[8..12].copy_from_slice(&(PAGE as u32).to_be_bytes());
        wal[16..24].copy_from_slice(&[9; 8]);
        let frame = |pgno: u32, commit: u32, salt: [u8; 8], fill: u8, iv: u8| {
            let mut f = vec![0u8; 24 + PAGE];
            f[..4].copy_from_slice(&pgno.to_be_bytes());
            f[4..8].copy_from_slice(&commit.to_be_bytes());
            f[8..16].copy_from_slice(&salt);
            f[24..].fill(fill);
            seal(&keys, &mut f[24..], pgno, iv);
            f
        };
        wal.extend(frame(2, 2, [9; 8], 0xAB, 3));
        wal.extend(frame(1, 0, [9; 8], 0xCD, 4));
        wal.extend(frame(2, 2, [1; 8], 0xEF, 5));

        let out = decrypt(&db, &wal, pass).unwrap();
        assert_eq!(out.len(), 2 * PAGE);
        assert_eq!(&out[..16], b"SQLite format 3\0");
        assert_eq!(&out[16..18], &plain[16..18]);
        assert_eq!((out[18], out[19]), (1, 1));
        assert_eq!(&out[20..PAGE - RESERVE], &plain[20..PAGE - RESERVE]);
        assert!(
            out[PAGE..2 * PAGE - RESERVE].iter().all(|b| *b == 0xAB),
            "the committed frame"
        );

        // A wrong passphrase is refused, not read as garbage.
        let err = decrypt(&db, &[], b"wrong").unwrap_err();
        assert!(err.contains("known key"), "{err}");
    }

    fn anlz(beats: &[(u16, u16, u32)]) -> Vec<u8> {
        let mut pqtz = Vec::new();
        pqtz.extend_from_slice(b"PQTZ");
        pqtz.extend_from_slice(&24u32.to_be_bytes());
        pqtz.extend_from_slice(&((24 + beats.len() * 8) as u32).to_be_bytes());
        pqtz.extend_from_slice(&[0; 8]);
        pqtz.extend_from_slice(&(beats.len() as u32).to_be_bytes());
        for (n, t, ms) in beats {
            pqtz.extend_from_slice(&n.to_be_bytes());
            pqtz.extend_from_slice(&t.to_be_bytes());
            pqtz.extend_from_slice(&ms.to_be_bytes());
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"PMAI");
        f.extend_from_slice(&28u32.to_be_bytes());
        f.extend_from_slice(&((28 + pqtz.len()) as u32).to_be_bytes());
        f.extend_from_slice(&[0; 16]);
        f.extend(pqtz);
        f
    }

    #[test]
    fn grids_from_anlz() {
        // 128 BPM, the first beat at 120 ms is beat 2 of its bar.
        let beats: Vec<(u16, u16, u32)> = (0..64u32)
            .map(|i| {
                (
                    ((i + 1) % 4 + 1) as u16,
                    12800,
                    120 + (i as f64 * 468.75).round() as u32,
                )
            })
            .collect();
        let (g, changes) = grid_from_anlz(&anlz(&beats)).unwrap();
        assert!(!changes);
        assert_eq!(g.bpm, 128.0);
        assert!((g.first_beat - 0.12).abs() < 1e-9);
        assert_eq!(g.first_beat_in_bar, 2);
        // A changing tempo is noted.
        let (_, changes) = grid_from_anlz(&anlz(&[(1, 12000, 0), (2, 12100, 500)])).unwrap();
        assert!(changes);
        assert!(grid_from_anlz(b"PMAI").is_none());
        assert!(grid_from_anlz(&anlz(&[])).is_none());
    }

    /// A small plain database with rekordbox's tables.
    pub(crate) fn sample() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            r#"
            CREATE TABLE djmdContent (ID VARCHAR, FolderPath VARCHAR, Title VARCHAR,
                ArtistID VARCHAR, AlbumID VARCHAR, GenreID VARCHAR, KeyID VARCHAR, ColorID VARCHAR,
                Commnt VARCHAR, ReleaseYear INTEGER, Length INTEGER, BPM INTEGER, Rating INTEGER,
                DJPlayCount INTEGER, DateCreated VARCHAR, AnalysisDataPath VARCHAR,
                updated_at VARCHAR, rb_local_deleted INTEGER DEFAULT 0);
            CREATE TABLE djmdArtist (ID VARCHAR, Name VARCHAR);
            CREATE TABLE djmdKey (ID VARCHAR, ScaleName VARCHAR);
            CREATE TABLE djmdColor (ID VARCHAR, Commnt VARCHAR);
            CREATE TABLE djmdCue (ID VARCHAR, ContentID VARCHAR, InMsec INTEGER, OutMsec INTEGER,
                Kind INTEGER, Comment VARCHAR, rb_local_deleted INTEGER DEFAULT 0);
            CREATE TABLE djmdPlaylist (ID VARCHAR, Seq INTEGER, Name VARCHAR, Attribute INTEGER,
                ParentID VARCHAR, rb_local_deleted INTEGER DEFAULT 0);
            CREATE TABLE djmdSongPlaylist (ID VARCHAR, PlaylistID VARCHAR, ContentID VARCHAR,
                TrackNo INTEGER, rb_local_deleted INTEGER DEFAULT 0);
            INSERT INTO djmdArtist VALUES ('a1', 'Lumen');
            INSERT INTO djmdKey VALUES ('k1', 'Am');
            INSERT INTO djmdColor VALUES ('c1', 'Red');
            INSERT INTO djmdContent (ID, FolderPath, Title, ArtistID, KeyID, ColorID, Length, BPM,
                Rating, DJPlayCount, DateCreated)
                VALUES ('1', 'C:/Music/Night Drive.mp3', 'Night Drive', 'a1', 'k1', 'c1', 245, 12400,
                4, 7, '2024-05-01');
            INSERT INTO djmdContent (ID, FolderPath, Title, BPM) VALUES ('2', 'C:/Music/b.flac', 'B', 9000);
            INSERT INTO djmdContent (ID, FolderPath, Title) VALUES ('3', 'tidal:tracks:123', 'Stream');
            INSERT INTO djmdContent (ID, FolderPath, Title, rb_local_deleted)
                VALUES ('4', 'C:/Music/trash.mp3', 'In the trash', 1);
            INSERT INTO djmdCue (ContentID, InMsec, OutMsec, Kind, Comment) VALUES
                ('1', 32000, -1, 1, 'Drop'), ('1', 64000, -1, 0, ''),
                ('1', 96000, 103742, 3, 'Loop'), ('1', 1000, -1, 2, 'gone'),
                ('1', 120000, -1, 5, 'D'), ('1', 150000, -1, 9, 'H');
            UPDATE djmdCue SET rb_local_deleted = 1 WHERE Comment = 'gone';
            INSERT INTO djmdPlaylist VALUES
                ('10', 1, 'Gigs', 1, 'root', 0), ('11', 1, 'Friday', 0, '10', 0),
                ('12', 2, 'Warm-up', 0, 'root', 0), ('13', 0, 'Smart', 4, 'root', 0),
                ('14', 3, 'Deleted', 0, 'root', 1);
            INSERT INTO djmdSongPlaylist (PlaylistID, ContentID, TrackNo) VALUES
                ('11', '2', 1), ('11', '1', 2), ('12', '1', 1), ('11', '3', 3);
            "#,
        )
        .unwrap();
        c
    }

    #[test]
    fn reads_tracks_cues_and_playlists() {
        let db = read_conn(&sample(), None, &HashMap::new()).unwrap();
        let lib = &db.library;
        assert_eq!(db.ids, ["1", "2"], "the stream and the trash are left out");
        assert_eq!(db.fingerprints.len(), 2);
        let t = &lib.tracks[0];
        assert_eq!(t.path, PathBuf::from("C:\\Music\\Night Drive.mp3"));
        assert_eq!(
            (t.title.as_str(), t.artist.as_str()),
            ("Night Drive", "Lumen")
        );
        assert_eq!(t.bpm, Some(124.0));
        assert_eq!(lib.tracks[1].bpm, Some(90.0));
        assert_eq!(t.key, MusicalKey::parse("Am"));
        assert_eq!((t.rating, t.play_count, t.duration), (4, 7, 245.0));
        assert_eq!(t.color, Some(0xFF0000));
        assert_eq!(t.date_added, Some(1_714_521_600));
        assert_eq!(
            t.cues.len(),
            4,
            "hot cues A, D, H and a memory cue; the deleted one left out"
        );
        // rekordbox's Kind 5 is D (pad 4), 9 is H (pad 8): there is no 4.
        let pad = |name: &str| t.cues.iter().find(|c| c.name == name).map(|c| c.kind);
        assert_eq!(pad("D"), Some(CueKind::Hot(3)));
        assert_eq!(pad("H"), Some(CueKind::Hot(7)));
        // The first version's pads are kept, to put them right.
        assert_eq!(db.misplaced_cues.len(), 1);
        let (i, was) = &db.misplaced_cues[0];
        assert_eq!(*i, 0);
        assert!(
            was.iter()
                .any(|c| c.name == "D" && c.kind == CueKind::Hot(4))
        );
        assert!(
            was.iter()
                .any(|c| c.name == "H" && c.kind == CueKind::Memory)
        );
        assert!(
            t.cues
                .iter()
                .any(|c| c.kind == CueKind::Hot(0) && c.name == "Drop" && c.position == 32.0)
        );
        assert!(
            t.cues
                .iter()
                .any(|c| c.kind == CueKind::Memory && c.position == 64.0)
        );
        assert_eq!(t.loops.len(), 1);
        assert_eq!((t.loops[0].slot, t.loops[0].end), (2, 103.742));
        assert!(lib.warnings.iter().any(|w| w.contains("1 tracks left out")));
        assert_eq!(db.smart_playlists, 1);
        assert_eq!(
            lib.playlists,
            vec![
                ForeignNode::Folder {
                    name: "Gigs".into(),
                    children: vec![ForeignNode::Playlist {
                        name: "Friday".into(),
                        tracks: vec![1, 0],
                    }],
                },
                ForeignNode::Playlist {
                    name: "Warm-up".into(),
                    tracks: vec![0],
                },
            ]
        );
        // Known and unchanged: cues are not read again.
        let known: HashMap<String, u64> = db
            .ids
            .iter()
            .cloned()
            .zip(db.fingerprints.iter().copied())
            .collect();
        let again = read_conn(&sample(), None, &known).unwrap();
        assert_eq!(again.fingerprints, db.fingerprints);
        assert!(again.library.tracks[0].cues.is_empty());
    }
}
