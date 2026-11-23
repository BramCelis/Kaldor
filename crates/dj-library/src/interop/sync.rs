//! Keeping the library in step with another program's library (the
//! rekordbox tab): run again whenever the other library changes.
//!
//! - Tracks are merged as by Import from… ([`super`]): a new track comes in
//!   with the other program's tags, cues, loops and grid; a track already in
//!   Kaldor keeps the work done in Kaldor. Each track's id in the other
//!   program and a fingerprint of its data there are kept
//!   (`synced_tracks`): a track that did not change there is skipped, and a
//!   track moved to another file there is pointed at the new file here.
//! - The playlists become crates under one root crate marked as synced
//!   (`crates.synced`), holding the whole collection. The tree under it is
//!   made the same as the other program's: crates made, put in order,
//!   filled in the playlist's order and removed with it. These crates are
//!   not the user's: the crate list shows them in their own tab, read-only.
//! - Tracks the other program no longer has stay in the library (they are
//!   only dropped from its crates).

use super::{ForeignLibrary, ForeignNode, InteropOptions, InteropReport};
use crate::db::{Library, Result};
use crate::model::{CrateId, TrackId};
use rusqlite::{OptionalExtension, params};
use std::collections::HashMap;

/// What a sync did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub tracks: usize,
    pub tracks_added: usize,
    /// Tracks taken over again because they changed in the other program.
    pub tracks_updated: usize,
    /// Tracks pointed at another file (moved in the other program).
    pub tracks_moved: usize,
    pub missing_files: usize,
    /// Crates made, filled differently, moved or removed.
    pub crates_changed: usize,
    pub playlists: usize,
    /// The tracks added whose file is there (to analyse).
    pub to_analyse: Vec<TrackId>,
}

impl SyncReport {
    /// Anything changed in the library.
    pub fn changed(&self) -> bool {
        self.tracks_added + self.tracks_updated + self.tracks_moved + self.crates_changed > 0
    }
}

impl Library {
    /// The fingerprints last taken over from `source`, by its track id.
    pub fn synced_fingerprints(&self, source: &str) -> Result<HashMap<String, u64>> {
        let mut st = self
            .conn
            .prepare("SELECT source_id, fingerprint FROM synced_tracks WHERE source = ?1")?;
        let rows = st.query_map([source], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The root crate of `source`'s synced crates, if it was synced.
    pub fn synced_root(&self, source: &str) -> Result<Option<CrateId>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM crates WHERE parent_id IS NULL AND synced = ?1 \
                 ORDER BY id LIMIT 1",
                [source],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Removes `source`'s synced crates and its track ids (the tracks
    /// stay in the library).
    pub fn forget_synced(&mut self, source: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM crates WHERE synced = ?1", [source])?;
        self.conn
            .execute("DELETE FROM synced_tracks WHERE source = ?1", [source])?;
        Ok(())
    }

    /// Brings `f` (read from `source`) into the library, see the module
    /// docs. `ids` and `fingerprints` go with `f.tracks` (tracks past the
    /// end of either are left out). The root crate is
    /// called `root_name` when it is made.
    pub fn sync_foreign(
        &mut self,
        source: &str,
        root_name: &str,
        f: &ForeignLibrary,
        ids: &[String],
        fingerprints: &[u64],
    ) -> Result<SyncReport> {
        let mut report = SyncReport {
            tracks: f.tracks.len(),
            ..SyncReport::default()
        };
        let known: HashMap<String, (TrackId, u64)> = {
            let mut st = self.conn.prepare(
                "SELECT source_id, track_id, fingerprint FROM synced_tracks WHERE source = ?1",
            )?;
            let rows = st.query_map([source], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    (r.get(1)?, r.get::<_, i64>(2)? as u64),
                ))
            })?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let opts = InteropOptions::default();
        let mut track_ids = Vec::with_capacity(f.tracks.len());
        for ((t, sid), fp) in f.tracks.iter().zip(ids).zip(fingerprints) {
            let mapped = known.get(sid).copied();
            if let Some((id, old)) = mapped
                && old == *fp
                && self.track(id)?.is_some()
            {
                track_ids.push(id);
                continue;
            }
            // Moved to another file in the other program: follow it, unless
            // that file is a track of its own here.
            if let Some((id, _)) = mapped
                && let Some(here) = self.track(id)?
                && here.meta.path != t.path
                && self.track_by_path(&t.path)?.is_none()
            {
                self.relocate(id, &t.path)?;
                if !t.path.exists() {
                    self.set_missing(id, true)?;
                }
                report.tracks_moved += 1;
            }
            let mut merged = InteropReport::default();
            let id = self.merge_track_untagged(t, &opts, &mut merged)?;
            if merged.tracks_added > 0 {
                report.tracks_added += 1;
            } else if mapped.is_some() {
                report.tracks_updated += 1;
            }
            report.missing_files += merged.missing_files;
            report.to_analyse.extend(merged.to_analyse);
            self.conn.execute(
                "INSERT OR REPLACE INTO synced_tracks (source, source_id, track_id, fingerprint) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![source, sid, id, *fp as i64],
            )?;
            track_ids.push(id);
        }
        // Track ids the other program no longer has.
        let live: std::collections::HashSet<&str> = ids.iter().map(String::as_str).collect();
        for sid in known.keys().filter(|s| !live.contains(s.as_str())) {
            self.conn.execute(
                "DELETE FROM synced_tracks WHERE source = ?1 AND source_id = ?2",
                params![source, sid],
            )?;
        }

        // The crates.
        let root = match self.synced_root(source)? {
            Some(r) => r,
            None => {
                let r = self.create_crate(root_name, None)?;
                self.mark_synced(r, source)?;
                report.crates_changed += 1;
                r
            }
        };
        if self.set_crate_exactly(root, &track_ids)? {
            report.crates_changed += 1;
        }
        self.mirror_nodes(source, &f.playlists, root, &track_ids, &mut report, 0)?;
        Ok(report)
    }

