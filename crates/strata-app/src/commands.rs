//! The command registry, the command palette (Ctrl+K), global keyboard
//! shortcuts and the shortcuts overlay (F1).
//!
//! Every user-facing action is a [`Command`]. The menu bar, the palette and
//! the keyboard all read labels and shortcuts from here, so they never drift.

use egui::{Key, KeyboardShortcut, Modifiers};

use crate::panels::Tab;
use crate::state::{Mode, Workspace};
use crate::theme::{self, ThemePref};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Command {
    NewMap,
    OpenMap,
    /// Native only: open an unpacked map folder.
    OpenFolder,
    Save,
    SaveAs,
    /// Native only: save as an unpacked map folder (component format).
    SaveAsFolder,
    ExportProject,
    OpenRecent(String),
    /// Import models, textures and sounds into the map.
    ImportAssets,
    /// Make a unit with its own model, icon and voice.
    NewCustomUnit,
    /// Make a sound from audio files in the map.
    NewSound,
    Undo,
    Redo,
    OpenPalette,
    ShowShortcuts,
    SetMode(Mode),
    /// Switch to a workspace (Ctrl+1..Ctrl+4).
    Workspace(Workspace),
    FocusTab(Tab),
    Theme(ThemePref),
    ResetLayout,
    ConnectAi,
    /// Read units, abilities and names from the user's StarCraft II install.
    LoadGameData,
    FrameMap,
    ShowProblems,
    InstallApp,
    /// Desktop: ask GitHub for a newer release.
    CheckForUpdates,
    /// Desktop: download and install the offered release.
    InstallUpdate,
    /// Desktop: restart into the installed update (guarded like closing a map).
    RestartToUpdate,
    ShowWelcome,
    About,
}

const fn ks(modifiers: Modifiers, key: Key) -> KeyboardShortcut {
    KeyboardShortcut::new(modifiers, key)
}

const CMD_SHIFT: Modifiers = Modifiers {
    alt: false,
    ctrl: false,
    shift: true,
    mac_cmd: false,
    command: true,
};

fn workspace_key(w: Workspace) -> Key {
    match w {
        Workspace::Map => Key::Num1,
        Workspace::Triggers => Key::Num2,
        Workspace::Data => Key::Num3,
        Workspace::Text => Key::Num4,
    }
}

fn mode_key(m: Mode) -> Key {
    match m.shortcut() {
        '1' => Key::Num1,
        '2' => Key::Num2,
        '3' => Key::Num3,
        '4' => Key::Num4,
        '5' => Key::Num5,
        '6' => Key::Num6,
        '7' => Key::Num7,
        '8' => Key::Num8,
        '9' => Key::Num9,
        _ => Key::Num0,
    }
}

impl Command {
    pub fn label(&self) -> String {
        match self {
            Command::NewMap => "New map…".into(),
            Command::OpenMap => "Open map…".into(),
            Command::OpenFolder => "Open map folder…".into(),
            Command::Save => "Save map".into(),
            Command::SaveAs => "Save map as .SC2Map file…".into(),
            Command::SaveAsFolder => "Save map as folder…".into(),
            Command::ImportAssets => "Import models, textures and sounds…".into(),
            Command::NewCustomUnit => "New custom unit…".into(),
            Command::NewSound => "New sound…".into(),
            Command::ExportProject => "Export project (.strata.json)".into(),
            Command::OpenRecent(p) => format!("Open {}", display_name(p)),
            Command::Undo => "Undo".into(),
            Command::Redo => "Redo".into(),
            Command::OpenPalette => "Search commands…".into(),
            Command::ShowShortcuts => "Keyboard shortcuts".into(),
            Command::SetMode(m) => format!("{} mode", m.label()),
            Command::Workspace(w) => format!("{} workspace", w.label()),
            Command::FocusTab(t) => format!("Show {}", t.title()),
            Command::Theme(ThemePref::Dark) => "Use dark theme".into(),
            Command::Theme(ThemePref::Light) => "Use light theme".into(),
            Command::Theme(ThemePref::System) => "Match system theme".into(),
            Command::ResetLayout => "Reset workspace layout".into(),
            Command::ConnectAi => "Connect an AI client (MCP)…".into(),
            Command::LoadGameData => "Load game data from StarCraft II folder…".into(),
            Command::FrameMap => "Frame the whole map".into(),
            Command::ShowProblems => "Check map for problems".into(),
            Command::InstallApp => "Install Strata as an app".into(),
            Command::CheckForUpdates => "Check for updates…".into(),
            Command::InstallUpdate => "Install update".into(),
            Command::RestartToUpdate => "Restart to update".into(),
            Command::ShowWelcome => "Welcome screen".into(),
            Command::About => "About Strata".into(),
        }
    }

