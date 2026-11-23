//! BPM and beatgrid detection.
//!
//! 1. Onset envelope: multi-band spectral flux (log-compressed magnitude
//!    spectra, half-wave rectified frame differences, summed over six bands
//!    after per-band normalisation).
//! 2. Tempo: autocorrelation of the envelope scored with a harmonic comb
//!    inside the configured BPM range, with a mild prior around 120 BPM to
//!    settle octave ambiguity.
//! 3. Beats: dynamic-programming beat tracking (after D. Ellis, "Beat
//!    Tracking by Dynamic Programming", 2007).
//! 4. Each beat is refined to about 1 ms using a short-block energy envelope
//!    of the signal, then a robust least-squares line through the beats gives
//!    the BPM and the grid phase.
//! 5. The downbeat is the beat phase (mod 4) with the strongest low-band
//!    onsets.

use dj_core::Beatgrid;
use realfft::RealFftPlanner;

/// Tempo search settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TempoOptions {
    pub min_bpm: f64,
    pub max_bpm: f64,
}

impl Default for TempoOptions {
    fn default() -> Self {
        Self {
            min_bpm: 70.0,
            max_bpm: 180.0,
        }
    }
}

/// Onset envelope with its frame timing.
#[derive(Clone, Debug)]
pub struct OnsetEnvelope {
    /// Summed onset strength per frame.
    pub values: Vec<f32>,
    /// Low-band (below 150 Hz) onset strength per frame.
    pub low: Vec<f32>,
    /// Frames per second.
    pub fps: f64,
    /// Time of frame 0 (centre of the first analysis window), seconds.
    pub offset: f64,
}

const BAND_EDGES_HZ: [f64; 7] = [0.0, 150.0, 400.0, 1000.0, 3000.0, 8000.0, 24_000.0];

