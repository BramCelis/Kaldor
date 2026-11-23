//! The mapping engine: raw MIDI bytes in, named control events out, driven
//! by a [`Profile`]. Knows nothing about what the controls do; that is the
//! controller behaviour's job (for the NS4FX in `dj-ui`).

use crate::message::MidiMessage;
use crate::profile::{Kind, Profile, Unit};
use std::collections::HashMap;

/// A centre-detent knob's absolute value (`Value::Abs`, the 7-bit value
/// / 127) as -1..1 with 64 exactly at 0: 0 is -1, 127 is +1. Each side
/// has its own step (1/64 left, 1/63 right), so the centre and the step
/// count match other DJ software's filter knobs.
pub fn centred(v: f32) -> f32 {
    let raw = v * 127.0;
    if raw < 64.0 {
        (raw - 64.0) / 64.0
    } else {
        (raw - 64.0) / 63.0
    }
}

/// A control's new state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Value {
    /// A button pressed (`true`) or released.
    Press(bool),
    /// A knob or fader, 0.0 to 1.0.
    Abs(f32),
    /// An encoder or jog: steps, positive = clockwise.
    Rel(i32),
    /// A switch position (the raw value).
    Switch(u8),
}

/// One control event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Event {
    /// Index into the profile's controls (see [`Mapper::name`]).
    pub control: usize,
    pub unit: Unit,
    /// Which one of a row (pads 0-7); 0 for single controls.
    pub index: u8,
    pub value: Value,
}

#[derive(Clone, Copy, Debug)]
enum Role {
    Plain,
    Msb,
    Lsb,
}

#[derive(Clone, Copy, Debug)]
struct Binding {
    control: usize,
    unit: Unit,
    index: u8,
    role: Role,
}

/// Soft takeover state of one target.
#[derive(Clone, Copy, Debug, Default)]
struct Pickup {
    /// The value the software has.
    target: Option<f32>,
    /// The last value the control sent for this target.
    last_in: Option<f32>,
}

/// Values closer than this to the software's value are taken at once.
const PICKUP_WINDOW: f32 = 0.03;

pub struct Mapper {
    profile: Profile,
    /// (is note, channel 0-15, number) to binding.
    table: HashMap<(bool, u8, u8), Binding>,
    /// Last MSB per (channel, MSB controller) of 14-bit controls.
    msb: HashMap<(u8, u8), u8>,
    pickup: HashMap<(usize, Unit, u8), Pickup>,
    /// Soft takeover on (the default) or off for every control.
    pickup_on: bool,
}

impl Mapper {
    pub fn new(profile: Profile) -> Self {
        let mut table = HashMap::new();
        for (i, c) in profile.controls.iter().enumerate() {
            let is_note = c.kind == Kind::Button;
            for (ch, unit) in c.channel.channels() {
                for k in 0..c.count {
                    let b = |role| Binding {
                        control: i,
                        unit,
                        // A single control can say which one of a row it
                        // stands for (`index` in the profile).
                        index: if c.count == 1 { c.index } else { k },
                        role,
                    };
                    match c.kind {
                        Kind::Absolute14 { lsb } => {
                            table.insert((false, ch, c.number), b(Role::Msb));
                            table.insert((false, ch, lsb), b(Role::Lsb));
                        }
                        _ => {
                            table.insert((is_note, ch, c.number + k), b(Role::Plain));
                        }
                    }
                }
            }
        }
        Self {
            profile,
            table,
            msb: HashMap::new(),
            pickup: HashMap::new(),
            pickup_on: true,
        }
    }

    /// Soft takeover on or off for every control that has it in the
    /// profile. Off: such controls send their value at once.
    pub fn set_pickup(&mut self, on: bool) {
        self.pickup_on = on;
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }

    /// The name of control `id` (an [`Event::control`]).
    pub fn name(&self, id: usize) -> &str {
        self.profile
            .controls
            .get(id)
            .map_or("", |c| c.name.as_str())
    }

    /// The id of the control called `name`.
    pub fn id(&self, name: &str) -> Option<usize> {
        self.profile.controls.iter().position(|c| c.name == name)
    }

    /// The name of the profile control this message belongs to, if any
    /// (no side effects, unlike [`Self::feed`]).
    pub fn control_name(&self, bytes: &[u8]) -> Option<&str> {
        let key = match MidiMessage::parse(bytes) {
            MidiMessage::NoteOn { channel, note, .. }
            | MidiMessage::NoteOff { channel, note, .. } => (true, channel, note),
            MidiMessage::ControlChange {
                channel, control, ..
            } => (false, channel, control),
            _ => return None,
        };
        let b = self.table.get(&key)?;
        Some(self.name(b.control))
    }

