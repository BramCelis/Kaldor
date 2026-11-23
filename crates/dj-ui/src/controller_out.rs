//! The NS4FX's lights, jog displays and meters.
//!
//! The UI works out what everything should show once a frame
//! ([`Feedback`], from the engine snapshot and the UI state); the
//! controller thread turns it into MIDI with [`OutputState`], sending only
//! what changed. Every message used here was confirmed on the controller
//! with an output probe.
//!
//! The rule: a button that can be used is dim, a button that is on
//! is bright. PLAY and CUE flicker (bright / dim) while a loaded deck is
//! paused.

use crate::perform::{AUTO_LOOP_PADS, PITCH_PADS, PadMode, ROLL_PADS};
use dj_midi::ns4fx_out::{
    LED_DIM, LED_OFF, LED_ON, display_bpm, display_duration, display_pitch, display_range,
    display_time, led, position_ring, spinner, vu,
};
use std::collections::HashMap;

/// A light value meaning "flicker": bright and dim in turn (resolved by
/// [`OutputState::messages`] from its `flicker_on` phase).
pub const FLICKER: u8 = 0xFF;
/// Pad brightness for "something here, not active" (a saved loop, a
/// loaded sample): a little brighter than dim (probe step 9).
const PAD_FILLED: u8 = 0x05;
/// Seconds per platter turn (33 1/3 rpm), for the platter dot.
const TURN_SECONDS: f64 = 1.8;
/// Level meter range: -24 dB lights the first of the 5 LEDs, 0 dB all.
const VU_FLOOR_DB: f32 = -24.0;

/// What one deck's lights and display show.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeckFeedback {
    /// The deck's pad mode (the DDJ-FLX6 lights its pads in that mode's
    /// notes).
    pub pad_mode: PadMode,
    pub loaded: bool,
    pub play: u8,
    pub cue: u8,
    pub sync: u8,
    pub pads: [u8; 8],
    /// The four pad mode lights (CUES, AUTO LOOP, FADER CUTS, SAMPLE).
    pub modes: [u8; 4],
    pub loop_on: bool,
    pub vinyl: bool,
    pub key_lock: bool,
    /// BPM-match arrows: (up, down).
    pub arrows: (bool, bool),
    pub bpm: f64,
    pub pitch_percent: f64,
    pub range_percent: u8,
    pub duration_ms: u64,
    pub time_ms: u64,
    /// Track position 0.0-1.0 (the ring).
    pub position: f64,
    /// Platter turn phase 0.0-1.0 (the dot).
    pub spin: f64,
}

/// Everything the controller shows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Feedback {
    pub decks: [DeckFeedback; 4],
    /// Headphone cue per mixer channel.
    pub pfl: [bool; 4],
    /// Channel level 0.0-1.0 per mixer channel.
    pub vu: [f32; 4],
    /// The six effect buttons' lights: bright = the chosen effect,
    /// [`FLICKER`] = the chosen color FX (as a DJM flashes its button),
    /// dim = the rest.
    pub fx: [u8; 6],
}

/// The four pad mode notes: CUES, AUTO LOOP, FADER CUTS, SAMPLE.
const MODE_NOTES: [u8; 4] = [0x00, 0x0D, 0x07, 0x0B];

/// The mode lights for a pad mode: all dim, the active one bright; a SHIFT
/// mode flickers the light of its button (the SHIFT modes have no light of
/// their own: probe steps 15-18).
fn mode_lights(m: PadMode) -> [u8; 4] {
    let mut l = [LED_DIM; 4];
    let (i, v) = match m {
        PadMode::Cue => (0, LED_ON),
        PadMode::SavedLoop => (0, FLICKER),
        PadMode::AutoLoop => (1, LED_ON),
        PadMode::Roll => (1, FLICKER),
        PadMode::Slicer => (2, FLICKER),
        PadMode::Sampler => (3, LED_ON),
        PadMode::PitchPlay => (3, FLICKER),
        // Not an NS4FX mode: no light of its own.
        PadMode::BeatJump | PadMode::Stems => return l,
    };
    l[i] = v;
    l
}

