//! Command palette (Ctrl+K) and global keyboard shortcuts.

use crate::state::{AppAction, EditorState, Mode};

#[derive(Default)]
pub struct CommandPalette {
    open: bool,
    query: String,
}

impl CommandPalette {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
    }

    pub fn show(&mut self, _ctx: &egui::Context, _state: &mut EditorState) {
        if !self.open {
            return;
        }
        self.open = false;
    }
}

pub struct Shortcuts;

impl Shortcuts {
    pub fn handle(ctx: &egui::Context, state: &mut EditorState, palette: &mut CommandPalette) {
        use egui::{Key, KeyboardShortcut, Modifiers};
        let undo = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
        let redo = KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z);
        let redo_y = KeyboardShortcut::new(Modifiers::COMMAND, Key::Y);
        let save = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
        let open = KeyboardShortcut::new(Modifiers::COMMAND, Key::O);
        let pal = KeyboardShortcut::new(Modifiers::COMMAND, Key::K);
        ctx.input_mut(|i| {
            if i.consume_shortcut(&redo) || i.consume_shortcut(&redo_y) {
                state.request(AppAction::Redo);
            }
            if i.consume_shortcut(&undo) {
                state.request(AppAction::Undo);
            }
            if i.consume_shortcut(&save) {
                state.request(AppAction::Save);
            }
            if i.consume_shortcut(&open) {
                state.request(AppAction::Open);
            }
            if i.consume_shortcut(&pal) {
                palette.open();
            }
        });
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        ctx.input(|i| {
            for m in Mode::ALL {
                let key = egui::Key::from_name(&m.shortcut().to_string());
                if key.is_some_and(|k| i.key_pressed(k) && i.modifiers.is_none()) {
                    state.mode = m;
                }
            }
        });
    }
}
