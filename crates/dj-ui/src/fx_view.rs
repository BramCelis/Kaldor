//! The FX panel under the waveforms (in place of the mixer), the FX setup
//! in the config, presets and tap tempo.
//!
//! Each unit: ON, multi / single mode, decks 1-4 and master (M), beats,
//! TAP, presets, wet/dry, and its effects: three with one depth knob each
//! (multi) or one with all three parameters (single).

use crate::app::{AppEvent, KaldorApp, region_ui};
use crate::theme::{self, Palette};
use crate::uimap::Region;
use crate::widgets::{self, flat_button};
use dj_audio::{
    BEATS, Command, EffectKind, FX_SLOTS, FX_TARGETS, FX_UNITS, FxCommand, FxMode, FxParams,
    FxSnapshot, MASTER_TARGET, UnitParams, beats_label,
};
use dj_core::NUM_DECKS;
use dj_core::config::{FxPreset, FxSlotConfig, FxUnitConfig};
use egui::{Align, Id, Layout, Rect, Sense, Stroke, pos2, vec2};
use std::time::{Duration, Instant};

/// Size the panel needs.
pub const FX_MIN: egui::Vec2 = egui::vec2(804.0, 218.0);
const ROW_H: f32 = 24.0;
const GAP: f32 = 4.0;

/// Tap tempo from button presses (seconds).
#[derive(Default)]
pub struct TapTempo {
    taps: Vec<f64>,
}

impl TapTempo {
    /// A tap at `t` seconds. Returns the tempo once there are two taps
    /// less than 2 s apart (the last 8 taps count).
    pub fn tap(&mut self, t: f64) -> Option<f64> {
        if self.taps.last().is_none_or(|l| t - l > 2.0 || t < *l) {
            self.taps.clear();
        }
        self.taps.push(t);
        if self.taps.len() > 8 {
            self.taps.remove(0);
        }
        dj_core::beatgrid::tap_tempo(&self.taps).filter(|b| (30.0..=300.0).contains(b))
    }
}

/// UI state of the FX.
pub struct FxUi {
    pub(crate) tap: TapTempo,
    started: Instant,
    /// FX settings were just sent to a new engine: the snapshot is not
    /// trusted until then (instant sent, frames rendered then).
    restoring: Option<(Instant, u64)>,
    /// UI frames since the config was last compared.
    since_check: u32,
    /// Name typed for a new preset, per unit.
    pub(crate) preset_name: [String; FX_UNITS],
}

impl Default for FxUi {
    fn default() -> Self {
        Self {
            tap: TapTempo::default(),
            started: Instant::now(),
            restoring: Some((Instant::now(), 0)),
            since_check: 0,
            preset_name: Default::default(),
        }
    }
}

impl FxUi {
    /// Seconds since the UI started (for tap tempo).
    pub(crate) fn now(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

fn mode_name(m: FxMode) -> &'static str {
    match m {
        FxMode::Multi => "multi",
        FxMode::Single => "single",
    }
}

/// A unit's setup for the config (whether it is on is not saved).
pub fn unit_config(p: &UnitParams) -> FxUnitConfig {
    FxUnitConfig {
        mode: mode_name(p.mode).into(),
        assign: p.assign.to_vec(),
        beats: BEATS[p.beats.min(BEATS.len() - 1)],
        mix: p.mix,
        slots: p
            .slots
            .iter()
            .map(|s| FxSlotConfig {
                effect: s.kind.id().into(),
                params: s.params.to_vec(),
            })
            .collect(),
    }
}

/// The commands that set unit `u` up as `c` (unknown effects and missing
/// entries keep what the unit has).
pub fn unit_commands(u: usize, c: &FxUnitConfig) -> Vec<FxCommand> {
    let mut out = Vec::new();
    out.push(FxCommand::Mode(
        u,
        if c.mode == "single" {
            FxMode::Single
        } else {
            FxMode::Multi
        },
    ));
    for (t, a) in c.assign.iter().take(FX_TARGETS).enumerate() {
        out.push(FxCommand::Assign(u, t, *a));
    }
    // The nearest beats setting.
    let beats = BEATS
        .iter()
        .enumerate()
        .min_by(|a, b| {
            (a.1.ln() - c.beats.max(1e-3).ln())
                .abs()
                .total_cmp(&(b.1.ln() - c.beats.max(1e-3).ln()).abs())
        })
        .map_or(dj_audio::fx::BEATS_ONE, |(i, _)| i);
    out.push(FxCommand::Beats(u, beats));
    out.push(FxCommand::Mix(u, c.mix));
    for (s, slot) in c.slots.iter().take(FX_SLOTS).enumerate() {
        let Some(kind) = EffectKind::from_id(&slot.effect) else {
            continue;
        };
        out.push(FxCommand::SlotKind(u, s, kind));
        for (i, v) in slot.params.iter().take(3).enumerate() {
            out.push(FxCommand::SlotParam(u, s, i, *v));
        }
    }
    out
}

impl KaldorApp {
    /// Sends the FX setup from the config to the engine (at start).
    pub(crate) fn send_fx_config(&mut self) {
        if self.config.fx.version < dj_core::config::FxConfig::VERSION && self.config.fx.migrate() {
            self.events.push(AppEvent::ConfigChanged);
        }
        let units = self.config.fx.units.clone();
        for (u, c) in units.iter().take(FX_UNITS).enumerate() {
            for cmd in unit_commands(u, c) {
                self.send(Command::Fx(cmd));
            }
        }
        self.fx.restoring = Some((Instant::now(), self.engine.snapshot().frames_rendered));
    }

