//! Errors and warnings in this map.

use crate::state::EditorState;

#[derive(Default)]
pub struct ProblemsState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Errors and warnings in this map.");
}