    /// Group shown next to the label in the palette and in the F1 overlay.
    pub fn category(&self) -> &'static str {
        match self {
            Command::NewMap
            | Command::OpenMap
            | Command::OpenFolder
            | Command::Save
            | Command::SaveAs
            | Command::SaveAsFolder
            | Command::ExportProject
            | Command::OpenRecent(_) => "File",
            Command::ImportAssets | Command::NewCustomUnit | Command::NewSound => "Assets",
            Command::Undo | Command::Redo => "Edit",
            Command::SetMode(_) => "Mode",
            Command::Workspace(_) => "Workspace",
            Command::FocusTab(_) => "Panel",
            Command::Theme(_) | Command::ResetLayout | Command::OpenPalette => "View",
            Command::FrameMap | Command::ShowProblems => "Map",
            Command::LoadGameData => "Data",
            Command::ConnectAi => "AI link",
            Command::ShowShortcuts
            | Command::InstallApp
            | Command::CheckForUpdates
            | Command::InstallUpdate
            | Command::RestartToUpdate
            | Command::ShowWelcome
            | Command::About => "Help",
        }
    }

    /// Extra words the palette matches on (not shown).
    fn keywords(&self) -> &'static str {
        match self {
            Command::NewMap => "create blank",
            Command::OpenMap => "load import sc2map file",
            Command::OpenFolder => "load directory unpacked",
            Command::Save => "write sc2map",
            Command::SaveAs => "write sc2map archive file convert",
            Command::SaveAsFolder => "write components component list directory unpacked convert",
            Command::ImportAssets => "import add custom model m3 texture dds sound mp3 ogg wav audio asset file",
            Command::NewCustomUnit => "create custom unit hero model portrait icon voice",
            Command::NewSound => "create custom sound audio voice mp3 ogg wav",
            Command::FocusTab(Tab::Assets) => "imports files models textures sounds custom",
            Command::ExportProject => "json backup download",
            Command::OpenRecent(_) => "recent history",
            Command::Undo => "revert back",
            Command::Redo => "again forward",
            Command::ShowShortcuts => "keys keyboard hotkeys help",
            Command::SetMode(Mode::Sculpt) => "terrain height raise lower smooth",
            Command::SetMode(Mode::Paint) => "texture terrain",
            Command::SetMode(Mode::Cliff) => "cliffs level ramp",
            Command::SetMode(Mode::Pathing) => "walkable buildable",
            Command::SetMode(Mode::Units) => "place",
            Command::SetMode(Mode::Doodads) => "props decoration place",
            Command::SetMode(Mode::Points) => "markers start location",
            Command::Workspace(Workspace::Map) => "terrain viewport switch layout",
            Command::Workspace(Workspace::Triggers) => "script galaxy events switch layout",
            Command::Workspace(Workspace::Data) => "catalog units abilities switch layout",
            Command::Workspace(Workspace::Text) => "strings localization switch layout",
            Command::FocusTab(Tab::Strings) => "strings localization",
            Command::FocusTab(Tab::Script) => "galaxy code",
            Command::FocusTab(Tab::Mcp) => "mcp ai claude",
            Command::FocusTab(Tab::MapProps) => "info players settings",
            Command::Theme(_) => "appearance color mode",
            Command::ResetLayout => "panels dock restore default workspace",
            Command::ConnectAi => "mcp claude assistant link disconnect",
            Command::LoadGameData => "base game install casc units catalog blizzard mods",
            Command::FrameMap => "zoom fit camera center",
            Command::ShowProblems => "validate errors warnings",
            Command::InstallApp => "pwa desktop offline",
            Command::CheckForUpdates | Command::InstallUpdate | Command::RestartToUpdate => "update upgrade version release new",
            Command::About => "version license",
            _ => "",
        }
    }

    pub fn shortcut(&self) -> Option<KeyboardShortcut> {
        Some(match self {
            Command::NewMap => ks(Modifiers::COMMAND, Key::N),
            Command::OpenMap => ks(Modifiers::COMMAND, Key::O),
            Command::Save => ks(Modifiers::COMMAND, Key::S),
            Command::SaveAs => ks(CMD_SHIFT, Key::S),
            Command::Undo => ks(Modifiers::COMMAND, Key::Z),
            Command::Redo => ks(CMD_SHIFT, Key::Z),
            Command::OpenPalette => ks(Modifiers::COMMAND, Key::K),
            Command::ShowShortcuts => ks(Modifiers::NONE, Key::F1),
            Command::SetMode(m) => ks(Modifiers::NONE, mode_key(*m)),
            Command::Workspace(w) => ks(Modifiers::COMMAND, workspace_key(*w)),
            _ => return None,
        })
    }

    /// Does this command edit text-field-like state (and so must yield to a
    /// focused text field)?
    fn yields_to_text(&self) -> bool {
        matches!(self, Command::Undo | Command::Redo | Command::SetMode(_))
    }
}

