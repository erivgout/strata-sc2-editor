//! Browse and edit units, abilities, weapons and other game data.

use crate::state::EditorState;

#[derive(Default)]
pub struct DataState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Browse and edit units, abilities, weapons and other game data.");
}
