//! The NS4FX in the headless app. MIDI bytes (as captured on the
//! real controller) go through the same processor the MIDI thread runs,
//! into the engine's controller queue and the UI. No MIDI port, no audio
//! device.

use dj_core::AppConfig;
use dj_core::config::DeckLayout;
use dj_midi::{Capture, Profile};
use dj_ui::controller::{ControllerLink, Processor};
use dj_ui::headless::Demo;
use dj_ui::perform::PadMode;

struct T {
    demo: Demo,
    p: Processor,
}

impl T {
    fn new() -> Self {
        let mut cfg = AppConfig::default();
        cfg.ui.layout = DeckLayout::FourStacked;
        let mut demo = Demo::new(cfg);
        let (link, p) = ControllerLink::offline(Profile::ns4fx());
        demo.app.attach_controller(link);
        let mut t = Self { demo, p };
        t.frames(3);
        t
    }

    fn frames(&mut self, n: usize) {
        self.demo.run(n, egui::vec2(1920.0, 1080.0), 1.0);
    }

    /// Feeds MIDI messages, then runs a few frames.
    fn midi(&mut self, msgs: &[&[u8]]) {
        for m in msgs {
            self.p.feed(m);
        }
        self.frames(3);
    }

    fn deck(&self, d: usize) -> dj_audio::DeckSnapshot {
        self.demo.app.snapshot().decks[d]
    }
}

#[test]
fn transport_mixer_layers_pad_modes_and_pitch() {
    let mut t = T::new();
    assert!(t.deck(0).playing, "the demo plays deck 1");
    // PLAY on the left side pauses deck 1.
    t.midi(&[&[0x90, 0x00, 0x7F], &[0x80, 0x00, 0x00]]);
    assert!(!t.deck(0).playing);
    // Channel 2 fader down, channel 3 headphone cue on.
    t.midi(&[&[0xB1, 0x1C, 0x00], &[0x92, 0x1B, 0x7F]]);
    let m = t.demo.app.snapshot().mixer;
    assert_eq!(m.channels[1].fader, 0.0);
    assert!(m.channels[2].pfl);
    // LAYER on the left: deck 3.
    t.midi(&[&[0x90, 0x08, 0x00], &[0x92, 0x08, 0x7F]]);
    assert_eq!(t.demo.app.layer(), [2, 1]);
    // SHIFT + SAMPLE on deck 3 (channel 7): pitch play; AUTO LOOP + pad 4
    // (through the UI) makes a 1-beat loop.
    t.midi(&[&[0x96, 0x0F, 0x7F]]);
    assert_eq!(t.demo.app.pad_mode(2), PadMode::PitchPlay);
    t.midi(&[
        &[0x96, 0x0D, 0x7F],
        &[0x96, 0x17, 0x7F],
        &[0x86, 0x17, 0x00],
    ]);
    assert_eq!(t.demo.app.pad_mode(2), PadMode::AutoLoop);
    let l = t.deck(2).loop_info.expect("a loop");
    assert!(l.active && (l.beats - 1.0).abs() < 1e-4, "{l:?}");
    // LOOP ON/OFF leaves it.
    t.midi(&[&[0x96, 0x40, 0x7F]]);
    assert!(!t.deck(2).loop_info.unwrap().active);
    // Deck 2's pitch fader: first in the middle (it meets the pitch, 0 %,
    // and takes over), then at the bottom (127, capture 4): faster, +8 %.
    t.midi(&[&[0xB1, 0x09, 0x40], &[0xB1, 0x29, 0x00]]);
    t.midi(&[&[0xB1, 0x09, 0x7F], &[0xB1, 0x29, 0x7F]]);
    t.frames(30);
    assert!(
        (t.deck(1).tempo - 1.08).abs() < 0.002,
        "tempo {}",
        t.deck(1).tempo
    );
    // At the top (0, the last values of capture 4 step 3): slower, -8 %.
    t.midi(&[&[0xB1, 0x09, 0x00], &[0xB1, 0x29, 0x00]]);
    t.frames(30);
    assert!(
        (t.deck(1).tempo - 0.92).abs() < 0.002,
        "tempo {}",
        t.deck(1).tempo
    );
}