/// File name without directories, for recent-file labels.
pub fn display_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// What the registry needs to know about the environment.
pub struct Context<'a> {
    pub recent_files: &'a [String],
    pub can_install: bool,
}

/// Every command available right now, in menu order.
pub fn all(cx: &Context<'_>) -> Vec<Command> {
    let mut v = vec![Command::NewMap, Command::OpenMap];
    if cfg!(not(target_arch = "wasm32")) {
        v.push(Command::OpenFolder);
    }
    v.extend([Command::Save, Command::SaveAs]);
    if cfg!(not(target_arch = "wasm32")) {
        v.push(Command::SaveAsFolder);
    }
    v.push(Command::ExportProject);
    v.extend([Command::ImportAssets, Command::NewCustomUnit, Command::NewSound]);
    v.extend(cx.recent_files.iter().cloned().map(Command::OpenRecent));
    v.extend([Command::Undo, Command::Redo]);
    v.extend(Workspace::ALL.map(Command::Workspace));
    v.extend(Mode::ALL.map(Command::SetMode));
    v.extend(Tab::ALL.map(Command::FocusTab));
    v.extend(ThemePref::ALL.map(Command::Theme));
    v.extend([
        Command::ResetLayout,
        Command::FrameMap,
        Command::ShowProblems,
        Command::LoadGameData,
        Command::ConnectAi,
        Command::ShowShortcuts,
        Command::ShowWelcome,
    ]);
    if cx.can_install {
        v.push(Command::InstallApp);
    }
    if cfg!(not(target_arch = "wasm32")) {
        v.push(Command::CheckForUpdates);
    }
    v.push(Command::About);
    v
}

/// Commands that have keyboard shortcuts, most specific first so that
/// Ctrl+Shift+S is consumed before Ctrl+S.
fn shortcut_commands() -> Vec<Command> {
    let mut v = vec![
        Command::SaveAs,
        Command::Redo,
        Command::NewMap,
        Command::OpenMap,
        Command::Save,
        Command::Undo,
        Command::OpenPalette,
        Command::ShowShortcuts,
    ];
    v.extend(Workspace::ALL.map(Command::Workspace));
    v.extend(Mode::ALL.map(Command::SetMode));
    v
}

/// Read global shortcuts for this frame. Shortcuts that would fight a
/// focused text field (undo, number keys) are left to the field.
pub fn handle_shortcuts(ctx: &egui::Context, blocked: bool) -> Vec<Command> {
    if blocked {
        return Vec::new();
    }
    let text_focus = ctx.text_edit_focused();
    let redo_y = ks(Modifiers::COMMAND, Key::Y);
    let mut out = Vec::new();
    ctx.input_mut(|i| {
        if !text_focus && i.consume_shortcut(&redo_y) {
            out.push(Command::Redo);
        }
        for cmd in shortcut_commands() {
            if text_focus && cmd.yields_to_text() {
                continue;
            }
            let Some(s) = cmd.shortcut() else { continue };
            let hit = if s.modifiers.is_none() {
                // Plain keys must not fire with modifiers held (Ctrl+1 etc.).
                i.modifiers.is_none() && i.consume_key(Modifiers::NONE, s.logical_key)
            } else {
                i.consume_shortcut(&s)
            };
            if hit {
                out.push(cmd);
            }
        }
    });
    out
}

