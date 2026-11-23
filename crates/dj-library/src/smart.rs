//! Smart crates: a crate that fills itself with the tracks matching its
//! rules ("Genre contains house" and "BPM between 120-126", or any of
//! several rules). The tracks are worked out every time the crate is read
//! ([`crate::Library::crate_tracks`]), so new and edited tracks show up
//! without a refresh, and the export and the playlist files see the same
//! list.
//!
//! Stored in `crates.smart` as text: `all` or `any` on the first line, then
//! one rule per line, `field<TAB>op<TAB>value`.

use crate::model::Track;
use dj_core::MusicalKey;

/// What a rule looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Artist,
    Title,
    Album,
    Genre,
    Comment,
    /// The file's path.
    Location,
    Bpm,
    Key,
    Year,
    /// Stars, 0 to 5.
    Rating,
    PlayCount,
    /// Seconds (typed as seconds or m:ss).
    Duration,
    /// Days ago.
    DateAdded,
}

/// The kind of value a field holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Text,
    Number,
    Key,
    Date,
}

/// How a rule compares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Contains,
    NotContains,
    Is,
    IsNot,
    StartsWith,
    EndsWith,
    AtLeast,
    AtMost,
    /// "a-b" (inclusive).
    Between,
    /// Same key, one step round the Camelot wheel, or its relative
    /// major / minor.
    Compatible,
    /// Added in the last N days.
    InLast,
    NotInLast,
}

impl Field {
    pub const ALL: [Field; 13] = [
        Field::Artist,
        Field::Title,
        Field::Album,
        Field::Genre,
        Field::Comment,
        Field::Location,
        Field::Bpm,
        Field::Key,
        Field::Year,
        Field::Rating,
        Field::PlayCount,
        Field::Duration,
        Field::DateAdded,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Field::Artist => "artist",
            Field::Title => "title",
            Field::Album => "album",
            Field::Genre => "genre",
            Field::Comment => "comment",
            Field::Location => "location",
            Field::Bpm => "bpm",
            Field::Key => "key",
            Field::Year => "year",
            Field::Rating => "rating",
            Field::PlayCount => "plays",
            Field::Duration => "duration",
            Field::DateAdded => "added",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Field::Artist => "Artist",
            Field::Title => "Title",
            Field::Album => "Album",
            Field::Genre => "Genre",
            Field::Comment => "Comment",
            Field::Location => "Location",
            Field::Bpm => "BPM",
            Field::Key => "Key",
            Field::Year => "Year",
            Field::Rating => "Rating",
            Field::PlayCount => "Play count",
            Field::Duration => "Length",
            Field::DateAdded => "Date added",
        }
    }

    pub fn from_id(s: &str) -> Option<Self> {
        Field::ALL.into_iter().find(|f| f.id() == s)
    }

    pub fn kind(self) -> Kind {
        match self {
            Field::Artist
            | Field::Title
            | Field::Album
            | Field::Genre
            | Field::Comment
            | Field::Location => Kind::Text,
            Field::Bpm | Field::Year | Field::Rating | Field::PlayCount | Field::Duration => {
                Kind::Number
            }
            Field::Key => Kind::Key,
            Field::DateAdded => Kind::Date,
        }
    }

    /// The comparisons that make sense for this field (the first is the
    /// default).
    pub fn ops(self) -> &'static [Op] {
        match self.kind() {
            Kind::Text => &[
                Op::Contains,
                Op::NotContains,
                Op::Is,
                Op::IsNot,
                Op::StartsWith,
                Op::EndsWith,
            ],
            Kind::Number => &[Op::Between, Op::Is, Op::IsNot, Op::AtLeast, Op::AtMost],
            Kind::Key => &[Op::Is, Op::IsNot, Op::Compatible],
            Kind::Date => &[Op::InLast, Op::NotInLast],
        }
    }

    /// What to type, for the empty value field.
    pub fn hint(self, op: Op) -> &'static str {
        match (self, op) {
            (_, Op::Between) if self == Field::Duration => "3:00-6:00",
            (Field::Bpm, Op::Between) => "120-126",
            (Field::Year, Op::Between) => "1990-1999",
            (_, Op::Between) => "1-5",
            (Field::Key, _) => "8A, 1m or Am",
            (Field::Duration, _) => "5:30",
            (_, Op::InLast | Op::NotInLast) => "days",
            (Field::Rating, _) => "0 to 5",
            _ => "",
        }
    }
}

