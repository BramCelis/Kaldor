//! Constant-tempo beatgrids. Times are in seconds from the start of the
//! track (independent of sample rate).

use serde::{Deserialize, Serialize};

/// Beats per bar. The grid assumes 4/4 throughout.
pub const BEATS_PER_BAR: i64 = 4;

/// A constant-tempo beatgrid.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Beatgrid {
    pub bpm: f64,
    /// Time of the first beat at or after 0 s.
    pub first_beat: f64,
    /// Position of that first beat in its bar, 1 to 4 (1 = downbeat).
    pub first_beat_in_bar: u8,
    /// A locked grid is not changed by re-analysis or edits.
    pub locked: bool,
    /// Index (counted from `first_beat`, like `beat_time`) of the downbeat
    /// of the anchor bar, bar 1 (bar index 0, hence the name): the bar BEAT
    /// 1 was set on, or the first downbeat at or after the track start.
    /// Bars are numbered from it, and every edit carries it along with the
    /// beats, so the numbers move with the grid.
    #[serde(default)]
    pub bar_zero: i64,
}

impl Beatgrid {
    /// A grid with beat one (bar 1) at `first_beat`.
    pub fn new(bpm: f64, first_beat: f64) -> Self {
        let mut g = Self {
            bpm,
            first_beat,
            first_beat_in_bar: 1,
            locked: false,
            bar_zero: 0,
        };
        g.normalise();
        g
    }

    /// Bars from the anchor bar to the bar of beat `n`: 0 for the anchor
    /// bar, 1 for the next, -1 for the one before.
    pub fn bar_index(&self, n: i64) -> i64 {
        let bar = |m: i64| (m + i64::from(self.first_beat_in_bar) - 1).div_euclid(BEATS_PER_BAR);
        bar(n) - bar(self.bar_zero)
    }

    /// Bar number of beat `n` as shown: the anchor bar is 1, the bars after
    /// it 2, 3, ..., the bars before it -1, -2, ... (there is no bar 0).
    pub fn bar_number(&self, n: i64) -> i64 {
        let i = self.bar_index(n);
        if i >= 0 { i + 1 } else { i }
    }

    /// Time of bar 1's downbeat (the bar 1 marker), in seconds. When bar 1
    /// starts before the file does (a track that opens on its 2nd beat), the
    /// track start, which is in bar 1. Bar 1 more than a bar before the
    /// start (a grid slipped far back): the first bar line after the start.
    pub fn bar_one_time(&self) -> f64 {
        let t = self.beat_time(self.bar_zero);
        let bar = self.period() * BEATS_PER_BAR as f64;
        if t >= 0.0 || !(bar.is_finite() && bar > 0.0) {
            t.max(0.0)
        } else if t > -bar {
            0.0
        } else {
            t + (-t / bar).ceil() * bar
        }
    }

    /// Numbers the bars as for a grid nobody has set beat one on: bar 1 is
    /// the bar the track starts in (the bar of the first beat at or after
    /// 0 s; its downbeat can be just before the start, when the track opens
    /// on a later beat of the bar). Until 2026-11-16 (later that day) this
    /// was the first downbeat after the start, which made the bar the music
    /// starts in bar -1.
    pub fn number_bars_from_start(&mut self) {
        let p = self.period();
        // The first beat at or after 0 s, then the downbeat of its bar.
        let first = if p.is_finite() && p > 0.0 {
            (-self.first_beat / p).ceil() as i64
        } else {
            0
        };
        let in_bar = (first + i64::from(self.first_beat_in_bar) - 1).rem_euclid(BEATS_PER_BAR);
        self.bar_zero = first - in_bar;
    }

    /// Beat length in seconds.
    pub fn period(&self) -> f64 {
        60.0 / self.bpm
    }

    /// Moves `first_beat` into `[0, period)` while keeping every beat, bar
    /// position and bar number where it was.
    fn normalise(&mut self) {
        let p = self.period();
        if !(p.is_finite() && p > 0.0) {
            return;
        }
        let shift = (self.first_beat / p).floor();
        self.first_beat -= shift * p;
        let bar = (i64::from(self.first_beat_in_bar) - 1 - shift as i64).rem_euclid(BEATS_PER_BAR);
        self.first_beat_in_bar = bar as u8 + 1;
        // Beat n is beat n + shift now.
        self.bar_zero += shift as i64;
    }

    /// Time of beat `n` (beat 0 is `first_beat`; negative `n` is allowed).
    pub fn beat_time(&self, n: i64) -> f64 {
        self.first_beat + n as f64 * self.period()
    }