/// Case-insensitive subsequence match. Returns a score (higher is better)
/// and the matched char indices in `text`.
pub fn fuzzy_match(query: &str, text: &str) -> Option<(i32, Vec<usize>)> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars
        .iter()
        .map(|c| c.to_lowercase().next().unwrap_or(*c))
        .collect();
    let mut idx = Vec::with_capacity(q.len());
    let mut score = 0i32;
    let mut qi = 0;
    let mut last: Option<usize> = None;
    for (ti, &c) in lower.iter().enumerate() {
        if qi < q.len() && c == q[qi] {
            let word_start = ti == 0 || !chars[ti - 1].is_alphanumeric();
            score += 10;
            if word_start {
                score += 8;
            }
            if ti == 0 {
                score += 6;
            }
            match last {
                Some(l) if l + 1 == ti => score += 12,
                Some(l) => score -= (ti - l - 1).min(6) as i32,
                None => score -= ti.min(10) as i32,
            }
            idx.push(ti);
            last = Some(ti);
            qi += 1;
        }
    }
    (qi == q.len()).then_some((score - chars.len() as i32 / 8, idx))
}

fn score(cmd: &Command, query: &str) -> Option<(i32, Vec<usize>)> {
    let label = cmd.label();
    if let Some(hit) = fuzzy_match(query, &label) {
        return Some(hit);
    }
    let alt = format!("{} {}", cmd.category(), cmd.keywords());
    fuzzy_match(query, &alt).map(|(s, _)| (s / 2 - 20, Vec::new()))
}

#[derive(Default)]
pub struct CommandPalette {
    open: bool,
    query: String,
    selected: usize,
    just_opened: bool,
}