impl Op {
    pub fn id(self) -> &'static str {
        match self {
            Op::Contains => "contains",
            Op::NotContains => "not_contains",
            Op::Is => "is",
            Op::IsNot => "is_not",
            Op::StartsWith => "starts",
            Op::EndsWith => "ends",
            Op::AtLeast => "at_least",
            Op::AtMost => "at_most",
            Op::Between => "between",
            Op::Compatible => "compatible",
            Op::InLast => "in_last",
            Op::NotInLast => "not_in_last",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Op::Contains => "contains",
            Op::NotContains => "does not contain",
            Op::Is => "is",
            Op::IsNot => "is not",
            Op::StartsWith => "starts with",
            Op::EndsWith => "ends with",
            Op::AtLeast => "is at least",
            Op::AtMost => "is at most",
            Op::Between => "is between",
            Op::Compatible => "mixes with",
            Op::InLast => "in the last",
            Op::NotInLast => "not in the last",
        }
    }

    const ALL: [Op; 12] = [
        Op::Contains,
        Op::NotContains,
        Op::Is,
        Op::IsNot,
        Op::StartsWith,
        Op::EndsWith,
        Op::AtLeast,
        Op::AtMost,
        Op::Between,
        Op::Compatible,
        Op::InLast,
        Op::NotInLast,
    ];

    pub fn from_id(s: &str) -> Option<Self> {
        Op::ALL.into_iter().find(|o| o.id() == s)
    }
}

/// One rule.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub field: Field,
    pub op: Op,
    pub value: String,
}

impl Rule {
    pub fn new(field: Field) -> Self {
        Self {
            field,
            op: field.ops()[0],
            value: String::new(),
        }
    }

    /// The value can be read (a rule that cannot never matches).
    pub fn is_valid(&self) -> bool {
        match self.field.kind() {
            Kind::Text => true,
            Kind::Key => MusicalKey::parse(&self.value).is_some(),
            Kind::Date => number(&self.value).is_some(),
            Kind::Number => self.range().is_some(),
        }
    }

    /// The numbers the rule compares with, as a range (`Is` on BPM: ±0.5).
    fn range(&self) -> Option<(f64, f64)> {
        let read = |s: &str| {
            if self.field == Field::Duration {
                duration(s)
            } else {
                number(s)
            }
        };
        match self.op {
            Op::Between => {
                let v = self.value.trim();
                // "120-126"; a leading "-" is not a separator.
                let cut = v.char_indices().skip(1).find(|(_, c)| *c == '-')?.0;
                let (a, b) = (read(&v[..cut])?, read(&v[cut + 1..])?);
                Some((a.min(b), a.max(b)))
            }
            _ => {
                let x = read(&self.value)?;
                let slack = if self.field == Field::Bpm { 0.5 } else { 0.0 };
                Some((x - slack, x + slack))
            }
        }
    }

    /// Does `t` match (`now`: seconds since the Unix epoch)?
    pub fn matches(&self, t: &Track, now: i64) -> bool {
        match self.field.kind() {
            Kind::Text => {
                let field = match self.field {
                    Field::Artist => t.meta.artist.clone(),
                    Field::Title => t.meta.title.clone(),
                    Field::Album => t.meta.album.clone(),
                    Field::Genre => t.meta.genre.clone(),
                    Field::Comment => t.meta.comment.clone(),
                    _ => t.meta.path.display().to_string(),
                }
                .to_lowercase();
                let v = self.value.trim().to_lowercase();
                match self.op {
                    Op::Contains => field.contains(&v),
                    Op::NotContains => !field.contains(&v),
                    Op::Is => field.trim() == v,
                    Op::IsNot => field.trim() != v,
                    Op::StartsWith => field.trim_start().starts_with(&v),
                    Op::EndsWith => field.trim_end().ends_with(&v),
                    _ => false,
                }
            }
            Kind::Key => {
                let Some(want) = MusicalKey::parse(&self.value) else {
                    return false;
                };
                match (self.op, t.key) {
                    (Op::Is, Some(k)) => k == want,
                    (Op::IsNot, k) => k != Some(want),
                    (Op::Compatible, Some(k)) => compatible(k, want),
                    _ => false,
                }
            }
            Kind::Date => {
                let Some(days) = number(&self.value) else {
                    return false;
                };
                let recent = t.date_added as f64 >= now as f64 - days * 86_400.0;
                match self.op {
                    Op::InLast => recent,
                    Op::NotInLast => !recent,
                    _ => false,
                }
            }
            Kind::Number => {
                let Some((a, b)) = self.range() else {
                    return false;
                };
                let x = match self.field {
                    Field::Bpm => t.bpm,
                    Field::Year => t.meta.year.map(f64::from),
                    Field::Rating => Some(f64::from(t.rating)),
                    Field::PlayCount => Some(f64::from(t.play_count)),
                    Field::Duration => Some(t.meta.duration),
                    _ => None,
                };
                // A track without the value (no BPM, no year) matches only
                // "is not".
                let Some(x) = x else {
                    return self.op == Op::IsNot;
                };
                // Whole numbers compare whole: a 245.6 s track "is" 4:05.
                let x = if self.field == Field::Duration {
                    x.floor()
                } else {
                    x
                };
                match self.op {
                    Op::Is | Op::Between => x >= a && x <= b,
                    Op::IsNot => x < a || x > b,
                    Op::AtLeast => x >= a,
                    Op::AtMost => x <= b,
                    _ => false,
                }
            }
        }
    }
}

