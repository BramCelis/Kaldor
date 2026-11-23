//! The two FX units. Each unit has three effect slots (multi mode: three
//! effects in series, one depth knob each; single mode: the first slot's
//! effect with all three parameters), a beats setting, a wet/dry mix and an
//! assignment to decks 1-4 and the master.
//!
//! Every assigned target (deck or master) has its own effect instances, so
//! two decks on one unit never share an echo. Deck FX sit after the channel
//! fader and crossfader: echo and reverb tails keep ringing when the fader
//! closes or the unit is switched off. Master FX sit before the master gain
//! and the limiter.
//!
//! All instances are allocated in [`Fx::new`]; processing never allocates.

use dj_core::NUM_DECKS;
use dj_dsp::fx::{Effect, EffectKind, FxContext};

pub const FX_UNITS: usize = 2;
pub const FX_SLOTS: usize = 3;
/// Decks 1-4 and the master.
pub const FX_TARGETS: usize = NUM_DECKS + 1;
/// Index of the master in [`UnitParams::assign`].
pub const MASTER_TARGET: usize = NUM_DECKS;

/// The beats settings, shortest first.
pub const BEATS: [f64; 11] = [
    1.0 / 16.0,
    1.0 / 8.0,
    1.0 / 4.0,
    1.0 / 2.0,
    3.0 / 4.0,
    1.0,
    2.0,
    4.0,
    8.0,
    16.0,
    32.0,
];
/// Index of 1 beat in [`BEATS`].
pub const BEATS_ONE: usize = 5;

/// Label of a beats setting ("1/4", "3/4", "8").
pub fn beats_label(index: usize) -> &'static str {
    const LABELS: [&str; 11] = [
        "1/16", "1/8", "1/4", "1/2", "3/4", "1", "2", "4", "8", "16", "32",
    ];
    LABELS[index.min(LABELS.len() - 1)]
}

