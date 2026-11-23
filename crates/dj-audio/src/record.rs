//! Master recording, audio-thread side: the master output (after the
//! limiter) goes into a lock-free ring that a writer thread (in the UI
//! crate) empties into a WAV or FLAC file.
//!
//! The ring carries interleaved stereo `f32`. A frame of two NaNs marks the
//! end of a recording; real samples are never NaN (they are sanitised
//! first), so the writer knows exactly where each file ends.

/// The reading end of the recording ring (see
/// [`crate::EngineHandle::take_record_reader`]): interleaved stereo, a frame
/// of two NaNs after each recording.
pub type RecordReader = rtrb::Consumer<f32>;

/// Seconds of audio the ring holds while the writer is busy.
pub const RECORD_BUFFER_SECONDS: usize = 4;

pub(crate) struct RecordTap {
    tx: rtrb::Producer<f32>,
    on: bool,
    /// The end marker still has to go into the ring (it was full).
    end_pending: bool,
    /// Frames recorded since the start.
    pub(crate) frames: u64,
    /// Frames lost because the ring was full.
    pub(crate) dropped: u64,
}

impl RecordTap {
    pub(crate) fn new(sample_rate: u32) -> (Self, rtrb::Consumer<f32>) {
        let cap = (sample_rate as usize).max(8_000) * 2 * RECORD_BUFFER_SECONDS;
        let (mut tx, mut rx) = rtrb::RingBuffer::new(cap);
        // Touch the whole ring once here, so the audio thread never takes
        // the page faults of its first pass (see `dj_dsp::prefaulted_zeros`).
        while tx.push(0.0).is_ok() {}
        while rx.pop().is_ok() {}
        (
            Self {
                tx,
                on: false,
                end_pending: false,
                frames: 0,
                dropped: 0,
            },
            rx,
        )
    }

    pub(crate) fn is_on(&self) -> bool {
        self.on
    }

    pub(crate) fn set(&mut self, on: bool) {
        if on == self.on {
            return;
        }
        if on {
            self.flush_end();
            self.on = true;
            self.frames = 0;
            self.dropped = 0;
        } else {
            self.on = false;
            self.end_pending = true;
            self.flush_end();
        }
    }

    fn flush_end(&mut self) {
        if self.end_pending && self.tx.slots() >= 2 {
            let _ = self.tx.push(f32::NAN);
            let _ = self.tx.push(f32::NAN);
            self.end_pending = false;
        }
    }

    /// Records one block of the master output.
    pub(crate) fn feed(&mut self, l: &[f32], r: &[f32]) {
        self.flush_end();
        if !self.on {
            return;
        }
        let n = l.len().min(r.len());
        let fit = n.min(self.tx.slots() / 2);
        if fit > 0
            && let Ok(chunk) = self.tx.write_chunk_uninit(fit * 2)
        {
            let samples = l
                .iter()
                .zip(r.iter())
                .take(fit)
                .flat_map(|(a, b)| [clean(*a), clean(*b)]);
            chunk.fill_from_iter(samples);
        }
        self.frames += fit as u64;
        self.dropped += (n - fit) as u64;
    }
}

#[inline]
fn clean(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}