fn number(s: &str) -> Option<f64> {
    s.trim()
        .replace(',', ".")
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
}

/// Seconds from "5:30", "1:02:03" or "330".
fn duration(s: &str) -> Option<f64> {
    let s = s.trim();
    if !s.contains(':') {
        return number(s);
    }
    let mut total = 0.0;
    for part in s.split(':') {
        total = total * 60.0 + number(part)?;
    }
    Some(total)
}

/// Camelot number and letter (A = minor).
fn wheel(k: MusicalKey) -> (u8, bool) {
    let c = k.camelot();
    (c[..c.len() - 1].parse().unwrap_or(0), k.minor)
}

/// Keys that mix: the same, one step either way round the Camelot wheel
/// (same letter), or the relative major / minor (same number).
pub fn compatible(a: MusicalKey, b: MusicalKey) -> bool {
    let ((na, ma), (nb, mb)) = (wheel(a), wheel(b));
    if ma == mb {
        let d = (i32::from(na) - i32::from(nb)).rem_euclid(12);
        d == 0 || d == 1 || d == 11
    } else {
        na == nb
    }
}

/// A smart crate's rules.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartRules {
    /// Every rule must match (else any one).
    pub match_all: bool,
    pub rules: Vec<Rule>,
}

impl Default for SmartRules {
    fn default() -> Self {
        Self {
            match_all: true,
            rules: vec![Rule::new(Field::Genre)],
        }
    }
}

