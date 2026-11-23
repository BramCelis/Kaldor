//! Track analysis: waveforms, BPM and beatgrid, key and loudness.
//! Everything here runs on worker threads, never on the audio thread.

pub mod analyze;
pub mod key;
pub mod loudness;
pub mod synth;
pub mod tempo;
pub mod waveform;

pub use analyze::{ANALYSIS_VERSION, Analysis, AnalysisOptions, analyze_audio, analyze_file};
pub use tempo::TempoOptions;
pub use waveform::{WavePoint, Waveform};
