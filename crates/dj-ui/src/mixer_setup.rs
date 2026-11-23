//! Setup > Mixer: how the EQ, channel filter, faders and limiter work, kept
//! in the config file (`[mixer]`) and sent to the engine at start.

use crate::app::{AppEvent, KaldorApp};
use dj_audio::{Command, MixerCommand, MixerParams};
use dj_core::config::MixerConfig;
use dj_dsp::curves::{CUT_LAG_RANGE, CrossfaderCurve, EQ_BOOSTS_DB, EqMode, FaderCurve};
use dj_dsp::svf::FilterSlope;
use std::time::{Duration, Instant};

/// Keeps `config.mixer` in step with the engine. After settings were sent
/// (start-up, a new audio output) it waits until the engine has applied
/// them, so the defaults of a fresh engine never overwrite the config.
#[derive(Default)]
pub(crate) struct MixerSync {
    waiting: Option<(Instant, u64)>,
    since_check: u32,
}

impl MixerSync {
    /// Settings were just sent to an engine that has rendered `frames`.
    pub(crate) fn wait(&mut self, frames: u64) {
        self.waiting = Some((Instant::now(), frames));
    }
}

/// The engine commands for a mixer setup.
pub(crate) fn commands(c: &MixerConfig) -> Vec<MixerCommand> {
    let d = MixerParams::default();
    vec![
        MixerCommand::EqMode(EqMode::from_id(&c.eq_mode).unwrap_or(d.eq_mode)),
        MixerCommand::EqBoost(c.eq_boost_db),
        MixerCommand::EqCrossovers(c.eq_low_hz, c.eq_high_hz),
        MixerCommand::FilterShape(
            FilterSlope::from_id(&c.filter_slope).unwrap_or(d.filter_slope),
            c.filter_resonance,
        ),
        MixerCommand::FaderCurve(FaderCurve::from_id(&c.fader_curve).unwrap_or(d.fader_curve)),
        MixerCommand::CrossfaderCurve(
            CrossfaderCurve::from_id(&c.crossfader_curve).unwrap_or(d.crossfader_curve),
        ),
        MixerCommand::CrossfaderReverse(c.crossfader_reverse),
        MixerCommand::CrossfaderCutLag(c.cut_lag),
        MixerCommand::Limiter(c.limiter),
        MixerCommand::LimiterCeiling(c.limiter_ceiling_db),
        MixerCommand::MeterHold(c.meter_hold_seconds),
        MixerCommand::CuePreEq(c.cue_pre_eq),
    ]
}

/// The mixer setup the engine runs with (the start-only settings, such as
/// the crossfader assign, from `base`).
pub(crate) fn config_of(p: &MixerParams, base: &MixerConfig) -> MixerConfig {
    MixerConfig {
        eq_mode: p.eq_mode.id().into(),
        eq_boost_db: p.eq_boost_db,
        eq_low_hz: p.eq_crossovers.0,
        eq_high_hz: p.eq_crossovers.1,
        filter_slope: p.filter_slope.id().into(),
        filter_resonance: p.filter_resonance,
        fader_curve: p.fader_curve.id().into(),
        crossfader_curve: p.crossfader_curve.id().into(),
        crossfader_reverse: p.crossfader_reverse,
        cut_lag: p.cut_lag,
        limiter: p.limiter,
        limiter_ceiling_db: p.limiter_ceiling_db,
        meter_hold_seconds: p.meter_hold_s,
        cue_pre_eq: p.cue_pre_eq,
        crossfader_assign: base.crossfader_assign.clone(),
        version: MixerConfig::VERSION,
    }
}

