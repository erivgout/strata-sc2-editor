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
use crate::state::{AppAction, EditorState, Mode, ToastKind, Workspace};
use crate::theme::{self, ThemePref, Tokens};

const SETTINGS_KEY: &str = "strata.settings.v1";
const MAX_RECENT_FILES: usize = 8;
const MAX_RECENT_COMMANDS: usize = 8;

/// Settings persisted across sessions (eframe storage: localStorage on web).
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePref,
    /// The workspace shown when Strata starts.
    pub workspace: Workspace,
    /// One dock layout per workspace.
    pub docks: WorkspaceDocks,
    /// The single layout saved before workspaces existed; it becomes the Map
    /// workspace's layout when loaded (see [`Settings::migrate`]).
    #[serde(rename = "dock", skip_serializing, deserialize_with = "present")]
    legacy_dock: Option<DockState<Tab>>,
    /// Recently opened files, newest first (native paths).
    pub recent: Vec<String>,
    /// Recently run palette commands, newest first.
    pub recent_commands: Vec<Command>,
    pub show_welcome: bool,
    /// Release tag the user chose to skip (desktop updater).
    pub skipped_update: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::Dark,
            workspace: Workspace::Map,
            docks: WorkspaceDocks::default(),
            legacy_dock: None,
            recent: Vec::new(),
            recent_commands: Vec::new(),
            show_welcome: true,
            skipped_update: None,
        }
    }
}

impl Settings {
    /// Bring settings saved by older versions up to date.
    fn migrate(mut self) -> Self {
        if let Some(dock) = self.legacy_dock.take() {
            self.docks.map = dock;
        }
        self
    }

    pub fn dock(&self) -> &DockState<Tab> {
        self.docks.get(self.workspace)
    }

    /// Show a workspace. Its main panel is reopened if it was closed, since
    /// a Triggers workspace without the trigger editor is no use.
    pub fn switch_workspace(&mut self, ws: Workspace) {
        self.workspace = ws;
        let main = ws.main_tab();
        if self.dock().find_tab(&main).is_none() {
            reveal_tab(self.dock_mut(), main);
        }
    }

    /// Bring a panel into view: in the current workspace if it is there,
    /// else in the workspace it belongs to, else open it here.
    pub fn focus_tab(&mut self, tab: Tab) {
        if self.dock().find_tab(&tab).is_none() {
            if let Some(home) = Workspace::home_of(tab) {
                self.workspace = home;
            }
        }
        reveal_tab(self.dock_mut(), tab);
    }

    pub fn dock_mut(&mut self) -> &mut DockState<Tab> {
        self.docks.get_mut(self.workspace)
    }
}

/// Read a field that was saved as a plain value into an `Option` (RON
/// writes options as `Some(..)`, so the default `Option` impl would reject it).
fn present<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    T::deserialize(d).map(Some)
}

/// The dock layouts of the four workspaces.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WorkspaceDocks {
    pub map: DockState<Tab>,
    pub triggers: DockState<Tab>,
    pub data: DockState<Tab>,
    pub text: DockState<Tab>,
}

impl Default for WorkspaceDocks {
    fn default() -> Self {
        Self {
            map: default_dock(Workspace::Map),
            triggers: default_dock(Workspace::Triggers),
            data: default_dock(Workspace::Data),
            text: default_dock(Workspace::Text),
        }
    }
}

impl WorkspaceDocks {
    pub fn get(&self, ws: Workspace) -> &DockState<Tab> {
        match ws {
            Workspace::Map => &self.map,
            Workspace::Triggers => &self.triggers,
            Workspace::Data => &self.data,
            Workspace::Text => &self.text,
        }
    }

    pub fn get_mut(&mut self, ws: Workspace) -> &mut DockState<Tab> {
        match ws {
            Workspace::Map => &mut self.map,
            Workspace::Triggers => &mut self.triggers,
            Workspace::Data => &mut self.data,
            Workspace::Text => &mut self.text,
        }
    }
}

