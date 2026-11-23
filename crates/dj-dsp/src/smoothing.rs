//! Parameter smoothing. Every user-facing parameter that changes gain or
//! filter settings goes through a smoother so changes do not click.

/// Moves linearly from the current value to a new target over a fixed number
/// of frames. When a new target arrives mid-ramp, the ramp restarts from the
/// current value, so there is never a jump.
#[derive(Clone, Debug)]
pub struct LinearSmoother {
    current: f32,
    target: f32,
    step: f32,
    remaining: u32,
    ramp_frames: u32,
}

impl LinearSmoother {
    /// Creates a smoother resting at `value` that ramps over `ramp_frames`.
    pub fn new(value: f32, ramp_frames: u32) -> Self {
        Self {
            current: value,
            target: value,
            step: 0.0,
            remaining: 0,
            ramp_frames: ramp_frames.max(1),
        }
    }

    /// Creates a smoother whose ramp lasts `ms` milliseconds at `sample_rate`.
    pub fn with_time(value: f32, sample_rate: u32, ms: f32) -> Self {
        let frames = (sample_rate as f32 * ms / 1000.0).round().max(1.0) as u32;
        Self::new(value, frames)
    }

    /// Sets a new target. Does nothing if the target is unchanged.
    pub fn set_target(&mut self, target: f32) {
        if target == self.target {
            return;
        }
        self.target = target;
        self.remaining = self.ramp_frames;
        self.step = (target - self.current) / self.ramp_frames as f32;
    }

    /// Jumps straight to `value` with no ramp.
    pub fn set_immediate(&mut self, value: f32) {
        self.current = value;
        self.target = value;
        self.remaining = 0;
        self.step = 0.0;
    }

    /// Advances one frame and returns the new value.
    #[inline]
    pub fn next_value(&mut self) -> f32 {
        if self.remaining > 0 {
            self.remaining -= 1;
            if self.remaining == 0 {
                self.current = self.target;
            } else {
                self.current += self.step;
            }
        }
        self.current
    }

    /// Advances `frames` frames at once and returns the new value. Useful
    /// for parameters that are updated once per sub-block.
    #[inline]
    pub fn advance(&mut self, frames: u32) -> f32 {
        if self.remaining > 0 {
            if frames >= self.remaining {
                self.remaining = 0;
                self.current = self.target;
            } else {
                self.remaining -= frames;
                self.current += self.step * frames as f32;
            }
        }
        self.current
    }

    /// Current value without advancing.
    pub fn current(&self) -> f32 {
        self.current
    }

    /// Target value.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// True when the smoother has reached its target.
    pub fn is_settled(&self) -> bool {
        self.remaining == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn reaches_target_exactly_after_ramp() {
        let mut s = LinearSmoother::new(0.0, 10);
        s.set_target(1.0);
        let mut last = 0.0;
        for _ in 0..10 {
            let v = s.next_value();
            assert!(v >= last, "monotonic");
            last = v;
        }
        assert_eq!(s.current(), 1.0);
        assert!(s.is_settled());
    }

    #[test]
    fn with_time_computes_frames() {
        let mut s = LinearSmoother::with_time(0.0, 48_000, 10.0);
        s.set_target(1.0);
        for _ in 0..479 {
            s.next_value();
        }
        assert!(s.current() < 1.0);
        s.next_value();
        assert_eq!(s.current(), 1.0);
    }

    #[test]
    fn advance_matches_per_sample() {
        let mut a = LinearSmoother::new(0.2, 100);
        let mut b = a.clone();
        a.set_target(0.9);
        b.set_target(0.9);
        for _ in 0..37 {
            a.next_value();
        }
        b.advance(37);
        assert!((a.current() - b.current()).abs() < 1e-5);
    }

    proptest! {
        /// No single step is larger than the full jump divided by the ramp
        /// length (no zipper steps), whatever targets arrive mid-ramp.
        #[test]
        fn steps_are_bounded(targets in prop::collection::vec(-1.0f32..1.0, 1..20), ramp in 1u32..400) {
            let mut s = LinearSmoother::new(0.0, ramp);
            let mut prev = s.current();
            for t in targets {
                s.set_target(t);
                for _ in 0..(ramp / 3 + 1) {
                    let v = s.next_value();
                    prop_assert!((v - prev).abs() <= 2.0 / ramp as f32 + 1e-5);
                    prev = v;
                }
            }
        }
    }
}
