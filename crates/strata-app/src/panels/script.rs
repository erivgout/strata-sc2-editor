//! Generated Galaxy script (read-only preview).

use crate::state::EditorState;

#[derive(Default)]
pub struct ScriptState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Generated Galaxy script (read-only preview).");
}
