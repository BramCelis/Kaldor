//! Readable summaries of what a control sent (for the capture report).
//! Describes only what was received; it does not interpret the values.

use crate::message::{MessageKey, MidiMessage, hex};

/// What one message key (type, channel, number) sent during a step.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeySummary {
    pub count: usize,
    /// Note-on and note-off counts (notes only).
    pub ons: usize,
    pub offs: usize,
    /// Every value seen, in order of first appearance.
    pub values: Vec<u16>,
    pub min: u16,
    pub max: u16,
    /// The first and the last value (where a fader started and ended).
    pub first: u16,
    pub last: u16,
}

/// Summary of a list of raw messages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Summary {
    /// Keys in the order they first arrived (pads pressed 1 to 8 come
    /// out in pad order).
    pub keys: Vec<(MessageKey, KeySummary)>,
    /// System exclusive and other messages, each distinct one once, with
    /// how often it came.
    pub system: Vec<(Vec<u8>, usize)>,
}

impl Summary {
    pub fn of<'a>(messages: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut s = Summary::default();
        for bytes in messages {
            let m = MidiMessage::parse(bytes);
            match (m.key(), m.value()) {
                (Some(k), Some(v)) => {
                    let i = match s.keys.iter().position(|(x, _)| *x == k) {
                        Some(i) => i,
                        None => {
                            s.keys.push((
                                k,
                                KeySummary {
                                    min: v,
                                    max: v,
                                    first: v,
                                    ..KeySummary::default()
                                },
                            ));
                            s.keys.len() - 1
                        }
                    };
                    let e = &mut s.keys[i].1;
                    e.count += 1;
                    match m {
                        MidiMessage::NoteOn { .. } => e.ons += 1,
                        MidiMessage::NoteOff { .. } => e.offs += 1,
                        _ => {}
                    }
                    if !e.values.contains(&v) {
                        e.values.push(v);
                    }
                    e.min = e.min.min(v);
                    e.max = e.max.max(v);
                    e.last = v;
                }
                _ => match s.system.iter_mut().find(|(b, _)| b == bytes) {
                    Some((_, n)) => *n += 1,
                    None => s.system.push((bytes.to_vec(), 1)),
                },
            }
        }
        s
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.system.is_empty()
    }

    /// One line per key and per system message, for example
    /// `note ch1 0x0B: on ×2 (vel 127), off ×2` or
    /// `CC ch1 0x13: 120 messages, values 0..127`.
    pub fn lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (k, s) in &self.keys {
            let vals = if s.values.len() <= 6 {
                let mut v = s.values.clone();
                v.sort_unstable();
                v.iter().map(u16::to_string).collect::<Vec<_>>().join(", ")
            } else {
                format!("{}..{} ({} different)", s.min, s.max, s.values.len())
            };
            let line = if s.ons + s.offs == s.count && s.count > 0 {
                let on_vals: Vec<_> = s.values.iter().filter(|v| **v > 0).collect();
                let vel = if on_vals.len() == 1 {
                    format!(" (vel {})", on_vals[0])
                } else if on_vals.is_empty() {
                    String::new()
                } else {
                    format!(" (vel {vals})")
                };
                format!("{k}: on ×{}{vel}, off ×{}", s.ons, s.offs)
            } else {
                let what = if s.count == 1 { "message" } else { "messages" };
                if s.count > 1 && s.values.len() > 2 {
                    format!(
                        "{k}: {} {what}, values {vals}, first {}, last {}",
                        s.count, s.first, s.last
                    )
                } else {
                    format!("{k}: {} {what}, values {vals}", s.count)
                }
            };
            out.push(line);
        }
        for (b, n) in &self.system {
            let m = MidiMessage::parse(b);
            let kind = if matches!(m, MidiMessage::SysEx(_)) {
                "sysex"
            } else {
                "system"
            };
            out.push(format!("{kind} ×{n}: {}", hex(b)));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarises_buttons_knobs_and_sysex() {
        let msgs: Vec<Vec<u8>> = vec![
            vec![0x90, 0x0B, 0x7F],
            vec![0x80, 0x0B, 0x00],
            vec![0x90, 0x0B, 0x7F],
            vec![0x90, 0x0B, 0x00],
            vec![0xB0, 0x13, 0x00],
            vec![0xB0, 0x13, 0x40],
            vec![0xB0, 0x13, 0x7F],
            vec![0xF0, 0x7E, 0xF7],
            vec![0xF0, 0x7E, 0xF7],
        ];
        let s = Summary::of(msgs.iter().map(Vec::as_slice));
        let lines = s.lines();
        assert_eq!(lines[0], "note ch1 0x0B: on ×2 (vel 127), off ×2");
        assert_eq!(
            lines[1],
            "CC ch1 0x13: 3 messages, values 0, 64, 127, first 0, last 127"
        );
        assert_eq!(lines[2], "sysex ×2: F0 7E F7");
        let many: Vec<Vec<u8>> = (0..100u8).map(|v| vec![0xB1, 0x06, v]).collect();
        let s = Summary::of(many.iter().map(Vec::as_slice));
        assert_eq!(
            s.lines()[0],
            "CC ch2 0x06: 100 messages, values 0..99 (100 different), first 0, last 99"
        );
    }
}
