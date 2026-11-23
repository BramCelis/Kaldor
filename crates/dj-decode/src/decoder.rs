//! File decoding (symphonia) and sample-rate conversion (rubato).

use crate::track::{TrackAudio, TrackState, TrackWriter};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Decoding errors.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("cannot open {path}: {source}")]
    Open {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("unsupported or unrecognised format: {0}")]
    Format(String),
    #[error("no audio track in file")]
    NoAudioTrack,
    #[error("decoder error: {0}")]
    Decoder(String),
    #[error("resampler error: {0}")]
    Resampler(String),
    #[error("cancelled")]
    Cancelled,
}

/// What the decoder learned about the file.
#[derive(Clone, Debug)]
pub struct SourceInfo {
    pub path: PathBuf,
    pub source_sample_rate: u32,
    pub source_channels: usize,
    pub codec: String,
    /// Estimated length at the target rate, if the container says.
    pub estimated_frames: Option<u64>,
}

/// Progress messages from [`spawn_load`].
#[derive(Debug)]
pub enum LoadEvent {
    /// The file opened; `audio` fills in from now on and can be played.
    Started {
        info: SourceInfo,
        audio: Arc<TrackAudio>,
    },
    /// Decoding finished.
    Finished { frames: u64, truncated: bool },
    /// Opening or decoding failed. If `Started` was sent before, what was
    /// decoded stays playable.
    Failed(String),
}

struct Opened {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    info: SourceInfo,
}

fn open(path: &Path, target_rate: u32) -> Result<Opened, DecodeError> {
    let file = File::open(path).map_err(|source| DecodeError::Open {
        path: path.to_path_buf(),
        source,
    })?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| DecodeError::Format(e.to_string()))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or(DecodeError::NoAudioTrack)?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(DecodeError::NoAudioTrack)?;
    let decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| DecodeError::Decoder(e.to_string()))?;
    let source_rate = params.sample_rate.unwrap_or(if target_rate == 0 {
        44_100
    } else {
        target_rate
    });
    // A target rate of 0 means "keep the file's own rate".
    let target_rate = if target_rate == 0 {
        source_rate
    } else {
        target_rate
    };
    let channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(2);
    let estimated_frames = track
        .num_frames
        .map(|n| (n as f64 * f64::from(target_rate) / f64::from(source_rate.max(1))).ceil() as u64);
    let info = SourceInfo {
        path: path.to_path_buf(),
        source_sample_rate: source_rate,
        source_channels: channels,
        codec: format!("{:?}", params.codec),
        estimated_frames,
    };
    Ok(Opened {
        track_id: track.id,
        format,
        decoder,
        info,
    })
}

/// Converts a stream of stereo frames to the target rate and writes them.
struct StreamResampler {
    inner: Option<Fft<f32>>,
    inbuf: Vec<f32>,
    outbuf: Vec<f32>,
    skip: usize,
    ratio: f64,
    total_in: u64,
    total_out: u64,
}

impl StreamResampler {
    fn new(source_rate: u32, target_rate: u32) -> Result<Self, DecodeError> {
        if source_rate == target_rate {
            return Ok(Self {
                inner: None,
                inbuf: Vec::new(),
                outbuf: Vec::new(),
                skip: 0,
                ratio: 1.0,
                total_in: 0,
                total_out: 0,
            });
        }
        let rs = Fft::<f32>::new(
            source_rate as usize,
            target_rate as usize,
            1024,
            2,
            FixedSync::Input,
        )
        .map_err(|e| DecodeError::Resampler(e.to_string()))?;
        let out_max = rs.output_frames_max();
        Ok(Self {
            skip: rs.output_delay(),
            outbuf: vec![0.0; out_max * 2],
            inbuf: Vec::with_capacity(4096 * 2),
            inner: Some(rs),
            ratio: f64::from(target_rate) / f64::from(source_rate),
            total_in: 0,
            total_out: 0,
        })
    }

    fn emit(&mut self, produced: usize, w: &mut TrackWriter, limit: Option<u64>) {
        let mut start = 0;
        if self.skip > 0 {
            let s = self.skip.min(produced);
            self.skip -= s;
            start = s;
        }
        let mut end = produced;
        if let Some(limit) = limit {
            let room = limit.saturating_sub(self.total_out) as usize;
            end = end.min(start + room);
        }
        if end > start {
            w.push_interleaved(&self.outbuf[start * 2..end * 2]);
            self.total_out += (end - start) as u64;
        }
    }

    fn push(&mut self, frames: &[f32], w: &mut TrackWriter) -> Result<(), DecodeError> {
        self.total_in += (frames.len() / 2) as u64;
        if self.inner.is_none() {
            w.push_interleaved(frames);
            return Ok(());
        }
        self.inbuf.extend_from_slice(frames);
        loop {
            let Some(rs) = self.inner.as_mut() else {
                return Ok(());
            };
            let need = rs.input_frames_next();
            if self.inbuf.len() / 2 < need {
                return Ok(());
            }
            let out_frames = self.outbuf.len() / 2;
            let input = InterleavedSlice::new(&self.inbuf[..need * 2], 2, need)
                .map_err(|e| DecodeError::Resampler(e.to_string()))?;
            let mut output = InterleavedSlice::new_mut(&mut self.outbuf, 2, out_frames)
                .map_err(|e| DecodeError::Resampler(e.to_string()))?;
            let (used, produced) = rs
                .process_into_buffer(&input, &mut output, None)
                .map_err(|e| DecodeError::Resampler(e.to_string()))?;
            self.inbuf.drain(..used * 2);
            self.emit(produced, w, None);
        }
    }