/// Level 0.0-1.0 from a linear peak.
fn level(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * peak.log10();
    ((db - VU_FLOOR_DB) / -VU_FLOOR_DB).clamp(0.0, 1.0)
}

fn on_dim(on: bool) -> u8 {
    if on { LED_ON } else { LED_DIM }
}

impl crate::app::KaldorApp {
    /// What the controller should show now.
    pub fn controller_feedback(&self) -> Feedback {
        let snap = self.engine.snapshot();
        let sr = f64::from(snap.sample_rate.max(1));
        let mut fb = Feedback::default();
        for i in 0..4 {
            let d = snap.decks[i];
            let ui = &self.decks[i];
            let perf = &ui.perf;
            let loaded = d.loaded;
            let tempo = crate::deck_view::shown_tempo(&d);
            let bpm = ui.bpm.filter(|_| loaded).map(|b| b * tempo);
            let pads: [u8; 8] = std::array::from_fn(|k| match perf.pad_mode {
                PadMode::Cue => on_dim(loaded && d.hot_cues[k].is_some()),
                PadMode::SavedLoop => match &perf.saved_loops[k] {
                    Some(l) if loaded => {
                        let active = d
                            .loop_info
                            .is_some_and(|a| a.active && (a.start / sr - l.start).abs() < 1e-3);
                        if active { LED_ON } else { PAD_FILLED }
                    }
                    _ => LED_DIM,
                },
                PadMode::AutoLoop => on_dim(
                    d.loop_info
                        .is_some_and(|l| l.active && (l.beats - AUTO_LOOP_PADS[k]).abs() < 1e-4),
                ),
                PadMode::Roll => on_dim(
                    d.rolling
                        && d.loop_info
                            .is_some_and(|l| (l.beats - ROLL_PADS[k]).abs() < 1e-4),
                ),
                PadMode::Sampler => {
                    let s = snap.sampler.slots[self.sampler.bank * 8 + k];
                    if s.playing {
                        LED_ON
                    } else if s.loaded {
                        PAD_FILLED
                    } else {
                        LED_DIM
                    }
                }
                PadMode::Slicer => on_dim(d.slicer_held == Some(k as u8)),
                PadMode::PitchPlay => on_dim(loaded && d.key_shift == PITCH_PADS[k]),
                // Lit while held (each pad is a jump, nothing stays on).
                PadMode::BeatJump => on_dim(perf.held[k]),
                PadMode::Stems => LED_OFF,
            });
            // BPM-match arrows against the deck on the other side.
            let other = self.layer[1 - i % 2];
            let target = {
                let o = snap.decks[other];
                self.decks[other]
                    .bpm
                    .filter(|_| o.loaded)
                    .map(|b| b * crate::deck_view::shown_tempo(&o))
            };
            let arrows = match (bpm, target) {
                (Some(a), Some(b)) if (a - b).abs() >= 0.05 => (a < b, a > b),
                _ => (false, false),
            };
            let paused = loaded && !d.playing && !d.previewing;
            let secs = d.position / sr;
            fb.decks[i] = DeckFeedback {
                pad_mode: perf.pad_mode,
                loaded,
                play: if d.playing {
                    LED_ON
                } else if paused {
                    FLICKER
                } else {
                    LED_DIM
                },
                cue: if d.previewing {
                    LED_ON
                } else if paused {
                    FLICKER
                } else {
                    LED_DIM
                },
                sync: on_dim(d.sync),
                pads,
                modes: mode_lights(perf.pad_mode),
                loop_on: d.loop_info.is_some_and(|l| l.active),
                vinyl: ui.jog_vinyl,
                key_lock: d.key_lock,
                arrows,
                bpm: bpm.unwrap_or(0.0),
                pitch_percent: if loaded { (tempo - 1.0) * 100.0 } else { 0.0 },
                range_percent: (d.pitch_range.fraction() * 100.0).round() as u8,
                duration_ms: (d.length / sr * 1000.0) as u64,
                time_ms: (secs * 1000.0).max(0.0) as u64,
                position: if d.length > 0.0 {
                    d.position / d.length
                } else {
                    0.0
                },
                spin: (secs / TURN_SECONDS).rem_euclid(1.0),
            };
        }
        // The chosen effect's button bright; the chosen color FX's button
        // flickers (it wins when both are the same button).
        let lit = crate::fx_panel::lit(&snap.fx.params);
        let color = snap.mixer.color_fx;
        let flash = if color == dj_audio::ColorFx::Filter {
            None
        } else {
            crate::fx_panel::COLOR.iter().position(|c| *c == color)
        };
        for (k, v) in fb.fx.iter_mut().enumerate() {
            *v = if flash == Some(k) {
                FLICKER
            } else if lit == Some(k) {
                LED_ON
            } else {
                LED_DIM
            };
        }
        for c in 0..4 {
            fb.pfl[c] = snap.mixer.channels[c].pfl;
            let m = snap.channel_meters[c];
            fb.vu[c] = level(m.peak[0].max(m.peak[1]));
        }
        fb
    }
}