impl SmartRules {
    /// Does `t` match? No rules: nothing matches (an empty smart crate,
    /// not the whole library).
    pub fn matches(&self, t: &Track, now: i64) -> bool {
        let rules: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|r| !(r.field.kind() == Kind::Text && r.value.trim().is_empty()))
            .collect();
        if rules.is_empty() {
            return false;
        }
        if self.match_all {
            rules.iter().all(|r| r.matches(t, now))
        } else {
            rules.iter().any(|r| r.matches(t, now))
        }
    }

    /// The ids of the matching tracks, in the order given.
    pub fn filter<'a>(
        &self,
        tracks: impl IntoIterator<Item = &'a Track>,
        now: i64,
    ) -> Vec<crate::TrackId> {
        tracks
            .into_iter()
            .filter(|t| self.matches(t, now))
            .map(|t| t.id)
            .collect()
    }

    /// The stored text.
    pub fn encode(&self) -> String {
        let mut s = String::from(if self.match_all { "all" } else { "any" });
        for r in &self.rules {
            let v: String = r
                .value
                .chars()
                .map(|c| {
                    if matches!(c, '\t' | '\n' | '\r') {
                        ' '
                    } else {
                        c
                    }
                })
                .collect();
            s.push_str(&format!("\n{}\t{}\t{v}", r.field.id(), r.op.id()));
        }
        s
    }

    /// From the stored text (rules this version does not know are left
    /// out).
    pub fn decode(s: &str) -> Self {
        let mut lines = s.lines();
        let match_all = lines.next().map(str::trim) != Some("any");
        let rules = lines
            .filter_map(|l| {
                let mut p = l.splitn(3, '\t');
                let field = Field::from_id(p.next()?)?;
                let op = Op::from_id(p.next()?).filter(|o| field.ops().contains(o))?;
                Some(Rule {
                    field,
                    op,
                    value: p.next().unwrap_or("").to_string(),
                })
            })
            .collect();
        Self { match_all, rules }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TrackMeta;

    fn track(id: i64, artist: &str, genre: &str, bpm: Option<f64>, key: &str) -> Track {
        Track {
            id,
            meta: TrackMeta {
                artist: artist.into(),
                genre: genre.into(),
                duration: 245.6,
                year: Some(2020),
                ..TrackMeta::default()
            },
            bpm,
            key: MusicalKey::parse(key),
            lufs: None,
            gain_db: None,
            rating: 4,
            color: None,
            play_count: 2,
            date_added: 1_000_000,
            missing: false,
            analysis_version: None,
        }
    }

    fn rule(field: Field, op: Op, value: &str) -> Rule {
        Rule {
            field,
            op,
            value: value.into(),
        }
    }

    #[test]
    fn rules_match() {
        let t = track(1, "Café Tacvba", "Deep House", Some(123.9), "8A");
        let now = 1_000_000 + 10 * 86_400;
        let yes = [
            rule(Field::Genre, Op::Contains, "house"),
            rule(Field::Artist, Op::StartsWith, "CAFÉ"),
            rule(Field::Genre, Op::Is, "deep house"),
            rule(Field::Genre, Op::NotContains, "techno"),
            rule(Field::Bpm, Op::Between, "120-126"),
            rule(Field::Bpm, Op::Is, "124"),
            rule(Field::Bpm, Op::AtLeast, "123.5"),
            rule(Field::Key, Op::Is, "Am"),
            rule(Field::Key, Op::Compatible, "9A"),
            rule(Field::Key, Op::Compatible, "8B"),
            rule(Field::Key, Op::Compatible, "7A"),
            rule(Field::Rating, Op::AtLeast, "4"),
            rule(Field::Duration, Op::Is, "4:05"),
            rule(Field::Duration, Op::Between, "3:00-5:00"),
            rule(Field::DateAdded, Op::InLast, "30"),
            rule(Field::Year, Op::Between, "2010-2029"),
        ];
        for r in &yes {
            assert!(r.matches(&t, now), "{r:?}");
        }
        let no = [
            rule(Field::Genre, Op::Contains, "techno"),
            rule(Field::Bpm, Op::Is, "128"),
            rule(Field::Key, Op::Compatible, "10A"),
            rule(Field::Key, Op::Compatible, "9B"),
            rule(Field::Key, Op::IsNot, "8A"),
            rule(Field::DateAdded, Op::InLast, "7"),
            rule(Field::PlayCount, Op::AtMost, "1"),
            rule(Field::Bpm, Op::Between, "nonsense"),
        ];
        for r in &no {
            assert!(!r.matches(&t, now), "{r:?}");
        }
        // No BPM: only "is not" matches.
        let unknown = track(2, "", "", None, "");
        assert!(!rule(Field::Bpm, Op::AtMost, "200").matches(&unknown, now));
        assert!(rule(Field::Bpm, Op::IsNot, "128").matches(&unknown, now));
        assert!(rule(Field::Key, Op::IsNot, "8A").matches(&unknown, now));
    }

    #[test]
    fn all_any_and_storage() {
        let a = track(1, "A", "House", Some(124.0), "8A");
        let b = track(2, "B", "Techno", Some(132.0), "5A");
        let mut s = SmartRules {
            match_all: true,
            rules: vec![
                rule(Field::Genre, Op::Contains, "house"),
                rule(Field::Bpm, Op::Between, "120-126"),
            ],
        };
        assert_eq!(s.filter([&a, &b], 0), vec![1]);
        s.match_all = false;
        s.rules[1] = rule(Field::Bpm, Op::AtLeast, "130");
        assert_eq!(s.filter([&a, &b], 0), vec![1, 2]);
        s.rules[0].value = "tab\there".into();
        let back = SmartRules::decode(&s.encode());
        assert_eq!(back.rules[0].value, "tab here");
        assert!(!back.match_all);
        assert_eq!(back.rules[1], s.rules[1]);
        // Unknown fields or ops from a newer version are left out.
        let d = SmartRules::decode("all\nmood\tis\thappy\nbpm\tcontains\tx\ngenre\tis\tDub");
        assert_eq!(d.rules, vec![rule(Field::Genre, Op::Is, "Dub")]);
        // Blank text rules are ignored; no rules match nothing.
        let empty = SmartRules {
            match_all: true,
            rules: vec![Rule::new(Field::Genre)],
        };
        assert!(empty.filter([&a, &b], 0).is_empty());
    }

    #[test]
    fn the_camelot_wheel_wraps() {
        let k = |s| MusicalKey::parse(s).unwrap();
        assert!(compatible(k("12A"), k("1A")));
        assert!(compatible(k("1B"), k("12B")));
        assert!(!compatible(k("1A"), k("3A")));
        assert!(!compatible(k("1A"), k("2B")));
    }
}