/// Computes the onset envelope of a mono signal.
// Frame and bin indices address several parallel arrays; index loops read
// clearer than zipped iterators here.
#[allow(clippy::needless_range_loop)]
pub fn onset_envelope(mono: &[f32], sample_rate: u32) -> OnsetEnvelope {
    let sr = f64::from(sample_rate.max(1));
    let win = if sample_rate > 50_000 { 2048 } else { 1024 };
    let hop = win / 4;
    let n_frames = if mono.len() >= win {
        (mono.len() - win) / hop + 1
    } else {
        0
    };
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(win);
    let mut input = fft.make_input_vec();
    let mut spectrum = fft.make_output_vec();
    let mut scratch = fft.make_scratch_vec();
    let window: Vec<f32> = (0..win)
        .map(|n| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / win as f64).cos()) as f32)
        .collect();
    let bins = win / 2 + 1;
    let band_of: Vec<usize> = (0..bins)
        .map(|k| {
            let f = k as f64 * sr / win as f64;
            BAND_EDGES_HZ
                .windows(2)
                .position(|e| f >= e[0] && f < e[1])
                .unwrap_or(BAND_EDGES_HZ.len() - 2)
        })
        .collect();
    let n_bands = BAND_EDGES_HZ.len() - 1;
    let mut prev = vec![0.0f32; bins];
    let mut cur = vec![0.0f32; bins];
    let mut flux = vec![vec![0.0f32; n_frames]; n_bands];
    for t in 0..n_frames {
        let start = t * hop;
        for ((i, w), s) in input
            .iter_mut()
            .zip(window.iter())
            .zip(&mono[start..start + win])
        {
            *i = w * s;
        }
        if fft
            .process_with_scratch(&mut input, &mut spectrum, &mut scratch)
            .is_err()
        {
            continue;
        }
        for (c, s) in cur.iter_mut().zip(spectrum.iter()) {
            *c = (1.0 + 100.0 * s.norm()).ln();
        }
        if t > 0 {
            for k in 1..bins {
                let d = cur[k] - prev[k];
                if d > 0.0 {
                    flux[band_of[k]][t] += d;
                }
            }
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    // Normalise each band by its mean so quiet bands count as much as loud
    // ones, then sum.
    let mut values = vec![0.0f32; n_frames];
    for band in &flux {
        let mean = band.iter().sum::<f32>() / band.len().max(1) as f32 + 1e-9;
        for (v, b) in values.iter_mut().zip(band.iter()) {
            *v += b / mean;
        }
    }
    let low_mean = flux[0].iter().sum::<f32>() / n_frames.max(1) as f32 + 1e-9;
    let low: Vec<f32> = flux[0].iter().map(|v| v / low_mean).collect();
    // Remove the slowly varying part (about 0.25 s moving average) and keep
    // the positive part.
    let fps = sr / hop as f64;
    let half = ((0.125 * fps) as usize).max(1);
    let values = detrend(&values, half);
    OnsetEnvelope {
        values,
        low,
        fps,
        offset: win as f64 / 2.0 / sr,
    }
}

fn detrend(x: &[f32], half: usize) -> Vec<f32> {
    let n = x.len();
    let mut prefix = vec![0.0f64; n + 1];
    for (i, v) in x.iter().enumerate() {
        prefix[i + 1] = prefix[i] + f64::from(*v);
    }
    (0..n)
        .map(|i| {
            let a = i.saturating_sub(half);
            let b = (i + half + 1).min(n);
            let mean = (prefix[b] - prefix[a]) / (b - a) as f64;
            (f64::from(x[i]) - mean).max(0.0) as f32
        })
        .collect()
}

fn interp(x: &[f64], pos: f64) -> f64 {
    let i = pos.floor();
    let f = pos - i;
    let i = i as usize;
    match (x.get(i), x.get(i + 1)) {
        (Some(a), Some(b)) => a + (b - a) * f,
        (Some(a), None) => *a,
        _ => 0.0,
    }
}

/// Tempo score for every 0.1 BPM step in the configured range (the curve
/// [`estimate_bpm`] maximises before its 3:2 check). Empty if the envelope
/// is too short.
pub fn tempo_scores(env: &OnsetEnvelope, opts: TempoOptions) -> Vec<(f64, f64)> {
    let Some(scorer) = TempoScorer::new(env, opts) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut bpm = opts.min_bpm;
    while bpm <= opts.max_bpm {
        out.push((bpm, scorer.score(bpm)));
        bpm += 0.1;
    }
    out
}

/// How much more strongly the kick band must repeat at a tempo 1.5x faster
/// or slower before [`estimate_bpm`] switches to it.
const THREE_TWO_SWITCH_RATIO: f64 = 1.5;
/// ...and the least repetition it must show there.
const THREE_TWO_MIN_KICK: f64 = 0.1;

/// Estimates the tempo in BPM from an onset envelope.
///
/// The best-scoring tempo, then a 3:2 check: if the kick band repeats
/// clearly more strongly at 1.5x or 1/1.5x that tempo, the best tempo near
/// there wins instead. (a bassline track scored 93.3 although its kick
/// repeated three times more strongly at 140.)
pub fn estimate_bpm(env: &OnsetEnvelope, opts: TempoOptions) -> Option<f64> {
    let scorer = TempoScorer::new(env, opts)?;
    let best = scorer.best_between(opts.min_bpm, opts.max_bpm, 0.1)?;
    let here = scorer.kick_repeat(best);
    let mut pick = best;
    for factor in [1.5, 1.0 / 1.5] {
        let other = best * factor;
        if !(opts.min_bpm..=opts.max_bpm).contains(&other) {
            continue;
        }
        let there = scorer.kick_repeat(other);
        if there >= THREE_TWO_MIN_KICK && there > THREE_TWO_SWITCH_RATIO * here.max(0.0) {
            if let Some(b) = scorer.best_between(other * 0.98, other * 1.02, 0.01) {
                pick = b;
            }
            break;
        }
    }
    // Refine around the pick.
    let fine = scorer.best_between(pick - 0.15, pick + 0.15, 0.005)?;
    (scorer.score(fine) > 0.0).then_some(fine)
}

/// Autocorrelation of `x` up to `max_lag`, normalised so lag 0 is 1.
fn normalized_acf(x: &[f32], max_lag: usize) -> Vec<f64> {
    let mean = x.iter().map(|v| f64::from(*v)).sum::<f64>() / x.len().max(1) as f64;
    let xc: Vec<f64> = x.iter().map(|v| f64::from(*v) - mean).collect();
    let mut acf = vec![0.0f64; max_lag + 1];
    for (lag, a) in acf.iter_mut().enumerate() {
        let n = xc.len().saturating_sub(lag).max(1);
        let s: f64 = xc.iter().zip(xc.iter().skip(lag)).map(|(p, q)| p * q).sum();
        *a = s / n as f64;
    }
    let zero = acf.first().copied().unwrap_or(0.0);
    if zero > 0.0 {
        acf.iter_mut().for_each(|a| *a /= zero);
    }
    acf
}

/// Weight of the kick band's own tempo score.
///
/// In 4/4 dance music the kick marks the beat. The all-band envelope can be
/// dominated by other rhythms: in bassline / speed garage the bass stabs
/// fall every 1.5 beats and fill several bands, which pulled real 140 BPM
/// tracks to 93.3. The kick band (below 150 Hz) repeats far more
/// strongly at the real beat (measured on a 140 BPM bassline track: 0.24 at
/// one beat against 0.05 at 1.5 beats), so its score gets a vote.
const LOW_BAND_WEIGHT: f64 = 1.0;

/// Tempo scoring from the envelopes' autocorrelation.
struct TempoScorer {
    fps: f64,
    acf: Vec<f64>,
    low_acf: Vec<f64>,
}

impl TempoScorer {
    fn new(env: &OnsetEnvelope, opts: TempoOptions) -> Option<Self> {
        let x = &env.values;
        if x.len() < 64 {
            return None;
        }
        let fps = env.fps;
        let max_lag = ((4.0 * 60.0 * fps / opts.min_bpm).ceil() as usize + 2).min(x.len() - 1);
        let half_width = ((0.125 * fps) as usize).max(1);
        Some(Self {
            fps,
            acf: normalized_acf(x, max_lag),
            low_acf: normalized_acf(&detrend(&env.low, half_width), max_lag),
        })
    }

    fn score(&self, bpm: f64) -> f64 {
        let lag = 60.0 * self.fps / bpm;
        let comb = |a: &[f64]| -> f64 {
            [(1.0, 1.0), (2.0, 0.5), (3.0, 0.33), (4.0, 0.25)]
                .iter()
                .map(|(k, w)| w * interp(a, k * lag))
                .sum()
        };
        // Strong onsets halfway between beats mean the real beat is twice
        // as fast: penalise this tempo by that half-lag correlation.
        let half = |a: &[f64]| interp(a, lag / 2.0).max(0.0);
        // Mild preference for tempos near 120 BPM to break octave ties.
        let prior = (-0.5 * ((bpm / 120.0).log2() / 1.2).powi(2)).exp();
        let full = comb(&self.acf) - 0.5 * half(&self.acf);
        let low = comb(&self.low_acf) - 0.5 * half(&self.low_acf);
        (full + LOW_BAND_WEIGHT * low) * prior
    }

    /// How strongly the kick band repeats after one beat at `bpm`.
    fn kick_repeat(&self, bpm: f64) -> f64 {
        interp(&self.low_acf, 60.0 * self.fps / bpm)
    }

    /// Best-scoring tempo in `[lo, hi]` on a `step` grid.
    fn best_between(&self, lo: f64, hi: f64, step: f64) -> Option<f64> {
        let mut best: Option<(f64, f64)> = None;
        let mut bpm = lo;
        while bpm <= hi {
            let s = self.score(bpm);
            if best.is_none_or(|(_, b)| s > b) {
                best = Some((bpm, s));
            }
            bpm += step;
        }
        best.map(|(b, _)| b)
    }
}

/// Dynamic-programming beat tracking. Returns beat positions in frames.
#[allow(clippy::needless_range_loop)]
pub fn track_beats(env: &OnsetEnvelope, bpm: f64) -> Vec<usize> {
    let x = &env.values;
    let n = x.len();
    if n == 0 || bpm <= 0.0 {
        return Vec::new();
    }
    let period = 60.0 * env.fps / bpm;
    let std = {
        let m = x.iter().map(|v| f64::from(*v)).sum::<f64>() / n as f64;
        (x.iter().map(|v| (f64::from(*v) - m).powi(2)).sum::<f64>() / n as f64).sqrt() + 1e-9
    };
    let tightness = 100.0;
    let lo = (period / 2.0).round().max(1.0) as usize;
    let hi = (period * 2.0).round() as usize;
    let mut cum = vec![0.0f64; n];
    let mut back = vec![usize::MAX; n];
    for t in 0..n {
        let local = f64::from(x[t]) / std;
        let mut best = 0.0;
        let mut arg = usize::MAX;
        if t >= lo {
            let from = t.saturating_sub(hi);
            for p in from..=t - lo {
                let r = ((t - p) as f64 / period).ln();
                let s = cum[p] - tightness * r * r;
                if arg == usize::MAX || s > best {
                    best = s;
                    arg = p;
                }
            }
        }
        cum[t] = local + if arg == usize::MAX { 0.0 } else { best };
        back[t] = arg;
    }
    // Best end point within the last period.
    let tail = n.saturating_sub(period.ceil() as usize);
    let mut t = (tail..n)
        .max_by(|a, b| cum[*a].total_cmp(&cum[*b]))
        .unwrap_or(n - 1);
    let mut beats = vec![t];
    while back[t] != usize::MAX {
        t = back[t];
        beats.push(t);
    }
    beats.reverse();
    beats
}

/// Short-block log-energy envelope for refining beat times.
struct FineEnvelope {
    log_energy: Vec<f32>,
    block: usize,
    sr: f64,
}

impl FineEnvelope {
    fn new(mono: &[f32], sample_rate: u32) -> Self {
        let block = ((f64::from(sample_rate) / 1000.0).round() as usize).max(1);
        let log_energy = mono
            .chunks(block)
            .map(|c| (c.iter().map(|v| v * v).sum::<f32>() + 1e-10).ln())
            .collect();
        Self {
            log_energy,
            block,
            sr: f64::from(sample_rate),
        }
    }

    /// The strongest energy rise within `[t - before, t + after]` seconds,
    /// or `None` if there is no clear rise.
    fn onset_near(&self, t: f64, before: f64, after: f64) -> Option<f64> {
        let to_block = |s: f64| (s * self.sr / self.block as f64).floor() as i64;
        let a = to_block(t - before).max(3);
        let b = to_block(t + after).min(self.log_energy.len() as i64 - 1);
        let mut best: Option<(f32, i64)> = None;
        for k in a..=b {
            let k_us = k as usize;
            let prev = self.log_energy[k_us - 3..k_us]
                .iter()
                .copied()
                .fold(f32::MIN, f32::max);
            let rise = self.log_energy[k_us] - prev;
            if best.is_none_or(|(r, _)| rise > r) {
                best = Some((rise, k));
            }
        }
        best.filter(|(r, _)| *r > 1.0)
            .map(|(_, k)| k as f64 * self.block as f64 / self.sr)
    }
}

/// Fits the beat grid `t = a + p * n` to beat times without having to
/// number the beats first.
///
/// `period0` comes from the tempo estimate, quantised to whole envelope
/// frames (5.8 ms), so it can be off by about 1 %. Numbering every beat with
/// it (`round((t - t0) / period0)`) slipped a beat after about 55 beats and
/// the fit then confirmed the wrong tempo (three tracks at 156.6
/// instead of 158, and grids sliding off the kick during a track); counting
/// beat to beat, or fitting progressively longer spans, broke on every
/// glitch of the beat tracker.
///
/// Instead, every period within 2 % of `period0` is scored by how well all
/// beat times line up on it (the length of the mean of exp(2 pi i t / p), a
/// phase-coherence measure): missing or extra beats only add a little
/// noise. The best period is accurate to about 1e-5, so numbering the beats
/// with it is safe for a final robust least-squares fit.
fn coherent_fit(times: &[f64], period0: f64) -> Option<(f64, f64)> {
    if times.len() < 4 || period0 <= 0.0 {
        return None;
    }
    let tau = 2.0 * std::f64::consts::PI;
    let coherence = |p: f64| -> (f64, f64) {
        let w = tau / p;
        let (c, s) = times.iter().fold((0.0, 0.0), |(c, s), t| {
            (c + (w * t).cos(), s + (w * t).sin())
        });
        ((c * c + s * s).sqrt(), s.atan2(c))
    };
    // Coarse then fine search over the period.
    let mut best = (period0, f64::MIN);
    for (lo, hi, steps) in [(0.98, 1.02, 2_000usize), (0.0, 0.0, 400)] {
        let (from, to) = if steps == 2_000 {
            (period0 * lo, period0 * hi)
        } else {
            let d = period0 * 0.04 / 2_000.0;
            (best.0 - d, best.0 + d)
        };
        for i in 0..=steps {
            let p = from + (to - from) * i as f64 / steps as f64;
            let r = coherence(p).0;
            if r > best.1 {
                best = (p, r);
            }
        }
    }
    let p = best.0;
    let phase = coherence(p).1;
    let a = (phase / tau * p).rem_euclid(p);
    let points: Vec<(f64, f64)> = times.iter().map(|&t| (((t - a) / p).round(), t)).collect();
    fit_line(&points)
}

/// Robust straight-line fit `t = a + p * n`. Returns `(a, p)`.
fn fit_line(points: &[(f64, f64)]) -> Option<(f64, f64)> {
    let mut pts: Vec<(f64, f64)> = points.to_vec();
    let mut result = None;
    for _ in 0..4 {
        if pts.len() < 4 {
            break;
        }
        let n = pts.len() as f64;
        let mx = pts.iter().map(|p| p.0).sum::<f64>() / n;
        let my = pts.iter().map(|p| p.1).sum::<f64>() / n;
        let sxx: f64 = pts.iter().map(|p| (p.0 - mx).powi(2)).sum();
        let sxy: f64 = pts.iter().map(|p| (p.0 - mx) * (p.1 - my)).sum();
        if sxx <= 0.0 {
            break;
        }
        let p = sxy / sxx;
        let a = my - p * mx;
        result = Some((a, p));
        let res: Vec<f64> = pts.iter().map(|q| q.1 - (a + p * q.0)).collect();
        let sd = (res.iter().map(|r| r * r).sum::<f64>() / n).sqrt();
        let limit = (3.0 * sd).clamp(0.002, 0.03);
        let keep: Vec<(f64, f64)> = pts
            .iter()
            .zip(res.iter())
            .filter(|(_, r)| r.abs() <= limit)
            .map(|(q, _)| *q)
            .collect();
        if keep.len() == pts.len() {
            break;
        }
        pts = keep;
    }
    result
}

/// Frequency band for [`attack_profile`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Band {
    /// Below 100 Hz: the body of the kick (and sub bass).
    Sub,
    /// 100-300 Hz: bass notes.
    Bass,
    /// Everything.
    Full,
}

