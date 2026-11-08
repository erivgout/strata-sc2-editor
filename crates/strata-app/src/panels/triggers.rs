//! Events, conditions and actions that script the map.

use crate::state::EditorState;

#[derive(Default)]
pub struct TriggersState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Events, conditions and actions that script the map.");
}
