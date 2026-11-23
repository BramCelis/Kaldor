//! The software mixer: four channel strips, crossfader, headphone cue and
//! master section. Runs on the audio thread; every gain is smoothed.
//!
//! Signal flow per channel:
//! deck -> trim (and auto-gain) -> 3-band EQ -> filter (or the color FX
//! on the filter knob) -> [PFL to cue bus] -> channel fader -> crossfader
//! -> FX units -> master bus.
//!
//! Master: (sampler, master FX) -> master gain -> limiter -> master out; booth = master * booth gain;
//! headphones = (cue bus * (1 - mix) + master * mix) * cue gain.

use dj_core::{DeckId, NUM_DECKS};
use dj_dsp::LinearSmoother;
use dj_dsp::biquad::{ClassicEq, IsolatorSplit};
use dj_dsp::color_fx::{ColorFx, ColorProcessor};
use dj_dsp::curves::{
    CLASSIC_KILL_DB, CUT_LAG_RANGE, CrossfaderCurve, DEFAULT_CUT_LAG, EQ_BOOSTS_DB, EqMode,
    FaderCurve, classic_eq_db, isolator_gain, trim_gain,
};
use dj_dsp::limiter::MasterLimiter;
use dj_dsp::svf::{DEFAULT_FILTER_RESONANCE, DjFilter, FilterSlope};
use dj_dsp::time_constant_coeff;

/// Crossfader assignment of a channel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum XfaderSide {
    A,
    /// Not affected by the crossfader.
    #[default]
    Thru,
    B,
}

/// EQ band.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EqBand {
    Low = 0,
    Mid = 1,
    High = 2,
}

/// Mixer commands.
#[derive(Clone, Copy, Debug)]
pub enum MixerCommand {
    /// Trim knob, -1 (-12 dB) to 1 (+12 dB).
    Trim(DeckId, f32),
    /// EQ knob, 0 (kill) to 0.5 (0 dB) to 1 (+6 dB).
    Eq(DeckId, EqBand, f32),
    Kill(DeckId, EqBand, bool),
    /// Filter knob, -1 (low-pass) to 0 (off) to 1 (high-pass).
    Filter(DeckId, f32),
    /// Channel fader, 0 to 1.
    Fader(DeckId, f32),
    XfaderAssign(DeckId, XfaderSide),
    /// Headphone cue (pre-fader listen).
    Pfl(DeckId, bool),
    /// Headphone cue on or off, whatever it is now (controller buttons).
    TogglePfl(DeckId),
    /// Auto-gain offset in dB from loudness analysis (0 to disable).
    AutoGain(DeckId, f32),
    /// Crossfader, -1 (side A) to 1 (side B).
    Crossfader(f32),
    CrossfaderCurve(CrossfaderCurve),
    CrossfaderReverse(bool),
    FaderCurve(FaderCurve),
    /// Headphone level in dB.
    CueGain(f32),
    /// Headphone mix, 0 (cue only) to 1 (master only).
    CueMix(f32),
    /// Master level in dB.
    MasterGain(f32),
    /// Booth level in dB.
    BoothGain(f32),
    Limiter(bool),
    /// EQ crossover frequencies in Hz (low/mid, mid/high).
    EqCrossovers(f32, f32),
    /// EQ topology: full-kill isolator or classic shelves.
    EqMode(EqMode),
    /// Most an EQ knob boosts, in dB (6 or 12).
    EqBoost(f32),
    /// Channel filter slope and resonance (0 to 1).
    FilterShape(FilterSlope, f32),
    /// Scratch cut-in distance of the Cut crossfader curve (-1..1 units).
    CrossfaderCutLag(f32),
    /// Limiter ceiling in dBFS (-2.0 to -0.1).
    LimiterCeiling(f32),
    /// How long the meters' peak lines stay up, in seconds (0 to 5).
    MeterHold(f32),
    /// Headphone cue taken before the EQ and filter (`true`) or after.
    CuePreEq(bool),
    /// What every channel's filter knob plays: the filter or a color FX.
    ColorFx(ColorFx),
}