/// Where within one beat a band's attacks fall: the band's energy rise in
/// 1 ms steps (dB above the average of the previous 2-10 ms, positive part),
/// averaged over all beats of `grid`. Index i is i ms after a grid line.
pub fn attack_profile(mono: &[f32], sample_rate: u32, grid: &Beatgrid, band: Band) -> Vec<f64> {
    let sr = f64::from(sample_rate.max(1));
    let filtered: Vec<f32> = match band {
        Band::Full => mono.to_vec(),
        Band::Sub => {
            let mut lp = dj_dsp::biquad::Lr4::lowpass(100.0, sr);
            mono.iter().map(|x| lp.process(*x)).collect()
        }
        Band::Bass => {
            let mut hp = dj_dsp::biquad::Lr4::highpass(100.0, sr);
            let mut lp = dj_dsp::biquad::Lr4::lowpass(300.0, sr);
            mono.iter().map(|x| lp.process(hp.process(*x))).collect()
        }
    };
    let block = ((sr / 1000.0).round() as usize).max(1);
    let level: Vec<f64> = filtered
        .chunks(block)
        .map(|c| 10.0 * (c.iter().map(|v| f64::from(v * v)).sum::<f64>() + 1e-12).log10())
        .collect();
    let period = grid.period();
    let bins = ((period * 1000.0).ceil() as usize).max(1);
    let mut sum = vec![0.0f64; bins];
    let mut count = vec![0usize; bins];
    for k in 10..level.len() {
        let before = level[k - 10..k - 2].iter().sum::<f64>() / 8.0;
        let rise = (level[k] - before).max(0.0);
        let t = k as f64 / 1000.0;
        let phase = (t - grid.first_beat).rem_euclid(period);
        let b = ((phase * 1000.0) as usize).min(bins - 1);
        sum[b] += rise;
        count[b] += 1;
    }
    sum.iter()
        .zip(count.iter())
        .map(|(s, c)| s / (*c).max(1) as f64)
        .collect()
}

