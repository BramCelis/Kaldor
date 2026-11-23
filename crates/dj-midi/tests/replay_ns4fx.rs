//! Replays the capture taken on the real NS4FX (tests/captures/ns4fx-capture.midilog)
//! through the mapping engine with the bundled profile: every step produces
//! the control it is named after, and nothing the controller sent is left
//! unmapped except what is known and listed here.

use dj_midi::{Capture, Event, Mapper, MidiMessage, Profile, Unit, Value};
use std::collections::BTreeSet;

fn capture() -> Capture {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/captures/ns4fx-capture.midilog"
    );
    let f = std::fs::File::open(path).expect("tests/captures/ns4fx-capture.midilog");
    Capture::read(std::io::BufReader::new(f)).expect("a valid capture")
}

/// (control, unit) events of one step.
fn step_events(c: &Capture, number: u32) -> Vec<(String, Unit, u8, Value)> {
    let mut m = Mapper::new(Profile::ns4fx());
    let mut out: Vec<Event> = Vec::new();
    let s = c
        .steps
        .iter()
        .find(|s| s.number == number)
        .unwrap_or_else(|| panic!("step {number}"));
    for (_, b) in &s.messages {
        m.feed(b, &mut out);
    }
    out.iter()
        .map(|e| (m.name(e.control).to_string(), e.unit, e.index, e.value))
        .collect()
}

fn has(ev: &[(String, Unit, u8, Value)], name: &str, unit: Unit) -> bool {
    ev.iter().any(|e| e.0 == name && e.1 == unit)
}

#[test]
fn every_step_produces_its_control() {
    let c = capture();
    use Unit::*;
    let expect: &[(u32, &str, Unit)] = &[
        (1, "browse", Global),
        (2, "browse_push", Global),
        (3, "browse_shift", Global),
        (4, "browse_shift_push", Global),
        (5, "load", Global),
        (10, "level", Strip(1)),
        (11, "eq_high", Strip(1)),
        (12, "eq_mid", Strip(1)),
        (13, "eq_low", Strip(1)),
        (14, "filter", Strip(1)),
        (15, "headphone_cue", Strip(1)),
        (16, "channel_fader", Strip(1)),
        (17, "xfader_assign", Strip(1)),
        (34, "level", Strip(4)),
        (41, "xfader_assign", Strip(4)),
        (42, "crossfader", Global),
        (43, "cue_gain", Global),
        (45, "cue_mix", Global),
        (49, "fx_wet_dry", Fx(1)),
        (49, "fx_wet_dry", Fx(2)),
        (50, "fx_beats", Fx(1)),
        (52, "fx_tap", Fx(1)),
        (54, "fx_select", Fx(1)),
        (57, "fx_select", Fx(2)),
        (60, "fx_switch", Fx(1)),
        (61, "fx_switch", Fx(2)),
        (62, "shift", Side(1)),
        (63, "layer", Deck(3)),
        (64, "play", Deck(1)),
        (65, "play_shift", Deck(1)),
        (66, "cue", Deck(1)),
        (67, "cue_shift", Deck(1)),
        (68, "sync", Deck(1)),
        (69, "sync_shift", Deck(1)),
        (70, "pitch", Deck(1)),
        (72, "bend_down", Deck(1)),
        (73, "bend_down_shift", Deck(1)),
        (74, "bend_up", Deck(1)),
        (75, "bend_up_shift", Deck(1)),
        (77, "scratch", Deck(1)),
        (78, "jog_touch", Deck(1)),
        (79, "jog", Deck(1)),
        (81, "jog", Deck(1)),
        (83, "loop_toggle", Deck(1)),
        (84, "loop_toggle_shift", Deck(1)),
        (85, "loop_halve", Deck(1)),
        (86, "loop_in", Deck(1)),
        (87, "loop_double", Deck(1)),
        (88, "loop_out", Deck(1)),
        (89, "mode_cues", Deck(1)),
        (90, "pad", Deck(1)),
        (91, "mode_cues_shift", Deck(1)),
        (92, "mode_auto_loop", Deck(1)),
        (94, "mode_auto_loop_shift", Deck(1)),
        (95, "mode_fader_cuts", Deck(1)),
        (97, "mode_fader_cuts_shift", Deck(1)),
        (98, "mode_sample", Deck(1)),
        (100, "mode_sample_shift", Deck(1)),
        (101, "pad_shift", Deck(1)),
        (103, "shift", Side(2)),
        (105, "play", Deck(2)),
        (111, "pitch", Deck(2)),
        (121, "jog", Deck(2)),
        (131, "pad", Deck(2)),
        (144, "layer", Deck(3)),
        (145, "play", Deck(3)),
        (148, "loop_toggle", Deck(3)),
        (149, "shift", Side(1)),
        (150, "pitch", Deck(3)),
        (152, "jog_touch", Deck(3)),
        (154, "pad", Deck(3)),
        (157, "layer", Deck(4)),
        (158, "play", Deck(4)),
        (165, "jog", Deck(4)),
        (167, "pad", Deck(4)),
        (169, "layer", Deck(2)),
    ];
    for (step, name, unit) in expect {
        let ev = step_events(&c, *step);
        assert!(
            has(&ev, name, *unit),
            "step {step}: no {name} on {unit:?}; got {:?}",
            ev.iter().map(|e| (&e.0, e.1)).collect::<Vec<_>>()
        );
    }
    // Pads 1 to 8 come out as indexes 0 to 7, in order.
    let pads: Vec<u8> = step_events(&c, 90)
        .iter()
        .filter(|e| e.0 == "pad" && e.3 == Value::Press(true))
        .map(|e| e.2)
        .collect();
    assert_eq!(pads, (0..8).collect::<Vec<u8>>());
    // The pitch fader went over its whole 14-bit range.
    let pitch: Vec<f32> = step_events(&c, 111)
        .iter()
        .filter_map(|e| match e.3 {
            Value::Abs(v) if e.0 == "pitch" => Some(v),
            _ => None,
        })
        .collect();
    let (lo, hi) = pitch
        .iter()
        .fold((1.0f32, 0.0f32), |(a, b), v| (a.min(*v), b.max(*v)));
    assert!(lo < 0.01 && hi > 0.99, "{lo}..{hi}");
}

