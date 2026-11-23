//! Decoding raw MIDI bytes into messages (MIDI 1.0 channel voice, system
//! exclusive and system messages).

use std::fmt;

/// One decoded MIDI message. Channels are 0-15 here and shown as 1-16.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    /// A note-on with velocity 0 is decoded as [`MidiMessage::NoteOff`]
    /// (the usual running-status shorthand), with `velocity` 0.
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    PolyPressure {
        channel: u8,
        note: u8,
        pressure: u8,
    },
    ControlChange {
        channel: u8,
        control: u8,
        value: u8,
    },
    ProgramChange {
        channel: u8,
        program: u8,
    },
    ChannelPressure {
        channel: u8,
        pressure: u8,
    },
    /// 14-bit value, 0..16383, centre 8192.
    PitchBend {
        channel: u8,
        value: u16,
    },
    /// The whole message including the leading 0xF0 and the final 0xF7.
    SysEx(Vec<u8>),
    /// System common and real-time messages, and anything malformed.
    Other(Vec<u8>),
}

impl MidiMessage {
    /// Decodes one complete message as delivered by the MIDI driver.
    pub fn parse(bytes: &[u8]) -> Self {
        let Some(&status) = bytes.first() else {
            return Self::Other(Vec::new());
        };
        let channel = status & 0x0F;
        let d = |i: usize| bytes.get(i).copied().filter(|b| *b < 0x80);
        let two = || Some((d(1)?, d(2)?));
        let parsed = match status & 0xF0 {
            0x80 => two().map(|(note, velocity)| Self::NoteOff {
                channel,
                note,
                velocity,
            }),
            0x90 => two().map(|(note, velocity)| {
                if velocity == 0 {
                    Self::NoteOff {
                        channel,
                        note,
                        velocity,
                    }
                } else {
                    Self::NoteOn {
                        channel,
                        note,
                        velocity,
                    }
                }
            }),
            0xA0 => two().map(|(note, pressure)| Self::PolyPressure {
                channel,
                note,
                pressure,
            }),
            0xB0 => two().map(|(control, value)| Self::ControlChange {
                channel,
                control,
                value,
            }),
            0xC0 => d(1).map(|program| Self::ProgramChange { channel, program }),
            0xD0 => d(1).map(|pressure| Self::ChannelPressure { channel, pressure }),
            0xE0 => two().map(|(lsb, msb)| Self::PitchBend {
                channel,
                value: u16::from(msb) << 7 | u16::from(lsb),
            }),
            _ if status == 0xF0 => Some(Self::SysEx(bytes.to_vec())),
            _ => None,
        };
        parsed.unwrap_or_else(|| Self::Other(bytes.to_vec()))
    }

    /// The MIDI channel (0-15) of a channel message.
    pub fn channel(&self) -> Option<u8> {
        match *self {
            Self::NoteOn { channel, .. }
            | Self::NoteOff { channel, .. }
            | Self::PolyPressure { channel, .. }
            | Self::ControlChange { channel, .. }
            | Self::ProgramChange { channel, .. }
            | Self::ChannelPressure { channel, .. }
            | Self::PitchBend { channel, .. } => Some(channel),
            Self::SysEx(_) | Self::Other(_) => None,
        }
    }

    /// What the message is about, without its value: the same control sends
    /// messages with the same key. Note-on and note-off of a note share a
    /// key. `None` for system messages.
    pub fn key(&self) -> Option<MessageKey> {
        let (kind, channel, number) = match *self {
            Self::NoteOn { channel, note, .. } | Self::NoteOff { channel, note, .. } => {
                (KeyKind::Note, channel, note)
            }
            Self::PolyPressure { channel, note, .. } => (KeyKind::PolyPressure, channel, note),
            Self::ControlChange {
                channel, control, ..
            } => (KeyKind::Control, channel, control),
            Self::ProgramChange { channel, .. } => (KeyKind::Program, channel, 0),
            Self::ChannelPressure { channel, .. } => (KeyKind::ChannelPressure, channel, 0),
            Self::PitchBend { channel, .. } => (KeyKind::PitchBend, channel, 0),
            Self::SysEx(_) | Self::Other(_) => return None,
        };
        Some(MessageKey {
            kind,
            channel,
            number,
        })
    }

    /// The value part: velocity (0 for a note-off), controller value,
    /// pressure, program, or the 14-bit pitch bend value.
    pub fn value(&self) -> Option<u16> {
        match *self {
            Self::NoteOn { velocity, .. } => Some(u16::from(velocity)),
            Self::NoteOff { .. } => Some(0),
            Self::PolyPressure { pressure, .. } | Self::ChannelPressure { pressure, .. } => {
                Some(u16::from(pressure))
            }
            Self::ControlChange { value, .. } => Some(u16::from(value)),
            Self::ProgramChange { program, .. } => Some(u16::from(program)),
            Self::PitchBend { value, .. } => Some(value),
            Self::SysEx(_) | Self::Other(_) => None,
        }
    }
}