/// The buttons choose one effect, the left paddle plays
/// it on decks 1 and 3, the right one on 2 and 4.
#[test]
fn fx_buttons_choose_the_effect_and_paddles_play_it_per_side() {
    use dj_audio::{EffectKind, FxMode};
    let mut t = T::new();
    // HPF (button 1), WET/DRY three quarters, left paddle up.
    t.midi(&[
        &[0x98, 0x00, 0x7F],
        &[0xB8, 0x04, 0x60],
        &[0xB9, 0x04, 0x60],
        &[0xB8, 0x03, 0x01],
    ]);
    let fx = t.demo.app.snapshot().fx.params;
    let (left, right) = (fx.units[0], fx.units[1]);
    assert!(left.on && !right.on);
    assert_eq!(left.assign, [true, false, true, false, false], "decks 1, 3");
    assert_eq!(left.mode, FxMode::Single);
    let s = left.slots[0];
    assert!(
        s.on && s.kind == EffectKind::Filter && s.params[1] == 1.0,
        "{s:?}"
    );
    assert!((s.params[0] - 96.0 / 127.0).abs() < 0.01, "cutoff {s:?}");
    // The right side has the same effect ready.
    assert_eq!(right.slots[0], s);
    // Echo (button 4); right paddle held down; left paddle back to the
    // centre.
    t.midi(&[
        &[0x99, 0x03, 0x7F],
        &[0xB9, 0x03, 0x02],
        &[0xB8, 0x03, 0x00],
    ]);
    let fx = t.demo.app.snapshot().fx.params;
    let (left, right) = (fx.units[0], fx.units[1]);
    assert!(!left.on && right.on);
    assert_eq!(
        right.assign,
        [false, true, false, true, false],
        "decks 2, 4"
    );
    assert_eq!(right.slots[0].kind, EffectKind::Echo);
    assert!((right.mix - 96.0 / 127.0).abs() < 0.01, "{}", right.mix);
    // Echo again: deselected.
    t.midi(&[&[0x99, 0x03, 0x7F]]);
    let fx = t.demo.app.snapshot().fx.params;
    assert!(!fx.units[0].slots[0].on && !fx.units[1].slots[0].on);
}

/// SHIFT + an effect button chooses the color FX on the
/// filter knobs, as on the DJMs; SHIFT + the chosen one again goes back to
/// the plain filter. The main effect is left alone.
#[test]
fn shift_effect_buttons_choose_the_color_fx() {
    use dj_audio::ColorFx;
    let mut t = T::new();
    let color = |t: &T| t.demo.app.snapshot().mixer.color_fx;
    // Echo (no SHIFT) first: the main effect.
    t.midi(&[&[0x99, 0x03, 0x7F]]);
    assert_eq!(
        dj_ui::fx_panel::lit(&t.demo.app.snapshot().fx.params),
        Some(3)
    );
    // SHIFT (left) + button 4 as the NS4FX sends it (channel 10, note 0):
    // dub echo.
    let shifted = |t: &mut T, msg: &[u8]| {
        t.midi(&[&[0x90, 0x20, 0x7F], msg, &[0x80, 0x20, 0x00]]);
    };
    shifted(&mut t, &[0x99, 0x00, 0x7F]);
    assert_eq!(color(&t), ColorFx::DubEcho);
    // Its button (Echo, the 4th) flickers, as on a DJM; the others dim.
    let fb = t.demo.app.controller_feedback();
    assert_eq!(fb.fx[3], dj_ui::controller_out::FLICKER);
    assert_eq!(fb.fx[4], dj_midi::ns4fx_out::LED_DIM);
    // SHIFT + button 5 (channel 10, note 3): space; again: the filter.
    shifted(&mut t, &[0x99, 0x03, 0x7F]);
    assert_eq!(color(&t), ColorFx::Space);
    shifted(&mut t, &[0x99, 0x03, 0x7F]);
    assert_eq!(color(&t), ColorFx::Filter);
    // No color FX: nothing flickers; Echo (the main effect) is bright.
    let fb = t.demo.app.controller_feedback();
    assert!(!fb.fx.contains(&dj_ui::controller_out::FLICKER));
    assert_eq!(fb.fx[3], dj_midi::ns4fx_out::LED_ON);
    // SHIFT + LPF (channel 9, note 4): noise; SHIFT + HPF (channel 9,
    // note 1): the standard filter.
    shifted(&mut t, &[0x98, 0x04, 0x7F]);
    assert_eq!(color(&t), ColorFx::Noise);
    shifted(&mut t, &[0x98, 0x01, 0x7F]);
    assert_eq!(color(&t), ColorFx::Filter);
    // The main effect is still Echo.
    assert_eq!(
        dj_ui::fx_panel::lit(&t.demo.app.snapshot().fx.params),
        Some(3)
    );
}

