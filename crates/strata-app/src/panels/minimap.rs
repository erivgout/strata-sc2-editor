//! Overview of the whole map. Click to move the camera.

use crate::state::EditorState;

#[derive(Default)]
pub struct MinimapState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Overview of the whole map. Click to move the camera.");
}