/// Message type of a [`MessageKey`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyKind {
    Note,
    PolyPressure,
    Control,
    Program,
    ChannelPressure,
    PitchBend,
}

/// Type, channel (0-15) and note or controller number of a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageKey {
    pub kind: KeyKind,
    pub channel: u8,
    pub number: u8,
}

impl fmt::Display for MessageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ch = self.channel + 1;
        match self.kind {
            KeyKind::Note => write!(f, "note ch{ch} 0x{:02X}", self.number),
            KeyKind::PolyPressure => write!(f, "poly-pressure ch{ch} 0x{:02X}", self.number),
            KeyKind::Control => write!(f, "CC ch{ch} 0x{:02X}", self.number),
            KeyKind::Program => write!(f, "program ch{ch}"),
            KeyKind::ChannelPressure => write!(f, "pressure ch{ch}"),
            KeyKind::PitchBend => write!(f, "pitch-bend ch{ch}"),
        }
    }
}

impl fmt::Display for MidiMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoteOn {
                channel,
                note,
                velocity,
            } => write!(
                f,
                "note-on   ch{:<2} note 0x{note:02X} ({note:3}) vel {velocity}",
                channel + 1
            ),
            Self::NoteOff {
                channel,
                note,
                velocity,
            } => write!(
                f,
                "note-off  ch{:<2} note 0x{note:02X} ({note:3}) vel {velocity}",
                channel + 1
            ),
            Self::PolyPressure {
                channel,
                note,
                pressure,
            } => write!(
                f,
                "poly-pr   ch{:<2} note 0x{note:02X} ({note:3}) val {pressure}",
                channel + 1
            ),
            Self::ControlChange {
                channel,
                control,
                value,
            } => write!(
                f,
                "CC        ch{:<2} cc   0x{control:02X} ({control:3}) val {value}",
                channel + 1
            ),
            Self::ProgramChange { channel, program } => {
                write!(f, "program   ch{:<2} {program}", channel + 1)
            }
            Self::ChannelPressure { channel, pressure } => {
                write!(f, "pressure  ch{:<2} {pressure}", channel + 1)
            }
            Self::PitchBend { channel, value } => {
                write!(f, "pitchbend ch{:<2} {value}", channel + 1)
            }
            Self::SysEx(b) => write!(f, "sysex     {} bytes: {}", b.len(), hex(b)),
            Self::Other(b) => write!(f, "other     {}", hex(b)),
        }
    }
}

/// Bytes as upper-case hex pairs separated by spaces.
pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn decodes_channel_messages() {
        assert_eq!(
            MidiMessage::parse(&[0x91, 0x0B, 0x7F]),
            MidiMessage::NoteOn {
                channel: 1,
                note: 0x0B,
                velocity: 127
            }
        );
        // Note-on with velocity 0 is a note-off.
        assert_eq!(
            MidiMessage::parse(&[0x91, 0x0B, 0x00]),
            MidiMessage::NoteOff {
                channel: 1,
                note: 0x0B,
                velocity: 0
            }
        );
        assert_eq!(
            MidiMessage::parse(&[0xB0, 0x13, 0x40]),
            MidiMessage::ControlChange {
                channel: 0,
                control: 0x13,
                value: 64
            }
        );
        assert_eq!(
            MidiMessage::parse(&[0xE3, 0x00, 0x40]),
            MidiMessage::PitchBend {
                channel: 3,
                value: 8192
            }
        );
        assert_eq!(
            MidiMessage::parse(&[0xF0, 0x00, 0x20, 0x7F, 0xF7]),
            MidiMessage::SysEx(vec![0xF0, 0x00, 0x20, 0x7F, 0xF7])
        );
        // Truncated or out-of-range data bytes are kept as they are.
        assert_eq!(
            MidiMessage::parse(&[0x90, 0x0B]),
            MidiMessage::Other(vec![0x90, 0x0B])
        );
        assert_eq!(
            MidiMessage::parse(&[0xB0, 0x80, 0x00]),
            MidiMessage::Other(vec![0xB0, 0x80, 0x00])
        );
        assert_eq!(MidiMessage::parse(&[0xF8]), MidiMessage::Other(vec![0xF8]));
    }

    #[test]
    fn keys_group_on_and_off_and_ignore_values() {
        let on = MidiMessage::parse(&[0x90, 0x0B, 0x7F]);
        let off = MidiMessage::parse(&[0x80, 0x0B, 0x00]);
        assert_eq!(on.key(), off.key());
        assert_eq!(on.value(), Some(127));
        assert_eq!(off.value(), Some(0));
        let cc = MidiMessage::parse(&[0xB2, 0x07, 0x10]);
        assert_eq!(cc.key().unwrap().to_string(), "CC ch3 0x07");
        assert_eq!(cc.channel(), Some(2));
    }

    proptest! {
        #[test]
        fn parsing_never_panics_and_keeps_the_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..8)) {
            let m = MidiMessage::parse(&bytes);
            let _ = m.to_string();
            if let MidiMessage::Other(b) | MidiMessage::SysEx(b) = &m {
                prop_assert_eq!(b, &bytes);
            }
        }
    }
}
