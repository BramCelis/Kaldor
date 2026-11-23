//! Time math. Deck positions are sample-accurate `f64` frame counters at the
//! engine sample rate; these helpers convert between frames, seconds and beats.

/// Converts a frame position to seconds.
pub fn frames_to_seconds(frames: f64, sample_rate: u32) -> f64 {
    frames / f64::from(sample_rate)
}

/// Converts seconds to a frame position.
pub fn seconds_to_frames(seconds: f64, sample_rate: u32) -> f64 {
    seconds * f64::from(sample_rate)
}

/// Length of one beat in frames at `bpm`.
pub fn beat_length_frames(bpm: f64, sample_rate: u32) -> f64 {
    60.0 * f64::from(sample_rate) / bpm
}

/// Formats seconds as `m:ss.t` (tenths), e.g. `3:07.4`. Negative values get
/// a leading minus sign.
pub fn format_mm_ss_t(seconds: f64) -> String {
    let neg = seconds < 0.0;
    let tenths_total = (seconds.abs() * 10.0).floor() as u64;
    let tenths = tenths_total % 10;
    let secs_total = tenths_total / 10;
    let m = secs_total / 60;
    let s = secs_total % 60;
    format!("{}{m}:{s:02}.{tenths}", if neg { "-" } else { "" })
}

/// Converts decibels to a linear gain factor.
pub fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// Converts a linear gain to decibels. Returns `-inf` for zero.
pub fn gain_to_db(gain: f32) -> f32 {
    20.0 * gain.log10()
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn formats() {
        assert_eq!(format_mm_ss_t(0.0), "0:00.0");
        assert_eq!(format_mm_ss_t(187.46), "3:07.4");
        assert_eq!(format_mm_ss_t(-5.0), "-0:05.0");
    }

    #[test]
    fn beat_length() {
        assert!((beat_length_frames(120.0, 48_000) - 24_000.0).abs() < 1e-9);
    }

    proptest! {
        #[test]
        fn seconds_frames_roundtrip(s in 0.0f64..10_000.0, sr in prop::sample::select(vec![44_100u32, 48_000, 96_000])) {
            let back = frames_to_seconds(seconds_to_frames(s, sr), sr);
            prop_assert!((back - s).abs() < 1e-9);
        }

        #[test]
        fn db_roundtrip(db in -90.0f32..12.0) {
            prop_assert!((gain_to_db(db_to_gain(db)) - db).abs() < 1e-3);
        }
    }
}
