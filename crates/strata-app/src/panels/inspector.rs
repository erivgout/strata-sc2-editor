//! Tool options and the properties of whatever is selected.

use crate::state::EditorState;

#[derive(Default)]
pub struct InspectorState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Tool options and the properties of whatever is selected.");
}