    /// Carries the FX over to a new engine (audio output changed), units
    /// on or off as they were.
    pub(crate) fn restore_fx(&mut self, p: &FxParams) {
        for cmd in p.commands() {
            self.send(Command::Fx(cmd));
        }
        for (u, unit) in p.units.iter().enumerate() {
            self.send(Command::Fx(FxCommand::UnitOn(u, unit.on)));
        }
        self.fx.restoring = Some((Instant::now(), self.engine.snapshot().frames_rendered));
    }

    /// Keeps the config's FX setup in step with the engine (changes from the
    /// screen, the keyboard or the controller), checked about twice a second.
    pub(crate) fn sync_fx_config(&mut self) {
        let snap = self.engine.snapshot();
        if let Some((at, frames)) = self.fx.restoring {
            // Wait until the engine has run a while with the sent settings.
            if at.elapsed() < Duration::from_millis(300) || snap.frames_rendered <= frames + 2048 {
                return;
            }
            self.fx.restoring = None;
        }
        self.fx.since_check += 1;
        if self.fx.since_check < 30 {
            return;
        }
        self.fx.since_check = 0;
        let now: Vec<FxUnitConfig> = snap.fx.params.units.iter().map(unit_config).collect();
        if now != self.config.fx.units {
            self.config.fx.units = now;
            self.events.push(AppEvent::ConfigChanged);
        }
    }

    /// A tap of the FX TAP button (screen, keyboard).
    pub(crate) fn fx_tap(&mut self) {
        let t = self.fx.now();
        if let Some(bpm) = self.fx.tap.tap(t) {
            self.send(Command::Fx(FxCommand::Tap(Some(bpm))));
        }
    }

    fn fx(&mut self, c: FxCommand) {
        self.send(Command::Fx(c));
    }
}

/// The panel in `rect` (scrolls when the area is smaller than it needs).
pub(crate) fn fx_area(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    app.map.enter(Region::Mixer, rect);
    let pal = Palette::get(app.config.ui.dark_theme);
    let fits = rect.width() >= FX_MIN.x && rect.height() >= FX_MIN.y;
    let panel = if fits {
        Rect::from_min_size(rect.min, vec2(rect.width(), FX_MIN.y))
    } else {
        rect
    };
    ui.painter().rect(
        panel,
        6.0,
        pal.panel,
        Stroke::new(1.0, pal.border),
        egui::StrokeKind::Inside,
    );
    let mut ui = region_ui(ui, rect, Layout::top_down(Align::Min), "fx");
    if fits {
        fx_panel(app, &mut ui, panel);
    } else {
        egui::ScrollArea::both()
            .id_salt("fx_scroll")
            .auto_shrink([false, false])
            .show(&mut ui, |ui| {
                app.map.scrolled = true;
                let (r, _) = ui.allocate_exact_size(FX_MIN, Sense::hover());
                fx_panel(app, ui, r);
                app.map.scrolled = false;
            });
    }
}

fn fx_panel(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect) {
    let inner = rect.shrink(6.0);
    let half_w = ((inner.width() - 12.0) / 2.0).floor();
    let snap = app.engine.snapshot().fx;
    for u in 0..FX_UNITS {
        let x = if u == 0 {
            inner.left()
        } else {
            inner.right() - half_w
        };
        let r = Rect::from_min_size(pos2(x, inner.top()), vec2(half_w, inner.height()));
        unit_panel(app, ui, u, r, &snap);
    }
    // Divider.
    let pal = Palette::get(app.config.ui.dark_theme);
    let x = inner.center().x;
    ui.painter().line_segment(
        [pos2(x, inner.top() + 4.0), pos2(x, inner.bottom() - 4.0)],
        Stroke::new(1.0, pal.border),
    );
}

/// Index for the UI map: unit * 10 + item.
fn idx(u: usize, i: usize) -> Option<usize> {
    Some(u * 10 + i)
}

fn unit_panel(app: &mut KaldorApp, ui: &mut egui::Ui, u: usize, rect: Rect, snap: &FxSnapshot) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let p = snap.params.units[u];
    let font = theme::prop(12.0);
    let small = theme::prop(10.5);