    /// Fractional beat index at time `t`.
    pub fn beat_at(&self, t: f64) -> f64 {
        (t - self.first_beat) / self.period()
    }

    /// Time of the beat nearest to `t`.
    pub fn nearest_beat(&self, t: f64) -> f64 {
        self.beat_time(self.beat_at(t).round() as i64)
    }

    /// True if beat `n` starts a bar.
    pub fn is_downbeat(&self, n: i64) -> bool {
        (n + i64::from(self.first_beat_in_bar) - 1).rem_euclid(BEATS_PER_BAR) == 0
    }

    /// Beats with `t0 <= time < t1`, as `(beat index, time, is downbeat)`.
    pub fn beats_between(&self, t0: f64, t1: f64) -> impl Iterator<Item = (i64, f64, bool)> + '_ {
        let start = self.beat_at(t0).ceil() as i64;
        let end = self.beat_at(t1).ceil() as i64;
        (start..end).map(move |n| (n, self.beat_time(n), self.is_downbeat(n)))
    }

    /// Shifts the whole grid by `dt` seconds.
    pub fn nudge(&mut self, dt: f64) {
        if !self.locked {
            self.first_beat += dt;
            self.normalise();
        }
    }

    /// Makes `t` a downbeat, the start of bar 1, and moves the grid so a
    /// beat falls exactly on it.
    pub fn set_beat_one(&mut self, t: f64) {
        if self.locked {
            return;
        }
        self.first_beat = t;
        self.first_beat_in_bar = 1;
        self.bar_zero = 0;
        self.normalise();
    }

    /// Doubles the tempo (a beat between each pair). Bar 1's downbeat stays
    /// a downbeat.
    pub fn double(&mut self) {
        if !self.locked {
            self.bpm *= 2.0;
            // Beat n is beat 2n now.
            self.bar_zero *= 2;
            self.downbeat_at_bar_zero();
            self.normalise();
        }
    }

    /// Halves the tempo: keeps every other beat, those that include bar 1's
    /// downbeat (so doubling and halving again gives the grid back), which
    /// stays a downbeat.
    pub fn halve(&mut self) {
        if !self.locked {
            if self.bar_zero.rem_euclid(2) == 1 {
                // Count from the next beat, so bar 1's downbeat is even.
                self.first_beat += self.period();
                self.bar_zero -= 1;
            }
            self.bpm /= 2.0;
            // Beat 2n is beat n now.
            self.bar_zero /= 2;
            self.downbeat_at_bar_zero();
            self.normalise();
        }
    }

    /// Bar positions such that beat `bar_zero` is a downbeat.
    fn downbeat_at_bar_zero(&mut self) {
        self.first_beat_in_bar = ((-self.bar_zero).rem_euclid(BEATS_PER_BAR) + 1) as u8;
    }

    /// Stretches or squeezes the grid around the beat nearest `anchor`: that
    /// beat stays where it is and the beat nearest `at` moves by `dt`
    /// seconds (later when positive), the beats in between in proportion.
    /// Bar positions are kept. Ignored when the result would leave
    /// 20-400 BPM.
    pub fn stretch(&mut self, anchor: f64, at: f64, dt: f64) {
        if self.locked || !dt.is_finite() {
            return;
        }
        let p = self.period();
        let k = self.beat_at(anchor).round();
        let a = self.first_beat + k * p;
        let mut n = ((at - a) / p).round();
        if n == 0.0 {
            // At the anchor itself: move the next beat.
            n = if at >= a { 1.0 } else { -1.0 };
        }
        let new_p = p + dt / n;
        let bpm = 60.0 / new_p;
        if !(new_p > 0.0 && (20.0..=400.0).contains(&bpm)) {
            return;
        }
        self.bpm = bpm;
        self.first_beat = a - k * new_p;
        self.normalise();
    }
}