/// With the Noise color FX, the FILTER knob turned left
/// silenced the track (back in the middle it came back). Through the
/// whole path: SHIFT + LPF chooses Noise, the knob goes left, deck 1 (the
/// demo plays it) keeps its level.
#[test]
fn noise_color_fx_turned_left_keeps_the_track() {
    use dj_audio::ColorFx;
    let mut t = T::new();
    let level = |t: &mut T| {
        t.frames(40);
        let m = t.demo.app.snapshot().channel_meters[0];
        m.peak[0].max(m.peak[1])
    };
    // The knob in the middle first (soft takeover picks it up there).
    t.midi(&[&[0xB0, 0x1A, 0x40]]);
    let dry = level(&mut t);
    assert!(dry > 0.01, "deck 1 plays: {dry}");
    // SHIFT + LPF (channel 9, note 4 with SHIFT): Noise.
    t.midi(&[
        &[0x90, 0x20, 0x7F],
        &[0x98, 0x04, 0x7F],
        &[0x80, 0x20, 0x00],
    ]);
    assert_eq!(t.demo.app.snapshot().mixer.color_fx, ColorFx::Noise);
    // Turned left in steps, as a hand does.
    for v in (0..0x40u8).rev().step_by(4) {
        t.midi(&[&[0xB0, 0x1A, v]]);
    }
    t.midi(&[&[0xB0, 0x1A, 0x00]]);
    let s = t.demo.app.snapshot();
    assert_eq!(s.mixer.color_fx, ColorFx::Noise, "still Noise");
    assert!(
        s.mixer.channels[0].filter < -0.9,
        "{}",
        s.mixer.channels[0].filter
    );
    let left = level(&mut t);
    assert!(left > dry * 0.8, "the track stays: {left} vs {dry}");
}

#[test]
fn browse_and_load() {
    let mut t = T::new();
    // Turn the browse knob 2 clicks: the second track is selected.
    t.midi(&[&[0xBF, 0x00, 0x01], &[0xBF, 0x00, 0x01]]);
    let second = t.demo.app.library().unwrap().view()[1];
    assert_eq!(t.demo.app.library().unwrap().selected_track, Some(second));
    // LOAD right while deck 2 plays: refused.
    t.midi(&[&[0x9F, 0x03, 0x7F], &[0x8F, 0x03, 0x00]]);
    assert!(
        t.demo.app.library().unwrap().status.contains("playing"),
        "{}",
        t.demo.app.library().unwrap().status
    );
    // Pause deck 2, then LOAD: the deck takes the track.
    t.midi(&[&[0x91, 0x00, 0x7F], &[0x81, 0x00, 0x00]]);
    t.midi(&[&[0x9F, 0x03, 0x7F], &[0x8F, 0x03, 0x00]]);
    let name = t
        .demo
        .app
        .library()
        .unwrap()
        .track(second)
        .unwrap()
        .display_name();
    assert_eq!(t.demo.app.deck_title(dj_core::DeckId::ALL[1]), name);
    // Push: the knob browses crates.
    t.midi(&[&[0x9F, 0x07, 0x7F], &[0x8F, 0x07, 0x00]]);
    assert!(t.demo.app.library().unwrap().browse_crates);
}

/// The whole capture taken on the NS4FX, replayed into the running app:
/// nothing panics and the app keeps drawing.
#[test]
fn the_whole_capture_replays_into_the_app() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../dj-midi/tests/captures/ns4fx-capture.midilog"
    );
    let c = Capture::read(std::io::BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    let mut t = T::new();
    let mut worst = std::time::Duration::ZERO;
    let mut total = std::time::Duration::ZERO;
    let mut n = 0u32;
    for (i, (_, _, bytes)) in c.messages().enumerate() {
        // The time from a message to its command in the engine's queue
        // (what the MIDI thread does per message).
        let t0 = std::time::Instant::now();
        t.p.feed(bytes);
        let dt = t0.elapsed();
        worst = worst.max(dt);
        total += dt;
        n += 1;
        if i % 200 == 0 {
            t.frames(1);
        }
    }
    t.frames(10);
    let mean = total / n.max(1);
    eprintln!("controller: {n} messages, mean {mean:?}, worst {worst:?} per message");
    // Even in a debug build far below the 3 ms MIDI-to-engine budget.
    assert!(
        mean < std::time::Duration::from_micros(500),
        "mean {mean:?}"
    );
}