    // Row 1: name, ON, mode, targets, presets.
    let row1 = Rect::from_min_size(rect.min, vec2(rect.width(), ROW_H));
    let mut x = row1.left();
    let mut cell = |w: f32| {
        let r = Rect::from_min_size(pos2(x, row1.top()), vec2(w, ROW_H));
        x += w + GAP;
        r
    };
    let name_r = cell(34.0);
    widgets::text_in(
        ui.painter(),
        name_r,
        &format!("FX {}", u + 1),
        theme::prop(13.0),
        pal.text,
        true,
    );
    let on_r = cell(44.0);
    if flat_button(
        ui,
        on_r,
        Id::unique(("fx_on", u)),
        "ON",
        font.clone(),
        None,
        p.on,
        pal.accent,
        false,
    )
    .on_hover_text(format!(
        "FX {} on or off (controller: FX switch {}; keys: Shift+{})",
        u + 1,
        u + 1,
        if u == 0 { "Z" } else { "C" }
    ))
    .clicked()
    {
        app.fx(FxCommand::ToggleUnit(u));
    }
    app.map.add(ui, "fx_on", idx(u, 0), on_r);
    let mode_r = cell(54.0);
    let single = p.mode == FxMode::Single;
    if flat_button(
        ui,
        mode_r,
        Id::unique(("fx_mode", u)),
        if single { "SINGLE" } else { "MULTI" },
        small.clone(),
        None,
        false,
        pal.accent,
        false,
    )
    .on_hover_text(
        "Multi: three effects, one depth knob each. Single: one effect with all its knobs",
    )
    .clicked()
    {
        app.fx(FxCommand::Mode(
            u,
            if single {
                FxMode::Multi
            } else {
                FxMode::Single
            },
        ));
    }
    app.map.add(ui, "fx_mode", idx(u, 0), mode_r);
    for t in 0..FX_TARGETS {
        let r = cell(22.0);
        let label = if t == MASTER_TARGET {
            "M".to_string()
        } else {
            (t + 1).to_string()
        };
        let color = if t < NUM_DECKS {
            theme::deck_color(dj_core::DeckId::ALL[t])
        } else {
            pal.accent
        };
        let hover = if t == MASTER_TARGET {
            format!("FX {} on the master", u + 1)
        } else {
            format!("FX {} on deck {}", u + 1, t + 1)
        };
        if flat_button(
            ui,
            r,
            Id::unique(("fx_assign", u, t)),
            &label,
            small.clone(),
            None,
            p.assign[t],
            color,
            false,
        )
        .on_hover_text(hover)
        .clicked()
        {
            app.fx(FxCommand::ToggleAssign(u, t));
        }
        app.map.add(ui, "fx_assign", idx(u, t), r);
    }
    let preset_r = Rect::from_min_max(pos2(x, row1.top()), row1.max);
    if preset_r.width() > 60.0 {
        presets(app, ui, u, preset_r, &p);
    }

