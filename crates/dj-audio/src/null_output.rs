//! A fake output device: a normal thread that runs the engine at real-time
//! pace and throws the audio away. Used when no device is available and in
//! tests, so nothing ever touches real hardware.

use crate::device::{AudioOutput, OutputInfo};
use crate::engine::Engine;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Runs an engine on a background thread at real-time pace.
pub struct NullOutput {
    info: OutputInfo,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<Engine>>,
}

impl NullOutput {
    /// Starts the thread. `channels` and `block_frames` mimic a device.
    pub fn start(engine: Engine, channels: u16, block_frames: u32) -> std::io::Result<Self> {
        let sample_rate = engine.sample_rate();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        let channels_usize = usize::from(channels.max(1));
        let block = block_frames.max(16) as usize;
        let thread = std::thread::Builder::new()
            .name("kaldor-null-audio".into())
            .spawn(move || {
                let mut engine = engine;
                let mut buf = vec![0.0f32; block * channels_usize];
                let period = Duration::from_secs_f64(block as f64 / f64::from(sample_rate));
                let mut next = Instant::now();
                while !stop_flag.load(Ordering::Acquire) {
                    engine.process(&mut buf, channels_usize);
                    next += period;
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        // Fell behind (debugger, slow machine): resync.
                        next = now;
                    }
                }
                engine
            })?;
        Ok(Self {
            info: OutputInfo {
                host: "null".into(),
                device: "Null output (no audio device)".into(),
                sample_rate,
                channels,
                buffer_frames: Some(block_frames.max(16)),
                sample_format: "f32".into(),
            },
            stop,
            thread: Some(thread),
        })
    }

    /// Stops the thread and returns the engine.
    pub fn stop(mut self) -> Option<Engine> {
        self.stop_inner()
    }

    fn stop_inner(&mut self) -> Option<Engine> {
        self.stop.store(true, Ordering::Release);
        self.thread.take().and_then(|t| t.join().ok())
    }
}

impl Drop for NullOutput {
    fn drop(&mut self) {
        let _ = self.stop_inner();
    }
}

impl AudioOutput for NullOutput {
    fn info(&self) -> &OutputInfo {
        &self.info
    }

    fn xruns(&self) -> u64 {
        0
    }

    fn output_latency(&self) -> Option<Duration> {
        self.info
            .buffer_frames
            .map(|b| Duration::from_secs_f64(f64::from(b) / f64::from(self.info.sample_rate)))
    }

    fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}