/// The layout a workspace starts with (and returns to on "Reset layout").
pub fn default_dock(ws: Workspace) -> DockState<Tab> {
    let mut dock = DockState::new(vec![ws.main_tab()]);
    let surface = dock.main_surface_mut();
    let root = NodeIndex::root();
    match ws {
        // The map is the hero; the editors stay one click away below it.
        Workspace::Map => {
            let [main, right] =
                surface.split_right(root, 0.77, vec![Tab::Inspector, Tab::MapProps]);
            surface.split_below(right, 0.64, vec![Tab::Minimap]);
            surface.split_below(
                main,
                0.68,
                vec![
                    Tab::Problems,
                    Tab::History,
                    Tab::Mcp,
                    Tab::Script,
                    Tab::Triggers,
                    Tab::Data,
                    Tab::Strings,
                ],
            );
        }
        // Triggers fill the window; the generated script and problems sit
        // beside them so codegen errors are visible while editing.
        Workspace::Triggers => {
            let [_, right] = surface.split_right(root, 0.70, vec![Tab::Script]);
            surface.split_below(right, 0.62, vec![Tab::Problems, Tab::History]);
        }
        Workspace::Data => {
            surface.split_below(root, 0.70, vec![Tab::Strings, Tab::Problems, Tab::History]);
        }
        Workspace::Text => {
            surface.split_below(root, 0.76, vec![Tab::Problems, Tab::History]);
        }
    }
    // The main panel starts focused so its tab label takes the accent.
    if let Some(path) = dock.find_tab(&ws.main_tab()) {
        dock.set_focused_node_and_surface(path.node_path());
    }
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
    /// Don't offer this release tag again (desktop updater).
    #[cfg(not(target_arch = "wasm32"))]
    SkipUpdate(String),
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
    /// Base-game data from the user's StarCraft II install.
    game_data: crate::game_data::GameData,
    #[cfg(not(target_arch = "wasm32"))]
    updater: crate::updater::Updater,
    /// The newer release offered to the user, if any.
    #[cfg(not(target_arch = "wasm32"))]
    update_offer: Option<crate::updater::Release>,
}

