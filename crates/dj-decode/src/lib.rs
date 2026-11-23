//! Decoding audio files to `f32` stereo at the engine sample rate.
//!
//! [`TrackAudio`] is the decoded track. It is filled progressively by a
//! decoder thread and can be read by the audio thread at the same time, so a
//! deck can start playing as soon as the first chunk is decoded.
//!
//! Supported through `symphonia`: MP3, AAC/M4A, ALAC, FLAC, WAV, AIFF, OGG
//! Vorbis.

mod decoder;
mod track;

pub use decoder::{DecodeError, LoadEvent, SourceInfo, decode_file, spawn_load};
pub use track::{CHUNK_FRAMES, TrackAudio, TrackState, TrackWriter};
