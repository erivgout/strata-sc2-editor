//! Connect an AI assistant over MCP to edit this map live.

use crate::state::EditorState;

#[derive(Default)]
pub struct McpState {}

pub fn show(ui: &mut egui::Ui, _state: &mut EditorState) {
    ui.label("Connect an AI assistant over MCP to edit this map live.");
}
