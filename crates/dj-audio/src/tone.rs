//! Test tone generator, used to check the master and headphone outputs.

use dj_dsp::LinearSmoother;

/// Which bus the test tone plays on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToneTarget {
    /// Master bus (NS4FX outputs 1-2).
    Master,
    /// Headphone cue bus (NS4FX outputs 3-4).
    Cue,
}

/// Test tone settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TestTone {
    pub target: ToneTarget,
    pub frequency_hz: f32,
    /// Level in dBFS. Clamped to -60..=-6 so a test tone is never full scale.
    pub level_db: f32,
}

impl TestTone {
    /// A 440 Hz tone at -18 dBFS on `target`.
    pub fn new(target: ToneTarget) -> Self {
        Self {
            target,
            frequency_hz: 440.0,
            level_db: -18.0,
        }
    }
}

/// Sine generator with a click-free fade in and out.
pub(crate) struct ToneGenerator {
    sample_rate: f32,
    phase: f64,
    increment: f64,
    target: ToneTarget,
    gain: LinearSmoother,
    active: Option<TestTone>,
}

impl ToneGenerator {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate: sample_rate as f32,
            phase: 0.0,
            increment: 0.0,
            target: ToneTarget::Master,
            gain: LinearSmoother::with_time(0.0, sample_rate, 10.0),
            active: None,
        }
    }

    pub(crate) fn set(&mut self, tone: Option<TestTone>) {
        match tone {
            Some(t) => {
                let freq = t.frequency_hz.clamp(20.0, self.sample_rate * 0.45);
                self.increment = f64::from(freq) / f64::from(self.sample_rate);
                // Switching target mid-tone would click: restart from silence.
                if self.active.is_some_and(|a| a.target != t.target) {
                    self.gain.set_immediate(0.0);
                }
                self.target = t.target;
                let level = t.level_db.clamp(-60.0, -6.0);
                self.gain.set_target(dj_core::time::db_to_gain(level));
                self.active = Some(t);
            }
            None => {
                self.gain.set_target(0.0);
                self.active = None;
            }
        }
    }

    pub(crate) fn active_target(&self) -> Option<ToneTarget> {
        self.active.map(|t| t.target)
    }

    /// Adds the tone to the given bus buffers. Does nothing when silent.
    pub(crate) fn render(
        &mut self,
        master: (&mut [f32], &mut [f32]),
        cue: (&mut [f32], &mut [f32]),
    ) {
        if self.gain.is_settled() && self.gain.current() == 0.0 {
            return;
        }
        let (l, r) = match self.target {
            ToneTarget::Master => master,
            ToneTarget::Cue => cue,
        };
        for (sl, sr) in l.iter_mut().zip(r.iter_mut()) {
            let g = self.gain.next_value();
            let s = (self.phase * std::f64::consts::TAU).sin() as f32 * g;
            self.phase += self.increment;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
            }
            *sl += s;
            *sr += s;
        }
    }
}