    /// Flushes the resampler so the output has the exact expected length.
    fn finish(&mut self, w: &mut TrackWriter) -> Result<(), DecodeError> {
        let expected = (self.total_in as f64 * self.ratio).round() as u64;
        for _ in 0..64 {
            if self.total_out >= expected {
                break;
            }
            let Some(rs) = self.inner.as_mut() else {
                break;
            };
            let need = rs.input_frames_next();
            let avail = (self.inbuf.len() / 2).min(need);
            self.inbuf.resize(need * 2, 0.0);
            let out_frames = self.outbuf.len() / 2;
            let input = InterleavedSlice::new(&self.inbuf[..need * 2], 2, need)
                .map_err(|e| DecodeError::Resampler(e.to_string()))?;
            let mut output = InterleavedSlice::new_mut(&mut self.outbuf, 2, out_frames)
                .map_err(|e| DecodeError::Resampler(e.to_string()))?;
            let idx = Indexing {
                partial_len: Some(avail),
                ..Default::default()
            };
            let (_, produced) = rs
                .process_into_buffer(&input, &mut output, Some(&idx))
                .map_err(|e| DecodeError::Resampler(e.to_string()))?;
            self.inbuf.clear();
            self.emit(produced, w, Some(expected));
        }
        Ok(())
    }
}

/// Converts any channel layout to interleaved stereo.
fn to_stereo(interleaved: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    match channels {
        0 => {}
        1 => {
            for &s in interleaved {
                out.push(s);
                out.push(s);
            }
        }
        2 => out.extend_from_slice(interleaved),
        n => {
            // Keep front left/right; the other channels are dropped.
            for f in interleaved.chunks_exact(n) {
                out.push(f[0]);
                out.push(f[1]);
            }
        }
    }
}

fn run(
    mut opened: Opened,
    target_rate: u32,
    mut writer: TrackWriter,
    cancel: &AtomicBool,
) -> Result<(u64, bool), DecodeError> {
    let mut resampler = StreamResampler::new(opened.info.source_sample_rate, target_rate)?;
    let mut samples: Vec<f32> = Vec::new();
    let mut stereo: Vec<f32> = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            writer.abort(TrackState::Cancelled);
            return Err(DecodeError::Cancelled);
        }
        let packet = match opened.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => {
                writer.abort(TrackState::Failed);
                return Err(DecodeError::Decoder(e.to_string()));
            }
        };
        if packet.track_id != opened.track_id {
            continue;
        }
        match opened.decoder.decode(&packet) {
            Ok(buf) => {
                let channels = buf.spec().channels().count();
                samples.resize(buf.samples_interleaved(), 0.0);
                buf.copy_to_slice_interleaved(&mut samples);
                to_stereo(&samples, channels, &mut stereo);
                resampler.push(&stereo, &mut writer)?;
            }
            // Corrupt packets are skipped, like every player does.
            Err(SymError::DecodeError(msg)) => {
                tracing::debug!("skipping corrupt packet: {msg}");
            }
            Err(e) => {
                writer.abort(TrackState::Failed);
                return Err(DecodeError::Decoder(e.to_string()));
            }
        }
    }
    resampler.finish(&mut writer)?;
    let frames = writer.frames_written();
    let truncated = writer.was_truncated();
    writer.finish();
    Ok((frames, truncated))
}

/// Decodes a whole file on the calling thread. A `target_rate` of 0 keeps
/// the file's own sample rate (used by analysis).
pub fn decode_file(
    path: &Path,
    target_rate: u32,
) -> Result<(SourceInfo, Arc<TrackAudio>), DecodeError> {
    let opened = open(path, target_rate)?;
    let target_rate = if target_rate == 0 {
        opened.info.source_sample_rate
    } else {
        target_rate
    };
    let info = opened.info.clone();
    let (audio, writer) = TrackAudio::new(target_rate, info.estimated_frames);
    run(opened, target_rate, writer, &AtomicBool::new(false))?;
    Ok((info, audio))
}

/// Starts decoding on a new thread. Events arrive on the returned channel;
/// set `cancel` to stop early.
pub fn spawn_load(
    path: PathBuf,
    target_rate: u32,
    cancel: Arc<AtomicBool>,
) -> std::io::Result<mpsc::Receiver<LoadEvent>> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("kaldor-decode".into())
        .spawn(move || {
            let opened = match open(&path, target_rate) {
                Ok(o) => o,
                Err(e) => {
                    let _ = tx.send(LoadEvent::Failed(e.to_string()));
                    return;
                }
            };
            let info = opened.info.clone();
            let (audio, writer) = TrackAudio::new(target_rate, info.estimated_frames);
            let _ = tx.send(LoadEvent::Started {
                info,
                audio: Arc::clone(&audio),
            });
            match run(opened, target_rate, writer, &cancel) {
                Ok((frames, truncated)) => {
                    let _ = tx.send(LoadEvent::Finished { frames, truncated });
                }
                Err(e) => {
                    let _ = tx.send(LoadEvent::Failed(e.to_string()));
                }
            }
        })?;
    Ok(rx)
}