impl CommandPalette {
    pub fn open(&mut self) {
        self.open = true;
        self.query.clear();
        self.selected = 0;
        self.just_opened = true;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn toggle(&mut self) {
        if self.open {
            self.close();
        } else {
            self.open();
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Draw the palette; returns the chosen command, if any.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        cx: &Context<'_>,
        recent: &[Command],
    ) -> Option<Command> {
        if !self.open {
            return None;
        }
        let t = theme::tokens(ctx);
        let commands = all(cx);

        // Rows: (command, highlighted char indices, is_recent).
        let mut rows: Vec<(Command, Vec<usize>, bool)> = Vec::new();
        if self.query.trim().is_empty() {
            for r in recent.iter().filter(|r| commands.contains(r)).take(5) {
                rows.push((r.clone(), Vec::new(), true));
            }
            for c in &commands {
                if !rows.iter().any(|(r, _, _)| r == c) {
                    rows.push((c.clone(), Vec::new(), false));
                }
            }
        } else {
            let mut scored: Vec<(i32, usize, Command, Vec<usize>)> = commands
                .iter()
                .enumerate()
                .filter_map(|(order, c)| {
                    let (mut s, idx) = score(c, &self.query)?;
                    if let Some(pos) = recent.iter().position(|r| r == c) {
                        s += 6 - pos.min(5) as i32;
                    }
                    Some((s, order, c.clone(), idx))
                })
                .collect();
            scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            rows = scored
                .into_iter()
                .map(|(_, _, c, idx)| (c, idx, false))
                .collect();
        }

        // Keyboard navigation, consumed before the text field sees it.
        let (up, down, enter, page_up, page_down) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::PageUp),
                i.consume_key(Modifiers::NONE, Key::PageDown),
            )
        });
        let n = rows.len();
        let mut moved = false;
        if n > 0 {
            if down {
                self.selected = (self.selected + 1) % n;
            }
            if up {
                self.selected = (self.selected + n - 1) % n;
            }
            if page_down {
                self.selected = (self.selected + 8).min(n - 1);
            }
            if page_up {
                self.selected = self.selected.saturating_sub(8);
            }
            moved = up || down || page_up || page_down;
            self.selected = self.selected.min(n - 1);
        }
        let mut chosen = (enter && n > 0).then(|| rows[self.selected].0.clone());

        let area = egui::Modal::default_area(egui::Id::new("command-palette"))
            .anchor(egui::Align2::CENTER_TOP, [0.0, 72.0]);
        let frame = egui::Frame::popup(&ctx.global_style())
            .fill(t.bg_raised)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(0));
        let modal = egui::Modal::new(egui::Id::new("command-palette"))
            .area(area)
            .frame(frame)
            .backdrop_color(egui::Color32::from_black_alpha(
                if ctx.global_style().visuals.dark_mode {
                    110
                } else {
                    60
                },
            ))
            .show(ctx, |ui| {
                let width = (ctx.content_rect().width() - 32.0).clamp(320.0, 600.0);
                ui.set_width(width);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);

                // Search field.
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(14, 10))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) = ui
                                .allocate_exact_size(egui::vec2(18.0, 24.0), egui::Sense::hover());
                            paint_search_icon(ui.painter(), r.center(), t.text_dim);
                            ui.add_space(8.0);
                            let edit = egui::TextEdit::singleline(&mut self.query)
                                .hint_text("Search commands")
                                .frame(egui::Frame::NONE)
                                .desired_width(f32::INFINITY)
                                .font(egui::FontId::new(16.0, egui::FontFamily::Proportional));
                            let resp = ui.add(edit);
                            if self.just_opened || !resp.has_focus() {
                                resp.request_focus();
                                self.just_opened = false;
                            }
                            if resp.changed() {
                                self.selected = 0;
                            }
                        });
                    });
                let sep = ui.available_rect_before_wrap();
                ui.painter()
                    .hline(sep.x_range(), sep.top(), egui::Stroke::new(1.0, t.stroke));

                if rows.is_empty() {
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(16, 18))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "No commands match “{}”.",
                                    self.query.trim()
                                ))
                                .color(t.text_dim),
                            );
                        });
                    return;
                }

                egui::ScrollArea::vertical()
                    .max_height(380.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.add_space(4.0);
                        let mut last_section: Option<bool> = None;
                        let browsing = self.query.trim().is_empty();
                        for (i, (cmd, hl, is_recent)) in rows.iter().enumerate() {
                            if browsing
                                && last_section != Some(*is_recent)
                                && rows.iter().any(|r| r.2)
                            {
                                section_label(
                                    ui,
                                    if *is_recent {
                                        "Recently used"
                                    } else {
                                        "All commands"
                                    },
                                    &t,
                                );
                                last_section = Some(*is_recent);
                            }
                            let resp = command_row(ui, ctx, cmd, hl, i == self.selected, &t);
                            if moved && i == self.selected {
                                resp.scroll_to_me(None);
                            }
                            if resp.hovered()
                                && ctx.input(|inp| inp.pointer.delta() != egui::Vec2::ZERO)
                            {
                                self.selected = i;
                            }
                            if resp.clicked() {
                                chosen = Some(cmd.clone());
                            }
                        }
                        ui.add_space(4.0);
                    });

                // Footer hint.
                let sep = ui.available_rect_before_wrap();
                ui.painter()
                    .hline(sep.x_range(), sep.top(), egui::Stroke::new(1.0, t.stroke));
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(14, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            for (k, what) in
                                [("⏶⏷", "to move"), ("Enter", "to run"), ("Esc", "to close")]
                            {
                                keycap(ui, k, &t);
                                ui.label(egui::RichText::new(what).small().color(t.text_dim));
                                ui.add_space(6.0);
                            }
                        });
                    });
            });
        if modal.should_close() {
            self.close();
        }
        if chosen.is_some() {
            self.close();
        }
        chosen
    }
}

fn section_label(ui: &mut egui::Ui, text: &str, t: &theme::Tokens) {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 14,
            right: 14,
            top: 6,
            bottom: 2,
        })
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).small().color(t.text_dim));
        });
}