    /// True when the profile has a control for this message (it may still
    /// produce no event: a 14-bit MSB, or a control waiting for pickup).
    pub fn knows(&self, bytes: &[u8]) -> bool {
        let key = match MidiMessage::parse(bytes) {
            MidiMessage::NoteOn { channel, note, .. }
            | MidiMessage::NoteOff { channel, note, .. } => (true, channel, note),
            MidiMessage::ControlChange {
                channel, control, ..
            } => (false, channel, control),
            _ => return false,
        };
        self.table.contains_key(&key)
    }

    /// Decodes one MIDI message into `out` (nothing for messages the
    /// profile does not know, or a pickup control that has not caught up).
    pub fn feed(&mut self, bytes: &[u8], out: &mut Vec<Event>) {
        let m = MidiMessage::parse(bytes);
        let (key, raw) = match m {
            MidiMessage::NoteOn { channel, note, .. } => ((true, channel, note), 127u8),
            MidiMessage::NoteOff { channel, note, .. } => ((true, channel, note), 0),
            MidiMessage::ControlChange {
                channel,
                control,
                value,
            } => ((false, channel, control), value),
            _ => return,
        };
        let Some(b) = self.table.get(&key).copied() else {
            return;
        };
        let Some(c) = self.profile.controls.get(b.control) else {
            return;
        };
        let value = match (c.kind, b.role) {
            (Kind::Button, _) => Value::Press(raw > 0),
            (Kind::Absolute, _) => Value::Abs(f32::from(raw) / 127.0),
            (Kind::Relative, _) => Value::Rel(if raw < 64 {
                i32::from(raw)
            } else {
                i32::from(raw) - 128
            }),
            (Kind::RelativeOffset, _) => Value::Rel(i32::from(raw) - 64),
            (Kind::RelativeSignMagnitude, _) => Value::Rel(if raw & 0x40 != 0 {
                -i32::from(raw & 0x3F)
            } else {
                i32::from(raw)
            }),
            (Kind::Switch, _) => Value::Switch(raw),
            (Kind::Absolute14 { .. }, Role::Msb) => {
                self.msb.insert((key.1, c.number), raw);
                return;
            }
            (Kind::Absolute14 { .. }, _) => {
                let msb = self.msb.get(&(key.1, c.number)).copied().unwrap_or(0);
                Value::Abs((f32::from(msb) * 128.0 + f32::from(raw)) / 16_383.0)
            }
        };
        let value = match value {
            Value::Abs(v) if c.invert => Value::Abs(1.0 - v),
            Value::Rel(r) if c.invert => Value::Rel(-r),
            v => v,
        };
        if let (true, Value::Abs(v)) = (c.pickup && self.pickup_on, value)
            && !self.picked_up(b.control, b.unit, b.index, v)
        {
            return;
        }
        out.push(Event {
            control: b.control,
            unit: b.unit,
            index: b.index,
            value,
        });
    }

    /// Soft takeover: a control that addresses a target whose value it does
    /// not match (after a LAYER change, or a change on screen) is ignored
    /// until it comes close to the target's value or moves across it.
    fn picked_up(&mut self, control: usize, unit: Unit, index: u8, v: f32) -> bool {
        let p = self.pickup.entry((control, unit, index)).or_default();
        let take = match p.target {
            None => true,
            Some(t) => {
                (v - t).abs() <= PICKUP_WINDOW
                    || p.last_in
                        .is_some_and(|l| (l - t).signum() != (v - t).signum())
            }
        };
        p.last_in = Some(v);
        if take {
            p.target = Some(v);
        }
        take
    }

