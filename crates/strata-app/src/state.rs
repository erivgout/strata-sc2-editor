//! Editor state shared by every panel. Panels keep their own UI-only state in
//! their own `*State` struct (see `PanelStates`) so they can evolve
//! independently.

use std::collections::BTreeSet;
use std::sync::Arc;

use strata_core::base_data::BaseCatalog;
use strata_core::brush::Brush;
use strata_core::history::{Edit, History, Origin};
use strata_core::objects::ObjectId;
use strata_core::ops::{Op, OpError};
use strata_core::triggers::TriggerId;
use strata_core::validate::Problem;
use strata_core::MapDocument;

use crate::panels;

/// The editing mode chosen in the left mode rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Mode {
    Select,
    Sculpt,
    Paint,
    Cliff,
    Pathing,
    Units,
    Doodads,
    Points,
    Regions,
    Cameras,
}

impl Mode {
    pub const ALL: [Mode; 10] = [
        Mode::Select,
        Mode::Sculpt,
        Mode::Paint,
        Mode::Cliff,
        Mode::Pathing,
        Mode::Units,
        Mode::Doodads,
        Mode::Points,
        Mode::Regions,
        Mode::Cameras,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Select => "Select",
            Mode::Sculpt => "Sculpt",
            Mode::Paint => "Paint",
            Mode::Cliff => "Cliffs",
            Mode::Pathing => "Pathing",
            Mode::Units => "Units",
            Mode::Doodads => "Doodads",
            Mode::Points => "Points",
            Mode::Regions => "Regions",
            Mode::Cameras => "Cameras",
        }
    }

    /// Number-row shortcut (1..9, 0) shown in tooltips and the palette.
    /// Letters are left free for camera panning (WASD) and tool keys.
    pub fn shortcut(self) -> char {
        match self {
            Mode::Select => '1',
            Mode::Sculpt => '2',
            Mode::Paint => '3',
            Mode::Cliff => '4',
            Mode::Pathing => '5',
            Mode::Units => '6',
            Mode::Doodads => '7',
            Mode::Points => '8',
            Mode::Regions => '9',
            Mode::Cameras => '0',
        }
    }
}

/// A full-window arrangement of panels for one kind of work, chosen in the
/// menu bar. Each workspace keeps its own dock layout.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
pub enum Workspace {
    #[default]
    Map,
    Triggers,
    Data,
    Text,
}

impl Workspace {
    pub const ALL: [Workspace; 4] = [
        Workspace::Map,
        Workspace::Triggers,
        Workspace::Data,
        Workspace::Text,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Workspace::Map => "Map",
            Workspace::Triggers => "Triggers",
            Workspace::Data => "Data",
            Workspace::Text => "Text",
        }
    }

    /// The panel the workspace is built around.
    pub fn main_tab(self) -> panels::Tab {
        match self {
            Workspace::Map => panels::Tab::Viewport,
            Workspace::Triggers => panels::Tab::Triggers,
            Workspace::Data => panels::Tab::Data,
            Workspace::Text => panels::Tab::Strings,
        }
    }

    /// The workspace a panel belongs to, if it has a home of its own.
    pub fn home_of(tab: panels::Tab) -> Option<Workspace> {
        use panels::Tab;
        match tab {
            Tab::Viewport => Some(Workspace::Map),
            Tab::Triggers | Tab::Script => Some(Workspace::Triggers),
            Tab::Data => Some(Workspace::Data),
            Tab::Strings => Some(Workspace::Text),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub objects: BTreeSet<ObjectId>,
    pub region: Option<ObjectId>,
    pub camera: Option<ObjectId>,
    pub trigger: Option<TriggerId>,
    /// Index into `doc.data.entries`.
    pub data_entry: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Error,
}

#[derive(Debug, Clone)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    /// Seconds remaining.
    pub ttl: f32,
}

/// Requests panels make of the app shell; drained once per frame.
#[derive(Debug, Clone, PartialEq)]
pub enum AppAction {
    Undo,
    Redo,
    FocusTab(panels::Tab),
    /// Center the viewport on a world position.
    FrameWorld(glam::Vec2),
}

/// UI-only state owned by each panel module.
#[derive(Default)]
pub struct PanelStates {
    pub viewport: panels::viewport::ViewportState,
    pub inspector: panels::inspector::InspectorState,
    pub minimap: panels::minimap::MinimapState,
    pub triggers: panels::triggers::TriggersState,
    pub script: panels::script::ScriptState,
    pub data: panels::data::DataState,
    pub strings: panels::strings::StringsState,
    pub map_props: panels::map_props::MapPropsState,
    pub problems: panels::problems::ProblemsState,
    pub mcp: panels::mcp::McpState,
}