/// Computes BPM from tap times (seconds). Needs at least two taps; uses a
/// least-squares fit over all taps.
pub fn tap_tempo(taps: &[f64]) -> Option<f64> {
    if taps.len() < 2 {
        return None;
    }
    let n = taps.len() as f64;
    let mean_i = (n - 1.0) / 2.0;
    let mean_t = taps.iter().sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, t) in taps.iter().enumerate() {
        let di = i as f64 - mean_i;
        num += di * (t - mean_t);
        den += di * di;
    }
    let period = num / den;
    (period > 0.0).then(|| 60.0 / period)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn stretch_keeps_the_anchor_and_moves_the_beat_at_the_playhead() {
        let mut g = Beatgrid::new(120.0, 0.25);
        g.set_beat_one(10.25);
        let bar_of = |g: &Beatgrid, t: f64| g.is_downbeat(g.beat_at(t).round() as i64);
        assert!(bar_of(&g, 10.25));
        // Beat 40 after the anchor (30.25 s) moves 20 ms later.
        g.stretch(10.25, 30.3, 0.02);
        assert!((g.nearest_beat(10.25) - 10.25).abs() < 1e-9, "anchor moved");
        assert!((g.nearest_beat(30.27) - 30.27).abs() < 1e-9);
        assert!((g.period() - (0.5 + 0.02 / 40.0)).abs() < 1e-12);
        assert!(bar_of(&g, 10.25) && bar_of(&g, 30.27), "bars kept");
        // Before the anchor works the other way round.
        let mut h = Beatgrid::new(120.0, 0.0);
        h.stretch(20.0, 10.0, -0.01);
        assert!((h.nearest_beat(20.0) - 20.0).abs() < 1e-9);
        assert!((h.nearest_beat(9.99) - 9.99).abs() < 1e-9);
        // At the anchor: the next beat moves. Locked: nothing.
        let mut c = Beatgrid::new(120.0, 0.0);
        c.stretch(4.0, 4.0, 0.01);
        assert!((c.period() - 0.51).abs() < 1e-12);
        let mut l = Beatgrid::new(120.0, 0.0);
        l.locked = true;
        l.stretch(4.0, 8.0, 0.01);
        assert_eq!(l.bpm, 120.0);
    }

    #[test]
    fn beats_and_bars() {
        let g = Beatgrid::new(120.0, 0.25);
        assert_eq!(g.beat_time(0), 0.25);
        assert_eq!(g.beat_time(4), 2.25);
        assert!(g.is_downbeat(0) && g.is_downbeat(4) && !g.is_downbeat(1));
        let beats: Vec<_> = g.beats_between(0.0, 1.3).collect();
        assert_eq!(beats.len(), 3);
        assert_eq!(beats[2].1, 1.25);
        assert!(
            (g.nearest_beat(1.0) - 0.75).abs() < 1e-12
                || (g.nearest_beat(1.0) - 1.25).abs() < 1e-12
        );
    }

    #[test]
    fn normalise_keeps_bar_positions() {
        // Downbeat at 1.3 s with a 0.5 s beat: earliest beat 0.3 s is
        // beat 3 of the bar before.
        let mut g = Beatgrid::new(120.0, 0.0);
        g.set_beat_one(1.3);
        assert!((g.first_beat - 0.3).abs() < 1e-12);
        assert_eq!(g.first_beat_in_bar, 3);
        assert!(g.is_downbeat(2));
    }

    #[test]
    fn edits_respect_lock() {
        let mut g = Beatgrid::new(128.0, 0.1);
        g.locked = true;
        g.nudge(0.01);
        g.double();
        assert_eq!(
            g,
            Beatgrid {
                locked: true,
                ..Beatgrid::new(128.0, 0.1)
            }
        );
        g.locked = false;
        g.double();
        assert_eq!(g.bpm, 256.0);
        g.halve();
        assert_eq!(g.bpm, 128.0);
    }

    /// Bar numbers stay with the grid: BEAT 1 makes the bar
    /// at the play position bar 1 (the bar before it is -1: there is no
    /// bar 0), and slipping, stretching, halving and doubling keep it there
    /// (they used to renumber from the start).
    #[test]
    fn bar_numbers_move_with_the_grid() {
        let bar_at = |g: &Beatgrid, t: f64| g.bar_number(g.beat_at(t).round() as i64);
        // An analysed grid whose first downbeat is at 1.3 s: the track
        // starts in bar 1 (its beats 3 and 4 at 0.3 and 0.8 s), 1.3 s is bar 2.
        let mut g = Beatgrid::new(120.0, 0.0);
        g.set_beat_one(1.3);
        g.number_bars_from_start();
        assert_eq!(bar_at(&g, 0.3), 1, "the bar the track starts in");
        assert_eq!(bar_at(&g, 1.3), 2);
        assert_eq!(bar_at(&g, 3.3), 3);
        // As the AFTER PARTY test track: first beat at 0.0002 s is beat 2 of
        // its bar; the music starts in bar 1, not -1.
        let mut a = Beatgrid::new(163.0, 0.0);
        a.set_beat_one(0.0002 - 60.0 / 163.0);
        a.number_bars_from_start();
        assert_eq!((a.first_beat_in_bar, bar_at(&a, 0.0002)), (2, 1));
        assert_eq!(a.bar_one_time(), 0.0, "Start at Bar 1: the track start");
        // A track that starts on a downbeat: bar 1 there.
        let mut d = Beatgrid::new(128.0, 0.05);
        d.number_bars_from_start();
        assert_eq!(bar_at(&d, 0.05), 1);
        assert!((d.bar_one_time() - 0.05).abs() < 1e-12);
        // BEAT 1 on the first kick at 17.3 s (two bars later): -1 then 1.
        g.set_beat_one(17.3);
        assert_eq!(bar_at(&g, 17.3), 1);
        assert_eq!(bar_at(&g, 15.3), -1, "no bar 0");
        assert_eq!(bar_at(&g, 13.3), -2);
        assert_eq!(bar_at(&g, 19.3), 2);
        assert_eq!(g.bar_index(g.bar_zero), 0);
        // Slipping by a whole beat and more: the bar moves with the grid.
        for dt in [0.002, 0.3, 0.6, -0.9] {
            let mut s = g;
            s.nudge(dt);
            assert_eq!(bar_at(&s, 17.3 + dt), 1, "slipped {dt}");
            assert_eq!(bar_at(&s, 19.3 + dt), 2);
        }
        // Stretched around a beat far away: bar 1 is still the same beat.
        let mut s = g;
        s.stretch(60.0, 61.0, 0.01);
        let one = s.nearest_beat(17.3);
        assert_eq!(bar_at(&s, one), 1);
        // Doubled and halved back: the same grid, bar 1 where it was (halving
        // used to keep the inserted beats and shift the grid half a beat).
        let mut d = g;
        d.double();
        assert_eq!(bar_at(&d, 17.3), 1);
        assert!(d.is_downbeat(d.beat_at(17.3).round() as i64));
        d.halve();
        assert!(
            (d.nearest_beat(17.3) - 17.3).abs() < 1e-9,
            "a beat at 17.3 s"
        );
        assert_eq!(bar_at(&d, 17.3), 1);
        assert_eq!(bar_at(&d, 19.3), 2);
        assert_eq!(d, g, "the grid as before");
        // Halved first (bar 1's downbeat may be an odd beat): it stays a beat.
        let mut h = g;
        h.nudge(0.5);
        h.halve();
        assert!((h.nearest_beat(17.8) - 17.8).abs() < 1e-9);
        assert_eq!(bar_at(&h, 17.8), 1);
        assert!(h.is_downbeat(h.beat_at(17.8).round() as i64));
    }

    /// The bar 1 marker: where BEAT 1 was set, moved along by edits; the
    /// first downbeat of the track for an untouched grid.
    #[test]
    fn bar_one_time_is_the_bar_1_marker() {
        let mut g = Beatgrid::new(120.0, 0.0);
        g.set_beat_one(1.3);
        g.number_bars_from_start();
        assert_eq!(g.bar_one_time(), 0.0, "bar 1 starts before the file");
        g.set_beat_one(17.3);
        assert!((g.bar_one_time() - 17.3).abs() < 1e-9, "BEAT 1");
        g.nudge(0.6);
        assert!(
            (g.bar_one_time() - 17.9).abs() < 1e-9,
            "slipped with the grid"
        );
        // Bar 1 starting just before the file (the track starts inside it):
        // the start. Slipped back more than a bar: the first bar line after.
        let mut p = Beatgrid::new(120.0, 0.0);
        p.set_beat_one(0.5);
        p.nudge(-1.0);
        assert_eq!(p.bar_one_time(), 0.0);
        p.nudge(-2.0);
        assert!(
            (p.bar_one_time() - 1.5).abs() < 1e-9,
            "{}",
            p.bar_one_time()
        );
    }

    #[test]
    fn tap_tempo_fits() {
        let taps: Vec<f64> = (0..8).map(|i| 1.0 + i as f64 * 0.5).collect();
        assert!((tap_tempo(&taps).unwrap() - 120.0).abs() < 1e-9);
        assert!(tap_tempo(&[1.0]).is_none());
    }

    proptest! {
        #[test]
        fn downbeats_survive_normalisation(bpm in 60.0f64..200.0, t in -100.0f64..400.0) {
            let mut g = Beatgrid::new(bpm, 0.0);
            g.set_beat_one(t);
            prop_assert!(g.first_beat >= 0.0 && g.first_beat < g.period() + 1e-9);
            let n = g.beat_at(t).round() as i64;
            prop_assert!(g.is_downbeat(n));
            prop_assert!((g.beat_time(n) - t).abs() < 1e-6);
            prop_assert_eq!(g.bar_number(n), 1, "beat one starts bar 1");
            prop_assert_eq!(g.bar_number(n - 1), -1, "no bar 0");
        }
    }
}
