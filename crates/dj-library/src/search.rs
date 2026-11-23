//! In-memory search index. Plain substring matching over a lower-cased
//! text per track is fast enough for 100,000 tracks (see the benchmark test)
//! and needs no extra database features.
//!
//! Query syntax: words (all must match artist, title, album, genre or
//! comment), `bpm:128` (±0.5), `bpm:120-130`, `key:8A` (any notation).

use crate::model::{Track, TrackId};
use dj_core::MusicalKey;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub terms: Vec<String>,
    pub bpm: Option<(f64, f64)>,
    pub key: Option<MusicalKey>,
}

impl Query {
    pub fn parse(s: &str) -> Self {
        let mut q = Query::default();
        for tok in s.split_whitespace() {
            let lower = tok.to_lowercase();
            if let Some(v) = lower.strip_prefix("bpm:") {
                let range = match v.split_once('-') {
                    Some((a, b)) => a.parse().ok().zip(b.parse().ok()),
                    None => v.parse::<f64>().ok().map(|x| (x - 0.5, x + 0.5)),
                };
                if let Some((a, b)) = range {
                    q.bpm = Some((f64::min(a, b), f64::max(a, b)));
                    continue;
                }
            }
            if let Some(v) = tok.strip_prefix("key:").or(tok.strip_prefix("KEY:"))
                && let Some(k) = MusicalKey::parse(v)
            {
                q.key = Some(k);
                continue;
            }
            q.terms.push(lower);
        }
        q
    }

    pub fn is_empty(&self) -> bool {
        self.terms.is_empty() && self.bpm.is_none() && self.key.is_none()
    }
}

struct Entry {
    id: TrackId,
    text: String,
    bpm: Option<f64>,
    key: Option<MusicalKey>,
}

/// Search index over all tracks.
pub struct SearchIndex {
    entries: Vec<Entry>,
}

impl SearchIndex {
    pub fn build(tracks: &[Track]) -> Self {
        let entries = tracks
            .iter()
            .map(|t| {
                let m = &t.meta;
                let mut text = String::with_capacity(
                    m.artist.len()
                        + m.title.len()
                        + m.album.len()
                        + m.genre.len()
                        + m.comment.len()
                        + 8,
                );
                for f in [&m.artist, &m.title, &m.album, &m.genre, &m.comment] {
                    text.push_str(&f.to_lowercase());
                    text.push('\u{1}');
                }
                if m.title.is_empty()
                    && let Some(stem) = m.path.file_stem()
                {
                    text.push_str(&stem.to_string_lossy().to_lowercase());
                }
                Entry {
                    id: t.id,
                    text,
                    bpm: t.bpm,
                    key: t.key,
                }
            })
            .collect();
        Self { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Matching track ids, in index order.
    pub fn search(&self, q: &Query) -> Vec<TrackId> {
        self.entries
            .iter()
            .filter(|e| {
                q.bpm
                    .is_none_or(|(a, b)| e.bpm.is_some_and(|x| x >= a && x <= b))
                    && q.key.is_none_or(|k| e.key == Some(k))
                    && q.terms.iter().all(|t| e.text.contains(t.as_str()))
            })
            .map(|e| e.id)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_filters() {
        let q = Query::parse("Daft  bpm:120-130 key:8A punk");
        assert_eq!(q.terms, vec!["daft", "punk"]);
        assert_eq!(q.bpm, Some((120.0, 130.0)));
        assert_eq!(q.key, Some(MusicalKey::new(9, true)));
        let q = Query::parse("bpm:128");
        assert_eq!(q.bpm, Some((127.5, 128.5)));
        // A malformed filter is searched as text.
        assert_eq!(Query::parse("bpm:fast").terms, vec!["bpm:fast"]);
        assert!(Query::parse("   ").is_empty());
    }
}