/// One channel's settings.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChannelParams {
    pub trim: f32,
    pub eq: [f32; 3],
    pub kill: [bool; 3],
    pub filter: f32,
    pub fader: f32,
    pub xfader: XfaderSide,
    pub pfl: bool,
    pub auto_gain_db: f32,
}

impl Default for ChannelParams {
    fn default() -> Self {
        Self {
            trim: 0.0,
            eq: [0.5; 3],
            kill: [false; 3],
            filter: 0.0,
            fader: 1.0,
            xfader: XfaderSide::Thru,
            pfl: false,
            auto_gain_db: 0.0,
        }
    }
}

/// All mixer settings; included in every engine snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MixerParams {
    pub channels: [ChannelParams; NUM_DECKS],
    pub crossfader: f32,
    pub crossfader_curve: CrossfaderCurve,
    pub crossfader_reverse: bool,
    pub fader_curve: FaderCurve,
    pub cue_gain_db: f32,
    pub cue_mix: f32,
    pub master_gain_db: f32,
    pub booth_gain_db: f32,
    pub limiter: bool,
    pub eq_crossovers: (f32, f32),
    pub eq_mode: EqMode,
    pub eq_boost_db: f32,
    pub filter_slope: FilterSlope,
    pub filter_resonance: f32,
    pub cut_lag: f32,
    pub limiter_ceiling_db: f32,
    pub meter_hold_s: f32,
    pub cue_pre_eq: bool,
    /// What the filter knobs play.
    pub color_fx: ColorFx,
}

impl Default for MixerParams {
    fn default() -> Self {
        Self {
            channels: [ChannelParams::default(); NUM_DECKS],
            crossfader: 0.0,
            crossfader_curve: CrossfaderCurve::default(),
            crossfader_reverse: false,
            fader_curve: FaderCurve::default(),
            cue_gain_db: 0.0,
            cue_mix: 0.0,
            master_gain_db: 0.0,
            booth_gain_db: 0.0,
            limiter: true,
            eq_crossovers: (
                IsolatorSplit::DEFAULT_LOW_HZ as f32,
                IsolatorSplit::DEFAULT_HIGH_HZ as f32,
            ),
            eq_mode: EqMode::default(),
            eq_boost_db: EQ_BOOSTS_DB[0],
            filter_slope: FilterSlope::default(),
            filter_resonance: DEFAULT_FILTER_RESONANCE,
            cut_lag: DEFAULT_CUT_LAG,
            limiter_ceiling_db: -0.3,
            meter_hold_s: DEFAULT_HOLD_SECONDS,
            cue_pre_eq: false,
            color_fx: ColorFx::Filter,
        }
    }
}

impl MixerParams {
    /// The commands that bring a fresh mixer to these settings (used to
    /// carry the mixer over when the audio device changes).
    pub fn commands(&self) -> Vec<MixerCommand> {
        let mut out = Vec::with_capacity(NUM_DECKS * 12 + 14);
        for (d, c) in DeckId::ALL.iter().zip(self.channels.iter()) {
            out.push(MixerCommand::Trim(*d, c.trim));
            for (b, band) in [EqBand::Low, EqBand::Mid, EqBand::High]
                .into_iter()
                .enumerate()
            {
                out.push(MixerCommand::Eq(*d, band, c.eq[b]));
                out.push(MixerCommand::Kill(*d, band, c.kill[b]));
            }
            out.push(MixerCommand::Filter(*d, c.filter));
            out.push(MixerCommand::Fader(*d, c.fader));
            out.push(MixerCommand::XfaderAssign(*d, c.xfader));
            out.push(MixerCommand::Pfl(*d, c.pfl));
            out.push(MixerCommand::AutoGain(*d, c.auto_gain_db));
        }
        out.extend([
            MixerCommand::Crossfader(self.crossfader),
            MixerCommand::CrossfaderCurve(self.crossfader_curve),
            MixerCommand::CrossfaderReverse(self.crossfader_reverse),
            MixerCommand::FaderCurve(self.fader_curve),
            MixerCommand::CueGain(self.cue_gain_db),
            MixerCommand::CueMix(self.cue_mix),
            MixerCommand::MasterGain(self.master_gain_db),
            MixerCommand::BoothGain(self.booth_gain_db),
            MixerCommand::Limiter(self.limiter),
            MixerCommand::EqCrossovers(self.eq_crossovers.0, self.eq_crossovers.1),
            MixerCommand::EqMode(self.eq_mode),
            MixerCommand::EqBoost(self.eq_boost_db),
            MixerCommand::FilterShape(self.filter_slope, self.filter_resonance),
            MixerCommand::CrossfaderCutLag(self.cut_lag),
            MixerCommand::LimiterCeiling(self.limiter_ceiling_db),
            MixerCommand::MeterHold(self.meter_hold_s),
            MixerCommand::CuePreEq(self.cue_pre_eq),
            MixerCommand::ColorFx(self.color_fx),
        ]);
        out
    }
}

