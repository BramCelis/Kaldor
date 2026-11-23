//! Master recording, writer side: a thread empties the engine's record ring
//! (see `dj_audio::record`) into 24-bit WAV or FLAC files.
//!
//! The UI starts a recording by telling the writer the file first
//! ([`Recorder::start`]) and then the engine (`Command::Record(true)`); it
//! stops it with `Command::Record(false)`. The engine marks the end in the
//! ring, so the file ends exactly where the recording did, however far
//! behind the writer is.
//!
//! WAV files are split before they reach 4 GB (about 4 hours at 48 kHz):
//! the next part continues in `<name>-part2.wav` and so on. FLAC files have
//! no such limit.

use dj_audio::RecordReader;
use dj_core::config::RecordFormat;
use flacenc::component::{BitRepr, Stream, StreamInfo};
use flacenc::error::Verify;
use flacenc::source::{Context, Fill, FrameBuf};
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

/// WAV parts are closed before this many bytes of audio.
const WAV_PART_BYTES: u64 = 3_900_000_000;
/// Frames per FLAC block.
const FLAC_BLOCK: usize = 4096;
/// How often the writer looks at the ring.
const POLL: Duration = Duration::from_millis(20);

/// What the writer reports back.
#[derive(Clone, Debug, PartialEq)]
pub enum RecorderEvent {
    /// A file was opened (also each new WAV part).
    Started(PathBuf),
    /// A file was finished.
    Finished {
        path: PathBuf,
        frames: u64,
        bytes: u64,
    },
    /// Something went wrong; the recording into this file stopped.
    Failed(String),
}

enum Msg {
    Start {
        path: PathBuf,
        format: RecordFormat,
        sample_rate: u32,
        split: Option<u64>,
    },
    Quit,
}

/// The writer thread. Dropping it finishes the open file with what has
/// arrived so far.
pub struct Recorder {
    tx: mpsc::Sender<Msg>,
    events: mpsc::Receiver<RecorderEvent>,
    thread: Option<JoinHandle<()>>,
}

impl Recorder {
    pub fn spawn(reader: RecordReader) -> Self {
        let (tx, rx) = mpsc::channel();
        let (ev_tx, events) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("kaldor-recorder".into())
            .spawn(move || Writer::new(reader, ev_tx).run(rx))
            .ok();
        Self { tx, events, thread }
    }

    /// The next recording goes to `path`. Send `Command::Record(true)` to
    /// the engine right after.
    pub fn start(&self, path: PathBuf, format: RecordFormat, sample_rate: u32) {
        self.start_split(path, format, sample_rate, None);
    }

    /// Like [`Self::start`], starting a new part every `split_frames`
    /// frames (`None`: only WAV's 4 GB limit splits).
    pub fn start_split(
        &self,
        path: PathBuf,
        format: RecordFormat,
        sample_rate: u32,
        split_frames: Option<u64>,
    ) {
        let _ = self.tx.send(Msg::Start {
            path,
            format,
            sample_rate,
            split: split_frames.filter(|s| *s > 0),
        });
    }