fn command_row(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    cmd: &Command,
    highlight: &[usize],
    selected: bool,
    t: &theme::Tokens,
) -> egui::Response {
    let height = 32.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let painter = ui.painter_at(rect);
    let row = rect.shrink2(egui::vec2(6.0, 1.0));
    if selected {
        let dark = ui.visuals().dark_mode;
        painter.rect_filled(row, 4.0, theme::selection_fill(t, dark));
        let bar = egui::Rect::from_min_size(
            row.min + egui::vec2(0.0, 6.0),
            egui::vec2(3.0, row.height() - 12.0),
        );
        painter.rect_filled(bar, 1.5, t.accent);
    }

    // Label with matched characters highlighted.
    let label = cmd.label();
    let mut job = egui::text::LayoutJob::default();
    let normal = egui::TextFormat::simple(
        egui::FontId::new(14.0, egui::FontFamily::Proportional),
        t.text,
    );
    let strong =
        egui::TextFormat::simple(egui::FontId::new(14.0, theme::semibold()), t.accent_text);
    for (ci, ch) in label.chars().enumerate() {
        let fmt = if highlight.contains(&ci) {
            strong.clone()
        } else {
            normal.clone()
        };
        job.append(&ch.to_string(), 0.0, fmt);
    }
    let galley = painter.layout_job(job);
    let text_pos = egui::pos2(row.left() + 12.0, row.center().y - galley.size().y / 2.0);
    let label_right = text_pos.x + galley.size().x;
    painter.galley(text_pos, galley, t.text);

    // Shortcut keycaps on the right, category before them.
    let mut x = row.right() - 8.0;
    if let Some(s) = cmd.shortcut() {
        let text = ctx.format_shortcut(&s);
        let g = ui.ctx().fonts_mut(|f| {
            f.layout_no_wrap(
                text,
                egui::FontId::new(12.0, egui::FontFamily::Proportional),
                t.text_dim,
            )
        });
        let w = g.size().x + 12.0;
        let cap = egui::Rect::from_min_size(
            egui::pos2(x - w, row.center().y - 10.0),
            egui::vec2(w, 20.0),
        );
        painter.rect(
            cap,
            4.0,
            t.bg_panel,
            egui::Stroke::new(1.0, t.stroke),
            egui::StrokeKind::Inside,
        );
        painter.galley(cap.center() - g.size() / 2.0, g, t.text_dim);
        x = cap.left() - 10.0;
    }
    let cat = ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(
            cmd.category().to_owned(),
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            t.text_dim,
        )
    });
    if x - cat.size().x > label_right + 12.0 {
        painter.galley(
            egui::pos2(x - cat.size().x, row.center().y - cat.size().y / 2.0),
            cat,
            t.text_dim,
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A small key label, e.g. "Ctrl+K".
pub fn keycap(ui: &mut egui::Ui, text: &str, t: &theme::Tokens) {
    let g = ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(
            text.to_owned(),
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
            t.text_dim,
        )
    });
    let size = egui::vec2(g.size().x + 10.0, 18.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect(
        rect,
        4.0,
        t.bg_panel,
        egui::Stroke::new(1.0, t.stroke),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .galley(rect.center() - g.size() / 2.0, g, t.text_dim);
}

pub fn paint_search_icon(p: &egui::Painter, c: egui::Pos2, color: egui::Color32) {
    let s = egui::Stroke::new(1.6, color);
    p.circle_stroke(c + egui::vec2(-1.5, -1.5), 5.0, s);
    p.line_segment([c + egui::vec2(2.2, 2.2), c + egui::vec2(6.0, 6.0)], s);
}

/// F1: every shortcut, grouped.
#[derive(Default)]
pub struct ShortcutsOverlay {
    pub open: bool,
}

impl ShortcutsOverlay {
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.open {
            return;
        }
        let t = theme::tokens(ctx);
        let groups: [(&str, Vec<(String, String)>); 5] = [
            (
                "File",
                [
                    Command::NewMap,
                    Command::OpenMap,
                    Command::Save,
                    Command::SaveAs,
                ]
                .iter()
                .map(|c| (c.label(), ctx.format_shortcut(&c.shortcut().unwrap())))
                .collect(),
            ),
            (
                "Edit and view",
                vec![
                    (
                        Command::Undo.label(),
                        ctx.format_shortcut(&Command::Undo.shortcut().unwrap()),
                    ),
                    (
                        Command::Redo.label(),
                        format!(
                            "{}  or  {}",
                            ctx.format_shortcut(&Command::Redo.shortcut().unwrap()),
                            ctx.format_shortcut(&ks(Modifiers::COMMAND, Key::Y))
                        ),
                    ),
                    (
                        "Search commands".into(),
                        ctx.format_shortcut(&Command::OpenPalette.shortcut().unwrap()),
                    ),
                    ("Keyboard shortcuts".into(), "F1".into()),
                ],
            ),
            (
                "Workspaces",
                Workspace::ALL
                    .iter()
                    .map(|w| {
                        let c = Command::Workspace(*w);
                        (
                            w.label().to_owned(),
                            ctx.format_shortcut(&c.shortcut().unwrap()),
                        )
                    })
                    .collect(),
            ),
            (
                "Modes",
                Mode::ALL
                    .iter()
                    .map(|m| (m.label().to_owned(), m.shortcut().to_string()))
                    .collect(),
            ),
            (
                "Command palette",
                vec![
                    ("Move selection".into(), "⏶ ⏷".into()),
                    ("Run command".into(), "Enter".into()),
                    ("Close".into(), "Esc".into()),
                ],
            ),
        ];
        let resp = egui::Modal::new(egui::Id::new("shortcuts-overlay"))
            .frame(theme::dialog_frame(ctx))
            .show(ctx, |ui| {
                ui.set_width(560.0_f32.min(ctx.content_rect().width() - 48.0));
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new("Keyboard shortcuts").text_style(theme::title_style()),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.open = false;
                        }
                    });
                });
                ui.label(
                egui::RichText::new(
                    "Shortcuts that edit (undo, number keys) pause while you type in a text field.",
                )
                .color(t.text_dim),
            );
                ui.add_space(12.0);
                ui.columns(2, |cols| {
                    for (gi, (title, items)) in groups.iter().enumerate() {
                        let ui = &mut cols[if gi % 2 == 0 { 0 } else { 1 }];
                        ui.label(egui::RichText::new(*title).text_style(egui::TextStyle::Heading));
                        ui.add_space(4.0);
                        egui::Grid::new(("sc-grid", gi))
                            .num_columns(2)
                            .spacing([16.0, 6.0])
                            .show(ui, |ui| {
                                for (label, keys) in items {
                                    ui.label(label);
                                    keycap(ui, keys, &t);
                                    ui.end_row();
                                }
                            });
                        ui.add_space(12.0);
                    }
                });
            });
        if resp.should_close() {
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_word_starts() {
        let (a, ia) = fuzzy_match("sm", "Save map").unwrap();
        let (b, _) = fuzzy_match("sm", "Use system theme").unwrap();
        assert!(a > b);
        assert_eq!(ia, vec![0, 5]);
        assert!(fuzzy_match("xyz", "Save map").is_none());
    }

    #[test]
    fn shortcuts_are_unique() {
        let cmds = shortcut_commands();
        for (i, a) in cmds.iter().enumerate() {
            for b in &cmds[i + 1..] {
                assert_ne!(a.shortcut(), b.shortcut(), "{a:?} and {b:?}");
            }
        }
        // Workspaces use Ctrl+digit; plain digits stay mode keys.
        let ws = Command::Workspace(Workspace::Triggers).shortcut().unwrap();
        assert_eq!(ws.modifiers, Modifiers::COMMAND);
        assert!(!Command::Workspace(Workspace::Data).yields_to_text());
    }

    #[test]
    fn every_command_has_a_label_and_category() {
        let cx = Context {
            recent_files: &["C:/maps/Test.SC2Map".into()],
            can_install: true,
        };
        for c in all(&cx) {
            assert!(!c.label().is_empty());
            assert!(!c.category().is_empty());
        }
        assert_eq!(
            Command::OpenRecent("C:/maps/Test.SC2Map".into()).label(),
            "Open Test.SC2Map"
        );
    }
}
