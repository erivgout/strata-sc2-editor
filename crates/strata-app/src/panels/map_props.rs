//! Name, size, players, teams and dependencies.

use crate::state::EditorState;

#[derive(Default)]
pub struct MapPropsState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Name, size, players, teams and dependencies.");
}
