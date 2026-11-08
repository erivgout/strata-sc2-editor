//! Localized text for every language.

use crate::state::EditorState;

#[derive(Default)]
pub struct StringsState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Localized text for every language.");
}
