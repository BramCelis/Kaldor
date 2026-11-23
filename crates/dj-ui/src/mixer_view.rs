//! The mixer under the waveforms: four channel strips in NS4FX order
//! 3-1-2-4 (trim, EQ with kills, filter, PFL cue, meter, fader, crossfader
//! assign), the crossfader, and the master section (master, booth, phones,
//! cue mix, limiter, curves, master meter). Collapsible with the bar above
//! it.

use crate::app::{AppEvent, KaldorApp, region_ui};
use crate::layout::{Frame, MIXER_H, MIXER_MIN_W, MixerState};
use crate::theme::{self, Palette, deck_color};
use crate::uimap::Region;
use crate::widgets::{self, Icon, flat_button};
use dj_audio::{Command, EqBand, MixerCommand, XfaderSide};
use dj_core::DeckId;
use dj_core::config::CentrePanel;
use dj_dsp::curves::{CrossfaderCurve, FaderCurve};
use egui::{Align, Id, Layout, Rect, Sense, Stroke, UiBuilder, pos2, vec2};

/// Channel order on screen, like the NS4FX mixer.
pub const STRIP_ORDER: [usize; 4] = [2, 0, 1, 3];

/// The master level at 12 o'clock of the master knobs, and at start. With
/// auto gain (tracks at -12 LUFS) two decks playing together peak a few dB
/// below the limiter; 0 dB (the old start) went straight into it.
pub const MASTER_CENTRE_DB: f32 = -9.0;
/// The master knobs' range: fully left and fully right.
pub const MASTER_RANGE_DB: (f32, f32) = (-40.0, 6.0);

/// Knob position (0 to 1) of a master level: the left half of the turn is
/// -40 to -9 dB, the right half -9 to +6 dB (12 o'clock = -9 dB).
pub fn master_pos(db: f32) -> f32 {
    let (lo, hi) = MASTER_RANGE_DB;
    let db = db.clamp(lo, hi);
    if db <= MASTER_CENTRE_DB {
        0.5 * (db - lo) / (MASTER_CENTRE_DB - lo)
    } else {
        0.5 + 0.5 * (db - MASTER_CENTRE_DB) / (hi - MASTER_CENTRE_DB)
    }
}

/// The master level at knob position `pos` (0 to 1); see [`master_pos`].
pub fn master_db(pos: f32) -> f32 {
    let (lo, hi) = MASTER_RANGE_DB;
    let pos = pos.clamp(0.0, 1.0);
    if pos <= 0.5 {
        lo + (MASTER_CENTRE_DB - lo) * pos / 0.5
    } else {
        MASTER_CENTRE_DB + (hi - MASTER_CENTRE_DB) * (pos - 0.5) / 0.5
    }
}

const CH_W: f32 = 120.0;
const CH_GAP: f32 = 6.0;
const MASTER_W: f32 = 240.0;
const XF_ROW_H: f32 = 26.0;
const KNOB_ROW_H: f32 = 24.0;

/// One knob row of a channel strip: map name, label, value, range, default,
/// bipolar arc, EQ band.
type KnobRow = (
    &'static str,
    &'static str,
    f32,
    std::ops::RangeInclusive<f32>,
    f32,
    bool,
    Option<EqBand>,
);

