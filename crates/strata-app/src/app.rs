//! The application shell: menu bar, mode rail, dock area, status bar,
//! command palette and toasts.

use egui_dock::{DockArea, DockState, NodeIndex};
use strata_core::MapDocument;

use crate::commands::{CommandPalette, Shortcuts};
use crate::panels::{Tab, Viewer};
use crate::state::{AppAction, EditorState, Mode};
use crate::theme::{self, ThemePref};

const SETTINGS_KEY: &str = "strata.settings.v1";

/// Settings persisted across sessions (eframe storage: localStorage on web).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Settings {
    pub theme: ThemePref,
    pub dock: DockState<Tab>,
    pub recent: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self { theme: ThemePref::Dark, dock: default_dock(), recent: Vec::new() }
    }
}

pub fn default_dock() -> DockState<Tab> {
    let mut dock = DockState::new(vec![Tab::Viewport]);
    let surface = dock.main_surface_mut();
    let [main, right] = surface.split_right(NodeIndex::root(), 0.77, vec![Tab::Inspector, Tab::MapProps]);
    surface.split_below(right, 0.64, vec![Tab::Minimap]);
    surface.split_below(
        main,
        0.68,
        vec![Tab::Triggers, Tab::Data, Tab::Strings, Tab::Script, Tab::Problems, Tab::History, Tab::Mcp],
    );
    dock
}

pub struct StrataApp {
    pub state: EditorState,
    pub settings: Settings,
    pub palette: CommandPalette,
    applied_theme: Option<(ThemePref, bool)>,
}

impl StrataApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        theme::install_fonts(&cc.egui_ctx);
        let settings: Settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        let doc = crate::io::restore_autosave().unwrap_or_else(|| MapDocument::new("Untitled map", 128, 128));
        Self { state: EditorState::new(doc), settings, palette: CommandPalette::default(), applied_theme: None }
    }

    fn apply_theme(&mut self, ctx: &egui::Context) {
        let system_dark = ctx.input(|i| i.raw.system_theme.map(|t| t == egui::Theme::Dark).unwrap_or(true));
        let key = (self.settings.theme, system_dark);
        if self.applied_theme != Some(key) {
            theme::apply(ctx, self.settings.theme.resolve(system_dark));
            self.applied_theme = Some(key);
        }
    }

    fn drain_actions(&mut self, ctx: &egui::Context) {
        for a in std::mem::take(&mut self.state.actions) {
            match a {
                AppAction::NewMap => crate::io::new_map(&mut self.state),
                AppAction::Open => crate::io::open(ctx, &mut self.state),
                AppAction::Save => crate::io::save(&mut self.state, false),
                AppAction::SaveAs => crate::io::save(&mut self.state, true),
                AppAction::Undo => self.state.undo(),
                AppAction::Redo => self.state.redo(),
                AppAction::OpenCommandPalette => self.palette.open(),
                AppAction::FocusTab(tab) => {
                    if let Some(found) = self.settings.dock.find_tab(&tab) {
                        let _ = self.settings.dock.set_active_tab(found);
                    } else {
                        self.settings.dock.push_to_focused_leaf(tab);
                    }
                }
                AppAction::FrameWorld(p) => crate::panels::viewport::frame_world(&mut self.state, p),
            }
        }
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("New map…").clicked() {
                    self.state.request(AppAction::NewMap);
                }
                if ui.button("Open…").clicked() {
                    self.state.request(AppAction::Open);
                }
                ui.separator();
                if ui.button("Save").clicked() {
                    self.state.request(AppAction::Save);
                }
                if ui.button("Save as…").clicked() {
                    self.state.request(AppAction::SaveAs);
                }
            });
            ui.menu_button("Edit", |ui| {
                if ui.add_enabled(self.state.history.can_undo(), egui::Button::new("Undo")).clicked() {
                    self.state.request(AppAction::Undo);
                }
                if ui.add_enabled(self.state.history.can_redo(), egui::Button::new("Redo")).clicked() {
                    self.state.request(AppAction::Redo);
                }
                ui.separator();
                if ui.button("Command palette…").clicked() {
                    self.state.request(AppAction::OpenCommandPalette);
                }
            });
            ui.menu_button("View", |ui| {
                for tab in Tab::ALL {
                    if ui.button(tab.title()).clicked() {
                        self.state.request(AppAction::FocusTab(tab));
                    }
                }
                ui.separator();
                ui.label("Theme");
                for t in ThemePref::ALL {
                    ui.radio_value(&mut self.settings.theme, t, t.label());
                }
                ui.separator();
                if ui.button("Reset layout").clicked() {
                    self.settings.dock = default_dock();
                }
            });
        });
    }

    fn mode_rail(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            for m in Mode::ALL {
                let selected = self.state.mode == m;
                let r = ui
                    .add_sized([56.0, 40.0], egui::Button::selectable(selected, m.label()))
                    .on_hover_text(format!("{} ({})", m.label(), m.shortcut()));
                if r.clicked() {
                    self.state.mode = m;
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(self.state.title());
            ui.separator();
            ui.label(format!("{} × {}", self.state.doc.info.width, self.state.doc.info.height));
            ui.separator();
            let n = self.state.problems().len();
            if ui.link(format!("{n} problems")).clicked() {
                self.state.request(AppAction::FocusTab(Tab::Problems));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(crate::panels::viewport::status_text(&self.state));
            });
        });
    }
}

impl eframe::App for StrataApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.apply_theme(&ctx);
        Shortcuts::handle(&ctx, &mut self.state, &mut self.palette);
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(self.state.title()));

        egui::Panel::top("menu").show(ui, |ui| self.menu_bar(ui));
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("modes").resizable(false).exact_size(68.0).show(ui, |ui| self.mode_rail(ui));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
            let style = egui_dock::Style::from_egui(ui.style());
            let mut viewer = Viewer { state: &mut self.state };
            DockArea::new(&mut self.settings.dock).style(style).show_inside(ui, &mut viewer);
        });

        self.palette.show(&ctx, &mut self.state);
        theme::show_toasts(&ctx, &mut self.state);
        self.drain_actions(&ctx);
        crate::io::tick_autosave(&ctx, &mut self.state);
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }
}
