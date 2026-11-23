//! Waveform data: per point, the peak level and the low, mid and high band
//! energy, at a base resolution of about 200 points per second, plus
//! mipmap levels that halve the resolution each step. A loaded deck also
//! gets a detail waveform at about 1200 points per second for its zoomed
//! scrolling view ([`Waveform::build_detail`]).

use dj_decode::TrackAudio;
use dj_dsp::biquad::ThreeBandSplit;

/// Target base resolution (at least 150 points/s are needed).
pub const TARGET_POINTS_PER_SECOND: f64 = 200.0;

/// Resolution of a loaded deck's detail waveform (scrolling view). At the
/// closest zoom (2 beats, 1 s across) a point is about one screen pixel on
/// a full-HD screen and up to 3 on a 4K one, blended in between (see
/// [`Waveform::at_seconds`]).
pub const DETAIL_POINTS_PER_SECOND: f64 = 1200.0;

/// Time constant of the band energy envelopes of the detail waveform.
pub const DETAIL_BAND_SECONDS: f64 = 0.004;

/// The detail waveform's outline joins the tops of the wave: points that
/// are the loudest within this time on either side (half a period of a
/// bass note of about 40 Hz' rectified wave).
pub const DETAIL_PEAK_SECONDS: f64 = 0.006;

/// One waveform point. Values are 0 to 255.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct WavePoint {
    /// Peak absolute sample (max of both channels), linear.
    pub peak: u8,
    /// RMS energy of the band below about 250 Hz.
    pub low: u8,
    /// RMS energy between about 250 Hz and 2.5 kHz.
    pub mid: u8,
    /// RMS energy above about 2.5 kHz.
    pub high: u8,
}

impl WavePoint {
    fn merge(a: WavePoint, b: WavePoint) -> WavePoint {
        WavePoint {
            peak: a.peak.max(b.peak),
            low: a.low.max(b.low),
            mid: a.mid.max(b.mid),
            high: a.high.max(b.high),
        }
    }
}

/// A track's waveform with mipmaps. Immutable once built; shared as `Arc`.
#[derive(Clone, Debug, PartialEq)]
pub struct Waveform {
    /// Points per second at level 0.
    pub points_per_second: f64,
    /// Frames of audio per level-0 point.
    pub frames_per_point: usize,
    /// Sample rate of the audio the waveform was built from.
    pub sample_rate: u32,
    /// `levels[0]` is full resolution; each next level has half as many
    /// points (each the max of two).
    pub levels: Vec<Vec<WavePoint>>,
}

fn to_u8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

impl Waveform {
    /// Builds the waveform from a decoded track. Reads only the frames that
    /// are ready, so call it after decoding completes.
    pub fn build(audio: &TrackAudio) -> Waveform {
        let sr = audio.sample_rate().max(1);
        let frames_per_point = ((f64::from(sr) / TARGET_POINTS_PER_SECOND).round() as usize).max(1);
        let total = audio.ready_frames() as usize;
        let n_points = total.div_ceil(frames_per_point);
        let mut base = Vec::with_capacity(n_points);
        let mut split = ThreeBandSplit::new(
            ThreeBandSplit::DEFAULT_LOW_HZ,
            ThreeBandSplit::DEFAULT_HIGH_HZ,
            f64::from(sr),
        );
        let mut buf = vec![0.0f32; frames_per_point * 2];
        for p in 0..n_points {
            let start = (p * frames_per_point) as u64;
            audio.read_interleaved(start, &mut buf);
            let mut peak = 0.0f32;
            let (mut lo, mut mi, mut hi) = (0.0f64, 0.0f64, 0.0f64);
            for f in buf.as_chunks::<2>().0 {
                peak = peak.max(f[0].abs()).max(f[1].abs());
                let (l, m, h) = split.process(0.5 * (f[0] + f[1]));
                lo += f64::from(l * l);
                mi += f64::from(m * m);
                hi += f64::from(h * h);
            }
            let n = frames_per_point as f64;
            // sqrt(2) scales a full-scale sine's RMS to 1.0.
            let scale = |e: f64| ((e / n).sqrt() * std::f64::consts::SQRT_2) as f32;
            base.push(WavePoint {
                peak: to_u8(peak),
                low: to_u8(scale(lo)),
                mid: to_u8(scale(mi)),
                high: to_u8(scale(hi)),
            });
        }
        Waveform {
            points_per_second: f64::from(sr) / frames_per_point as f64,
            frames_per_point,
            sample_rate: sr,
            levels: with_mips(base),
        }
    }

