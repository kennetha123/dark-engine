//! Form fields shared by the editor's panels: each reports whether it changed, so a form can
//! record one undo step per burst of editing (see [`Tracker`]).

use egui::{ComboBox, DragValue, Id, Response, Ui, emath::Numeric};

/// What names a field for egui (and so for undo): anything hashable, printable and copied.
pub trait Salt: std::hash::Hash + std::fmt::Debug + Copy {}
impl<T: std::hash::Hash + std::fmt::Debug + Copy> Salt for T {}

/// Which field of a form changed this frame, if any.
#[derive(Default)]
pub struct Tracker {
    pub changed: Option<Id>,
    /// The change was a press (add, remove, move): an undo step of its own, never merged
    /// with the next.
    pub step: bool,
}

impl Tracker {
    pub fn track(&mut self, response: Response) -> Response {
        if response.changed() {
            self.changed = Some(response.id);
        }
        response
    }

    /// Marks a change made without a widget response (a combo).
    pub fn mark(&mut self, id: impl std::hash::Hash + std::fmt::Debug) {
        self.changed = Some(Id::new(id));
    }

    /// Marks a change made by a press: each is its own undo step.
    pub fn step(&mut self, id: impl std::hash::Hash + std::fmt::Debug) {
        self.mark(id);
        self.step = true;
    }

    /// `label`, then a number field for `value` in `range`, with `help` on hover. A value out of
    /// range is left as it is until someone changes it: showing a field never edits anything.
    pub fn number<N: Numeric>(
        &mut self,
        ui: &mut Ui,
        label: &str,
        value: &mut N,
        range: std::ops::RangeInclusive<N>,
        help: &str,
    ) {
        ui.horizontal(|ui| {
            let label = ui.label(label);
            let field = DragValue::new(value)
                .range(range)
                .clamp_existing_to_range(false);
            let response = self.track(ui.add(field));
            if !help.is_empty() {
                label.on_hover_text(help);
                response.on_hover_text(help);
            }
        });
    }

    pub fn check(&mut self, ui: &mut Ui, label: &str, value: &mut bool, help: &str) {
        let response = self.track(ui.checkbox(value, label));
        if !help.is_empty() {
            response.on_hover_text(help);
        }
    }

    /// A text field for a plain string (not translated text: an id or a file name).
    pub fn text(&mut self, ui: &mut Ui, label: &str, value: &mut String, help: &str) {
        ui.horizontal(|ui| {
            ui.label(label).on_hover_text(help);
            self.track(ui.text_edit_singleline(value))
                .on_hover_text(help);
        });
    }

    /// Picks one of `options` (shown as `show` says).
    pub fn pick(
        &mut self,
        ui: &mut Ui,
        salt: impl Salt,
        label: &str,
        value: &mut String,
        options: &[String],
        show: impl Fn(&str) -> String,
    ) {
        ui.horizontal(|ui| {
            ui.label(label);
            let before = value.clone();
            ComboBox::from_id_salt(salt)
                .selected_text(show(value))
                .show_ui(ui, |ui| {
                    for option in options {
                        ui.selectable_value(value, option.clone(), show(option));
                    }
                });
            if *value != before {
                self.mark(salt);
            }
        });
    }

    /// Picks one of `options`, or none (`nothing` names that).
    #[allow(clippy::too_many_arguments)]
    pub fn pick_optional(
        &mut self,
        ui: &mut Ui,
        salt: impl Salt,
        label: &str,
        value: &mut Option<String>,
        options: &[String],
        nothing: &str,
        show: impl Fn(&str) -> String,
    ) {
        ui.horizontal(|ui| {
            ui.label(label);
            let before = value.clone();
            ComboBox::from_id_salt(salt)
                .selected_text(value.as_deref().map_or(nothing.to_owned(), &show))
                .show_ui(ui, |ui| {
                    ui.selectable_value(value, None, nothing);
                    for option in options {
                        ui.selectable_value(value, Some(option.clone()), show(option));
                    }
                });
            if *value != before {
                self.mark(salt);
            }
        });
    }

    /// Ticks any of `options` (several can be chosen), kept in `options` order.
    pub fn pick_many(
        &mut self,
        ui: &mut Ui,
        salt: impl Salt,
        label: &str,
        value: &mut Vec<String>,
        options: &[String],
        show: impl Fn(&str) -> String,
    ) {
        ui.label(label);
        ui.indent(salt, |ui| {
            ui.horizontal_wrapped(|ui| {
                for option in options {
                    let mut on = value.contains(option);
                    if ui.checkbox(&mut on, show(option)).changed() {
                        if on {
                            value.push(option.clone());
                            value.sort_by_key(|v| options.iter().position(|o| o == v));
                        } else {
                            value.retain(|v| v != option);
                        }
                        self.mark((salt, option));
                    }
                }
            });
        });
    }
}

/// Whether `id` can name something new: short, lower case letters, digits and `_`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Keeps what can be part of an id, lower-cased (typing "Old Bridge" gives "old_bridge").
pub fn tidy_id(text: &mut String) {
    let tidy: String = text
        .chars()
        .map(|c| if c == ' ' || c == '-' { '_' } else { c })
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_ascii_lowercase();
    *text = tidy;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_tidied_as_typed() {
        let mut id = "Old Bridge-2!".to_owned();
        tidy_id(&mut id);
        assert_eq!(id, "old_bridge_2");
        assert!(valid_id(&id));
        assert!(!valid_id(""));
        assert!(!valid_id("Upper"));
    }
}