/// The bar and, when open, the mixer or the pads panel.
pub(crate) fn mixer_area(app: &mut KaldorApp, ui: &mut egui::Ui, frame: &Frame) {
    // The Extended layout has no mixer, pads or FX area.
    let Some(bar) = frame.mixer_bar else {
        return;
    };
    mixer_bar(app, ui, bar, frame.mixer_state);
    if let Some(rect) = frame.mixer {
        match app.config.ui.centre_panel {
            CentrePanel::Pads => {
                crate::perform_view::pads_area(app, ui, rect);
                return;
            }
            CentrePanel::Fx => {
                crate::fx_view::fx_area(app, ui, rect);
                return;
            }
            CentrePanel::Mixer => {}
        }
        app.map.enter(Region::Mixer, rect);
        let pal = Palette::get(app.config.ui.dark_theme);
        let content = vec2(MIXER_MIN_W, MIXER_H);
        let fits = rect.width() >= content.x && rect.height() >= content.y;
        // A console centred under the waveforms (or the whole area when it
        // scrolls in place of the waveforms).
        let console = if fits {
            Rect::from_center_size(pos2(rect.center().x, rect.top() + content.y / 2.0), content)
        } else {
            rect
        };
        ui.painter().rect(
            console,
            6.0,
            pal.panel,
            Stroke::new(1.0, pal.border),
            egui::StrokeKind::Inside,
        );
        let mut ui = region_ui(ui, rect, Layout::top_down(Align::Min), "mixer");
        if fits {
            mixer(app, &mut ui, console);
        } else {
            // In place of the waveforms on a small window: scroll.
            egui::ScrollArea::both()
                .id_salt("mixer_scroll")
                .auto_shrink([false, false])
                .show(&mut ui, |ui| {
                    app.map.scrolled = true;
                    let (r, _) = ui.allocate_exact_size(content, Sense::hover());
                    mixer(app, ui, r);
                    app.map.scrolled = false;
                });
        }
    }
}

/// Bar above the mixer: open/close switch, MIXER and PADS tabs, and when the
/// window is too small a button to show the panel in place of the
/// waveforms.
fn mixer_bar(app: &mut KaldorApp, ui: &mut egui::Ui, rect: Rect, state: MixerState) {
    app.map.enter(Region::MixerBar, rect);
    let pal = Palette::get(app.config.ui.dark_theme);
    let open = app.config.ui.mixer_open;
    let toggle_r = Rect::from_min_size(rect.min, vec2(30.0, rect.height()));
    let t = flat_button(
        ui,
        toggle_r,
        Id::unique("mixer_toggle"),
        "",
        theme::prop(12.0),
        Some(if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        }),
        false,
        pal.accent,
        false,
    )
    .on_hover_text(if open {
        "Hide the mixer and pads"
    } else {
        "Show the mixer or pads"
    });
    if t.clicked() {
        app.config.ui.mixer_open = !open;
        app.mixer_swap = false;
        app.events.push(AppEvent::ConfigChanged);
    }
    app.map.add(ui, "mixer_toggle", None, toggle_r);
    let mut x = toggle_r.right() + 4.0;
    for (panel, label, name, hover) in [
        (
            CentrePanel::Mixer,
            "MIXER",
            "tab_mixer",
            "The mixer: EQ, filter, faders, crossfader, headphones, master",
        ),
        (
            CentrePanel::Pads,
            "PADS",
            "tab_pads",
            "Pads, loops, beat jump, slip, key shift and grid editing of the LAYER decks",
        ),
        (
            CentrePanel::Fx,
            "FX",
            "tab_fx",
            "The two FX units: effects, beats, wet/dry, decks and master",
        ),
    ] {
        let r = Rect::from_min_size(
            pos2(x, rect.top()),
            vec2(
                if panel == CentrePanel::Fx { 40.0 } else { 56.0 },
                rect.height(),
            ),
        );
        let on = open && app.config.ui.centre_panel == panel;
        let b = flat_button(
            ui,
            r,
            Id::unique(name),
            label,
            theme::prop(12.0),
            None,
            on,
            pal.accent,
            false,
        )
        .on_hover_text(if on { "Click again to close" } else { hover });
        if b.clicked() {
            if on {
                // The open tab again: close the panel.
                app.config.ui.mixer_open = false;
                app.mixer_swap = false;
            } else {
                app.config.ui.centre_panel = panel;
                app.config.ui.mixer_open = true;
            }
            app.events.push(AppEvent::ConfigChanged);
        }
        app.map.add(ui, name, None, r);
        x = r.right() + 4.0;
    }
    let what = match app.config.ui.centre_panel {
        CentrePanel::Pads => "pads",
        CentrePanel::Fx => "FX",
        CentrePanel::Mixer => "mixer",
    };
    let rest = Rect::from_min_max(pos2(x + 6.0, rect.top()), rect.max);
    match state {
        MixerState::NoRoom | MixerState::Swapped => {
            let label = if state == MixerState::NoRoom {
                format!("Show the {what} (replaces the waveforms)")
            } else {
                "Back to the waveforms".to_string()
            };
            let w = 240.0f32.min(rest.width());
            if w > 40.0 {
                let br = Rect::from_min_size(rest.min, vec2(w, rect.height()));
                let b = flat_button(
                    ui,
                    br,
                    Id::unique("mixer_swap"),
                    &label,
                    theme::prop(12.0),
                    None,
                    false,
                    pal.accent,
                    false,
                );
                if b.clicked() {
                    app.mixer_swap = state == MixerState::NoRoom;
                }
                app.map.add(ui, "mixer_swap", None, br);
                let note = Rect::from_min_max(pos2(br.right() + 10.0, rect.top()), rest.max);
                if state == MixerState::NoRoom && note.width() > 40.0 {
                    widgets::text_in(
                        &ui.painter_at(note),
                        note,
                        "not enough room under the waveforms (drag the divider down or enlarge the window)",
                        theme::prop(11.5),
                        pal.text_weak,
                        false,
                    );
                }
            }
        }
        MixerState::Open | MixerState::Closed => {
            let line_y = rect.center().y;
            ui.painter().line_segment(
                [pos2(rest.left(), line_y), pos2(rest.right(), line_y)],
                Stroke::new(1.0, pal.border),
            );
        }
    }
}