/// Tempo the FX run at when nothing gives one (no grid, nothing tapped).
pub const FALLBACK_BPM: f64 = 120.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FxMode {
    /// Three effects, one depth knob each.
    #[default]
    Multi,
    /// One effect (slot 1) with all its parameters.
    Single,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotParams {
    pub kind: EffectKind,
    pub on: bool,
    /// 0 to 1. In multi mode only the first is used.
    pub params: [f32; 3],
}

impl SlotParams {
    pub fn new(kind: EffectKind) -> Self {
        Self {
            kind,
            on: false,
            params: kind.defaults(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnitParams {
    pub on: bool,
    pub mode: FxMode,
    /// Decks 1-4, then the master.
    pub assign: [bool; FX_TARGETS],
    /// Index into [`BEATS`].
    pub beats: usize,
    /// Wet/dry, 0 (dry) to 1 (only the effect): 0.5 is the track and the
    /// effects both at full level (see [`wet_level`], [`track_level`]).
    pub mix: f32,
    pub slots: [SlotParams; FX_SLOTS],
}

impl UnitParams {
    /// Slots that play in the current mode.
    pub fn slots_in_use(&self) -> usize {
        match self.mode {
            FxMode::Multi => FX_SLOTS,
            FxMode::Single => 1,
        }
    }
}

/// All FX settings; part of every engine snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxParams {
    pub units: [UnitParams; FX_UNITS],
    /// Tapped tempo (replaces the decks' tempo for all FX), if any.
    pub tap_bpm: Option<f64>,
}

impl Default for FxParams {
    fn default() -> Self {
        // Unit 1 on the left decks (1 and 3), unit 2 on the right (2 and 4),
        // like the controller's two FX sides.
        let unit = |left: bool, kinds: [EffectKind; FX_SLOTS]| UnitParams {
            on: false,
            mode: FxMode::Multi,
            assign: std::array::from_fn(|t| t < NUM_DECKS && (t % 2 == 0) == left),
            beats: BEATS_ONE,
            mix: 0.5,
            slots: kinds.map(SlotParams::new),
        };
        Self {
            units: [
                unit(
                    true,
                    [EffectKind::Echo, EffectKind::Reverb, EffectKind::Flanger],
                ),
                unit(
                    false,
                    [EffectKind::PingPong, EffectKind::Filter, EffectKind::Gate],
                ),
            ],
            tap_bpm: None,
        }
    }
}

impl FxParams {
    /// The commands that bring fresh FX units to these settings (used to
    /// carry them over when the audio device changes). Units start off.
    pub fn commands(&self) -> Vec<FxCommand> {
        let mut out = Vec::new();
        for (u, p) in self.units.iter().enumerate() {
            out.push(FxCommand::Mode(u, p.mode));
            for (t, a) in p.assign.iter().enumerate() {
                out.push(FxCommand::Assign(u, t, *a));
            }
            out.push(FxCommand::Beats(u, p.beats));
            out.push(FxCommand::Mix(u, p.mix));
            for (s, slot) in p.slots.iter().enumerate() {
                out.push(FxCommand::SlotKind(u, s, slot.kind));
                for (i, v) in slot.params.iter().enumerate() {
                    out.push(FxCommand::SlotParam(u, s, i, *v));
                }
                out.push(FxCommand::SlotOn(u, s, slot.on));
            }
        }
        out.push(FxCommand::Tap(self.tap_bpm));
        out
    }
}

/// FX commands. Unit, slot, target and parameter indices out of range are
/// ignored.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FxCommand {
    UnitOn(usize, bool),
    ToggleUnit(usize),
    Mode(usize, FxMode),
    /// Unit, target (deck index or [`MASTER_TARGET`]), on.
    Assign(usize, usize, bool),
    ToggleAssign(usize, usize),
    /// Unit, index into [`BEATS`].
    Beats(usize, usize),
    /// Unit, steps (+1 = longer).
    BeatsStep(usize, i32),
    /// Unit, wet/dry 0 to 1.
    Mix(usize, f32),
    SlotKind(usize, usize, EffectKind),
    /// Unit, slot, steps through [`EffectKind::ALL`].
    SlotKindStep(usize, usize, i32),
    SlotOn(usize, usize, bool),
    ToggleSlot(usize, usize),
    /// Unit, slot, parameter (0-2), value 0 to 1.
    SlotParam(usize, usize, usize, f32),
    /// Unit, change of the depth (first parameter) of every slot that is on.
    DepthStep(usize, f32),
    /// Tapped tempo for all FX (`None`: follow the decks again). A tap also
    /// puts the FX beat at the downbeat.
    Tap(Option<f64>),
}

/// What the UI needs to see of the FX.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FxSnapshot {
    pub params: FxParams,
    /// Per unit and slot: the effect is sounding somewhere (on, or a tail).
    pub sounding: [[bool; FX_SLOTS]; FX_UNITS],
    /// Tempo each target's FX run at.
    pub bpm: [f32; FX_TARGETS],
}

impl Default for FxSnapshot {
    fn default() -> Self {
        Self {
            params: FxParams::default(),
            sounding: [[false; FX_SLOTS]; FX_UNITS],
            bpm: [FALLBACK_BPM as f32; FX_TARGETS],
        }
    }
}

/// A target's beat clock: follows the deck's grid when it has one, runs on
/// by itself otherwise.
#[derive(Clone, Copy, Debug)]
struct Clock {
    bpm: f64,
    bar_beat: f64,
}

struct Instance {
    fx: Effect,
    running: bool,
    gain: f32,
    /// How much of the effect is heard (from its Depth), smoothed.
    wet: f32,
    /// Frames the tail has been silent.
    silent: u32,
    /// How much of the track a tail effect lets through (see `FxContext`).
    pass: f32,
}

struct Unit {
    /// Per target (decks, master). On the heap: as an array, the 15
    /// instances were built on the stack, which overflowed the main thread
    /// of debug builds.
    inst: Vec<[Instance; FX_SLOTS]>,
    mix_from: f32,
    mix_to: f32,
}

/// Peak difference below which a tail counts as silent.
const SILENT: f32 = 1e-5;

/// Effect level of the WET/DRY knob `m` (0 to 1): rises to full at the
/// middle and stays there.
pub fn wet_level(m: f32) -> f32 {
    (2.0 * m).clamp(0.0, 1.0)
}

/// Track level of the tail effects at WET/DRY `m`: full up to the middle,
/// then down to nothing at fully wet (only the effect).
pub fn track_level(m: f32) -> f32 {
    (2.0 * (1.0 - m)).clamp(0.0, 1.0)
}

pub(crate) struct Fx {
    sample_rate: u32,
    params: FxParams,
    units: [Unit; FX_UNITS],
    clocks: [Clock; FX_TARGETS],
    /// Beat of the tap clock (when a tempo is tapped).
    tap_beat: f64,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
    in_l: Vec<f32>,
    in_r: Vec<f32>,
    /// Fade time of insert effects, in frames.
    fade: f32,
}

fn slot(units: &mut [UnitParams; FX_UNITS], u: usize, s: usize) -> Option<&mut SlotParams> {
    units.get_mut(u).and_then(|p| p.slots.get_mut(s))
}

impl Fx {
    pub(crate) fn new(sample_rate: u32, max_block: usize) -> Self {
        let params = FxParams::default();
        let units = std::array::from_fn(|u| Unit {
            inst: (0..FX_TARGETS)
                .map(|_| {
                    std::array::from_fn(|s| {
                        let mut fx = Effect::new(sample_rate);
                        fx.set_kind(params.units[u].slots[s].kind);
                        Instance {
                            fx,
                            running: false,
                            gain: 0.0,
                            wet: 0.0,
                            silent: 0,
                            pass: 1.0,
                        }
                    })
                })
                .collect(),
            mix_from: params.units[u].mix,
            mix_to: params.units[u].mix,
        });
        Self {
            sample_rate,
            params,
            units,
            clocks: [Clock {
                bpm: FALLBACK_BPM,
                bar_beat: 0.0,
            }; FX_TARGETS],
            tap_beat: 0.0,
            dry_l: vec![0.0; max_block],
            dry_r: vec![0.0; max_block],
            in_l: vec![0.0; max_block],
            in_r: vec![0.0; max_block],
            fade: 0.01 * sample_rate as f32,
        }
    }

    pub(crate) fn command(&mut self, cmd: FxCommand) {
        let units = &mut self.params.units;
        match cmd {
            FxCommand::UnitOn(u, on) => {
                if let Some(p) = units.get_mut(u) {
                    p.on = on;
                }
            }
            FxCommand::ToggleUnit(u) => {
                if let Some(p) = units.get_mut(u) {
                    p.on = !p.on;
                }
            }
            FxCommand::Mode(u, m) => {
                if let Some(p) = units.get_mut(u) {
                    p.mode = m;
                }
            }
            FxCommand::Assign(u, t, on) => {
                if let Some(a) = units.get_mut(u).and_then(|p| p.assign.get_mut(t)) {
                    *a = on;
                }
            }
            FxCommand::ToggleAssign(u, t) => {
                if let Some(a) = units.get_mut(u).and_then(|p| p.assign.get_mut(t)) {
                    *a = !*a;
                }
            }
            FxCommand::Beats(u, i) => {
                if let Some(p) = units.get_mut(u) {
                    p.beats = i.min(BEATS.len() - 1);
                }
            }
            FxCommand::BeatsStep(u, d) => {
                if let Some(p) = units.get_mut(u) {
                    p.beats =
                        (p.beats as i64 + i64::from(d)).clamp(0, BEATS.len() as i64 - 1) as usize;
                }
            }
            FxCommand::Mix(u, m) => {
                if let Some(p) = units.get_mut(u) {
                    p.mix = m.clamp(0.0, 1.0);
                }
            }
            FxCommand::SlotKind(u, s, k) => {
                if let Some(sp) = slot(units, u, s)
                    && sp.kind != k
                {
                    *sp = SlotParams {
                        on: sp.on,
                        ..SlotParams::new(k)
                    };
                }
            }
            FxCommand::SlotKindStep(u, s, d) => {
                if let Some(sp) = slot(units, u, s) {
                    let n = EffectKind::ALL.len() as i64;
                    let i = EffectKind::ALL
                        .iter()
                        .position(|k| *k == sp.kind)
                        .unwrap_or(0) as i64;
                    let k = EffectKind::ALL[(i + i64::from(d)).rem_euclid(n) as usize];
                    *sp = SlotParams {
                        on: sp.on,
                        ..SlotParams::new(k)
                    };
                }
            }
            FxCommand::SlotOn(u, s, on) => {
                if let Some(sp) = slot(units, u, s) {
                    sp.on = on;
                }
            }
            FxCommand::ToggleSlot(u, s) => {
                if let Some(sp) = slot(units, u, s) {
                    sp.on = !sp.on;
                }
            }
            FxCommand::SlotParam(u, s, i, v) => {
                if let Some(x) = slot(units, u, s).and_then(|sp| sp.params.get_mut(i)) {
                    *x = v.clamp(0.0, 1.0);
                }
            }
            FxCommand::DepthStep(u, d) => {
                if let Some(p) = units.get_mut(u) {
                    let n = p.slots_in_use();
                    for sp in p.slots.iter_mut().take(n).filter(|s| s.on) {
                        sp.params[0] = (sp.params[0] + d).clamp(0.0, 1.0);
                    }
                }
            }
            FxCommand::Tap(bpm) => {
                self.params.tap_bpm = bpm.filter(|b| b.is_finite() && *b > 20.0 && *b < 400.0);
                self.tap_beat = 0.0;
            }
        }
    }

    /// Before the targets of a block: wet/dry ramps and the tap clock.
    pub(crate) fn begin_block(&mut self, n: usize) {
        for (unit, p) in self.units.iter_mut().zip(self.params.units.iter()) {
            unit.mix_from = unit.mix_to;
            unit.mix_to = p.mix;
        }
        if let Some(bpm) = self.params.tap_bpm {
            self.tap_beat += n as f64 * bpm / (60.0 * f64::from(self.sample_rate));
        }
    }

    /// Processes target `t` (deck index or [`MASTER_TARGET`]) in place.
    /// `beat` is the target's (BPM, bar-aligned beat) when it has a grid;
    /// `fallback_bpm` is used when it has none.
    pub(crate) fn process(
        &mut self,
        t: usize,
        beat: Option<(f64, f64)>,
        fallback_bpm: f64,
        l: &mut [f32],
        r: &mut [f32],
    ) {
        let n = l.len().min(r.len()).min(self.dry_l.len());
        let (Some(l), Some(r)) = (l.get_mut(..n), r.get_mut(..n)) else {
            return;
        };
        let sr = f64::from(self.sample_rate);
        // The clock: tapped tempo, else the deck's grid, else free-running.
        let tap = self
            .params
            .tap_bpm
            .map(|b| (b, self.tap_beat - n as f64 * b / (60.0 * sr)));
        let Some(clock) = self.clocks.get_mut(t) else {
            return;
        };
        match tap.or(beat) {
            Some((bpm, bar_beat)) => {
                clock.bpm = bpm.clamp(20.0, 400.0);
                clock.bar_beat = bar_beat;
            }
            None => clock.bpm = fallback_bpm.clamp(20.0, 400.0),
        }
        let beat_frames = 60.0 * sr / clock.bpm;
        let bar_beat = clock.bar_beat;
        if beat.is_none() || tap.is_some() {
            clock.bar_beat += n as f64 / beat_frames;
        }

        for (u, unit) in self.units.iter_mut().enumerate() {
            let p = self.params.units[u];
            let unit_active = p.on && p.assign.get(t).copied().unwrap_or(false);
            let used = p.slots_in_use();
            let Some(inst) = unit.inst.get_mut(t) else {
                continue;
            };
            let any = inst
                .iter()
                .enumerate()
                .any(|(s, i)| i.running || (unit_active && s < used && p.slots[s].on));
            if !any {
                // Effects that loop or capture keep a short history.
                if unit_active {
                    for (s, i) in inst.iter_mut().enumerate().take(used) {
                        let k = p.slots[s].kind;
                        if k.needs_history() {
                            if i.fx.kind() != k {
                                i.fx.set_kind(k);
                            }
                            i.fx.warm(l, r);
                        }
                    }
                }
                continue;
            }
            let (Some(dl), Some(dr), Some(il), Some(ir)) = (
                self.dry_l.get_mut(..n),
                self.dry_r.get_mut(..n),
                self.in_l.get_mut(..n),
                self.in_r.get_mut(..n),
            ) else {
                return;
            };
            dl.copy_from_slice(l);
            dr.copy_from_slice(r);
            for (s, i) in inst.iter_mut().enumerate() {
                let sp = p.slots[s];
                let active = unit_active && s < used && sp.on;
                if i.fx.kind() != sp.kind {
                    i.fx.set_kind(sp.kind);
                    i.running = false;
                    i.gain = 0.0;
                }
                // Multi mode: only the Depth knob; the others at their defaults.
                let knobs = match p.mode {
                    FxMode::Single => sp.params,
                    FxMode::Multi => {
                        let mut d = sp.kind.defaults();
                        d[0] = sp.params[0];
                        d
                    }
                };
                let (params, wet) = sp.kind.internal(knobs);
                // Past the middle of WET/DRY a tail effect that is on lets
                // less of the track through (fully wet: only its tail); off,
                // the track comes back while the tail rings out.
                let pass_target = if active { track_level(p.mix) } else { 1.0 };
                let ctx = FxContext {
                    beat_frames,
                    beats: BEATS[p.beats.min(BEATS.len() - 1)],
                    bar_beat,
                    params,
                    feed: active,
                    pass_from: i.pass,
                    pass_to: pass_target,
                };
                if !i.running {
                    if !active {
                        if unit_active && s < used && sp.kind.needs_history() {
                            i.fx.warm(l, r);
                        }
                        continue;
                    }
                    i.fx.start(&ctx);
                    i.running = true;
                    i.gain = 0.0;
                    i.wet = wet;
                    i.silent = 0;
                }
                il.copy_from_slice(l);
                ir.copy_from_slice(r);
                i.fx.process(&ctx, l, r);
                i.pass = pass_target;
                let tail = sp.kind.has_tail();
                let target = if active || tail { 1.0 } else { 0.0 };
                let step = 1.0 / self.fade;
                let mut peak = 0.0f32;
                for (((xl, xr), a), b) in
                    l.iter_mut().zip(r.iter_mut()).zip(il.iter()).zip(ir.iter())
                {
                    if i.gain < target {
                        i.gain = (i.gain + step).min(target);
                    } else if i.gain > target {
                        i.gain = (i.gain - step).max(target);
                    }
                    if i.wet < wet {
                        i.wet = (i.wet + step).min(wet);
                    } else if i.wet > wet {
                        i.wet = (i.wet - step).max(wet);
                    }
                    let wl = *xl - a;
                    let wr = *xr - b;
                    peak = peak.max(wl.abs()).max(wr.abs());
                    let g = i.gain * i.wet;
                    *xl = a + g * wl;
                    *xr = b + g * wr;
                }
                if !active {
                    if tail {
                        if peak < SILENT {
                            i.silent = i.silent.saturating_add(n as u32);
                        } else {
                            i.silent = 0;
                        }
                        if i.silent > self.sample_rate / 2 {
                            i.running = false;
                        }
                    } else if i.gain <= 0.0 {
                        i.running = false;
                    }
                }
            }
            // Wet/dry: up to the middle the effects fade in; past it the
            // tail effects take the track out (see `pass`).
            let (from, to) = (unit.mix_from, unit.mix_to);
            let k = 1.0 / n.max(1) as f32;
            for (idx, ((xl, xr), (a, b))) in l
                .iter_mut()
                .zip(r.iter_mut())
                .zip(dl.iter().zip(dr.iter()))
                .enumerate()
            {
                let m = wet_level(from + (to - from) * (idx as f32 + 1.0) * k);
                *xl = a + m * (*xl - a);
                *xr = b + m * (*xr - b);
            }
        }
    }

    pub(crate) fn snapshot(&self) -> FxSnapshot {
        let mut sounding = [[false; FX_SLOTS]; FX_UNITS];
        for (u, unit) in self.units.iter().enumerate() {
            for target in &unit.inst {
                for (s, i) in target.iter().enumerate() {
                    sounding[u][s] |= i.running;
                }
            }
        }
        FxSnapshot {
            params: self.params,
            sounding,
            bpm: std::array::from_fn(|t| self.clocks[t].bpm as f32),
        }
    }
}
