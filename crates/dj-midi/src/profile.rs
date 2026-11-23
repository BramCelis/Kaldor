//! Controller profiles: which MIDI message each physical control sends, in
//! a TOML file (for the NS4FX: `profiles/numark-ns4fx.toml`, written from
//! a capture of the controller).
//!
//! ```toml
//! name = "Numark NS4FX"
//! port = "NS4FX"
//!
//! [[control]]
//! name = "play"
//! note = 0x00          # a button: note-on when pressed, note-off released
//! channel = "deck"     # MIDI channel 1-4 = decks 1-4
//!
//! [[control]]
//! name = "pad"
//! notes = [0x14, 0x1B] # eight buttons: index 0-7
//! channel = "deck+4"   # channels 5-8 = decks 1-4
//!
//! [[control]]
//! name = "pitch"
//! cc14 = [0x09, 0x29]  # 14-bit value: MSB, LSB
//! channel = "deck"
//! pickup = true        # soft takeover after a layer change
//! ```
//!
//! Channel rules: `"deck"` (1-4), `"deck+4"` (5-8 = decks 1-4), `"left+4"`
//! (5 and 7 = decks 1 and 3), `"right+4"` (6 and 8 = decks 2 and 4), `"side"`
//! (1-2 = left, right), `"strip"` (1-4 = mixer channels), `"fx"` (9-10 =
//! FX units 1-2), or a fixed channel number 1-16 (`"16"`).
//!
//! A control has exactly one of `note`, `notes`, `cc` (with `kind`
//! absolute, relative or switch) or `cc14`.

use serde::Deserialize;
use thiserror::Error;

/// Which part of the controller a message belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Deck 1-4.
    Deck(u8),
    /// Left (1) or right (2) side.
    Side(u8),
    /// Mixer channel strip 1-4.
    Strip(u8),
    /// FX unit 1-2.
    Fx(u8),
    Global,
}

/// How a control's messages are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Note-on (pressed) and note-off (released).
    Button,
    /// 7-bit controller value 0-127, read as 0.0 to 1.0.
    Absolute,
    /// 14-bit controller value from an MSB and an LSB controller.
    Absolute14 { lsb: u8 },
    /// Relative encoder, two's complement: 1 = +1, 127 = -1.
    Relative,
    /// Relative encoder centred on 64: 65 = +1, 63 = -1 (Pioneer DJ jogs).
    RelativeOffset,
    /// Sign-magnitude relative encoder: 0x01-0x3F up, 0x41-0x7F down.
    RelativeSignMagnitude,
    /// A switch with a few positions: the raw value.
    Switch,
}

/// One physical control (or a row of them).
#[derive(Clone, Debug, PartialEq)]
pub struct Control {
    pub name: String,
    pub kind: Kind,
    /// Note or controller number (the MSB for 14-bit), and how many in a
    /// row (pads: 8 notes, index 0-7).
    pub number: u8,
    pub count: u8,
    pub channel: ChannelRule,
    pub pickup: bool,
    pub invert: bool,
    /// The index its events carry instead of 0 (a single note standing
    /// for one of a row, e.g. SHIFT + LOAD of deck 3).
    pub index: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelRule {
    Deck,
    DeckPlus4,
    /// Pad section of the left decks only: channels 5 and 7 = decks 1, 3.
    LeftPlus4,
    /// Pad section of the right decks only: channels 6 and 8 = decks 2, 4.
    RightPlus4,
    Side,
    Strip,
    Fx,
    Fixed(u8),
    /// One fixed channel (1-16) addressing one unit, e.g. a filter knob of
    /// mixer channel 3 that the controller sends on its global channel.
    FixedUnit(u8, Unit),
    /// A list of channels (1-16), the first for unit 1 and so on (the
    /// DDJ-FLX6's pads: channels 8, 10, 12, 14 for decks 1-4).
    List(UnitKind, [u8; 4], u8),
}

/// What a [`ChannelRule::List`] addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnitKind {
    Deck,
    Strip,
    Side,
    Fx,
}