/// The controller's own state the lights need (from the MIDI side).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ControllerState {
    /// Decks whose pads the controller drives itself (FADER CUTS mode).
    pub fader_cuts: [bool; 4],
    /// How far each jog wheel has turned (revolutions), for the platter
    /// dot of a deck without a track.
    pub jog_turns: [f64; 4],
    /// Decks in the DDJ-FLX6's KEY SHIFT pad mode (the controller's own).
    pub key_shift_mode: [bool; 4],
}

/// What was last sent, so only changes go out.
#[derive(Default)]
pub struct OutputState {
    /// (status, number) of 3-byte messages to their last value.
    sent: HashMap<(u8, u8), u8>,
    /// Display values last sent per deck: BPM and pitch in tenths, length
    /// in ms, time in tenths of a second.
    bpm: [Option<i64>; 4],
    pitch: [Option<i64>; 4],
    duration: [Option<u64>; 4],
    time: [Option<u64>; 4],
}

impl OutputState {
    /// Forget everything (after connecting): the next call sends it all.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn short(&mut self, m: [u8; 3], out: &mut Vec<Vec<u8>>) {
        if self.sent.insert((m[0], m[1]), m[2]) != Some(m[2]) {
            out.push(m.to_vec());
        }
    }

    /// The messages that bring the controller to `fb`. `flicker_on`: the
    /// bright half of the flicker.
    pub fn messages(
        &mut self,
        fb: &Feedback,
        ctl: &ControllerState,
        flicker_on: bool,
        out: &mut Vec<Vec<u8>>,
    ) {
        let lit = |v: u8| match v {
            FLICKER if flicker_on => LED_ON,
            FLICKER => LED_DIM,
            v => v,
        };
        let onoff = |b: bool| if b { LED_ON } else { LED_OFF };
        for (i, d) in fb.decks.iter().enumerate() {
            let ch = i as u8;
            let pad_ch = 4 + ch;
            let deck = ch + 1;
            self.short(led(ch, 0x00, lit(d.play)), out);
            self.short(led(ch, 0x01, lit(d.cue)), out);
            self.short(led(ch, 0x02, lit(d.sync)), out);
            self.short(led(ch, 0x07, on_dim(d.vinyl)), out);
            // Display symbols: on or off.
            self.short(led(ch, 0x0D, onoff(d.key_lock)), out);
            self.short(led(ch, 0x09, onoff(d.arrows.0)), out);
            self.short(led(ch, 0x0A, onoff(d.arrows.1)), out);
            self.short(display_range(deck, d.range_percent), out);
            // Loop buttons: ON/OFF bright with a loop, 1/2 and x2 always
            // available.
            self.short(led(pad_ch, 0x40, on_dim(d.loop_on)), out);
            self.short(led(pad_ch, 0x34, LED_DIM), out);
            self.short(led(pad_ch, 0x35, LED_DIM), out);
            let modes = if ctl.fader_cuts[i] {
                [LED_DIM, LED_DIM, LED_ON, LED_DIM]
            } else {
                d.modes
            };
            for (n, v) in MODE_NOTES.iter().zip(modes) {
                self.short(led(pad_ch, *n, lit(v)), out);
            }
            if !ctl.fader_cuts[i] {
                for (k, v) in d.pads.iter().enumerate() {
                    self.short(led(pad_ch, 0x14 + k as u8, lit(*v)), out);
                }
            }
            self.short(position_ring(deck, d.position), out);
            // The platter dot turns with the track, or with the jog when no
            // track is loaded.
            let spin = if d.loaded {
                d.spin
            } else {
                ctl.jog_turns[i].rem_euclid(1.0)
            };
            self.short(spinner(deck, spin), out);
            // The displays show tenths; send when what they show changes.
            let bpm = (d.bpm * 10.0).round() as i64;
            if self.bpm[i] != Some(bpm) {
                self.bpm[i] = Some(bpm);
                out.push(display_bpm(deck, d.bpm));
            }
            let pitch = (d.pitch_percent * 10.0).round() as i64;
            if self.pitch[i] != Some(pitch) {
                self.pitch[i] = Some(pitch);
                out.push(display_pitch(deck, d.pitch_percent));
            }
            if self.duration[i] != Some(d.duration_ms) {
                self.duration[i] = Some(d.duration_ms);
                out.push(display_duration(deck, d.duration_ms));
            }
            let tenths = d.time_ms / 100;
            if self.time[i] != Some(tenths) {
                self.time[i] = Some(tenths);
                out.push(display_time(deck, tenths * 100));
            }
        }
        for c in 0..4u8 {
            let i = usize::from(c);
            self.short(led(c, 0x1B, on_dim(fb.pfl[i])), out);
            // 5 LEDs: send whole steps only (probe: 40 of 81 lit 3 of 5).
            let steps = (fb.vu[i] * 5.0).round() / 5.0;
            self.short(vu(c + 1, steps), out);
        }
        // Effect buttons: dim = available, bright = the chosen effect,
        // flickering = the chosen color FX.
        for (k, v) in fb.fx.iter().enumerate() {
            let channel = if k < 3 { 8 } else { 9 };
            self.short(led(channel, k as u8, lit(*v)), out);
        }
    }

    /// The messages that bring a DDJ-FLX6 to `fb` (its lights are on or
    /// off: what the NS4FX shows dim is off, a flicker blinks).
    pub fn flx6_messages(
        &mut self,
        fb: &Feedback,
        ctl: &ControllerState,
        flicker_on: bool,
        out: &mut Vec<Vec<u8>>,
    ) {
        use dj_midi::ddj_flx6_out as flx;
        let on = |v: u8| match v {
            FLICKER => flicker_on,
            v => v > LED_DIM,
        };
        for (i, d) in fb.decks.iter().enumerate() {
            let deck = i as u8 + 1;
            self.short(flx::deck_light(deck, flx::PLAY, on(d.play)), out);
            self.short(flx::deck_light(deck, flx::CUE, on(d.cue)), out);
            self.short(flx::deck_light(deck, flx::SYNC, on(d.sync)), out);
            self.short(flx::deck_light(deck, flx::LOOP_IN, d.loop_on), out);
            self.short(flx::deck_light(deck, flx::LOOP_OUT, d.loop_on), out);
            // The pad mode: its button lit, its pads in its notes.
            let (light, base) = if ctl.key_shift_mode[i] {
                (flx::mode_light::KEY_SHIFT, Some(flx::pad_base::KEY_SHIFT))
            } else {
                flx_mode(d.pad_mode)
            };
            for m in flx::mode_light::ALL {
                self.short(flx::deck_light(deck, m, m == light), out);
            }
            if let Some(base) = base {
                for (k, v) in d.pads.iter().enumerate() {
                    let lit = !ctl.key_shift_mode[i] && on(*v);
                    for shift in [false, true] {
                        self.short(flx::pad_light(deck, shift, base + k as u8, lit), out);
                    }
                }
            }
            let turn = if d.loaded {
                d.spin
            } else {
                ctl.jog_turns[i].rem_euclid(1.0)
            };
            self.short(flx::jog_light(deck, turn), out);
        }
        for c in 0..4u8 {
            let i = usize::from(c);
            self.short(flx::deck_light(c + 1, flx::CH_CUE, fb.pfl[i]), out);
            self.short(flx::level_meter(c + 1, fb.vu[i]), out);
        }
    }

    /// Every light that was sent on, off again (the app is closing).
    pub fn all_off(&mut self, out: &mut Vec<Vec<u8>>) {
        for (&(status, number), v) in self.sent.iter_mut() {
            if *v != 0 && status & 0xF0 != 0xF0 {
                out.push(vec![status, number, 0]);
                *v = 0;
            }
        }
    }
}