/// The two strongest attacks in a profile from [`attack_profile`]:
/// (milliseconds from the grid line, signed within half a beat; strength
/// relative to the profile's mean).
pub fn strongest_attacks(profile: &[f64]) -> Vec<(f64, f64)> {
    let n = profile.len();
    if n == 0 {
        return Vec::new();
    }
    let mean = profile.iter().sum::<f64>() / n as f64 + 1e-12;
    let mut peaks: Vec<(usize, f64)> = (0..n)
        .filter(|&i| {
            let w = 15;
            (1..=w).all(|d| {
                profile[i] >= profile[(i + n - d) % n] && profile[i] >= profile[(i + d) % n]
            })
        })
        .map(|i| (i, profile[i]))
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    peaks
        .into_iter()
        .take(2)
        .map(|(i, v)| {
            let ms = if i > n / 2 {
                i as f64 - n as f64
            } else {
                i as f64
            };
            (ms, v / mean)
        })
        .collect()
}

/// Human-readable trace of every stage of [`detect_beatgrid`], for
/// `kaldor --analyze` (diagnosing wrong BPMs on real music).
pub fn tempo_report(mono: &[f32], sample_rate: u32, opts: TempoOptions) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let env = onset_envelope(mono, sample_rate);
    let _ = writeln!(
        out,
        "onset envelope: {} frames at {:.2} fps",
        env.values.len(),
        env.fps
    );
    // Local maxima of the tempo score, best first.
    let scores = tempo_scores(&env, opts);
    let mut peaks: Vec<(f64, f64)> = scores
        .windows(3)
        .filter(|w| w[1].1 >= w[0].1 && w[1].1 >= w[2].1)
        .map(|w| w[1])
        .collect();
    peaks.sort_by(|a, b| b.1.total_cmp(&a.1));
    let _ = writeln!(out, "tempo candidates (score):");
    for (bpm, s) in peaks.iter().take(6) {
        let _ = writeln!(out, "  {bpm:7.2} BPM  {s:.4}");
    }
    let Some(bpm0) = estimate_bpm(&env, opts) else {
        let _ = writeln!(out, "estimate_bpm: none");
        return out;
    };
    let _ = writeln!(out, "estimate_bpm: {bpm0:.3}");
    // How strongly each envelope repeats at multiples of the chosen beat.
    // A real beat repeats at 1 and 2 beats; a dotted rhythm at 1.5 and 3.
    let lag = 60.0 * env.fps / bpm0;
    let max_lag = ((3.5 * lag).ceil() as usize).min(env.values.len().saturating_sub(1));
    let half_width = ((0.125 * env.fps) as usize).max(1);
    for (name, acf) in [
        ("all bands", normalized_acf(&env.values, max_lag)),
        (
            "kick band",
            normalized_acf(&detrend(&env.low, half_width), max_lag),
        ),
    ] {
        let _ = writeln!(
            out,
            "  {name}: repeats at 2/3 beat {:+.3}, 1 beat {:+.3}, 1.5 beats {:+.3}, 2 beats {:+.3}, 3 beats {:+.3}",
            interp(&acf, lag * 2.0 / 3.0),
            interp(&acf, lag),
            interp(&acf, lag * 1.5),
            interp(&acf, lag * 2.0),
            interp(&acf, lag * 3.0),
        );
    }
    let beats = track_beats(&env, bpm0);
    let period = 60.0 * env.fps / bpm0;
    let mut ibis: Vec<f64> = beats.windows(2).map(|w| (w[1] - w[0]) as f64).collect();
    let near = ibis
        .iter()
        .filter(|d| (*d / period - 1.0).abs() < 0.1)
        .count();
    ibis.sort_by(f64::total_cmp);
    let median = ibis.get(ibis.len() / 2).copied().unwrap_or(0.0);
    let _ = writeln!(
        out,
        "beat tracker: {} beats, median interval {:.3} BPM, {}/{} intervals within 10 % of the estimate",
        beats.len(),
        if median > 0.0 {
            60.0 * env.fps / median
        } else {
            0.0
        },
        near,
        ibis.len()
    );
    let grid = detect_beatgrid(mono, sample_rate, opts);
    if let Some((_, times, env)) = beat_times(mono, sample_rate, opts) {
        let period0 = 60.0 / bpm0;
        if let Some((_, p_fit)) = coherent_fit(&times, period0) {
            let _ = writeln!(
                out,
                "beat-tracker fit: {:.3} BPM, beat coherence {:.3} (whole {:.0} BPM: {:.3})",
                60.0 / p_fit,
                coherence(&times, p_fit),
                (60.0 / p_fit).round(),
                coherence(&times, 60.0 / (60.0 / p_fit).round())
            );
        }
        let points = kick_points(&env);
        let (pb, sb) = kick_period_search(&points, period0);
        let whole = (60.0 / pb).round();
        let sw = kick_score(&points, 60.0 / whole);
        let _ = writeln!(
            out,
            "kick: best {:.3} BPM (score {sb:.3}), whole {whole:.0} BPM (score {sw:.3}, {:.0} % of best); steady kick: {}",
            60.0 / pb,
            100.0 * sw / sb.max(1e-12),
            if sb >= KICK_SCORE_MIN { "yes" } else { "no" }
        );
    }
    match grid {
        Some(g) => {
            let _ = writeln!(
                out,
                "beatgrid: {:.3} BPM, first beat {:.4} s, first beat in bar {}",
                g.bpm, g.first_beat, g.first_beat_in_bar
            );
            let _ = writeln!(
                out,
                "bar numbers: bar 1's downbeat at {:.4} s, the first beat is in bar {}",
                g.beat_time(g.bar_zero),
                g.bar_number(0)
            );
            // Beat one of the grid as the reference for the bar positions.
            let one =
                g.first_beat + f64::from((5 - i32::from(g.first_beat_in_bar)) % 4) * g.period();
            let att = bar_position_attacks(mono, sample_rate, one, g.period());
            let (_, score) = downbeat_phase(mono, sample_rate, one, g.period());
            for (k, a) in att.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "  bar position {}: attack low {:5.1} dB, mid {:5.1} dB, high {:5.1} dB; downbeat score {:.3}",
                    k + 1,
                    a[0],
                    a[1],
                    a[2],
                    score[k]
                );
            }
            for (name, band) in [
                ("sub <100 Hz", Band::Sub),
                ("bass 100-300", Band::Bass),
                ("full", Band::Full),
            ] {
                let peaks = strongest_attacks(&attack_profile(mono, sample_rate, &g, band));
                let list: Vec<String> = peaks
                    .iter()
                    .map(|(ms, s)| format!("{ms:+.0} ms (x{s:.1})"))
                    .collect();
                let _ = writeln!(out, "  attacks vs grid, {name}: {}", list.join(", "));
            }
        }
        None => {
            let _ = writeln!(out, "beatgrid: none");
        }
    }
    out
}

