//! The engine: owns all audio-thread state and renders interleaved output.

use crate::command::Command;
use crate::deck::{Deck, MasterBeat};
use crate::fx::{FALLBACK_BPM, Fx, MASTER_TARGET};
use crate::mixer::Mixer;
use crate::perf::SyncMode;
use crate::record::RecordTap;
use crate::sampler::{RecordBuffer, Returned, Sampler};
use crate::snapshot::EngineSnapshot;
use crate::tone::ToneGenerator;
use dj_core::{ChannelPair, NUM_DECKS, OutputRouting};
use dj_decode::TrackAudio;
use dj_dsp::DenormalGuard;
use std::sync::Arc;
use std::time::Instant;

/// Capacity of the UI-to-audio command queue.
pub const COMMAND_QUEUE_CAPACITY: usize = 1024;
/// Capacity of the audio-to-UI snapshot queue.
pub const SNAPSHOT_QUEUE_CAPACITY: usize = 64;
/// Capacity of the queue that hands replaced tracks back to the UI thread.
pub const RETIRE_QUEUE_CAPACITY: usize = 64;
/// Capacity of the queue that hands finished sampler recordings back.
pub const RECORDING_QUEUE_CAPACITY: usize = 8;

/// Engine construction parameters.
#[derive(Clone, Copy, Debug)]
pub struct EngineConfig {
    pub sample_rate: u32,
    /// Largest number of frames rendered in one internal sub-block. Device
    /// callbacks of any size are split into sub-blocks of at most this size,
    /// so this only sets the size of the pre-allocated scratch buffers.
    pub max_block_frames: usize,
    pub routing: OutputRouting,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            max_block_frames: 512,
            routing: OutputRouting::default(),
        }
    }
}

/// Error returned by [`EngineHandle::send`].
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("the engine command queue is full")]
    QueueFull,
}

/// Pre-allocated stereo bus buffers.
struct Buses {
    master_l: Vec<f32>,
    master_r: Vec<f32>,
    cue_l: Vec<f32>,
    cue_r: Vec<f32>,
    booth_l: Vec<f32>,
    booth_r: Vec<f32>,
}

impl Buses {
    fn new(frames: usize) -> Self {
        Self {
            master_l: vec![0.0; frames],
            master_r: vec![0.0; frames],
            cue_l: vec![0.0; frames],
            cue_r: vec![0.0; frames],
            booth_l: vec![0.0; frames],
            booth_r: vec![0.0; frames],
        }
    }

    fn clear(&mut self, n: usize) {
        for buf in [
            &mut self.master_l,
            &mut self.master_r,
            &mut self.cue_l,
            &mut self.cue_r,
            &mut self.booth_l,
            &mut self.booth_r,
        ] {
            if let Some(s) = buf.get_mut(..n) {
                s.fill(0.0);
            }
        }
    }
}

/// Measures callback time as a fraction of the callback period.
struct LoadMeter {
    last: f32,
    peak: f32,
}

/// The audio engine. Lives on the audio thread.
pub struct Engine {
    sample_rate: u32,
    max_block: usize,
    routing: OutputRouting,
    commands: rtrb::Consumer<Command>,
    /// A second command queue, fed by the controller (MIDI) thread.
    controller: rtrb::Consumer<Command>,
    snapshots: rtrb::Producer<EngineSnapshot>,
    retired: rtrb::Producer<Arc<TrackAudio>>,
    recordings: rtrb::Producer<RecordBuffer>,
    /// A finished recording waiting for room in the queue.
    recording_out: Option<RecordBuffer>,
    tone: ToneGenerator,
    mixer: Mixer,
    decks: [Deck; NUM_DECKS],
    sampler: Sampler,
    fx: Fx,
    record: RecordTap,
    buses: Buses,
    frames_rendered: u64,
    load: LoadMeter,
    master_peak: [f32; 2],
    cue_peak: [f32; 2],
    /// Samples cut at full scale (master, headphones, booth).
    clipped: [u64; 3],
    leaked_tracks: u64,
    sync_master: Option<usize>,
    sync_mode: SyncMode,
    /// A stopped master hands over to a playing synced deck.
    auto_sync_master: bool,
    quantize: bool,
    master_bpm: Option<f64>,
}

