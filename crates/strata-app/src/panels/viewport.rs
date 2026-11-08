//! The map.

use crate::state::EditorState;

#[derive(Default)]
pub struct ViewportState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("The map.");
}

/// Center the camera on a world position (used by Problems, Minimap, MCP).
pub fn frame_world(_state: &mut EditorState, _p: glam::Vec2) {}

/// Right side of the status bar: cursor position, height, brush.
pub fn status_text(_state: &EditorState) -> String {
    String::new()
}
