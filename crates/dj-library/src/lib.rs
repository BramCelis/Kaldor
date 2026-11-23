//! The Kaldor music library: an SQLite database of tracks, analysis results
//! (BPM, beatgrid, key, loudness, waveform), cues, loops and crates, plus
//! folder import and search.
//!
//! The `Library` lives on one thread (the UI thread). Analysis runs on an
//! [`AnalysisQueue`] worker pool and its results are stored by the owner.

mod db;
pub mod history;
pub mod import;
pub mod interop;
pub mod jobs;
pub mod model;
mod schema;
pub mod search;
pub mod smart;

pub use db::{Library, LibraryError, Result};
pub use import::{AUDIO_EXTENSIONS, ImportReport, is_audio_file, read_meta, scan_folder};
pub use jobs::{AnalysisDone, AnalysisJob, AnalysisQueue};
pub use model::{CrateId, CrateNode, Cue, CueKind, SavedLoop, Track, TrackId, TrackMeta};
pub use schema::latest_version as schema_version;
pub use smart::SmartRules;