/// Peak (since last snapshot), meter level, peak hold and RMS (300 ms) of a
/// stereo signal.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Meter {
    pub peak: [f32; 2],
    pub rms: [f32; 2],
    /// The meter bar, as on a hardware DJ mixer: rises at once with the
    /// peaks and falls at 20 dB per second.
    pub level: [f32; 2],
    /// The peak line: held for 0.8 s, then falling at 40 dB per second
    /// back onto the bar (never below it).
    pub hold: [f32; 2],
}

/// Fall of the bar, hold time and fall of the peak line.
const LEVEL_FALL_DB_PER_SECOND: f32 = 20.0;
/// Default hold of the peak line (settable with [`MixerCommand::MeterHold`]).
pub const DEFAULT_HOLD_SECONDS: f32 = 0.8;
const HOLD_FALL_DB_PER_SECOND: f32 = 40.0;

#[derive(Clone, Copy, Debug, Default)]
struct MeterState {
    peak: [f32; 2],
    ms: [f32; 2],
    /// Peak of the block being fed.
    block: [f32; 2],
    level: [f32; 2],
    hold: [f32; 2],
    /// Frames the hold stays before it falls.
    hold_left: [u32; 2],
}

impl MeterState {
    #[inline]
    fn feed(&mut self, l: f32, r: f32, k: f32) {
        self.block[0] = self.block[0].max(l.abs());
        self.block[1] = self.block[1].max(r.abs());
        self.ms[0] += (l * l - self.ms[0]) * k;
        self.ms[1] += (r * r - self.ms[1]) * k;
    }

    /// After a block of `n` frames: bar and peak line ballistics.
    fn end_block(&mut self, n: usize, sample_rate: u32, hold_seconds: f32) {
        let sr = sample_rate.max(1) as f32;
        let fall = |db_per_s: f32| 10f32.powf(-db_per_s * n as f32 / (20.0 * sr));
        for c in 0..2 {
            let b = self.block[c];
            self.peak[c] = self.peak[c].max(b);
            self.level[c] = (self.level[c] * fall(LEVEL_FALL_DB_PER_SECOND)).max(b);
            if self.level[c] < 1e-6 {
                self.level[c] = 0.0;
            }
            if b >= self.hold[c] {
                self.hold[c] = b;
                self.hold_left[c] = (hold_seconds * sr) as u32;
            } else if self.hold_left[c] > 0 {
                self.hold_left[c] = self.hold_left[c].saturating_sub(n as u32);
            } else {
                self.hold[c] *= fall(HOLD_FALL_DB_PER_SECOND);
            }
            self.hold[c] = self.hold[c].max(self.level[c]);
            self.block[c] = 0.0;
        }
    }

    fn read(&self) -> Meter {
        Meter {
            peak: self.peak,
            rms: [self.ms[0].sqrt(), self.ms[1].sqrt()],
            level: self.level,
            hold: self.hold,
        }
    }
}

struct Strip {
    split_l: IsolatorSplit,
    split_r: IsolatorSplit,
    classic: ClassicEq,
    trim: LinearSmoother,
    /// Isolator band gains (linear).
    eq: [LinearSmoother; 3],
    /// 0 = EQ bypassed (bit-exact dry signal), 1 = EQ in the path. The EQ
    /// always runs so its state is continuous; only the output blends.
    eq_mix: LinearSmoother,
    /// 0 = isolator, 1 = classic; between the two while the mode changes.
    eq_mode: LinearSmoother,
    filter: DjFilter,
    /// The color FX on the filter knob (when one is chosen).
    color: ColorProcessor,
    fader: LinearSmoother,
    xfader: LinearSmoother,
    pfl: LinearSmoother,
    /// Headphone cue taken before the EQ (the other `pfl` is after it).
    pfl_pre: LinearSmoother,
    meter: MeterState,
}

