//! Opening, saving, downloading and autosave.

use strata_core::MapDocument;

use crate::state::{EditorState, ToastKind};

pub fn new_map(state: &mut EditorState) {
    state.load(MapDocument::new("Untitled map", 128, 128), None);
}

pub fn open(_ctx: &egui::Context, state: &mut EditorState) {
    state.toast(ToastKind::Info, "Opening maps is not wired up yet");
}

pub fn save(state: &mut EditorState, _save_as: bool) {
    state.toast(ToastKind::Info, "Saving maps is not wired up yet");
}

pub fn restore_autosave() -> Option<MapDocument> {
    None
}

pub fn tick_autosave(_ctx: &egui::Context, _state: &mut EditorState) {}