impl KaldorApp {
    /// Sends the mixer setup from the config to the engine (at start).
    pub(crate) fn send_mixer_config(&mut self) {
        if self.config.mixer.version < MixerConfig::VERSION && self.config.mixer.migrate() {
            self.events.push(AppEvent::ConfigChanged);
        }
        for cmd in commands(&self.config.mixer) {
            self.send(Command::Mixer(cmd));
        }
        // The crossfader side each channel starts on.
        for (d, side) in dj_core::DeckId::ALL
            .into_iter()
            .zip(self.config.mixer.crossfader_assign.clone())
        {
            let side = match side.as_str() {
                "a" => dj_audio::XfaderSide::A,
                "b" => dj_audio::XfaderSide::B,
                _ => dj_audio::XfaderSide::Thru,
            };
            self.send(Command::Mixer(MixerCommand::XfaderAssign(d, side)));
        }
        let frames = self.engine.snapshot().frames_rendered;
        self.mixer_sync.wait(frames);
    }

    /// Writes changes made on the screen back to the config, checked about
    /// twice a second.
    pub(crate) fn sync_mixer_config(&mut self) {
        let frames = self.engine.snapshot().frames_rendered;
        if let Some((at, sent)) = self.mixer_sync.waiting {
            if at.elapsed() < Duration::from_millis(300) || frames <= sent + 2048 {
                return;
            }
            self.mixer_sync.waiting = None;
        }
        self.mixer_sync.since_check += 1;
        if self.mixer_sync.since_check < 30 {
            return;
        }
        self.mixer_sync.since_check = 0;
        let now = config_of(&self.engine.snapshot().mixer, &self.config.mixer);
        if now != self.config.mixer {
            self.config.mixer = now;
            self.events.push(AppEvent::ConfigChanged);
        }
    }
}

