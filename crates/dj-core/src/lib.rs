//! Shared types for Kaldor: deck identifiers, output routing, configuration
//! and time math. This crate does no I/O and has no platform dependencies.

pub mod beatgrid;
pub mod config;
pub mod deck;
pub mod key;
pub mod routing;
pub mod time;

pub use beatgrid::Beatgrid;
pub use config::{AppConfig, AudioConfig, PerformanceConfig, UiConfig};
pub use deck::{DeckId, NUM_DECKS};
pub use key::{KeyNotation, MusicalKey};
pub use routing::{ChannelPair, OutputRouting};
