//! Offline rendering: runs the engine without a device, for tests, benchmarks
//! and the smoke test. Deterministic: the same commands give the same output.

use crate::engine::Engine;

/// Drives an [`Engine`] in fixed-size blocks, like a device callback would.
pub struct OfflineRenderer {
    engine: Engine,
    channels: usize,
    block_frames: usize,
    scratch: Vec<f32>,
}

impl OfflineRenderer {
    pub fn new(engine: Engine, channels: usize, block_frames: usize) -> Self {
        let channels = channels.max(1);
        let block_frames = block_frames.max(1);
        Self {
            engine,
            channels,
            block_frames,
            scratch: vec![0.0; channels * block_frames],
        }
    }

    pub fn engine(&mut self) -> &mut Engine {
        &mut self.engine
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Renders one callback-sized block and returns it (interleaved).
    pub fn render_block(&mut self) -> &[f32] {
        self.engine.process(&mut self.scratch, self.channels);
        &self.scratch
    }

    /// Renders at least `frames` frames (rounded up to whole blocks) and
    /// returns them interleaved.
    pub fn render(&mut self, frames: usize) -> Vec<f32> {
        let blocks = frames.div_ceil(self.block_frames);
        let mut out = Vec::with_capacity(blocks * self.block_frames * self.channels);
        for _ in 0..blocks {
            self.engine.process(&mut self.scratch, self.channels);
            out.extend_from_slice(&self.scratch);
        }
        out
    }
}

/// Extracts one channel from an interleaved buffer.
pub fn channel(interleaved: &[f32], channels: usize, index: usize) -> Vec<f32> {
    interleaved
        .chunks_exact(channels)
        .filter_map(|f| f.get(index).copied())
        .collect()
}

/// RMS level of a signal.
pub fn rms(signal: &[f32]) -> f32 {
    if signal.is_empty() {
        return 0.0;
    }
    let sum: f64 = signal.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    (sum / signal.len() as f64).sqrt() as f32
}