/// The settings window's Mixer tab in signal order: auto gain, EQ, filter,
/// faders and the master, as cards of setting rows.
pub(crate) fn mixer_setup(app: &mut KaldorApp, ui: &mut egui::Ui) {
    use crate::settings_ui::{button, card, segmented, switch};
    let pal = crate::theme::Palette::get(app.config.ui.dark_theme);
    let p = app.engine.snapshot().mixer;
    let mut out: Vec<MixerCommand> = Vec::new();
    // Auto gain lives in `[library]` (it comes from the loudness analysis)
    // but works like each channel's trim, so it is set here.
    let mut gain_changed = false;
    card(ui, &pal, "Auto gain", |rows| {
        let lib = &mut app.config.library;
        rows.row(
            "Auto gain",
            "Evens out track levels from their measured loudness, on top of the trim (at most ±12 dB).",
            |ui| {
                let r = switch(ui, lib.auto_gain);
                if r.clicked() {
                    lib.auto_gain = !lib.auto_gain;
                    gain_changed = true;
                }
                app.map.add(ui, "auto_gain", None, r.rect);
            },
        );
        rows.row(
            "Target loudness",
            "The level auto gain brings every track to. -12 LUFS suits loud club masters; lower leaves more headroom.",
            |ui| {
                ui.add_enabled_ui(lib.auto_gain, |ui| {
                    let r = ui.add(
                        egui::Slider::new(&mut lib.target_lufs, -20.0..=-6.0)
                            .step_by(0.5)
                            .fixed_decimals(1)
                            .suffix(" LUFS"),
                    );
                    gain_changed |= r.changed();
                    app.map.add(ui, "target_lufs", None, r.rect);
                });
            },
        );
    });
    if gain_changed {
        app.reapply_auto_gain();
        app.events.push(AppEvent::ConfigChanged);
    }
    card(ui, &pal, "EQ", |rows| {
        rows.row(
            "EQ type",
            "Isolator: each knob cuts its band to silence. Classic: shelves and a bell, -26 dB fully left.",
            |ui| {
                egui::ComboBox::from_id_salt("setup_eq_mode")
                    .selected_text(p.eq_mode.label())
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for m in EqMode::ALL {
                            if ui.selectable_label(p.eq_mode == m, m.label()).clicked() {
                                out.push(MixerCommand::EqMode(m));
                            }
                        }
                    });
            },
        );
        rows.row("EQ boost", "How far the EQ knobs go up.", |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let labels: Vec<String> =
                    EQ_BOOSTS_DB.iter().map(|b| format!("+{b:.0} dB")).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let sel = EQ_BOOSTS_DB.iter().position(|b| *b == p.eq_boost_db);
                if let (Some(i), _) = segmented(ui, &refs, sel) {
                    out.push(MixerCommand::EqBoost(EQ_BOOSTS_DB[i]));
                }
            });
        });
        rows.row(
            "Crossovers",
            "Where low meets mid and mid meets high (default 150 Hz / 4 kHz).",
            |ui| {
                if button(ui, "Reset", false, None)
                    .on_hover_text("150 Hz / 4 kHz")
                    .clicked()
                {
                    out.push(MixerCommand::EqCrossovers(150.0, 4000.0));
                }
                let (mut lo, mut hi) = p.eq_crossovers;
                let b = ui.add(
                    egui::DragValue::new(&mut hi)
                        .range(500.0..=10_000.0)
                        .speed(20.0)
                        .suffix(" Hz"),
                );
                ui.label("/");
                let a = ui.add(
                    egui::DragValue::new(&mut lo)
                        .range(50.0..=1000.0)
                        .speed(2.0)
                        .suffix(" Hz"),
                );
                if a.changed() || b.changed() {
                    out.push(MixerCommand::EqCrossovers(lo, hi));
                }
            },
        );
    });
    card(ui, &pal, "Filter", |rows| {
        rows.row("Slope", "How steep the filter knob cuts.", |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let labels: Vec<&str> = FilterSlope::ALL.iter().map(|s| s.label()).collect();
                let sel = FilterSlope::ALL.iter().position(|s| *s == p.filter_slope);
                if let (Some(i), _) = segmented(ui, &labels, sel) {
                    out.push(MixerCommand::FilterShape(
                        FilterSlope::ALL[i],
                        p.filter_resonance,
                    ));
                }
            });
        });
        rows.row(
            "Resonance",
            "Peak at the cut-off: none at the centre, full after the first bit of the turn.",
            |ui| {
                let mut res = p.filter_resonance;
                if ui
                    .add(egui::Slider::new(&mut res, 0.0..=1.0).show_value(true))
                    .changed()
                {
                    out.push(MixerCommand::FilterShape(p.filter_slope, res));
                }
            },
        );
    });
    card(ui, &pal, "Faders", |rows| {
        rows.row(
            "Channel faders",
            "From the slowest fade (Club) to the fastest (Sharp).",
            |ui| {
                egui::ComboBox::from_id_salt("setup_fader_curve")
                    .selected_text(p.fader_curve.label())
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for c in FaderCurve::ALL {
                            if ui.selectable_label(p.fader_curve == c, c.label()).clicked() {
                                out.push(MixerCommand::FaderCurve(c));
                            }
                        }
                    });
            },
        );
        rows.row(
            "Crossfader",
            "Smooth: constant power. Dip: half level in the middle. Linear: full in the middle. Sharp: full over the middle 60 %. Cut: scratch cut at the ends.",
            |ui| {
                egui::ComboBox::from_id_salt("setup_xf_curve")
                    .selected_text(p.crossfader_curve.label())
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for c in CrossfaderCurve::ALL {
                            if ui
                                .selectable_label(p.crossfader_curve == c, c.label())
                                .clicked()
                            {
                                out.push(MixerCommand::CrossfaderCurve(c));
                            }
                        }
                    });
            },
        );
        rows.row("Reverse crossfader", "Swaps the A and B sides.", |ui| {
            if switch(ui, p.crossfader_reverse).clicked() {
                out.push(MixerCommand::CrossfaderReverse(!p.crossfader_reverse));
            }
        });
        rows.row(
            "Cut-in distance",
            "How far from each end the Cut curve fades in (scratching).",
            |ui| {
                let mut lag = p.cut_lag * 100.0;
                if ui
                    .add(
                        egui::Slider::new(
                            &mut lag,
                            CUT_LAG_RANGE.0 * 100.0..=CUT_LAG_RANGE.1 * 100.0,
                        )
                        .suffix(" %")
                        .fixed_decimals(1),
                    )
                    .changed()
                {
                    out.push(MixerCommand::CrossfaderCutLag(lag / 100.0));
                }
            },
        );
    });
    card(ui, &pal, "Master", |rows| {
        rows.row(
            "Limiter",
            "A soft clipper from -3 dBFS and a lookahead limiter at the ceiling below.",
            |ui| {
                let r = switch(ui, p.limiter);
                if r.clicked() {
                    out.push(MixerCommand::Limiter(!p.limiter));
                }
                app.map.add(ui, "limiter", None, r.rect);
            },
        );
        rows.row(
            "Limiter ceiling",
            "The loudest the master gets. -0.3 dBFS is the usual; lower gives converters and speakers more room.",
            |ui| {
                let mut c = p.limiter_ceiling_db;
                let r = ui.add(
                    egui::Slider::new(&mut c, -2.0..=-0.1)
                        .step_by(0.1)
                        .fixed_decimals(1)
                        .suffix(" dBFS"),
                );
                if r.changed() {
                    out.push(MixerCommand::LimiterCeiling(c));
                }
                app.map.add(ui, "limiter_ceiling", None, r.rect);
            },
        );
        rows.row(
            "Meter peak hold",
            "How long the peak line on the meters stays up before it falls.",
            |ui| {
                let mut h = p.meter_hold_s;
                let r = ui.add(
                    egui::Slider::new(&mut h, 0.0..=5.0)
                        .step_by(0.1)
                        .fixed_decimals(1)
                        .suffix(" s"),
                );
                if r.changed() {
                    out.push(MixerCommand::MeterHold(h));
                }
                app.map.add(ui, "meter_hold", None, r.rect);
            },
        );
    });
    card(ui, &pal, "Headphones", |rows| {
        rows.row(
            "Cue before the EQ",
            "The headphones hear a channel before its EQ and filter, so you can check a track with its bass cut. Off: what the EQ makes of it.",
            |ui| {
                let r = switch(ui, p.cue_pre_eq);
                if r.clicked() {
                    out.push(MixerCommand::CuePreEq(!p.cue_pre_eq));
                }
                app.map.add(ui, "cue_pre_eq", None, r.rect);
            },
        );
    });
    let mut assign_changed = false;
    card(ui, &pal, "Crossfader assign at start", |rows| {
        let assign = &mut app.config.mixer.crossfader_assign;
        if assign.len() < 4 {
            assign.resize(4, "thru".into());
        }
        for (d, side) in assign.iter_mut().enumerate().take(4) {
            rows.row(
                &format!("Channel {}", d + 1),
                if d == 0 {
                    "The side of the crossfader each channel starts on. The NS4FX's own switches win once it is connected."
                } else {
                    ""
                },
                |ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let ids = ["a", "thru", "b"];
                        let sel = ids.iter().position(|s| *s == side.as_str());
                        let (c, rects) = segmented(ui, &["A", "Off", "B"], sel);
                        if let Some(i) = c {
                            *side = ids[i].into();
                            assign_changed = true;
                        }
                        for (i, r) in rects.into_iter().enumerate() {
                            app.map.add(ui, "xfader_assign_default", Some(d * 3 + i), r);
                        }
                    });
                },
            );
        }
    });
    if assign_changed {
        app.events.push(AppEvent::ConfigChanged);
    }
    card(ui, &pal, "Reset", |rows| {
        rows.row(
            "Mixer defaults",
            "Puts the EQ, filter, fader and limiter settings back (auto gain stays).",
            |ui| {
                let d = button(ui, "Reset all", false, None);
                if d.clicked() {
                    out.extend(commands(&MixerConfig::default()));
                }
                app.map.add(ui, "mixer_defaults", None, d.rect);
            },
        );
    });
    for c in out {
        app.send(Command::Mixer(c));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_through_the_engine_params() {
        let c = MixerConfig {
            eq_mode: "classic".into(),
            eq_boost_db: 12.0,
            eq_low_hz: 300.0,
            eq_high_hz: 3000.0,
            filter_slope: "24".into(),
            filter_resonance: 0.4,
            fader_curve: "sharp".into(),
            crossfader_curve: "cut".into(),
            crossfader_reverse: true,
            cut_lag: 0.01,
            limiter: false,
            limiter_ceiling_db: -1.0,
            meter_hold_seconds: 2.0,
            cue_pre_eq: true,
            crossfader_assign: vec!["a".into(), "thru".into(), "thru".into(), "b".into()],
            version: MixerConfig::VERSION,
        };
        let mut p = MixerParams::default();
        for cmd in commands(&c) {
            apply(&mut p, cmd);
        }
        assert_eq!(config_of(&p, &c), c);
        assert_eq!(
            config_of(&MixerParams::default(), &MixerConfig::default()),
            MixerConfig::default()
        );
    }

    #[test]
    fn auto_gain_settings_reach_loaded_tracks() {
        let mut demo = crate::headless::Demo::new(dj_core::AppConfig::default());
        let size = egui::vec2(1280.0, 800.0);
        let gain = |demo: &crate::headless::Demo, d: usize| {
            demo.app.engine.snapshot().mixer.channels[d].auto_gain_db
        };
        // Deck 1 holds a track at -18 LUFS, deck 2 one with only a stored
        // gain (no loudness known).
        demo.app.decks[0].loudness = (Some(-18.0), Some(6.0));
        demo.app.decks[1].loudness = (None, Some(-2.0));
        demo.app.reapply_auto_gain();
        demo.run(3, size, 1.0);
        assert_eq!(gain(&demo, 0), 6.0);
        assert_eq!(gain(&demo, 1), -2.0);
        // A new target moves the gain of the track with a known loudness.
        demo.app.config.library.target_lufs = -14.0;
        demo.app.reapply_auto_gain();
        demo.run(3, size, 1.0);
        assert_eq!(gain(&demo, 0), 4.0);
        assert_eq!(gain(&demo, 1), -2.0);
        // Off: no auto gain on any deck.
        demo.app.config.library.auto_gain = false;
        demo.app.reapply_auto_gain();
        demo.run(3, size, 1.0);
        assert_eq!(gain(&demo, 0), 0.0);
        assert_eq!(gain(&demo, 1), 0.0);
    }

    /// The parts of the engine's command handling the round trip needs.
    fn apply(p: &mut MixerParams, cmd: MixerCommand) {
        match cmd {
            MixerCommand::EqMode(m) => p.eq_mode = m,
            MixerCommand::EqBoost(b) => p.eq_boost_db = b,
            MixerCommand::EqCrossovers(a, b) => p.eq_crossovers = (a, b),
            MixerCommand::FilterShape(s, r) => {
                p.filter_slope = s;
                p.filter_resonance = r;
            }
            MixerCommand::FaderCurve(c) => p.fader_curve = c,
            MixerCommand::CrossfaderCurve(c) => p.crossfader_curve = c,
            MixerCommand::CrossfaderReverse(r) => p.crossfader_reverse = r,
            MixerCommand::CrossfaderCutLag(l) => p.cut_lag = l,
            MixerCommand::Limiter(l) => p.limiter = l,
            MixerCommand::LimiterCeiling(c) => p.limiter_ceiling_db = c,
            MixerCommand::MeterHold(s) => p.meter_hold_s = s,
            MixerCommand::CuePreEq(on) => p.cue_pre_eq = on,
            _ => {}
        }
    }
}
