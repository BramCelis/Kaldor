//! Settings > Keyboard: every keyboard action with its key; Change waits
//! for the next key press (Escape cancels), Clear removes the key, and
//! Reset puts the default keymap back. A key given to one action is taken
//! from any other action that had it.

use crate::app::{AppEvent, KaldorApp};
use crate::settings_ui::{button, card};
use crate::theme::Palette;
use dj_core::config::{KeyAction, KeyBinding, default_keymap, key_label};
use egui::RichText;

/// Every action, in the order of the default keymap, then any others the
/// keymap has.
fn actions(keymap: &[KeyBinding]) -> Vec<KeyAction> {
    let mut out: Vec<KeyAction> = Vec::new();
    for b in default_keymap().iter().chain(keymap) {
        if !out.contains(&b.action) {
            out.push(b.action);
        }
    }
    out
}

/// Gives `key` (with `shift`) to `action`: taken from any other action,
/// replacing the action's first key (or added). Returns the actions that
/// lost the key.
pub(crate) fn assign(
    keymap: &mut Vec<KeyBinding>,
    action: KeyAction,
    key: &str,
    shift: bool,
) -> Vec<KeyAction> {
    let mut lost = Vec::new();
    keymap.retain(|b| {
        let same = b.key == key && b.shift == shift;
        if same && b.action != action {
            lost.push(b.action);
        }
        !same || b.action == action
    });
    if keymap
        .iter()
        .any(|b| b.action == action && b.key == key && b.shift == shift)
    {
        return lost;
    }
    match keymap.iter_mut().find(|b| b.action == action) {
        Some(b) => {
            b.key = key.to_string();
            b.shift = shift;
        }
        None => keymap.push(KeyBinding {
            key: key.to_string(),
            shift,
            action,
        }),
    }
    lost
}

impl KaldorApp {
    /// While a key is being changed: the next key press (taken before the
    /// app's own key handling, which waits meanwhile).
    fn capture_key(&mut self, ui: &egui::Ui) {
        let Some(action) = self.key_capture else {
            return;
        };
        let press = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                } => Some((*key, *physical_key, modifiers.shift)),
                _ => None,
            })
        });
        let Some((key, physical, shift)) = press else {
            return;
        };
        self.key_capture = None;
        if key == egui::Key::Escape {
            self.key_message = "Not changed.".into();
            return;
        }
        // The physical key, so Shift+1 is "Digit1" and not "!".
        let k = physical.unwrap_or(key);
        let name = k.name().to_string();
        let lost = assign(&mut self.config.ui.keymap, action, &name, shift);
        self.key_message = if lost.is_empty() {
            format!("{} is now {}.", action.label(), key_label(&name, shift))
        } else {
            format!(
                "{} is now {} (taken from {}).",
                action.label(),
                key_label(&name, shift),
                lost.iter()
                    .map(|a| a.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        self.events.push(AppEvent::ConfigChanged);
    }

    /// Changes a key as a press in the Keyboard tab would (tests).
    pub fn assign_key_for_test(&mut self, action: KeyAction, key: &str, shift: bool) {
        assign(&mut self.config.ui.keymap, action, key, shift);
        self.events.push(AppEvent::ConfigChanged);
    }
}

/// The Keyboard tab.
pub(crate) fn keyboard_setup(app: &mut KaldorApp, ui: &mut egui::Ui) {
    app.capture_key(ui);
    let pal = Palette::get(app.config.ui.dark_theme);
    let mut reset = false;
    let message = app.key_message.clone();
    card(ui, &pal, "Keyboard", |rows| {
        rows.row(
            "Keys",
            if message.is_empty() {
                "Change, then press the new key (with Shift if you like). Escape cancels. A key used elsewhere moves to the new action."
            } else {
                &message
            },
            |ui| {
                let b = button(ui, "Reset to defaults", false, None);
                reset = b.clicked();
                app.map.add(ui, "key_reset", None, b.rect);
            },
        );
    });
    if reset {
        app.config.ui.keymap = default_keymap();
        app.key_capture = None;
        app.key_message = "All keys are back to the defaults.".into();
        app.events.push(AppEvent::ConfigChanged);
    }
    let all = actions(&app.config.ui.keymap);
    let mut groups: Vec<&'static str> = Vec::new();
    for a in &all {
        if !groups.contains(&a.group()) {
            groups.push(a.group());
        }
    }
    let mut change: Option<KeyAction> = None;
    let mut clear: Option<KeyAction> = None;
    for g in groups {
        card(ui, &pal, g, |rows| {
            for a in all.iter().filter(|a| a.group() == g) {
                let keys: Vec<String> = app
                    .config
                    .ui
                    .keymap
                    .iter()
                    .filter(|b| b.action == *a)
                    .map(|b| key_label(&b.key, b.shift))
                    .collect();
                let waiting = app.key_capture == Some(*a);
                rows.row(&a.label(), "", |ui| {
                    let c = button(ui, "Clear", false, None);
                    if c.clicked() {
                        clear = Some(*a);
                    }
                    app.map.add(ui, "key_clear", None, c.rect);
                    let b = button(
                        ui,
                        if waiting { "Press a key…" } else { "Change" },
                        waiting,
                        None,
                    );
                    if b.clicked() {
                        change = Some(*a);
                    }
                    app.map.add(ui, "key_change", None, b.rect);
                    ui.add_space(8.0);
                    let text = if keys.is_empty() {
                        "not set".to_string()
                    } else {
                        keys.join("  /  ")
                    };
                    ui.label(RichText::new(text).font(crate::theme::mono(13.0)).color(
                        if keys.is_empty() {
                            pal.text_weak
                        } else {
                            pal.text
                        },
                    ));
                });
            }
        });
    }
    if let Some(a) = change {
        app.key_capture = if app.key_capture == Some(a) {
            None
        } else {
            Some(a)
        };
        app.key_message.clear();
    }
    if let Some(a) = clear {
        app.config.ui.keymap.retain(|b| b.action != a);
        app.key_message = format!("{} has no key now.", a.label());
        app.events.push(AppEvent::ConfigChanged);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assigning_a_used_key_moves_it() {
        let mut km = default_keymap();
        // W is deck 1 play; give it to deck 2 play.
        let lost = assign(&mut km, KeyAction::Play(2), "W", false);
        assert_eq!(lost, vec![KeyAction::Play(1)]);
        assert!(
            km.iter()
                .any(|b| b.action == KeyAction::Play(2) && b.key == "W")
        );
        assert!(!km.iter().any(|b| b.action == KeyAction::Play(1)));
        // Shift+Insert is free: nothing is taken.
        let lost = assign(&mut km, KeyAction::Play(1), "Insert", true);
        assert!(lost.is_empty());
        assert!(
            km.iter()
                .any(|b| b.action == KeyAction::Play(1) && b.key == "Insert" && b.shift)
        );
        // Every action has a label and a group.
        for a in actions(&km) {
            assert!(!a.label().is_empty() && !a.group().is_empty());
        }
    }
}