impl UnitKind {
    fn unit(self, n: u8) -> Unit {
        match self {
            Self::Deck => Unit::Deck(n),
            Self::Strip => Unit::Strip(n),
            Self::Side => Unit::Side(n),
            Self::Fx => Unit::Fx(n),
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "deck" => Self::Deck,
            "strip" => Self::Strip,
            "side" => Self::Side,
            "fx" => Self::Fx,
            _ => return None,
        })
    }
}

/// Which light messages a controller understands (each written from that
/// controller's documentation or a probe on it).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lights {
    /// No lights are sent.
    #[default]
    None,
    /// The Numark NS4FX's (`dj_midi::ns4fx_out`).
    Ns4fx,
    /// The Pioneer DJ DDJ-FLX6's (`dj_midi::ddj_flx6_out`).
    DdjFlx6,
}

impl ChannelRule {
    /// The 0-based MIDI channels this rule covers, each with its unit.
    pub fn channels(self) -> Vec<(u8, Unit)> {
        match self {
            Self::Deck => (1..=4).map(|d| (d - 1, Unit::Deck(d))).collect(),
            Self::DeckPlus4 => (1..=4).map(|d| (d + 3, Unit::Deck(d))).collect(),
            Self::LeftPlus4 => [1, 3].iter().map(|&d| (d + 3, Unit::Deck(d))).collect(),
            Self::RightPlus4 => [2, 4].iter().map(|&d| (d + 3, Unit::Deck(d))).collect(),
            Self::Side => (1..=2).map(|s| (s - 1, Unit::Side(s))).collect(),
            Self::Strip => (1..=4).map(|c| (c - 1, Unit::Strip(c))).collect(),
            Self::Fx => (1..=2).map(|f| (f + 7, Unit::Fx(f))).collect(),
            Self::Fixed(c) => vec![(c - 1, Unit::Global)],
            Self::FixedUnit(c, u) => vec![(c - 1, u)],
            Self::List(kind, chans, n) => chans
                .iter()
                .take(usize::from(n))
                .enumerate()
                .map(|(i, c)| (c - 1, kind.unit(i as u8 + 1)))
                .collect(),
        }
    }
}

/// A controller profile.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    /// Part of the MIDI port name to look for.
    pub port: String,
    /// Jog wheel ticks per revolution.
    pub jog_ticks_per_turn: f64,
    /// The light messages it understands.
    pub lights: Lights,
    pub controls: Vec<Control>,
}

