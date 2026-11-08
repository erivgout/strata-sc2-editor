//! Live link to a local `strata-mcp` server: receives ops from AI clients and
//! applies them to the open document as undoable edits.

use crate::state::EditorState;

/// Called once per frame by the app shell, before panels are drawn.
pub fn tick(_ctx: &egui::Context, _state: &mut EditorState) {}
