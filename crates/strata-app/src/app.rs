//! The application shell: menu bar, mode rail, dock area, status bar,
//! dialogs (welcome, new map, unsaved changes, about), notices and toasts.

use egui::{Color32, Pos2, Rect, RichText, Stroke, Vec2};
use egui_dock::{DockArea, DockState, NodeIndex};
use strata_core::history::{Edit, Origin};
use strata_core::validate::Severity;
use strata_core::MapDocument;

use crate::commands::{self, Command, CommandPalette, ShortcutsOverlay};
use crate::io::{Io, IoEvent, SaveError};
use crate::panels::{Tab, Viewer};
use crate::state::{AppAction, EditorState, Mode, ToastKind};
use crate::theme::{self, ThemePref, Tokens};

const SETTINGS_KEY: &str = "strata.settings.v1";
const MAX_RECENT_FILES: usize = 8;
const MAX_RECENT_COMMANDS: usize = 8;

/// Settings persisted across sessions (eframe storage: localStorage on web).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePref,
    pub dock: DockState<Tab>,
    /// Recently opened files, newest first (native paths).
    pub recent: Vec<String>,
    /// Recently run palette commands, newest first.
    pub recent_commands: Vec<Command>,
    pub show_welcome: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::Dark,
            dock: default_dock(),
            recent: Vec::new(),
            recent_commands: Vec::new(),
            show_welcome: true,
        }
    }
}

pub fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [main, right] =
        surface.split_right(NodeIndex::root(), 0.77, vec![Tab::Inspector, Tab::MapProps]);
    surface.split_below(right, 0.64, vec![Tab::Minimap]);
    surface.split_below(
        main,
        0.68,
        vec![
            Tab::Triggers,
            Tab::Data,
            Tab::Strings,
            Tab::Script,
            Tab::Problems,
            Tab::History,
            Tab::Mcp,
        ],
    );
    dock
}

/// Sizes offered by the new map dialog (square maps).
const SIZE_PRESETS: [u32; 6] = [64, 96, 128, 160, 192, 256];

/// Tileset ids and display names offered by the new map dialog.
const TILESETS: [(&str, &str); 14] = [
    ("Char", "Char (volcanic)"),
    ("Agria", "Agria (grassland)"),
    ("BelShir", "Bel'Shir (jungle)"),
    ("Korhal", "Korhal (city)"),
    ("MarSara", "Mar Sara (desert)"),
    ("Zerus", "Zerus (primal)"),
    ("Aiur", "Aiur"),
    ("Shakuras", "Shakuras"),
    ("Ulnar", "Ulnar (ruins)"),
    ("Braxis", "Braxis (ice)"),
    ("Avernus", "Avernus"),
    ("Redstone", "Redstone"),
    ("Xil", "Xil (lava)"),
    ("Haven", "Haven"),
];

struct NewMapForm {
    name: String,
    preset: Option<u32>,
    width: u32,
    height: u32,
    tileset: String,
    base_level: u8,
}

impl Default for NewMapForm {
    fn default() -> Self {
        Self {
            name: "Untitled map".into(),
            preset: Some(128),
            width: 128,
            height: 128,
            tileset: "Char".into(),
            base_level: 2,
        }
    }
}

impl NewMapForm {
    fn size(&self) -> (u32, u32) {
        match self.preset {
            Some(s) => (s, s),
            None => (snap8(self.width), snap8(self.height)),
        }
    }

    fn build(&self) -> MapDocument {
        let (w, h) = self.size();
        let mut doc = MapDocument::new(self.name.trim(), w, h);
        doc.info.tileset = self.tileset.clone();
        let level = self.base_level.min(15);
        let t = &mut doc.terrain;
        let detail = t.heights.first().copied().unwrap_or(0.0)
            - t.cliff_levels.first().copied().unwrap_or(0) as f32
                * strata_core::terrain::CLIFF_LEVEL_HEIGHT;
        t.cliff_levels.iter_mut().for_each(|c| *c = level);
        let height = level as f32 * strata_core::terrain::CLIFF_LEVEL_HEIGHT + detail;
        t.heights.iter_mut().for_each(|v| *v = height);
        doc
    }
}

fn snap8(v: u32) -> u32 {
    (((v + 4) / 8) * 8).clamp(16, 256)
}

/// What to do after the unsaved-changes guard is answered.
enum Then {
    Run(Command),
    Load(Box<MapDocument>, String, Option<String>),
}

enum Dialog {
    Welcome,
    NewMap(NewMapForm),
    Unsaved(Then),
    About,
}

#[derive(Clone)]
enum NoticeAction {
    Run(Command),
    DiscardRestored,
    Reload,
    Dismiss,
}

