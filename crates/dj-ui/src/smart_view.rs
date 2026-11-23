//! The smart crate window: a name, "all" or "any" of the rules, one row
//! per rule (field, comparison, value) and the number of tracks that match
//! now. Save makes the crate (or changes it); the crate then fills itself.

use crate::app::KaldorApp;
use crate::settings_ui::{button, segmented};
use crate::theme::Palette;
use dj_library::smart::{Field, Rule, SmartRules};
use dj_library::{CrateId, Library};
use egui::{Color32, RichText};

/// A smart crate being made or edited.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartEdit {
    /// The crate edited (`None`: a new one).
    pub id: Option<CrateId>,
    /// Where a new one goes.
    pub parent: Option<CrateId>,
    pub name: String,
    pub rules: SmartRules,
    /// The name field takes the focus once.
    focus: bool,
}

impl SmartEdit {
    /// A new smart crate under `parent`.
    pub fn new(parent: Option<CrateId>) -> Self {
        Self {
            id: None,
            parent,
            name: "Smart crate".into(),
            rules: SmartRules::default(),
            focus: true,
        }
    }

    /// The window for an existing smart crate.
    pub fn open(lib: &Library, id: CrateId, name: &str) -> Option<Self> {
        let rules = lib.smart_rules(id).ok().flatten()?;
        Some(Self {
            id: Some(id),
            parent: None,
            name: name.to_string(),
            rules,
            focus: false,
        })
    }
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(if pal.dark { pal.raised } else { pal.bg })
        .stroke(egui::Stroke::new(1.0, pal.border))
        .corner_radius(14.0)
        .inner_margin(egui::Margin::same(22))
}

impl KaldorApp {
    /// Saves what the smart crate window holds (its Save button; tests).
    pub fn save_smart_now(&mut self) {
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        let Some(e) = lib.smart_edit.take() else {
            return;
        };
        if lib.protect {
            lib.status =
                "Library is protected: crates cannot be changed (Settings > Library)".into();
            return;
        }
        let name = if e.name.trim().is_empty() {
            "Smart crate".to_string()
        } else {
            e.name.trim().to_string()
        };
        let r = match e.id {
            Some(id) => lib
                .lib
                .set_smart_rules(id, &e.rules)
                .and_then(|()| lib.lib.rename_crate(id, &name))
                .map(|()| id),
            None => lib.lib.create_smart_crate(&name, e.parent, &e.rules),
        };
        match r {
            Ok(id) => {
                lib.reload();
                // A new smart sub-crate in a closed crate: open it.
                lib.reveal_crate(id);
                lib.open_crate(crate::library::Selection::Crate(id));
                lib.mark_dirty();
                lib.status = format!("Smart crate \u{201c}{name}\u{201d} saved");
            }
            Err(err) => lib.status = format!("{err}"),
        }
    }
}

/// The window, while a smart crate is made or edited.
pub(crate) fn window(app: &mut KaldorApp, ctx: &egui::Context) {
    let pal = Palette::get(app.config.ui.dark_theme);
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    let Some(mut e) = lib.smart_edit.clone() else {
        return;
    };
    let matching = e.rules.filter(lib.all_tracks(), now()).len();
    let mut save = false;
    let mut cancel = false;
    let mut marks = Vec::new();
    egui::Modal::new(egui::Id::unique("smart_crate"))
        .backdrop_color(Color32::from_black_alpha(160))
        .frame(frame(&pal))
        .show(ctx, |ui| {
            ui.set_min_width(560.0);
            ui.set_max_width(640.0);
            ui.label(
                RichText::new(if e.id.is_some() {
                    "Smart crate"
                } else {
                    "New smart crate"
                })
                .size(17.0)
                .color(pal.text),
            );
            ui.add_space(4.0);
            ui.label(
                RichText::new(
                    "Fills itself with the tracks that match, also tracks added or \
                     analysed later.",
                )
                .size(13.0)
                .color(pal.text_weak),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Name").color(pal.text));
                let r = ui.add(egui::TextEdit::singleline(&mut e.name).desired_width(260.0));
                marks.push(("smart_name", r.rect, ui.clip_rect()));
                if e.focus {
                    r.request_focus();
                    e.focus = false;
                }
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Tracks that match").color(pal.text));
                let (c, rects) = segmented(
                    ui,
                    &["all rules", "any rule"],
                    Some(usize::from(!e.rules.match_all)),
                );
                if let Some(c) = c {
                    e.rules.match_all = c == 0;
                }
                for r in rects {
                    marks.push(("smart_match", r, ui.clip_rect()));
                }
            });
            ui.add_space(8.0);
            let mut remove = None;
            let n = e.rules.rules.len();
            for (i, rule) in e.rules.rules.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    let r = egui::ComboBox::from_id_salt(("smart_field", i))
                        .selected_text(rule.field.label())
                        .width(110.0)
                        .show_ui(ui, |ui| {
                            for f in Field::ALL {
                                if ui.selectable_label(rule.field == f, f.label()).clicked()
                                    && rule.field != f
                                {
                                    let value = std::mem::take(&mut rule.value);
                                    *rule = Rule::new(f);
                                    // Text stays when the field stays text.
                                    if f.kind() == dj_library::smart::Kind::Text {
                                        rule.value = value;
                                    }
                                }
                            }
                        });
                    marks.push(("smart_field", r.response.rect, ui.clip_rect()));
                    let r = egui::ComboBox::from_id_salt(("smart_op", i))
                        .selected_text(rule.op.label())
                        .width(130.0)
                        .show_ui(ui, |ui| {
                            for o in rule.field.ops() {
                                ui.selectable_value(&mut rule.op, *o, o.label());
                            }
                        });
                    marks.push(("smart_op", r.response.rect, ui.clip_rect()));
                    let ok = rule.is_valid() || rule.value.trim().is_empty();
                    let mut edit = egui::TextEdit::singleline(&mut rule.value)
                        .hint_text(rule.field.hint(rule.op))
                        .desired_width(170.0);
                    if !ok {
                        edit = edit.text_color(pal.error);
                    }
                    let r = ui.add(edit);
                    marks.push(("smart_value", r.rect, ui.clip_rect()));
                    if n > 1 {
                        let b = button(ui, "\u{d7}", false, None).on_hover_text("Remove this rule");
                        marks.push(("smart_remove", b.rect, ui.clip_rect()));
                        if b.clicked() {
                            remove = Some(i);
                        }
                    }
                });
            }
            if let Some(i) = remove {
                e.rules.rules.remove(i);
            }
            ui.add_space(4.0);
            let b = button(ui, "+ Rule", false, None);
            marks.push(("smart_add", b.rect, ui.clip_rect()));
            if b.clicked() {
                e.rules.rules.push(Rule::new(Field::Artist));
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new(format!(
                    "{matching} track{} match now",
                    if matching == 1 { "" } else { "s" }
                ))
                .size(14.0)
                .color(pal.text),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                let b = button(ui, "Save", true, None);
                marks.push(("smart_save", b.rect, ui.clip_rect()));
                save = b.clicked();
                let c = button(ui, "Cancel", false, None);
                marks.push(("smart_cancel", c.rect, ui.clip_rect()));
                cancel = c.clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape));
            });
        });
    for (n, r, c) in marks {
        app.map.add_rect(n, r, c);
    }
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    if cancel {
        lib.smart_edit = None;
        return;
    }
    lib.smart_edit = Some(e);
    if save {
        app.save_smart_now();
    }
}
