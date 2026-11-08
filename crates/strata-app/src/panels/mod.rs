//! Dockable panels. Each module exposes `XState` (UI-only state, stored in
//! `EditorState::ui`) and `show(ui, state)`.

pub mod data;
pub mod history;
pub mod inspector;
pub mod map_props;
pub mod mcp;
pub mod minimap;
pub mod problems;
pub mod script;
pub mod strings;
pub mod triggers;
pub mod viewport;

use crate::state::EditorState;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Tab {
    Viewport,
    Inspector,
    Minimap,
    Triggers,
    Script,
    Data,
    Strings,
    MapProps,
    Problems,
    History,
    Mcp,
}

impl Tab {
    pub const ALL: [Tab; 11] = [
        Tab::Viewport,
        Tab::Inspector,
        Tab::Minimap,
        Tab::Triggers,
        Tab::Script,
        Tab::Data,
        Tab::Strings,
        Tab::MapProps,
        Tab::Problems,
        Tab::History,
        Tab::Mcp,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Tab::Viewport => "Map",
            Tab::Inspector => "Inspector",
            Tab::Minimap => "Minimap",
            Tab::Triggers => "Triggers",
            Tab::Script => "Script",
            Tab::Data => "Data",
            Tab::Strings => "Text",
            Tab::MapProps => "Map properties",
            Tab::Problems => "Problems",
            Tab::History => "History",
            Tab::Mcp => "AI link",
        }
    }

    pub fn show(self, ui: &mut egui::Ui, state: &mut EditorState) {
        match self {
            Tab::Viewport => viewport::show(ui, state),
            Tab::Inspector => inspector::show(ui, state),
            Tab::Minimap => minimap::show(ui, state),
            Tab::Triggers => triggers::show(ui, state),
            Tab::Script => script::show(ui, state),
            Tab::Data => data::show(ui, state),
            Tab::Strings => strings::show(ui, state),
            Tab::MapProps => map_props::show(ui, state),
            Tab::Problems => problems::show(ui, state),
            Tab::History => history::show(ui, state),
            Tab::Mcp => mcp::show(ui, state),
        }
    }
}

/// Bridges egui_dock to our panels.
pub struct Viewer<'a> {
    pub state: &'a mut EditorState,
}

impl egui_dock::TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        tab.show(ui, self.state);
    }

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("strata-tab", *tab))
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn clear_background(&self, tab: &Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn scroll_bars(&self, tab: &Tab) -> [bool; 2] {
        match tab {
            Tab::Viewport
            | Tab::Minimap
            | Tab::Triggers
            | Tab::Data
            | Tab::Strings
            | Tab::Script => [false, false],
            _ => [false, true],
        }
    }
}
