//! Messages to the NS4FX: LEDs, the jog wheel displays and meters.
//!
//! Written from a description of the protocol (the Mixxx NS4FX mapping was
//! read to understand it; no code taken) and checked on the real controller
//! with an output probe. Only what
//! the probe confirmed is used by the app.
//!
//! Decks are 1-4 here; MIDI channels are 0-based in the status bytes.

/// Start of every display message.
const DISPLAY: [u8; 4] = [0xF0, 0x00, 0x20, 0x7F];

/// Display commands.
const CMD_BPM: u8 = 0x01;
const CMD_PITCH: u8 = 0x02;
const CMD_DURATION: u8 = 0x03;
const CMD_TIME: u8 = 0x04;

/// Takes the controller out of its demo light show (sent at start).
pub const LEAVE_DEMO: [u8; 14] = [
    0xF0, 0x00, 0x01, 0x3F, 0x7F, 0x3A, 0x60, 0x00, 0x04, 0x04, 0x01, 0x00, 0x00, 0xF7,
];
/// Asks the controller for the positions of all its controls. Verified on
/// an NS4FX: it answers at once
/// with one message per knob, fader and switch (the pitch faders of the
/// active decks as 14-bit pairs) and a release for every button. Not in the
/// Mixxx mapping.
pub const STATE_REQUEST: [u8; 7] = [0xF0, 0x00, 0x20, 0x7F, 0x03, 0x01, 0xF7];
/// Hands the controller back (sent at exit).
pub const GOODBYE: [u8; 6] = [0xF0, 0x00, 0x20, 0x7F, 0x02, 0xF7];

/// LED brightness values.
pub const LED_ON: u8 = 0x7F;
pub const LED_DIM: u8 = 0x01;
pub const LED_OFF: u8 = 0x00;

fn ch(deck: u8) -> u8 {
    deck.clamp(1, 4) - 1
}

/// A number as 8 nibbles: a sign nibble (8 = positive or zero, 7 =
/// negative), then the magnitude in 7 nibbles, most significant first.
pub fn nibbles8(v: i64) -> [u8; 8] {
    let sign = if v < 0 { 0x07 } else { 0x08 };
    let m = v.unsigned_abs().min(0x0FFF_FFFF);
    let mut out = [sign, 0, 0, 0, 0, 0, 0, 0];
    for (i, o) in out.iter_mut().enumerate().skip(1) {
        *o = ((m >> (4 * (7 - i))) & 0x0F) as u8;
    }
    out
}

fn display(deck: u8, cmd: u8, payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(payload.len() + 7);
    m.extend_from_slice(&DISPLAY);
    m.push(deck.clamp(1, 4));
    m.push(cmd);
    m.extend_from_slice(payload);
    m.push(0xF7);
    m
}

/// The BPM on a deck's display: hundredths of a BPM in 6 nibbles.
pub fn display_bpm(deck: u8, bpm: f64) -> Vec<u8> {
    let v = (bpm.max(0.0) * 100.0).round() as i64;
    display(deck, CMD_BPM, &nibbles8(v)[2..])
}

/// The pitch on a deck's display, in percent (+3.25 = 3.25 % faster):
/// hundredths of a percent, offset by 0x800000, in 6 nibbles.
pub fn display_pitch(deck: u8, percent: f64) -> Vec<u8> {
    let v = (percent * 100.0).round() as i64;
    let x = (0x80_0000 + v).clamp(0, 0xFF_FFFF) as u32;
    let n: Vec<u8> = (0..6)
        .map(|i| ((x >> (4 * (5 - i))) & 0x0F) as u8)
        .collect();
    display(deck, CMD_PITCH, &n)
}

/// The track length (milliseconds) for the deck's display.
pub fn display_duration(deck: u8, ms: u64) -> Vec<u8> {
    let v = ms.max(1) as i64 - 1;
    display(deck, CMD_DURATION, &nibbles8(v))
}

/// The play position (milliseconds) for the deck's display.
pub fn display_time(deck: u8, ms: u64) -> Vec<u8> {
    display(deck, CMD_TIME, &nibbles8(ms as i64))
}

/// A button LED: note-on with the brightness as velocity.
pub fn led(channel: u8, note: u8, value: u8) -> [u8; 3] {
    [0x90 | (channel & 0x0F), note & 0x7F, value & 0x7F]
}

/// The pitch range shown on the display (percent, up to 99).
pub fn display_range(deck: u8, percent: u8) -> [u8; 3] {
    [0x90 | ch(deck), 0x0E, percent.min(99)]
}

/// The ring around the display: track position 0.0-1.0.
pub fn position_ring(deck: u8, fraction: f64) -> [u8; 3] {
    [
        0xB0 | ch(deck),
        0x3F,
        (fraction.clamp(0.0, 1.0) * 52.0).round() as u8,
    ]
}

/// The platter dot: phase 0.0-1.0 of one 33 1/3 rpm turn.
pub fn spinner(deck: u8, phase: f64) -> [u8; 3] {
    [
        0xB0 | ch(deck),
        0x06,
        64 + (phase.rem_euclid(1.0) * 51.0).round() as u8,
    ]
}

/// A deck's level meter: 0.0-1.0.
pub fn vu(deck: u8, level: f32) -> [u8; 3] {
    [
        0xB0 | ch(deck),
        0x1F,
        (level.clamp(0.0, 1.0) * 81.0).round() as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_sign_and_seven_nibbles() {
        assert_eq!(nibbles8(0), [8, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(nibbles8(12_800), [8, 0, 0, 0, 3, 2, 0, 0]);
        assert_eq!(nibbles8(-1), [7, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(nibbles8(245_000), [8, 0, 0, 3, 11, 13, 0, 8]);
    }

    #[test]
    fn display_messages() {
        // 128.00 BPM on deck 1: 12800 = 0x3200 in six nibbles.
        assert_eq!(
            display_bpm(1, 128.0),
            vec![
                0xF0, 0x00, 0x20, 0x7F, 0x01, 0x01, 0x00, 0x00, 0x03, 0x02, 0x00, 0x00, 0xF7
            ]
        );
        // Pitch 0 is the offset alone; -1.50 % is 0x800000 - 150.
        assert_eq!(
            &display_pitch(2, 0.0)[4..13],
            &[2, 2, 8, 0, 0, 0, 0, 0, 0xF7]
        );
        let x = 0x80_0000 - 150;
        let n: Vec<u8> = (0..6).map(|i| ((x >> (4 * (5 - i))) & 0xF) as u8).collect();
        assert_eq!(&display_pitch(1, -1.5)[6..12], &n[..]);
        assert_eq!(display_time(3, 61_500)[4..6], [3, 4]);
        assert_eq!(display_duration(4, 1)[6..14], [8, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(position_ring(1, 0.5), [0xB0, 0x3F, 26]);
        assert_eq!(spinner(2, 0.0), [0xB1, 0x06, 64]);
        assert_eq!(spinner(2, 0.999), [0xB1, 0x06, 115]);
        assert_eq!(vu(4, 1.0), [0xB3, 0x1F, 81]);
        assert_eq!(display_range(1, 8), [0x90, 0x0E, 8]);
        assert_eq!(led(4, 0x14, LED_ON), [0x94, 0x14, 0x7F]);
    }
}