    /// Events since the last call.
    pub fn poll(&self) -> Vec<RecorderEvent> {
        self.events.try_iter().collect()
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Pending {
    path: PathBuf,
    format: RecordFormat,
    sample_rate: u32,
    split: Option<u64>,
}

struct Writer {
    reader: RecordReader,
    events: mpsc::Sender<RecorderEvent>,
    file: Option<OpenFile>,
    /// A start that waits for the previous file's end marker.
    next: Option<Pending>,
    /// Left sample waiting for its right one.
    half: Option<f32>,
}

struct OpenFile {
    out: Output,
    /// Path of the recording (part 1).
    base: PathBuf,
    path: PathBuf,
    format: RecordFormat,
    sample_rate: u32,
    part: u32,
    frames: u64,
    /// Start a new part after this many frames.
    split: Option<u64>,
}

enum Output {
    Wav(hound::WavWriter<BufWriter<File>>),
    Flac(Box<FlacWriter>),
}

impl Writer {
    fn new(reader: RecordReader, events: mpsc::Sender<RecorderEvent>) -> Self {
        Self {
            reader,
            events,
            file: None,
            next: None,
            half: None,
        }
    }

    fn run(mut self, rx: mpsc::Receiver<Msg>) {
        loop {
            match rx.recv_timeout(POLL) {
                Ok(Msg::Start {
                    path,
                    format,
                    sample_rate,
                    split,
                }) => {
                    let p = Pending {
                        path,
                        format,
                        sample_rate,
                        split,
                    };
                    if self.file.is_some() {
                        self.next = Some(p);
                    } else {
                        self.open(p);
                    }
                }
                Ok(Msg::Quit) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.drain();
                    self.finish();
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.drain();
        }
    }

    fn report(&self, e: RecorderEvent) {
        let _ = self.events.send(e);
    }

    fn open(&mut self, p: Pending) {
        match open_output(&p.path, p.format, p.sample_rate) {
            Ok(out) => {
                self.report(RecorderEvent::Started(p.path.clone()));
                self.file = Some(OpenFile {
                    out,
                    base: p.path.clone(),
                    path: p.path,
                    format: p.format,
                    sample_rate: p.sample_rate,
                    part: 1,
                    frames: 0,
                    split: p.split,
                });
            }
            Err(e) => self.report(RecorderEvent::Failed(format!(
                "could not create {}: {e}",
                p.path.display()
            ))),
        }
    }

    /// Reads everything in the ring.
    fn drain(&mut self) {
        let n = self.reader.slots();
        if n == 0 {
            return;
        }
        let Ok(chunk) = self.reader.read_chunk(n) else {
            return;
        };
        let (a, b) = chunk.as_slices();
        let samples: Vec<f32> = a.iter().chain(b.iter()).copied().collect();
        chunk.commit_all();
        for s in samples {
            match self.half.take() {
                None => self.half = Some(s),
                Some(l) => self.frame(l, s),
            }
        }
    }

    fn frame(&mut self, l: f32, r: f32) {
        if l.is_nan() && r.is_nan() {
            self.finish();
            if let Some(p) = self.next.take() {
                self.open(p);
            }
            return;
        }
        let Some(f) = self.file.as_mut() else {
            return;
        };
        let result = match &mut f.out {
            Output::Wav(w) => w
                .write_sample(to_i24(l))
                .and_then(|_| w.write_sample(to_i24(r)))
                .map_err(|e| e.to_string()),
            Output::Flac(w) => w.push(to_i24(l), to_i24(r)),
        };
        if let Err(e) = result {
            let msg = format!("{}: {e}", f.path.display());
            self.file = None;
            self.report(RecorderEvent::Failed(msg));
            return;
        }
        f.frames += 1;
        let wav_full = f.format == RecordFormat::Wav && f.frames * 6 >= WAV_PART_BYTES;
        if wav_full || f.split.is_some_and(|s| f.frames >= s) {
            self.next_part();
        }
    }

    /// Closes a full WAV part and goes on in the next one.
    fn next_part(&mut self) {
        let Some(f) = self.file.as_ref() else {
            return;
        };
        let (base, format, rate, part, split) =
            (f.base.clone(), f.format, f.sample_rate, f.part + 1, f.split);
        self.finish();
        let path = part_path(&base, part);
        match open_output(&path, format, rate) {
            Ok(out) => {
                self.report(RecorderEvent::Started(path.clone()));
                self.file = Some(OpenFile {
                    out,
                    base,
                    path,
                    format,
                    sample_rate: rate,
                    part,
                    frames: 0,
                    split,
                });
            }
            Err(e) => self.report(RecorderEvent::Failed(format!(
                "could not create {}: {e}",
                path.display()
            ))),
        }
    }

    fn finish(&mut self) {
        let Some(f) = self.file.take() else {
            return;
        };
        let result = match f.out {
            Output::Wav(w) => w.finalize().map_err(|e| e.to_string()),
            Output::Flac(w) => w.finish(),
        };
        match result {
            Ok(()) => {
                let bytes = std::fs::metadata(&f.path).map(|m| m.len()).unwrap_or(0);
                self.report(RecorderEvent::Finished {
                    path: f.path,
                    frames: f.frames,
                    bytes,
                });
            }
            Err(e) => self.report(RecorderEvent::Failed(format!("{}: {e}", f.path.display()))),
        }
    }
}

fn open_output(path: &Path, format: RecordFormat, sample_rate: u32) -> Result<Output, String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    match format {
        RecordFormat::Wav => {
            let spec = hound::WavSpec {
                channels: 2,
                sample_rate,
                bits_per_sample: 24,
                sample_format: hound::SampleFormat::Int,
            };
            hound::WavWriter::create(path, spec)
                .map(Output::Wav)
                .map_err(|e| e.to_string())
        }
        RecordFormat::Flac => {
            FlacWriter::create(path, sample_rate).map(|w| Output::Flac(Box::new(w)))
        }
    }
}

/// `name.wav` -> `name-part2.wav`.
fn part_path(base: &Path, part: u32) -> PathBuf {
    let stem = base
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "recording".into());
    let ext = base
        .extension()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "wav".into());
    base.with_file_name(format!("{stem}-part{part}.{ext}"))
}

