//! Key detection: a chroma vector (energy per pitch class) summed over the
//! track, correlated with the Krumhansl-Kessler key profiles for all 24 keys.

use dj_core::MusicalKey;
use realfft::RealFftPlanner;

/// Krumhansl-Kessler probe-tone profiles (C major / C minor).
const MAJOR: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// Lowest note analysed (MIDI 33 = A1, 55 Hz) and the number of notes: five
/// whole octaves up to A6 (1760 Hz), so every pitch class is counted
/// equally often. (An earlier version summed FFT bins per pitch class over
/// 60-2100 Hz; bins are evenly spaced in Hz, so some pitch classes got more
/// bins than others and white noise came out as A minor.)
const LOWEST_MIDI: i32 = 33;
const NOTES: usize = 60;
/// Half-width of the moving average that estimates the broadband floor
/// around each note, in notes (one octave each way).
const FLOOR_HALF_WIDTH: usize = 12;
/// Expected excess of a note over the floor on pure noise, in noise
/// deviations: the mean of the positive part of a zero-mean normal variable
/// is 1 / sqrt(2 pi) = 0.4 deviations. Subtracting it makes noise add nothing
/// on average, whatever the note's bin count, while real notes (10-30 dB
/// above the floor) lose only a dB or two.
const NOISE_EXCESS: f64 = 0.4;
/// Overtones 2 to 6 as (semitones above the fundamental, weight) for
/// harmonic summation in [`chroma`].
const HARMONICS: [(usize, f64); 5] = [(12, 0.8), (19, 0.64), (24, 0.51), (28, 0.41), (31, 0.33)];

/// FFT bins of each note: the bins whose centre lies within the note's
/// semitone, or (low notes narrower than a bin) the bin nearest its centre.
/// `tuning` shifts every note by that many semitones (see [`estimate_tuning`]).
fn note_bins(win: usize, sr: f64, tuning: f64) -> Vec<std::ops::Range<usize>> {
    let bin_hz = sr / win as f64;
    (0..NOTES)
        .map(|i| {
            let midi = f64::from(LOWEST_MIDI) + i as f64 + tuning;
            let f = 440.0 * ((midi - 69.0) / 12.0).exp2();
            let lo = (f * (-1.0f64 / 24.0).exp2() / bin_hz).ceil() as usize;
            let hi = (f * (1.0f64 / 24.0).exp2() / bin_hz).ceil() as usize;
            if hi > lo {
                lo..hi
            } else {
                let c = (f / bin_hz).round() as usize;
                c..c + 1
            }
        })
        .collect()
}

/// FFT frame length for key analysis: at 44.1 kHz a bin is 2.7 Hz, enough
/// to separate bass notes.
fn key_window(sample_rate: u32) -> usize {
    if sample_rate > 50_000 { 32_768 } else { 16_384 }
}

fn hann(win: usize) -> Vec<f32> {
    (0..win)
        .map(|n| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / win as f64).cos()) as f32)
        .collect()
}

/// Peaks used per frame for the tuning estimate.
const TUNING_PEAKS_PER_FRAME: usize = 24;

/// Global tuning offset of the music in semitones, in [-0.5, 0.5): how far
/// its notes sit from the A = 440 Hz grid.
///
/// Sped-up or pitched samples (common in bootlegs and bassline edits) are
/// often off by a fraction of a semitone; every note then splits its energy
/// over two pitch classes and the key comes out wrong (several tracks
/// had their strongest pitch class outside their key). The strongest
/// spectral peaks between 150 Hz and 2 kHz, located to a fraction of a bin
/// by parabolic interpolation, vote for their distance to the nearest
/// semitone (a circular mean, weighted by magnitude).
pub fn estimate_tuning(mono: &[f32], sample_rate: u32) -> f64 {
    let sr = f64::from(sample_rate.max(1));
    let win = key_window(sample_rate);
    let bin_hz = sr / win as f64;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(win);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let window = hann(win);
    let lo = (150.0 / bin_hz).ceil() as usize;
    let hi = ((2_000.0 / bin_hz) as usize).min(win / 2 - 1);
    let (mut sum_cos, mut sum_sin) = (0.0f64, 0.0f64);
    let mut mags = vec![0.0f64; win / 2 + 1];
    let mut peaks: Vec<(f64, usize)> = Vec::new();
    let mut start = 0;
    while start + win <= mono.len() {
        for ((i, w), s) in input
            .iter_mut()
            .zip(window.iter())
            .zip(&mono[start..start + win])
        {
            *i = w * s;
        }
        start += win;
        if fft
            .process_with_scratch(&mut input, &mut spectrum, &mut scratch)
            .is_err()
        {
            continue;
        }
        for (m, c) in mags.iter_mut().zip(spectrum.iter()) {
            *m = f64::from(c.norm());
        }
        peaks.clear();
        for k in lo.max(1)..hi {
            if mags[k] > mags[k - 1] && mags[k] >= mags[k + 1] {
                peaks.push((mags[k], k));
            }
        }
        peaks.sort_by(|a, b| b.0.total_cmp(&a.0));
        for &(m, k) in peaks.iter().take(TUNING_PEAKS_PER_FRAME) {
            let (a, b, c) = (
                (mags[k - 1] + 1e-12).ln(),
                (m + 1e-12).ln(),
                (mags[k + 1] + 1e-12).ln(),
            );
            let denom = a - 2.0 * b + c;
            let delta = if denom.abs() > 1e-12 {
                (0.5 * (a - c) / denom).clamp(-0.5, 0.5)
            } else {
                0.0
            };
            let f = (k as f64 + delta) * bin_hz;
            let midi = 69.0 + 12.0 * (f / 440.0).log2();
            let angle = 2.0 * std::f64::consts::PI * (midi - midi.round());
            sum_cos += m * angle.cos();
            sum_sin += m * angle.sin();
        }
    }
    if sum_cos == 0.0 && sum_sin == 0.0 {
        return 0.0;
    }
    let t = sum_sin.atan2(sum_cos) / (2.0 * std::f64::consts::PI);
    if t >= 0.5 { t - 1.0 } else { t }
}