    // Row 2: beats, TAP, tempo.
    let row2 = Rect::from_min_size(
        pos2(rect.left(), row1.bottom() + GAP),
        vec2(rect.width(), ROW_H),
    );
    let mut x = row2.left();
    let mut cell = |w: f32| {
        let r = Rect::from_min_size(pos2(x, row2.top()), vec2(w, ROW_H));
        x += w + GAP;
        r
    };
    let label_r = cell(40.0);
    widgets::text_in(
        ui.painter(),
        label_r,
        "BEATS",
        small.clone(),
        pal.text_weak,
        false,
    );
    let minus = cell(22.0);
    if flat_button(
        ui,
        minus,
        Id::unique(("fx_beats_down", u)),
        "<",
        font.clone(),
        None,
        false,
        pal.accent,
        false,
    )
    .on_hover_text("Shorter (controller: BEATS knob; keys: Home)")
    .clicked()
    {
        app.fx(FxCommand::BeatsStep(u, -1));
    }
    app.map.add(ui, "fx_beats_down", idx(u, 0), minus);
    let val = cell(38.0);
    widgets::text_in(
        ui.painter(),
        val,
        beats_label(p.beats),
        theme::mono(13.0),
        pal.text,
        true,
    );
    app.map.add(ui, "fx_beats", idx(u, 0), val);
    let plus = cell(22.0);
    if flat_button(
        ui,
        plus,
        Id::unique(("fx_beats_up", u)),
        ">",
        font.clone(),
        None,
        false,
        pal.accent,
        false,
    )
    .on_hover_text("Longer (controller: BEATS knob; keys: End)")
    .clicked()
    {
        app.fx(FxCommand::BeatsStep(u, 1));
    }
    app.map.add(ui, "fx_beats_up", idx(u, 0), plus);
    let tap_r = cell(40.0);
    let tapped = snap.params.tap_bpm.is_some();
    let tap = flat_button(
        ui,
        tap_r,
        Id::unique(("fx_tap", u)),
        "TAP",
        small.clone(),
        None,
        tapped,
        pal.accent,
        false,
    )
    .on_hover_text("Tap the FX tempo (all FX). Right-click: follow the decks again (controller: SHIFT+TAP; keys: Shift+X)");
    if tap.clicked() {
        app.fx_tap();
    }
    if tap.secondary_clicked() {
        app.fx(FxCommand::Tap(None));
    }
    app.map.add(ui, "fx_tap", idx(u, 0), tap_r);
    let tempo_r = Rect::from_min_max(pos2(x, row2.top()), row2.max);
    let bpm = match snap.params.tap_bpm {
        Some(b) => format!("{b:.1} BPM tapped"),
        None => {
            let t = (0..FX_TARGETS).find(|t| p.assign[*t]);
            match t {
                Some(t) if t == MASTER_TARGET => format!("{:.1} BPM (master)", snap.bpm[t]),
                Some(t) => format!("{:.1} BPM (deck {})", snap.bpm[t], t + 1),
                None => "not on a deck".into(),
            }
        }
    };
    widgets::text_in(
        ui.painter(),
        tempo_r,
        &bpm,
        small.clone(),
        pal.text_weak,
        false,
    );

    // Effects and wet/dry.
    let body = Rect::from_min_max(pos2(rect.left(), row2.bottom() + GAP + 2.0), rect.max);
    let mix_w = 64.0;
    let mix_r = Rect::from_min_max(pos2(body.right() - mix_w, body.top()), body.max);
    let fx_r = Rect::from_min_max(body.min, pos2(mix_r.left() - GAP * 2.0, body.bottom()));
    knob_with_label(
        app,
        ui,
        mix_r,
        ("fx_mix", idx(u, 0)),
        "WET/DRY",
        p.mix,
        0.5,
        &format!(
            "FX {} wet/dry: effects {:.0} %, track {:.0} % (middle: both full; \
             fully right: only the effects; controller: WET/DRY)",
            u + 1,
            dj_audio::fx::wet_level(p.mix) * 100.0,
            dj_audio::fx::track_level(p.mix) * 100.0
        ),
        |v| FxCommand::Mix(u, v),
    );
    match p.mode {
        FxMode::Multi => {
            let w = ((fx_r.width() - GAP * 2.0) / 3.0).floor();
            for s in 0..FX_SLOTS {
                let col = Rect::from_min_size(
                    pos2(fx_r.left() + s as f32 * (w + GAP), fx_r.top()),
                    vec2(w, fx_r.height()),
                );
                slot_column(app, ui, u, s, col, snap, true);
            }
        }
        FxMode::Single => {
            let w = ((fx_r.width() - GAP * 3.0) / 4.0).floor();
            let col = Rect::from_min_size(fx_r.min, vec2(w, fx_r.height()));
            slot_column(app, ui, u, 0, col, snap, false);
            let sp = p.slots[0];
            let names = sp.kind.param_names();
            for (i, name) in names.into_iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(
                        fx_r.left() + (i + 1) as f32 * (w + GAP),
                        fx_r.top() + ROW_H + GAP,
                    ),
                    vec2(w, fx_r.height() - ROW_H - GAP),
                );
                if name.is_empty() {
                    continue;
                }
                knob_with_label(
                    app,
                    ui,
                    r,
                    ("fx_param", idx(u, i)),
                    name,
                    sp.params[i],
                    sp.kind.defaults()[i],
                    &format!("{} {:.0} %", name, sp.params[i] * 100.0),
                    move |v| FxCommand::SlotParam(u, 0, i, v),
                );
            }
        }
    }
}