#[derive(Debug, Error)]
pub enum ProfileError {
    #[error("profile: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("profile control \"{name}\": {msg}")]
    Control { name: String, msg: String },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    name: String,
    port: String,
    #[serde(default)]
    lights: Option<String>,
    #[serde(default)]
    jog: RawJog,
    #[serde(rename = "control", default)]
    controls: Vec<RawControl>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawJog {
    ticks_per_turn: f64,
}

impl Default for RawJog {
    fn default() -> Self {
        Self {
            ticks_per_turn: 1000.0,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawControl {
    name: String,
    channel: RawChannel,
    /// With a channel list: what its channels address (deck, strip, side,
    /// fx). With one fixed channel and `number`: the one unit addressed.
    unit: Option<String>,
    number: Option<u8>,
    index: Option<u8>,
    note: Option<u8>,
    notes: Option<[u8; 2]>,
    cc: Option<u8>,
    cc14: Option<[u8; 2]>,
    kind: Option<String>,
    #[serde(default)]
    pickup: bool,
    #[serde(default)]
    invert: bool,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawChannel {
    Name(String),
    List(Vec<u8>),
}

impl Profile {
    pub fn parse(text: &str) -> Result<Self, ProfileError> {
        let raw: RawProfile = toml::from_str(text)?;
        let lights = match raw.lights.as_deref() {
            None | Some("none") => Lights::None,
            Some("ns4fx") => Lights::Ns4fx,
            Some("ddj-flx6") => Lights::DdjFlx6,
            Some(other) => {
                return Err(ProfileError::Control {
                    name: "lights".into(),
                    msg: format!("\"{other}\": lights must be none, ns4fx or ddj-flx6"),
                });
            }
        };
        let mut controls = Vec::new();
        for c in raw.controls {
            let err = |msg: &str| ProfileError::Control {
                name: c.name.clone(),
                msg: msg.to_string(),
            };
            let unit = match c.unit.as_deref() {
                None => None,
                Some(u) => Some(
                    UnitKind::parse(u)
                        .ok_or_else(|| err("unit must be deck, strip, side or fx"))?,
                ),
            };
            let channel = match &c.channel {
                RawChannel::List(list) => {
                    let Some(kind) = unit else {
                        return Err(err("a channel list needs unit = deck, strip, side or fx"));
                    };
                    if list.is_empty()
                        || list.len() > 4
                        || list.iter().any(|c| !(1..=16).contains(c))
                    {
                        return Err(err("a channel list has 1 to 4 channels, each 1-16"));
                    }
                    let mut chans = [1u8; 4];
                    chans[..list.len()].copy_from_slice(list);
                    ChannelRule::List(kind, chans, list.len() as u8)
                }
                RawChannel::Name(name) => match name.as_str() {
                    "deck" => ChannelRule::Deck,
                    "deck+4" => ChannelRule::DeckPlus4,
                    "left+4" => ChannelRule::LeftPlus4,
                    "right+4" => ChannelRule::RightPlus4,
                    "side" => ChannelRule::Side,
                    "strip" => ChannelRule::Strip,
                    "fx" => ChannelRule::Fx,
                    n => match (n.parse::<u8>(), unit, c.number) {
                        (Ok(ch @ 1..=16), None, None) => ChannelRule::Fixed(ch),
                        (Ok(ch @ 1..=16), Some(kind), Some(k @ 1..=4)) => {
                            ChannelRule::FixedUnit(ch, kind.unit(k))
                        }
                        (Ok(1..=16), _, _) => {
                            return Err(err(
                                "a fixed channel for one unit needs both unit and number (1-4)",
                            ));
                        }
                        _ => {
                            return Err(err(
                                "channel must be deck, deck+4, left+4, right+4, side, strip, fx, 1-16 or a list of channels",
                            ));
                        }
                    },
                },
            };
            let sources = [
                c.note.is_some(),
                c.notes.is_some(),
                c.cc.is_some(),
                c.cc14.is_some(),
            ];
            if sources.iter().filter(|s| **s).count() != 1 {
                return Err(err("needs exactly one of note, notes, cc, cc14"));
            }
            let (kind, number, count) = if let Some(n) = c.note {
                (Kind::Button, n, 1)
            } else if let Some([a, b]) = c.notes {
                if b < a {
                    return Err(err("notes = [first, last]"));
                }
                (Kind::Button, a, b - a + 1)
            } else if let Some(n) = c.cc {
                let kind = match c.kind.as_deref() {
                    Some("absolute") | None => Kind::Absolute,
                    Some("relative") => Kind::Relative,
                    Some("relative-offset") => Kind::RelativeOffset,
                    Some("relative-sign-magnitude") => Kind::RelativeSignMagnitude,
                    Some("switch") => Kind::Switch,
                    Some(_) => {
                        return Err(err(
                            "kind must be absolute, relative, relative-offset, relative-sign-magnitude or switch",
                        ));
                    }
                };
                (kind, n, 1)
            } else if let Some([msb, lsb]) = c.cc14 {
                (Kind::Absolute14 { lsb }, msb, 1)
            } else {
                unreachable!("one source is set")
            };
            if [number, number.saturating_add(count - 1)]
                .iter()
                .any(|n| *n > 127)
            {
                return Err(err("note and controller numbers are 0-127"));
            }
            controls.push(Control {
                name: c.name,
                kind,
                number,
                count,
                channel,
                pickup: c.pickup,
                invert: c.invert,
                index: c.index.unwrap_or(0),
            });
        }
        Ok(Profile {
            name: raw.name,
            port: raw.port,
            jog_ticks_per_turn: raw.jog.ticks_per_turn.max(1.0),
            lights,
            controls,
        })
    }

    /// The NS4FX profile shipped with Kaldor.
    pub fn ns4fx() -> Self {
        Self::parse(NS4FX_PROFILE).expect("the bundled NS4FX profile is valid")
    }

    /// The DDJ-FLX6 profile shipped with Kaldor.
    pub fn ddj_flx6() -> Self {
        Self::parse(DDJ_FLX6_PROFILE).expect("the bundled DDJ-FLX6 profile is valid")
    }

    /// Every profile shipped with Kaldor, with its id (for the settings).
    pub fn bundled() -> Vec<(&'static str, Profile)> {
        vec![("ns4fx", Self::ns4fx()), ("ddj-flx6", Self::ddj_flx6())]
    }
}

/// `profiles/numark-ns4fx.toml`, bundled.
pub const NS4FX_PROFILE: &str = include_str!("../../../profiles/numark-ns4fx.toml");
/// `profiles/pioneer-ddj-flx6.toml`, bundled.
pub const DDJ_FLX6_PROFILE: &str = include_str!("../../../profiles/pioneer-ddj-flx6.toml");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_kinds_and_rejects_mistakes() {
        let p = Profile::parse(
            r#"
            name = "Test"
            port = "TEST"
            [jog]
            ticks_per_turn = 900
            [[control]]
            name = "play"
            note = 0x00
            channel = "deck"
            [[control]]
            name = "pad"
            notes = [0x14, 0x1B]
            channel = "deck+4"
            [[control]]
            name = "pitch"
            cc14 = [0x09, 0x29]
            channel = "deck"
            pickup = true
            [[control]]
            name = "browse"
            cc = 0x00
            kind = "relative"
            channel = "16"
            "#,
        )
        .unwrap();
        assert_eq!(p.jog_ticks_per_turn, 900.0);
        assert_eq!(p.controls[1].count, 8);
        assert_eq!(p.controls[2].kind, Kind::Absolute14 { lsb: 0x29 });
        assert!(p.controls[2].pickup);
        assert_eq!(p.controls[3].channel, ChannelRule::Fixed(16));
        assert_eq!(
            ChannelRule::DeckPlus4.channels()[0],
            (4, Unit::Deck(1)),
            "channel 5 (0-based 4) is deck 1"
        );
        for bad in [
            "name='x'\nport='y'\n[[control]]\nname='a'\nchannel='deck'\n",
            "name='x'\nport='y'\n[[control]]\nname='a'\nchannel='deck'\nnote=1\ncc=2\n",
            "name='x'\nport='y'\n[[control]]\nname='a'\nchannel='17'\nnote=1\n",
            "name='x'\nport='y'\n[[control]]\nname='a'\nchannel='deck'\ncc=1\nkind='knob'\n",
            "name='x'\nport='y'\nbogus=1\n",
        ] {
            assert!(Profile::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_bundled_ns4fx_profile_loads() {
        let p = Profile::ns4fx();
        assert_eq!(p.port, "NS4FX");
        assert!(p.controls.len() > 40);
    }
}

#[cfg(test)]
mod flx6_tests {
    use super::*;
    use crate::mapping::{Mapper, Value};

    fn events(m: &mut Mapper, msgs: &[&[u8]]) -> Vec<(String, Unit, u8, Value)> {
        let mut out = Vec::new();
        for b in msgs {
            m.feed(b, &mut out);
        }
        out.iter()
            .map(|e| (m.name(e.control).to_string(), e.unit, e.index, e.value))
            .collect()
    }

    /// Messages straight from AlphaTheta's DDJ-FLX6 MIDI list decode to the
    /// controls the profile names.
    #[test]
    fn the_bundled_ddj_flx6_profile_decodes_the_documented_messages() {
        let p = Profile::ddj_flx6();
        assert_eq!((p.port.as_str(), p.lights), ("DDJ-FLX6", Lights::DdjFlx6));
        let mut m = Mapper::new(p);
        let e = events(
            &mut m,
            &[
                &[0x90, 0x0B, 0x7F], // PLAY deck 1 (Fig. 1-1)
                &[0x93, 0x0C, 0x7F], // CUE deck 4 (Fig. 1-2)
                &[0x92, 0x3C, 0x7F], // DECK: deck 3 now controlled (1-5L)
                &[0xB1, 0x22, 0x43], // jog deck 2, +3 (1-4R)
                &[0xB0, 0x21, 0x3E], // wheel side deck 1, -2
                &[0x9B, 0x02, 0x7F], // pad 3 deck 3, HOT CUE mode (5-5)
                &[0x9E, 0x67, 0x7F], // SHIFT + pad 8 deck 4, BEAT LOOP mode
                &[0x96, 0x48, 0x7F], // LOAD (Deck3) (4-6)
                &[0x96, 0x61, 0x7F], // SHIFT + LOAD (Deck4)
                &[0xB6, 0x40, 0x7F], // BROWSE one click back (4-1)
                &[0xB6, 0x19, 0x40], // FILTER (CH3) MSB (3-7)
                &[0xB6, 0x39, 0x00], // ... LSB: centre
                &[0x95, 0x71, 0x7F], // BFX SELECT: FX2-2 (2-1)
                &[0x94, 0x1E, 0x7F], // BFX CH SELECT CH3 (2-2)
            ],
        );
        let names: Vec<(&str, Unit, u8)> = e.iter().map(|x| (x.0.as_str(), x.1, x.2)).collect();
        assert_eq!(
            names,
            vec![
                ("play", Unit::Deck(1), 0),
                ("cue", Unit::Deck(4), 0),
                ("layer", Unit::Deck(3), 0),
                ("jog", Unit::Deck(2), 0),
                ("jog_side", Unit::Deck(1), 0),
                ("pad_hot_cue", Unit::Deck(3), 2),
                ("pad_beat_loop_shift", Unit::Deck(4), 7),
                ("load", Unit::Global, 2),
                ("eject", Unit::Global, 3),
                ("browse", Unit::Global, 0),
                ("filter", Unit::Strip(3), 0),
                ("beatfx_select", Unit::Fx(2), 1),
                ("beatfx_channel", Unit::Fx(1), 2),
            ]
        );
        assert_eq!(e[3].3, Value::Rel(3));
        assert_eq!(e[4].3, Value::Rel(-2));
        assert_eq!(e[9].3, Value::Rel(-1));
        let Value::Abs(f) = e[10].3 else { panic!() };
        assert!((f - 0.5).abs() < 0.01, "{f}");
    }

    #[test]
    fn channel_lists_and_units_parse() {
        let bad = [
            "name='x'\nport='y'\n[[control]]\nname='a'\nnote=1\nchannel=[8,10]\n",
            "name='x'\nport='y'\n[[control]]\nname='a'\nnote=1\nchannel='7'\nunit='strip'\n",
            "name='x'\nport='y'\nlights='blinky'\n",
        ];
        for b in bad {
            assert!(Profile::parse(b).is_err(), "{b}");
        }
        assert_eq!(
            ChannelRule::List(UnitKind::Deck, [8, 10, 12, 14], 4).channels()[3],
            (13, Unit::Deck(4))
        );
    }
}
