//! Musical keys and their notations: musical (Am, F#), Camelot (8A) and
//! Open Key (1m).

use serde::{Deserialize, Serialize};

const NAMES: [&str; 12] = [
    "C", "C#", "D", "Eb", "E", "F", "F#", "G", "Ab", "A", "Bb", "B",
];

/// A major or minor key. `tonic` is a pitch class, 0 = C to 11 = B.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MusicalKey {
    pub tonic: u8,
    pub minor: bool,
}

/// How keys are displayed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum KeyNotation {
    #[default]
    Camelot,
    OpenKey,
    Musical,
}

impl MusicalKey {
    pub fn new(tonic: u8, minor: bool) -> Self {
        Self {
            tonic: tonic % 12,
            minor,
        }
    }

    /// The key `semitones` higher (negative: lower), e.g. after a key shift.
    pub fn transposed(self, semitones: i32) -> Self {
        Self::new(
            (i32::from(self.tonic) + semitones).rem_euclid(12) as u8,
            self.minor,
        )
    }

    /// Index 0..24: majors 0-11, minors 12-23.
    pub fn index(self) -> u8 {
        self.tonic + if self.minor { 12 } else { 0 }
    }

    pub fn from_index(i: u8) -> Self {
        Self::new(i % 12, (12..24).contains(&i))
    }

    /// Camelot number 1..=12.
    fn camelot_number(self) -> u8 {
        // Minor keys share the number of their relative major.
        let major_tonic = if self.minor {
            (self.tonic + 3) % 12
        } else {
            self.tonic
        };
        ((major_tonic * 7) % 12 + 7) % 12 + 1
    }

    /// Camelot notation, e.g. "8A" (A minor) or "8B" (C major).
    pub fn camelot(self) -> String {
        format!(
            "{}{}",
            self.camelot_number(),
            if self.minor { 'A' } else { 'B' }
        )
    }

    /// Open Key notation, e.g. "1m" (A minor) or "1d" (C major).
    pub fn open_key(self) -> String {
        let n = (self.camelot_number() + 12 - 8) % 12 + 1;
        format!("{n}{}", if self.minor { 'm' } else { 'd' })
    }

    /// Musical notation, e.g. "Am", "F#", "Bbm".
    pub fn musical(self) -> String {
        format!(
            "{}{}",
            NAMES[usize::from(self.tonic % 12)],
            if self.minor { "m" } else { "" }
        )
    }

    pub fn display(self, notation: KeyNotation) -> String {
        match notation {
            KeyNotation::Camelot => self.camelot(),
            KeyNotation::OpenKey => self.open_key(),
            KeyNotation::Musical => self.musical(),
        }
    }

    /// Parses Camelot ("8A"), Open Key ("1m") or musical notation ("Am",
    /// "A minor", "F#", "Gbm", "Ebmaj").
    pub fn parse(s: &str) -> Option<Self> {
        let t = s.trim();
        if t.is_empty() {
            return None;
        }
        // Camelot / Open Key: number then letter.
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            let n: u8 = digits.parse().ok()?;
            let rest = t[digits.len()..].trim().to_ascii_lowercase();
            if !(1..=12).contains(&n) {
                return None;
            }
            let (camelot, minor) = match rest.as_str() {
                "a" => (n, true),
                "b" => (n, false),
                "m" => ((n + 6) % 12 + 1, true),
                "d" => ((n + 6) % 12 + 1, false),
                _ => return None,
            };
            return (0..24)
                .map(Self::from_index)
                .find(|k| k.minor == minor && k.camelot_number() == camelot);
        }
        let mut chars = t.chars();
        let letter = chars.next()?.to_ascii_uppercase();
        let base: i32 = match letter {
            'C' => 0,
            'D' => 2,
            'E' => 4,
            'F' => 5,
            'G' => 7,
            'A' => 9,
            'B' => 11,
            _ => return None,
        };
        let rest: String = chars.collect();
        let (shift, rest) = if let Some(r) = rest.strip_prefix('#').or(rest.strip_prefix('♯')) {
            (1, r)
        } else if let Some(r) = rest.strip_prefix('b').or(rest.strip_prefix('♭')) {
            (-1, r)
        } else {
            (0, rest.as_str())
        };
        let mode = rest.trim().to_ascii_lowercase();
        let minor = match mode.as_str() {
            "" | "maj" | "major" | "dur" => false,
            "m" | "min" | "minor" | "moll" => true,
            _ => return None,
        };
        Some(Self::new((base + shift).rem_euclid(12) as u8, minor))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transposes() {
        let am = MusicalKey::new(9, true);
        assert_eq!(am.transposed(2).musical(), "Bm");
        assert_eq!(am.transposed(-10).musical(), "Bm");
        assert_eq!(am.transposed(7).camelot(), "9A");
        assert_eq!(am.transposed(0), am);
    }

    #[test]
    fn camelot_wheel() {
        let cases = [
            (MusicalKey::new(9, true), "8A", "1m", "Am"),
            (MusicalKey::new(0, false), "8B", "1d", "C"),
            (MusicalKey::new(7, false), "9B", "2d", "G"),
            (MusicalKey::new(11, false), "1B", "6d", "B"),
            (MusicalKey::new(5, true), "4A", "9m", "Fm"),
            (MusicalKey::new(6, true), "11A", "4m", "F#m"),
            (MusicalKey::new(10, false), "6B", "11d", "Bb"),
        ];
        for (k, cam, ok, mus) in cases {
            assert_eq!(k.camelot(), cam, "{k:?}");
            assert_eq!(k.open_key(), ok, "{k:?}");
            assert_eq!(k.musical(), mus, "{k:?}");
        }
    }

    #[test]
    fn parse_all_notations_roundtrip() {
        for i in 0..24 {
            let k = MusicalKey::from_index(i);
            assert_eq!(MusicalKey::parse(&k.camelot()), Some(k));
            assert_eq!(MusicalKey::parse(&k.open_key()), Some(k));
            assert_eq!(MusicalKey::parse(&k.musical()), Some(k));
        }
        assert_eq!(MusicalKey::parse("A minor"), Some(MusicalKey::new(9, true)));
        assert_eq!(MusicalKey::parse("Gbm"), Some(MusicalKey::new(6, true)));
        assert_eq!(MusicalKey::parse("Ebmaj"), Some(MusicalKey::new(3, false)));
        assert_eq!(MusicalKey::parse("13A"), None);
        assert_eq!(MusicalKey::parse("H"), None);
        assert_eq!(MusicalKey::parse(""), None);
    }
}