/// The browse knob keeps the selected track visible in a long list
/// (also a page at a time with SHIFT), and pushing it shows where it is.
#[test]
fn browsing_scrolls_the_list_and_shows_the_focus() {
    let mut t = T::new();
    // The list keeps its scroll position only within one egui context.
    t.demo.keep_context();
    {
        let lib = t.demo.app.library().unwrap();
        for i in 0..60 {
            lib.lib
                .upsert_track(&dj_library::TrackMeta {
                    path: format!("demo://extra{i:02}.wav").into(),
                    artist: "Extra".into(),
                    title: format!("Track {i:02}"),
                    duration: 60.0,
                    ..Default::default()
                })
                .unwrap();
        }
        lib.reload();
    }
    t.frames(3);
    let visible = |t: &mut T| {
        let lib = t.demo.app.library().unwrap();
        let sel = lib.selected_track.unwrap();
        let row = lib.view().iter().position(|x| *x == sel).unwrap() as f32;
        let (offset, height) = lib.list_scroll;
        let pitch = lib.row_pitch;
        (
            row * pitch >= offset - 0.5 && (row + 1.0) * pitch <= offset + height + 0.5,
            row,
        )
    };
    // 40 clicks down, one at a time.
    for _ in 0..40 {
        t.p.feed(&[0xBF, 0x00, 0x01]);
        t.frames(1);
    }
    t.frames(2);
    let (ok, row) = visible(&mut t);
    assert!(ok && row == 39.0, "row {row} not visible");
    // SHIFT + browse: 10 rows per click, back up.
    t.midi(&[&[0xBF, 0x01, 0x7F], &[0xBF, 0x01, 0x7F]]);
    t.frames(2);
    let (ok, row) = visible(&mut t);
    assert!(ok && row == 19.0, "row {row} not visible");
    // Push: crates (no track selected); push again: the first track.
    t.midi(&[&[0x9F, 0x07, 0x7F], &[0x8F, 0x07, 0x00]]);
    assert!(t.demo.app.library().unwrap().browse_crates);
    assert!(t.demo.app.library().unwrap().selected_track.is_none());
    t.midi(&[&[0x9F, 0x07, 0x7F], &[0x8F, 0x07, 0x00]]);
    let lib = t.demo.app.library().unwrap();
    assert!(!lib.browse_crates && lib.knob_used);
    let first = lib.view()[0];
    assert_eq!(lib.selected_track, Some(first));
}

/// Messages the profile does not know are listed for the Setup tab (so a
/// control that sends something unexpected can be found).
#[test]
fn unknown_messages_are_kept_for_setup() {
    let mut t = T::new();
    t.midi(&[
        &[0x90, 0x00, 0x7F],
        &[0x9F, 0x30, 0x7F],
        &[0xB5, 0x77, 0x40],
    ]);
    let u = t.demo.app.controller_unknown();
    assert_eq!(u.len(), 2, "{u:?}");
    assert!(u[0].contains("ch16") && u[0].contains("0x30"), "{u:?}");
    assert!(u[1].contains("CC") && u[1].contains("0x77"), "{u:?}");
}

/// Follow-up capture: LOAD sends one note per deck (0x04 = deck 3), and the
/// right deck's FADER CUTS value moves the crossfader mirrored, back to the
/// hand's position when the pads are released.
#[test]
fn load_on_deck_3_and_right_deck_fader_cuts() {
    let mut t = T::new();
    // Pause deck 3, select the second track, LOAD with deck 3 active (0x04).
    t.midi(&[&[0x92, 0x00, 0x7F], &[0x82, 0x00, 0x00]]);
    t.midi(&[&[0xBF, 0x00, 0x01], &[0xBF, 0x00, 0x01]]);
    let second = t.demo.app.library().unwrap().view()[1];
    t.midi(&[&[0x9F, 0x04, 0x7F], &[0x8F, 0x04, 0x00]]);
    let name = t
        .demo
        .app
        .library()
        .unwrap()
        .track(second)
        .unwrap()
        .display_name();
    assert_eq!(t.demo.app.deck_title(dj_core::DeckId::ALL[2]), name);

    // Crossfader in the middle by hand; right deck FADER CUTS; a pad held:
    // the controller's 127 (deck cut) puts the crossfader fully left.
    t.midi(&[&[0xBF, 0x08, 0x40]]);
    t.midi(&[
        &[0x95, 0x07, 0x7F],
        &[0x95, 0x14, 0x7F],
        &[0xB1, 0x08, 0x7F],
    ]);
    let x = t.demo.app.snapshot().mixer.crossfader;
    assert!((x + 1.0).abs() < 1e-3, "crossfader {x}");
    // Released: back to the hand's position (the middle).
    t.midi(&[&[0x85, 0x14, 0x00]]);
    let x = t.demo.app.snapshot().mixer.crossfader;
    assert!(
        (x - (64.0 / 127.0 * 2.0 - 1.0)).abs() < 1e-3,
        "crossfader {x}"
    );
}