    /// Puts right cues an earlier reader placed wrongly: for each track
    /// (index into `f.tracks`) whose cues in the library are still exactly
    /// the wrong ones (`was`), takes `f`'s. Cues changed in Kaldor since
    /// stay. Returns how many tracks were put right.
    pub fn fix_misplaced_cues(
        &mut self,
        f: &ForeignLibrary,
        misplaced: &[(usize, Vec<crate::Cue>)],
    ) -> Result<usize> {
        fn sorted(mut c: Vec<crate::Cue>) -> Vec<crate::Cue> {
            c.sort_by(|a, b| {
                a.position
                    .total_cmp(&b.position)
                    .then_with(|| format!("{:?}", a.kind).cmp(&format!("{:?}", b.kind)))
            });
            c
        }
        let mut fixed = 0;
        for (i, was) in misplaced {
            let Some(t) = f.tracks.get(*i) else { continue };
            let Some(here) = self.track_by_path(&t.path)? else {
                continue;
            };
            if sorted(self.cues(here.id)?) == sorted(was.clone()) {
                self.set_cues(here.id, &t.cues)?;
                fixed += 1;
            }
        }
        Ok(fixed)
    }

    fn mark_synced(&mut self, c: CrateId, source: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE crates SET synced = ?1 WHERE id = ?2",
            params![source, c],
        )?;
        Ok(())
    }

    /// Makes crate `c` hold exactly `tracks` in that order (a track listed
    /// twice once). True when it changed.
    fn set_crate_exactly(&mut self, c: CrateId, tracks: &[TrackId]) -> Result<bool> {
        let mut seen = std::collections::HashSet::new();
        let want: Vec<TrackId> = tracks.iter().copied().filter(|t| seen.insert(*t)).collect();
        if self.crate_tracks(c)? == want {
            return Ok(false);
        }
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM crate_tracks WHERE crate_id = ?1", [c])?;
        for (pos, t) in want.iter().enumerate() {
            tx.execute(
                "INSERT INTO crate_tracks (crate_id, track_id, position) VALUES (?1, ?2, ?3)",
                params![c, t, pos as i64],
            )?;
        }
        tx.commit()?;
        Ok(true)
    }

    fn mirror_nodes(
        &mut self,
        source: &str,
        nodes: &[ForeignNode],
        parent: CrateId,
        ids: &[TrackId],
        report: &mut SyncReport,
        depth: usize,
    ) -> Result<()> {
        if depth > 32 {
            return Ok(());
        }
        let existing: Vec<(CrateId, String, i64)> = {
            let mut st = self.conn.prepare(
                "SELECT id, name, position FROM crates WHERE parent_id = ?1 ORDER BY position, id",
            )?;
            st.query_map([parent], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?
        };
        let mut used = vec![false; existing.len()];
        for (pos, n) in nodes.iter().enumerate() {
            let (name, tracks, children) = match n {
                ForeignNode::Folder { name, children } => (name, &[][..], Some(children)),
                ForeignNode::Playlist { name, tracks } => (name, &tracks[..], None),
            };
            let name = match name.trim() {
                "" => "Untitled",
                n => n,
            };
            let found = existing
                .iter()
                .enumerate()
                .find(|(k, e)| !used[*k] && e.1 == name)
                .map(|(k, e)| (k, e.0, e.2));
            let c = match found {
                Some((k, id, at)) => {
                    used[k] = true;
                    if at != pos as i64 {
                        self.conn.execute(
                            "UPDATE crates SET position = ?1 WHERE id = ?2",
                            params![pos as i64, id],
                        )?;
                        report.crates_changed += 1;
                    }
                    id
                }
                None => {
                    let id = self.create_crate(name, Some(parent))?;
                    self.conn.execute(
                        "UPDATE crates SET position = ?1, synced = ?2 WHERE id = ?3",
                        params![pos as i64, source, id],
                    )?;
                    report.crates_changed += 1;
                    id
                }
            };
            let list: Vec<TrackId> = tracks.iter().filter_map(|i| ids.get(*i).copied()).collect();
            if self.set_crate_exactly(c, &list)? {
                report.crates_changed += 1;
            }
            match children {
                Some(ch) => self.mirror_nodes(source, ch, c, ids, report, depth + 1)?,
                None => {
                    report.playlists += 1;
                    // A folder that became a playlist: its old sub-crates go.
                    self.mirror_nodes(source, &[], c, ids, report, depth + 1)?;
                }
            }
        }
        for (k, e) in existing.iter().enumerate() {
            if !used[k] {
                self.delete_crate(e.0)?;
                report.crates_changed += 1;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::rekordbox_db::{read_conn, tests::sample};
    use super::*;

    fn tree(lib: &Library) -> Vec<(String, Vec<String>)> {
        let root = lib.synced_root("rekordbox").unwrap().unwrap();
        let mut out = Vec::new();
        fn walk(
            lib: &Library,
            nodes: &[crate::CrateNode],
            path: &str,
            out: &mut Vec<(String, Vec<String>)>,
        ) {
            for n in nodes {
                let p = format!("{path}/{}", n.name);
                let titles = lib
                    .crate_tracks(n.id)
                    .unwrap()
                    .into_iter()
                    .map(|t| lib.track(t).unwrap().unwrap().meta.title)
                    .collect();
                out.push((p.clone(), titles));
                walk(lib, &n.children, &p, out);
            }
        }
        let all = lib.crate_tree().unwrap();
        let r = all.iter().find(|c| c.id == root).unwrap();
        assert!(r.synced);
        walk(lib, std::slice::from_ref(r), "", &mut out);
        out
    }

    fn sync(lib: &mut Library, conn: &rusqlite::Connection) -> SyncReport {
        let known = lib.synced_fingerprints("rekordbox").unwrap();
        let db = read_conn(conn, None, &known).unwrap();
        lib.sync_foreign(
            "rekordbox",
            "rekordbox",
            &db.library,
            &db.ids,
            &db.fingerprints,
        )
        .unwrap()
    }

    #[test]
    fn mirrors_rekordbox_and_follows_its_changes() {
        let mut lib = Library::open_in_memory().unwrap();
        // A crate of the user's own with the same name as the root is not
        // touched.
        let own = lib.create_crate("rekordbox", None).unwrap();
        let rb = sample();
        let r = sync(&mut lib, &rb);
        assert_eq!((r.tracks, r.tracks_added, r.playlists), (2, 2, 2));
        assert_eq!(
            r.missing_files, 2,
            "the sample's files are not on this computer"
        );
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert_eq!(
            tree(&lib),
            vec![
                ("/rekordbox".into(), s(&["Night Drive", "B"])),
                ("/rekordbox/Gigs".into(), vec![]),
                ("/rekordbox/Gigs/Friday".into(), s(&["B", "Night Drive"])),
                ("/rekordbox/Warm-up".into(), s(&["Night Drive"])),
            ]
        );
        let id = lib
            .track_by_path(std::path::Path::new("C:\\Music\\Night Drive.mp3"))
            .unwrap()
            .unwrap()
            .id;
        assert_eq!(lib.cues(id).unwrap().len(), 4);
        assert!(lib.crate_tracks(own).unwrap().is_empty());
        // The import's root crate is the user's, not the synced one.
        assert_eq!(lib.child_crate(None, "rekordbox").unwrap(), Some(own));

        // Nothing changed: nothing to do.
        let r = sync(&mut lib, &rb);
        assert!(!r.changed(), "{r:?}");

        // Changes in rekordbox: a playlist renamed, one reordered, one
        // removed, a new one, a track moved to another file.
        rb.execute_batch(
            "UPDATE djmdPlaylist SET Name = 'Friday late' WHERE ID = '11';
             UPDATE djmdSongPlaylist SET TrackNo = 9 WHERE PlaylistID = '11' AND ContentID = '2';
             UPDATE djmdPlaylist SET rb_local_deleted = 1 WHERE ID = '12';
             INSERT INTO djmdPlaylist VALUES ('15', 0, 'Peak', 0, 'root', 0);
             INSERT INTO djmdSongPlaylist (PlaylistID, ContentID, TrackNo) VALUES ('15', '2', 1);
             UPDATE djmdContent SET FolderPath = 'D:/Moved/b.flac' WHERE ID = '2';",
        )
        .unwrap();
        let r = sync(&mut lib, &rb);
        assert_eq!((r.tracks_added, r.tracks_moved), (0, 1), "{r:?}");
        assert_eq!(
            tree(&lib),
            vec![
                ("/rekordbox".into(), s(&["Night Drive", "B"])),
                ("/rekordbox/Peak".into(), s(&["B"])),
                ("/rekordbox/Gigs".into(), vec![]),
                (
                    "/rekordbox/Gigs/Friday late".into(),
                    s(&["Night Drive", "B"])
                ),
            ]
        );
        assert_eq!(lib.tracks().unwrap().len(), 2, "moved, not added twice");
        assert!(
            lib.track_by_path(std::path::Path::new("D:\\Moved\\b.flac"))
                .unwrap()
                .is_some()
        );

        // Kaldor's own cues stay when rekordbox's change.
        lib.set_cues(id, &[]).unwrap();
        let mine = vec![crate::Cue {
            kind: crate::CueKind::Hot(5),
            position: 1.0,
            name: "mine".into(),
            color: None,
        }];
        lib.set_cues(id, &mine).unwrap();
        rb.execute_batch("UPDATE djmdCue SET InMsec = 33000 WHERE Comment = 'Drop';")
            .unwrap();
        let r = sync(&mut lib, &rb);
        assert_eq!(r.tracks_updated, 1);
        assert_eq!(lib.cues(id).unwrap(), mine);

        // Cues the first reader put on the wrong pads are put right, unless
        // they were changed in Kaldor since.
        let db = read_conn(&rb, None, &HashMap::new()).unwrap();
        let (_, was) = db.misplaced_cues[0].clone();
        lib.set_cues(id, &was).unwrap();
        assert_eq!(
            lib.fix_misplaced_cues(&db.library, &db.misplaced_cues)
                .unwrap(),
            1
        );
        assert_eq!(lib.cues(id).unwrap().len(), db.library.tracks[0].cues.len());
        assert!(
            lib.cues(id)
                .unwrap()
                .iter()
                .any(|c| c.name == "D" && c.kind == crate::CueKind::Hot(3))
        );
        lib.set_cues(id, &mine).unwrap();
        assert_eq!(
            lib.fix_misplaced_cues(&db.library, &db.misplaced_cues)
                .unwrap(),
            0
        );
        assert_eq!(lib.cues(id).unwrap(), mine);

        // Forgetting the sync removes its crates, not the tracks.
        lib.forget_synced("rekordbox").unwrap();
        assert!(lib.synced_root("rekordbox").unwrap().is_none());
        assert_eq!(lib.tracks().unwrap().len(), 2);
        assert!(lib.synced_fingerprints("rekordbox").unwrap().is_empty());
    }
}