/// A Kaldor pad mode's DDJ-FLX6 mode light and first pad note (none: a
/// mode the controller has no button for).
fn flx_mode(m: PadMode) -> (u8, Option<u8>) {
    use dj_midi::ddj_flx6_out::{mode_light as l, pad_base as p};
    match m {
        PadMode::Cue => (l::HOT_CUE, Some(p::HOT_CUE)),
        PadMode::PitchPlay => (l::KEYBOARD, Some(p::KEYBOARD)),
        PadMode::Roll => (l::PAD_FX, Some(p::PAD_FX)),
        PadMode::BeatJump => (l::BEAT_JUMP, Some(p::BEAT_JUMP)),
        PadMode::AutoLoop => (l::BEAT_LOOP, Some(p::BEAT_LOOP)),
        PadMode::Sampler => (l::SAMPLER, Some(p::SAMPLER)),
        PadMode::Slicer => (l::SAMPLE_SCRATCH, Some(p::SAMPLE_SCRATCH)),
        PadMode::SavedLoop | PadMode::Stems => (0, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sends_everything_once_then_only_changes() {
        let mut s = OutputState::default();
        let ctl = ControllerState::default();
        let mut fb = Feedback::default();
        fb.decks[0].play = LED_ON;
        fb.decks[0].bpm = 128.0;
        fb.decks[0].modes = mode_lights(PadMode::Cue);
        fb.fx = [LED_DIM; 6];
        let mut out = Vec::new();
        s.messages(&fb, &ctl, true, &mut out);
        assert!(out.contains(&vec![0x90, 0x00, 0x7F]), "PLAY lit");
        assert!(out.contains(&display_bpm(1, 128.0)));
        assert!(out.contains(&vec![0x94, 0x00, 0x7F]), "CUES mode light");
        assert!(out.contains(&vec![0x94, 0x0D, LED_DIM]), "other modes dim");
        assert!(out.contains(&vec![0x94, 0x34, LED_DIM]), "LOOP 1/2 dim");
        assert!(out.contains(&vec![0x98, 0x00, LED_DIM]), "FX dim");
        let first = out.len();
        out.clear();
        s.messages(&fb, &ctl, true, &mut out);
        assert!(out.is_empty(), "nothing changed: {out:?}");
        // A change: only that message (and the time display's tenth).
        fb.decks[0].play = LED_DIM;
        fb.decks[0].time_ms = 1_049;
        s.messages(&fb, &ctl, true, &mut out);
        assert_eq!(out.len(), 2, "{out:?}");
        assert_eq!(out[0], vec![0x90, 0x00, LED_DIM]);
        out.clear();
        fb.decks[0].time_ms = 1_099;
        s.messages(&fb, &ctl, true, &mut out);
        assert!(out.is_empty());
        // Flicker: bright, then dim with the phase.
        fb.decks[0].play = FLICKER;
        s.messages(&fb, &ctl, true, &mut out);
        assert_eq!(out, vec![vec![0x90, 0x00, LED_ON]]);
        out.clear();
        s.messages(&fb, &ctl, false, &mut out);
        assert_eq!(out, vec![vec![0x90, 0x00, LED_DIM]]);
        // A chosen color FX: its effect button flickers (Echo, the 4th:
        // channel 10, note 3).
        out.clear();
        fb.fx[3] = FLICKER;
        s.messages(&fb, &ctl, true, &mut out);
        assert!(out.contains(&vec![0x99, 0x03, LED_ON]), "{out:?}");
        out.clear();
        s.messages(&fb, &ctl, false, &mut out);
        assert!(out.contains(&vec![0x99, 0x03, LED_DIM]), "{out:?}");
        fb.fx[3] = LED_DIM;
        s.messages(&fb, &ctl, false, &mut out);
        // FADER CUTS: its light bright, pads left alone.
        out.clear();
        fb.decks[0].pads[0] = LED_ON;
        let cuts = ControllerState {
            fader_cuts: [true, false, false, false],
            ..ControllerState::default()
        };
        s.messages(&fb, &cuts, false, &mut out);
        assert!(out.contains(&vec![0x94, 0x07, LED_ON]));
        assert!(out.contains(&vec![0x94, 0x00, LED_DIM]));
        assert!(!out.iter().any(|m| m[0] == 0x94 && m[1] == 0x14));
        // No track: the platter dot follows the jog.
        out.clear();
        let jog = ControllerState {
            jog_turns: [0.25, 0.0, 0.0, 0.0],
            ..ControllerState::default()
        };
        s.messages(&fb, &jog, false, &mut out);
        assert!(out.contains(&spinner(1, 0.25).to_vec()), "{out:?}");
        // After a reset everything goes out again.
        s.reset();
        out.clear();
        s.messages(&fb, &ctl, false, &mut out);
        assert!(out.len() >= first);
    }

    #[test]
    fn mode_lights_follow_the_rule() {
        assert_eq!(
            mode_lights(PadMode::Cue),
            [LED_ON, LED_DIM, LED_DIM, LED_DIM]
        );
        assert_eq!(
            mode_lights(PadMode::Roll),
            [LED_DIM, FLICKER, LED_DIM, LED_DIM]
        );
        assert_eq!(
            mode_lights(PadMode::Slicer),
            [LED_DIM, LED_DIM, FLICKER, LED_DIM]
        );
    }

    #[test]
    fn meter_levels() {
        assert_eq!(level(0.0), 0.0);
        assert_eq!(level(1.0), 1.0);
        assert!((level(0.5) - (1.0 - 6.02 / 24.0)).abs() < 0.01);
        assert_eq!(level(0.001), 0.0);
    }
}

#[cfg(test)]
mod flx6_tests {
    use super::*;

    /// The DDJ-FLX6's lights from the feedback: the documented notes, only
    /// changes after the first time, everything off at the end.
    #[test]
    fn ddj_flx6_lights() {
        let mut fb = Feedback::default();
        fb.decks[0].play = LED_ON;
        fb.decks[0].cue = FLICKER;
        fb.decks[0].pad_mode = PadMode::AutoLoop;
        fb.decks[0].pads[0] = LED_ON;
        fb.decks[0].pads[1] = LED_DIM;
        fb.pfl[1] = true;
        let ctl = ControllerState::default();
        let mut st = OutputState::default();
        let mut out = Vec::new();
        st.flx6_messages(&fb, &ctl, true, &mut out);
        for m in [
            [0x90, 0x0B, 0x7F], // PLAY deck 1
            [0x90, 0x0C, 0x7F], // CUE flicker, bright half
            [0x90, 0x6D, 0x7F], // BEAT LOOP mode light
            [0x90, 0x1B, 0x00], // HOT CUE mode light off
            [0x97, 0x60, 0x7F], // BEAT LOOP pad 1, channel 8
            [0x98, 0x60, 0x7F], // ... and its SHIFT channel
            [0x97, 0x61, 0x00], // dim = off
            [0x91, 0x54, 0x7F], // CH CUE 2
        ] {
            assert!(out.contains(&m.to_vec()), "{m:02X?} in {out:02X?}");
        }
        // The same again: only the flicker changes.
        out.clear();
        st.flx6_messages(&fb, &ctl, false, &mut out);
        assert_eq!(out, vec![vec![0x90, 0x0C, 0x00]]);
        // KEY SHIFT (the controller's own mode): its light, pads dark.
        let ks = ControllerState {
            key_shift_mode: [true, false, false, false],
            ..ControllerState::default()
        };
        out.clear();
        st.flx6_messages(&fb, &ks, false, &mut out);
        assert!(out.contains(&vec![0x90, 0x6F, 0x7F]), "{out:02X?}");
        assert!(out.contains(&vec![0x90, 0x6D, 0x00]), "{out:02X?}");
        // Closing: every light that is on goes off.
        out.clear();
        st.all_off(&mut out);
        assert!(out.contains(&vec![0x90, 0x0B, 0x00]));
        assert!(out.iter().all(|m| m[2] == 0));
    }
}