/// The controller's lights and displays follow the app.
#[test]
fn lights_and_displays_follow_the_decks() {
    use dj_midi::ns4fx_out::{LED_DIM, LED_ON};
    use dj_ui::controller_out::FLICKER;
    let mut t = T::new();
    let fb = t.demo.app.controller_feedback();
    let d1 = fb.decks[0];
    assert_eq!(d1.play, LED_ON, "deck 1 plays");
    assert!((d1.bpm - 124.0).abs() < 0.05, "BPM {}", d1.bpm);
    // Mode lights: CUES bright, the others dim; empty pads dim.
    assert_eq!(d1.modes, [LED_ON, LED_DIM, LED_DIM, LED_DIM]);
    assert_eq!(d1.pads, [LED_DIM; 8], "no hot cues yet: available, dim");
    assert_eq!(d1.cue, LED_DIM, "playing: CUE dim");
    assert!(d1.time_ms > 0 && d1.duration_ms > 29_000);
    assert!(d1.vinyl);
    // A hot cue on pad 3, deck 1 paused, deck 2 key lock off, SHIFT + AUTO
    // LOOP (roll) on deck 1.
    t.midi(&[&[0x94, 0x16, 0x7F], &[0x84, 0x16, 0x00]]);
    t.midi(&[&[0x90, 0x00, 0x7F], &[0x80, 0x00, 0x00]]);
    t.midi(&[
        &[0x91, 0x0B, 0x7F],
        &[0x91, 0x0C, 0x7F],
        &[0x81, 0x0B, 0x00],
        &[0x81, 0x0C, 0x00],
    ]);
    t.midi(&[&[0x94, 0x06, 0x7F]]);
    t.frames(10);
    let fb = t.demo.app.controller_feedback();
    assert_eq!(
        fb.decks[0].play, FLICKER,
        "loaded and paused: PLAY flickers"
    );
    assert_eq!(fb.decks[0].cue, FLICKER, "and CUE");
    assert_eq!(
        fb.decks[0].modes,
        [LED_DIM, FLICKER, LED_DIM, LED_DIM],
        "roll: the AUTO LOOP light flickers"
    );
    // The demo starts deck 2 with key lock on: both bend buttons turn it off.
    assert!(!fb.decks[1].key_lock);
    t.midi(&[&[0x94, 0x00, 0x7F]]);
    let fb = t.demo.app.controller_feedback();
    assert_eq!(fb.decks[0].pads[2], LED_ON, "hot cue 3 lit");
    // Deck 2 plays 128 BPM at +2 % (130.6): deck 1 (124) is slower -> up
    // arrow; deck 3 (140) is faster -> down arrow.
    assert_eq!(fb.decks[0].arrows, (true, false));
    assert_eq!(fb.decks[2].arrows, (false, true));
}