    /// Builds the detail waveform for a deck's scrolling view: about
    /// [`DETAIL_POINTS_PER_SECOND`] points per second, so that even at the
    /// closest zoom one point covers only a few screen pixels (the 200/s
    /// library waveform would be stretched into blocks there). Built from
    /// the decoded audio when a track is loaded and kept in memory only.
    ///
    /// The peak is an outline over the tops of the wave (see
    /// [`DETAIL_PEAK_SECONDS`]) and the band energies are envelopes over
    /// [`DETAIL_BAND_SECONDS`]: a point is far shorter than one period of a
    /// bass note, so per-point values would follow the wave itself.
    pub fn build_detail(audio: &TrackAudio) -> Waveform {
        let sr = audio.sample_rate().max(1);
        let frames_per_point = ((f64::from(sr) / DETAIL_POINTS_PER_SECOND).round() as usize).max(1);
        let total = audio.ready_frames() as usize;
        let n_points = total.div_ceil(frames_per_point);
        let mut base = Vec::with_capacity(n_points);
        let mut split = ThreeBandSplit::new(
            ThreeBandSplit::DEFAULT_LOW_HZ,
            ThreeBandSplit::DEFAULT_HIGH_HZ,
            f64::from(sr),
        );
        // Two smoothing stages per band: the ripple of a bass note's energy
        // (twice its frequency) drops about ten times, with little delay.
        let k = 1.0 - (-1.0 / (DETAIL_BAND_SECONDS * f64::from(sr))).exp();
        let mut env = [[0.0f64; 2]; 3];
        let mut smooth = |band: usize, x: f32| {
            let e = &mut env[band];
            e[0] += k * (f64::from(x * x) - e[0]);
            e[1] += k * (e[0] - e[1]);
            e[1]
        };
        let (mut el, mut em, mut eh) = (0.0f64, 0.0f64, 0.0f64);
        // Read in blocks of whole points.
        let block_points = 256;
        let mut buf = vec![0.0f32; frames_per_point * block_points * 2];
        let mut p = 0;
        while p < n_points {
            let n = block_points.min(n_points - p);
            let frames = n * frames_per_point;
            audio.read_interleaved((p * frames_per_point) as u64, &mut buf[..frames * 2]);
            for point in buf[..frames * 2].chunks(frames_per_point * 2) {
                let mut peak = 0.0f32;
                for f in point.as_chunks::<2>().0 {
                    peak = peak.max(f[0].abs()).max(f[1].abs());
                    let (l, m, h) = split.process(0.5 * (f[0] + f[1]));
                    el = smooth(0, l);
                    em = smooth(1, m);
                    eh = smooth(2, h);
                }
                // sqrt(2) scales a full-scale sine's RMS to 1.0.
                let scale = |e: f64| (e.max(0.0).sqrt() * std::f64::consts::SQRT_2) as f32;
                base.push(WavePoint {
                    peak: to_u8(peak),
                    low: to_u8(scale(el)),
                    mid: to_u8(scale(em)),
                    high: to_u8(scale(eh)),
                });
            }
            p += n;
        }
        // The outline over the tops of the wave's cycles (a single 0.8 ms
        // point would follow every cycle of a bass note down to its zero
        // crossings).
        let reach = (DETAIL_PEAK_SECONDS * f64::from(sr) / frames_per_point as f64).ceil() as usize;
        let raw: Vec<u8> = base.iter().map(|p| p.peak).collect();
        for (p, v) in base.iter_mut().zip(peak_outline(&raw, reach.max(1))) {
            p.peak = v;
        }
        Waveform {
            points_per_second: f64::from(sr) / frames_per_point as f64,
            frames_per_point,
            sample_rate: sr,
            levels: with_mips(base),
        }
    }

