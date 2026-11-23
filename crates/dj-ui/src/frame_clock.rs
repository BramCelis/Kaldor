//! Clocks for smooth waveform scrolling.
//!
//! Two sources of wobble are handled here:
//!
//! - The UI code of a frame runs a few milliseconds earlier or later from
//!   frame to frame, but the screen shows frames at a fixed rate (vsync).
//!   Advancing the waveforms by the measured time between two frames adds
//!   that jitter to the scroll. [`FramePacer`] turns the measured frame
//!   times into a steady clock that advances in whole refresh periods.
//! - The engine reports positions once per audio block, and the UI sees a
//!   report anywhere from 0 to one block period after it was made.
//!   [`AudioClock`] estimates where the audio clock is *now* from the
//!   reports that arrived with the least delay.

/// Steady frame clock: advances by whole display refresh periods, learns the
/// period from the frame times and follows the measured time slowly.
#[derive(Clone, Copy, Debug)]
pub struct FramePacer {
    /// Display refresh period (seconds), learned from the frame times.
    period: f64,
    /// Paced time of the last frame and the measured time it came from.
    last: Option<(f64, f64)>,
}

impl Default for FramePacer {
    fn default() -> Self {
        Self {
            period: 1.0 / 60.0,
            last: None,
        }
    }
}

impl FramePacer {
    /// How fast the paced clock follows the measured one (per frame).
    const FOLLOW: f64 = 0.03;
    /// How fast the period estimate learns (per frame).
    const LEARN: f64 = 0.02;

    /// The refresh period in use (seconds).
    pub fn period(&self) -> f64 {
        self.period
    }

    /// Takes the measured time of a new frame (seconds, from any fixed
    /// start) and returns `(paced time, paced time since the last frame)`.
    pub fn tick(&mut self, measured: f64) -> (f64, f64) {
        let Some((paced, prev)) = self.last else {
            self.last = Some((measured, measured));
            return (measured, 0.0);
        };
        let dm = measured - prev;
        if dm.is_nan() || dm <= 0.0 {
            // The same frame again (or time went backwards): no step.
            return (paced, 0.0);
        }
        // Learn the period from steps near it (a dropped frame or a stall is
        // two or more periods and teaches nothing).
        if dm < 1.5 * self.period {
            let step = dm.clamp(0.5 * self.period, 1.5 * self.period);
            self.period =
                (self.period + (step - self.period) * Self::LEARN).clamp(1.0 / 400.0, 1.0 / 20.0);
        }
        let mut next = paced + (dm / self.period).round().max(1.0) * self.period;
        let off = measured - next;
        if off.abs() > 3.0 * self.period {
            // A stall or a jump in time: start again from the measured time.
            next = measured;
        } else {
            next += off * Self::FOLLOW;
        }
        self.last = Some((next, measured));
        (next, next - paced)
    }
}

/// The audio clock as seen from the UI.
///
/// Each report says "`frames` frames rendered" and arrives some time after
/// the block was rendered. The report with the shortest delay gives the
/// largest `frames / rate - time`; that offset (slowly lowered, so a
/// slightly slower audio clock is followed too) turns any UI time into an
/// audio frame count. The offset in use glides towards that best offset, so
/// a quicker report does not make the waveforms jump.
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioClock {
    /// `frames / rate - time` of the best recent report (seconds).
    best: Option<f64>,
    /// The offset in use, gliding towards `best`.
    offset: Option<f64>,
    /// The last report and when it was seen.
    frames: u64,
    seen_at: f64,
}

impl AudioClock {
    /// How fast the offset may sink (seconds per second): covers the drift
    /// between the audio device clock and the system clock.
    const SINK: f64 = 0.0005;
    /// A report this far behind the estimate means the audio stalled or
    /// restarted: start again from it.
    const RESET: f64 = 0.1;
    /// Share of the distance to `best` the offset in use covers per report.
    const GLIDE: f64 = 0.1;