pub struct EditorState {
    pub doc: MapDocument,
    pub history: History,
    /// File name shown in the title bar, e.g. "MyMap.SC2Map".
    pub file_name: Option<String>,
    pub selection: Selection,
    pub mode: Mode,
    pub brush: Brush,
    /// Unit type / doodad model placed by the Units / Doodads modes.
    pub place_type: String,
    /// Owner for newly placed units.
    pub place_player: u8,
    pub toasts: Vec<Toast>,
    pub actions: Vec<AppAction>,
    pub ui: PanelStates,
    /// The base game's catalogs, once read from the user's install.
    pub base: Option<Arc<BaseCatalog>>,
    problems: (u64, Vec<Problem>),
}

impl EditorState {
    pub fn new(doc: MapDocument) -> Self {
        Self {
            doc,
            history: History::default(),
            file_name: None,
            selection: Selection::default(),
            mode: Mode::Sculpt,
            brush: Brush::default(),
            place_type: "Marine".into(),
            place_player: 1,
            toasts: Vec::new(),
            actions: Vec::new(),
            ui: PanelStates::default(),
            base: None,
            problems: (u64::MAX, Vec::new()),
        }
    }

    /// Replace the open document (after New / Open).
    pub fn load(&mut self, doc: MapDocument, file_name: Option<String>) {
        let ui = std::mem::take(&mut self.ui);
        *self = Self {
            mode: self.mode,
            brush: self.brush,
            base: self.base.take(),
            ui,
            ..Self::new(doc)
        };
        self.file_name = file_name;
        self.history.mark_saved();
    }

    /// Record an edit that has already been applied to `doc`.
    pub fn push_edit(&mut self, label: impl Into<String>, edit: Edit) {
        self.history.push(label, Origin::User, edit);
    }

    /// Apply and record an edit.
    pub fn commit(&mut self, label: impl Into<String>, edit: Edit) {
        self.history
            .commit(&mut self.doc, label, Origin::User, edit);
    }

    /// Run a serializable op (command palette, MCP) and record it.
    pub fn run_op(&mut self, op: Op, origin: Origin) -> Result<serde_json::Value, OpError> {
        let applied = op.run(&mut self.doc)?;
        self.history.push(applied.label, origin, applied.edit);
        Ok(applied.result)
    }

    pub fn undo(&mut self) {
        match self.history.undo(&mut self.doc).map(str::to_owned) {
            Some(l) => self.toast(ToastKind::Info, format!("Undid {l}")),
            None => self.toast(ToastKind::Info, "Nothing to undo"),
        }
        self.prune_selection();
    }

    pub fn redo(&mut self) {
        match self.history.redo(&mut self.doc).map(str::to_owned) {
            Some(l) => self.toast(ToastKind::Info, format!("Redid {l}")),
            None => self.toast(ToastKind::Info, "Nothing to redo"),
        }
        self.prune_selection();
    }

    /// Drop selected ids that no longer exist.
    pub fn prune_selection(&mut self) {
        let objs = &self.doc.objects;
        self.selection.objects.retain(|id| objs.get(*id).is_some());
        if let Some(r) = self.selection.region {
            if !self.doc.regions.iter().any(|x| x.id == r) {
                self.selection.region = None;
            }
        }
        if let Some(i) = self.selection.data_entry {
            if i >= self.doc.data.entries.len() {
                self.selection.data_entry = None;
            }
        }
    }

    pub fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toasts.push(Toast {
            kind,
            text: text.into(),
            ttl: 3.5,
        });
    }

    pub fn request(&mut self, a: AppAction) {
        self.actions.push(a);
    }

    /// Validation results, recomputed only when the document changes.
    pub fn problems(&mut self) -> &[Problem] {
        let rev = self.history.revision();
        if self.problems.0 != rev {
            self.problems = (rev, strata_core::validate::validate(&self.doc));
        }
        &self.problems.1
    }

    pub fn title(&self) -> String {
        let name = self.file_name.as_deref().unwrap_or(&self.doc.info.name);
        let star = if self.history.is_modified() {
            " •"
        } else {
            ""
        };
        format!("{name}{star} — Strata")
    }
}
