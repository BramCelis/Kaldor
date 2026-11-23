//! The full analysis of one track: beatgrid, key, loudness, waveform.

use crate::key::detect_key;
use crate::loudness::{auto_gain_db, integrated_lufs};
use crate::tempo::{TempoOptions, detect_beatgrid};
use crate::waveform::Waveform;
use dj_core::{Beatgrid, MusicalKey};
use dj_decode::{DecodeError, SourceInfo, TrackAudio, decode_file};
use std::path::Path;

/// Version of the analysis algorithms. Stored with results so tracks can be
/// re-analysed when the algorithms improve.
pub const ANALYSIS_VERSION: u32 = 6;

/// Analysis settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnalysisOptions {
    pub tempo: TempoOptions,
    /// Auto-gain target loudness in LUFS.
    pub target_lufs: f64,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            tempo: TempoOptions::default(),
            target_lufs: -12.0,
        }
    }
}

/// Everything the analysis produces for one track.
#[derive(Clone, Debug)]
pub struct Analysis {
    pub beatgrid: Option<Beatgrid>,
    pub key: Option<MusicalKey>,
    pub key_confidence: f32,
    pub lufs: Option<f64>,
    /// Suggested auto-gain in dB.
    pub gain_db: Option<f64>,
    pub waveform: Waveform,
    pub duration: f64,
    pub version: u32,
}

/// Mono downmix of a whole track.
pub fn mono(audio: &TrackAudio) -> Vec<f32> {
    let total = audio.ready_frames() as usize;
    let mut out = Vec::with_capacity(total);
    let mut buf = vec![0.0f32; 8192 * 2];
    let mut pos = 0usize;
    while pos < total {
        let n = (total - pos).min(8192);
        audio.read_interleaved(pos as u64, &mut buf[..n * 2]);
        out.extend(
            buf[..n * 2]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|f| 0.5 * (f[0] + f[1])),
        );
        pos += n;
    }
    out
}

/// Analyses decoded audio.
pub fn analyze_audio(audio: &TrackAudio, opts: &AnalysisOptions) -> Analysis {
    let sr = audio.sample_rate();
    let m = mono(audio);
    let beatgrid = detect_beatgrid(&m, sr, opts.tempo);
    let key = detect_key(&m, sr);
    drop(m);
    let lufs = integrated_lufs(audio);
    Analysis {
        beatgrid,
        key: key.map(|k| k.0),
        key_confidence: key.map_or(0.0, |k| k.1),
        lufs,
        gain_db: lufs.map(|l| auto_gain_db(l, opts.target_lufs)),
        waveform: Waveform::build(audio),
        duration: audio.duration_seconds(),
        version: ANALYSIS_VERSION,
    }
}

/// Decodes a file at its own sample rate and analyses it.
pub fn analyze_file(
    path: &Path,
    opts: &AnalysisOptions,
) -> Result<(SourceInfo, Analysis), DecodeError> {
    let (info, audio) = decode_file(path, 0)?;
    Ok((info, analyze_audio(&audio, opts)))
}
