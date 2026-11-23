//! Messages to the Pioneer DJ DDJ-FLX6: button and pad lights, the channel
//! level meters and the jog illumination.
//!
//! Written from the MIDI-OUT columns of AlphaTheta's "DDJ-FLX6 List of
//! MIDI messages":
//! a light is the same note as its button's message, 0x7F on, 0x00 off.
//! Not yet checked on a controller.
//!
//! Decks are 1-4 here; MIDI channels are 0-based in the status bytes.

/// Light values (the document lists only on and off).
pub const ON: u8 = 0x7F;
pub const OFF: u8 = 0x00;

/// Deck buttons with a light (Fig. 1-1 to 1-16, MIDI-OUT on the deck's
/// channel).
pub const PLAY: u8 = 0x0B;
pub const CUE: u8 = 0x0C;
pub const LOOP_IN: u8 = 0x10;
pub const LOOP_OUT: u8 = 0x11;
pub const SYNC: u8 = 0x58;
pub const MASTER: u8 = 0x5C;
/// CH CUE of a mixer channel (Fig. 3-8).
pub const CH_CUE: u8 = 0x54;

/// The lights of the eight pad modes (Fig. 5-1 to 5-4, MIDI-OUT). Note
/// that the document gives SHIFT + PAD FX (KEY SHIFT) the light 0x6F and
/// SHIFT + SAMPLER (SAMPLE SCRATCH) 0x6B, the other way round from their
/// buttons' messages; followed as written until checked.
pub mod mode_light {
    pub const HOT_CUE: u8 = 0x1B;
    pub const KEYBOARD: u8 = 0x69;
    pub const PAD_FX: u8 = 0x1E;
    pub const KEY_SHIFT: u8 = 0x6F;
    pub const BEAT_JUMP: u8 = 0x20;
    pub const BEAT_LOOP: u8 = 0x6D;
    pub const SAMPLER: u8 = 0x22;
    pub const SAMPLE_SCRATCH: u8 = 0x6B;
    pub const ALL: [u8; 8] = [
        HOT_CUE,
        KEYBOARD,
        PAD_FX,
        KEY_SHIFT,
        BEAT_JUMP,
        BEAT_LOOP,
        SAMPLER,
        SAMPLE_SCRATCH,
    ];
}

/// The first pad note of each pad mode (Fig. 5-5); pad k is this + k.
pub mod pad_base {
    pub const HOT_CUE: u8 = 0x00;
    pub const PAD_FX: u8 = 0x10;
    pub const BEAT_JUMP: u8 = 0x20;
    pub const SAMPLER: u8 = 0x30;
    pub const KEYBOARD: u8 = 0x40;
    pub const KEY_SHIFT: u8 = 0x50;
    pub const BEAT_LOOP: u8 = 0x60;
    pub const SAMPLE_SCRATCH: u8 = 0x70;
}

fn ch(deck: u8) -> u8 {
    deck.clamp(1, 4) - 1
}

/// A light on a deck's channel (also the mixer channel of the same
/// number).
pub fn deck_light(deck: u8, note: u8, on: bool) -> [u8; 3] {
    [0x90 | ch(deck), note, if on { ON } else { OFF }]
}

/// A pad light: channels 8, 10, 12, 14 for decks 1-4, and with `shift`
/// the SHIFT channel after it (so the pads stay lit while SHIFT is held).
pub fn pad_light(deck: u8, shift: bool, note: u8, on: bool) -> [u8; 3] {
    let channel = 7 + 2 * ch(deck) + u8::from(shift);
    [0x90 | channel, note, if on { ON } else { OFF }]
}

/// The level meter of a mixer channel (Fig. 3-11): 0x26 and up lights one
/// green, 0x41 two greens, 0x57 an orange, 0x65 two oranges, 0x77 red.
/// `level` is 0.0 (silence) to 1.0 (0 dB).
pub fn level_meter(channel: u8, level: f32) -> [u8; 3] {
    let v = (level.clamp(0.0, 1.0) * 127.0).round() as u8;
    [0xB0 | ch(channel), 0x02, v]
}

/// The jog illumination (Fig. 1-4 JOG ILLUMINATION): one of 72 positions
/// (0x01-0x48; 0x00 off) on channel 12, data 1 = deck - 1.
pub fn jog_light(deck: u8, turn: f64) -> [u8; 3] {
    let pos = if turn.is_finite() {
        (turn.rem_euclid(1.0) * 72.0).floor() as u8 % 72 + 1
    } else {
        0
    };
    [0xBB, ch(deck), pos]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_match_the_document() {
        assert_eq!(deck_light(1, PLAY, true), [0x90, 0x0B, 0x7F]);
        assert_eq!(deck_light(4, SYNC, false), [0x93, 0x58, 0x00]);
        assert_eq!(
            pad_light(1, false, pad_base::HOT_CUE, true),
            [0x97, 0x00, 0x7F]
        );
        assert_eq!(pad_light(1, true, 0x03, true), [0x98, 0x03, 0x7F]);
        assert_eq!(
            pad_light(3, false, pad_base::BEAT_LOOP + 7, true),
            [0x9B, 0x67, 0x7F]
        );
        assert_eq!(pad_light(4, true, 0x00, false), [0x9E, 0x00, 0x00]);
        assert_eq!(level_meter(2, 1.0), [0xB1, 0x02, 0x7F]);
        assert_eq!(jog_light(2, 0.0), [0xBB, 0x01, 0x01]);
        assert_eq!(jog_light(1, 0.999), [0xBB, 0x00, 0x48]);
        assert_eq!(jog_light(1, 1.25), [0xBB, 0x00, 19]);
    }
}