    /// Tells the mapper the software's value of a pickup control (changed
    /// on screen or by sync), so the control picks it up from there.
    pub fn set_value(&mut self, control: usize, unit: Unit, index: u8, v: f32) {
        let p = self.pickup.entry((control, unit, index)).or_default();
        p.target = Some(v);
        p.last_in = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centred_knob_has_64_at_the_centre() {
        let c = |raw: u8| centred(f32::from(raw) / 127.0);
        assert_eq!(c(0), -1.0);
        assert!(c(64).abs() < 1e-6);
        assert!((c(127) - 1.0).abs() < 1e-6);
        assert!((c(63) + 1.0 / 64.0).abs() < 1e-6);
        assert!((c(65) - 1.0 / 63.0).abs() < 1e-6);
    }

    fn events(m: &mut Mapper, msgs: &[&[u8]]) -> Vec<(String, Unit, u8, Value)> {
        let mut out = Vec::new();
        for b in msgs {
            m.feed(b, &mut out);
        }
        out.iter()
            .map(|e| (m.name(e.control).to_string(), e.unit, e.index, e.value))
            .collect()
    }

    #[test]
    fn decodes_ns4fx_messages() {
        let mut m = Mapper::new(Profile::ns4fx());
        let ev = events(
            &mut m,
            &[
                &[0x90, 0x00, 0x7F], // deck 1 PLAY
                &[0x80, 0x00, 0x00], // released
                &[0x92, 0x00, 0x7F], // deck 3 PLAY (layer)
                &[0x94, 0x16, 0x7F], // deck 1 pad 3
                &[0x97, 0x1B, 0x7F], // deck 4 pad 8
                &[0xB0, 0x06, 0x02], // deck 1 jog +2
                &[0xB1, 0x06, 0x7E], // deck 2 jog -2
                &[0xBF, 0x00, 0x7F], // browse -1
                &[0xB2, 0x1E, 0x02], // strip 3 crossfader assign right
                &[0x98, 0x01, 0x7F], // FX select 2 (unit 1)
                &[0x99, 0x04, 0x7F], // FX select 5 (unit 2)
                &[0xB0, 0x1C, 0x40], // strip 1 fader
                &[0x9F, 0x03, 0x7F], // right LOAD
                &[0x91, 0x20, 0x7F], // right SHIFT
                &[0xB5, 0x77, 0x00], // unknown: nothing
            ],
        );
        let names: Vec<_> = ev.iter().map(|e| (e.0.as_str(), e.1, e.2)).collect();
        assert_eq!(
            names,
            vec![
                ("play", Unit::Deck(1), 0),
                ("play", Unit::Deck(1), 0),
                ("play", Unit::Deck(3), 0),
                ("pad", Unit::Deck(1), 2),
                ("pad", Unit::Deck(4), 7),
                ("jog", Unit::Deck(1), 0),
                ("jog", Unit::Deck(2), 0),
                ("browse", Unit::Global, 0),
                ("xfader_assign", Unit::Strip(3), 0),
                ("fx_select", Unit::Fx(1), 1),
                ("fx_select", Unit::Fx(2), 4),
                ("channel_fader", Unit::Strip(1), 0),
                ("load", Unit::Global, 1),
                ("shift", Unit::Side(2), 0),
            ]
        );
        assert_eq!(ev[0].3, Value::Press(true));
        assert_eq!(ev[1].3, Value::Press(false));
        assert_eq!(ev[5].3, Value::Rel(2));
        assert_eq!(ev[6].3, Value::Rel(-2));
        assert_eq!(ev[7].3, Value::Rel(-1));
        assert_eq!(ev[8].3, Value::Switch(2));
        assert_eq!(ev[11].3, Value::Abs(64.0 / 127.0));
    }

    #[test]
    fn pitch_is_14_bit_and_picks_up_after_a_layer_change() {
        // The NS4FX profile no longer uses pickup for the pitch (the deck
        // takes the fader over relatively); pickup itself still works.
        let mut profile = Profile::ns4fx();
        let pitch = profile
            .controls
            .iter()
            .position(|c| c.name == "pitch")
            .unwrap();
        profile.controls[pitch].pickup = true;
        let mut m = Mapper::new(profile);
        // MSB then LSB; only the LSB completes a value.
        let ev = events(&mut m, &[&[0xB0, 0x09, 0x40], &[0xB0, 0x29, 0x00]]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].3, Value::Abs(8192.0 / 16_383.0));
        // Deck 3 was last at the top; the fader (now at the centre) on
        // deck 3 is ignored until it reaches the top...
        let id = m.id("pitch").unwrap();
        m.set_value(id, Unit::Deck(3), 0, 1.0);
        let ev = events(
            &mut m,
            &[
                &[0xB2, 0x09, 0x40],
                &[0xB2, 0x29, 0x00],
                &[0xB2, 0x09, 0x60],
                &[0xB2, 0x29, 0x00],
            ],
        );
        assert!(ev.is_empty(), "{ev:?}");
        let ev = events(&mut m, &[&[0xB2, 0x09, 0x7F], &[0xB2, 0x29, 0x7F]]);
        assert_eq!(ev.len(), 1, "taken near the target");
        // ...or until it moves across it.
        m.set_value(id, Unit::Deck(3), 0, 0.5);
        let ev = events(
            &mut m,
            &[
                &[0xB2, 0x09, 0x10],
                &[0xB2, 0x29, 0x00],
                &[0xB2, 0x09, 0x70],
                &[0xB2, 0x29, 0x00],
            ],
        );
        assert_eq!(ev.len(), 1, "crossed from below to above");
    }

    #[test]
    fn soft_takeover_can_be_turned_off() {
        let mut profile = Profile::ns4fx();
        let pitch = profile
            .controls
            .iter()
            .position(|c| c.name == "pitch")
            .unwrap();
        profile.controls[pitch].pickup = true;
        let mut m = Mapper::new(profile);
        let id = m.id("pitch").unwrap();
        m.set_value(id, Unit::Deck(3), 0, 1.0);
        m.set_pickup(false);
        // Far from the software's value, but sent at once.
        let ev = events(&mut m, &[&[0xB2, 0x09, 0x40], &[0xB2, 0x29, 0x00]]);
        assert_eq!(ev.len(), 1, "{ev:?}");
    }
}