#[inline]
fn to_i24(v: f32) -> i32 {
    ((v.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32).clamp(-8_388_608, 8_388_607)
}

/// A FLAC file written frame by frame; the header (with the length and MD5)
/// is written again at the end.
struct FlacWriter {
    file: BufWriter<File>,
    info: StreamInfo,
    config: flacenc::error::Verified<flacenc::config::Encoder>,
    md5: Context,
    block: Vec<i32>,
    frame_buf: FrameBuf,
    frame_number: usize,
    sink: flacenc::bitsink::ByteSink,
}

impl FlacWriter {
    fn create(path: &Path, sample_rate: u32) -> Result<Self, String> {
        let info = StreamInfo::new(sample_rate as usize, 2, 24).map_err(|e| format!("{e:?}"))?;
        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(|(_, e)| format!("{e:?}"))?;
        let frame_buf = FrameBuf::with_size(2, FLAC_BLOCK).map_err(|e| format!("{e:?}"))?;
        let mut w = Self {
            file: BufWriter::new(File::create(path).map_err(|e| e.to_string())?),
            info,
            config,
            md5: Context::new(24, 2),
            block: Vec::with_capacity(FLAC_BLOCK * 2),
            frame_buf,
            frame_number: 0,
            sink: flacenc::bitsink::ByteSink::new(),
        };
        w.write_header()?;
        Ok(w)
    }

    fn write_header(&mut self) -> Result<(), String> {
        self.sink.clear();
        Stream::with_stream_info(self.info.clone())
            .write(&mut self.sink)
            .map_err(|e| format!("{e:?}"))?;
        self.file
            .write_all(self.sink.as_slice())
            .map_err(|e| e.to_string())
    }

    fn push(&mut self, l: i32, r: i32) -> Result<(), String> {
        self.block.push(l);
        self.block.push(r);
        if self.block.len() >= FLAC_BLOCK * 2 {
            self.encode_block()?;
        }
        Ok(())
    }

    fn encode_block(&mut self) -> Result<(), String> {
        if self.block.is_empty() {
            return Ok(());
        }
        self.frame_buf
            .fill_interleaved(&self.block)
            .map_err(|e| format!("{e:?}"))?;
        self.md5
            .fill_interleaved(&self.block)
            .map_err(|e| format!("{e:?}"))?;
        let frame = flacenc::encode_fixed_size_frame(
            &self.config,
            &self.frame_buf,
            self.frame_number,
            &self.info,
        )
        .map_err(|e| format!("{e:?}"))?;
        self.info.update_frame_info(&frame);
        self.sink.clear();
        frame.write(&mut self.sink).map_err(|e| format!("{e:?}"))?;
        self.file
            .write_all(self.sink.as_slice())
            .map_err(|e| e.to_string())?;
        self.frame_number += 1;
        self.block.clear();
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        self.encode_block()?;
        // The FLAC format leaves the (shorter) last block out of the minimum
        // block size: every block but the last is FLAC_BLOCK frames.
        if self.frame_number > 1 {
            self.info
                .set_block_sizes(FLAC_BLOCK, FLAC_BLOCK)
                .map_err(|e| format!("{e:?}"))?;
        }
        self.info.set_md5_digest(&self.md5.md5_digest());
        self.file.flush().map_err(|e| e.to_string())?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        self.write_header()?;
        self.file.flush().map_err(|e| e.to_string())
    }
}

/// `Kaldor 2026-11-12 21-30-05` in local time (UTC where the platform gives
/// no local time without extra libraries).
pub fn timestamp_name() -> String {
    let (y, mo, d, h, mi, s) = now_parts();
    format!("Kaldor {y:04}-{mo:02}-{d:02} {h:02}-{mi:02}-{s:02}")
}

/// Local date and time: (year, month, day, hour, minute, second).
#[cfg(windows)]
pub(crate) fn now_parts() -> (u32, u32, u32, u32, u32, u32) {
    // SAFETY: GetLocalTime only writes the returned struct; no pointers.
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (
        u32::from(t.wYear),
        u32::from(t.wMonth),
        u32::from(t.wDay),
        u32::from(t.wHour),
        u32::from(t.wMinute),
        u32::from(t.wSecond),
    )
}

/// Local date and time (UTC on platforms without a local time here).
#[cfg(not(windows))]
pub(crate) fn now_parts() -> (u32, u32, u32, u32, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y as u32,
        m as u32,
        d as u32,
        (rem / 3600) as u32,
        (rem / 60 % 60) as u32,
        (rem % 60) as u32,
    )
}

/// The default recordings folder: `Music/Kaldor Recordings` in the home
/// folder (the current folder if there is no home).
pub fn default_folder() -> PathBuf {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Music").join("Kaldor Recordings")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_names_and_sample_conversion() {
        assert_eq!(
            part_path(Path::new("a/Kaldor x.wav"), 2),
            PathBuf::from("a/Kaldor x-part2.wav")
        );
        assert_eq!(to_i24(1.0), 8_388_607);
        assert_eq!(to_i24(-1.5), -8_388_607);
        assert_eq!(to_i24(0.0), 0);
        let name = timestamp_name();
        assert!(
            name.starts_with("Kaldor 20") && name.len() == "Kaldor 2026-11-12 21-30-05".len(),
            "{name}"
        );
    }
}
