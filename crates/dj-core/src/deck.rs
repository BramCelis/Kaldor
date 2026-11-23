//! Deck identifiers.

use serde::{Deserialize, Serialize};

/// Number of decks the engine runs.
pub const NUM_DECKS: usize = 4;

/// One of the four decks. Decks are numbered 1 to 4 in the UI and
/// indexed 0 to 3 internally.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DeckId(u8);

impl DeckId {
    /// All decks in order.
    pub const ALL: [DeckId; NUM_DECKS] = [DeckId(0), DeckId(1), DeckId(2), DeckId(3)];

    /// Creates a deck id from a zero-based index. Returns `None` when the
    /// index is out of range.
    pub fn from_index(index: usize) -> Option<Self> {
        if index < NUM_DECKS {
            Some(DeckId(index as u8))
        } else {
            None
        }
    }

    /// Zero-based index, always below [`NUM_DECKS`].
    pub fn index(self) -> usize {
        self.0 as usize
    }

    /// One-based deck number as printed in the UI.
    pub fn number(self) -> u8 {
        self.0 + 1
    }

    /// Default crossfader side: decks 1 and 3 on side A, 2 and 4 on side B.
    pub fn default_crossfader_side_is_a(self) -> bool {
        self.0.is_multiple_of(2)
    }
}

impl std::fmt::Display for DeckId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Deck {}", self.number())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_index_bounds() {
        assert_eq!(DeckId::from_index(0).map(DeckId::number), Some(1));
        assert_eq!(DeckId::from_index(3).map(DeckId::number), Some(4));
        assert!(DeckId::from_index(4).is_none());
    }

    #[test]
    fn crossfader_defaults() {
        let sides: Vec<bool> = DeckId::ALL
            .iter()
            .map(|d| d.default_crossfader_side_is_a())
            .collect();
        assert_eq!(sides, vec![true, false, true, false]);
    }
}