    /// Takes a report: `frames` rendered so far, seen at `time` (seconds).
    pub fn observe(&mut self, frames: u64, time: f64, rate: f64) {
        let v = frames as f64 / rate - time;
        let restarted = frames < self.frames;
        let best = match self.best {
            Some(o) if !restarted => {
                let sunk = o - Self::SINK * (time - self.seen_at).max(0.0);
                if v < sunk - Self::RESET {
                    v
                } else {
                    sunk.max(v)
                }
            }
            _ => v,
        };
        self.best = Some(best);
        self.offset = Some(match self.offset {
            Some(o) if !restarted && (best - o).abs() < Self::RESET => o + (best - o) * Self::GLIDE,
            _ => best,
        });
        self.frames = frames;
        self.seen_at = time;
    }

    /// Audio frames rendered by `time` (estimate), never more than `limit`
    /// seconds ahead of or behind the last report.
    pub fn frames_at(&self, time: f64, rate: f64, limit: f64) -> Option<f64> {
        let o = self.offset?;
        let last = self.frames as f64;
        Some(((o + time) * rate).clamp(last - limit * rate, last + limit * rate))
    }

    /// The last reported frame count.
    pub fn last_frames(&self) -> u64 {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic noise source (no dependency needed).
    fn noise(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((*seed >> 33) as f64 / (1u64 << 31) as f64) - 0.5
    }

    #[test]
    fn pacer_turns_jittery_frame_times_into_even_steps() {
        let period = 1.0 / 60.0;
        let mut p = FramePacer::default();
        let mut seed = 7;
        let mut steps = Vec::new();
        for i in 0..600 {
            // Vsync-locked frames whose UI code runs up to ±3 ms off.
            let measured = 5.0 + i as f64 * period + noise(&mut seed) * 0.006;
            steps.push(p.tick(measured).1);
        }
        for s in &steps[60..] {
            assert!((s - period).abs() < 0.0005, "step {s}");
        }
        // A dropped frame is two periods, not a smeared step.
        let last = p.tick(5.0 + 600.0 * period).0;
        let (t, dt) = p.tick(5.0 + 602.0 * period + 0.001);
        assert!((dt - 2.0 * period).abs() < 0.001, "dt {dt}");
        assert!((t - last - dt).abs() < 1e-12);
    }

    #[test]
    fn pacer_learns_a_faster_display() {
        let period = 1.0 / 144.0;
        let mut p = FramePacer::default();
        for i in 0..1_000 {
            p.tick(i as f64 * period);
        }
        assert!((p.period() - period).abs() < 0.0002, "{}", p.period());
        let (_, dt) = p.tick(1_000.0 * period);
        assert!((dt - period).abs() < 0.0003, "{dt}");
    }

    #[test]
    fn audio_clock_hides_the_report_delay() {
        // 48 kHz, 480-frame blocks (10 ms); each report is seen 0-10 ms
        // after its block. The estimate at the frame times must advance
        // evenly although the raw reports do not.
        let rate = 48_000.0;
        let mut c = AudioClock::default();
        let mut seed = 3;
        let mut est = Vec::new();
        for i in 0..400 {
            let t = i as f64 / 60.0;
            let rendered = ((t * rate) as u64 / 480) * 480;
            let seen = t + noise(&mut seed).abs() * 0.01;
            c.observe(rendered, seen, rate);
            est.push(c.frames_at(seen, rate, 0.05).unwrap() - seen * rate);
        }
        // The estimate minus the true time is nearly constant once a
        // short-delay report has been seen.
        let tail = &est[60..];
        let (lo, hi) = tail
            .iter()
            .fold((f64::MAX, f64::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        assert!(hi - lo < 0.002 * rate, "spread {} frames", hi - lo);
        // And it never jumps by more than half a millisecond between frames.
        let worst = tail
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f64::max);
        assert!(worst < 0.0005 * rate, "step {worst} frames");
    }

    #[test]
    fn audio_clock_restarts_with_a_new_engine() {
        let rate = 48_000.0;
        let mut c = AudioClock::default();
        c.observe(480_000, 10.0, rate);
        c.observe(960, 10.5, rate);
        let f = c.frames_at(10.5, rate, 0.05).unwrap();
        assert!((f - 960.0).abs() < 1.0, "{f}");
    }
}