/// A persistent card with actions (unlike toasts, which time out).
struct Notice {
    key: &'static str,
    kind: ToastKind,
    text: String,
    actions: Vec<(&'static str, NoticeAction)>,
}

pub struct StrataApp {
    pub state: EditorState,
    pub settings: Settings,
    pub palette: CommandPalette,
    shortcuts: ShortcutsOverlay,
    io: Io,
    dialog: Option<Dialog>,
    notices: Vec<Notice>,
    applied_theme: Option<(ThemePref, bool)>,
    last_title: String,
    can_install: bool,
    last_web_poll: f64,
    /// Run after the current save completes (from the unsaved guard).
    after_save: Option<Then>,
    /// Deferred follow-up of a guard, run at the start of the next frame.
    pending_then: Option<Then>,
    /// Commands requested this frame (menus, rail, palette, keyboard).
    queued: Vec<Command>,
}

impl StrataApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        let mut app = Self {
            state: EditorState::new(MapDocument::new("Untitled map", 128, 128)),
            settings,
            palette: CommandPalette::default(),
            shortcuts: ShortcutsOverlay::default(),
            io: Io::default(),
            dialog: None,
            notices: Vec::new(),
            applied_theme: None,
            last_title: String::new(),
            can_install: false,
            last_web_poll: -10.0,
            after_save: None,
            pending_then: None,
            queued: Vec::new(),
        };
        if let Some(rec) = crate::io::restore_autosave() {
            app.state.load(rec.doc, rec.file_name);
            // Mark the restored map as unsaved so the guard and title show it.
            app.state
                .history
                .push("Restore unsaved map", Origin::User, Edit::Batch(Vec::new()));
            app.io.note_restored(app.state.history.revision());
            app.notice(Notice {
                key: "restored",
                kind: ToastKind::Info,
                text: format!(
                    "Restored your unsaved map from {}. Save it to keep it.",
                    crate::io::relative_time(rec.saved_at_ms)
                ),
                actions: vec![
                    ("Keep", NoticeAction::Dismiss),
                    ("Discard", NoticeAction::DiscardRestored),
                ],
            });
        } else if app.settings.show_welcome {
            app.dialog = Some(Dialog::Welcome);
        }
        app
    }

    fn notice(&mut self, n: Notice) {
        self.notices.retain(|o| o.key != n.key);
        self.notices.push(n);
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let system_dark = ctx.input(|i| {
            i.raw
                .system_theme
                .map(|t| t == egui::Theme::Dark)
                .unwrap_or(true)
        });
        let key = (self.settings.theme, system_dark);
        if self.applied_theme != Some(key) {
            theme::apply(ctx, self.settings.theme.resolve(system_dark));
            self.applied_theme = Some(key);
        }
    }

    // ---------------------------------------------------------------------
    // Commands

    fn remember_command(&mut self, cmd: &Command) {
        if matches!(cmd, Command::CommandPalette) {
            return;
        }
        let r = &mut self.settings.recent_commands;
        r.retain(|c| c != cmd);
        r.insert(0, cmd.clone());
        r.truncate(MAX_RECENT_COMMANDS);
    }

    pub fn run(&mut self, ctx: &egui::Context, cmd: Command) {
        self.run_inner(ctx, cmd, false);
    }

    fn run_inner(&mut self, ctx: &egui::Context, cmd: Command, confirmed: bool) {
        let replaces_doc = matches!(
            cmd,
            Command::NewMap | Command::OpenMap | Command::OpenFolder | Command::OpenRecent(_)
        );
        if replaces_doc && !confirmed && self.state.history.is_modified() {
            self.dialog = Some(Dialog::Unsaved(Then::Run(cmd)));
            return;
        }
        match cmd {
            Command::NewMap => self.dialog = Some(Dialog::NewMap(NewMapForm::default())),
            Command::OpenMap => self.io.open(ctx),
            Command::OpenFolder => {
                #[cfg(not(target_arch = "wasm32"))]
                self.io.open_folder(ctx);
            }
            Command::OpenRecent(path) => {
                #[cfg(not(target_arch = "wasm32"))]
                self.io.open_path(ctx, path);
                #[cfg(target_arch = "wasm32")]
                let _ = path;
            }
            Command::Save => self.save(ctx, false),
            Command::SaveAs => self.save(ctx, true),
            Command::ExportProject => {
                let r =
                    self.io
                        .export_project(ctx, &self.state.doc, self.state.file_name.as_deref());
                self.finish_save(r, true);
            }
            Command::Undo => self.state.undo(),
            Command::Redo => self.state.redo(),
            Command::CommandPalette => self.palette.toggle(),
            Command::ShowShortcuts => self.shortcuts.open = !self.shortcuts.open,
            Command::SetMode(m) => self.state.mode = m,
            Command::FocusTab(t) => self.focus_tab(t),
            Command::Theme(p) => self.settings.theme = p,
            Command::ResetLayout => self.settings.dock = default_dock(),
            Command::ConnectAi => self.focus_tab(Tab::Mcp),
            Command::FrameMap => {
                let center = self.state.doc.world_size() * 0.5;
                crate::panels::viewport::frame_world(&mut self.state, center);
                self.focus_tab(Tab::Viewport);
            }
            Command::ShowProblems => {
                let (errors, warnings) = self.problem_counts();
                self.focus_tab(Tab::Problems);
                let msg = match (errors, warnings) {
                    (0, 0) => "No problems found".to_owned(),
                    (e, w) => format!("{} and {}", plural(e, "error"), plural(w, "warning")),
                };
                let kind = if errors > 0 {
                    ToastKind::Error
                } else {
                    ToastKind::Success
                };
                self.state.toast(kind, msg);
            }
            Command::InstallApp => crate::io::install(),
            Command::ShowWelcome => self.dialog = Some(Dialog::Welcome),
            Command::About => self.dialog = Some(Dialog::About),
        }
    }

    fn focus_tab(&mut self, tab: Tab) {
        let dock = &mut self.settings.dock;
        if let Some(found) = dock.find_tab(&tab) {
            let _ = dock.set_active_tab(found);
            dock.set_focused_node_and_surface(found.node_path());
            return;
        }
        // Closed: reopen it next to its usual neighbours.
        let neighbour = match tab {
            Tab::Inspector | Tab::MapProps | Tab::Minimap => {
                [Tab::Inspector, Tab::MapProps, Tab::Minimap]
            }
            _ => [Tab::Triggers, Tab::Problems, Tab::History],
        };
        if let Some(found) = neighbour.iter().find_map(|n| dock.find_tab(n)) {
            dock.set_focused_node_and_surface(found.node_path());
        }
        dock.push_to_focused_leaf(tab);
    }

    fn problem_counts(&mut self) -> (usize, usize) {
        let p = self.state.problems();
        let errors = p.iter().filter(|p| p.severity == Severity::Error).count();
        let warnings = p.iter().filter(|p| p.severity == Severity::Warning).count();
        (errors, warnings)
    }

    fn drain_actions(&mut self, ctx: &egui::Context) {
        for a in std::mem::take(&mut self.state.actions) {
            #[allow(unreachable_patterns)]
            let cmd = match a {
                AppAction::NewMap => Command::NewMap,
                AppAction::Open => Command::OpenMap,
                AppAction::Save => Command::Save,
                AppAction::SaveAs => Command::SaveAs,
                AppAction::Undo => Command::Undo,
                AppAction::Redo => Command::Redo,
                AppAction::OpenCommandPalette => Command::CommandPalette,
                AppAction::FocusTab(tab) => Command::FocusTab(tab),
                AppAction::FrameWorld(p) => {
                    crate::panels::viewport::frame_world(&mut self.state, p);
                    continue;
                }
                _ => continue,
            };
            self.run(ctx, cmd);
        }
    }

    // ---------------------------------------------------------------------
    // Files

    fn save(&mut self, ctx: &egui::Context, save_as: bool) {
        let r = self.io.save(
            ctx,
            &self.state.doc,
            self.state.file_name.as_deref(),
            save_as,
        );
        self.finish_save(r, false);
    }

    fn finish_save(&mut self, result: Result<Option<String>, SaveError>, project: bool) {
        match result {
            Ok(Some(name)) => {
                self.state.history.mark_saved();
                crate::io::clear_autosave();
                self.notices
                    .retain(|n| n.key != "save-failed" && n.key != "restored");
                let verb = if project { "Exported" } else { "Saved" };
                if !project || self.state.file_name.is_none() {
                    self.state.file_name = Some(name.clone());
                }
                self.state
                    .toast(ToastKind::Success, format!("{verb} {name}"));
                if let Some(then) = self.after_save.take() {
                    self.pending_then = Some(then);
                }
            }
            Ok(None) => {}
            Err(e) => {
                self.after_save = None;
                let mut actions = Vec::new();
                let mut text = e.message;
                if e.offer_export {
                    text.push_str(" Export a Strata project instead: it keeps everything and opens in Strata.");
                    actions.push(("Export project", NoticeAction::Run(Command::ExportProject)));
                }
                actions.push(("Dismiss", NoticeAction::Dismiss));
                self.notice(Notice {
                    key: "save-failed",
                    kind: ToastKind::Error,
                    text,
                    actions,
                });
            }
        }
    }

    fn handle_io(&mut self) {
        while let Some(ev) = self.io.poll() {
            match ev {
                IoEvent::Opened { doc, name, path } => {
                    if self.state.history.is_modified() {
                        self.dialog = Some(Dialog::Unsaved(Then::Load(doc, name, path)));
                    } else {
                        self.load(*doc, name, path);
                    }
                }
                IoEvent::OpenFailed { message } => {
                    self.notice(Notice {
                        key: "open-failed",
                        kind: ToastKind::Error,
                        text: message,
                        actions: vec![("Dismiss", NoticeAction::Dismiss)],
                    });
                }
                #[cfg(not(target_arch = "wasm32"))]
                IoEvent::SaveTo { path, project } => {
                    let r = self.io.write_native(&self.state.doc, &path, project);
                    self.finish_save(r.map(Some), project);
                }
                IoEvent::Cancelled => self.after_save = None,
            }
        }
    }

    fn load(&mut self, doc: MapDocument, name: String, path: Option<String>) {
        self.state.load(doc, Some(name.clone()));
        self.io.set_opened_path(path.as_deref());
        if let Some(p) = path {
            self.settings.recent.retain(|r| r != &p);
            self.settings.recent.insert(0, p);
            self.settings.recent.truncate(MAX_RECENT_FILES);
        }
        self.notices
            .retain(|n| n.key != "open-failed" && n.key != "restored");
        if matches!(self.dialog, Some(Dialog::Welcome)) {
            self.dialog = None;
        }
        self.state
            .toast(ToastKind::Success, format!("Opened {name}"));
        let center = self.state.doc.world_size() * 0.5;
        crate::panels::viewport::frame_world(&mut self.state, center);
    }

    fn new_map(&mut self, doc: MapDocument) {
        let name = doc.info.name.clone();
        self.state.load(doc, None);
        self.io.reset_path();
        self.notices.retain(|n| n.key != "restored");
        self.state
            .toast(ToastKind::Success, format!("Created {name}"));
        let center = self.state.doc.world_size() * 0.5;
        crate::panels::viewport::frame_world(&mut self.state, center);
    }

    fn do_then(&mut self, ctx: &egui::Context, then: Then) {
        match then {
            Then::Run(cmd) => self.run_inner(ctx, cmd, true),
            Then::Load(doc, name, path) => self.load(*doc, name, path),
        }
    }

    fn handle_drops(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        if let Some(file) = dropped.into_iter().next() {
            self.io.open_dropped(ctx, file);
        }
    }

    fn poll_web(&mut self, ctx: &egui::Context) {
        if cfg!(not(target_arch = "wasm32")) {
            return;
        }
        let now = ctx.input(|i| i.time);
        if now - self.last_web_poll < 2.0 {
            return;
        }
        self.last_web_poll = now;
        self.can_install = crate::io::can_install();
        if crate::io::update_ready() && !self.notices.iter().any(|n| n.key == "update") {
            self.notice(Notice {
                key: "update",
                kind: ToastKind::Info,
                text: "A new version of Strata is ready. Reload to use it; save your map first."
                    .into(),
                actions: vec![
                    ("Reload", NoticeAction::Reload),
                    ("Later", NoticeAction::Dismiss),
                ],
            });
        }
        ctx.request_repaint_after(std::time::Duration::from_secs(2));
    }

    // ---------------------------------------------------------------------
    // Chrome

    fn menu_cmd(&mut self, ui: &mut egui::Ui, cmd: Command) {
        self.menu_cmd_labeled(ui, cmd.label(), cmd, true);
    }

    fn menu_cmd_labeled(&mut self, ui: &mut egui::Ui, label: String, cmd: Command, enabled: bool) {
        let mut b = egui::Button::new(label);
        if let Some(s) = cmd.shortcut() {
            b = b.shortcut_text(ui.ctx().format_shortcut(&s));
        }
        if ui.add_enabled(enabled, b).clicked() {
            self.queued.push(cmd);
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let bar = ui.max_rect();
        egui::MenuBar::new().ui(ui, |ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::new(22.0, 20.0), egui::Sense::hover());
            paint_logo(ui.painter(), r.center(), 16.0, &t);
            ui.add_space(2.0);
            ui.menu_button("File", |ui| {
                self.menu_cmd(ui, Command::NewMap);
                self.menu_cmd(ui, Command::OpenMap);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    self.menu_cmd(ui, Command::OpenFolder);
                    let recent = self.settings.recent.clone();
                    ui.add_enabled_ui(!recent.is_empty(), |ui| {
                        ui.menu_button("Open recent", |ui| {
                            for p in recent {
                                let label = commands::display_name(&p).to_owned();
                                if ui.button(label).on_hover_text(&p).clicked() {
                                    self.queued.push(Command::OpenRecent(p));
                                }
                            }
                            ui.separator();
                            if ui.button("Clear recent files").clicked() {
                                self.settings.recent.clear();
                            }
                        });
                    });
                }
                ui.separator();
                self.menu_cmd(ui, Command::Save);
                self.menu_cmd(ui, Command::SaveAs);
                self.menu_cmd(ui, Command::ExportProject);
                #[cfg(not(target_arch = "wasm32"))]
                {
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            });
            ui.menu_button("Edit", |ui| {
                let undo = self
                    .state
                    .history
                    .undo_entries()
                    .last()
                    .map(|e| format!("Undo {}", e.label));
                let redo = self
                    .state
                    .history
                    .redo_entries()
                    .last()
                    .map(|e| format!("Redo {}", e.label));
                let (can_undo, can_redo) = (undo.is_some(), redo.is_some());
                self.menu_cmd_labeled(
                    ui,
                    undo.unwrap_or_else(|| "Undo".into()),
                    Command::Undo,
                    can_undo,
                );
                self.menu_cmd_labeled(
                    ui,
                    redo.unwrap_or_else(|| "Redo".into()),
                    Command::Redo,
                    can_redo,
                );
                ui.separator();
                self.menu_cmd(ui, Command::CommandPalette);
            });
            ui.menu_button("View", |ui| {
                ui.menu_button("Modes", |ui| {
                    for m in Mode::ALL {
                        self.menu_cmd(ui, Command::SetMode(m));
                    }
                });
                ui.menu_button("Panels", |ui| {
                    for tab in Tab::ALL {
                        self.menu_cmd_labeled(ui, tab.title().into(), Command::FocusTab(tab), true);
                    }
                });
                ui.separator();
                ui.label(RichText::new("Theme").small().color(t.text_dim));
                for p in ThemePref::ALL {
                    if ui.radio(self.settings.theme == p, p.label()).clicked() {
                        self.queued.push(Command::Theme(p));
                    }
                }
                ui.separator();
                self.menu_cmd(ui, Command::ResetLayout);
            });
            ui.menu_button("Map", |ui| {
                self.menu_cmd_labeled(
                    ui,
                    "Map properties".into(),
                    Command::FocusTab(Tab::MapProps),
                    true,
                );
                self.menu_cmd(ui, Command::FrameMap);
                self.menu_cmd(ui, Command::ShowProblems);
                ui.separator();
                self.menu_cmd(ui, Command::ConnectAi);
            });
            ui.menu_button("Help", |ui| {
                self.menu_cmd(ui, Command::ShowShortcuts);
                self.menu_cmd(ui, Command::ShowWelcome);
                if self.can_install {
                    self.menu_cmd(ui, Command::InstallApp);
                }
                ui.separator();
                self.menu_cmd(ui, Command::About);
            });

            // Command palette affordance, centered on the bar.
            let w = 300.0_f32.min(bar.width() - ui.min_rect().right() * 2.0 - 16.0);
            if w > 160.0 {
                let rect = Rect::from_center_size(
                    Pos2::new(bar.center().x, ui.min_rect().center().y),
                    Vec2::new(w, 24.0),
                );
                let id = ui.id().with("palette-hint");
                let resp = ui.interact(rect, id, egui::Sense::click());
                let hovered = resp.hovered();
                let p = ui.painter();
                let dark = ui.visuals().dark_mode;
                p.rect(
                    rect,
                    6.0,
                    if hovered {
                        theme::hover_fill(&t, dark)
                    } else {
                        t.bg_deep
                    },
                    Stroke::new(1.0, t.stroke),
                    egui::StrokeKind::Inside,
                );
                commands::paint_search_icon(
                    p,
                    Pos2::new(rect.left() + 16.0, rect.center().y),
                    t.text_dim,
                );
                p.text(
                    Pos2::new(rect.left() + 30.0, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    "Search commands…",
                    egui::FontId::proportional(14.0),
                    t.text_dim,
                );
                let keys = ui
                    .ctx()
                    .format_shortcut(&Command::CommandPalette.shortcut().unwrap_or(
                        egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K),
                    ));
                p.text(
                    Pos2::new(rect.right() - 10.0, rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    keys,
                    egui::FontId::proportional(12.0),
                    t.text_dim,
                );
                if resp
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    self.queued.push(Command::CommandPalette);
                }
            }
        });
    }

    fn mode_rail(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let dark = ui.visuals().dark_mode;
        ui.add_space(6.0);
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (i, m) in Mode::ALL.into_iter().enumerate() {
                // Group: select | terrain | objects | markers.
                if matches!(i, 1 | 5 | 7) {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(28.0, 9.0), egui::Sense::hover());
                    ui.painter()
                        .hline(r.x_range(), r.center().y, Stroke::new(1.0, t.stroke));
                }
                let selected = self.state.mode == m;
                let (rect, resp) =
                    ui.allocate_exact_size(Vec2::new(40.0, 40.0), egui::Sense::click());
                let p = ui.painter();
                if selected {
                    p.rect_filled(rect, 6.0, theme::selection_fill(&t, dark));
                    let bar = Rect::from_min_size(
                        Pos2::new(ui.max_rect().left() - 2.0, rect.top() + 8.0),
                        Vec2::new(3.0, rect.height() - 16.0),
                    );
                    p.rect_filled(bar, 1.5, t.accent);
                } else if resp.hovered() {
                    p.rect_filled(rect, 6.0, theme::hover_fill(&t, dark));
                }
                if resp.has_focus() {
                    p.rect_stroke(
                        rect,
                        6.0,
                        Stroke::new(1.5, t.accent),
                        egui::StrokeKind::Inside,
                    );
                }
                let color = if selected {
                    t.accent_text
                } else if resp.hovered() {
                    t.text
                } else {
                    t.text_dim
                };
                paint_mode_icon(p, m, rect.center() - Vec2::new(0.0, 1.0), color);
                p.text(
                    rect.right_bottom() + Vec2::new(-4.0, -2.0),
                    egui::Align2::RIGHT_BOTTOM,
                    m.shortcut().to_string(),
                    egui::FontId::proportional(10.0),
                    t.text_dim.gamma_multiply(0.8),
                );
                let resp = resp.on_hover_ui(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(format!("{} mode", m.label())).family(theme::medium()),
                        );
                        commands::keycap(ui, &m.shortcut().to_string(), &t);
                    });
                });
                if resp.clicked() {
                    self.queued.push(Command::SetMode(m));
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let (errors, warnings) = self.problem_counts();
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let modified = self.state.history.is_modified();
            let name = self
                .state
                .file_name
                .clone()
                .unwrap_or_else(|| self.state.doc.info.name.clone());
            ui.label(RichText::new(name).family(theme::medium()).color(t.text));
            if modified {
                ui.label(RichText::new("• Unsaved").small().color(t.text_dim))
                    .on_hover_text("Changes are autosaved every 30 seconds until you save.");
            }
            dot(ui, &t);
            let i = &self.state.doc.info;
            ui.label(
                RichText::new(format!("{} × {}", i.width, i.height))
                    .small()
                    .color(t.text_dim),
            );
            dot(ui, &t);
            let (text, color) = match (errors, warnings) {
                (0, 0) => ("No problems".to_owned(), t.text_dim),
                (0, w) => (plural(w, "warning"), t.warn),
                (e, 0) => (plural(e, "error"), t.error),
                (e, w) => (
                    format!("{}, {}", plural(e, "error"), plural(w, "warning")),
                    t.error,
                ),
            };
            let resp = ui
                .add(
                    egui::Label::new(RichText::new(text).small().color(color))
                        .sense(egui::Sense::click()),
                )
                .on_hover_text("Show problems");
            if resp.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if resp.clicked() {
                self.queued.push(Command::FocusTab(Tab::Problems));
            }
            if let Some(busy) = &self.io.busy {
                dot(ui, &t);
                ui.add(egui::Spinner::new().size(12.0).color(t.accent));
                ui.label(RichText::new(busy).small().color(t.text_dim));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let dt = ui.input(|i| i.stable_dt);
                if dt > 0.0 && dt < 0.25 {
                    ui.label(
                        RichText::new(format!("{:.0} fps", 1.0 / dt))
                            .small()
                            .color(t.text_dim),
                    );
                    dot(ui, &t);
                }
                ui.label(
                    RichText::new(format!("{} mode", self.state.mode.label()))
                        .small()
                        .color(t.text_dim),
                );
                let s = crate::panels::viewport::status_text(&self.state);
                if !s.is_empty() {
                    dot(ui, &t);
                    ui.label(RichText::new(s).small().color(t.text_dim));
                }
            });
        });
    }

    // ---------------------------------------------------------------------
    // Overlays

    fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(dialog) = self.dialog.take() else {
            return;
        };
        let t = theme::tokens(ctx);
        let mut keep = true;
        let mut next: Option<Dialog> = None;
        let mut then: Option<Then> = None;
        let mut created: Option<MapDocument> = None;
        let mut dialog = dialog;
        let id = egui::Id::new("strata-dialog");
        let frame = egui::Frame::popup(&ctx.global_style())
            .fill(t.bg_raised)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(20));
        let modal = egui::Modal::new(id).frame(frame);

        match &mut dialog {
            Dialog::Welcome => {
                let resp = modal.show(ctx, |ui| {
                    ui.set_width(520.0_f32.min(ctx.content_rect().width() - 64.0));
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 8.0, t.bg_deep);
                        paint_logo(ui.painter(), r.center(), 26.0, &t);
                        ui.add_space(4.0);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Strata").text_style(theme::title_style()));
                            ui.label(RichText::new("A StarCraft II map editor").color(t.text_dim));
                        });
                    });
                    ui.add_space(16.0);
                    ui.columns(2, |cols| {
                        if action_card(&mut cols[0], "New map", "Start from a blank map: pick a size and tileset.", &t)
                        {
                            self.queued.push(Command::NewMap);
                        }
                        if action_card(&mut cols[1], "Open map…", "Open an .SC2Map file or a Strata project.", &t) {
                            self.queued.push(Command::OpenMap);
                        }
                    });
                    if !self.settings.recent.is_empty() {
                        ui.add_space(16.0);
                        ui.label(RichText::new("Recent").family(theme::medium()).color(t.text_dim));
                        ui.add_space(2.0);
                        for p in self.settings.recent.clone().into_iter().take(5) {
                            let r = ui
                                .add(
                                    egui::Button::new(RichText::new(commands::display_name(&p)).color(t.text))
                                        .frame(false)
                                        .min_size(Vec2::new(ui.available_width(), 26.0)),
                                )
                                .on_hover_text(&p);
                            if r.clicked() {
                                self.queued.push(Command::OpenRecent(p));
                            }
                        }
                    }
                    ui.add_space(16.0);
                    ui.label(
                        RichText::new("Tip: drop an .SC2Map or .strata.json file anywhere in this window to open it.")
                            .color(t.text_dim),
                    );
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.settings.show_welcome, "Show this when Strata starts");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.button("Start with an empty map").clicked() {
                                ui.close();
                            }
                        });
                    });
                });
                if resp.should_close() {
                    keep = false;
                }
                if !self.queued.is_empty() {
                    keep = false;
                }
            }
            Dialog::NewMap(form) => {
                let resp = modal.show(ctx, |ui| {
                    ui.set_width(440.0_f32.min(ctx.content_rect().width() - 64.0));
                    ui.label(RichText::new("New map").text_style(theme::title_style()));
                    ui.add_space(12.0);
                    egui::Grid::new("new-map")
                        .num_columns(2)
                        .spacing([16.0, 10.0])
                        .show(ui, |ui| {
                            ui.label("Name");
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut form.name).desired_width(260.0),
                            );
                            if ui.memory(|m| m.focused().is_none()) {
                                r.request_focus();
                            }
                            ui.end_row();

                            ui.label("Size");
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for s in SIZE_PRESETS {
                                    if ui
                                        .selectable_label(form.preset == Some(s), s.to_string())
                                        .clicked()
                                    {
                                        form.preset = Some(s);
                                        form.width = s;
                                        form.height = s;
                                    }
                                }
                                if ui
                                    .selectable_label(form.preset.is_none(), "Custom")
                                    .clicked()
                                {
                                    form.preset = None;
                                }
                            });
                            ui.end_row();

                            if form.preset.is_none() {
                                ui.label("");
                                ui.horizontal(|ui| {
                                    ui.add(
                                        egui::DragValue::new(&mut form.width)
                                            .range(16..=256)
                                            .speed(8)
                                            .suffix(" wide"),
                                    );
                                    ui.label(RichText::new("×").color(t.text_dim));
                                    ui.add(
                                        egui::DragValue::new(&mut form.height)
                                            .range(16..=256)
                                            .speed(8)
                                            .suffix(" tall"),
                                    );
                                });
                                ui.end_row();
                                ui.label("");
                                let (w, h) = form.size();
                                ui.label(
                                    RichText::new(format!(
                                        "Sizes snap to multiples of 8: {w} × {h} cells."
                                    ))
                                    .small()
                                    .color(t.text_dim),
                                );
                                ui.end_row();
                            }

                            ui.label("Tileset");
                            let current = TILESETS
                                .iter()
                                .find(|(id, _)| *id == form.tileset)
                                .map_or(form.tileset.as_str(), |x| x.1);
                            egui::ComboBox::from_id_salt("tileset")
                                .width(260.0)
                                .selected_text(current)
                                .show_ui(ui, |ui| {
                                    for (id, name) in TILESETS {
                                        ui.selectable_value(&mut form.tileset, id.to_owned(), name);
                                    }
                                });
                            ui.end_row();

                            ui.label("Base level");
                            ui.horizontal(|ui| {
                                ui.add(egui::Slider::new(&mut form.base_level, 0..=15));
                                ui.label(RichText::new("cliff level").small().color(t.text_dim));
                            });
                            ui.end_row();
                        });
                    ui.add_space(16.0);
                    let valid = !form.name.trim().is_empty();
                    ui.horizontal(|ui| {
                        if !valid {
                            ui.label(RichText::new("Give the map a name.").color(t.warn));
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                            if primary_button(ui, "Create map", valid, &t).clicked()
                                || (enter && valid)
                            {
                                created = Some(form.build());
                            }
                            if ui.button("Cancel").clicked() {
                                ui.close();
                            }
                        });
                    });
                });
                if resp.should_close() || created.is_some() {
                    keep = false;
                }
            }
            Dialog::Unsaved(_) => {
                let name = self
                    .state
                    .file_name
                    .clone()
                    .unwrap_or_else(|| self.state.doc.info.name.clone());
                let mut choice: Option<u8> = None;
                let resp = modal.show(ctx, |ui| {
                    ui.set_width(400.0);
                    ui.label(
                        RichText::new(format!("Save changes to {name}?"))
                            .text_style(theme::title_style()),
                    );
                    ui.add_space(8.0);
                    ui.label("Your changes will be lost if you continue without saving.");
                    ui.add_space(16.0);
                    ui.horizontal(|ui| {
                        if ui.button("Discard changes").clicked() {
                            choice = Some(1);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if primary_button(ui, "Save map", true, &t).clicked() {
                                choice = Some(0);
                            }
                            if ui.button("Cancel").clicked() {
                                choice = Some(2);
                            }
                        });
                    });
                });
                if resp.should_close() {
                    choice = choice.or(Some(2));
                }
                if let Some(c) = choice {
                    keep = false;
                    let Dialog::Unsaved(th) = std::mem::replace(&mut dialog, Dialog::About) else {
                        unreachable!()
                    };
                    match c {
                        0 => {
                            self.after_save = Some(th);
                            self.queued.push(Command::Save);
                        }
                        1 => then = Some(th),
                        _ => {}
                    }
                }
            }
            Dialog::About => {
                let resp = modal.show(ctx, |ui| {
                    ui.set_width(420.0);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(Vec2::splat(40.0), egui::Sense::hover());
                        ui.painter().rect_filled(r, 8.0, t.bg_deep);
                        paint_logo(ui.painter(), r.center(), 26.0, &t);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Strata").text_style(theme::title_style()));
                            ui.label(
                                RichText::new(format!("Version {}", env!("CARGO_PKG_VERSION"))).color(t.text_dim),
                            );
                        });
                    });
                    ui.add_space(12.0);
                    ui.label("A fast StarCraft II map editor for the web, as an installable app, and on the desktop.");
                    ui.add_space(8.0);
                    ui.label(RichText::new("Licensed under MIT or Apache-2.0. Barlow and JetBrains Mono are used under the SIL Open Font License.").color(t.text_dim));
                    ui.add_space(8.0);
                    ui.label(RichText::new("Strata is not affiliated with or endorsed by Blizzard Entertainment. StarCraft is a trademark of Blizzard Entertainment, Inc.").small().color(t.text_dim));
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            ui.close();
                        }
                    });
                });
                if resp.should_close() {
                    keep = false;
                }
            }
        }

        if keep {
            self.dialog = Some(dialog);
        } else {
            self.dialog = next.take();
        }
        if let Some(doc) = created {
            self.new_map(doc);
        }
        if let Some(then) = then {
            self.do_then(ctx, then);
        }
    }

    fn show_notices(&mut self, ctx: &egui::Context) {
        if self.notices.is_empty() {
            return;
        }
        let t = theme::tokens(ctx);
        let mut clicked: Option<(usize, NoticeAction)> = None;
        egui::Area::new(egui::Id::new("notices"))
            .anchor(egui::Align2::RIGHT_TOP, [-16.0, 44.0])
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for (i, n) in self.notices.iter().enumerate() {
                    theme::card(ui, theme::toast_color(&t, n.kind), |ui| {
                        ui.add(egui::Label::new(RichText::new(&n.text).color(t.text)).wrap());
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            for (j, (label, action)) in n.actions.iter().enumerate() {
                                let r = if j == 0 && n.actions.len() > 1 {
                                    primary_button(ui, label, true, &t)
                                } else {
                                    ui.button(*label)
                                };
                                if r.clicked() {
                                    clicked = Some((i, action.clone()));
                                }
                            }
                        });
                    });
                }
            });
        if let Some((i, action)) = clicked {
            self.notices.remove(i);
            match action {
                NoticeAction::Run(cmd) => self.queued.push(cmd),
                NoticeAction::DiscardRestored => {
                    crate::io::clear_autosave();
                    self.state
                        .load(MapDocument::new("Untitled map", 128, 128), None);
                    self.io.reset_path();
                    self.state
                        .toast(ToastKind::Info, "Discarded the restored map");
                }
                NoticeAction::Reload => crate::io::reload_for_update(),
                NoticeAction::Dismiss => {}
            }
        }
    }

    fn drop_overlay(&self, ctx: &egui::Context) {
        let hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
        if !hovering {
            return;
        }
        let t = theme::tokens(ctx);
        let screen = ctx.content_rect();
        let p = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop-overlay"),
        ));
        p.rect_filled(screen, 0.0, Color32::from_black_alpha(140));
        let r = screen.shrink(24.0);
        p.rect_stroke(
            r,
            12.0,
            Stroke::new(2.0, t.accent),
            egui::StrokeKind::Inside,
        );
        p.text(
            r.center(),
            egui::Align2::CENTER_CENTER,
            "Drop to open the map",
            egui::FontId::new(20.0, theme::semibold()),
            t.text,
        );
        p.text(
            r.center() + Vec2::new(0.0, 26.0),
            egui::Align2::CENTER_CENTER,
            ".SC2Map archives and .strata.json projects",
            egui::FontId::proportional(14.0),
            t.text_dim,
        );
    }
}