/// The controller thread's way into the engine (see
/// [`EngineHandle::take_controller_queue`]). Only commands that carry no
/// track audio belong here (no loads).
pub struct ControllerQueue {
    tx: rtrb::Producer<Command>,
}

impl ControllerQueue {
    pub fn send(&mut self, cmd: Command) -> Result<(), SendError> {
        self.tx.push(cmd).map_err(|_| SendError::QueueFull)
    }
}

/// The UI side of the engine: sends commands, receives snapshots.
pub struct EngineHandle {
    commands: rtrb::Producer<Command>,
    /// The controller queue's sending end, until a controller takes it.
    controller: Option<ControllerQueue>,
    snapshots: rtrb::Consumer<EngineSnapshot>,
    retired: rtrb::Consumer<Arc<TrackAudio>>,
    recordings: rtrb::Consumer<RecordBuffer>,
    /// The master recording ring, until the writer thread takes it.
    record_reader: Option<crate::record::RecordReader>,
    finished_recordings: Vec<RecordBuffer>,
    latest: EngineSnapshot,
    dropped_commands: u64,
}

impl Engine {
    /// Creates an engine and its UI handle.
    pub fn new(config: EngineConfig) -> (Engine, EngineHandle) {
        let (cmd_tx, cmd_rx) = rtrb::RingBuffer::new(COMMAND_QUEUE_CAPACITY);
        let (ctl_tx, ctl_rx) = rtrb::RingBuffer::new(COMMAND_QUEUE_CAPACITY);
        let (snap_tx, snap_rx) = rtrb::RingBuffer::new(SNAPSHOT_QUEUE_CAPACITY);
        let (ret_tx, ret_rx) = rtrb::RingBuffer::new(RETIRE_QUEUE_CAPACITY);
        let (rec_tx, rec_rx) = rtrb::RingBuffer::new(RECORDING_QUEUE_CAPACITY);
        let max_block = config.max_block_frames.max(16);
        let sr = config.sample_rate;
        let (record, record_reader) = RecordTap::new(sr);
        let engine = Engine {
            sample_rate: sr,
            max_block,
            routing: config.routing,
            commands: cmd_rx,
            controller: ctl_rx,
            snapshots: snap_tx,
            retired: ret_tx,
            recordings: rec_tx,
            recording_out: None,
            tone: ToneGenerator::new(sr),
            mixer: Mixer::new(sr, max_block),
            decks: std::array::from_fn(|_| Deck::new(sr, max_block)),
            sampler: Sampler::new(sr, max_block),
            fx: Fx::new(sr, max_block),
            record,
            sync_master: None,
            sync_mode: SyncMode::default(),
            auto_sync_master: false,
            quantize: false,
            master_bpm: None,
            buses: Buses::new(max_block),
            frames_rendered: 0,
            load: LoadMeter {
                last: 0.0,
                peak: 0.0,
            },
            master_peak: [0.0; 2],
            cue_peak: [0.0; 2],
            clipped: [0; 3],
            leaked_tracks: 0,
        };
        let handle = EngineHandle {
            commands: cmd_tx,
            controller: Some(ControllerQueue { tx: ctl_tx }),
            snapshots: snap_rx,
            retired: ret_rx,
            recordings: rec_rx,
            record_reader: Some(record_reader),
            finished_recordings: Vec::new(),
            latest: EngineSnapshot {
                sample_rate: sr,
                routing: config.routing,
                ..EngineSnapshot::default()
            },
            dropped_commands: 0,
        };
        (engine, handle)
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Frames the master, booth and headphone outputs lag the decks by:
    /// the master soft clipper's oversampling and the limiter's lookahead
    /// (about 2 ms).
    pub fn output_latency_frames(&self) -> usize {
        self.mixer.master_latency()
    }

    /// Renders one device callback into `out`, an interleaved buffer with
    /// `channels` channels. Real-time safe: no allocation, locks or panics.
    pub fn process(&mut self, out: &mut [f32], channels: usize) {
        let start = Instant::now();
        let _ftz = DenormalGuard::new();

        out.fill(0.0);
        if channels == 0 {
            return;
        }
        self.drain_commands();

        let frames = out.len() / channels;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(self.max_block);
            self.render_block(n);
            if let Some(chunk) = out.get_mut(done * channels..(done + n) * channels) {
                self.write_interleaved(chunk, channels, n);
            }
            done += n;
        }
        self.frames_rendered += frames as u64;
        self.measure_load(start, frames);
        self.publish_snapshot(frames);
    }

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.commands.pop() {
            self.apply(cmd);
        }
        // Commands straight from the controller thread (MIDI).
        while let Ok(cmd) = self.controller.pop() {
            self.apply(cmd);
        }
    }

    fn apply(&mut self, cmd: Command) {
        {
            match cmd {
                Command::SetTestTone(t) => self.tone.set(t),
                Command::SetRouting(r) => self.routing = r,
                Command::Mixer(m) => self.mixer.command(m),
                Command::Deck(id, cmd) => {
                    let i = id.index();
                    let reload = matches!(
                        cmd,
                        crate::deck::DeckCommand::Load { .. }
                            | crate::deck::DeckCommand::LoadWithMarkers { .. }
                            | crate::deck::DeckCommand::Eject
                    );
                    if let Some(deck) = self.decks.get_mut(i)
                        && let Some(old) = deck.command(cmd)
                    {
                        self.retire(old);
                    }
                    // A new track on the master deck: pick a master again.
                    if reload && self.sync_master == Some(i) {
                        self.sync_master = None;
                    }
                }
                Command::Sync(id, on) => self.set_sync(id.index(), on),
                Command::ToggleSync(id) => {
                    let i = id.index();
                    let on = self.decks.get(i).is_some_and(|d| d.sync_on());
                    self.set_sync(i, !on);
                }
                Command::SyncMaster(id) => {
                    let i = id.index();
                    if self
                        .decks
                        .get(i)
                        .is_some_and(|d| d.is_loaded() && d.grid().is_some())
                    {
                        self.sync_master = Some(i);
                        if let Some(d) = self.decks.get_mut(i) {
                            d.leave_follow();
                        }
                    }
                }
                Command::SetSyncMode(m) => self.sync_mode = m,
                Command::SetAutoSyncMaster(on) => self.auto_sync_master = on,
                Command::Quantize(on) => {
                    self.quantize = on;
                    for d in &mut self.decks {
                        d.quantize = on;
                    }
                }
                Command::Fx(cmd) => self.fx.command(cmd),
                Command::Record(on) => self.record.set(on),
                Command::Sampler(cmd) => match self.sampler.command(cmd) {
                    Returned::Track(Some(t)) => self.retire(t),
                    Returned::Recording(Some(r)) => self.hand_back_recording(r),
                    _ => {}
                },
            }
        }
    }

    /// SYNC pressed (`on`) or released on deck `i`.
    fn set_sync(&mut self, i: usize, on: bool) {
        let Some(deck) = self.decks.get_mut(i) else {
            return;
        };
        if !deck.set_sync(on) || !on {
            return;
        }
        let valid = |d: &Deck| d.is_loaded() && d.grid().is_some();
        let master_ok = self
            .sync_master
            .is_some_and(|m| m != i && self.decks.get(m).is_some_and(valid));
        if master_ok || self.sync_master == Some(i) {
            return;
        }
        // No master: a playing deck with a grid, else any deck with a grid,
        // else this deck leads.
        let pick = |playing: bool| {
            (0..NUM_DECKS).find(|&k| {
                k != i
                    && self
                        .decks
                        .get(k)
                        .is_some_and(|d| valid(d) && (!playing || d.is_moving()))
            })
        };
        self.sync_master = pick(true).or_else(|| pick(false)).or(Some(i));
    }

    /// Before each block: the master's tempo and phase for the followers.
    fn update_sync(&mut self) {
        if let Some(m) = self.sync_master
            && !self
                .decks
                .get(m)
                .is_some_and(|d| d.is_loaded() && d.grid().is_some())
        {
            self.sync_master = None;
        }
        // The master stopped: a playing synced deck takes over.
        if self.auto_sync_master
            && let Some(m) = self.sync_master
            && self.decks.get(m).is_some_and(|d| !d.is_moving())
            && let Some(k) = (0..NUM_DECKS).find(|&k| {
                k != m
                    && self
                        .decks
                        .get(k)
                        .is_some_and(|d| d.sync_on() && d.is_moving() && d.grid().is_some())
            })
        {
            self.sync_master = Some(k);
            if let Some(d) = self.decks.get_mut(k) {
                d.leave_follow();
            }
        }
        if self.sync_master.is_none() {
            // Synced decks without a master: a playing one leads.
            self.sync_master = (0..NUM_DECKS).find(|&k| {
                self.decks
                    .get(k)
                    .is_some_and(|d| d.sync_on() && d.is_moving() && d.grid().is_some())
            });
            if let Some(m) = self.sync_master
                && let Some(d) = self.decks.get_mut(m)
            {
                d.leave_follow();
            }
        }
        let master: Option<MasterBeat> = self
            .sync_master
            .and_then(|m| self.decks.get(m))
            .and_then(Deck::master_beat);
        // Without a master, samples follow the first playing deck with a grid.
        self.master_bpm = master.map(|m| m.bpm).or_else(|| {
            self.decks
                .iter()
                .filter(|d| d.is_moving())
                .find_map(|d| d.master_beat().map(|m| m.bpm))
        });
        let mode = self.sync_mode;
        for (k, d) in self.decks.iter_mut().enumerate() {
            if d.sync_on() && Some(k) != self.sync_master {
                d.follow(master, mode);
            }
        }
    }

    /// Hands a finished recording to the UI thread (later if the queue is
    /// full; never freed here).
    fn hand_back_recording(&mut self, r: RecordBuffer) {
        if let Some(waiting) = self.recording_out.take()
            && let Err(rtrb::PushError::Full(w)) = self.recordings.push(waiting)
        {
            // Still no room: keep the older one; never free on this thread.
            self.recording_out = Some(w);
            std::mem::forget(r);
            self.leaked_tracks += 1;
            return;
        }
        if let Err(rtrb::PushError::Full(r)) = self.recordings.push(r) {
            self.recording_out = Some(r);
        }
    }

    /// Hands a replaced track to the UI thread to free. If the queue is
    /// full, the track is leaked rather than freed here: freeing memory on
    /// the audio thread is not allowed.
    fn retire(&mut self, track: Arc<TrackAudio>) {
        if let Err(rtrb::PushError::Full(track)) = self.retired.push(track) {
            std::mem::forget(track);
            self.leaked_tracks += 1;
        }
    }

    /// Renders `n` frames (at most `max_block`) into the bus buffers.
    fn render_block(&mut self, n: usize) {
        self.update_sync();
        self.buses.clear(n);
        let b = &mut self.buses;
        let (Some(ml), Some(mr), Some(cl), Some(cr), Some(bl), Some(br)) = (
            b.master_l.get_mut(..n),
            b.master_r.get_mut(..n),
            b.cue_l.get_mut(..n),
            b.cue_r.get_mut(..n),
            b.booth_l.get_mut(..n),
            b.booth_r.get_mut(..n),
        ) else {
            return;
        };

        self.fx.begin_block(n);
        let fallback = self.master_bpm.unwrap_or(FALLBACK_BPM);
        // The master FX follow the sync master, else the first playing deck.
        let master_beat = self
            .sync_master
            .and_then(|m| self.decks.get(m))
            .and_then(Deck::fx_beat)
            .or_else(|| {
                self.decks
                    .iter()
                    .filter(|d| d.is_moving())
                    .find_map(Deck::fx_beat)
            });
        for (i, deck) in self.decks.iter_mut().enumerate() {
            let beat = deck.fx_beat();
            deck.render(n);
            if let (Some(dl), Some(dr)) = (deck.out_l.get_mut(..n), deck.out_r.get_mut(..n)) {
                self.mixer
                    .channel(i, dl, dr, (&mut *cl, &mut *cr), beat, fallback);
                self.fx.process(i, beat, fallback, dl, dr);
                self.mixer.sum_into_master(i, dl, dr, (&mut *ml, &mut *mr));
            }
        }
        // The sampler goes straight to the master bus (before master gain
        // and limiter).
        let (sl, sr) = self.sampler.render(n, self.master_bpm);
        for ((m, s), (m2, s2)) in ml
            .iter_mut()
            .zip(sl.iter())
            .zip(mr.iter_mut().zip(sr.iter()))
        {
            *m += *s;
            *m2 += *s2;
        }
        self.fx
            .process(MASTER_TARGET, master_beat, fallback, ml, mr);
        self.mixer.master(
            (&mut *ml, &mut *mr),
            (&mut *cl, &mut *cr),
            (&mut *bl, &mut *br),
        );
        let recorded = self.sampler.record(ml, mr);
        self.record.feed(ml, mr);

        // The test tone bypasses the mixer, so it is heard at a known level
        // whatever the faders are set to.
        self.tone.render((&mut *ml, &mut *mr), (&mut *cl, &mut *cr));

        // Into range for the device; every sample that has to be cut is
        // counted (a hard clip is a click).
        let clipped = &mut self.clipped;
        let mut cut = |v: &mut f32, k: usize| {
            if v.abs() > 1.0 {
                clipped[k] += 1;
            }
            *v = sanitize(*v);
        };
        for (i, (l, r)) in ml.iter_mut().zip(mr.iter_mut()).enumerate() {
            cut(l, 0);
            cut(r, 0);
            self.master_peak[0] = self.master_peak[0].max(l.abs());
            self.master_peak[1] = self.master_peak[1].max(r.abs());
            if let (Some(c0), Some(c1)) = (cl.get_mut(i), cr.get_mut(i)) {
                cut(c0, 1);
                cut(c1, 1);
                self.cue_peak[0] = self.cue_peak[0].max(c0.abs());
                self.cue_peak[1] = self.cue_peak[1].max(c1.abs());
            }
            if let (Some(b0), Some(b1)) = (bl.get_mut(i), br.get_mut(i)) {
                cut(b0, 2);
                cut(b1, 2);
            }
        }
        if let Some(done) = recorded {
            self.hand_back_recording(done);
        }
    }

    /// Copies bus buffers into the interleaved device buffer according to
    /// the routing.
    fn write_interleaved(&self, out: &mut [f32], channels: usize, n: usize) {
        let r = self.routing;
        let b = &self.buses;
        for (i, frame) in out.chunks_exact_mut(channels).take(n).enumerate() {
            let ml = b.master_l.get(i).copied().unwrap_or(0.0);
            let mr = b.master_r.get(i).copied().unwrap_or(0.0);
            let cl = b.cue_l.get(i).copied().unwrap_or(0.0);
            let cr = b.cue_r.get(i).copied().unwrap_or(0.0);
            // (Already in range: see `render_block`.)
            let bl = b.booth_l.get(i).copied().unwrap_or(0.0);
            let br = b.booth_r.get(i).copied().unwrap_or(0.0);
            if r.split_cue {
                set_pair(frame, r.master, 0.5 * (cl + cr), 0.5 * (ml + mr));
            } else {
                set_pair(frame, r.master, ml, mr);
                if let Some(cue) = r.cue {
                    set_pair(frame, cue, cl, cr);
                }
            }
            if let Some(booth) = r.booth {
                set_pair(frame, booth, bl, br);
            }
        }
    }

    fn measure_load(&mut self, start: Instant, frames: usize) {
        if frames == 0 {
            return;
        }
        let period = frames as f32 / self.sample_rate as f32;
        let load = start.elapsed().as_secs_f32() / period;
        self.load.last = load;
        // Peak hold with a slow decay (about 3 s at typical callback rates).
        self.load.peak = (self.load.peak * 0.998).max(load);
    }

    fn publish_snapshot(&mut self, frames: usize) {
        let mut snap = EngineSnapshot {
            sample_rate: self.sample_rate,
            frames_rendered: self.frames_rendered,
            last_block_frames: frames as u32,
            callback_load: self.load.last,
            callback_load_peak: self.load.peak,
            master_peak: self.master_peak,
            cue_peak: self.cue_peak,
            test_tone: self.tone.active_target(),
            routing: self.routing,
            leaked_tracks: self.leaked_tracks,
            mixer: *self.mixer.params(),
            sync_master: self.sync_master,
            sync_mode: self.sync_mode,
            quantize: self.quantize,
            master_bpm: self.master_bpm,
            sampler: self.sampler.snapshot(),
            fx: self.fx.snapshot(),
            recording: self.record.is_on(),
            record_frames: self.record.frames,
            record_dropped: self.record.dropped,
            clipped: self.clipped,
            ..EngineSnapshot::default()
        };
        for (s, d) in snap.decks.iter_mut().zip(self.decks.iter_mut()) {
            *s = d.snapshot();
        }
        let (channels, master, cue, limiter_gain) = self.mixer.meters();
        snap.channel_meters = channels;
        snap.master_meter = master;
        snap.cue_meter = cue;
        snap.limiter_gain = limiter_gain;
        // If the UI is not reading, skip this snapshot; the next one carries
        // the newer state anyway. Peaks accumulate until one is delivered.
        if self.snapshots.push(snap).is_ok() {
            self.master_peak = [0.0; 2];
            self.cue_peak = [0.0; 2];
            for d in &mut self.decks {
                d.clear_peak();
            }
            self.mixer.clear_peaks();
        }
    }
}