/// Tempo estimate and beat times (tracked on the envelope, refined to about
/// 1 ms against the signal).
fn beat_times(
    mono: &[f32],
    sample_rate: u32,
    opts: TempoOptions,
) -> Option<(f64, Vec<f64>, OnsetEnvelope)> {
    let env = onset_envelope(mono, sample_rate);
    let bpm0 = estimate_bpm(&env, opts)?;
    let beats = track_beats(&env, bpm0);
    if beats.len() < 8 {
        return None;
    }
    let fine = FineEnvelope::new(mono, sample_rate);
    let frame_time = |f: usize| env.offset + f as f64 / env.fps;
    let times = beats
        .iter()
        .map(|&b| {
            let t = frame_time(b);
            fine.onset_near(t, 0.04, 0.04).unwrap_or(t)
        })
        .collect();
    Some((bpm0, times, env))
}

/// Phase coherence of beat times on period `p`: 1 when every beat sits on
/// the grid, about 0 when they are unrelated to it.
fn coherence(times: &[f64], p: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI / p;
    let (c, s) = times.iter().fold((0.0, 0.0), |(c, s), t| {
        (c + (w * t).cos(), s + (w * t).sin())
    });
    (c * c + s * s).sqrt() / times.len().max(1) as f64
}

/// Weighted phase coherence of `points` on period `p`, and the grid offset
/// (s, in `[0, p)`) where they line up.
fn weighted_coherence(points: &[(f64, f64)], p: f64) -> (f64, f64) {
    let tau = 2.0 * std::f64::consts::PI;
    let w = tau / p;
    let (mut c, mut s, mut total) = (0.0, 0.0, 0.0);
    for (t, r) in points {
        c += r * (w * t).cos();
        s += r * (w * t).sin();
        total += r;
    }
    let phase = s.atan2(c);
    (
        (c * c + s * s).sqrt() / total.max(1e-12),
        (phase / tau * p).rem_euclid(p),
    )
}

/// Kick-band onset envelope as weighted points `(time, strength)`.
fn kick_points(env: &OnsetEnvelope) -> Vec<(f64, f64)> {
    let low = detrend(&env.low, ((0.125 * env.fps) as usize).max(1));
    low.iter()
        .enumerate()
        .filter(|(_, x)| **x > 0.0)
        .map(|(i, x)| (env.offset + i as f64 / env.fps, f64::from(*x)))
        .collect()
}

/// How well the kick band repeats on period `p`: weighted phase coherence at
/// the beat plus at the half beat. The half beat counts because a loud
/// off-beat bass in the same band cancels the beat-rate term (hard bounce,
/// bassline), and at 70 BPM-style half tempo the kick falls twice per beat.
fn kick_score(points: &[(f64, f64)], p: f64) -> f64 {
    weighted_coherence(points, p).0 + weighted_coherence(points, p / 2.0).0
}

/// The period within 1.5 % of `period0` where the kick repeats best, and its
/// [`kick_score`] (coarse then fine search). `(period0, -inf)` without kick
/// data.
fn kick_period_search(points: &[(f64, f64)], period0: f64) -> (f64, f64) {
    let mut best = (period0, f64::MIN);
    if points.len() < 64 {
        return best;
    }
    let span = 0.015;
    for i in 0..=600 {
        let q = period0 * (1.0 - span + 2.0 * span * f64::from(i) / 600.0);
        let s = kick_score(points, q);
        if s > best.1 {
            best = (q, s);
        }
    }
    let step = period0 * 2.0 * span / 600.0;
    let centre = best.0;
    for i in -50..=50 {
        let q = centre + step * f64::from(i) / 50.0;
        let s = kick_score(points, q);
        if s > best.1 {
            best = (q, s);
        }
    }
    best
}

/// Dance music is produced at whole BPMs, and a grid at 70.05 instead of 70
/// drifts 0.15 s over four minutes. So a whole BPM replaces the measured
/// one when it scores at least this share of it...
const WHOLE_BPM_SHARE: f64 = 0.9;

/// ...and, with a steady kick (whose tempo is measured to about 0.01 BPM),
/// lies at most this far from it. On 75 reference tracks the kick
/// tempo was within 0.03 of a whole BPM on all but three; a real 179.9 BPM
/// track stays at 179.9.
const WHOLE_BPM_MAX_DISTANCE: f64 = 0.05;

/// The kick score must reach this at the chosen period for [`refine_grid`] to
/// replace the beat-tracker fit (tracks without a steady kick keep it).
const KICK_SCORE_MIN: f64 = 0.15;

/// 1 ms energy rises (dB above the mean of the previous 2-10 ms, positive
/// part) below 150 Hz and of the full signal.
struct Rises {
    low: Vec<f64>,
    full: Vec<f64>,
    /// Block length in seconds.
    dt: f64,
}