impl eframe::App for StrataApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        crate::mcp_link::tick(&ctx, &mut self.state);
        self.poll_web(&ctx);
        self.handle_io();
        if let Some(then) = self.pending_then.take() {
            self.do_then(&ctx, then);
        }
        self.handle_drops(&ctx);

        let blocked = self.palette.is_open() || self.dialog.is_some() || self.shortcuts.open;
        let mut cmds = commands::handle_shortcuts(&ctx, blocked);
        if blocked {
            // Keyboard toggles still close what they opened.
            ctx.input_mut(|i| {
                if self.palette.is_open()
                    && i.consume_shortcut(&Command::CommandPalette.shortcut().unwrap_or(
                        egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K),
                    ))
                {
                    cmds.push(Command::CommandPalette);
                }
                if self.shortcuts.open && i.consume_key(egui::Modifiers::NONE, egui::Key::F1) {
                    cmds.push(Command::ShowShortcuts);
                }
            });
        }
        self.queued.extend(cmds);

        let title = self.state.title();
        if title != self.last_title {
            crate::io::set_title(&ctx, &title);
            self.last_title = title;
        }

        let t = theme::tokens(&ctx);
        let bar = |fill: Color32| {
            egui::Frame::new()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(8, 3))
        };
        egui::Panel::top("menu")
            .frame(bar(t.bg_panel))
            .show_separator_line(true)
            .show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status")
            .frame(bar(t.bg_panel).inner_margin(egui::Margin::symmetric(12, 3)))
            .exact_size(26.0)
            .show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("modes")
            .resizable(false)
            .exact_size(52.0)
            .frame(
                egui::Frame::new()
                    .fill(t.bg_panel)
                    .inner_margin(egui::Margin::symmetric(6, 4)),
            )
            .show(ui, |ui| self.mode_rail(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(t.bg_deep))
            .show(ui, |ui| {
                let style = theme::dock_style(ui.style());
                let mut viewer = Viewer {
                    state: &mut self.state,
                };
                DockArea::new(&mut self.settings.dock)
                    .style(style)
                    .show_leaf_collapse_buttons(false)
                    .show_leaf_close_all_buttons(false)
                    .show_inside(ui, &mut viewer);
            });

        let cx = commands::Context {
            recent_files: &self.settings.recent,
            can_install: self.can_install,
        };
        if let Some(cmd) = self.palette.show(&ctx, &cx, &self.settings.recent_commands) {
            self.remember_command(&cmd);
            self.queued.push(cmd);
        }
        self.shortcuts.show(&ctx);
        self.dialogs(&ctx);
        self.show_notices(&ctx);
        theme::show_toasts(&ctx, &mut self.state);
        self.drop_overlay(&ctx);

        for cmd in std::mem::take(&mut self.queued) {
            self.run(&ctx, cmd);
        }
        self.drain_actions(&ctx);

        let modified = self.state.history.is_modified();
        let rev = self.state.history.revision();
        let warn = self.io.tick_autosave(
            &ctx,
            &self.state.doc,
            modified,
            rev,
            self.state.file_name.as_deref(),
        );
        if let Some(text) = warn {
            self.notice(Notice {
                key: "autosave",
                kind: ToastKind::Error,
                text,
                actions: vec![
                    ("Export project", NoticeAction::Run(Command::ExportProject)),
                    ("Dismiss", NoticeAction::Dismiss),
                ],
            });
        }
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

fn dot(ui: &mut egui::Ui, t: &Tokens) {
    ui.label(RichText::new("•").small().color(t.stroke));
}

/// The one filled-accent button in a dialog: its default action.
fn primary_button(ui: &mut egui::Ui, label: &str, enabled: bool, t: &Tokens) -> egui::Response {
    let text = RichText::new(label)
        .family(theme::medium())
        .color(if ui.visuals().dark_mode {
            t.bg_deep
        } else {
            Color32::WHITE
        });
    ui.add_enabled(
        enabled,
        egui::Button::new(text)
            .fill(t.accent)
            .stroke(Stroke::NONE)
            .min_size(Vec2::new(96.0, 28.0)),
    )
}

/// A large clickable card used on the welcome screen.
fn action_card(ui: &mut egui::Ui, title: &str, body: &str, t: &Tokens) -> bool {
    let dark = ui.visuals().dark_mode;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 84.0), egui::Sense::click());
    let p = ui.painter();
    let fill = if resp.hovered() {
        theme::hover_fill(t, dark)
    } else {
        t.bg_panel
    };
    let stroke = if resp.has_focus() {
        Stroke::new(1.5, t.accent)
    } else {
        Stroke::new(1.0, t.stroke)
    };
    p.rect(rect, 6.0, fill, stroke, egui::StrokeKind::Inside);
    p.text(
        rect.left_top() + Vec2::new(14.0, 14.0),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::new(16.0, theme::semibold()),
        t.text,
    );
    let galley = p.layout(
        body.to_owned(),
        egui::FontId::proportional(13.0),
        t.text_dim,
        w - 28.0,
    );
    p.galley(rect.left_top() + Vec2::new(14.0, 40.0), galley, t.text_dim);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// The Strata mark: three strata stepping up, as in the app icon.