/// The whole mixer in `r` (MIXER_MIN_W x MIXER_H).
fn mixer(app: &mut KaldorApp, ui: &mut egui::Ui, r: Rect) {
    let k = r.shrink(6.0);
    let strip_h = k.height() - XF_ROW_H - 4.0;
    for (n, &i) in STRIP_ORDER.iter().enumerate() {
        let sr = Rect::from_min_size(
            pos2(k.left() + n as f32 * (CH_W + CH_GAP), k.top()),
            vec2(CH_W, strip_h),
        );
        channel_strip(app, ui, sr, i);
    }
    let strips_w = 4.0 * CH_W + 3.0 * CH_GAP;
    let xf = Rect::from_min_size(
        pos2(k.left(), k.bottom() - XF_ROW_H),
        vec2(strips_w, XF_ROW_H),
    );
    crossfader(app, ui, xf);
    let master = Rect::from_min_size(
        pos2(k.left() + strips_w + 10.0, k.top()),
        vec2(MASTER_W, k.height()),
    );
    master_section(app, ui, master);
    let meter_col = Rect::from_min_max(pos2(master.right() + 8.0, k.top()), k.max);
    master_meter(app, ui, meter_col);
}

fn channel_strip(app: &mut KaldorApp, ui: &mut egui::Ui, r: Rect, i: usize) {
    let snap = *app.engine.snapshot();
    let c = snap.mixer.channels[i];
    let id = DeckId::ALL[i];
    let pal = Palette::get(app.config.ui.dark_theme);
    let color = deck_color(id);
    ui.painter().rect(
        r,
        5.0,
        pal.raised,
        Stroke::new(1.0, pal.border),
        egui::StrokeKind::Inside,
    );
    let inner = r.shrink(4.0);
    // Header: badge and PFL cue.
    let head = Rect::from_min_size(inner.min, vec2(inner.width(), 18.0));
    let badge = Rect::from_min_size(head.min, vec2(24.0, 18.0));
    ui.painter().rect_filled(badge, 3.0, color);
    ui.painter().text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        id.number().to_string(),
        theme::display(ui.ctx(), 10.0),
        Palette::on(color),
    );
    app.map.add(ui, "channel", Some(i), badge);
    let pfl_r = Rect::from_min_max(pos2(head.right() - 48.0, head.top()), head.max);
    let pfl = flat_button(
        ui,
        pfl_r,
        Id::unique(("pfl", i)),
        "CUE",
        theme::prop(11.0),
        None,
        c.pfl,
        pal.cue,
        false,
    )
    .on_hover_text("Headphone cue (PFL)");
    if pfl.clicked() {
        app.send(Command::Mixer(MixerCommand::Pfl(id, !c.pfl)));
    }
    app.map.add(ui, "pfl", Some(i), pfl_r);

    // Knob rows: TRIM, HI, MID, LOW, FILT, with kills next to the EQ.
    let rows_top = head.bottom() + 4.0;
    let knob_d = KNOB_ROW_H - 2.0;
    let rows: [KnobRow; 5] = [
        ("trim", "TRIM", c.trim, -1.0..=1.0, 0.0, true, None),
        (
            "eq_hi",
            "HI",
            c.eq[EqBand::High as usize],
            0.0..=1.0,
            0.5,
            true,
            Some(EqBand::High),
        ),
        (
            "eq_mid",
            "MID",
            c.eq[EqBand::Mid as usize],
            0.0..=1.0,
            0.5,
            true,
            Some(EqBand::Mid),
        ),
        (
            "eq_low",
            "LOW",
            c.eq[EqBand::Low as usize],
            0.0..=1.0,
            0.5,
            true,
            Some(EqBand::Low),
        ),
        ("filter", "FILT", c.filter, -1.0..=1.0, 0.0, true, None),
    ];
    for (n, (name, label, value, range, default, bipolar, band)) in rows.into_iter().enumerate() {
        let y = rows_top + n as f32 * KNOB_ROW_H;
        let kr = Rect::from_min_size(pos2(inner.left() + 2.0, y + 1.0), vec2(knob_d, knob_d));
        let hover = format!("{label} (double-click: centre)");
        let (_, new) = widgets::knob(
            ui,
            kr,
            Id::unique((name, i)),
            value,
            range,
            default,
            bipolar,
            color,
            &hover,
        );
        app.map.add(ui, name, Some(i), kr);
        ui.painter().text(
            pos2(kr.right() + 4.0, kr.center().y),
            egui::Align2::LEFT_CENTER,
            label,
            theme::prop(10.5),
            pal.text_weak,
        );
        if let Some(v) = new {
            let cmd = match (name, band) {
                ("trim", _) => MixerCommand::Trim(id, v),
                ("filter", _) => MixerCommand::Filter(id, v),
                (_, Some(b)) => MixerCommand::Eq(id, b, v),
                _ => continue,
            };
            app.send(Command::Mixer(cmd));
        }
        if let Some(b) = band {
            let kill_r = Rect::from_min_size(
                pos2(inner.left() + 58.0, y + 4.0),
                vec2(20.0, KNOB_ROW_H - 8.0),
            );
            let on = c.kill[b as usize];
            let kname: &'static str = match b {
                EqBand::High => "kill_hi",
                EqBand::Mid => "kill_mid",
                EqBand::Low => "kill_low",
            };
            let kb = flat_button(
                ui,
                kill_r,
                Id::unique((kname, i)),
                &label[..1],
                theme::prop(10.0),
                None,
                on,
                pal.error,
                false,
            )
            .on_hover_text(format!("Kill {label}"));
            if kb.clicked() {
                app.send(Command::Mixer(MixerCommand::Kill(id, b, !on)));
            }
            app.map.add(ui, kname, Some(i), kill_r);
        }
    }
    let rows_bottom = rows_top + 5.0 * KNOB_ROW_H;
    // Meter and channel fader beside the knobs.
    let meter_r = Rect::from_min_max(
        pos2(inner.left() + 84.0, rows_top + 2.0),
        pos2(inner.left() + 92.0, rows_bottom - 2.0),
    );
    widgets::paint_meter(ui.painter(), meter_r, snap.channel_meters[i], pal.well);
    app.map.add(ui, "channel_meter", Some(i), meter_r);
    let fader_r = Rect::from_min_max(
        pos2(inner.left() + 95.0, rows_top),
        pos2(inner.right(), rows_bottom),
    );
    let (resp, new) = widgets::vfader(ui, fader_r, Id::unique(("fader", i)), c.fader, color, false);
    let resp = resp.on_hover_text("Channel fader (double-click: full)");
    if resp.double_clicked() {
        app.send(Command::Mixer(MixerCommand::Fader(id, 1.0)));
    } else if let Some(f) = new {
        app.send(Command::Mixer(MixerCommand::Fader(id, f)));
    }
    app.map.add(ui, "fader", Some(i), fader_r);
    // Crossfader assign.
    let ar = Rect::from_min_max(
        pos2(inner.left(), rows_bottom + 4.0),
        pos2(inner.right(), inner.bottom()),
    );
    let bw = (ar.width() - 4.0) / 3.0;
    for (n, (side, name)) in [
        (XfaderSide::A, "A"),
        (XfaderSide::Thru, "THRU"),
        (XfaderSide::B, "B"),
    ]
    .into_iter()
    .enumerate()
    {
        let br = Rect::from_min_size(
            pos2(ar.left() + n as f32 * (bw + 2.0), ar.top()),
            vec2(bw, ar.height()),
        );
        let b = flat_button(
            ui,
            br,
            Id::unique(("xf_assign", i, n)),
            name,
            theme::prop(10.0),
            None,
            c.xfader == side,
            pal.accent,
            false,
        )
        .on_hover_text(match side {
            XfaderSide::A => "Crossfader side A",
            XfaderSide::Thru => "Not on the crossfader (thru)",
            XfaderSide::B => "Crossfader side B",
        });
        if b.clicked() {
            app.send(Command::Mixer(MixerCommand::XfaderAssign(id, side)));
        }
        app.map.add(ui, "xf_assign", Some(i), br);
    }
}