impl Rises {
    fn new(mono: &[f32], sample_rate: u32) -> Self {
        let sr = f64::from(sample_rate.max(1));
        let mut lp = dj_dsp::biquad::Lr4::lowpass(150.0, sr);
        let block = ((sr / 1000.0).round() as usize).max(1);
        let db = |e: f64| 10.0 * (e / block as f64 + 1e-12).log10();
        let mut low_level = Vec::with_capacity(mono.len() / block + 1);
        let mut full_level = Vec::with_capacity(mono.len() / block + 1);
        for c in mono.chunks(block) {
            let (mut el, mut ef) = (0.0, 0.0);
            for x in c {
                let y = f64::from(lp.process(*x));
                el += y * y;
                ef += f64::from(*x) * f64::from(*x);
            }
            low_level.push(db(el));
            full_level.push(db(ef));
        }
        let rise = |level: &[f64]| -> Vec<f64> {
            (0..level.len())
                .map(|k| {
                    if k < 10 {
                        return 0.0;
                    }
                    let before = level[k - 10..k - 2].iter().sum::<f64>() / 8.0;
                    (level[k] - before).max(0.0)
                })
                .collect()
        };
        Self {
            low: rise(&low_level),
            full: rise(&full_level),
            dt: block as f64 / sr,
        }
    }

    /// Index of the largest value of `v` in `[a, b]` (clamped), with it.
    fn peak(v: &[f64], a: i64, b: i64) -> Option<(f64, usize)> {
        let a = a.max(0);
        let b = b.min(v.len() as i64 - 1);
        (a..=b)
            .map(|k| (v[k as usize], k as usize))
            .max_by(|x, y| x.0.total_cmp(&y.0))
    }

    /// For each grid line `o + n p`: the strongest kick (below 150 Hz) rise
    /// within ±`win` s. Returns the sum of those peaks and, for the clear
    /// kicks (>= 6 dB), where the attack starts relative to the line.
    ///
    /// The kick is found in the low band but timed in the full signal: the
    /// low band rises 5-10 ms after the click of the kick (slower build-up
    /// and filter delay), and the click is what the waveform shows. The
    /// start is where the full-band rise first reaches half its peak.
    fn kick_lines(&self, o: f64, p: f64, win: f64) -> (f64, Vec<f64>) {
        let dt = self.dt;
        let len = self.low.len() as f64 * dt;
        let half = (win / dt).round() as i64;
        let ms = |s: f64| (s / dt).round() as i64;
        let mut sum = 0.0;
        let mut offsets = Vec::new();
        let mut n = 0.0;
        loop {
            let t = o + n * p;
            if t > len {
                break;
            }
            n += 1.0;
            let c = (t / dt).round() as i64;
            let Some((r, k)) = Self::peak(&self.low, c - half, c + half) else {
                continue;
            };
            sum += r;
            if r < 6.0 {
                continue;
            }
            let k = k as i64;
            let Some((fr, fk)) = Self::peak(&self.full, k - ms(0.02), k + ms(0.005)) else {
                continue;
            };
            let mut s = fk;
            while s > 0 && self.full[s - 1] >= 0.5 * fr {
                s -= 1;
            }
            offsets.push(s as f64 * dt - t);
        }
        (sum, offsets)
    }
}

/// A whole BPM instead of period `p` when `score` rates it at least
/// [`WHOLE_BPM_SHARE`] of `p`'s score and it is at most `max_distance` BPM
/// away.
fn snap_whole_bpm(p: f64, max_distance: f64, score: impl Fn(f64) -> f64) -> f64 {
    let bpm = 60.0 / p;
    let whole = 60.0 / bpm.round();
    if (bpm - bpm.round()).abs() <= max_distance && score(whole) >= WHOLE_BPM_SHARE * score(p) {
        whole
    } else {
        p
    }
}

/// Final grid `(first line, period)` from the beat-tracker fit
/// `(a_fit, p_fit)`, the tracked beat times and the kick.
///
/// The beat-tracker fit ([`coherent_fit`]) is only as good as the tracked
/// beat times, and on bassline / speed garage tracks they scatter (fits
/// of 140.74, 139.74 and 70.07 on reference tracks; grids slightly off
/// the kick). So:
/// - Period: where the kick band repeats best ([`kick_score`]) within 1.5 %
///   of the tempo estimate; if the kick is not steady enough
///   ([`KICK_SCORE_MIN`]), the beat-tracker fit. Either is snapped to a
///   whole BPM when that fits nearly as well ([`snap_whole_bpm`]).
/// - Lines: on the kick attacks. The candidate offset whose lines catch the
///   strongest kick rises (with a steady kick: the beat or the off-beat of
///   the kick band's half-beat phase), then moved by the median distance to
///   the starts of the clear kick attacks.
fn refine_grid(
    mono: &[f32],
    sample_rate: u32,
    env: &OnsetEnvelope,
    times: &[f64],
    period0: f64,
    (a_fit, p_fit): (f64, f64),
) -> (f64, f64) {
    let points = kick_points(env);
    let best = kick_period_search(&points, period0);
    let steady_kick = best.1 >= KICK_SCORE_MIN;
    let (p, candidates) = if steady_kick {
        let p = snap_whole_bpm(best.0, WHOLE_BPM_MAX_DISTANCE, |q| kick_score(&points, q));
        let (_, half_phase) = weighted_coherence(&points, p / 2.0);
        (p, vec![half_phase, half_phase + p / 2.0])
    } else {
        // The beat-tracker tempo is less precise: no distance limit.
        let p = snap_whole_bpm(p_fit, 0.5, |q| coherence(times, q));
        let ones: Vec<(f64, f64)> = times.iter().map(|t| (*t, 1.0)).collect();
        let (_, phase) = weighted_coherence(&ones, p);
        // Keep the fitted offset when the period did not change.
        let o = if p == p_fit { a_fit } else { phase };
        (p, vec![o])
    };
    // The envelope phases are only good to about 30 ms (frame-based, and
    // the kick band builds up late), so search 1 ms steps around each
    // candidate for the offset whose lines catch the strongest kick rises.
    let rises = Rises::new(mono, sample_rate);
    let reach = (p / 8.0).min(0.06);
    let steps = (reach * 1000.0).round() as i32;
    let mut found: Option<(f64, f64)> = None;
    for c in candidates {
        for i in -steps..=steps {
            let o = c + f64::from(i) * 0.001;
            let s = rises.kick_lines(o, p, 0.003).0;
            if found.is_none_or(|(_, b)| s > b) {
                found = Some((o, s));
            }
        }
    }
    let Some((mut o, _)) = found else {
        return (a_fit, p_fit);
    };
    for _ in 0..3 {
        let (_, mut offs) = rises.kick_lines(o, p, 0.015);
        if offs.len() < 8 {
            break;
        }
        offs.sort_by(f64::total_cmp);
        let shift = offs[offs.len() / 2];
        o += shift;
        if shift.abs() < 0.0005 {
            break;
        }
    }
    (o, p)
}