/// MIDI learn: a function picked in the app goes to the next control used
/// on the controller; learned controls replace the profile's function;
/// SHIFT alone is not learned (so SHIFT combinations can be); removing a
/// binding gives the control its profile function back.
#[test]
fn midi_learn_buttons_faders_and_shift_combinations() {
    use dj_core::config::MidiFunction;
    let mut t = T::new();
    // Pad 3 of deck 2 onto a note the profile does not use.
    t.demo.app.start_learn(MidiFunction::Pad(3), Some(2));
    t.frames(2);
    t.midi(&[&[0x9F, 0x30, 0x7F], &[0x8F, 0x30, 0x00]]);
    assert!(t.demo.app.learning().is_none(), "learned");
    let b = t.demo.app.learned()[0];
    assert_eq!((b.note, b.channel, b.number), (true, 16, 0x30));
    assert_eq!(b.function, MidiFunction::Pad(3));
    assert_eq!(b.deck, Some(2));
    assert!(
        t.demo.app.config().controller.learned.len() == 1,
        "saved in the config"
    );
    // Using it: deck 2 (CUES mode) gets hot cue 3.
    t.midi(&[&[0x9F, 0x30, 0x7F], &[0x8F, 0x30, 0x00]]);
    assert!(t.deck(1).hot_cues[2].is_some(), "hot cue 3 on deck 2");

    // A fader: deck 3's volume on a controller the profile does not use.
    t.demo.app.start_learn(MidiFunction::Volume, Some(3));
    t.frames(2);
    t.midi(&[&[0xB5, 0x50, 0x40]]);
    t.midi(&[&[0xB5, 0x50, 0x00]]);
    assert_eq!(t.demo.app.snapshot().mixer.channels[2].fader, 0.0);

    // Over the profile: deck 1's PLAY button becomes SYNC for deck 1.
    t.demo.app.start_learn(MidiFunction::Sync, Some(1));
    t.frames(2);
    t.midi(&[&[0x90, 0x00, 0x7F], &[0x80, 0x00, 0x00]]);
    let playing = t.deck(0).playing;
    t.midi(&[&[0x90, 0x00, 0x7F], &[0x80, 0x00, 0x00]]);
    assert_eq!(t.deck(0).playing, playing, "PLAY no longer plays");
    assert!(t.deck(0).sync, "it syncs");

    // SHIFT alone is skipped: SHIFT + PLAY (0x04) is what gets learned.
    t.demo.app.start_learn(MidiFunction::Quantize, None);
    t.frames(2);
    t.midi(&[&[0x90, 0x20, 0x7F]]);
    assert!(t.demo.app.learning().is_some(), "SHIFT alone not learned");
    t.midi(&[
        &[0x90, 0x04, 0x7F],
        &[0x80, 0x04, 0x00],
        &[0x80, 0x20, 0x00],
    ]);
    let last = *t.demo.app.learned().last().unwrap();
    assert_eq!((last.channel, last.number), (1, 0x04));
    assert_eq!(last.deck, None, "quantize has no deck");

    // Removing the PLAY binding: PLAY plays again.
    let i = t
        .demo
        .app
        .learned()
        .iter()
        .position(|b| b.number == 0x00 && b.channel == 1)
        .unwrap();
    t.demo.app.remove_learned(i);
    t.frames(2);
    let playing = t.deck(0).playing;
    t.midi(&[&[0x90, 0x00, 0x7F], &[0x80, 0x00, 0x00]]);
    assert_ne!(t.deck(0).playing, playing, "PLAY is play again");
    t.demo.app.clear_learned();
    assert!(t.demo.app.learned().is_empty());

    // The learn panel is in the settings window, Controller tab.
    t.demo.app.show_setup_tab(dj_ui::SetupTab::Controller);
    t.frames(3);
    assert!(
        !t.demo.app.ui_map().find("learn_start", None).is_empty(),
        "Learn button in Setup"
    );
}