pub fn paint_logo(p: &egui::Painter, c: Pos2, size: f32, t: &Tokens) {
    let k = size / 300.0;
    let w = (34.0 * k).max(1.5);
    let strata: [([(f32, f32); 4], f32); 3] = [
        (
            [
                (116.0, 360.0),
                (268.0, 360.0),
                (316.0, 312.0),
                (396.0, 312.0),
            ],
            0.38,
        ),
        (
            [
                (116.0, 280.0),
                (212.0, 280.0),
                (260.0, 232.0),
                (396.0, 232.0),
            ],
            0.68,
        ),
        (
            [
                (116.0, 200.0),
                (156.0, 200.0),
                (204.0, 152.0),
                (396.0, 152.0),
            ],
            1.0,
        ),
    ];
    for (pts, op) in strata {
        let color = lerp_color(t.bg_deep, t.accent, op);
        let pts: Vec<Pos2> = pts
            .iter()
            .map(|(x, y)| c + Vec2::new((x - 256.0) * k, (y - 256.0) * k))
            .collect();
        p.add(egui::Shape::line(pts.clone(), Stroke::new(w, color)));
        for q in [pts[0], pts[3]] {
            p.circle_filled(q, w / 2.0, color);
        }
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
}

/// 20 x 20 line icons for the mode rail, drawn with the painter.
fn paint_mode_icon(p: &egui::Painter, mode: Mode, c: Pos2, color: Color32) {
    let s = Stroke::new(1.6, color);
    let at = |x: f32, y: f32| c + Vec2::new(x, y);
    let poly = |pts: &[(f32, f32)], closed: bool| {
        let v: Vec<Pos2> = pts.iter().map(|&(x, y)| at(x, y)).collect();
        if closed {
            egui::Shape::closed_line(v, s)
        } else {
            egui::Shape::line(v, s)
        }
    };
    match mode {
        Mode::Select => {
            p.add(poly(
                &[
                    (-5.0, -8.0),
                    (-5.0, 6.0),
                    (-1.5, 2.5),
                    (1.5, 8.5),
                    (4.0, 7.3),
                    (1.0, 1.5),
                    (5.5, 1.0),
                ],
                true,
            ));
        }
        Mode::Sculpt => {
            p.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [at(-9.0, 6.0), at(-4.0, 6.0), at(-3.0, -7.0), at(0.0, -7.0)],
                false,
                Color32::TRANSPARENT,
                s,
            ));
            p.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [at(0.0, -7.0), at(3.0, -7.0), at(4.0, 6.0), at(9.0, 6.0)],
                false,
                Color32::TRANSPARENT,
                s,
            ));
            p.line_segment(
                [at(-9.0, 7.5), at(9.0, 7.5)],
                Stroke::new(1.2, color.gamma_multiply(0.6)),
            );
        }
        Mode::Paint => {
            p.line_segment([at(7.5, -8.0), at(0.5, -1.0)], Stroke::new(2.2, color));
            p.add(poly(&[(-1.0, -2.5), (2.0, 0.5)], false));
            p.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [at(-0.8, -1.2), at(-6.0, -1.0), at(-5.0, 4.0), at(-8.5, 8.0)],
                false,
                Color32::TRANSPARENT,
                s,
            ));
            p.add(egui::epaint::CubicBezierShape::from_points_stroke(
                [at(-8.5, 8.0), at(-3.0, 8.0), at(1.0, 5.0), at(0.8, 0.6)],
                false,
                Color32::TRANSPARENT,
                s,
            ));
        }
        Mode::Cliff => {
            p.add(poly(
                &[
                    (-9.0, 7.0),
                    (-4.0, 7.0),
                    (-2.5, 2.0),
                    (1.0, 2.0),
                    (2.5, -3.0),
                    (5.5, -3.0),
                    (7.0, -8.0),
                    (9.0, -8.0),
                ],
                false,
            ));
            p.line_segment(
                [at(-9.0, 7.0), at(9.0, 7.0)],
                Stroke::new(1.2, color.gamma_multiply(0.6)),
            );
        }
        Mode::Pathing => {
            let r = Rect::from_center_size(c, Vec2::splat(16.0));
            p.rect_stroke(r, 2.0, s, egui::StrokeKind::Middle);
            for i in 1..3 {
                let d = -8.0 + i as f32 * 16.0 / 3.0;
                p.line_segment(
                    [at(d, -8.0), at(d, 8.0)],
                    Stroke::new(1.0, color.gamma_multiply(0.7)),
                );
                p.line_segment(
                    [at(-8.0, d), at(8.0, d)],
                    Stroke::new(1.0, color.gamma_multiply(0.7)),
                );
            }
            let cell = Rect::from_min_size(at(8.0 / 3.0 - 0.0, -8.0), Vec2::splat(16.0 / 3.0));
            p.rect_filled(cell.shrink(0.5), 0.0, color.gamma_multiply(0.75));
            let cell = Rect::from_min_size(at(-8.0, 8.0 / 3.0 - 0.0), Vec2::splat(16.0 / 3.0));
            p.rect_filled(cell.shrink(0.5), 0.0, color.gamma_multiply(0.75));
        }
        Mode::Units => {
            p.circle_stroke(at(0.0, 1.0), 7.0, s);
            p.circle_filled(at(0.0, 1.0), 2.4, color);
            p.add(poly(&[(-2.5, -8.5), (0.0, -11.0), (2.5, -8.5)], false));
        }
        Mode::Doodads => {
            p.add(poly(&[(-1.0, -9.0), (-7.0, 1.0), (5.0, 1.0)], true));
            p.line_segment([at(-1.0, 1.0), at(-1.0, 6.0)], s);
            p.add(egui::Shape::convex_polygon(
                vec![at(3.0, 8.0), at(4.5, 4.5), at(8.0, 3.5), at(9.5, 8.0)],
                color.gamma_multiply(0.75),
                Stroke::NONE,
            ));
            p.line_segment(
                [at(-9.0, 8.0), at(9.5, 8.0)],
                Stroke::new(1.2, color.gamma_multiply(0.6)),
            );
        }
        Mode::Points => {
            p.line_segment([at(-5.0, -9.0), at(-5.0, 9.0)], s);
            p.add(poly(&[(-5.0, -9.0), (7.0, -5.5), (-5.0, -1.5)], true));
            p.add(egui::Shape::convex_polygon(
                vec![at(-5.0, -9.0), at(7.0, -5.5), at(-5.0, -1.5)],
                color.gamma_multiply(0.35),
                Stroke::NONE,
            ));
        }
        Mode::Regions => {
            let r = Rect::from_center_size(c, Vec2::new(16.0, 14.0));
            dashed_rect(p, r, color);
            for q in [
                r.left_top(),
                r.right_top(),
                r.left_bottom(),
                r.right_bottom(),
            ] {
                p.rect_filled(Rect::from_center_size(q, Vec2::splat(4.0)), 1.0, color);
            }
        }
        Mode::Cameras => {
            let body = Rect::from_min_max(at(-9.0, -5.0), at(3.0, 6.0));
            p.rect_stroke(body, 2.0, s, egui::StrokeKind::Middle);
            p.add(poly(
                &[(3.0, -1.0), (9.0, -5.0), (9.0, 6.0), (3.0, 2.0)],
                true,
            ));
            p.circle_filled(at(-3.0, 0.5), 1.8, color);
        }
    }
}

fn dashed_rect(p: &egui::Painter, r: Rect, color: Color32) {
    let s = Stroke::new(1.4, color);
    let edges = [
        (r.left_top(), r.right_top()),
        (r.right_top(), r.right_bottom()),
        (r.right_bottom(), r.left_bottom()),
        (r.left_bottom(), r.left_top()),
    ];
    for (a, b) in edges {
        p.extend(egui::Shape::dashed_line(&[a, b], s, 2.5, 2.0));
    }
}