    /// The waveform at `t` seconds, blended between the two nearest
    /// full-resolution points (each point stands for the middle of its
    /// span). `None` outside the track. For views zoomed in so far that a
    /// point is wider than a screen pixel: no steps between the points.
    pub fn at_seconds(&self, t: f64) -> Option<WavePoint> {
        let pts = self.levels.first()?;
        if pts.is_empty() || t < 0.0 || t > self.duration_seconds() {
            return None;
        }
        let x = t * f64::from(self.sample_rate) / self.frames_per_point as f64 - 0.5;
        let last = pts.len() - 1;
        let i = (x.floor().max(0.0) as usize).min(last);
        let j = (i + 1).min(last);
        let f = (x - i as f64).clamp(0.0, 1.0) as f32;
        let (a, b) = (pts[i], pts[j]);
        let mix = |p: u8, q: u8| (f32::from(p) + (f32::from(q) - f32::from(p)) * f).round() as u8;
        Some(WavePoint {
            peak: mix(a.peak, b.peak),
            low: mix(a.low, b.low),
            mid: mix(a.mid, b.mid),
            high: mix(a.high, b.high),
        })
    }

    /// Seconds of audio one full-resolution point covers.
    pub fn point_seconds(&self) -> f64 {
        self.frames_per_point as f64 / f64::from(self.sample_rate.max(1))
    }

    /// Number of points at full resolution.
    pub fn len(&self) -> usize {
        self.levels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Picks the coarsest mip level that still has at least one point per
    /// `frames_per_pixel` frames. Returns the level's points and how many
    /// audio frames one point covers.
    pub fn level_for(&self, frames_per_pixel: f64) -> (&[WavePoint], f64) {
        let mut level = 0;
        let mut fpp = self.frames_per_point as f64;
        while level + 1 < self.levels.len() && fpp * 2.0 <= frames_per_pixel {
            level += 1;
            fpp *= 2.0;
        }
        (self.levels.get(level).map_or(&[][..], Vec::as_slice), fpp)
    }

    /// Max-merged point covering frames `[start, end)`, or `None` outside the
    /// track. Uses the mip level that fits the span.
    pub fn span(&self, start: f64, end: f64) -> Option<WavePoint> {
        if end <= 0.0 || end <= start {
            return None;
        }
        let (pts, fpp) = self.level_for((end - start).max(1.0));
        let a = (start.max(0.0) / fpp).floor() as usize;
        let b = ((end / fpp).ceil() as usize).max(a + 1);
        let slice = pts.get(a..b.min(pts.len()))?;
        slice.iter().copied().reduce(WavePoint::merge)
    }

    /// Serialises to bytes (4 bytes per level-0 point) for the library
    /// cache. Mip levels are rebuilt on load.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12 + self.len() * 4);
        out.extend_from_slice(&(self.frames_per_point as u32).to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&(self.points_per_second as f32).to_le_bytes());
        for p in self.levels.first().into_iter().flatten() {
            out.extend_from_slice(&[p.peak, p.low, p.mid, p.high]);
        }
        out
    }

    /// Inverse of [`Self::to_bytes`].
    pub fn from_bytes(bytes: &[u8]) -> Option<Waveform> {
        let fpp = u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) as usize;
        let sr = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?);
        let pps = f32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?);
        let base: Vec<WavePoint> = bytes
            .get(12..)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| WavePoint {
                peak: c[0],
                low: c[1],
                mid: c[2],
                high: c[3],
            })
            .collect();
        Some(Waveform {
            points_per_second: f64::from(pps),
            frames_per_point: fpp.max(1),
            sample_rate: sr.max(1),
            levels: with_mips(base),
        })
    }
}

