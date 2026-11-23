//! Capture files: raw MIDI input with timestamps, grouped in labelled steps
//! ("Deck 1: PLAY button"), so a capture taken on the hardware can be read
//! back and replayed in tests.
//!
//! The format is plain text, one item per line:
//!
//! ```text
//! # djapp MIDI capture v1
//! # port: NS4FX
//! step 12 Deck 1 (left): PLAY button
//! note pressed twice
//!   1532.250 90 0B 7F
//!   1612.004 80 0B 00
//! step 13 Deck 1 (left): CUE button
//! skip
//! ```
//!
//! `#` lines are comments, `step` starts a step (number, then the label),
//! `note` is free text typed during the capture, `skip` marks a step the
//! user skipped, `redo` empties the current step (it is recorded again),
//! and indented lines are messages: milliseconds since the capture started,
//! then the bytes in hex. A step number that comes again continues that
//! step.

use std::io::{self, BufRead, Write};
use thiserror::Error;

/// First line of every capture file. It still says djapp (the name before
/// Kaldor) because the captures in `tests/captures/` start with it.
pub const HEADER: &str = "# djapp MIDI capture v1";

/// One step of a capture: a control and what it sent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Step {
    pub number: u32,
    pub label: String,
    /// Notes typed during the capture, one per line.
    pub notes: Vec<String>,
    pub skipped: bool,
    /// (milliseconds since the start, raw bytes).
    pub messages: Vec<(f64, Vec<u8>)>,
}

/// A whole capture file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Capture {
    /// `# key: value` comments from the top of the file.
    pub info: Vec<(String, String)>,
    /// Messages that arrived before the first step (step 0).
    pub steps: Vec<Step>,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("line {line}: {msg}")]
    Parse { line: usize, msg: String },
    #[error(transparent)]
    Io(#[from] io::Error),
}

impl Capture {
    /// Reads a capture file. A step number that comes again (the user went
    /// back, or used an extra step in between) continues that step; `redo`
    /// empties the current step first.
    pub fn read(r: impl BufRead) -> Result<Self, CaptureError> {
        let mut c = Capture::default();
        let mut cur: Option<usize> = None;
        for (i, line) in r.lines().enumerate() {
            let line = line?;
            let n = i + 1;
            let err = |msg: &str| CaptureError::Parse {
                line: n,
                msg: msg.to_string(),
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix('#') {
                if let Some((k, v)) = rest.trim().split_once(": ") {
                    c.info.push((k.trim().to_string(), v.trim().to_string()));
                }
                continue;
            }
            // Messages before the first step go to step 0.
            let mut here = |c: &mut Capture| -> usize {
                *cur.get_or_insert_with(|| c.step_index(0, "(before the first step)"))
            };
            if line.starts_with(char::is_whitespace) {
                let mut parts = line.split_whitespace();
                let t: f64 = parts
                    .next()
                    .and_then(|t| t.parse().ok())
                    .ok_or_else(|| err("bad time"))?;
                let bytes = parts
                    .map(|h| u8::from_str_radix(h, 16))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| err("bad hex byte"))?;
                if bytes.is_empty() {
                    return Err(err("message without bytes"));
                }
                let k = here(&mut c);
                c.steps[k].messages.push((t, bytes));
            } else if let Some(rest) = line.strip_prefix("step ") {
                let (num, label) = rest.split_once(' ').unwrap_or((rest, ""));
                let number = num.parse().map_err(|_| err("bad step number"))?;
                cur = Some(c.step_index(number, label));
            } else if let Some(rest) = line.strip_prefix("note ") {
                let k = here(&mut c);
                c.steps[k].notes.push(rest.to_string());
            } else if line.trim() == "skip" {
                let k = here(&mut c);
                c.steps[k].skipped = true;
            } else if line.trim() == "redo" {
                let k = here(&mut c);
                let s = &mut c.steps[k];
                s.messages.clear();
                s.notes.clear();
                s.skipped = false;
            } else {
                return Err(err("unknown line"));
            }
        }
        Ok(c)
    }

    /// Index of step `number`, added (with `label`) if it is new.
    fn step_index(&mut self, number: u32, label: &str) -> usize {
        if let Some(i) = self.steps.iter().position(|s| s.number == number) {
            return i;
        }
        self.steps.push(Step {
            number,
            label: label.to_string(),
            ..Step::default()
        });
        self.steps.len() - 1
    }

    /// All messages in order, with their step number.
    pub fn messages(&self) -> impl Iterator<Item = (u32, f64, &[u8])> {
        self.steps.iter().flat_map(|s| {
            s.messages
                .iter()
                .map(move |(t, b)| (s.number, *t, b.as_slice()))
        })
    }
}