fn crossfader(app: &mut KaldorApp, ui: &mut egui::Ui, r: Rect) {
    let mp = app.engine.snapshot().mixer;
    let pal = Palette::get(app.config.ui.dark_theme);
    let lab = 16.0;
    ui.painter().text(
        pos2(r.left() + lab / 2.0, r.center().y),
        egui::Align2::CENTER_CENTER,
        "A",
        theme::prop(12.0),
        pal.text_weak,
    );
    ui.painter().text(
        pos2(r.right() - lab / 2.0, r.center().y),
        egui::Align2::CENTER_CENTER,
        "B",
        theme::prop(12.0),
        pal.text_weak,
    );
    let fr = Rect::from_min_max(
        pos2(r.left() + lab + 4.0, r.top() + 2.0),
        pos2(r.right() - lab - 4.0, r.bottom() - 2.0),
    );
    let (resp, new) = widgets::hfader(
        ui,
        fr,
        Id::unique("crossfader"),
        (mp.crossfader + 1.0) / 2.0,
        pal.accent,
    );
    let resp = resp.on_hover_text("Crossfader (double-click: centre)");
    if resp.double_clicked() {
        app.send(Command::Mixer(MixerCommand::Crossfader(0.0)));
    } else if let Some(t) = new {
        app.send(Command::Mixer(MixerCommand::Crossfader(t * 2.0 - 1.0)));
    }
    app.map.add(ui, "crossfader", None, fr);
}

