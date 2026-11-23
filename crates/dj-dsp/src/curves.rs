//! Fader, crossfader, EQ and filter knob curves.
//!
//! The tapers: dB-mapped channel faders,
//! constant-power / linear / scratch-cut crossfaders with a centre dead
//! zone, and asymmetric EQ knobs (strong cut on the left half, gentle boost
//! on the right, a snapped 0 dB detent in the middle).

/// Channel fader (upfader) curve, from the slowest fade to the fastest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FaderCurve {
    /// Club mixer taper, linear in dB: -60 dB just above the bottom up to
    /// -6 dB at 80 % travel, then -6 dB to 0 dB over the top 20 %. Fine
    /// control near the top, a smooth fade to silence at the bottom.
    #[default]
    Club,
    /// Power taper, `gain = x^2.2` (about -13 dB at half travel).
    Smooth,
    /// Gain proportional to travel.
    Linear,
    /// Fast start: most of the level in the first part of the travel.
    Sharp,
}

impl FaderCurve {
    pub const ALL: [FaderCurve; 4] = [
        FaderCurve::Club,
        FaderCurve::Smooth,
        FaderCurve::Linear,
        FaderCurve::Sharp,
    ];

    pub fn gain(self, x: f32) -> f32 {
        let x = x.clamp(0.0, 1.0);
        if x <= 0.0 {
            return 0.0;
        }
        match self {
            FaderCurve::Club => {
                let db = if x <= 0.8 {
                    -60.0 + 54.0 * (x / 0.8)
                } else {
                    -6.0 + 6.0 * ((x - 0.8) / 0.2)
                };
                if x >= 1.0 { 1.0 } else { db_to_gain(db) }
            }
            FaderCurve::Smooth => x.powf(2.2),
            FaderCurve::Linear => x,
            FaderCurve::Sharp => x.sqrt(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FaderCurve::Club => "Club (dB)",
            FaderCurve::Smooth => "Smooth",
            FaderCurve::Linear => "Linear",
            FaderCurve::Sharp => "Sharp",
        }
    }

    /// Stable name for the config file.
    pub fn id(self) -> &'static str {
        match self {
            FaderCurve::Club => "club",
            FaderCurve::Smooth => "smooth",
            FaderCurve::Linear => "linear",
            FaderCurve::Sharp => "sharp",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// Crossfader positions closer to the middle than this count as the middle.
pub const CROSSFADER_DEAD_ZONE: f32 = 0.015;
/// Default scratch cut-in distance (in crossfader units, -1 to 1).
pub const DEFAULT_CUT_LAG: f32 = 0.03;
/// Allowed scratch cut-in distances.
pub const CUT_LAG_RANGE: (f32, f32) = (0.005, 0.05);

/// Crossfader curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CrossfaderCurve {
    /// Constant power (`a² + b² = 1`): no loudness dip in long blends.
    #[default]
    Smooth,
    /// Linear crossfade: both sides at half level (-6 dB) in the middle.
    Dip,
    /// Both sides at full level in the middle; each fades out linearly over
    /// the far half.
    Linear,
    /// Both sides at full level over the middle 60 %, fading at the ends.
    Sharp,
    /// Scratch cut: full level until the last few millimetres (the cut-in
    /// distance), then a short fade.
    Cut,
}

impl CrossfaderCurve {
    pub const ALL: [CrossfaderCurve; 5] = [
        CrossfaderCurve::Smooth,
        CrossfaderCurve::Dip,
        CrossfaderCurve::Linear,
        CrossfaderCurve::Sharp,
        CrossfaderCurve::Cut,
    ];

    /// Gains `(a, b)` for crossfader position `x` from -1 (full A) to 1
    /// (full B), with the default cut-in distance.
    pub fn gains(self, x: f32) -> (f32, f32) {
        self.gains_with_lag(x, DEFAULT_CUT_LAG)
    }

    /// Gains `(a, b)` with a given scratch cut-in distance `lag` (only
    /// [`CrossfaderCurve::Cut`] uses it).
    pub fn gains_with_lag(self, x: f32, lag: f32) -> (f32, f32) {
        let mut pos = x.clamp(-1.0, 1.0);
        if pos.abs() < CROSSFADER_DEAD_ZONE {
            pos = 0.0;
        }
        let t = (pos + 1.0) * 0.5;
        match self {
            CrossfaderCurve::Smooth => {
                let th = t * std::f32::consts::FRAC_PI_2;
                let (a, b) = (th.cos(), th.sin());
                // Exact ends.
                (
                    if pos >= 1.0 { 0.0 } else { a },
                    if pos <= -1.0 { 0.0 } else { b },
                )
            }
            CrossfaderCurve::Dip => (1.0 - t, t),
            CrossfaderCurve::Linear => ((1.0 - pos).clamp(0.0, 1.0), (1.0 + pos).clamp(0.0, 1.0)),
            CrossfaderCurve::Sharp => (((1.0 - t) * 5.0).min(1.0), (t * 5.0).min(1.0)),
            CrossfaderCurve::Cut => {
                let lag = lag.clamp(CUT_LAG_RANGE.0, CUT_LAG_RANGE.1);
                (
                    ((1.0 - pos) / lag).clamp(0.0, 1.0),
                    ((pos + 1.0) / lag).clamp(0.0, 1.0),
                )
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CrossfaderCurve::Smooth => "Smooth",
            CrossfaderCurve::Dip => "Dip",
            CrossfaderCurve::Linear => "Linear",
            CrossfaderCurve::Sharp => "Sharp",
            CrossfaderCurve::Cut => "Cut",
        }
    }

    /// Stable name for the config file.
    pub fn id(self) -> &'static str {
        match self {
            CrossfaderCurve::Smooth => "smooth",
            CrossfaderCurve::Dip => "dip",
            CrossfaderCurve::Linear => "linear",
            CrossfaderCurve::Sharp => "sharp",
            CrossfaderCurve::Cut => "cut",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// The channel EQ's topology.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EqMode {
    /// Full-kill isolator: three parallel Linkwitz-Riley (LR4) bands; each
    /// knob runs from silence (fully left) through 0 dB to the boost.
    #[default]
    Isolator,
    /// Classic mixer EQ: low shelf, mid bell and high shelf in series
    /// (RBJ cookbook, shelf slope 1, bell Q 0.7); -26 dB fully left.
    Classic,
}

impl EqMode {
    pub const ALL: [EqMode; 2] = [EqMode::Isolator, EqMode::Classic];

    pub fn label(self) -> &'static str {
        match self {
            EqMode::Isolator => "Isolator (full kill)",
            EqMode::Classic => "Classic (shelf / bell)",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            EqMode::Isolator => "isolator",
            EqMode::Classic => "classic",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// Knob values (0 to 1) this close to the middle snap to 0 dB: MIDI 63 to
/// 65 on a 7-bit knob.
pub const EQ_DETENT: f32 = 1.5 / 127.0;
/// Deepest cut of the classic EQ knob (fully left).
pub const CLASSIC_EQ_MIN_DB: f32 = -26.0;
/// Level of a killed band in classic mode (a shelf cannot reach silence).
pub const CLASSIC_KILL_DB: f32 = -60.0;
/// EQ boosts the setup offers.
pub const EQ_BOOSTS_DB: [f32; 2] = [6.0, 12.0];

/// Where an EQ knob is: `Some(norm)` below the detent (0 = fully left, 1 =
/// at the detent), `None` at the detent, or the boost side as `Err(norm)`
/// (0 = at the detent, 1 = fully right).
fn eq_side(v: f32) -> Result<Option<f32>, f32> {
    let v = v.clamp(0.0, 1.0);
    if (v - 0.5).abs() <= EQ_DETENT {
        Ok(None)
    } else if v < 0.5 {
        Ok(Some((v / (0.5 - EQ_DETENT)).clamp(0.0, 1.0)))
    } else {
        Err(((v - 0.5 - EQ_DETENT) / (0.5 - EQ_DETENT)).clamp(0.0, 1.0))
    }
}

/// Isolator knob position 0..1 to linear band gain: 0 is silence, the
/// middle (with its detent) is 1.0, fully right is the boost. Both sides
/// are linear in amplitude, as measured on the reference EQ: half-way down
/// is -6 dB, a quarter -12 dB.
pub fn isolator_gain(v: f32, boost_db: f32) -> f32 {
    match eq_side(v) {
        Ok(None) => 1.0,
        Ok(Some(norm)) => norm,
        Err(norm) => 1.0 + norm * (db_to_gain(boost_db) - 1.0),
    }
}

/// Classic EQ knob position 0..1 to band gain in dB: -26 dB fully left
/// (`-26 * (1 - norm)^1.5`), 0 dB in the middle, `boost_db` fully right.
pub fn classic_eq_db(v: f32, boost_db: f32) -> f32 {
    match eq_side(v) {
        Ok(None) => 0.0,
        Ok(Some(norm)) => CLASSIC_EQ_MIN_DB * (1.0 - norm).powf(1.5),
        Err(norm) => boost_db * norm,
    }
}

/// EQ knob position 0..1 to linear gain with the isolator taper and a
/// +6 dB boost (kept for callers that only need the default).
pub fn eq_knob_gain(v: f32) -> f32 {
    isolator_gain(v, 6.0)
}

/// Trim knob position -1..1 to linear gain: -12 dB to +12 dB.
pub fn trim_gain(v: f32) -> f32 {
    10f32.powf(12.0 * v.clamp(-1.0, 1.0) / 20.0)
}

/// dB to linear gain.
#[inline]
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_crossfader_is_constant_power() {
        for i in 0..=20 {
            let x = -1.0 + i as f32 * 0.1;
            let (a, b) = CrossfaderCurve::Smooth.gains(x);
            assert!((a * a + b * b - 1.0).abs() < 1e-5);
        }
        let (a, b) = CrossfaderCurve::Smooth.gains(0.0);
        assert!((a - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-5);
        assert!((a - b).abs() < 1e-6);
    }

    #[test]
    fn crossfader_ends() {
        for c in CrossfaderCurve::ALL {
            let (a, b) = c.gains(-1.0);
            assert!((a - 1.0).abs() < 1e-6 && b.abs() < 1e-6, "{c:?}");
            let (a, b) = c.gains(1.0);
            assert!(a.abs() < 1e-6 && (b - 1.0).abs() < 1e-6, "{c:?}");
        }
        assert_eq!(CrossfaderCurve::Cut.gains(0.9), (1.0, 1.0));
        assert_eq!(CrossfaderCurve::Sharp.gains(0.0), (1.0, 1.0));
        assert_eq!(CrossfaderCurve::Linear.gains(0.0), (1.0, 1.0));
        assert_eq!(CrossfaderCurve::Dip.gains(0.0), (0.5, 0.5));
    }

    #[test]
    fn crossfader_centre_dead_zone() {
        for c in CrossfaderCurve::ALL {
            assert_eq!(c.gains(0.01), c.gains(0.0), "{c:?}");
            assert_eq!(c.gains(-0.01), c.gains(0.0), "{c:?}");
        }
    }

    #[test]
    fn cut_lag_sets_the_fade_distance() {
        let (a, _) = CrossfaderCurve::Cut.gains_with_lag(1.0 - 0.01, 0.02);
        assert!((a - 0.5).abs() < 1e-3, "{a}");
        let (a, _) = CrossfaderCurve::Cut.gains_with_lag(1.0 - 0.03, 0.02);
        assert_eq!(a, 1.0);
    }

    #[test]
    fn fader_curves_are_monotonic_with_fixed_ends() {
        for c in FaderCurve::ALL {
            assert_eq!(c.gain(0.0), 0.0);
            assert_eq!(c.gain(1.0), 1.0);
            let mut prev = 0.0;
            for i in 1..=100 {
                let g = c.gain(i as f32 / 100.0);
                assert!(g >= prev, "{c:?}");
                prev = g;
            }
        }
    }

    #[test]
    fn club_fader_points() {
        let db = |x: f32| 20.0 * FaderCurve::Club.gain(x).log10();
        assert!((db(0.8) + 6.0).abs() < 1e-3);
        assert!((db(0.9) + 3.0).abs() < 1e-3);
        assert!((db(0.4) + 33.0).abs() < 1e-3);
        assert!((db(1e-4) + 60.0).abs() < 0.1);
    }

    #[test]
    fn eq_knob_points() {
        assert_eq!(isolator_gain(0.0, 6.0), 0.0);
        assert_eq!(isolator_gain(0.5, 6.0), 1.0);
        // MIDI 63, 64 and 65 all snap to 0 dB.
        for m in [63.0f32, 64.0, 65.0] {
            assert_eq!(isolator_gain(m / 127.0, 6.0), 1.0, "{m}");
            assert_eq!(classic_eq_db(m / 127.0, 6.0), 0.0, "{m}");
        }
        assert!((isolator_gain(1.0, 6.0) - db_to_gain(6.0)).abs() < 1e-5);
        assert!((isolator_gain(1.0, 12.0) - db_to_gain(12.0)).abs() < 1e-4);
        // Quarter turn left: half the cut travel, half the amplitude.
        let q = isolator_gain(0.25 - EQ_DETENT / 2.0, 6.0);
        assert!((q - 0.5).abs() < 1e-3, "{q}");
        // Measured on the reference (deep in the band): MIDI 8, 16, 32 at
        // about -17, -11.5 and -5.8 dB.
        for (m, want) in [(8.0f32, -17.0f32), (16.0, -11.5), (32.0, -5.8)] {
            let db = 20.0 * isolator_gain(m / 127.0, 6.0).log10();
            assert!((db - want).abs() < 1.0, "MIDI {m}: {db}");
        }
        assert_eq!(classic_eq_db(0.0, 6.0), -26.0);
        assert!((classic_eq_db(1.0, 12.0) - 12.0).abs() < 1e-5);
        assert!((trim_gain(0.0) - 1.0).abs() < 1e-6);
        assert_eq!(eq_knob_gain(0.5), 1.0);
    }

    #[test]
    fn ids_round_trip() {
        for c in FaderCurve::ALL {
            assert_eq!(FaderCurve::from_id(c.id()), Some(c));
        }
        for c in CrossfaderCurve::ALL {
            assert_eq!(CrossfaderCurve::from_id(c.id()), Some(c));
        }
        for c in EqMode::ALL {
            assert_eq!(EqMode::from_id(c.id()), Some(c));
        }
    }
}