/// Writes a capture file line by line (flushed after every line, so an
/// interrupted capture keeps everything up to that point).
pub struct CaptureWriter<W: Write> {
    w: W,
}

impl<W: Write> CaptureWriter<W> {
    /// Starts a file with the header and `# key: value` lines.
    pub fn new(mut w: W, info: &[(&str, &str)]) -> io::Result<Self> {
        writeln!(w, "{HEADER}")?;
        for (k, v) in info {
            writeln!(w, "# {k}: {}", one_line(v))?;
        }
        w.flush()?;
        Ok(Self { w })
    }

    /// Continues an existing capture file (no header).
    pub fn continue_file(w: W) -> Self {
        Self { w }
    }

    pub fn step(&mut self, number: u32, label: &str) -> io::Result<()> {
        writeln!(self.w, "step {number} {}", one_line(label))?;
        self.w.flush()
    }

    pub fn note(&mut self, text: &str) -> io::Result<()> {
        writeln!(self.w, "note {}", one_line(text))?;
        self.w.flush()
    }

    pub fn skip(&mut self) -> io::Result<()> {
        writeln!(self.w, "skip")?;
        self.w.flush()
    }

    /// Empties the current step (the user records it again).
    pub fn redo(&mut self) -> io::Result<()> {
        writeln!(self.w, "redo")?;
        self.w.flush()
    }

    pub fn message(&mut self, ms: f64, bytes: &[u8]) -> io::Result<()> {
        writeln!(self.w, "  {ms:.3} {}", crate::message::hex(bytes))?;
        self.w.flush()
    }

    pub fn into_inner(self) -> W {
        self.w
    }
}

fn one_line(s: &str) -> String {
    s.replace(['\r', '\n'], " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_reads_back() {
        let mut w =
            CaptureWriter::new(Vec::new(), &[("port", "NS4FX"), ("tool", "midi-monitor")]).unwrap();
        w.message(5.0, &[0xF0, 0x01, 0xF7]).unwrap();
        w.step(1, "Deck 1 (left): PLAY button").unwrap();
        w.note("pressed twice").unwrap();
        w.message(1532.25, &[0x90, 0x0B, 0x7F]).unwrap();
        w.message(1612.0, &[0x80, 0x0B, 0x00]).unwrap();
        w.step(2, "Deck 1 (left): CUE button").unwrap();
        w.skip().unwrap();
        // Back to step 1: redo it, then an extra step, then step 1 again.
        w.step(1, "Deck 1 (left): PLAY button").unwrap();
        w.redo().unwrap();
        w.message(2000.0, &[0x90, 0x0B, 0x7F]).unwrap();
        w.step(1001, "Extra: censor").unwrap();
        w.message(2100.0, &[0x90, 0x10, 0x7F]).unwrap();
        w.step(1, "Deck 1 (left): PLAY button").unwrap();
        w.message(2200.0, &[0x80, 0x0B, 0x00]).unwrap();
        let bytes = w.into_inner();
        let c = Capture::read(&bytes[..]).unwrap();
        assert_eq!(c.info[0], ("port".into(), "NS4FX".into()));
        assert_eq!(c.steps.len(), 4);
        assert_eq!(c.steps[0].number, 0);
        assert_eq!(c.steps[0].messages, vec![(5.0, vec![0xF0, 0x01, 0xF7])]);
        assert_eq!(c.steps[1].label, "Deck 1 (left): PLAY button");
        // Redone: the first two messages and the note are gone, and the
        // step continued after the extra step.
        assert!(c.steps[1].notes.is_empty());
        assert_eq!(
            c.steps[1].messages,
            vec![
                (2000.0, vec![0x90, 0x0B, 0x7F]),
                (2200.0, vec![0x80, 0x0B, 0x00])
            ]
        );
        assert!(c.steps[2].skipped && c.steps[2].messages.is_empty());
        assert_eq!(c.steps[3].label, "Extra: censor");
        let all: Vec<_> = c.messages().map(|(s, _, b)| (s, b.to_vec())).collect();
        assert_eq!(all[1], (1, vec![0x90, 0x0B, 0x7F]));
    }

    #[test]
    fn reports_bad_lines() {
        let e = Capture::read("step 1 x\n  1.0 9G 00\n".as_bytes()).unwrap_err();
        assert!(e.to_string().contains("line 2"), "{e}");
        assert!(Capture::read("hello\n".as_bytes()).is_err());
    }
}