/// Effect menu, ON button and (multi mode) depth knob of one slot.
fn slot_column(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    u: usize,
    s: usize,
    col: Rect,
    snap: &FxSnapshot,
    depth: bool,
) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let sp = snap.params.units[u].slots[s];
    let menu_r = Rect::from_min_size(col.min, vec2(col.width(), ROW_H));
    let mut mui = region_ui(
        ui,
        menu_r,
        Layout::left_to_right(Align::Center),
        ("fx_menu", u, s),
    );
    let mut picked = None;
    egui::ComboBox::from_id_salt(("fx_kind", u, s))
        .width(menu_r.width() - 8.0)
        .selected_text(sp.kind.name())
        .show_ui(&mut mui, |ui| {
            for k in EffectKind::ALL {
                if ui.selectable_label(k == sp.kind, k.name()).clicked() {
                    picked = Some(k);
                }
            }
        })
        .response
        .on_hover_text("The effect (controller: SHIFT + effect button)");
    if let Some(k) = picked {
        app.fx(FxCommand::SlotKind(u, s, k));
    }
    app.map.add(ui, "fx_kind", idx(u, s), menu_r);
    let on_r = Rect::from_min_size(
        pos2(col.left(), menu_r.bottom() + GAP),
        vec2(col.width(), ROW_H + 2.0),
    );
    let tail = snap.sounding[u][s] && !sp.on;
    if flat_button(
        ui,
        on_r,
        Id::unique(("fx_slot_on", u, s)),
        if tail { "tail..." } else { sp.kind.name() },
        theme::prop(12.0),
        None,
        sp.on,
        pal.accent,
        false,
    )
    .on_hover_text(format!(
        "{} on or off (controller: effect button {}; keys: Shift+{})",
        sp.kind.name(),
        u * 3 + s + 1,
        ["Q", "W", "E", "U", "I", "O"][u * 3 + s]
    ))
    .clicked()
    {
        app.fx(FxCommand::ToggleSlot(u, s));
    }
    app.map.add(ui, "fx_slot_on", idx(u, s), on_r);
    if depth {
        let name = sp.kind.param_names()[0];
        let r = Rect::from_min_max(pos2(col.left(), on_r.bottom() + GAP), col.max);
        knob_with_label(
            app,
            ui,
            r,
            ("fx_depth", idx(u, s)),
            name,
            sp.params[0],
            sp.kind.defaults()[0],
            &format!(
                "{} {:.0} % (controller: SHIFT + BEATS for the effects that are on)",
                name,
                sp.params[0] * 100.0
            ),
            move |v| FxCommand::SlotParam(u, s, 0, v),
        );
    }
}

/// A knob centred in `rect` with its label under it.
#[allow(clippy::too_many_arguments)]
fn knob_with_label(
    app: &mut KaldorApp,
    ui: &mut egui::Ui,
    rect: Rect,
    map: (&'static str, Option<usize>),
    label: &str,
    value: f32,
    default: f32,
    hover: &str,
    cmd: impl Fn(f32) -> FxCommand,
) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let size = (rect.height() - 18.0).min(rect.width()).clamp(16.0, 48.0);
    let kr = Rect::from_center_size(
        pos2(rect.center().x, rect.top() + size / 2.0 + 2.0),
        vec2(size, size),
    );
    let (_, new) = widgets::knob(
        ui,
        kr,
        Id::unique(("fx_knob", map.0, map.1)),
        value,
        0.0..=1.0,
        default,
        false,
        pal.accent,
        hover,
    );
    if let Some(v) = new {
        app.fx(cmd(v));
    }
    app.map.add(ui, map.0, map.1, kr);
    let lr = Rect::from_min_size(
        pos2(rect.left(), kr.bottom() + 2.0),
        vec2(rect.width(), 14.0),
    );
    widgets::text_in(
        ui.painter(),
        lr,
        label,
        theme::prop(10.5),
        pal.text_weak,
        true,
    );
}