impl StrataApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value::<Settings>(s, SETTINGS_KEY))
            .map(Settings::migrate)
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
            game_data: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            updater: Default::default(),
            #[cfg(not(target_arch = "wasm32"))]
            update_offer: None,
        };
        app.game_data.start(&cc.egui_ctx);
        #[cfg(not(target_arch = "wasm32"))]
        app.updater.check(&cc.egui_ctx, false);
        // STRATA_OPEN=<map>: open a map on start (native; used to check
        // rendering against real maps).
        #[cfg(not(target_arch = "wasm32"))]
        let open_on_start = std::env::var("STRATA_OPEN").ok().map(|path| app.io.open_path(&cc.egui_ctx, path));
        #[cfg(target_arch = "wasm32")]
        let open_on_start: Option<()> = None;
        let restored = if open_on_start.is_some() { None } else { crate::io::restore_autosave() };
        if let Some(rec) = restored {
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
        } else {
            app.state.history.mark_saved();
            if app.settings.show_welcome {
                app.dialog = Some(Dialog::Welcome);
            }
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
        if matches!(cmd, Command::OpenPalette) {
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
            Command::NewMap
                | Command::OpenMap
                | Command::OpenFolder
                | Command::OpenRecent(_)
                | Command::RestartToUpdate
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
            Command::OpenPalette => self.palette.toggle(),
            Command::ShowShortcuts => self.shortcuts.open = !self.shortcuts.open,
            Command::SetMode(m) => self.state.mode = m,
            Command::Workspace(w) => self.switch_workspace(w),
            Command::FocusTab(t) => self.focus_tab(t),
            Command::Theme(p) => self.settings.theme = p,
            Command::ResetLayout => {
                let ws = self.settings.workspace;
                *self.settings.dock_mut() = default_dock(ws);
                self.state
                    .toast(ToastKind::Info, format!("Reset the {} layout", ws.label()));
            }
            Command::ConnectAi => self.focus_tab(Tab::Mcp),
            Command::LoadGameData => {
                if let Err(e) = self.game_data.pick_folder(ctx) {
                    self.notice(Notice {
                        key: "game-data",
                        kind: ToastKind::Error,
                        text: e,
                        actions: vec![("Dismiss", NoticeAction::Dismiss)],
                    });
                }
            }
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
            #[cfg(not(target_arch = "wasm32"))]
            Command::CheckForUpdates => self.updater.check(ctx, true),
            #[cfg(not(target_arch = "wasm32"))]
            Command::InstallUpdate => {
                if let Some(r) = self.update_offer.clone() {
                    self.updater.install(ctx, r);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            Command::RestartToUpdate => match crate::updater::relaunch() {
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.state.toast(ToastKind::Error, e),
            },
            #[cfg(target_arch = "wasm32")]
            Command::CheckForUpdates | Command::InstallUpdate | Command::RestartToUpdate => {}
        }
    }

    fn switch_workspace(&mut self, ws: Workspace) {
        self.settings.switch_workspace(ws);
    }

    fn focus_tab(&mut self, tab: Tab) {
        self.settings.focus_tab(tab);
    }

    fn problem_counts(&mut self) -> (usize, usize) {
        let p = self.state.problems();
        let errors = p.iter().filter(|p| p.severity == Severity::Error).count();
        let warnings = p.iter().filter(|p| p.severity == Severity::Warning).count();
        (errors, warnings)
    }

    fn drain_actions(&mut self, ctx: &egui::Context) {
        for a in std::mem::take(&mut self.state.actions) {
            let cmd = match a {
                AppAction::Undo => Command::Undo,
                AppAction::Redo => Command::Redo,
                AppAction::FocusTab(tab) => Command::FocusTab(tab),
                AppAction::FrameWorld(p) => {
                    crate::panels::viewport::frame_world(&mut self.state, p);
                    continue;
                }
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

    /// Turn updater events into notices.
    #[cfg(not(target_arch = "wasm32"))]
    fn handle_updates(&mut self, ctx: &egui::Context) {
        use crate::updater::{Event, Status};
        while let Some(ev) = self.updater.poll() {
            match ev {
                Event::Checked { result: Ok(Some(r)), manual } => {
                    if !manual && self.settings.skipped_update.as_deref() == Some(r.tag.as_str()) {
                        continue;
                    }
                    self.notice(Notice {
                        key: "update",
                        kind: ToastKind::Info,
                        text: format!(
                            "Strata {} is available (you have {}). Update now? Strata downloads it, checks it and installs it.",
                            r.version_text(),
                            env!("CARGO_PKG_VERSION")
                        ),
                        actions: vec![
                            ("Update now", NoticeAction::Run(Command::InstallUpdate)),
                            ("Later", NoticeAction::Dismiss),
                            ("Skip this version", NoticeAction::SkipUpdate(r.tag.clone())),
                        ],
                    });
                    self.update_offer = Some(r);
                }
                Event::Checked { result: Ok(None), manual: true } => self
                    .state
                    .toast(ToastKind::Success, format!("Strata {} is up to date", env!("CARGO_PKG_VERSION"))),
                Event::Checked { result: Err(e), manual: true } => {
                    self.state.toast(ToastKind::Error, format!("Couldn't check for updates: {e}"))
                }
                Event::Checked { result: Err(e), manual: false } => log::info!("update check: {e}"),
                Event::Checked { .. } => {}
                Event::Progress(_) => {}
                Event::Installed(Ok(r)) => self.notice(Notice {
                    key: "update",
                    kind: ToastKind::Success,
                    text: format!("Strata {} is installed. Restart to start using it.", r.version_text()),
                    actions: vec![
                        ("Restart now", NoticeAction::Run(Command::RestartToUpdate)),
                        ("Later", NoticeAction::Dismiss),
                    ],
                }),
                Event::Installed(Err(e)) => self.notice(Notice {
                    key: "update",
                    kind: ToastKind::Error,
                    text: format!("The update wasn't installed: {e}. Strata is unchanged."),
                    actions: vec![
                        ("Try again", NoticeAction::Run(Command::InstallUpdate)),
                        ("Dismiss", NoticeAction::Dismiss),
                    ],
                }),
            }
        }
        if let Status::Downloading { version, progress } = &self.updater.status {
            let text = format!("Downloading Strata {version}… {:.0}%", progress * 100.0);
            if self.notices.iter().all(|n| n.key != "update" || n.text != text) {
                self.notice(Notice { key: "update", kind: ToastKind::Info, text, actions: Vec::new() });
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn handle_game_data(&mut self) {
        use crate::game_data::Event;
        match self.game_data.poll() {
            Some(Event::Loaded { catalog, from_cache }) => {
                if !from_cache {
                    self.state.toast(
                        ToastKind::Success,
                        format!(
                            "Loaded StarCraft II {} data: {} units, {} catalog entries",
                            catalog.version,
                            catalog.units().len(),
                            catalog.len()
                        ),
                    );
                }
                self.notices.retain(|n| n.key != "game-data");
                self.state.base = Some(catalog);
            }
            Some(Event::Failed(text)) => self.notice(Notice {
                key: "game-data",
                kind: ToastKind::Error,
                text,
                actions: vec![
                    ("Choose folder", NoticeAction::Run(Command::LoadGameData)),
                    ("Dismiss", NoticeAction::Dismiss),
                ],
            }),
            None => {}
        }
    }

    /// Status-bar item for the base-game data.
    fn game_data_status(&mut self, ui: &mut egui::Ui, t: &theme::Tokens) {
        use crate::game_data::Status;
        let (text, tip, color, clickable) = match (&self.game_data.status, &self.state.base) {
            (Status::Loading(p), _) => {
                ui.add(egui::Spinner::new().size(12.0).color(t.accent));
                (p.clone(), "Reading the base game's units and abilities".to_owned(), t.text_dim, false)
            }
            (_, Some(b)) => (
                format!("SC2 {} data", b.version),
                format!(
                    "{} placeable units and {} catalog entries from {}. Click to reload from another folder.",
                    b.units().len(),
                    b.len(),
                    b.mods.join(", ")
                ),
                t.text_dim,
                true,
            ),
            (Status::Failed(e), None) => ("No game data".to_owned(), e.clone(), t.warn, true),
            (status, None) => {
                let why = match status {
                    Status::Missing(w) => format!("{w}. "),
                    _ => String::new(),
                };
                (
                    "No game data".to_owned(),
                    format!("{why}Unit lists show common units only. Click to choose your StarCraft II folder."),
                    t.text_dim,
                    true,
                )
            }
        };
        let resp = ui
            .add(egui::Label::new(RichText::new(text).small().color(color)).sense(if clickable {
                egui::Sense::click()
            } else {
                egui::Sense::hover()
            }))
            .on_hover_text(tip);
        if clickable && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if clickable && resp.clicked() {
            self.queued.push(Command::LoadGameData);
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
        let menus = egui::MenuBar::new().ui(ui, |ui| {
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
                self.menu_cmd(ui, Command::OpenPalette);
            });
            ui.menu_button("View", |ui| {
                ui.label(RichText::new("Workspace").small().color(t.text_dim));
                for w in Workspace::ALL {
                    let label = if w == self.settings.workspace {
                        RichText::new(w.label()).color(t.accent_text)
                    } else {
                        RichText::new(w.label())
                    };
                    let cmd = Command::Workspace(w);
                    let mut b = egui::Button::new(label);
                    if let Some(s) = cmd.shortcut() {
                        b = b.shortcut_text(ui.ctx().format_shortcut(&s));
                    }
                    if ui.add(b).clicked() {
                        self.queued.push(cmd);
                    }
                }
                let ws = self.settings.workspace;
                self.menu_cmd_labeled(
                    ui,
                    format!("Reset {} layout", ws.label()),
                    Command::ResetLayout,
                    true,
                );
                ui.separator();
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
                self.menu_cmd(ui, Command::LoadGameData);
                self.menu_cmd(ui, Command::ConnectAi);
            });
            ui.menu_button("Help", |ui| {
                self.menu_cmd(ui, Command::ShowShortcuts);
                self.menu_cmd(ui, Command::ShowWelcome);
                if self.can_install {
                    self.menu_cmd(ui, Command::InstallApp);
                }
                #[cfg(not(target_arch = "wasm32"))]
                self.menu_cmd(ui, Command::CheckForUpdates);
                ui.separator();
                self.menu_cmd(ui, Command::About);
            })
            .response
            .rect
        });

        // Workspace switcher and command palette affordance. The search box
        // is centered on the bar when there is room, the switcher sits just
        // left of it; on narrow windows both slide right of the menus and
        // the search box shrinks, then hides.
        let menus = menus.inner;
        let y = menus.center().y;
        let seg_w = workspace_switcher_width(ui);
        let gap = 8.0;
        let left = menus.right() + 16.0;
        let right = bar.right() - 4.0;
        if seg_w > right - left {
            return;
        }
        let search_w = 300.0_f32.min(right - left - seg_w - gap);
        let show_search = search_w >= 160.0;
        let group_w = seg_w + if show_search { gap + search_w } else { 0.0 };
        let preferred = if show_search {
            bar.center().x - search_w / 2.0 - gap - seg_w
        } else {
            bar.center().x - seg_w / 2.0
        };
        let x0 = preferred.max(left).min(right - group_w);
        let seg_rect = Rect::from_min_size(Pos2::new(x0, y - 12.0), Vec2::new(seg_w, 24.0));
        self.workspace_switcher(ui, seg_rect, &t);
        if show_search {
            let rect = Rect::from_min_size(
                Pos2::new(seg_rect.right() + gap, y - 12.0),
                Vec2::new(search_w, 24.0),
            );
            self.palette_hint(ui, rect, &t);
        }
    }

    /// A segmented control: one segment per workspace, the active one in
    /// the accent.
    fn workspace_switcher(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens) {
        let dark = ui.visuals().dark_mode;
        ui.painter().rect(
            rect,
            6.0,
            t.bg_deep,
            Stroke::new(1.0, t.stroke),
            egui::StrokeKind::Inside,
        );
        let n = Workspace::ALL.len() as f32;
        let w = (rect.width() - 4.0) / n;
        for (i, ws) in Workspace::ALL.into_iter().enumerate() {
            let r = Rect::from_min_size(
                Pos2::new(rect.left() + 2.0 + i as f32 * w, rect.top() + 2.0),
                Vec2::new(w, rect.height() - 4.0),
            );
            let resp = ui.interact(r, ui.id().with(("workspace", ws)), egui::Sense::click());
            let active = self.settings.workspace == ws;
            resp.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    active,
                    format!("{} workspace", ws.label()),
                )
            });
            let p = ui.painter();
            let (fill, color) = if active {
                (theme::selection_fill(t, dark), t.accent_text)
            } else if resp.hovered() {
                (theme::hover_fill(t, dark), t.text)
            } else {
                (Color32::TRANSPARENT, t.text_dim)
            };
            p.rect_filled(r, 4.0, fill);
            if resp.has_focus() {
                p.rect_stroke(r, 4.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
            }
            p.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                ws.label(),
                egui::FontId::new(14.0, theme::medium()),
                color,
            );
            let cmd = Command::Workspace(ws);
            let keys = cmd.shortcut().map(|s| ui.ctx().format_shortcut(&s));
            let resp = resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_ui(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(cmd.label()).family(theme::medium()));
                        if let Some(k) = &keys {
                            commands::keycap(ui, k, t);
                        }
                    });
                });
            if resp.clicked() {
                self.queued.push(cmd);
            }
        }
    }

    fn palette_hint(&mut self, ui: &mut egui::Ui, rect: Rect, t: &Tokens) {
        let resp = ui.interact(rect, ui.id().with("palette-hint"), egui::Sense::click());
        let p = ui.painter();
        let dark = ui.visuals().dark_mode;
        let fill = if resp.hovered() {
            theme::hover_fill(t, dark)
        } else {
            t.bg_deep
        };
        p.rect(
            rect,
            6.0,
            fill,
            Stroke::new(1.0, t.stroke),
            egui::StrokeKind::Inside,
        );
        if resp.has_focus() {
            p.rect_stroke(
                rect,
                6.0,
                Stroke::new(1.5, t.accent),
                egui::StrokeKind::Inside,
            );
        }
        commands::paint_search_icon(
            p,
            Pos2::new(rect.left() + 16.0, rect.center().y),
            t.text_dim,
        );
        let keys = ui
            .ctx()
            .format_shortcut(&Command::OpenPalette.shortcut().unwrap_or(
                egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K),
            ));
        let keys = p.layout_no_wrap(keys, egui::FontId::proportional(12.0), t.text_dim);
        let keys_left = rect.right() - 10.0 - keys.size().x;
        p.galley(
            Pos2::new(keys_left, rect.center().y - keys.size().y / 2.0),
            keys,
            t.text_dim,
        );
        let hint = p.layout(
            "Search commands…".to_owned(),
            egui::FontId::proportional(14.0),
            t.text_dim,
            f32::INFINITY,
        );
        let text_left = rect.left() + 30.0;
        if text_left + hint.size().x < keys_left - 8.0 {
            p.galley(
                Pos2::new(text_left, rect.center().y - hint.size().y / 2.0),
                hint,
                t.text_dim,
            );
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        resp.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Search commands")
        });
        if resp.clicked() {
            self.queued.push(Command::OpenPalette);
        }
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

    /// One line: document facts on the left; cursor, mode and frame rate on
    /// the right. Nothing wraps; long text is cut with an ellipsis.
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let t = theme::tokens(ui.ctx());
        let (errors, warnings) = self.problem_counts();
        let map_visible = self.settings.dock().find_tab(&Tab::Viewport).is_some();
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let modified = self.state.history.is_modified();
            let name = self
                .state
                .file_name
                .clone()
                .unwrap_or_else(|| self.state.doc.info.name.clone());
            let name_w = (ui.available_width() * 0.3).max(80.0);
            ui.allocate_ui(Vec2::new(name_w, ui.available_height()), |ui| {
                ui.add(egui::Label::new(
                    RichText::new(name).family(theme::medium()).color(t.text),
                ))
                .on_hover_text(self.state.title());
            });
            if modified {
                dot(ui, &t);
                ui.label(RichText::new("Unsaved changes").small().color(t.text))
                    .on_hover_text("Changes are autosaved every 30 seconds until you save.");
            } else if self.state.file_name.is_some() {
                dot(ui, &t);
                ui.label(RichText::new("Saved").small().color(t.text_dim));
            }
            dot(ui, &t);
            let i = &self.state.doc.info;
            ui.label(
                RichText::new(format!("{} × {}", i.width, i.height))
                    .small()
                    .color(t.text_dim),
            )
            .on_hover_text("Map size in cells");
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
            dot(ui, &t);
            self.game_data_status(ui, &t);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                // A fixed slot so the labels to its left don't jitter as the
                // number changes or disappears while idle.
                let dt = ui.input(|i| i.stable_dt);
                let fps = (dt > 0.0 && dt < 0.05).then(|| format!("{:.0} fps", 1.0 / dt));
                ui.allocate_ui_with_layout(
                    Vec2::new(46.0, ui.available_height()),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        ui.set_min_width(46.0);
                        if let Some(fps) = fps {
                            ui.label(RichText::new(fps).small().color(t.text_dim));
                        }
                    },
                );
                if !map_visible {
                    return;
                }
                dot(ui, &t);
                ui.label(
                    RichText::new(format!("{} mode", self.state.mode.label()))
                        .small()
                        .color(t.text),
                );
                let s = crate::panels::viewport::status_text(&self.state);
                if !s.is_empty() && ui.available_width() > 60.0 {
                    dot(ui, &t);
                    ui.add(egui::Label::new(RichText::new(s).small().color(t.text_dim)).truncate());
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
        let modal = egui::Modal::new(id).frame(theme::dialog_frame(ctx));

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
                #[cfg(not(target_arch = "wasm32"))]
                NoticeAction::SkipUpdate(tag) => self.settings.skipped_update = Some(tag),
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
        self.handle_game_data();
        #[cfg(not(target_arch = "wasm32"))]
        self.handle_updates(&ctx);
        if let Some(src) = self.game_data.take_assets() {
            self.state.assets.attach(src, &ctx);
        }
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
                    && i.consume_shortcut(&Command::OpenPalette.shortcut().unwrap_or(
                        egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K),
                    ))
                {
                    cmds.push(Command::OpenPalette);
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
        // Modes act on the map, so the rail only shows where the map does.
        if self.settings.dock().find_tab(&Tab::Viewport).is_some() {
            egui::Panel::left("modes")
                .resizable(false)
                .exact_size(52.0)
                .frame(
                    egui::Frame::new()
                        .fill(t.bg_panel)
                        .inner_margin(egui::Margin::symmetric(6, 4)),
                )
                .show(ui, |ui| self.mode_rail(ui));
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(t.bg_deep))
            .show(ui, |ui| {
                let ws = self.settings.workspace;
                if !has_tabs(self.settings.dock()) {
                    if empty_workspace(ui, ws, &t) {
                        self.queued.push(Command::ResetLayout);
                    }
                    return;
                }
                // Only the active workspace's dock is drawn, so a panel that
                // appears in several workspaces (the map, Problems) is never
                // shown twice in one frame.
                let style = theme::dock_style(ui.style());
                let mut viewer = Viewer {
                    state: &mut self.state,
                };
                DockArea::new(self.settings.docks.get_mut(ws))
                    .id(egui::Id::new(("strata-dock", ws)))
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

/// Focus `tab` in `dock`, reopening it next to its usual neighbours if it
/// was closed.
fn reveal_tab(dock: &mut DockState<Tab>, tab: Tab) {
    if let Some(found) = dock.find_tab(&tab) {
        let _ = dock.set_active_tab(found);
        dock.set_focused_node_and_surface(found.node_path());
        return;
    }
    let neighbours: &[Tab] = match tab {
        Tab::Inspector | Tab::MapProps | Tab::Minimap => {
            &[Tab::Inspector, Tab::MapProps, Tab::Minimap]
        }
        Tab::Viewport | Tab::Triggers | Tab::Data | Tab::Strings => {
            &[Tab::Viewport, Tab::Triggers, Tab::Data, Tab::Strings]
        }
        Tab::Script | Tab::Problems | Tab::History | Tab::Mcp => {
            &[Tab::Problems, Tab::History, Tab::Mcp, Tab::Script]
        }
    };
    if let Some(found) = neighbours.iter().find_map(|n| dock.find_tab(n)) {
        dock.set_focused_node_and_surface(found.node_path());
    }
    dock.push_to_focused_leaf(tab);
}

/// Width of the workspace switcher: equal segments sized to the longest label.
fn workspace_switcher_width(ui: &egui::Ui) -> f32 {
    let font = egui::FontId::new(14.0, theme::medium());
    let widest = Workspace::ALL
        .iter()
        .map(|w| {
            ui.painter()
                .layout_no_wrap(w.label().to_owned(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    (widest + 28.0).round() * Workspace::ALL.len() as f32 + 4.0
}

/// Shown when every panel of a workspace was closed. Returns true when the
/// user asks for the default layout back.
fn empty_workspace(ui: &mut egui::Ui, ws: Workspace, t: &Tokens) -> bool {
    let mut reset = false;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.3).max(24.0));
        ui.label(
            RichText::new(format!("The {} workspace has no panels open.", ws.label()))
                .size(16.0)
                .color(t.text),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new("Bring back its default panels, or open one from View › Panels.")
                .color(t.text_dim),
        );
        ui.add_space(12.0);
        reset = primary_button(ui, &format!("Reset {} layout", ws.label()), true, t).clicked();
    });
    reset
}

/// Whether a dock shows any panel at all.
fn has_tabs(dock: &DockState<Tab>) -> bool {
    dock.iter_all_tabs().next().is_some()
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
            .min_size(Vec2::new(88.0, 24.0)),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tabs(dock: &DockState<Tab>) -> Vec<Tab> {
        dock.iter_all_tabs().map(|(_, t)| *t).collect()
    }

    #[test]
    fn settings_from_before_workspaces_load_cleanly() {
        let mut old_dock = DockState::new(vec![Tab::Viewport]);
        old_dock
            .main_surface_mut()
            .split_right(NodeIndex::root(), 0.5, vec![Tab::History]);
        // The settings shape saved before workspaces existed.
        #[derive(serde::Serialize)]
        struct OldSettings {
            theme: ThemePref,
            dock: DockState<Tab>,
            recent: Vec<String>,
            recent_commands: Vec<Command>,
            show_welcome: bool,
        }
        let mut storage = MemoryStorage::default();
        let old = OldSettings {
            theme: ThemePref::Light,
            dock: old_dock,
            recent: vec!["C:/maps/A.SC2Map".into()],
            recent_commands: vec![Command::ResetLayout, Command::Save],
            show_welcome: false,
        };
        eframe::set_value(&mut storage, SETTINGS_KEY, &old);

        let s = eframe::get_value::<Settings>(&storage, SETTINGS_KEY)
            .expect("old settings load")
            .migrate();
        assert_eq!(s.theme, ThemePref::Light);
        assert!(!s.show_welcome);
        assert_eq!(s.workspace, Workspace::Map);
        assert_eq!(tabs(&s.docks.map), vec![Tab::Viewport, Tab::History]);
        assert!(s.docks.triggers.find_tab(&Tab::Triggers).is_some());
        assert_eq!(s.recent_commands, vec![Command::ResetLayout, Command::Save]);

        // Saved again, the old field is gone and the layout survives.
        let mut s = s;
        s.workspace = Workspace::Data;
        eframe::set_value(&mut storage, SETTINGS_KEY, &s);
        assert!(!storage.0.get(SETTINGS_KEY).unwrap().contains("dock:"));
        let again = eframe::get_value::<Settings>(&storage, SETTINGS_KEY)
            .expect("new settings load")
            .migrate();
        assert_eq!(again.workspace, Workspace::Data);
        assert_eq!(tabs(&again.docks.map), vec![Tab::Viewport, Tab::History]);
    }

    #[derive(Default)]
    struct MemoryStorage(std::collections::HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn every_workspace_starts_with_its_main_panel() {
        for ws in Workspace::ALL {
            let dock = default_dock(ws);
            assert!(dock.find_tab(&ws.main_tab()).is_some(), "{ws:?}");
            let all = tabs(&dock);
            let mut unique = all.clone();
            unique.sort_by_key(|t| *t as u8);
            unique.dedup();
            assert_eq!(all.len(), unique.len(), "{ws:?} lists a panel twice");
        }
        // The map workspace keeps every editor reachable.
        let map = tabs(&default_dock(Workspace::Map));
        for t in [Tab::Triggers, Tab::Data, Tab::Strings, Tab::Script] {
            assert!(map.contains(&t));
        }
    }

    #[test]
    fn focus_tab_prefers_the_current_workspace_then_the_tabs_home() {
        let mut s = Settings::default();
        // Triggers are in the Map workspace's bottom strip: stay here.
        s.focus_tab(Tab::Triggers);
        assert_eq!(s.workspace, Workspace::Map);

        // Data is not in the Text workspace: go to the Data workspace.
        s.workspace = Workspace::Text;
        s.focus_tab(Tab::Data);
        assert_eq!(s.workspace, Workspace::Data);

        // The map lives in the Map workspace.
        s.focus_tab(Tab::Viewport);
        assert_eq!(s.workspace, Workspace::Map);

        // A panel with no home opens in the current workspace.
        s.workspace = Workspace::Triggers;
        assert!(s.dock().find_tab(&Tab::Mcp).is_none());
        s.focus_tab(Tab::Mcp);
        assert_eq!(s.workspace, Workspace::Triggers);
        assert!(s.dock().find_tab(&Tab::Mcp).is_some());
    }

    #[test]
    fn switching_reopens_a_closed_main_panel() {
        let mut s = Settings::default();
        let path = s.docks.triggers.find_tab(&Tab::Triggers).unwrap();
        s.docks.triggers.remove_tab(path);
        assert!(s.docks.triggers.find_tab(&Tab::Triggers).is_none());
        s.switch_workspace(Workspace::Triggers);
        assert!(s.dock().find_tab(&Tab::Triggers).is_some());
    }
}