/// Chroma vector of a mono signal: tonal strength per pitch class, C = 0.
///
/// The note grid follows the music's tuning ([`estimate_tuning`]). Per
/// frame: mean power per note (so broadband sound counts equally for every
/// note), in dB; the broadband floor (moving average over an octave each
/// way) is subtracted so mainly tonal peaks remain; the notes are folded
/// into 12 pitch classes and the frame is normalised.
pub fn chroma(mono: &[f32], sample_rate: u32) -> [f64; 12] {
    let sr = f64::from(sample_rate.max(1));
    let win = key_window(sample_rate);
    let hop = win / 2;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(win);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let window = hann(win);
    let bins = note_bins(win, sr, estimate_tuning(mono, sample_rate));
    // How far a note's level wanders on pure noise, in dB: the level of a
    // mean of n bins' power varies by about 5.6 / sqrt(n) dB (Hann-windowed
    // bins are correlated, so count them as n / 1.5). Notes with few bins
    // (the bottom of each octave block) wander more and would collect more
    // random excess over the floor, biasing the chroma; NOISE_EXCESS removes
    // that expected excess per note.
    let wander: Vec<f64> = bins
        .iter()
        .map(|r| 5.6 / (r.len() as f64 / 1.5).max(1.0).sqrt())
        .collect();
    let mut level = [0.0f64; NOTES];
    let mut acc = [0.0f64; 12];
    let mut start = 0;
    while start + win <= mono.len() {
        for ((i, w), s) in input
            .iter_mut()
            .zip(window.iter())
            .zip(&mono[start..start + win])
        {
            *i = w * s;
        }
        start += hop;
        if fft
            .process_with_scratch(&mut input, &mut spectrum, &mut scratch)
            .is_err()
        {
            continue;
        }
        let mut loudest = f64::MIN;
        for (l, range) in level.iter_mut().zip(bins.iter()) {
            let band = spectrum.get(range.clone()).unwrap_or(&[]);
            let power = band.iter().map(|c| f64::from(c.norm_sqr())).sum::<f64>()
                / band.len().max(1) as f64;
            *l = 10.0 * (power + 1e-20).log10();
            loudest = loudest.max(*l);
        }
        // Skip silence (nothing within 100 dB of full scale).
        if loudest < -100.0 + 10.0 * (win as f64).log10() {
            continue;
        }
        let mut tonal = [0.0f64; NOTES];
        let mut total = 0.0;
        for (i, (l, t)) in level.iter().zip(tonal.iter_mut()).enumerate() {
            let a = i.saturating_sub(FLOOR_HALF_WIDTH);
            let b = (i + FLOOR_HALF_WIDTH + 1).min(NOTES);
            let floor = level[a..b].iter().sum::<f64>() / (b - a) as f64;
            let excess = (l - floor).max(0.0);
            total += excess;
            *t = excess - NOISE_EXCESS * wander[i];
        }
        let mut frame = [0.0f64; 12];
        for i in 0..NOTES {
            // Harmonic summation: a note also gets credit from the notes at
            // its overtone positions, so real roots stand out and the
            // overtones of a rich bass (the 5th harmonic is a major third)
            // do not read as notes of their own.
            let salience = tonal[i]
                + HARMONICS
                    .iter()
                    .filter_map(|(up, w)| tonal.get(i + up).map(|t| w * t))
                    .sum::<f64>();
            let pc = (LOWEST_MIDI + i as i32).rem_euclid(12) as usize;
            frame[pc] += salience;
        }
        // Normalise per frame (by the raw excess) so loud passages do not
        // dominate.
        if total > 1e-6 {
            for (a, f) in acc.iter_mut().zip(frame.iter()) {
                *a += f / total;
            }
        }
    }
    acc
}

fn pearson(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let ma = a.iter().sum::<f64>() / 12.0;
    let mb = b.iter().sum::<f64>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        num += (a[i] - ma) * (b[i] - mb);
        da += (a[i] - ma).powi(2);
        db += (b[i] - mb).powi(2);
    }
    if da <= 0.0 || db <= 0.0 {
        0.0
    } else {
        num / (da * db).sqrt()
    }
}

/// Best key for a chroma vector, with a confidence (correlation margin over
/// the runner-up, 0 to 2).
pub fn key_from_chroma(chroma: &[f64; 12]) -> Option<(MusicalKey, f32)> {
    if chroma.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    let mut scores: Vec<(f64, MusicalKey)> = Vec::with_capacity(24);
    for tonic in 0..12u8 {
        for (minor, profile) in [(false, &MAJOR), (true, &MINOR)] {
            let mut rotated = [0.0; 12];
            for (i, r) in rotated.iter_mut().enumerate() {
                *r = profile[(i + 12 - usize::from(tonic)) % 12];
            }
            scores.push((pearson(chroma, &rotated), MusicalKey::new(tonic, minor)));
        }
    }
    scores.sort_by(|a, b| b.0.total_cmp(&a.0));
    let best = scores.first()?;
    let second = scores.get(1).map_or(0.0, |s| s.0);
    Some((best.1, (best.0 - second) as f32))
}

/// Detects the key of a mono signal.
pub fn detect_key(mono: &[f32], sample_rate: u32) -> Option<(MusicalKey, f32)> {
    key_from_chroma(&chroma(mono, sample_rate))
}