/// The outline of per-point peaks: straight lines down from one top to the
/// next, where a top is a point at least as loud as every point within
/// `reach` on either side (the top of a cycle of the wave, or of a hit).
/// It goes through every top, so a hit keeps its height, and a decaying
/// note is drawn as a smooth slope instead of cycle-wide steps. Upwards
/// there are no lines (a hit starts at once). A point louder than the line
/// keeps its own height.
fn peak_outline(raw: &[u8], reach: usize) -> Vec<u8> {
    let n = raw.len();
    let is_top = |i: usize| {
        let a = i.saturating_sub(reach);
        let b = (i + reach + 1).min(n);
        raw[a..b].iter().all(|v| *v <= raw[i])
    };
    let mut out = raw.to_vec();
    let mut prev: Option<usize> = None;
    for i in 0..n {
        if !is_top(i) {
            continue;
        }
        // Only downward: a line up to a louder top would start a hit early,
        // so it jumps up where the sound does.
        if let Some(t) = prev
            && i > t + 1
            && raw[i] <= raw[t]
        {
            let (a, b) = (f32::from(raw[t]), f32::from(raw[i]));
            for (k, o) in out.iter_mut().enumerate().take(i).skip(t + 1) {
                let f = (k - t) as f32 / (i - t) as f32;
                *o = (*o).max((a + (b - a) * f).round() as u8);
            }
        }
        prev = Some(i);
    }
    out
}

/// `base` and its mip levels: each next level has half as many points,
/// each the max of two.
fn with_mips(base: Vec<WavePoint>) -> Vec<Vec<WavePoint>> {
    let mut levels = vec![base];
    while let Some(last) = levels.last() {
        if last.len() <= 1 {
            break;
        }
        let next = last
            .chunks(2)
            .map(|c| match c {
                [a, b] => WavePoint::merge(*a, *b),
                [a] => *a,
                _ => WavePoint::default(),
            })
            .collect();
        levels.push(next);
    }
    levels
}

impl Waveform {
    /// Like [`Self::span`] but in seconds, independent of sample rate.
    pub fn span_seconds(&self, start: f64, end: f64) -> Option<WavePoint> {
        let sr = f64::from(self.sample_rate);
        self.span(start * sr, end * sr)
    }