/// Full beatgrid detection on a mono signal.
pub fn detect_beatgrid(mono: &[f32], sample_rate: u32, opts: TempoOptions) -> Option<Beatgrid> {
    let (bpm0, times, env) = beat_times(mono, sample_rate, opts)?;
    let period0 = 60.0 / bpm0;
    let (a_fit, p_fit) = coherent_fit(&times, period0)?;
    if p_fit.is_nan() || p_fit <= 0.0 {
        return None;
    }
    let (a, p) = refine_grid(mono, sample_rate, &env, &times, period0, (a_fit, p_fit));
    let bpm = 60.0 / p;
    if !(opts.min_bpm * 0.9..=opts.max_bpm * 1.1).contains(&bpm) {
        return None;
    }

    let (down_phase, _) = downbeat_phase(mono, sample_rate, a, p);

    // Grid anchored on a downbeat, then normalised to the first beat >= 0;
    // bars numbered from the first downbeat of the track (bar 1).
    let mut grid = Beatgrid::new(bpm, 0.0);
    grid.set_beat_one(a + down_phase as f64 * p);
    grid.number_bars_from_start();
    Some(grid)
}

/// Mean attack per bar position (beat `a + n p` is position `n mod 4`): the
/// largest 1 ms energy rise (dB above the previous 2-10 ms) in the first
/// 40 ms of the beat, below 150 Hz (kick), 150 Hz-2 kHz (snare body) and
/// above 2 kHz (snare / clap / hats).
fn bar_position_attacks(mono: &[f32], sample_rate: u32, a: f64, p: f64) -> [[f64; 3]; 4] {
    let sr = f64::from(sample_rate.max(1));
    let block = ((sr / 1000.0).round() as usize).max(1);
    let mut split = dj_dsp::biquad::ThreeBandSplit::new(150.0, 2_000.0, sr);
    let mut level = vec![[0.0f64; 3]; mono.len() / block + 1];
    for (i, x) in mono.iter().enumerate() {
        let (l, m, h) = split.process(*x);
        let e = &mut level[i / block];
        e[0] += f64::from(l * l);
        e[1] += f64::from(m * m);
        e[2] += f64::from(h * h);
    }
    for e in &mut level {
        for v in e.iter_mut() {
            *v = 10.0 * (*v / block as f64 + 1e-12).log10();
        }
    }
    let dt = block as f64 / sr;
    let mut sum = [[0.0f64; 3]; 4];
    let mut count = [0usize; 4];
    let n_min = (-a / p).ceil() as i64;
    let mut n = n_min;
    loop {
        let t = a + n as f64 * p;
        let k0 = (t / dt).round() as usize;
        if k0 + 40 >= level.len() {
            break;
        }
        let pos = n.rem_euclid(4) as usize;
        for b in 0..3 {
            let mut best = 0.0f64;
            for k in k0.max(10)..k0 + 40 {
                let before = level[k - 10..k - 2].iter().map(|e| e[b]).sum::<f64>() / 8.0;
                best = best.max(level[k][b] - before);
            }
            sum[pos][b] += best;
        }
        count[pos] += 1;
        n += 1;
    }
    let mut out = [[0.0f64; 3]; 4];
    for (o, (s, c)) in out.iter_mut().zip(sum.iter().zip(count.iter())) {
        for b in 0..3 {
            o[b] = s[b] / (*c).max(1) as f64;
        }
    }
    out
}

/// Smallest bar-to-bar change that counts as a section change: summed over
/// the three bands, in natural-log energy (0.5 is about 1.6x in one band).
/// Steady passages stay below it, so they leave the decision to the accent.
const DOWNBEAT_MIN_CHANGE: f64 = 0.5;

/// Weight of the section-change score against the bass-accent score.
const DOWNBEAT_CHANGE_WEIGHT: f64 = 2.0;