/// The preset menu of unit `u`: load, delete, save the current setup.
fn presets(app: &mut KaldorApp, ui: &mut egui::Ui, u: usize, rect: Rect, p: &UnitParams) {
    let mut pui = region_ui(
        ui,
        rect,
        Layout::right_to_left(Align::Center),
        ("fx_presets", u),
    );
    let mut load = None;
    let mut delete = None;
    let mut save = false;
    let names: Vec<String> = app
        .config
        .fx
        .presets
        .iter()
        .map(|p| p.name.clone())
        .collect();
    let mut name = std::mem::take(&mut app.fx.preset_name[u]);
    let r = egui::ComboBox::from_id_salt(("fx_preset", u))
        .width((rect.width() - 8.0).min(120.0))
        .selected_text("Presets")
        .show_ui(&mut pui, |ui| {
            if names.is_empty() {
                ui.label("No presets yet");
            }
            for (i, n) in names.iter().enumerate() {
                ui.horizontal(|ui| {
                    if ui
                        .small_button("×")
                        .on_hover_text("Delete this preset")
                        .clicked()
                    {
                        delete = Some(i);
                    }
                    if ui.selectable_label(false, n).clicked() {
                        load = Some(i);
                    }
                });
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut name)
                        .hint_text("name")
                        .desired_width(110.0),
                );
                if ui.button("Save").clicked() && !name.trim().is_empty() {
                    save = true;
                }
            });
        })
        .response
        .on_hover_text("Load, save or delete FX unit setups");
    app.map.add(ui, "fx_presets", idx(u, 0), r.rect);
    if save {
        let n = name.trim().to_string();
        let unit = unit_config(p);
        match app.config.fx.presets.iter_mut().find(|x| x.name == n) {
            Some(x) => x.unit = unit,
            None => app.config.fx.presets.push(FxPreset { name: n, unit }),
        }
        name.clear();
        app.events.push(AppEvent::ConfigChanged);
    }
    app.fx.preset_name[u] = name;
    if let Some(i) = load
        && let Some(preset) = app.config.fx.presets.get(i).cloned()
    {
        for c in unit_commands(u, &preset.unit) {
            app.fx(c);
        }
    }
    if let Some(i) = delete
        && i < app.config.fx.presets.len()
    {
        app.config.fx.presets.remove(i);
        app.events.push(AppEvent::ConfigChanged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_tempo_needs_two_close_taps() {
        let mut t = TapTempo::default();
        assert_eq!(t.tap(10.0), None);
        let bpm = t.tap(10.5).unwrap();
        assert!((bpm - 120.0).abs() < 1e-9);
        assert!((t.tap(11.0).unwrap() - 120.0).abs() < 1e-9);
        // A pause starts again.
        assert_eq!(t.tap(20.0), None);
    }

    #[test]
    fn unit_config_round_trips_through_commands() {
        let mut p = FxParams::default();
        p.units[1].mode = FxMode::Single;
        p.units[1].beats = 2;
        p.units[1].mix = 0.8;
        p.units[1].assign = [false, false, false, false, true];
        p.units[1].slots[2] = dj_audio::SlotParams::new(EffectKind::Slicer);
        p.units[1].slots[2].params[0] = 0.9;
        let c = unit_config(&p.units[1]);
        let cmds = unit_commands(1, &c);
        // Apply to defaults the way the engine does.
        let (engine, mut h) = dj_audio::Engine::new(dj_audio::EngineConfig::default());
        let mut r = dj_audio::offline::OfflineRenderer::new(engine, 2, 256);
        for cmd in cmds {
            h.send(Command::Fx(cmd)).unwrap();
        }
        r.render_block();
        assert_eq!(h.update().fx.params.units[1], p.units[1]);
        // Unknown effects are skipped.
        let mut bad = c.clone();
        bad.slots[0].effect = "warp_drive".into();
        assert!(
            !unit_commands(1, &bad)
                .iter()
                .any(|c| matches!(c, FxCommand::SlotKind(_, 0, _)))
        );
    }
}