/// Dezipper time for knobs and faders.
const SMOOTH_MS: f32 = 8.0;
/// Crossfader smoothing is short so cuts stay sharp for scratching, but
/// never a step (a 0.5 ms ramp).
const XFADER_SMOOTH_MS: f32 = 0.5;

impl Strip {
    fn new(sr: u32, max_block: usize, xover: (f32, f32)) -> Self {
        let split = IsolatorSplit::new(f64::from(xover.0), f64::from(xover.1), f64::from(sr));
        let (lo, hi) = ClassicEq::freqs_for(f64::from(xover.0), f64::from(xover.1));
        Self {
            split_l: split,
            split_r: split,
            classic: ClassicEq::new(lo, hi, f64::from(sr)),
            trim: LinearSmoother::with_time(1.0, sr, SMOOTH_MS),
            eq: std::array::from_fn(|_| LinearSmoother::with_time(1.0, sr, SMOOTH_MS)),
            eq_mix: LinearSmoother::with_time(0.0, sr, 20.0),
            eq_mode: LinearSmoother::with_time(0.0, sr, 20.0),
            filter: DjFilter::new(sr),
            color: ColorProcessor::new(sr, max_block),
            fader: LinearSmoother::with_time(1.0, sr, SMOOTH_MS),
            xfader: LinearSmoother::with_time(1.0, sr, XFADER_SMOOTH_MS),
            pfl: LinearSmoother::with_time(0.0, sr, SMOOTH_MS),
            pfl_pre: LinearSmoother::with_time(0.0, sr, SMOOTH_MS),
            meter: MeterState::default(),
        }
    }
}

/// A fixed stereo delay (the headphone cue waits for the master limiter's
/// lookahead so cue and master stay in time).
struct StereoDelay {
    l: Vec<f32>,
    r: Vec<f32>,
    pos: usize,
}

impl StereoDelay {
    fn new(frames: usize) -> Self {
        Self {
            l: vec![0.0; frames.max(1)],
            r: vec![0.0; frames.max(1)],
            pos: 0,
        }
    }

    #[inline]
    fn push(&mut self, l: f32, r: f32) -> (f32, f32) {
        let (Some(a), Some(b)) = (self.l.get_mut(self.pos), self.r.get_mut(self.pos)) else {
            return (l, r);
        };
        let out = (std::mem::replace(a, l), std::mem::replace(b, r));
        self.pos += 1;
        if self.pos >= self.l.len() {
            self.pos = 0;
        }
        out
    }
}

/// The mixer.
pub(crate) struct Mixer {
    sample_rate: u32,
    params: MixerParams,
    strips: [Strip; NUM_DECKS],
    master_gain: LinearSmoother,
    booth_gain: LinearSmoother,
    cue_gain: LinearSmoother,
    cue_mix: LinearSmoother,
    limiter: MasterLimiter,
    /// The headphones and the booth get the same protection as the master
    /// (always on): before, a hot pre-fader cue or a cue / booth level
    /// above 0 dB was hard-clipped at full scale, heard as crackle and
    /// clicks. Each adds about 2 ms to its own output.
    cue_limiter: MasterLimiter,
    booth_limiter: MasterLimiter,
    cue_delay: StereoDelay,
    master_meter: MeterState,
    cue_meter: MeterState,
    meter_k: f32,
}