    /// Duration covered, in seconds.
    pub fn duration_seconds(&self) -> f64 {
        self.len() as f64 * self.frames_per_point as f64 / f64::from(self.sample_rate.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn tone(freq: f64, seconds: f64, amp: f64) -> std::sync::Arc<TrackAudio> {
        let sr = 48_000.0;
        let n = (seconds * sr) as usize;
        let mut s = Vec::with_capacity(n * 2);
        for i in 0..n {
            let v = (amp * (2.0 * PI * freq * i as f64 / sr).sin()) as f32;
            s.push(v);
            s.push(v);
        }
        TrackAudio::from_interleaved(48_000, &s)
    }

    #[test]
    fn resolution_and_mips() {
        let w = Waveform::build(&tone(100.0, 10.0, 0.5));
        assert!(w.points_per_second >= 150.0);
        assert_eq!(w.len(), 2000);
        assert_eq!(w.levels[1].len(), 1000);
        assert_eq!(w.levels.last().unwrap().len(), 1);
    }

    #[test]
    fn bands_follow_frequency() {
        let low = Waveform::build(&tone(60.0, 2.0, 0.8));
        let high = Waveform::build(&tone(8000.0, 2.0, 0.8));
        let p = low.levels[0][200];
        // A 5 ms point holds less than one 60 Hz period, so RMS varies per point.
        assert!(p.low > 100 && p.high < 10, "{p:?}");
        assert!((i32::from(p.peak) - 204).abs() <= 2, "peak {}", p.peak);
        let p = high.levels[0][200];
        assert!(p.high > 150 && p.low < 10, "{p:?}");
    }

    #[test]
    fn span_and_level_selection() {
        let w = Waveform::build(&tone(440.0, 4.0, 0.5));
        let (_, fpp) = w.level_for(1000.0);
        assert!(fpp <= 1000.0 && fpp * 2.0 > 1000.0);
        assert!(w.span(0.0, 48_000.0).unwrap().peak > 100);
        assert!(w.span(-100.0, -1.0).is_none());
    }

    #[test]
    fn detail_is_fine_and_its_bass_colour_does_not_flicker() {
        let w = Waveform::build_detail(&tone(60.0, 2.0, 0.8));
        assert!(w.points_per_second >= 1000.0, "{}", w.points_per_second);
        assert_eq!(w.len(), 2400);
        // A 0.8 ms point is a twentieth of a 60 Hz period: with per-point
        // RMS the low band would swing from 0 to full; the envelope stays
        // within a few percent once settled.
        let lows: Vec<i32> = w.levels[0][200..2000]
            .iter()
            .map(|p| i32::from(p.low))
            .collect();
        let (lo, hi) = (lows.iter().min().unwrap(), lows.iter().max().unwrap());
        assert!(*lo > 150 && hi - lo < 40, "low band {lo}..{hi}");
        // The peak is an outline, not every cycle of the wave: no point
        // falls to the zero crossings.
        let peaks: Vec<u8> = w.levels[0][200..2000].iter().map(|p| p.peak).collect();
        let min = *peaks.iter().min().unwrap();
        assert!(min > 150, "peak falls to {min}");
        // A click stays sharp: the outline grows by the window only.
        let mut click = vec![0.0f32; 48_000 * 2];
        for s in &mut click[48_000..48_000 + 20] {
            *s = 0.9;
        }
        let c = Waveform::build_detail(&TrackAudio::from_interleaved(48_000, &click));
        let loud = c.levels[0].iter().filter(|p| p.peak > 100).count();
        let window_points = (DETAIL_PEAK_SECONDS * c.points_per_second).ceil() as usize + 2;
        assert!((1..=window_points).contains(&loud), "{loud} loud points");
    }

    /// A decaying 60 Hz note (a kick's tail): its outline goes down
    /// steadily, not in steps a cycle long (8 ms, 10 points).
    #[test]
    fn a_decaying_note_has_a_smooth_outline() {
        let sr = 48_000.0;
        let s: Vec<f32> = (0..48_000)
            .flat_map(|i| {
                let t = f64::from(i) / sr;
                let v = (0.8 * (-t / 0.1).exp() * (2.0 * PI * 60.0 * t).sin()) as f32;
                [v, v]
            })
            .collect();
        let w = Waveform::build_detail(&TrackAudio::from_interleaved(48_000, &s));
        // 50 to 125 ms: loud enough to fall by about a step per point or
        // more (later the rounding to whole steps alone repeats values).
        let peaks: Vec<i32> = w.levels[0][60..150]
            .iter()
            .map(|p| i32::from(p.peak))
            .collect();
        let mut run = 1;
        let mut longest = 1;
        for k in 1..peaks.len() {
            assert!(
                peaks[k] <= peaks[k - 1] + 1,
                "goes up at {k}: {} -> {}",
                peaks[k - 1],
                peaks[k]
            );
            run = if peaks[k] == peaks[k - 1] { run + 1 } else { 1 };
            longest = longest.max(run);
        }
        assert!(longest <= 4, "a flat step {longest} points long: {peaks:?}");
    }

    #[test]
    fn at_seconds_blends_between_points() {
        let mut w = Waveform::build(&tone(440.0, 1.0, 0.5));
        let pt = |peak| WavePoint {
            peak,
            low: peak,
            mid: 0,
            high: 0,
        };
        w.levels[0][10] = pt(100);
        w.levels[0][11] = pt(200);
        let s = w.point_seconds();
        // The middles of points 10 and 11, and half-way between them.
        assert_eq!(w.at_seconds(10.5 * s).unwrap().peak, 100);
        assert_eq!(w.at_seconds(11.5 * s).unwrap().peak, 200);
        assert_eq!(w.at_seconds(11.0 * s).unwrap().peak, 150);
        assert_eq!(w.at_seconds(10.75 * s).unwrap().low, 125);
        assert!(w.at_seconds(-0.1).is_none());
        assert!(w.at_seconds(2.0).is_none());
    }

    #[test]
    fn bytes_roundtrip() {
        let w = Waveform::build(&tone(440.0, 1.0, 0.5));
        let back = Waveform::from_bytes(&w.to_bytes()).unwrap();
        assert_eq!(back.levels, w.levels);
        assert_eq!(back.frames_per_point, w.frames_per_point);
    }
}
