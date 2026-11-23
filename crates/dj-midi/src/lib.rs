//! MIDI for Kaldor: decoding raw MIDI, capture files that record what each
//! controller control sends (taken with the `midi-monitor` tool on the real
//! NS4FX), device I/O through `midir`, the data-driven mapping engine, the
//! NS4FX profile and the NS4FX's LED and display messages. The profile is
//! built from the captures only: no message is guessed.

pub mod capture;
pub mod ddj_flx6_out;
pub mod device;
pub mod mapping;
pub mod message;
pub mod ns4fx_out;
pub mod profile;
pub mod summary;

pub use capture::{Capture, CaptureWriter, Step};
pub use mapping::{Event, Mapper, Value, centred};
pub use message::{KeyKind, MessageKey, MidiMessage};
pub use profile::{Profile, Unit};
pub use summary::Summary;
