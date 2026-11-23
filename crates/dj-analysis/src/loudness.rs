//! EBU R128 integrated loudness, for auto-gain.

use dj_decode::TrackAudio;
use ebur128::{EbuR128, Mode};

/// Integrated loudness of a stereo track in LUFS, or `None` for silence.
pub fn integrated_lufs(audio: &TrackAudio) -> Option<f64> {
    let mut meter = EbuR128::new(2, audio.sample_rate(), Mode::I).ok()?;
    let mut buf = vec![0.0f32; 8192 * 2];
    let total = audio.ready_frames();
    let mut pos = 0u64;
    while pos < total {
        let n = ((total - pos) as usize).min(8192);
        audio.read_interleaved(pos, &mut buf[..n * 2]);
        meter.add_frames_f32(&buf[..n * 2]).ok()?;
        pos += n as u64;
    }
    let l = meter.loudness_global().ok()?;
    l.is_finite().then_some(l)
}

/// Gain in dB that brings a track at `lufs` to `target`, limited to ±12 dB.
pub fn auto_gain_db(lufs: f64, target: f64) -> f64 {
    (target - lufs).clamp(-12.0, 12.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ebu_reference_sine_reads_minus_23() {
        // EBU Tech 3341 test 1: stereo 1 kHz sine at -23 dBFS reads -23 LUFS.
        let amp = 10f64.powf(-23.0 / 20.0);
        let s = crate::synth::tones(48_000, &[1000.0], 20.0, amp);
        let t = TrackAudio::from_interleaved(48_000, &s);
        let l = integrated_lufs(&t).unwrap();
        assert!((l + 23.0).abs() < 0.1, "{l}");
        assert!((auto_gain_db(l, -12.0) - 11.0).abs() < 0.1);
        assert_eq!(auto_gain_db(-40.0, -12.0), 12.0);
    }

    #[test]
    fn silence_has_no_loudness() {
        let t = TrackAudio::from_interleaved(48_000, &vec![0.0; 96_000]);
        assert!(integrated_lufs(&t).is_none());
    }
}
