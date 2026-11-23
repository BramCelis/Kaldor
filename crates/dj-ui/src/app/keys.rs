//! Keyboard shortcuts: the keymap played on the decks, mixer and FX.

use super::KaldorApp;
use dj_audio::{Command, DeckCommand, MixerCommand};
use dj_core::DeckId;
use dj_core::config::KeyAction;

impl KaldorApp {
    pub(super) fn handle_keys(&mut self, ui: &egui::Ui) {
        // A key being changed in the settings is not played.
        if self.key_capture.is_some() && self.show_setup {
            return;
        }
        if ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        // Key events of this frame: logical key, physical key, pressed, Shift.
        let events: Vec<(egui::Key, Option<egui::Key>, bool, bool)> = ui.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed,
                        repeat: false,
                        modifiers,
                        // Ctrl / Cmd + key belongs to the library (Ctrl+A),
                        // not to the deck keys.
                    } if !(modifiers.ctrl || modifiers.command) || !*pressed => {
                        Some((*key, *physical_key, *pressed, modifiers.shift))
                    }
                    _ => None,
                })
                .collect()
        });
        if events.is_empty() {
            return;
        }
        let bindings = self.config.ui.keymap.clone();
        for (key, physical, pressed, shift) in events {
            for b in &bindings {
                let Some(bk) = egui::Key::from_name(&b.key) else {
                    continue;
                };
                // With Shift a key can arrive as its symbol ("!" for Shift+1):
                // then the physical key counts.
                let hit = key == bk || (shift && physical == Some(bk));
                // Presses need the binding's Shift state; releases end holds
                // whatever the modifiers are.
                if hit && (!pressed || b.shift == shift) {
                    self.key_action(b.action, pressed);
                }
            }
        }
    }

    /// Runs a keyboard action (`pressed` false: the key went up).
    pub fn key_action(&mut self, action: KeyAction, pressed: bool) {
        use crate::perform::{GRID_ADJUST, GRID_NUDGE, GridEdit, PadMode};
        let snap = *self.engine.snapshot();
        let deck = |n: u8| DeckId::from_index(usize::from(n.saturating_sub(1)));
        let index = |n: u8| usize::from(n.saturating_sub(1));
        let kb = self.kb_deck;
        match action {
            KeyAction::Cue(n) => {
                if let Some(d) = deck(n) {
                    self.send(Command::Deck(d, DeckCommand::Cue(pressed)));
                }
            }
            KeyAction::BendDown(n) | KeyAction::BendUp(n) => {
                let sign = if matches!(action, KeyAction::BendUp(_)) {
                    1.0
                } else {
                    -1.0
                };
                if let Some(d) = deck(n) {
                    let amount = if pressed { sign * 0.04 } else { 0.0 };
                    self.send(Command::Deck(d, DeckCommand::PitchBend(amount)));
                }
            }
            KeyAction::Pad(n) => self.pad(kb, index(n), pressed),
            _ if !pressed => {}
            KeyAction::Play(n) => {
                if let Some(d) = deck(n) {
                    self.send(Command::Deck(d, DeckCommand::TogglePlay));
                }
            }
            KeyAction::KeyLock(n) => {
                if let Some(d) = deck(n) {
                    let on = !snap.decks[d.index()].key_lock;
                    self.send(Command::Deck(d, DeckCommand::SetKeyLock(on)));
                }
            }
            KeyAction::Pfl(n) => {
                if let Some(d) = deck(n) {
                    let on = !snap.mixer.channels[d.index()].pfl;
                    self.send(Command::Mixer(MixerCommand::Pfl(d, on)));
                }
            }
            KeyAction::CrossfaderLeft => {
                let x = (snap.mixer.crossfader - 0.1).max(-1.0);
                self.send(Command::Mixer(MixerCommand::Crossfader(x)));
            }
            KeyAction::CrossfaderRight => {
                let x = (snap.mixer.crossfader + 0.1).min(1.0);
                self.send(Command::Mixer(MixerCommand::Crossfader(x)));
            }
            KeyAction::CrossfaderCenter => {
                self.send(Command::Mixer(MixerCommand::Crossfader(0.0)));
            }
            KeyAction::ZoomIn => {
                self.config.ui.zoom_beats =
                    dj_core::config::clamp_zoom(self.config.ui.zoom_beats / 2.0);
            }
            KeyAction::ZoomOut => {
                self.config.ui.zoom_beats =
                    dj_core::config::clamp_zoom(self.config.ui.zoom_beats * 2.0);
            }
            KeyAction::FocusDeck(n) => self.set_keyboard_deck(index(n)),
            KeyAction::PadSecondary(n) => self.pad_secondary(kb, index(n)),
            KeyAction::PadMode(n) => {
                if let Some(m) = PadMode::ALL.get(index(n)) {
                    self.set_pad_mode(kb, *m);
                }
            }
            KeyAction::LoopIn => self.deck_command(kb, DeckCommand::LoopIn),
            KeyAction::LoopOut => self.deck_command(kb, DeckCommand::LoopOut),
            KeyAction::Reloop => self.deck_command(kb, DeckCommand::Reloop),
            KeyAction::AutoLoop => self.auto_loop(kb),
            KeyAction::LoopHalve => self.scale_loop(kb, 0.5),
            KeyAction::LoopDouble => self.scale_loop(kb, 2.0),
            KeyAction::LoopMoveBack => self.deck_command(kb, DeckCommand::LoopMove(-1)),
            KeyAction::LoopMoveForward => self.deck_command(kb, DeckCommand::LoopMove(1)),
            KeyAction::BeatJumpBack => self.beat_jump(kb, -1.0),
            KeyAction::BeatJumpForward => self.beat_jump(kb, 1.0),
            KeyAction::BeatJumpSizeDown => self.scale_jump(kb, 0.5),
            KeyAction::BeatJumpSizeUp => self.scale_jump(kb, 2.0),
            KeyAction::Sync => self.toggle_sync(kb),
            KeyAction::SyncMaster => self.make_master(kb),
            KeyAction::Slip => self.toggle_slip(kb),
            KeyAction::Quantize => {
                let on = !self.config.performance.quantize;
                self.set_quantize(on);
            }
            KeyAction::KeyShiftDown => self.key_shift_by(kb, -1),
            KeyAction::KeyShiftUp => self.key_shift_by(kb, 1),
            KeyAction::KeyShiftReset => self.key_shift_by(kb, 0),
            KeyAction::GridSetBeatOne => self.grid_edit(kb, GridEdit::SetBeatOne),
            KeyAction::GridNudgeEarlier => self.grid_edit(kb, GridEdit::Nudge(-GRID_NUDGE)),
            KeyAction::GridNudgeLater => self.grid_edit(kb, GridEdit::Nudge(GRID_NUDGE)),
            KeyAction::GridAdjustEarlier => self.grid_edit(kb, GridEdit::Adjust(-GRID_ADJUST)),
            KeyAction::GridAdjustLater => self.grid_edit(kb, GridEdit::Adjust(GRID_ADJUST)),
            KeyAction::GridHalve => self.grid_edit(kb, GridEdit::Halve),
            KeyAction::GridDouble => self.grid_edit(kb, GridEdit::Double),
            KeyAction::GridTap => self.grid_edit(kb, GridEdit::Tap),
            KeyAction::GridLock => self.grid_edit(kb, GridEdit::ToggleLock),
            KeyAction::SamplerBankPrev => {
                let b = self.sampler.bank.saturating_sub(1);
                self.set_sampler_bank(b);
            }
            KeyAction::SamplerBankNext => {
                let b = self.sampler.bank + 1;
                self.set_sampler_bank(b);
            }
            KeyAction::FxUnit(n) if pressed && (1..=2).contains(&n) => {
                self.send(Command::Fx(dj_audio::FxCommand::ToggleUnit(index(n))));
            }
            KeyAction::FxEffect(u, s)
                if pressed && (1..=2).contains(&u) && (1..=3).contains(&s) =>
            {
                self.send(Command::Fx(dj_audio::FxCommand::ToggleSlot(
                    index(u),
                    index(s),
                )));
            }
            KeyAction::FxBeatsDown | KeyAction::FxBeatsUp if pressed => {
                let d = if action == KeyAction::FxBeatsUp {
                    1
                } else {
                    -1
                };
                for u in 0..dj_audio::FX_UNITS {
                    self.send(Command::Fx(dj_audio::FxCommand::BeatsStep(u, d)));
                }
            }
            KeyAction::FxTap if pressed => self.fx_tap(),
            KeyAction::Record if pressed => self.toggle_recording(),
            KeyAction::ToggleFullscreen if pressed => {
                let on = !self.config.ui.fullscreen;
                self.set_fullscreen(on);
            }
            KeyAction::FxUnit(_)
            | KeyAction::FxEffect(..)
            | KeyAction::FxBeatsDown
            | KeyAction::FxBeatsUp
            | KeyAction::FxTap
            | KeyAction::Record
            | KeyAction::ToggleFullscreen => {}
        }
    }
}