/// Which beat phase (mod 4) of the grid `t = a + n * p` is beat one, with
/// the combined score per phase.
///
/// Two cues, each normalised to shares of 1 over the four phases:
/// - **Section changes** (weight 2): in electronic music sections, drops,
///   builds and new elements start on beat one. For every beat, the sound of
///   the bar after it (mean log energy below 150 Hz, 150 Hz-2 kHz and above
///   2 kHz per beat) is compared with the bar before it; big changes count
///   most (squared). Hard dance and bounce tracks put an equally heavy kick
///   and bass on every beat, so the accent cue alone was close to a coin toss
///   there (bar markers on the second kick).
/// - **Bass accent**: bass energy in the 80 ms after the beat (the only cue
///   before; it still decides tracks without clear section changes).
pub fn downbeat_phase(mono: &[f32], sample_rate: u32, a: f64, p: f64) -> (i64, [f64; 4]) {
    let sr = f64::from(sample_rate.max(1));
    if p <= 0.0 {
        return (0, [0.0; 4]);
    }
    // Beat n covers [a + n p, a + (n + 1) p); index from the first beat >= 0.
    let n_min = (-a / p).ceil() as i64;
    let beats = ((mono.len() as f64 / sr - a) / p).floor() as i64 - n_min;
    if beats < 1 {
        return (0, [0.0; 4]);
    }
    let beats = beats as usize;
    let mut split = dj_dsp::biquad::ThreeBandSplit::new(150.0, 2_000.0, sr);
    let mut lp = dj_dsp::biquad::Lr4::lowpass(150.0, sr);
    let mut bands = vec![[0.0f64; 3]; beats];
    let mut counts = vec![0usize; beats];
    let mut accent = vec![0.0f64; beats];
    let accent_span = 0.08;
    for (i, x) in mono.iter().enumerate() {
        let (l, m, h) = split.process(*x);
        let bass = lp.process(*x);
        let t = i as f64 / sr - a;
        let n = (t / p).floor() as i64 - n_min;
        let Ok(n) = usize::try_from(n) else { continue };
        if n >= beats {
            break;
        }
        let e = &mut bands[n];
        e[0] += f64::from(l * l);
        e[1] += f64::from(m * m);
        e[2] += f64::from(h * h);
        counts[n] += 1;
        if t - (n as i64 + n_min) as f64 * p < accent_span {
            accent[n] += f64::from(bass * bass);
        }
    }
    let feature: Vec<[f64; 3]> = bands
        .iter()
        .zip(counts.iter())
        .map(|(e, c)| {
            let c = (*c).max(1) as f64;
            [
                (e[0] / c + 1e-12).ln(),
                (e[1] / c + 1e-12).ln(),
                (e[2] / c + 1e-12).ln(),
            ]
        })
        .collect();
    let mut change = [0.0f64; 4];
    let mut accented = [0.0f64; 4];
    for (n, acc) in accent.iter().enumerate() {
        let phase = (n as i64 + n_min).rem_euclid(4) as usize;
        accented[phase] += acc.sqrt();
        if n < 4 || n + 4 > beats {
            continue;
        }
        let mean = |r: std::ops::Range<usize>| -> [f64; 3] {
            let mut s = [0.0; 3];
            for f in &feature[r] {
                for (a, b) in s.iter_mut().zip(f.iter()) {
                    *a += b / 4.0;
                }
            }
            s
        };
        let (before, after) = (mean(n - 4..n), mean(n..n + 4));
        let d: f64 = before
            .iter()
            .zip(after.iter())
            .map(|(x, y)| (x - y).abs())
            .sum();
        if d >= DOWNBEAT_MIN_CHANGE {
            change[phase] += d * d;
        }
    }
    let share = |v: [f64; 4]| -> [f64; 4] {
        let s: f64 = v.iter().sum();
        if s > 0.0 { v.map(|x| x / s) } else { [0.25; 4] }
    };
    let (c, b) = (share(change), share(accented));
    let mut score = [0.0f64; 4];
    for k in 0..4 {
        score[k] = DOWNBEAT_CHANGE_WEIGHT * c[k] + b[k];
    }
    let allowed = downbeat_candidates(&bar_position_attacks(mono, sample_rate, a, p));
    let best = (0..4)
        .filter(|k| allowed[*k])
        .max_by(|x, y| score[*x].total_cmp(&score[*y]))
        .unwrap_or(0);
    (best as i64, score)
}

/// A bar position whose kick attack is this much weaker than the strongest
/// position's cannot be beat one.
const DOWNBEAT_KICK_MARGIN_DB: f64 = 6.0;

/// When the bright (above 2 kHz) attacks of positions 2+4 and 1+3 differ by
/// this much, the brighter pair is the backbeat (snare / clap) and beat one
/// is in the other pair.
const BACKBEAT_MIN_DB: f64 = 3.0;

/// Bar positions that can be beat one, from [`bar_position_attacks`]:
/// - beat one has a kick (Dr Cryptic's marker was on a nearly empty
///   beat right before the snare);
/// - beat one is not on the backbeat (Fck That Sht's marker was on
///   the snare).
///
/// If the rules leave nothing, every position is allowed.
fn downbeat_candidates(att: &[[f64; 3]; 4]) -> [bool; 4] {
    let max_low = att.iter().map(|a| a[0]).fold(f64::MIN, f64::max);
    let mut allowed = att.map(|a| a[0] >= max_low - DOWNBEAT_KICK_MARGIN_DB);
    let bright = |i: usize, j: usize| (att[i][2] + att[j][2]) / 2.0;
    let (odd, even) = (bright(0, 2), bright(1, 3));
    if (odd - even).abs() >= BACKBEAT_MIN_DB {
        // Positions 1 and 3 are indices 0 and 2.
        let backbeat_is_odd = odd > even;
        for (k, ok) in allowed.iter_mut().enumerate() {
            if (k % 2 == 0) == backbeat_is_odd {
                *ok = false;
            }
        }
    }
    if allowed.iter().any(|a| *a) {
        allowed
    } else {
        [true; 4]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_fit_rejects_outliers() {
        let mut pts: Vec<(f64, f64)> = (0..100).map(|i| (i as f64, 0.3 + 0.5 * i as f64)).collect();
        pts[10].1 += 0.2;
        pts[50].1 -= 0.1;
        let (a, p) = fit_line(&pts).unwrap();
        assert!((a - 0.3).abs() < 1e-9 && (p - 0.5).abs() < 1e-9);
    }
}

#[cfg(test)]
mod whole_bpm_tests {
    use super::*;

    #[test]
    fn whole_bpm_needs_a_close_tempo_and_a_close_score() {
        let p = |bpm: f64| 60.0 / bpm;
        let flat = |_: f64| 1.0;
        // Close and fitting as well: rounded.
        assert_eq!(60.0 / snap_whole_bpm(p(140.03), 0.05, flat), 140.0);
        // Too far for a precisely measured tempo.
        assert!((60.0 / snap_whole_bpm(p(179.9), 0.05, flat) - 179.9).abs() < 1e-9);
        // Close, but the whole BPM fits clearly worse.
        let peaked = |q: f64| {
            if (q - p(154.91)).abs() < 1e-9 {
                1.0
            } else {
                0.4
            }
        };
        assert!((60.0 / snap_whole_bpm(p(154.91), 0.5, peaked) - 154.91).abs() < 1e-9);
    }
}

#[cfg(test)]
mod downbeat_rule_tests {
    use super::*;

    /// Attack tables (low, mid, high dB per bar position) measured on
    /// reference tracks.
    #[test]
    fn beat_one_has_a_kick_and_is_not_the_backbeat() {
        // Fck That Sht: bright snare/clap on positions 1 and 3.
        let fck = [
            [18.1, 23.3, 21.4],
            [20.4, 22.3, 15.7],
            [16.6, 19.0, 24.6],
            [21.9, 24.1, 11.8],
        ];
        assert_eq!(downbeat_candidates(&fck), [false, true, false, true]);
        // Dr Cryptic: position 1 is nearly empty, snare on 2 (and 4).
        let cryptic = [
            [6.7, 4.1, 3.0],
            [21.5, 19.0, 9.9],
            [24.7, 12.0, 4.6],
            [12.9, 12.6, 5.3],
        ];
        assert_eq!(downbeat_candidates(&cryptic), [false, false, true, false]);
        // Pluggerz: kick on every beat, no backbeat: anything goes.
        let flat = [
            [12.7, 7.7, 2.3],
            [12.5, 8.0, 2.2],
            [11.9, 7.6, 2.4],
            [12.4, 7.8, 1.9],
        ];
        assert_eq!(downbeat_candidates(&flat), [true; 4]);
    }
}