fn db_gain(db: f32) -> f32 {
    if db <= -80.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

impl Mixer {
    pub(crate) fn new(sample_rate: u32, max_block: usize) -> Self {
        let params = MixerParams::default();
        let limiter = MasterLimiter::new(sample_rate);
        let mut m = Self {
            sample_rate,
            strips: std::array::from_fn(|_| {
                Strip::new(sample_rate, max_block, params.eq_crossovers)
            }),
            master_gain: LinearSmoother::with_time(1.0, sample_rate, SMOOTH_MS),
            booth_gain: LinearSmoother::with_time(1.0, sample_rate, SMOOTH_MS),
            cue_gain: LinearSmoother::with_time(1.0, sample_rate, SMOOTH_MS),
            cue_mix: LinearSmoother::with_time(0.0, sample_rate, SMOOTH_MS),
            cue_delay: StereoDelay::new(limiter.latency()),
            limiter,
            cue_limiter: MasterLimiter::new(sample_rate),
            booth_limiter: MasterLimiter::new(sample_rate),
            master_meter: MeterState::default(),
            cue_meter: MeterState::default(),
            meter_k: 1.0 - time_constant_coeff(sample_rate, 300.0),
            params,
        };
        m.apply_all(true);
        m
    }

    pub(crate) fn params(&self) -> &MixerParams {
        &self.params
    }

    /// Frames the master output lags the channels by (the limiter's
    /// lookahead and oversampling).
    pub(crate) fn master_latency(&self) -> usize {
        self.limiter.latency()
    }

    pub(crate) fn command(&mut self, cmd: MixerCommand) {
        let p = &mut self.params;
        let ch = |id: DeckId| id.index().min(NUM_DECKS - 1);
        match cmd {
            MixerCommand::Trim(d, v) => p.channels[ch(d)].trim = v.clamp(-1.0, 1.0),
            MixerCommand::Eq(d, b, v) => p.channels[ch(d)].eq[b as usize] = v.clamp(0.0, 1.0),
            MixerCommand::Kill(d, b, k) => p.channels[ch(d)].kill[b as usize] = k,
            MixerCommand::Filter(d, v) => p.channels[ch(d)].filter = v.clamp(-1.0, 1.0),
            MixerCommand::Fader(d, v) => p.channels[ch(d)].fader = v.clamp(0.0, 1.0),
            MixerCommand::XfaderAssign(d, s) => p.channels[ch(d)].xfader = s,
            MixerCommand::Pfl(d, on) => p.channels[ch(d)].pfl = on,
            MixerCommand::TogglePfl(d) => {
                let c = &mut p.channels[ch(d)];
                c.pfl = !c.pfl;
            }
            MixerCommand::AutoGain(d, db) => p.channels[ch(d)].auto_gain_db = db.clamp(-24.0, 24.0),
            MixerCommand::Crossfader(x) => p.crossfader = x.clamp(-1.0, 1.0),
            MixerCommand::CrossfaderCurve(c) => p.crossfader_curve = c,
            MixerCommand::CrossfaderReverse(r) => p.crossfader_reverse = r,
            MixerCommand::FaderCurve(c) => p.fader_curve = c,
            MixerCommand::CueGain(db) => p.cue_gain_db = db.clamp(-80.0, 12.0),
            MixerCommand::CueMix(m) => p.cue_mix = m.clamp(0.0, 1.0),
            MixerCommand::MasterGain(db) => p.master_gain_db = db.clamp(-80.0, 12.0),
            MixerCommand::BoothGain(db) => p.booth_gain_db = db.clamp(-80.0, 12.0),
            MixerCommand::Limiter(on) => p.limiter = on,
            MixerCommand::EqCrossovers(lo, hi) => {
                let lo = lo.clamp(50.0, 1000.0);
                let hi = hi.clamp(lo * 2.0, 10_000.0);
                p.eq_crossovers = (lo, hi);
                let sr = f64::from(self.sample_rate);
                let split = IsolatorSplit::new(f64::from(lo), f64::from(hi), sr);
                let (clo, chi) = ClassicEq::freqs_for(f64::from(lo), f64::from(hi));
                for s in &mut self.strips {
                    s.split_l = split;
                    s.split_r = split;
                    s.classic.set_freqs(clo, chi);
                }
            }
            MixerCommand::EqMode(m) => p.eq_mode = m,
            MixerCommand::EqBoost(db) => {
                // Only the offered boosts (the nearest one).
                p.eq_boost_db = if db >= 9.0 {
                    EQ_BOOSTS_DB[1]
                } else {
                    EQ_BOOSTS_DB[0]
                };
            }
            MixerCommand::FilterShape(slope, res) => {
                p.filter_slope = slope;
                p.filter_resonance = res.clamp(0.0, 1.0);
            }
            MixerCommand::CrossfaderCutLag(lag) => {
                p.cut_lag = lag.clamp(CUT_LAG_RANGE.0, CUT_LAG_RANGE.1);
            }
            MixerCommand::LimiterCeiling(db) => {
                p.limiter_ceiling_db = db.clamp(-2.0, -0.1);
                self.limiter.set_ceiling_db(p.limiter_ceiling_db);
            }
            MixerCommand::MeterHold(s) => p.meter_hold_s = s.clamp(0.0, 5.0),
            MixerCommand::CuePreEq(on) => p.cue_pre_eq = on,
            MixerCommand::ColorFx(c) => p.color_fx = c,
        }
        self.apply_all(false);
    }

    /// Pushes the parameter values into the smoothers.
    fn apply_all(&mut self, immediate: bool) {
        let p = self.params;
        let (ga, gb) = p.crossfader_curve.gains_with_lag(p.crossfader, p.cut_lag);
        let (ga, gb) = if p.crossfader_reverse {
            (gb, ga)
        } else {
            (ga, gb)
        };
        let set = |sm: &mut LinearSmoother, v: f32| {
            if immediate {
                sm.set_immediate(v);
            } else {
                sm.set_target(v);
            }
        };
        for (s, c) in self.strips.iter_mut().zip(p.channels.iter()) {
            set(&mut s.trim, trim_gain(c.trim) * db_gain(c.auto_gain_db));
            let mut flat = true;
            let mut db = [0.0f32; 3];
            for (b, d) in db.iter_mut().enumerate() {
                let g = if c.kill[b] {
                    0.0
                } else {
                    isolator_gain(c.eq[b], p.eq_boost_db)
                };
                *d = if c.kill[b] {
                    CLASSIC_KILL_DB
                } else {
                    classic_eq_db(c.eq[b], p.eq_boost_db)
                };
                flat &= g == 1.0;
                set(&mut s.eq[b], g);
            }
            s.classic.set_gains_db(db);
            if immediate {
                s.classic.settle();
            }
            set(&mut s.eq_mix, if flat { 0.0 } else { 1.0 });
            set(
                &mut s.eq_mode,
                match p.eq_mode {
                    EqMode::Isolator => 0.0,
                    EqMode::Classic => 1.0,
                },
            );
            s.filter.set(c.filter);
            s.filter.set_shape(p.filter_slope, p.filter_resonance);
            set(&mut s.fader, p.fader_curve.gain(c.fader));
            let xf = match c.xfader {
                XfaderSide::A => ga,
                XfaderSide::B => gb,
                XfaderSide::Thru => 1.0,
            };
            set(&mut s.xfader, xf);
            set(&mut s.pfl, if c.pfl && !p.cue_pre_eq { 1.0 } else { 0.0 });
            set(
                &mut s.pfl_pre,
                if c.pfl && p.cue_pre_eq { 1.0 } else { 0.0 },
            );
        }
        set(&mut self.master_gain, db_gain(p.master_gain_db));
        set(&mut self.booth_gain, db_gain(p.booth_gain_db));
        set(&mut self.cue_gain, db_gain(p.cue_gain_db));
        set(&mut self.cue_mix, p.cue_mix);
    }

    /// Processes one channel in place up to the faders (the result is what
    /// goes to the master, before the channel's FX) and adds its PFL to the
    /// cue bus. `beat` is the deck's (BPM, bar-aligned beat) when it has a
    /// beatgrid, else the color FX run at `fallback_bpm`.
    pub(crate) fn channel(
        &mut self,
        index: usize,
        l: &mut [f32],
        r: &mut [f32],
        cue: (&mut [f32], &mut [f32]),
        beat: Option<(f64, f64)>,
        fallback_bpm: f64,
    ) {
        let color = self.params.color_fx;
        let knob = self.params.channels.get(index).map_or(0.0, |c| c.filter);
        let Some(s) = self.strips.get_mut(index) else {
            return;
        };
        // Trim and EQ. Only the EQ in use runs (both while the mode
        // changes); the other is reset so it starts clean.
        let isolator_only = s.eq_mode.is_settled() && s.eq_mode.current() == 0.0;
        let classic_only = s.eq_mode.is_settled() && s.eq_mode.current() == 1.0;
        if isolator_only {
            s.classic.reset();
        }
        if classic_only {
            s.split_l.reset();
            s.split_r.reset();
        }
        let (cue_l, cue_r) = cue;
        for ((xl, xr), (ql, qr)) in l
            .iter_mut()
            .zip(r.iter_mut())
            .zip(cue_l.iter_mut().zip(cue_r.iter_mut()))
        {
            let t = s.trim.next_value();
            let g0 = s.eq[0].next_value();
            let g1 = s.eq[1].next_value();
            let g2 = s.eq[2].next_value();
            let mix = s.eq_mix.next_value();
            let mode = s.eq_mode.next_value();
            let (dl, dr) = (*xl * t, *xr * t);
            // Headphone cue before the EQ (when set so).
            let pre = s.pfl_pre.next_value();
            *ql += dl * pre;
            *qr += dr * pre;
            let (mut il, mut ir) = (0.0, 0.0);
            if !classic_only {
                let (a, b, c) = s.split_l.process(dl);
                il = a * g0 + b * g1 + c * g2;
                let (a, b, c) = s.split_r.process(dr);
                ir = a * g0 + b * g1 + c * g2;
            }
            let (mut cl, mut cr) = (0.0, 0.0);
            if !isolator_only {
                (cl, cr) = s.classic.process(dl, dr);
            }
            let el = il + mode * (cl - il);
            let er = ir + mode * (cr - ir);
            *xl = dl + mix * (el - dl);
            *xr = dr + mix * (er - dr);
        }
        // The filter knob: the filter, or the color FX chosen for it. A
        // color FX switched off (or changed) starts clean next time.
        if color == ColorFx::Filter {
            s.filter.process(l, r);
        } else {
            s.color.process(color, knob, beat, fallback_bpm, l, r);
        }
        // PFL, fader, crossfader.
        for ((xl, xr), (cl, cr)) in l
            .iter_mut()
            .zip(r.iter_mut())
            .zip(cue_l.iter_mut().zip(cue_r.iter_mut()))
        {
            let pfl = s.pfl.next_value();
            *cl += *xl * pfl;
            *cr += *xr * pfl;
            let g = s.fader.next_value() * s.xfader.next_value();
            *xl *= g;
            *xr *= g;
        }
    }

    /// Adds a processed channel (after its FX) to the master bus and feeds
    /// its meter.
    pub(crate) fn sum_into_master(
        &mut self,
        index: usize,
        l: &[f32],
        r: &[f32],
        master: (&mut [f32], &mut [f32]),
    ) {
        let k = self.meter_k;
        let sr = self.sample_rate;
        let hold = self.params.meter_hold_s;
        let Some(s) = self.strips.get_mut(index) else {
            return;
        };
        let (ml, mr) = master;
        for ((xl, xr), (ml, mr)) in l.iter().zip(r.iter()).zip(ml.iter_mut().zip(mr.iter_mut())) {
            *ml += *xl;
            *mr += *xr;
            s.meter.feed(*xl, *xr, k);
        }
        s.meter.end_block(l.len().min(r.len()), sr, hold);
    }

    /// Master section: gain, soft clipper and lookahead limiter, booth copy,
    /// headphone mix (the cue bus is delayed by the limiter's latency so it
    /// stays in time with the master).
    pub(crate) fn master(
        &mut self,
        master: (&mut [f32], &mut [f32]),
        cue: (&mut [f32], &mut [f32]),
        booth: (&mut [f32], &mut [f32]),
    ) {
        let (ml, mr) = master;
        for (l, r) in ml.iter_mut().zip(mr.iter_mut()) {
            let g = self.master_gain.next_value();
            *l *= g;
            *r *= g;
        }
        self.limiter.process(ml, mr, self.params.limiter);
        let k = self.meter_k;
        let (bl, br) = booth;
        let (cl, cr) = cue;
        for ((((l, r), bl), br), (cl, cr)) in ml
            .iter()
            .zip(mr.iter())
            .zip(bl.iter_mut())
            .zip(br.iter_mut())
            .zip(cl.iter_mut().zip(cr.iter_mut()))
        {
            self.master_meter.feed(*l, *r, k);
            let b = self.booth_gain.next_value();
            *bl = l * b;
            *br = r * b;
            let (ql, qr) = self.cue_delay.push(*cl, *cr);
            let mix = self.cue_mix.next_value();
            let cg = self.cue_gain.next_value();
            *cl = (ql * (1.0 - mix) + l * mix) * cg;
            *cr = (qr * (1.0 - mix) + r * mix) * cg;
        }
        // Headphones and booth: soft clip and brickwall at -0.3 dBFS, as the
        // master (always on: they are not covered by the master's switch).
        self.cue_limiter.process(cl, cr, true);
        self.booth_limiter.process(bl, br, true);
        for (l, r) in cl.iter().zip(cr.iter()) {
            self.cue_meter.feed(*l, *r, k);
        }
        let n = ml.len().min(mr.len());
        let hold = self.params.meter_hold_s;
        self.master_meter.end_block(n, self.sample_rate, hold);
        self.cue_meter.end_block(n, self.sample_rate, hold);
    }

    /// Meters for the snapshot: channels, master, cue, limiter gain.
    pub(crate) fn meters(&self) -> ([Meter; NUM_DECKS], Meter, Meter, f32) {
        (
            std::array::from_fn(|i| self.strips[i].meter.read()),
            self.master_meter.read(),
            self.cue_meter.read(),
            self.limiter.gain,
        )
    }

    /// Resets the peak part of all meters (after a snapshot was delivered).
    pub(crate) fn clear_peaks(&mut self) {
        for s in &mut self.strips {
            s.meter.peak = [0.0; 2];
        }
        self.master_meter.peak = [0.0; 2];
        self.cue_meter.peak = [0.0; 2];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(m: &mut MeterState, v: f32, blocks: usize) {
        for _ in 0..blocks {
            m.feed(v, v, 0.0);
            m.end_block(256, 48_000, DEFAULT_HOLD_SECONDS);
        }
    }

    #[test]
    fn bar_falls_and_the_peak_line_lands_on_it() {
        let mut m = MeterState::default();
        run(&mut m, 0.5, 1);
        assert_eq!(m.read().level[0], 0.5);
        assert_eq!(m.read().hold[0], 0.5);
        // 0.5 s of silence: the bar fell 10 dB, the line is still held.
        run(&mut m, 0.0, 94);
        let r = m.read();
        assert!((r.level[0] / 0.5 - 0.316).abs() < 0.02, "{}", r.level[0]);
        assert_eq!(r.hold[0], 0.5);
        // A steady quieter signal: the bar sits on it, and after the hold
        // the line falls back onto the bar.
        run(&mut m, 0.1, 400);
        let r = m.read();
        assert!((r.level[0] - 0.1).abs() < 1e-6);
        assert!((r.hold[0] - 0.1).abs() < 1e-6, "{}", r.hold[0]);
        // A new peak lifts both at once.
        run(&mut m, 0.9, 1);
        assert_eq!(m.read().level[0], 0.9);
        assert_eq!(m.read().hold[0], 0.9);
    }
}

#[cfg(test)]
mod hold_tests {
    use super::*;

    /// The peak line stays up for the hold time set in the settings.
    #[test]
    fn peak_line_hold_time_follows_the_setting() {
        for (hold, blocks_up) in [(0.0f32, 0usize), (2.0, 370)] {
            let mut m = MeterState::default();
            m.feed(0.5, 0.5, 0.0);
            m.end_block(256, 48_000, hold);
            // 1.9 s (356 blocks of 256 frames) of silence.
            for _ in 0..356 {
                m.feed(0.0, 0.0, 0.0);
                m.end_block(256, 48_000, hold);
            }
            let h = m.read().hold[0];
            if blocks_up > 0 {
                assert_eq!(h, 0.5, "held for {hold} s");
            } else {
                assert!(h < 0.5, "no hold: {h}");
            }
        }
    }
}