fn master_section(app: &mut KaldorApp, ui: &mut egui::Ui, r: Rect) {
    let mp = app.engine.snapshot().mixer;
    let pal = Palette::get(app.config.ui.dark_theme);
    ui.painter().rect(
        r,
        5.0,
        pal.raised,
        Stroke::new(1.0, pal.border),
        egui::StrokeKind::Inside,
    );
    let inner = r.shrink(6.0);
    ui.painter().text(
        inner.left_top(),
        egui::Align2::LEFT_TOP,
        "MASTER SECTION",
        theme::prop(10.5),
        pal.text_weak,
    );
    // Four knobs: master, booth, phones (dB, value typed below), cue mix.
    // The master knob uses the master scale (12 o'clock = MASTER_CENTRE_DB).
    let cells_top = inner.top() + 16.0;
    let cell_w = inner.width() / 4.0;
    type DbCmd = fn(f32) -> MixerCommand;
    let db_knobs: [(&'static str, &str, f32, DbCmd); 3] = [
        (
            "master_gain",
            "MASTER",
            mp.master_gain_db,
            MixerCommand::MasterGain,
        ),
        (
            "booth_gain",
            "BOOTH",
            mp.booth_gain_db,
            MixerCommand::BoothGain,
        ),
        (
            "phones_gain",
            "PHONES",
            mp.cue_gain_db,
            MixerCommand::CueGain,
        ),
    ];
    let knob_d = 30.0;
    for (n, (name, label, value, cmd)) in db_knobs.into_iter().enumerate() {
        let cx = inner.left() + cell_w * (n as f32 + 0.5);
        let kr = Rect::from_center_size(pos2(cx, cells_top + knob_d / 2.0), vec2(knob_d, knob_d));
        let master = name == "master_gain";
        let new = if master {
            widgets::knob(
                ui,
                kr,
                Id::unique(name),
                master_pos(value),
                0.0..=1.0,
                0.5,
                false,
                pal.accent,
                &format!(
                    "{label} {value:.1} dB (double-click: 12 o'clock, {MASTER_CENTRE_DB:.0} dB)"
                ),
            )
            .1
            .map(master_db)
        } else {
            widgets::knob(
                ui,
                kr,
                Id::unique(name),
                value,
                -40.0..=6.0,
                0.0,
                false,
                pal.accent,
                &format!("{label} {value:.1} dB (double-click: 0 dB)"),
            )
            .1
        };
        if let Some(v) = new {
            app.send(Command::Mixer(cmd(v)));
        }
        app.map.add(ui, name, None, kr);
        ui.painter().text(
            pos2(cx, kr.bottom() + 7.0),
            egui::Align2::CENTER_CENTER,
            label,
            theme::prop(10.0),
            pal.text_weak,
        );
        // The value: drag or type, like the old slider's number field.
        let vr = Rect::from_center_size(pos2(cx, kr.bottom() + 23.0), vec2(cell_w - 6.0, 16.0));
        let mut x = value;
        let dv = ui.put(
            vr,
            egui::DragValue::new(&mut x)
                .range(-40.0..=6.0)
                .fixed_decimals(1)
                .speed(0.1)
                .suffix(" dB"),
        );
        if dv.changed() {
            app.send(Command::Mixer(cmd(x)));
        }
        app.map.add(ui, "gain_value", None, vr);
    }
    let cx = inner.left() + cell_w * 3.5;
    let kr = Rect::from_center_size(pos2(cx, cells_top + knob_d / 2.0), vec2(knob_d, knob_d));
    let (_, new) = widgets::knob(
        ui,
        kr,
        Id::unique("cue_mix"),
        mp.cue_mix,
        0.0..=1.0,
        0.0,
        false,
        pal.cue,
        "Cue mix: headphones hear cue (left) to master (right)",
    );
    if let Some(m) = new {
        app.send(Command::Mixer(MixerCommand::CueMix(m)));
    }
    app.map.add(ui, "cue_mix", None, kr);
    ui.painter().text(
        pos2(cx, kr.bottom() + 7.0),
        egui::Align2::CENTER_CENTER,
        "CUE MIX",
        theme::prop(10.0),
        pal.text_weak,
    );
    ui.painter().text(
        pos2(cx, kr.bottom() + 21.0),
        egui::Align2::CENTER_CENTER,
        "cue · master",
        theme::prop(9.5),
        pal.text_weak,
    );

    // Limiter, crossfader curve and reverse, fader curve.
    let mut y = cells_top + knob_d + 36.0;
    let row_h = 22.0;
    let lim_r = Rect::from_min_size(pos2(inner.left(), y), vec2(96.0, row_h));
    let lim = flat_button(
        ui,
        lim_r,
        Id::unique("limiter"),
        "LIMITER",
        theme::prop(11.5),
        None,
        mp.limiter,
        pal.accent,
        false,
    )
    .on_hover_text("Master limiter on/off");
    if lim.clicked() {
        app.send(Command::Mixer(MixerCommand::Limiter(!mp.limiter)));
    }
    app.map.add(ui, "limiter", None, lim_r);
    y += row_h + 5.0;
    let label_w = 70.0;
    let text = |ui: &egui::Ui, y: f32, s: &str| {
        ui.painter().text(
            pos2(inner.left(), y + row_h / 2.0),
            egui::Align2::LEFT_CENTER,
            s,
            theme::prop(11.0),
            pal.text_weak,
        );
    };
    text(ui, y, "Crossfader");
    let combo_r = Rect::from_min_size(pos2(inner.left() + label_w, y), vec2(96.0, row_h));
    let mut child = ui.new_child(UiBuilder::new().max_rect(combo_r).id_salt("xf_curve_ui"));
    let combo = egui::ComboBox::from_id_salt("xf_curve")
        .width(combo_r.width() - 8.0)
        .selected_text(mp.crossfader_curve.label())
        .show_ui(&mut child, |ui| {
            for c in CrossfaderCurve::ALL {
                if ui
                    .selectable_label(mp.crossfader_curve == c, c.label())
                    .clicked()
                {
                    app.send(Command::Mixer(MixerCommand::CrossfaderCurve(c)));
                }
            }
        });
    let combo = combo.response.on_hover_text("Crossfader curve");
    app.map.add(ui, "xf_curve", None, combo.rect);
    let rev_r = Rect::from_min_max(
        pos2(combo_r.right() + 6.0, y),
        pos2(inner.right(), y + row_h),
    );
    let rev = flat_button(
        ui,
        rev_r,
        Id::unique("xf_reverse"),
        "REV",
        theme::prop(11.0),
        None,
        mp.crossfader_reverse,
        pal.accent,
        false,
    )
    .on_hover_text("Reverse the crossfader");
    if rev.clicked() {
        app.send(Command::Mixer(MixerCommand::CrossfaderReverse(
            !mp.crossfader_reverse,
        )));
    }
    app.map.add(ui, "xf_reverse", None, rev_r);
    y += row_h + 5.0;
    text(ui, y, "Faders");
    let combo_r = Rect::from_min_size(
        pos2(inner.left() + label_w, y),
        vec2(inner.width() - label_w, row_h),
    );
    let mut child = ui.new_child(UiBuilder::new().max_rect(combo_r).id_salt("fader_curve_ui"));
    let combo = egui::ComboBox::from_id_salt("fader_curve")
        .width(combo_r.width() - 8.0)
        .selected_text(mp.fader_curve.label())
        .show_ui(&mut child, |ui| {
            for c in FaderCurve::ALL {
                if ui
                    .selectable_label(mp.fader_curve == c, c.label())
                    .clicked()
                {
                    app.send(Command::Mixer(MixerCommand::FaderCurve(c)));
                }
            }
        });
    let combo = combo.response.on_hover_text("Channel fader curve");
    app.map.add(ui, "fader_curve", None, combo.rect);
}

fn master_meter(app: &mut KaldorApp, ui: &mut egui::Ui, r: Rect) {
    let snap = *app.engine.snapshot();
    let pal = Palette::get(app.config.ui.dark_theme);
    ui.painter().text(
        pos2(r.center().x, r.top() + 6.0),
        egui::Align2::CENTER_CENTER,
        "M",
        theme::prop(10.5),
        pal.text_weak,
    );
    let mr = Rect::from_min_max(
        pos2(r.center().x - 8.0, r.top() + 14.0),
        pos2(r.center().x + 8.0, r.bottom() - 18.0),
    );
    widgets::paint_meter(ui.painter(), mr, snap.master_meter, pal.well);
    let resp = ui
        .interact(mr, Id::unique("mixer_master_meter"), Sense::hover())
        .on_hover_text("Master level");
    app.map.add(ui, "mixer_master_meter", None, resp.rect);
    let lr = Rect::from_min_max(pos2(r.left(), r.bottom() - 16.0), r.max);
    ui.painter().text(
        lr.center(),
        egui::Align2::CENTER_CENTER,
        "LIM",
        theme::prop(10.0),
        if snap.limiter_gain < 0.999 {
            pal.warn
        } else {
            pal.text_weak.gamma_multiply(0.4)
        },
    );
    app.map.add(ui, "mixer_limiter_light", None, lr);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The master knob scale: 12 o'clock is -9 dB, the ends -40 and +6 dB,
    /// and position and level convert back and forth.
    #[test]
    fn master_knob_scale() {
        assert_eq!(master_db(0.5), MASTER_CENTRE_DB);
        assert_eq!(master_pos(MASTER_CENTRE_DB), 0.5);
        assert_eq!((master_db(0.0), master_db(1.0)), MASTER_RANGE_DB);
        assert_eq!(master_pos(0.0), 0.8, "0 dB is at 0.8 of the turn");
        for db in [-40.0, -30.0, -9.0, -3.0, 0.0, 6.0] {
            assert!((master_db(master_pos(db)) - db).abs() < 1e-4, "{db}");
        }
        // Beyond the range: the ends.
        assert_eq!(master_pos(-80.0), 0.0);
        assert_eq!(master_pos(12.0), 1.0);
    }
}
