//! Every change, newest first. Click one to go back to it.

use crate::state::EditorState;

#[derive(Default)]
pub struct HistoryState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Every change, newest first. Click one to go back to it.");
}