#[inline]
fn sanitize(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[inline]
fn set_pair(frame: &mut [f32], pair: ChannelPair, l: f32, r: f32) {
    if let Some(s) = frame.get_mut(pair.left as usize) {
        *s = l;
    }
    if let Some(s) = frame.get_mut(pair.right as usize) {
        *s = r;
    }
}

impl EngineHandle {
    /// The sending end of the engine's second command queue, for the
    /// controller (MIDI) thread: its commands reach the audio thread at the
    /// next block, without waiting for a UI frame. Given out once per engine.
    pub fn take_controller_queue(&mut self) -> Option<ControllerQueue> {
        self.controller.take()
    }

    /// The reading end of the master recording ring, for the writer
    /// thread. Given out once per engine.
    pub fn take_record_reader(&mut self) -> Option<crate::record::RecordReader> {
        self.record_reader.take()
    }

    /// Queues a command for the audio thread.
    pub fn send(&mut self, cmd: Command) -> Result<(), SendError> {
        self.commands.push(cmd).map_err(|_| {
            self.dropped_commands += 1;
            SendError::QueueFull
        })
    }

    /// Reads all pending snapshots and returns the newest. Peak values are
    /// merged across the snapshots read. Also frees tracks the engine no
    /// longer uses.
    pub fn update(&mut self) -> &EngineSnapshot {
        while let Ok(track) = self.retired.pop() {
            drop(track);
        }
        while let Ok(rec) = self.recordings.pop() {
            self.finished_recordings.push(rec);
        }
        let mut master_peak = [0.0f32; 2];
        let mut cue_peak = [0.0f32; 2];
        let mut deck_peaks = [[0.0f32; 2]; NUM_DECKS];
        let mut channel_peaks = [[0.0f32; 2]; NUM_DECKS];
        let mut master_meter_peak = [0.0f32; 2];
        let mut cue_meter_peak = [0.0f32; 2];
        let mut any = false;
        while let Ok(s) = self.snapshots.pop() {
            for c in 0..2 {
                master_peak[c] = master_peak[c].max(s.master_peak[c]);
                cue_peak[c] = cue_peak[c].max(s.cue_peak[c]);
                master_meter_peak[c] = master_meter_peak[c].max(s.master_meter.peak[c]);
                cue_meter_peak[c] = cue_meter_peak[c].max(s.cue_meter.peak[c]);
                for (p, d) in deck_peaks.iter_mut().zip(s.decks.iter()) {
                    p[c] = p[c].max(d.peak[c]);
                }
                for (p, m) in channel_peaks.iter_mut().zip(s.channel_meters.iter()) {
                    p[c] = p[c].max(m.peak[c]);
                }
            }
            self.latest = s;
            any = true;
        }
        if any {
            self.latest.master_peak = master_peak;
            self.latest.cue_peak = cue_peak;
            self.latest.master_meter.peak = master_meter_peak;
            self.latest.cue_meter.peak = cue_meter_peak;
            for (d, p) in self.latest.decks.iter_mut().zip(deck_peaks) {
                d.peak = p;
            }
            for (m, p) in self.latest.channel_meters.iter_mut().zip(channel_peaks) {
                m.peak = p;
            }
        }
        &self.latest
    }

    /// The newest snapshot read by [`Self::update`].
    pub fn snapshot(&self) -> &EngineSnapshot {
        &self.latest
    }

    /// Number of commands dropped because the queue was full.
    pub fn dropped_commands(&self) -> u64 {
        self.dropped_commands
    }

    /// Sampler recordings finished since the last call (read by
    /// [`Self::update`]).
    pub fn take_recordings(&mut self) -> Vec<RecordBuffer> {
        std::mem::take(&mut self.finished_recordings)
    }
}