/// The control messages of one step of a capture file in `crates/dj-midi/tests/captures/`.
fn dump(file: &str, step: u32) -> Vec<Vec<u8>> {
    let path = format!(
        "{}/../dj-midi/tests/captures/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    let c = Capture::read(std::io::BufReader::new(std::fs::File::open(path).unwrap())).unwrap();
    c.messages()
        .filter(|(s, _, _)| *s == step)
        .map(|(_, _, b)| b.to_vec())
        .collect()
}

/// A state dump after plugging the controller back in (capture 8,
/// a real NS4FX's answer) is applied like the one at start-up, also
/// over changes made in between.
#[test]
fn state_dump_after_replug_is_applied() {
    let mut t = T::new();
    // Start-up: capture 7's answer.
    t.p.begin_state_dump();
    let first = dump("ns4fx-capture-7.midilog", 3);
    t.midi(&first.iter().map(Vec::as_slice).collect::<Vec<_>>());
    let m = t.demo.app.snapshot().mixer;
    assert_eq!(m.channels[0].fader, 1.0, "start-up");
    assert_eq!(m.channels[1].filter, 1.0, "start-up");
    // Changes while unplugged: channel 1 fader down, crossfader right.
    t.midi(&[&[0xB0, 0x1C, 0x00], &[0xBF, 0x08, 0x7F]]);
    t.frames(30);
    // Plugged back in: capture 8's answer.
    t.p.begin_state_dump();
    let second = dump("ns4fx-capture-8.midilog", 1);
    t.midi(&second.iter().map(Vec::as_slice).collect::<Vec<_>>());
    t.frames(30);
    let s = t.demo.app.snapshot();
    let m = s.mixer;
    assert_eq!(m.channels[0].fader, 1.0, "fader 1 back up (B0 1C 7F)");
    assert_eq!(m.crossfader, -1.0, "crossfader left (BF 08 00)");
    assert!(
        (m.channels[0].eq[1] - 74.0 / 127.0).abs() < 1e-6,
        "mid EQ 1 (B0 18 4A)"
    );
    assert_eq!(m.channels[0].filter, -1.0);
    // FX paddles in the middle in capture 8 (B8/B9 03 00): units off.
    assert!(!s.fx.params.units[0].on && !s.fx.params.units[1].on);
    assert!(
        (t.deck(0).pitch + 1.0).abs() < 1e-6,
        "left pitch at the top"
    );
}

/// The reported sync case, through the NS4FX: deck 1 at +5 %
/// (its fader there), deck 2 synced to it; deck 1 stopped, a new track
/// (138 BPM) on it, synced and played; then deck 2 stopped. Deck 1 becomes
/// the master and keeps the synced tempo (as in Serato); its fader at +5 %
/// waits as a soft-takeover ghost, also through fader jitter.
#[test]
fn a_synced_deck_keeps_its_tempo_when_the_master_stops() {
    let mut cfg = AppConfig::default();
    cfg.ui.layout = DeckLayout::FourStacked;
    cfg.performance.auto_sync_master = true;
    let mut demo = Demo::new(cfg);
    let (link, p) = ControllerLink::offline(Profile::ns4fx());
    demo.app.attach_controller(link);
    let mut t = T { demo, p };
    t.frames(3);
    // Deck 1's fader: to the middle (takes over), then +5 %.
    t.midi(&[&[0xB0, 0x09, 0x40], &[0xB0, 0x29, 0x00]]);
    t.midi(&[&[0xB0, 0x09, 0x67], &[0xB0, 0x29, 0x7F]]);
    t.frames(20);
    assert!((t.deck(0).tempo - 1.05).abs() < 1e-3, "{}", t.deck(0).tempo);
    // SYNC deck 2, stop deck 1: deck 2 takes over as master.
    t.midi(&[&[0x91, 0x02, 0x7F], &[0x81, 0x02, 0x00]]);
    t.midi(&[&[0x90, 0x00, 0x7F], &[0x80, 0x00, 0x00]]);
    t.frames(20);
    assert_eq!(t.demo.app.snapshot().sync_master, Some(1));
    // A new track on deck 1 (138 BPM), SYNC and PLAY.
    let c = dj_analysis::synth::ClickTrack::new(138.0, 30.0).to_track();
    let an = dj_analysis::analyze_audio(&c, &dj_analysis::AnalysisOptions::default());
    t.demo
        .app
        .load_audio_analyzed(dj_core::DeckId::ALL[0], "new", c, &an);
    t.frames(20);
    t.midi(&[&[0x90, 0x02, 0x7F], &[0x80, 0x02, 0x00]]);
    t.midi(&[&[0x90, 0x00, 0x7F], &[0x80, 0x00, 0x00]]);
    t.frames(20);
    let synced = t.deck(1).tempo * an.beatgrid.unwrap().bpm.recip() * 128.0;
    assert!(t.deck(0).sync && t.deck(0).playing);
    // Stop deck 2: deck 1 is the master now and keeps its tempo.
    t.midi(&[&[0x91, 0x00, 0x7F], &[0x81, 0x00, 0x00]]);
    t.frames(20);
    let s = t.demo.app.snapshot();
    assert_eq!(s.sync_master, Some(0));
    assert!(
        (s.decks[0].tempo - synced).abs() < 2e-3,
        "deck 1 jumped from {synced:.4} to {:.4}",
        s.decks[0].tempo
    );
    assert!(s.decks[0].pitch_fader_ghost.is_some(), "the fader waits");
    // Fader jitter (the last bit) does not take over.
    t.midi(&[&[0xB0, 0x29, 0x7E]]);
    t.frames(20);
    assert!((t.deck(0).tempo - synced).abs() < 2e-3);
}