/// The follow-up capture (tests/captures/ns4fx-capture-2.midilog).
fn followup() -> Capture {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/captures/ns4fx-capture-2.midilog"
    );
    let f = std::fs::File::open(path).expect("tests/captures/ns4fx-capture-2.midilog");
    Capture::read(std::io::BufReader::new(f)).expect("a valid capture")
}

/// The FX capture (tests/captures/ns4fx-capture-3.midilog): the FX section with SHIFT.
fn fx_capture() -> Capture {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/captures/ns4fx-capture-3.midilog"
    );
    let f = std::fs::File::open(path).expect("tests/captures/ns4fx-capture-3.midilog");
    Capture::read(std::io::BufReader::new(f)).expect("a valid capture")
}

/// SHIFT + effect buttons 1-6 send notes of the same FX section (some of
/// them another button's note without SHIFT); the behaviour sorts them out
/// with the SHIFT state.
#[test]
fn fx_capture_shift_effect_buttons() {
    let c = fx_capture();
    let expect: [(u32, Unit, u8); 6] = [
        (2, Unit::Fx(1), 0x01),
        (3, Unit::Fx(1), 0x04),
        (4, Unit::Fx(1), 0x02),
        (5, Unit::Fx(2), 0x00),
        (6, Unit::Fx(2), 0x03),
        (7, Unit::Fx(2), 0x05),
    ];
    for (step, unit, note) in expect {
        let ev = step_events(&c, step);
        assert!(has(&ev, "shift", Unit::Side(1)), "step {step}: SHIFT");
        assert!(
            ev.iter()
                .any(|e| e.0 == "fx_select" && e.1 == unit && e.2 == note),
            "step {step}: {ev:?}"
        );
    }
}

#[test]
fn nothing_in_either_capture_is_unmapped() {
    let m = Mapper::new(Profile::ns4fx());
    let mut unmapped = BTreeSet::new();
    for c in [capture(), followup(), fx_capture()] {
        for (_, _, b) in c.messages() {
            if !m.knows(b) {
                unmapped.insert(MidiMessage::parse(b).to_string());
            }
        }
    }
    assert!(unmapped.is_empty(), "{unmapped:?}");
}

#[test]
fn followup_load_per_deck_loop_buttons_per_side_and_fader_cuts() {
    let c = followup();
    use Unit::*;
    let load = |step| {
        step_events(&c, step)
            .iter()
            .filter(|e| e.0 == "load" && e.3 == Value::Press(true))
            .map(|e| e.2)
            .collect::<Vec<u8>>()
    };
    // LOAD: index = deck (0-based): deck 3 and deck 4 have their own notes.
    assert_eq!(load(1), vec![2]);
    assert_eq!(load(3), vec![3]);
    assert_eq!(load(4), vec![0]);
    // SHIFT + LOOP 1/2 then x2: loop in then loop out, on both sides.
    for (step, deck) in [(7, Deck(1)), (8, Deck(2))] {
        let names: Vec<String> = step_events(&c, step)
            .into_iter()
            .filter(|e| e.1 == deck && e.3 == Value::Press(true))
            .map(|e| e.0)
            .collect();
        assert_eq!(names, vec!["loop_in", "loop_out"], "step {step}");
    }
    // The right deck's FADER CUTS values.
    assert!(has(&step_events(&c, 10), "fader_cut", Deck(2)));
}

/// The state dump the NS4FX sends after `F0 00 20 7F 03 01 F7`
/// (`tests/captures/ns4fx-capture-7.midilog`). Prints every message with the control
/// it maps to; every one must be known.
#[test]
fn state_dump_maps_to_known_controls() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/captures/ns4fx-capture-7.midilog"
    );
    let f = std::fs::File::open(path).expect("tests/captures/ns4fx-capture-7.midilog");
    let c = Capture::read(std::io::BufReader::new(f)).expect("a valid capture");
    let m = Mapper::new(Profile::ns4fx());
    let mut unknown = Vec::new();
    for (step, _, bytes) in c.messages() {
        if step != 3 {
            continue;
        }
        let name = m.control_name(bytes);
        eprintln!("{:<12} -> {:?}", dj_midi::message::hex(bytes), name);
        if name.is_none() {
            unknown.push(dj_midi::message::hex(bytes));
        }
    }
    assert!(unknown.is_empty(), "unknown: {unknown:?}");
}
