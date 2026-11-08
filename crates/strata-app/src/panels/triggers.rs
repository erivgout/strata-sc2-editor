//! Trigger editor: a tree of folders, triggers, custom functions and global
//! variables on the left; the selected item's events, conditions, local
//! variables and actions on the right, each line a sentence whose parameters
//! are clickable chips.
//!
//! Editing works on a copy of the selected item each frame; when the copy
//! differs from the document it is committed as one undoable
//! `Edit::Triggers`. Typing in the same field keeps amending the same history
//! entry (see [`commit`]), so undo steps by field, not by keystroke.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use egui::{Align2, Color32, FontId, Id, Key, Modifiers, PopupCloseBehavior, Rect, RichText, Sense, Stroke, Ui, Vec2};
use serde::{Deserialize, Serialize};
use strata_core::galaxy;
use strata_core::history::Edit;
use strata_core::objects::ObjectKind;
use strata_core::strings::{Strings, DEFAULT_LOCALE};
use strata_core::trigger_lib::{self, FnKind, FnSpec, ParamSpec};
use strata_core::triggers::{
    Call, CustomFunction, Expr, GalaxyType, Literal, Stmt, SwitchCase, Trigger, TriggerId, TriggerItem, TriggerLibrary, Variable,
};
use strata_core::MapDocument;

use crate::panels::Tab;
use crate::state::{AppAction, EditorState, ToastKind};
use crate::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// A line in the item editor.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum LineRef {
    Event(usize),
    Condition(usize),
    Param(usize),
    Local(usize),
    /// Path into nested action lists: `[i]`, `[i, block, j]`, ...
    Action(Vec<usize>),
}

impl LineRef {
    fn section(&self) -> Section {
        match self {
            LineRef::Event(_) => Section::Events,
            LineRef::Condition(_) => Section::Conditions,
            LineRef::Param(_) => Section::Params,
            LineRef::Local(_) => Section::Locals,
            LineRef::Action(_) => Section::Actions,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Events,
    Conditions,
    Params,
    Locals,
    Actions,
}

/// Where a picked library entry is inserted.
#[derive(Debug, Clone, PartialEq)]
enum InsertAt {
    Event(usize),
    Condition(usize),
    /// List path (even length) and index in that list.
    Action(Vec<usize>, usize),
}

#[derive(Debug, Clone, PartialEq)]
enum Purpose {
    Event,
    Condition,
    Action,
}

struct Picker {
    purpose: Purpose,
    at: InsertAt,
    item: TriggerId,
    pos: egui::Pos2,
    fresh: bool,
    rect: Option<Rect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Tree,
    Editor,
    Elsewhere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RightTab {
    Editor,
    Script,
}

struct LastCommit {
    key: Id,
    revision: u64,
    before: Box<TriggerLibrary>,
    before_strings: Option<Box<Strings>>,
}

pub struct TriggersState {
    filter: String,
    rename: Option<(TriggerId, String, bool)>,
    selected_var: Option<usize>,
    collapsed_tree: HashSet<TriggerId>,
    collapsed: HashSet<(TriggerId, Vec<usize>)>,
    cursor: Option<(TriggerId, LineRef)>,
    picker: Option<Picker>,
    focus: Focus,
    tab: RightTab,
    dragging: Option<TriggerId>,
    last_commit: Option<LastCommit>,
    script_job: Option<(u64, bool, egui::text::LayoutJob)>,
    checks: (u64, Vec<(String, String)>),
    focus_filter: bool,
    scroll_to_cursor: bool,
}

impl Default for TriggersState {
    fn default() -> Self {
        Self {
            filter: String::new(),
            rename: None,
            selected_var: None,
            collapsed_tree: HashSet::new(),
            collapsed: HashSet::new(),
            cursor: None,
            picker: None,
            focus: Focus::Elsewhere,
            tab: RightTab::Editor,
            dragging: None,
            last_commit: None,
            script_job: None,
            checks: (u64::MAX, Vec::new()),
            focus_filter: false,
            scroll_to_cursor: false,
        }
    }
}

/// Clipboard payload for copy / paste of trigger lines and tree items.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Clip {
    Events(Vec<Call>),
    Conditions(Vec<Expr>),
    Variables(Vec<Variable>),
    Actions(Vec<Stmt>),
    Items(Vec<TriggerItem>),
}

#[derive(Serialize, Deserialize)]
struct ClipDoc {
    strata_clip: Clip,
}

fn clip_text(c: Clip) -> String {
    serde_json::to_string_pretty(&ClipDoc { strata_clip: c }).unwrap_or_default()
}

fn parse_clip(s: &str) -> Option<Clip> {
    serde_json::from_str::<ClipDoc>(s).ok().map(|d| d.strata_clip)
}

// ---------------------------------------------------------------------------
// Commit with coalescing
// ---------------------------------------------------------------------------

/// Commit a new trigger library (and optionally new strings). With `key`,
/// consecutive commits from the same widget amend one history entry as long
/// as nothing else touched history in between.
fn commit(state: &mut EditorState, label: String, after: TriggerLibrary, strings: Option<Strings>, key: Option<Id>) {
    if after == state.doc.triggers && strings.as_ref().is_none_or(|s| *s == state.doc.strings) {
        return;
    }
    let revision = state.history.revision();
    let coalesce = key.is_some()
        && state.ui.triggers.last_commit.as_ref().is_some_and(|l| Some(l.key) == key && l.revision == revision && state.history.can_undo());
    let (before, mut before_strings) = if coalesce {
        let l = state.ui.triggers.last_commit.take().expect("checked");
        state.history.undo(&mut state.doc);
        (l.before, l.before_strings)
    } else {
        (Box::new(state.doc.triggers.clone()), None)
    };
    if strings.is_some() && before_strings.is_none() {
        before_strings = Some(Box::new(state.doc.strings.clone()));
    }
    let trig = Edit::Triggers { before: before.clone(), after: Box::new(after) };
    let edit = match (&strings, &before_strings) {
        (Some(s), Some(bs)) => Edit::Batch(vec![Edit::Strings { before: bs.clone(), after: Box::new(s.clone()) }, trig]),
        (None, Some(bs)) => Edit::Batch(vec![Edit::Strings { before: bs.clone(), after: Box::new(state.doc.strings.clone()) }, trig]),
        _ => trig,
    };
    state.commit(label, edit);
    state.ui.triggers.last_commit = key.map(|key| LastCommit { key, revision: state.history.revision(), before, before_strings });
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let ctx = ui.ctx().clone();
    let panel = ui.max_rect();
    if ctx.input(|i| i.pointer.any_pressed()) {
        if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
            if !panel.contains(p) {
                state.ui.triggers.focus = Focus::Elsewhere;
            }
        }
    }

    // Keep the selection valid.
    if let Some(id) = state.selection.trigger {
        if state.doc.triggers.find(id).is_none() && state.doc.triggers.function(id).is_none() {
            state.selection.trigger = None;
        }
    }
    if state.ui.triggers.selected_var.is_some_and(|i| i >= state.doc.triggers.variables.len()) {
        state.ui.triggers.selected_var = None;
    }

    let mut tree_out = Vec::new();
    egui::Panel::left("triggers-tree").resizable(true).default_size(250.0).size_range(170.0..=520.0).show(ui, |ui| {
        let r = ui.max_rect();
        if ctx.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(r) {
            state.ui.triggers.focus = Focus::Tree;
        }
        tree_ui(ui, state, &mut tree_out);
    });
    apply_tree_actions(&ctx, state, tree_out);

    egui::CentralPanel::default().show(ui, |ui| {
        let r = ui.max_rect();
        if ctx.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(r) && state.ui.triggers.picker.is_none() {
            state.ui.triggers.focus = Focus::Editor;
        }
        ui.horizontal(|ui| {
            let st = &mut state.ui.triggers;
            ui.selectable_value(&mut st.tab, RightTab::Editor, "Editor");
            ui.selectable_value(&mut st.tab, RightTab::Script, "Custom script");
        });
        ui.separator();
        match state.ui.triggers.tab {
            RightTab::Editor => editor_ui(ui, state),
            RightTab::Script => custom_script_ui(ui, state),
        }
    });
}

// ---------------------------------------------------------------------------
// Tree
// ---------------------------------------------------------------------------

enum TreeAction {
    Select(TriggerId),
    SelectVar(usize),
    StartRename(TriggerId),
    Rename(TriggerId, String),
    SetEnabled(TriggerId, bool),
    NewTrigger,
    NewFolder,
    NewFunction,
    NewVariable,
    Duplicate(TriggerId),
    Delete(TriggerId),
    Move(TriggerId, isize),
    Drop(TriggerId, Option<TriggerId>, usize),
    DeleteVar(usize),
    DuplicateVar(usize),
    Copy(TriggerId),
    Paste,
    ToggleFolder(TriggerId),
}

struct Row {
    id: TriggerId,
    rect: Rect,
    folder: bool,
    parent: Option<TriggerId>,
    index: usize,
}

fn matches_filter(name: &str, filter: &str) -> bool {
    filter.trim().is_empty() || trigger_lib::fuzzy_score(filter, name).is_some()
}

fn subtree_matches(item: &TriggerItem, filter: &str) -> bool {
    match item {
        TriggerItem::Trigger(t) => matches_filter(&t.name, filter),
        TriggerItem::Folder { name, children, .. } => matches_filter(name, filter) || children.iter().any(|c| subtree_matches(c, filter)),
    }
}

fn tree_ui(ui: &mut Ui, state: &mut EditorState, out: &mut Vec<TreeAction>) {
    let tk = theme::tokens(ui.ctx());
    ui.horizontal(|ui| {
        if ui.button("New trigger").on_hover_text("Add a trigger after the selection").clicked() {
            out.push(TreeAction::NewTrigger);
        }
        if ui.button("New folder").clicked() {
            out.push(TreeAction::NewFolder);
        }
        ui.menu_button("More", |ui| {
            if ui.button("New function").on_hover_text("A reusable action or value function").clicked() {
                out.push(TreeAction::NewFunction);
            }
            if ui.button("New variable").on_hover_text("A global variable").clicked() {
                out.push(TreeAction::NewVariable);
            }
        });
    });
    let st = &mut state.ui.triggers;
    let filter_resp = ui.add(
        egui::TextEdit::singleline(&mut st.filter).hint_text("Filter triggers").desired_width(f32::INFINITY).id_salt("trigger-filter"),
    );
    if st.focus_filter {
        filter_resp.request_focus();
        st.focus_filter = false;
    }
    ui.add_space(4.0);

    let lib = &state.doc.triggers;
    let filter = st.filter.clone();
    let mut rows: Vec<Row> = Vec::new();
    egui::ScrollArea::vertical().id_salt("trigger-tree-scroll").auto_shrink([false, false]).show(ui, |ui| {
        if lib.items.is_empty() {
            ui.label(RichText::new("No triggers yet. Triggers run actions when events happen in the game.").color(tk.text_dim));
            if ui.button("New trigger").clicked() {
                out.push(TreeAction::NewTrigger);
            }
        }
        tree_items(ui, &tk, state, &lib.items, 0, None, &filter, &mut rows, out);

        // Functions.
        if !lib.functions.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new("Functions").color(tk.text_dim).small());
            for f in &lib.functions {
                if !matches_filter(&f.name, &filter) {
                    continue;
                }
                let selected = state.selection.trigger == Some(f.id) && state.ui.triggers.selected_var.is_none();
                let kind = if f.returns.is_some() { "function" } else { "action" };
                let resp = tree_row(ui, &tk, &f.name, Some(kind), 0, selected, None, true);
                if resp.clicked() {
                    out.push(TreeAction::Select(f.id));
                }
                if resp.double_clicked() {
                    out.push(TreeAction::StartRename(f.id));
                }
                resp.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        out.push(TreeAction::StartRename(f.id));
                    }
                    if ui.button("Delete").clicked() {
                        out.push(TreeAction::Delete(f.id));
                    }
                });
                rename_overlay(ui, state, f.id, &resp, out);
            }
        }

        // Variables.
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Variables").color(tk.text_dim).small());
            if ui.small_button("Add").on_hover_text("New global variable").clicked() {
                out.push(TreeAction::NewVariable);
            }
        });
        if lib.variables.is_empty() {
            ui.label(RichText::new("No global variables.").color(tk.text_dim).small());
        }
        for (i, v) in lib.variables.iter().enumerate() {
            if !matches_filter(&v.name, &filter) {
                continue;
            }
            let selected = state.ui.triggers.selected_var == Some(i);
            let ty = match v.array_len {
                Some(n) => format!("{}[{n}]", v.ty.label()),
                None => v.ty.label(),
            };
            let resp = tree_row(ui, &tk, &v.name, Some(&ty), 0, selected, None, true);
            if resp.clicked() {
                out.push(TreeAction::SelectVar(i));
            }
            resp.context_menu(|ui| {
                if ui.button("Duplicate").clicked() {
                    out.push(TreeAction::DuplicateVar(i));
                }
                if ui.button("Delete").clicked() {
                    out.push(TreeAction::DeleteVar(i));
                }
            });
        }
    });

    // Drag and drop between rows.
    let ctx = ui.ctx().clone();
    if let Some(drag) = state.ui.triggers.dragging {
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        let mut target: Option<(Option<TriggerId>, usize, Rect, bool)> = None;
        if let Some(p) = pointer {
            if let Some(row) = rows.iter().find(|r| r.rect.y_range().contains(p.y) && r.rect.x_range().contains(p.x)) {
                let rel = (p.y - row.rect.top()) / row.rect.height();
                target = if row.folder && (0.3..0.7).contains(&rel) {
                    Some((Some(row.id), usize::MAX, row.rect, true))
                } else if rel < 0.5 {
                    Some((row.parent, row.index, row.rect, false))
                } else {
                    Some((row.parent, row.index + 1, row.rect, false))
                };
                if let Some((f, _, _, _)) = &target {
                    if f.is_some_and(|f| state.doc.triggers.is_within(f, drag)) {
                        target = None;
                    }
                }
            }
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("trigger-drag")));
            if let Some(item) = state.doc.triggers.find(drag) {
                painter.text(p + Vec2::new(12.0, 0.0), Align2::LEFT_CENTER, item.name(), FontId::proportional(14.0), tk.text);
            }
            if let Some((_, idx, rect, into)) = &target {
                if *into {
                    painter.rect_stroke(*rect, 4.0, Stroke::new(1.0, tk.accent), egui::StrokeKind::Inside);
                } else {
                    let y = if rows.iter().any(|r| r.rect == *rect && r.index == *idx) { rect.top() } else { rect.bottom() };
                    painter.line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], Stroke::new(2.0, tk.accent));
                }
            }
        }
        if ctx.input(|i| i.pointer.any_released()) {
            if let Some((folder, idx, _, _)) = target {
                out.push(TreeAction::Drop(drag, folder, idx));
            }
            state.ui.triggers.dragging = None;
        }
    }

    // Keyboard.
    let st = &state.ui.triggers;
    if st.focus == Focus::Tree && !ctx.egui_wants_keyboard_input() && st.rename.is_none() {
        let sel = state.selection.trigger;
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) || i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                let down = i.key_down(Key::ArrowDown);
                let ids: Vec<TriggerId> = rows.iter().map(|r| r.id).collect();
                let cur = sel.and_then(|s| ids.iter().position(|x| *x == s));
                let next = match cur {
                    Some(c) if down => (c + 1).min(ids.len().saturating_sub(1)),
                    Some(c) => c.saturating_sub(1),
                    None => 0,
                };
                if let Some(id) = ids.get(next) {
                    out.push(TreeAction::Select(*id));
                }
            }
            if let Some(id) = sel {
                if i.consume_key(Modifiers::NONE, Key::F2) {
                    out.push(TreeAction::StartRename(id));
                }
                if i.consume_key(Modifiers::NONE, Key::Delete) {
                    out.push(TreeAction::Delete(id));
                }
                if i.consume_key(Modifiers::COMMAND, Key::D) {
                    out.push(TreeAction::Duplicate(id));
                }
                if i.consume_key(Modifiers::ALT, Key::ArrowUp) {
                    out.push(TreeAction::Move(id, -1));
                }
                if i.consume_key(Modifiers::ALT, Key::ArrowDown) {
                    out.push(TreeAction::Move(id, 1));
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowLeft) || i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                    out.push(TreeAction::ToggleFolder(id));
                }
                if i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                    out.push(TreeAction::Copy(id));
                }
            }
            if i.consume_key(Modifiers::COMMAND, Key::F) {
                state.ui.triggers.focus_filter = true;
            }
        });
        let pasted = ctx.input(|i| i.events.iter().find_map(|e| if let egui::Event::Paste(s) = e { Some(s.clone()) } else { None }));
        if let Some(s) = pasted {
            if matches!(parse_clip(&s), Some(Clip::Items(_))) {
                state.ui.triggers.rename = None;
                out.push(TreeAction::Paste);
                ctx.data_mut(|d| d.insert_temp(Id::new("trigger-paste"), s));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn tree_items(
    ui: &mut Ui,
    tk: &Tokens,
    state: &EditorState,
    items: &[TriggerItem],
    depth: usize,
    parent: Option<TriggerId>,
    filter: &str,
    rows: &mut Vec<Row>,
    out: &mut Vec<TreeAction>,
) {
    for (index, item) in items.iter().enumerate() {
        if !subtree_matches(item, filter) {
            continue;
        }
        let id = item.id();
        let selected = state.selection.trigger == Some(id) && state.ui.triggers.selected_var.is_none();
        match item {
            TriggerItem::Folder { name, children, .. } => {
                let open = !state.ui.triggers.collapsed_tree.contains(&id) || !filter.trim().is_empty();
                let resp = tree_row(ui, tk, name, None, depth, selected, Some(open), true);
                handle_row(ui, state, id, &resp, depth, true, out);
                rows.push(Row { id, rect: resp.rect, folder: true, parent, index });
                if open {
                    tree_items(ui, tk, state, children, depth + 1, Some(id), filter, rows, out);
                    if children.is_empty() {
                        ui.horizontal(|ui| {
                            ui.add_space(16.0 + (depth + 1) as f32 * 14.0);
                            ui.label(RichText::new("Empty folder").color(tk.text_dim).small());
                        });
                    }
                }
            }
            TriggerItem::Trigger(t) => {
                let resp = tree_row(ui, tk, &t.name, None, depth, selected, None, t.enabled);
                // Enabled checkbox drawn at the row start.
                let cb = Rect::from_min_size(resp.rect.left_center() + Vec2::new(4.0 + depth as f32 * 14.0, -6.0), Vec2::splat(12.0));
                let cb_resp = ui.interact(cb.expand(2.0), Id::new(("trigger-enabled", id)), Sense::click());
                let painter = ui.painter();
                painter.rect_stroke(
                    cb,
                    2.0,
                    Stroke::new(1.0, if cb_resp.hovered() { tk.accent } else { tk.text_dim }),
                    egui::StrokeKind::Inside,
                );
                if t.enabled {
                    painter.line(
                        vec![
                            cb.left_center() + Vec2::new(2.5, 0.0),
                            cb.center_bottom() + Vec2::new(-1.0, -3.0),
                            cb.right_top() + Vec2::new(-2.5, 3.0),
                        ],
                        Stroke::new(1.5, tk.text),
                    );
                }
                let cb_resp = cb_resp.on_hover_text(if t.enabled { "Enabled. Click to disable." } else { "Disabled. Click to enable." });
                if cb_resp.clicked() {
                    out.push(TreeAction::SetEnabled(id, !t.enabled));
                }
                handle_row(ui, state, id, &resp, depth, false, out);
                rows.push(Row { id, rect: resp.rect, folder: false, parent, index });
            }
        }
    }
}

/// Paint one tree row and return its response. `open` is `Some` for folders.
#[allow(clippy::too_many_arguments)]
fn tree_row(
    ui: &mut Ui,
    tk: &Tokens,
    name: &str,
    detail: Option<&str>,
    depth: usize,
    selected: bool,
    open: Option<bool>,
    enabled: bool,
) -> egui::Response {
    let h = 22.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click_and_drag());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, 4.0, tk.accent.gamma_multiply(0.18));
        painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, h)), 0.0, tk.accent);
    } else if resp.hovered() {
        painter.rect_filled(rect, 4.0, tk.bg_raised);
    }
    let x = rect.left() + 4.0 + depth as f32 * 14.0;
    if let Some(open) = open {
        let c = egui::pos2(x + 6.0, rect.center().y);
        let pts = if open {
            vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(4.0, -2.0), c + Vec2::new(0.0, 3.0)]
        } else {
            vec![c + Vec2::new(-2.0, -4.0), c + Vec2::new(3.0, 0.0), c + Vec2::new(-2.0, 4.0)]
        };
        painter.add(egui::Shape::convex_polygon(pts, tk.text_dim, Stroke::NONE));
    }
    let color = if enabled { tk.text } else { tk.text_dim };
    let text_x = x + 18.0;
    let galley = painter.text(egui::pos2(text_x, rect.center().y), Align2::LEFT_CENTER, name, FontId::proportional(14.0), color);
    if let Some(d) = detail {
        painter.text(egui::pos2(galley.right() + 8.0, rect.center().y), Align2::LEFT_CENTER, d, FontId::proportional(12.0), tk.text_dim);
    }
    resp
}

fn handle_row(
    ui: &mut Ui,
    state: &EditorState,
    id: TriggerId,
    resp: &egui::Response,
    depth: usize,
    folder: bool,
    out: &mut Vec<TreeAction>,
) {
    let arrow_hit = folder && resp.interact_pointer_pos().is_some_and(|p| p.x < resp.rect.left() + 18.0 + depth as f32 * 14.0);
    if resp.clicked() {
        if arrow_hit {
            out.push(TreeAction::ToggleFolder(id));
        } else {
            out.push(TreeAction::Select(id));
        }
    }
    if resp.double_clicked() && !arrow_hit {
        out.push(TreeAction::StartRename(id));
    }
    if resp.drag_started() && state.ui.triggers.rename.is_none() {
        out.push(TreeAction::Select(id));
        ui.ctx().data_mut(|d| d.insert_temp(Id::new("trigger-drag-start"), id));
    }
    resp.context_menu(|ui| {
        if ui.button("New trigger").clicked() {
            out.push(TreeAction::Select(id));
            out.push(TreeAction::NewTrigger);
        }
        if ui.button("New folder").clicked() {
            out.push(TreeAction::Select(id));
            out.push(TreeAction::NewFolder);
        }
        ui.separator();
        if ui.button("Rename").clicked() {
            out.push(TreeAction::StartRename(id));
        }
        if ui.button("Duplicate").clicked() {
            out.push(TreeAction::Duplicate(id));
        }
        if ui.button("Copy").clicked() {
            out.push(TreeAction::Copy(id));
        }
        if ui.button("Move up").clicked() {
            out.push(TreeAction::Move(id, -1));
        }
        if ui.button("Move down").clicked() {
            out.push(TreeAction::Move(id, 1));
        }
        ui.separator();
        if ui.button("Delete").clicked() {
            out.push(TreeAction::Delete(id));
        }
    });
    rename_overlay(ui, state, id, resp, out);
}

/// Inline rename text box over a tree row.
fn rename_overlay(ui: &mut Ui, state: &EditorState, id: TriggerId, resp: &egui::Response, out: &mut Vec<TreeAction>) {
    let Some((rid, buf, fresh)) = &state.ui.triggers.rename else { return };
    if *rid != id {
        return;
    }
    let mut text = buf.clone();
    let rect = Rect::from_min_max(resp.rect.min + Vec2::new(20.0, 1.0), resp.rect.max - Vec2::new(2.0, 1.0));
    let edit_id = Id::new(("trigger-rename", id));
    let r = ui.put(rect, egui::TextEdit::singleline(&mut text).id(edit_id));
    if *fresh {
        r.request_focus();
        if let Some(mut s) = egui::TextEdit::load_state(ui.ctx(), edit_id) {
            s.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(text.chars().count()),
            )));
            s.store(ui.ctx(), edit_id);
        }
    }
    // Read input before taking the data lock: nesting context locks deadlocks
    // natively and panics on wasm.
    let escape = ui.input(|i| i.key_pressed(Key::Escape));
    let lost = r.lost_focus();
    ui.ctx().data_mut(|d| d.insert_temp(Id::new("trigger-rename-buf"), (id, text.clone(), lost, escape)));
    if lost {
        if escape {
            out.push(TreeAction::Rename(id, String::new()));
        } else {
            out.push(TreeAction::Rename(id, text));
        }
    }
}

fn apply_tree_actions(ctx: &egui::Context, state: &mut EditorState, actions: Vec<TreeAction>) {
    // Keep the rename buffer in sync with the overlay.
    if let Some((id, text, _, _)) = ctx.data(|d| d.get_temp::<(TriggerId, String, bool, bool)>(Id::new("trigger-rename-buf"))) {
        if let Some((rid, buf, fresh)) = &mut state.ui.triggers.rename {
            if *rid == id {
                *buf = text;
                *fresh = false;
            }
        }
        ctx.data_mut(|d| d.remove::<(TriggerId, String, bool, bool)>(Id::new("trigger-rename-buf")));
    }
    let drag_key = Id::new("trigger-drag-start");
    if let Some(id) = ctx.data_mut(|d| {
        let v = d.get_temp::<TriggerId>(drag_key);
        d.remove::<TriggerId>(drag_key);
        v
    }) {
        state.ui.triggers.dragging = Some(id);
    }
    for a in actions {
        let mut lib = state.doc.triggers.clone();
        let name_of = |lib: &TriggerLibrary, id: TriggerId| {
            lib.find(id).map(|i| i.name().to_owned()).or_else(|| lib.function(id).map(|f| f.name.clone())).unwrap_or_default()
        };
        // Insertion point for new items: after the selection, or inside a selected folder.
        let place = |lib: &TriggerLibrary, sel: Option<TriggerId>| -> (Option<TriggerId>, usize) {
            match sel.and_then(|s| lib.find(s).map(|i| (s, i))) {
                Some((s, TriggerItem::Folder { children, .. })) => (Some(s), children.len()),
                Some((s, _)) => lib.locate(s).map(|(f, i)| (f, i + 1)).unwrap_or((None, lib.items.len())),
                None => (None, lib.items.len()),
            }
        };
        match a {
            TreeAction::Select(id) => {
                state.selection.trigger = Some(id);
                state.ui.triggers.selected_var = None;
                state.ui.triggers.focus = Focus::Tree;
            }
            TreeAction::SelectVar(i) => {
                state.ui.triggers.selected_var = Some(i);
                state.ui.triggers.focus = Focus::Tree;
            }
            TreeAction::StartRename(id) => {
                let name = name_of(&lib, id);
                state.ui.triggers.rename = Some((id, name, true));
            }
            TreeAction::Rename(id, name) => {
                state.ui.triggers.rename = None;
                let name = name.trim().to_owned();
                if name.is_empty() || name == name_of(&lib, id) {
                    continue;
                }
                if let Some(item) = lib.find_mut(id) {
                    item.set_name(name.clone());
                } else if let Some(f) = lib.function_mut(id) {
                    f.name = name.clone();
                }
                commit(state, format!("Rename to {name}"), lib, None, None);
            }
            TreeAction::SetEnabled(id, on) => {
                if let Some(t) = lib.trigger_mut(id) {
                    t.enabled = on;
                    let label = format!("{} trigger {}", if on { "Enable" } else { "Disable" }, t.name);
                    commit(state, label, lib, None, None);
                }
            }
            TreeAction::NewTrigger | TreeAction::NewFolder => {
                let folder = matches!(a, TreeAction::NewFolder);
                let id = lib.alloc_id();
                let name = lib.unique_name(if folder { "New folder" } else { "New trigger" });
                let item = if folder {
                    TriggerItem::Folder { id, name: name.clone(), children: vec![] }
                } else {
                    TriggerItem::Trigger(Trigger::new(id, name.clone()))
                };
                let (f, i) = place(&lib, state.selection.trigger);
                lib.insert(f, i, item);
                commit(state, format!("Create {name}"), lib, None, None);
                state.selection.trigger = Some(id);
                state.ui.triggers.selected_var = None;
                state.ui.triggers.rename = Some((id, name, true));
            }
            TreeAction::NewFunction => {
                let id = lib.alloc_id();
                let name = lib.unique_name("New function");
                lib.functions.push(CustomFunction::new(id, name.clone()));
                commit(state, format!("Create function {name}"), lib, None, None);
                state.selection.trigger = Some(id);
                state.ui.triggers.selected_var = None;
                state.ui.triggers.rename = Some((id, name, true));
            }
            TreeAction::NewVariable => {
                let name = lib.unique_variable_name("Variable");
                lib.variables.push(Variable::new(name.clone(), GalaxyType::Int));
                let i = lib.variables.len() - 1;
                commit(state, format!("Create variable {name}"), lib, None, None);
                state.ui.triggers.selected_var = Some(i);
            }
            TreeAction::Duplicate(id) => {
                if lib.find(id).is_some() {
                    if let Some(new) = lib.duplicate(id) {
                        let label = format!("Duplicate {}", name_of(&lib, id));
                        commit(state, label, lib, None, None);
                        state.selection.trigger = Some(new);
                    }
                } else if let Some(f) = lib.function(id).cloned() {
                    let mut copy = f.clone();
                    copy.id = lib.alloc_id();
                    copy.name = lib.unique_name(&format!("{} copy", f.name));
                    let new = copy.id;
                    lib.functions.push(copy);
                    commit(state, format!("Duplicate {}", f.name), lib, None, None);
                    state.selection.trigger = Some(new);
                }
            }
            TreeAction::Delete(id) => {
                let name = name_of(&lib, id);
                if lib.remove(id).is_none() {
                    lib.functions.retain(|f| f.id != id);
                }
                commit(state, format!("Delete {name}"), lib, None, None);
                if state.selection.trigger == Some(id) {
                    state.selection.trigger = None;
                }
            }
            TreeAction::Move(id, d) => {
                if lib.move_by(id, d) {
                    commit(state, format!("Move {}", name_of(&lib, id)), lib, None, None);
                }
            }
            TreeAction::Drop(id, folder, index) => {
                let index = if index == usize::MAX {
                    match folder.and_then(|f| lib.find(f)) {
                        Some(TriggerItem::Folder { children, .. }) => children.len(),
                        _ => 0,
                    }
                } else {
                    index
                };
                if lib.locate(id).is_some() && lib.move_to(id, folder, index) {
                    commit(state, format!("Move {}", name_of(&lib, id)), lib, None, None);
                }
            }
            TreeAction::DeleteVar(i) => {
                if i < lib.variables.len() {
                    let v = lib.variables.remove(i);
                    commit(state, format!("Delete variable {}", v.name), lib, None, None);
                    state.ui.triggers.selected_var = None;
                }
            }
            TreeAction::DuplicateVar(i) => {
                if let Some(v) = lib.variables.get(i).cloned() {
                    let mut copy = v.clone();
                    copy.name = lib.unique_variable_name(&format!("{} copy", v.name));
                    lib.variables.insert(i + 1, copy);
                    commit(state, format!("Duplicate variable {}", v.name), lib, None, None);
                    state.ui.triggers.selected_var = Some(i + 1);
                }
            }
            TreeAction::Copy(id) => {
                if let Some(item) = lib.find(id) {
                    ctx.copy_text(clip_text(Clip::Items(vec![item.clone()])));
                    state.toast(ToastKind::Info, format!("Copied {}", item.name()));
                }
            }
            TreeAction::Paste => {
                let text: Option<String> = ctx.data_mut(|d| d.remove_temp(Id::new("trigger-paste")));
                let Some(Clip::Items(items)) = text.as_deref().and_then(parse_clip) else { continue };
                let (mut f, mut i) = place(&lib, state.selection.trigger);
                let mut last = None;
                for mut item in items {
                    reid(&mut lib, &mut item);
                    let name = lib.unique_name(item.name());
                    item.set_name(name);
                    last = Some(item.id());
                    lib.insert(f, i, item);
                    let p = lib.locate(last.expect("set")).unwrap_or((None, 0));
                    f = p.0;
                    i = p.1 + 1;
                }
                commit(state, "Paste triggers".into(), lib, None, None);
                state.selection.trigger = last;
            }
            TreeAction::ToggleFolder(id) => {
                if matches!(lib.find(id), Some(TriggerItem::Folder { .. })) {
                    let c = &mut state.ui.triggers.collapsed_tree;
                    if !c.remove(&id) {
                        c.insert(id);
                    }
                }
            }
        }
    }
}

fn reid(lib: &mut TriggerLibrary, item: &mut TriggerItem) {
    match item {
        TriggerItem::Folder { id, children, .. } => {
            *id = lib.alloc_id();
            for c in children {
                reid(lib, c);
            }
        }
        TriggerItem::Trigger(t) => t.id = lib.alloc_id(),
    }
}

// ---------------------------------------------------------------------------
// Item editor
// ---------------------------------------------------------------------------

#[derive(Clone, PartialEq)]
enum Work {
    Trigger(Trigger),
    Function(CustomFunction),
}

impl Work {
    fn name(&self) -> &str {
        match self {
            Work::Trigger(t) => &t.name,
            Work::Function(f) => &f.name,
        }
    }
    fn locals(&mut self) -> &mut Vec<Variable> {
        match self {
            Work::Trigger(t) => &mut t.locals,
            Work::Function(f) => &mut f.locals,
        }
    }
    fn actions(&mut self) -> &mut Vec<Stmt> {
        match self {
            Work::Trigger(t) => &mut t.actions,
            Work::Function(f) => &mut f.actions,
        }
    }
    fn put(self, lib: &mut TriggerLibrary) {
        match self {
            Work::Trigger(t) => {
                if let Some(slot) = lib.trigger_mut(t.id) {
                    *slot = t;
                }
            }
            Work::Function(f) => {
                if let Some(slot) = lib.function_mut(f.id) {
                    *slot = f;
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum LineOp {
    Delete,
    Duplicate,
    MoveUp,
    MoveDown,
    ToggleDisabled,
    Copy,
}

/// Variable visible in the current scope.
#[derive(Clone)]
struct VarInfo {
    name: String,
    ty: String,
    array: bool,
    scope: &'static str,
}

/// Render context for one item editor frame.
struct Cx<'a> {
    ctx: egui::Context,
    tk: Tokens,
    doc: &'a MapDocument,
    item: TriggerId,
    vars: Vec<VarInfo>,
    is_function: bool,
    returns: Option<GalaxyType>,
    cursor: Option<LineRef>,
    collapsed: &'a mut HashSet<(TriggerId, Vec<usize>)>,
    lines: Vec<LineRef>,
    ops: Vec<(LineRef, LineOp)>,
    open_picker: Option<(Purpose, InsertAt, egui::Pos2)>,
    new_locals: Vec<Variable>,
    texts: Vec<(String, String)>,
    coalesce: Option<Id>,
    label: Option<String>,
    clicked_line: Option<LineRef>,
    cursor_rect: Option<Rect>,
}

impl Cx<'_> {
    fn text_value(&self, key: &str) -> Option<String> {
        if let Some((_, v)) = self.texts.iter().rev().find(|(k, _)| k == key) {
            return Some(v.clone());
        }
        self.doc.strings.get(DEFAULT_LOCALE, key).map(str::to_owned)
    }

    /// A string key no other text uses.
    fn new_text_key(&self) -> String {
        let existing = self.doc.strings.all_keys();
        (1u32..)
            .map(|n| format!("Param/Value/S{n:06}"))
            .find(|k| !existing.contains(k) && !self.texts.iter().any(|(t, _)| t == k))
            .expect("unbounded")
    }

    fn var(&self, name: &str) -> Option<&VarInfo> {
        self.vars.iter().find(|v| v.name == name)
    }
}

/// What a parameter slot accepts.
#[derive(Clone)]
struct Slot {
    label: String,
    ty: String,
    editor: String,
    link: String,
    presets: &'static [(&'static str, &'static str)],
    bits: bool,
    alts: &'static str,
}

impl Slot {
    fn of(ty: &str, label: &str) -> Slot {
        Slot { label: label.into(), ty: ty.into(), editor: ty.into(), link: String::new(), presets: &[], bits: false, alts: "" }
    }
    fn param(p: &ParamSpec) -> Slot {
        Slot {
            label: p.label.into(),
            ty: p.ty.into(),
            editor: p.editor.into(),
            link: p.link.into(),
            presets: p.presets,
            bits: p.bits,
            alts: p.alts,
        }
    }
    fn any(label: &str) -> Slot {
        Slot::of("", label)
    }
}

fn type_ok(want: &str, have: &str) -> bool {
    want.is_empty() || want == have || (want == "unitref" && have == "unit") || (want == "fixed" && have == "int")
}

fn infer(cx: &Cx, e: &Expr) -> Option<String> {
    match e {
        Expr::Literal(Literal::Int(_)) => Some("int".into()),
        Expr::Literal(Literal::Fixed(_)) => Some("fixed".into()),
        Expr::Literal(Literal::Bool(_)) => Some("bool".into()),
        Expr::Literal(Literal::String(_)) => Some("string".into()),
        Expr::Literal(Literal::Text(_)) => Some("text".into()),
        Expr::Variable(v) | Expr::Element { variable: v, .. } => cx.var(v).map(|v| v.ty.clone()),
        Expr::Call(c) => match trigger_lib::get(&c.function) {
            Some(f) => (!f.ret.is_empty()).then(|| f.ret.to_owned()),
            None => cx.doc.triggers.function_named(&c.function).and_then(|f| f.returns.as_ref()).map(|t| t.galaxy_name().to_owned()),
        },
        Expr::Binary { op, lhs, rhs } => match op.as_str() {
            "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||" => Some("bool".into()),
            _ => infer(cx, lhs).or_else(|| infer(cx, rhs)),
        },
        Expr::Not(_) => Some("bool".into()),
        Expr::Trigger(_) => Some("trigger".into()),
        _ => None,
    }
}

fn editor_ui(ui: &mut Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    if let Some(i) = state.ui.triggers.selected_var {
        variable_editor(ui, state, i);
        return;
    }
    let Some(id) = state.selection.trigger else {
        empty_state(ui, &tk, "Select a trigger on the left, or create one with New trigger.");
        return;
    };
    let lib = &state.doc.triggers;
    let original = match (lib.find(id), lib.function(id)) {
        (Some(TriggerItem::Trigger(t)), _) => Work::Trigger(t.clone()),
        (Some(TriggerItem::Folder { name, children, .. }), _) => {
            let n = children.len();
            let name = name.clone();
            ui.heading(&name);
            ui.label(RichText::new(format!("Folder with {n} items. Drag triggers onto it to organize them.")).color(tk.text_dim));
            return;
        }
        (None, Some(f)) => Work::Function(f.clone()),
        _ => return,
    };
    let mut work = original.clone();

    // Checks for this item, cached by revision.
    let rev = state.history.revision();
    if state.ui.triggers.checks.0 != rev {
        state.ui.triggers.checks = (rev, strata_core::triggers::check(&state.doc.triggers));
    }
    let problems: Vec<String> = state.ui.triggers.checks.1.iter().filter(|(n, _)| n == work.name()).map(|(_, m)| m.clone()).collect();

    let mut vars: Vec<VarInfo> = Vec::new();
    let (is_function, returns) = match &work {
        Work::Trigger(_) => (false, None),
        Work::Function(f) => {
            for p in &f.params {
                vars.push(VarInfo {
                    name: p.name.clone(),
                    ty: p.ty.galaxy_name().into(),
                    array: p.array_len.is_some(),
                    scope: "parameter",
                });
            }
            (true, f.returns.clone())
        }
    };
    for v in work.locals().iter() {
        vars.push(VarInfo { name: v.name.clone(), ty: v.ty.galaxy_name().into(), array: v.array_len.is_some(), scope: "local" });
    }
    for v in &state.doc.triggers.variables {
        vars.push(VarInfo { name: v.name.clone(), ty: v.ty.galaxy_name().into(), array: v.array_len.is_some(), scope: "global" });
    }

    let cursor = state.ui.triggers.cursor.as_ref().filter(|(i, _)| *i == id).map(|(_, l)| l.clone());
    let mut reveal = false;
    let ctx = ui.ctx().clone();
    let (ops, open_picker, new_locals, texts, coalesce, label, lines, clicked_line, cursor_rect) = {
        let EditorState { doc, ui: panels, .. } = &mut *state;
        let mut cx = Cx {
            ctx: ctx.clone(),
            tk,
            doc,
            item: id,
            vars,
            is_function,
            returns,
            cursor,
            collapsed: &mut panels.triggers.collapsed,
            lines: Vec::new(),
            ops: Vec::new(),
            open_picker: None,
            new_locals: Vec::new(),
            texts: Vec::new(),
            coalesce: None,
            label: None,
            clicked_line: None,
            cursor_rect: None,
        };
        let scroll_to_cursor = std::mem::take(&mut panels.triggers.scroll_to_cursor);
        egui::ScrollArea::vertical().id_salt(("trigger-editor", id)).auto_shrink([false, false]).show(ui, |ui| {
            reveal = header_ui(ui, &mut cx, &mut work, &problems);
            ui.add_space(6.0);
            body_ui(ui, &mut cx, &mut work);
            ui.add_space(40.0);
            if scroll_to_cursor {
                if let Some(r) = cx.cursor_rect {
                    ui.scroll_to_rect(r, None);
                }
            }
        });
        (cx.ops, cx.open_picker, cx.new_locals, cx.texts, cx.coalesce, cx.label, cx.lines, cx.clicked_line, cx.cursor_rect)
    };
    let _ = cursor_rect;
    if let Some(l) = clicked_line {
        state.ui.triggers.cursor = Some((id, l));
        state.ui.triggers.focus = Focus::Editor;
    }
    work.locals().extend(new_locals);

    // Keyboard on the editor.
    let mut ops = ops;
    let st = &mut state.ui.triggers;
    let mut paste: Option<Clip> = None;
    if st.focus == Focus::Editor && st.picker.is_none() && !ctx.egui_wants_keyboard_input() {
        let cur = st.cursor.as_ref().filter(|(i, _)| *i == id).map(|(_, l)| l.clone());
        let mut open: Option<Section> = None;
        ctx.input_mut(|i| {
            if let Some(c) = &cur {
                if i.consume_key(Modifiers::NONE, Key::Delete) || i.consume_key(Modifiers::NONE, Key::Backspace) {
                    ops.push((c.clone(), LineOp::Delete));
                }
                if i.consume_key(Modifiers::COMMAND, Key::D) {
                    ops.push((c.clone(), LineOp::Duplicate));
                }
                if i.consume_key(Modifiers::ALT, Key::ArrowUp) {
                    ops.push((c.clone(), LineOp::MoveUp));
                }
                if i.consume_key(Modifiers::ALT, Key::ArrowDown) {
                    ops.push((c.clone(), LineOp::MoveDown));
                }
                if i.consume_key(Modifiers::COMMAND, Key::Slash) {
                    ops.push((c.clone(), LineOp::ToggleDisabled));
                }
                if i.events.iter().any(|e| matches!(e, egui::Event::Copy)) {
                    ops.push((c.clone(), LineOp::Copy));
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowLeft) || i.consume_key(Modifiers::NONE, Key::ArrowRight) {
                    if let LineRef::Action(p) = c {
                        let key = (id, p.clone());
                        if !st.collapsed.remove(&key) {
                            st.collapsed.insert(key);
                        }
                    }
                }
            }
            let up = i.consume_key(Modifiers::NONE, Key::ArrowUp);
            let down = i.consume_key(Modifiers::NONE, Key::ArrowDown);
            if up || down {
                let pos = cur.as_ref().and_then(|c| lines.iter().position(|l| l == c));
                let next = match pos {
                    Some(p) if down => (p + 1).min(lines.len().saturating_sub(1)),
                    Some(p) => p.saturating_sub(1),
                    None => 0,
                };
                if let Some(l) = lines.get(next) {
                    st.cursor = Some((id, l.clone()));
                    st.scroll_to_cursor = true;
                }
            }
            if i.consume_key(Modifiers::COMMAND, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Insert) {
                open = Some(cur.as_ref().map(LineRef::section).unwrap_or(Section::Actions));
            }
            for e in &i.events {
                if let egui::Event::Paste(s) = e {
                    paste = parse_clip(s);
                }
            }
        });
        if let Some(section) = open {
            let pos = ctx.input(|i| i.pointer.hover_pos()).unwrap_or(ui.max_rect().center());
            let at = insert_point(&work, cur.as_ref(), section);
            if let Some((purpose, at)) = at {
                st.picker = Some(Picker { purpose, at, item: id, pos, fresh: true, rect: None });
            } else if section == Section::Locals {
                ops.push((LineRef::Local(usize::MAX), LineOp::Duplicate));
            }
        }
    }
    if let Some((purpose, at, pos)) = open_picker {
        st.picker = Some(Picker { purpose, at, item: id, pos, fresh: true, rect: None });
    }

    // Line operations.
    let mut label = label;
    let mut new_cursor = None;
    for (line, op) in ops {
        if op == LineOp::Copy {
            if let Some(clip) = copy_line(&mut work, &line) {
                ctx.copy_text(clip_text(clip));
                state.toast(ToastKind::Info, "Copied line");
            }
            continue;
        }
        if let LineRef::Local(usize::MAX) = line {
            let n = work.locals().len() + 1;
            work.locals().push(Variable::new(format!("Local {n}"), GalaxyType::Int));
            label = Some("Add local variable".into());
            continue;
        }
        if let Some(c) = line_op(&mut work, &line, op) {
            new_cursor = Some(c);
        }
        label = Some(
            match op {
                LineOp::Delete => "Delete line",
                LineOp::Duplicate => "Duplicate line",
                LineOp::MoveUp | LineOp::MoveDown => "Move line",
                LineOp::ToggleDisabled => "Toggle action",
                LineOp::Copy => unreachable!(),
            }
            .into(),
        );
    }
    if let Some(c) = new_cursor {
        state.ui.triggers.cursor = Some((id, c));
        state.ui.triggers.scroll_to_cursor = true;
    }
    if let Some(clip) = paste {
        let cur = state.ui.triggers.cursor.as_ref().filter(|(i, _)| *i == id).map(|(_, l)| l.clone());
        if let Some(c) = paste_clip(&mut work, cur.as_ref(), clip) {
            state.ui.triggers.cursor = Some((id, c));
            state.ui.triggers.scroll_to_cursor = true;
            label = Some("Paste".into());
        }
    }

    // Library picker.
    if let Some(choice) = picker_ui(&ctx, state, id) {
        let (at, choice) = choice;
        if let Some(c) = insert_choice(&mut work, &at, &choice, &state.doc.triggers) {
            state.ui.triggers.cursor = Some((id, c));
            state.ui.triggers.focus = Focus::Editor;
            state.ui.triggers.scroll_to_cursor = true;
            label = Some(format!("Add {}", choice_label(&choice, &state.doc.triggers)));
        }
    }

    if state.ui.triggers.scroll_to_cursor {
        ctx.request_repaint();
    }
    if reveal {
        state.ui.script.reveal(id);
        state.request(AppAction::FocusTab(Tab::Script));
    }

    if work != original || !texts.is_empty() {
        let mut lib = state.doc.triggers.clone();
        let name = work.name().to_owned();
        work.put(&mut lib);
        let strings = (!texts.is_empty()).then(|| {
            let mut s = state.doc.strings.clone();
            for (k, v) in &texts {
                s.set(DEFAULT_LOCALE, k, v);
            }
            s
        });
        let label = label.unwrap_or_else(|| format!("Edit {name}"));
        commit(state, label, lib, strings, coalesce);
    }
}

fn empty_state(ui: &mut Ui, tk: &Tokens, text: &str) {
    ui.add_space(24.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(text).color(tk.text_dim));
    });
}

/// Name, flags and comment. Returns true if "Show script" was clicked.
fn header_ui(ui: &mut Ui, cx: &mut Cx, work: &mut Work, problems: &[String]) -> bool {
    let tk = cx.tk;
    let mut reveal = false;
    ui.horizontal(|ui| {
        let (name, id_salt) = match work {
            Work::Trigger(t) => (&mut t.name, "name"),
            Work::Function(f) => (&mut f.name, "fname"),
        };
        let r =
            ui.add(egui::TextEdit::singleline(name).font(FontId::proportional(16.0)).desired_width(260.0).id(Id::new((id_salt, cx.item))));
        if r.changed() {
            cx.coalesce = Some(r.id);
            cx.label = Some("Rename".into());
        }
        match work {
            Work::Trigger(t) => {
                if ui.checkbox(&mut t.enabled, "Enabled").on_hover_text("Disabled triggers are left out of the script").changed() {
                    cx.label = Some(if t.enabled { "Enable trigger" } else { "Disable trigger" }.into());
                }
                if ui
                    .checkbox(&mut t.initially_on, "Initially on")
                    .on_hover_text("Off triggers can be turned on by other triggers")
                    .changed()
                {
                    cx.label = Some("Toggle initially on".into());
                }
            }
            Work::Function(f) => {
                let mut kind = f.returns.clone();
                let text = match &kind {
                    None => "Action (no return value)".to_owned(),
                    Some(t) => format!("Returns {}", t.label().to_lowercase()),
                };
                egui::ComboBox::from_id_salt(("returns", f.id)).selected_text(text).width(190.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut kind, None, "Action (no return value)");
                    for t in GalaxyType::common() {
                        let l = format!("Returns {}", t.label().to_lowercase());
                        ui.selectable_value(&mut kind, Some(t), l);
                    }
                });
                if kind != f.returns {
                    f.returns = kind;
                    cx.label = Some("Change return type".into());
                }
            }
        }
        if ui.button("Show script").on_hover_text("Show the generated Galaxy for this item").clicked() {
            reveal = true;
        }
    });
    let comment = match work {
        Work::Trigger(t) => &mut t.comment,
        Work::Function(f) => &mut f.comment,
    };
    let r = ui.add(
        egui::TextEdit::multiline(comment)
            .hint_text("Add a comment: what does this do and why?")
            .desired_rows(1)
            .desired_width(f32::INFINITY)
            .id(Id::new(("comment", cx.item))),
    );
    if r.changed() {
        cx.coalesce = Some(r.id);
        cx.label = Some("Edit comment".into());
    }
    for p in problems {
        ui.label(RichText::new(p).color(tk.warn).small());
    }
    reveal
}

fn section_header(ui: &mut Ui, cx: &mut Cx, title: &str, count: usize, add: Option<(Purpose, InsertAt)>, add_local: bool) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(15.0).strong());
        if count > 0 {
            ui.label(RichText::new(count.to_string()).color(cx.tk.text_dim).small());
        }
        let r = ui.small_button("Add").on_hover_text("Add a line (Ctrl+Enter)");
        if r.clicked() {
            if let Some((p, at)) = add {
                cx.open_picker = Some((p, at, r.rect.left_bottom()));
            } else if add_local {
                cx.ops.push((LineRef::Local(usize::MAX), LineOp::Duplicate));
            }
        }
    });
    let r = ui.max_rect();
    ui.painter()
        .line_segment([egui::pos2(r.left(), ui.cursor().top()), egui::pos2(r.right(), ui.cursor().top())], Stroke::new(1.0, cx.tk.stroke));
    ui.add_space(2.0);
}

fn body_ui(ui: &mut Ui, cx: &mut Cx, work: &mut Work) {
    let tk = cx.tk;
    let item = cx.item;
    match work {
        Work::Trigger(t) => {
            section_header(ui, cx, "Events", t.events.len(), Some((Purpose::Event, InsertAt::Event(t.events.len()))), false);
            if t.events.is_empty() {
                hint(ui, &tk, "No events. This trigger only runs when another trigger runs it.");
            }
            for (i, e) in t.events.iter_mut().enumerate() {
                line_frame(ui, cx, LineRef::Event(i), 0, |ui, cx| {
                    call_ui(ui, cx, e, Id::new((item, "ev", i)), false, true);
                });
            }
            section_header(
                ui,
                cx,
                "Conditions",
                t.conditions.len(),
                Some((Purpose::Condition, InsertAt::Condition(t.conditions.len()))),
                false,
            );
            if t.conditions.is_empty() {
                hint(ui, &tk, "No conditions. The actions always run.");
            }
            for (i, c) in t.conditions.iter_mut().enumerate() {
                line_frame(ui, cx, LineRef::Condition(i), 0, |ui, cx| {
                    slot_ui(ui, cx, c, &Slot::of("bool", "Condition"), Id::new((item, "cond", i)), 0);
                });
            }
        }
        Work::Function(f) => {
            section_header(ui, cx, "Parameters", f.params.len(), None, false);
            if f.params.is_empty() {
                hint(ui, &tk, "No parameters.");
            }
            let mut add = false;
            for (i, p) in f.params.iter_mut().enumerate() {
                line_frame(ui, cx, LineRef::Param(i), 0, |ui, cx| variable_row(ui, cx, p, Id::new((item, "param", i)), false));
            }
            if ui.small_button("Add parameter").clicked() {
                add = true;
            }
            if add {
                let n = f.params.len() + 1;
                f.params.push(Variable::new(format!("Parameter {n}"), GalaxyType::Int));
                cx.label = Some("Add parameter".into());
            }
        }
    }

    let locals = work.locals();
    section_header(ui, cx, "Local variables", locals.len(), None, true);
    if locals.is_empty() {
        hint(ui, &tk, "No local variables.");
    }
    for (i, v) in locals.iter_mut().enumerate() {
        line_frame(ui, cx, LineRef::Local(i), 0, |ui, cx| variable_row(ui, cx, v, Id::new((item, "local", i)), true));
    }

    let actions = work.actions();
    section_header(ui, cx, "Actions", actions.len(), Some((Purpose::Action, InsertAt::Action(vec![], actions.len()))), false);
    if actions.is_empty() {
        hint(ui, &tk, "No actions yet. Press Ctrl+Enter or click Add to add one.");
    }
    actions_ui(ui, cx, actions, &[], 0);
}

fn hint(ui: &mut Ui, tk: &Tokens, text: &str) {
    ui.label(RichText::new(text).color(tk.text_dim).small());
}

/// One selectable editor line. `add` draws the content.
fn line_frame(ui: &mut Ui, cx: &mut Cx, line: LineRef, depth: usize, add: impl FnOnce(&mut Ui, &mut Cx)) {
    let selected = cx.cursor.as_ref() == Some(&line);
    let bg = ui.painter().add(egui::Shape::Noop);
    let r = ui.scope_builder(egui::UiBuilder::new().sense(Sense::click()), |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.add_space(6.0 + depth as f32 * 20.0);
            add(ui, cx);
        });
    });
    let resp = r.response;
    let rect = Rect::from_min_max(egui::pos2(ui.max_rect().left(), resp.rect.top()), egui::pos2(ui.max_rect().right(), resp.rect.bottom()));
    let tk = cx.tk;
    if selected {
        ui.painter().set(bg, egui::Shape::rect_filled(rect, 4.0, tk.accent.gamma_multiply(0.14)));
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())), 0.0, tk.accent);
        cx.cursor_rect = Some(rect);
    } else if resp.hovered() {
        ui.painter().set(bg, egui::Shape::rect_filled(rect, 4.0, tk.bg_raised.gamma_multiply(0.6)));
    }
    if resp.clicked() || resp.secondary_clicked() {
        cx.clicked_line = Some(line.clone());
    }
    resp.context_menu(|ui| {
        let ops: &[(&str, LineOp)] = &[
            ("Copy (Ctrl+C)", LineOp::Copy),
            ("Duplicate (Ctrl+D)", LineOp::Duplicate),
            ("Move up (Alt+Up)", LineOp::MoveUp),
            ("Move down (Alt+Down)", LineOp::MoveDown),
        ];
        for (l, op) in ops {
            if ui.button(*l).clicked() {
                cx.ops.push((line.clone(), *op));
            }
        }
        if matches!(line, LineRef::Action(_)) && ui.button("Disable or enable (Ctrl+/)").clicked() {
            cx.ops.push((line.clone(), LineOp::ToggleDisabled));
        }
        ui.separator();
        if ui.button("Delete (Del)").clicked() {
            cx.ops.push((line.clone(), LineOp::Delete));
        }
    });
    cx.lines.push(line);
}

fn variable_row(ui: &mut Ui, cx: &mut Cx, v: &mut Variable, id: Id, with_initial: bool) {
    let r = ui.add(egui::TextEdit::singleline(&mut v.name).desired_width(140.0).id(id.with("name")));
    if r.changed() {
        cx.coalesce = Some(r.id);
        cx.label = Some("Rename variable".into());
    }
    let mut ty = v.ty.clone();
    egui::ComboBox::from_id_salt(id.with("ty")).selected_text(ty.label()).width(110.0).show_ui(ui, |ui| {
        for t in GalaxyType::common() {
            let l = t.label();
            ui.selectable_value(&mut ty, t, l);
        }
    });
    if ty != v.ty {
        v.ty = ty;
        v.initial = None;
        cx.label = Some("Change variable type".into());
    }
    let mut is_array = v.array_len.is_some();
    if ui.checkbox(&mut is_array, "Array").changed() {
        v.array_len = is_array.then_some(16);
        cx.label = Some("Toggle array".into());
    }
    if let Some(n) = &mut v.array_len {
        let r = ui.add(egui::DragValue::new(n).range(1..=8192).prefix("size "));
        if r.changed() {
            cx.coalesce = Some(r.id);
            cx.label = Some("Change array size".into());
        }
    }
    if with_initial {
        ui.label(RichText::new("initial").color(cx.tk.text_dim));
        let slot = Slot::of(v.ty.galaxy_name(), "Initial value");
        let mut e = v.initial.clone().unwrap_or_else(Expr::unset);
        slot_ui(ui, cx, &mut e, &slot, id.with("init"), 0);
        let new = (!e.is_unset()).then_some(e);
        if new != v.initial {
            v.initial = new;
        }
    }
}

fn variable_editor(ui: &mut Ui, state: &mut EditorState, i: usize) {
    let tk = theme::tokens(ui.ctx());
    let Some(original) = state.doc.triggers.variables.get(i).cloned() else { return };
    let mut v = original.clone();
    ui.heading("Global variable");
    ui.label(RichText::new("Visible to every trigger. Compiled as gv_name.").color(tk.text_dim));
    ui.add_space(8.0);
    let vars = state
        .doc
        .triggers
        .variables
        .iter()
        .map(|v| VarInfo { name: v.name.clone(), ty: v.ty.galaxy_name().into(), array: v.array_len.is_some(), scope: "global" })
        .collect();
    let (texts, coalesce) = {
        let EditorState { doc, ui: panels, .. } = &mut *state;
        let mut cx = Cx {
            ctx: ui.ctx().clone(),
            tk,
            doc,
            item: TriggerId(u32::MAX),
            vars,
            is_function: false,
            returns: None,
            cursor: None,
            collapsed: &mut panels.triggers.collapsed,
            lines: Vec::new(),
            ops: Vec::new(),
            open_picker: None,
            new_locals: Vec::new(),
            texts: Vec::new(),
            coalesce: None,
            label: None,
            clicked_line: None,
            cursor_rect: None,
        };
        ui.horizontal_wrapped(|ui| variable_row(ui, &mut cx, &mut v, Id::new(("gvar", i)), true));
        ui.add_space(6.0);
        let r = ui.add(
            egui::TextEdit::multiline(&mut v.comment)
                .hint_text("Comment")
                .desired_rows(2)
                .desired_width(f32::INFINITY)
                .id(Id::new(("gvar-c", i))),
        );
        if r.changed() {
            cx.coalesce = Some(r.id);
        }
        ui.add_space(6.0);
        let used = galaxy::global_var(&v.name);
        ui.label(RichText::new(format!("Galaxy name: {used}")).color(tk.text_dim).monospace());
        (cx.texts, cx.coalesce)
    };
    if v != original || !texts.is_empty() {
        let mut lib = state.doc.triggers.clone();
        lib.variables[i] = v.clone();
        let strings = (!texts.is_empty()).then(|| {
            let mut s = state.doc.strings.clone();
            for (k, val) in &texts {
                s.set(DEFAULT_LOCALE, k, val);
            }
            s
        });
        commit(state, format!("Edit variable {}", v.name), lib, strings, coalesce);
    }
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

fn actions_ui(ui: &mut Ui, cx: &mut Cx, list: &mut [Stmt], list_path: &[usize], depth: usize) {
    for (i, s) in list.iter_mut().enumerate() {
        let mut path = list_path.to_vec();
        path.push(i);
        stmt_ui(ui, cx, s, &path, depth);
    }
    if depth > 0 {
        ui.horizontal(|ui| {
            ui.add_space(6.0 + depth as f32 * 20.0);
            let r = ui.add(egui::Label::new(RichText::new("Add action").color(cx.tk.text_dim).small()).sense(Sense::click()));
            if r.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            if r.clicked() {
                cx.open_picker = Some((Purpose::Action, InsertAt::Action(list_path.to_vec(), list.len()), r.rect.left_bottom()));
            }
        });
    }
}

fn block_label(ui: &mut Ui, cx: &Cx, text: &str, depth: usize) {
    ui.horizontal(|ui| {
        ui.add_space(6.0 + depth as f32 * 20.0);
        ui.label(RichText::new(text).color(cx.tk.text_dim));
    });
}

fn collapse_arrow(ui: &mut Ui, cx: &mut Cx, path: &[usize]) -> bool {
    let key = (cx.item, path.to_vec());
    let collapsed = cx.collapsed.contains(&key);
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::click());
    let c = rect.center();
    let pts = if collapsed {
        vec![c + Vec2::new(-2.0, -4.0), c + Vec2::new(3.0, 0.0), c + Vec2::new(-2.0, 4.0)]
    } else {
        vec![c + Vec2::new(-4.0, -2.0), c + Vec2::new(4.0, -2.0), c + Vec2::new(0.0, 3.0)]
    };
    let color = if resp.hovered() { cx.tk.accent } else { cx.tk.text_dim };
    ui.painter().add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
    if resp.on_hover_text(if collapsed { "Expand" } else { "Collapse" }).clicked() {
        if collapsed {
            cx.collapsed.remove(&key);
        } else {
            cx.collapsed.insert(key);
        }
        return !collapsed;
    }
    collapsed
}

fn word(ui: &mut Ui, cx: &Cx, text: &str) {
    ui.label(RichText::new(text).color(cx.tk.text));
}

fn stmt_ui(ui: &mut Ui, cx: &mut Cx, s: &mut Stmt, path: &[usize], depth: usize) {
    let line = LineRef::Action(path.to_vec());
    let id = Id::new((cx.item, "act", path.to_vec()));
    let disabled = matches!(s, Stmt::Disabled(_));
    let s = match s {
        Stmt::Disabled(inner) => inner.as_mut(),
        s => s,
    };
    let has_children = !s.children().is_empty();
    let mut collapsed = false;
    line_frame(ui, cx, line, depth, |ui, cx| {
        if has_children {
            collapsed = collapse_arrow(ui, cx, path);
        }
        if disabled {
            ui.label(RichText::new("Disabled:").color(cx.tk.text_dim).italics());
        }
        match s {
            Stmt::Call(c) => {
                call_ui(ui, cx, c, id, false, false);
            }
            Stmt::Set { variable, value, index } => {
                word(ui, cx, "Set");
                var_chip(ui, cx, variable, None, id.with("var"));
                let vi = cx.var(variable).cloned();
                if vi.as_ref().is_some_and(|v| v.array) {
                    word(ui, cx, "[");
                    let mut ix = index.clone().unwrap_or(Expr::int(0));
                    slot_ui(ui, cx, &mut ix, &Slot::of("int", "Index"), id.with("ix"), 1);
                    *index = Some(ix);
                    word(ui, cx, "]");
                } else if index.is_some() {
                    *index = None;
                }
                word(ui, cx, "=");
                let ty = vi.map(|v| v.ty).unwrap_or_default();
                slot_ui(ui, cx, value, &Slot::of(&ty, "Value"), id.with("val"), 0);
            }
            Stmt::If { cond, .. } => {
                word(ui, cx, "If");
                slot_ui(ui, cx, cond, &Slot::of("bool", "Condition"), id.with("cond"), 0);
            }
            Stmt::While { cond, .. } => {
                word(ui, cx, "While");
                slot_ui(ui, cx, cond, &Slot::of("bool", "Condition"), id.with("cond"), 0);
            }
            Stmt::For { variable, from, to, step, .. } => {
                word(ui, cx, "For each integer");
                var_chip(ui, cx, variable, Some("int"), id.with("var"));
                word(ui, cx, "from");
                slot_ui(ui, cx, from, &Slot::of("int", "Start"), id.with("from"), 0);
                word(ui, cx, "to");
                slot_ui(ui, cx, to, &Slot::of("int", "End"), id.with("to"), 0);
                word(ui, cx, "step");
                let mut st = step.clone().unwrap_or(Expr::int(1));
                slot_ui(ui, cx, &mut st, &Slot::of("int", "Step"), id.with("step"), 0);
                *step = if st == Expr::int(1) { None } else { Some(st) };
            }
            Stmt::ForEachUnit { variable, group, .. } => {
                word(ui, cx, "For each unit");
                var_chip(ui, cx, variable, Some("unit"), id.with("var"));
                word(ui, cx, "in");
                slot_ui(ui, cx, group, &Slot::of("unitgroup", "Unit group"), id.with("g"), 0);
            }
            Stmt::ForEachPlayer { variable, group, .. } => {
                word(ui, cx, "For each player");
                var_chip(ui, cx, variable, Some("int"), id.with("var"));
                word(ui, cx, "in");
                slot_ui(ui, cx, group, &Slot::of("playergroup", "Player group"), id.with("g"), 0);
            }
            Stmt::Switch { value, .. } => {
                word(ui, cx, "Switch on");
                slot_ui(ui, cx, value, &Slot::any("Value"), id.with("v"), 0);
            }
            Stmt::Wait(e) => {
                word(ui, cx, "Wait");
                slot_ui(ui, cx, e, &Slot::of("fixed", "Seconds"), id.with("t"), 0);
                word(ui, cx, "game seconds");
            }
            Stmt::Break => word(ui, cx, "Break out of the loop"),
            Stmt::Continue => word(ui, cx, "Continue with the next loop iteration"),
            Stmt::Return => word(ui, cx, if cx.is_function { "Return" } else { "Skip remaining actions" }),
            Stmt::ReturnValue(e) => {
                word(ui, cx, "Return");
                let ty = cx.returns.as_ref().map(|t| t.galaxy_name().to_owned()).unwrap_or_default();
                slot_ui(ui, cx, e, &Slot::of(&ty, "Value"), id.with("r"), 0);
            }
            Stmt::Comment(text) => {
                ui.label(RichText::new("//").color(cx.tk.text_dim).monospace());
                let r = ui.add(egui::TextEdit::singleline(text).hint_text("Comment").desired_width(360.0).id(id.with("comment")));
                if r.changed() {
                    cx.coalesce = Some(r.id);
                    cx.label = Some("Edit comment".into());
                }
            }
            Stmt::Raw(code) => {
                ui.label(RichText::new("Galaxy").color(cx.tk.text_dim));
                let r = ui.add(egui::TextEdit::multiline(code).code_editor().desired_rows(1).desired_width(420.0).id(id.with("raw")));
                if r.changed() {
                    cx.coalesce = Some(r.id);
                    cx.label = Some("Edit Galaxy".into());
                }
            }
            Stmt::Disabled(_) => {}
        }
    });
    if collapsed || !has_children {
        return;
    }
    let child = |b: usize| {
        let mut p = path.to_vec();
        p.push(b);
        p
    };
    match s {
        Stmt::If { then, otherwise, .. } => {
            block_label(ui, cx, "Then", depth + 1);
            actions_ui(ui, cx, then, &child(0), depth + 1);
            block_label(ui, cx, "Else", depth + 1);
            actions_ui(ui, cx, otherwise, &child(1), depth + 1);
        }
        Stmt::While { body, .. } | Stmt::For { body, .. } | Stmt::ForEachUnit { body, .. } | Stmt::ForEachPlayer { body, .. } => {
            actions_ui(ui, cx, body, &child(0), depth + 1);
        }
        Stmt::Switch { cases, default, .. } => {
            let n = cases.len();
            let mut remove = None;
            for (ci, case) in cases.iter_mut().enumerate() {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(6.0 + (depth + 1) as f32 * 20.0);
                    ui.label(RichText::new("Case").color(cx.tk.text_dim));
                    slot_ui(ui, cx, &mut case.value, &Slot::any("Case value"), Id::new((cx.item, "case", path.to_vec(), ci)), 0);
                    if ui.small_button("Remove case").clicked() {
                        remove = Some(ci);
                    }
                });
                actions_ui(ui, cx, &mut case.body, &child(ci), depth + 2);
            }
            if let Some(ci) = remove {
                cases.remove(ci);
                cx.label = Some("Remove case".into());
            }
            ui.horizontal(|ui| {
                ui.add_space(6.0 + (depth + 1) as f32 * 20.0);
                if ui.small_button("Add case").clicked() {
                    cases.push(SwitchCase { value: Expr::unset(), body: vec![] });
                    cx.label = Some("Add case".into());
                }
            });
            block_label(ui, cx, "Default", depth + 1);
            actions_ui(ui, cx, default, &child(n), depth + 2);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Calls, slots and chips
// ---------------------------------------------------------------------------

/// Render a call as its phrase. Top-level calls draw their words as plain
/// text; nested calls draw them clickable and return the response of the
/// first word, which opens the enclosing slot's editor.
fn call_ui(ui: &mut Ui, cx: &mut Cx, c: &mut Call, id: Id, nested: bool, event: bool) -> Option<egui::Response> {
    let tk = cx.tk;
    let mut first: Option<egui::Response> = None;
    let text = |ui: &mut Ui, t: &str, first: &mut Option<egui::Response>| {
        let t = t.trim();
        if t.is_empty() {
            return;
        }
        if nested {
            let r = ui.add(egui::Label::new(RichText::new(t).color(tk.text)).sense(Sense::click()));
            if r.hovered() {
                ui.painter().line_segment([r.rect.left_bottom(), r.rect.right_bottom()], Stroke::new(1.0, tk.accent));
            }
            match first {
                Some(f) => *f = f.union(r),
                None => *first = Some(r),
            }
        } else {
            ui.label(RichText::new(t).color(tk.text));
        }
    };
    if let Some(spec) = trigger_lib::get(&c.function) {
        while c.args.len() < spec.params.len() {
            c.args.push(spec.params[c.args.len()].default_expr());
        }
        for part in spec.phrase_parts() {
            match part {
                trigger_lib::PhrasePart::Text(t) => text(ui, t, &mut first),
                trigger_lib::PhrasePart::Param(i) => {
                    let p = &spec.params[i];
                    slot_ui(ui, cx, &mut c.args[i], &Slot::param(p), id.with(i), 1);
                }
            }
        }
        if !nested && !event {
            ui.label(RichText::new(spec.category).color(tk.text_dim).small()).on_hover_text(spec.desc);
        }
    } else if let Some(f) = cx.doc.triggers.function_named(&c.function).cloned() {
        while c.args.len() < f.params.len() {
            c.args.push(Expr::unset());
        }
        text(ui, &f.name, &mut first);
        for (i, p) in f.params.iter().enumerate() {
            if i > 0 {
                text(ui, ",", &mut first);
            }
            slot_ui(ui, cx, &mut c.args[i], &Slot::of(p.ty.galaxy_name(), &p.name), id.with(i), 1);
        }
    } else {
        text(ui, &format!("{}(", c.function), &mut first);
        for (i, a) in c.args.iter_mut().enumerate() {
            slot_ui(ui, cx, a, &Slot::any("Argument"), id.with(i), 1);
        }
        text(ui, ")", &mut first);
        if !nested {
            ui.label(RichText::new("Unknown function").color(tk.warn).small());
        }
    }
    first
}

fn bits_label(slot: &Slot, value: &str) -> String {
    let parts: Vec<&str> = value.split('|').map(str::trim).filter(|p| !p.is_empty()).collect();
    parts.iter().map(|p| slot.presets.iter().find(|(_, v)| v == p).map(|(l, _)| *l).unwrap_or(p)).collect::<Vec<_>>().join(" + ")
}

fn expr_label(cx: &Cx, e: &Expr, slot: &Slot) -> (String, Color32) {
    let tk = cx.tk;
    match e {
        Expr::Literal(Literal::Int(v)) => (v.to_string(), tk.text),
        Expr::Literal(Literal::Fixed(v)) => (galaxy::fixed_literal(*v), tk.text),
        Expr::Literal(Literal::Bool(b)) => {
            let words = slot.alts.split_once('/');
            let w = match words {
                Some((on, off)) => if *b { on } else { off }.to_owned(),
                None => b.to_string(),
            };
            (w, tk.text)
        }
        Expr::Literal(Literal::String(s)) if slot.editor == "gamelink" => {
            if s.is_empty() {
                ("(none)".into(), tk.warn)
            } else {
                (s.clone(), tk.text)
            }
        }
        Expr::Literal(Literal::String(s)) => (format!("\"{s}\""), tk.ok),
        Expr::Literal(Literal::Text(k)) => match cx.text_value(k) {
            Some(v) if !v.is_empty() => (format!("\"{}\"", truncate(&v, 40)), tk.ok),
            _ => ("(empty text)".into(), tk.warn),
        },
        Expr::Variable(v) => (v.clone(), if cx.var(v).is_some() { tk.text } else { tk.error }),
        Expr::Preset(p) if !slot.bits && slot.presets.iter().any(|(_, v)| v == p) => {
            (slot.presets.iter().find(|(_, v)| v == p).map(|(l, _)| (*l).to_owned()).unwrap_or_default(), tk.text)
        }
        Expr::Preset(p) if p == "null" => ("none".into(), tk.text),
        Expr::Preset(p) if slot.bits || p.contains('|') => (bits_label(slot, p), tk.text),
        Expr::Preset(p) => (slot.presets.iter().find(|(_, v)| v == p).map(|(l, _)| (*l).to_owned()).unwrap_or_else(|| p.clone()), tk.text),
        Expr::Placed(n) => (n.clone(), tk.text),
        Expr::Trigger(id) => match cx.doc.triggers.trigger(*id) {
            Some(t) => (t.name.clone(), tk.text),
            None => ("(missing trigger)".into(), tk.error),
        },
        Expr::Raw(r) if r.trim().is_empty() => (format!("({})", slot.label.to_lowercase()), tk.warn),
        Expr::Raw(r) => (truncate(r, 48), tk.text_dim),
        Expr::Call(c) => (format!("{}(...)", c.function), tk.text),
        Expr::Binary { op, .. } => (op.clone(), tk.text),
        Expr::Not(_) => ("not".into(), tk.text),
        Expr::Element { variable, .. } => (format!("{variable}[...]"), tk.text),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_owned()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

/// A clickable parameter chip with an accent outline when hovered or open.
fn chip(ui: &mut Ui, tk: &Tokens, text: &str, color: Color32, open: bool, mono: bool) -> egui::Response {
    ui.scope(|ui| {
        let v = ui.visuals_mut();
        let base = if open { tk.accent } else { tk.stroke };
        v.widgets.inactive.bg_stroke = Stroke::new(1.0, base);
        v.widgets.inactive.weak_bg_fill = tk.bg_raised;
        v.widgets.hovered.bg_stroke = Stroke::new(1.0, tk.accent);
        v.widgets.hovered.weak_bg_fill = tk.bg_raised;
        v.widgets.active.bg_stroke = Stroke::new(1.0, tk.accent);
        v.widgets.active.weak_bg_fill = tk.bg_raised;
        ui.spacing_mut().button_padding = Vec2::new(5.0, 1.0);
        let mut rt = RichText::new(text).color(color);
        if mono {
            rt = rt.monospace();
        }
        ui.add(egui::Button::new(rt).corner_radius(4.0))
    })
    .inner
}

fn slot_ui(ui: &mut Ui, cx: &mut Cx, e: &mut Expr, slot: &Slot, id: Id, depth: usize) {
    let pop = id.with("pop");
    let open = egui::Popup::is_id_open(&cx.ctx, pop);
    let anchor: egui::Response = match e {
        Expr::Call(c)
            if depth < 8 && (trigger_lib::get(&c.function).is_some() || cx.doc.triggers.function_named(&c.function).is_some()) =>
        {
            let tk = cx.tk;
            ui.label(RichText::new("(").color(tk.text_dim));
            let r = call_ui(ui, cx, c, id.with("call"), true, false);
            ui.label(RichText::new(")").color(tk.text_dim));
            match r {
                Some(r) => r,
                None => chip(ui, &tk, "function", tk.text, open, false),
            }
        }
        Expr::Binary { op, lhs, rhs } => {
            let tk = cx.tk;
            let logical = op == "&&" || op == "||";
            let compare = matches!(op.as_str(), "==" | "!=" | "<" | "<=" | ">" | ">=");
            let lt = if logical {
                "bool".to_owned()
            } else if compare {
                infer(cx, rhs).unwrap_or_default()
            } else {
                slot.ty.clone()
            };
            let rt = if logical {
                "bool".to_owned()
            } else if compare {
                infer(cx, lhs).unwrap_or_default()
            } else {
                slot.ty.clone()
            };
            if depth > 0 {
                ui.label(RichText::new("(").color(tk.text_dim));
            }
            slot_ui(ui, cx, lhs, &Slot::of(&lt, "Value"), id.with("l"), depth + 1);
            let word = match op.as_str() {
                "&&" => "and",
                "||" => "or",
                o => o,
            };
            let r = chip(ui, &tk, word, tk.text, open, true);
            slot_ui(ui, cx, rhs, &Slot::of(&rt, "Value"), id.with("r"), depth + 1);
            if depth > 0 {
                ui.label(RichText::new(")").color(tk.text_dim));
            }
            r
        }
        Expr::Not(inner) => {
            let tk = cx.tk;
            let r = chip(ui, &tk, "not", tk.text, open, false);
            slot_ui(ui, cx, inner, &Slot::of("bool", "Condition"), id.with("n"), depth + 1);
            r
        }
        Expr::Element { variable, index } => {
            let tk = cx.tk;
            let r = chip(ui, &tk, variable, tk.text, open, false);
            ui.label(RichText::new("[").color(tk.text_dim));
            slot_ui(ui, cx, index, &Slot::of("int", "Index"), id.with("ix"), depth + 1);
            ui.label(RichText::new("]").color(tk.text_dim));
            r
        }
        _ => {
            let (text, color) = expr_label(cx, e, slot);
            let mono = matches!(e, Expr::Raw(r) if !r.trim().is_empty()) || matches!(e, Expr::Call(_));
            let tk = cx.tk;
            chip(ui, &tk, &text, color, open, mono)
        }
    };
    let anchor =
        anchor.on_hover_text(if slot.ty.is_empty() { slot.label.clone() } else { format!("{} ({})", slot.label, type_word(&slot.ty)) });
    if anchor.clicked() {
        egui::Popup::toggle_id(&cx.ctx, pop);
    }
    if egui::Popup::is_id_open(&cx.ctx, pop) {
        egui::Popup::new(pop, cx.ctx.clone(), &anchor, ui.layer_id())
            .open_memory(None)
            .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
            .width(340.0)
            .show(|ui| value_editor(ui, cx, e, slot, pop));
    }
}

fn type_word(ty: &str) -> String {
    match ty {
        "unitref" => "Unit".into(),
        "abilcmd" => "Ability command".into(),
        "camerainfo" => "Camera".into(),
        "soundlink" => "Sound link".into(),
        _ => GalaxyType::from_name(ty).label(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VTab {
    Value,
    Preset,
    Variable,
    Function,
    Object,
    Trigger,
    Galaxy,
}

impl VTab {
    fn label(self) -> &'static str {
        match self {
            VTab::Value => "Value",
            VTab::Preset => "Preset",
            VTab::Variable => "Variable",
            VTab::Function => "Function",
            VTab::Object => "Object",
            VTab::Trigger => "Trigger",
            VTab::Galaxy => "Galaxy",
        }
    }
}

fn literal_kind(ty: &str, editor: &str) -> bool {
    matches!(ty, "int" | "fixed" | "bool" | "string" | "text" | "byte") || editor == "gamelink"
}

fn object_kind(ty: &str) -> bool {
    matches!(ty, "point" | "region" | "unit" | "unitref" | "camerainfo")
}

fn value_editor(ui: &mut Ui, cx: &mut Cx, e: &mut Expr, slot: &Slot, pop: Id) {
    let tk = cx.tk;
    let mut tabs = Vec::new();
    if literal_kind(&slot.ty, &slot.editor) || slot.ty.is_empty() {
        tabs.push(VTab::Value);
    }
    if !slot.presets.is_empty() {
        tabs.push(VTab::Preset);
    }
    tabs.push(VTab::Variable);
    tabs.push(VTab::Function);
    if object_kind(&slot.ty) {
        tabs.push(VTab::Object);
    }
    if slot.ty == "trigger" {
        tabs.push(VTab::Trigger);
    }
    tabs.push(VTab::Galaxy);
    let default = match e {
        Expr::Preset(_) if tabs.contains(&VTab::Preset) => VTab::Preset,
        Expr::Variable(_) | Expr::Element { .. } => VTab::Variable,
        Expr::Call(_) | Expr::Binary { .. } | Expr::Not(_) => VTab::Function,
        Expr::Placed(_) if tabs.contains(&VTab::Object) => VTab::Object,
        Expr::Trigger(_) if tabs.contains(&VTab::Trigger) => VTab::Trigger,
        Expr::Raw(r) if !r.trim().is_empty() => VTab::Galaxy,
        _ if tabs.contains(&VTab::Value) => VTab::Value,
        _ if tabs.contains(&VTab::Preset) => VTab::Preset,
        _ if tabs.contains(&VTab::Object) => VTab::Object,
        _ => VTab::Function,
    };
    let tab_id = pop.with("tab");
    let mut tab: VTab = ui.data(|d| d.get_temp(tab_id)).unwrap_or(default);
    if !tabs.contains(&tab) {
        tab = default;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new(&slot.label).strong());
        if !slot.ty.is_empty() {
            ui.label(RichText::new(type_word(&slot.ty)).color(tk.text_dim).small());
        }
    });
    ui.horizontal_wrapped(|ui| {
        for t in &tabs {
            if ui.selectable_label(tab == *t, t.label()).clicked() {
                tab = *t;
            }
        }
    });
    ui.data_mut(|d| d.insert_temp(tab_id, tab));
    ui.separator();
    let close = |cx: &Cx| egui::Popup::close_id(&cx.ctx, pop);
    match tab {
        VTab::Value => literal_editor(ui, cx, e, slot, pop),
        VTab::Preset => {
            egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                if slot.bits {
                    let current = match e {
                        Expr::Preset(p) => p.clone(),
                        _ => String::new(),
                    };
                    let mut set: Vec<&str> = current.split('|').map(str::trim).filter(|p| !p.is_empty() && *p != "0").collect();
                    let mut changed = false;
                    for (label, value) in slot.presets {
                        let mut on = set.contains(value);
                        if ui.checkbox(&mut on, *label).changed() {
                            changed = true;
                            if on {
                                set.push(value);
                            } else {
                                set.retain(|v| v != value);
                            }
                        }
                    }
                    if changed {
                        *e = if set.is_empty() { Expr::int(0) } else { Expr::Preset(set.join(" | ")) };
                    }
                } else {
                    for (label, value) in slot.presets {
                        let sel = matches!(e, Expr::Preset(p) if p == value);
                        if ui.selectable_label(sel, *label).on_hover_text(*value).clicked() {
                            *e = Expr::Preset((*value).into());
                            close(cx);
                        }
                    }
                }
            });
        }
        VTab::Variable => {
            let all_id = pop.with("allvars");
            let mut all: bool = ui.data(|d| d.get_temp(all_id)).unwrap_or(false);
            let vars: Vec<VarInfo> = cx.vars.iter().filter(|v| all || slot.ty.is_empty() || type_ok(&slot.ty, &v.ty)).cloned().collect();
            if vars.is_empty() {
                ui.label(RichText::new(format!("No {} variables in scope.", type_word(&slot.ty).to_lowercase())).color(tk.text_dim));
            }
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for v in vars {
                    let sel = matches!(e, Expr::Variable(n) | Expr::Element { variable: n, .. } if *n == v.name);
                    let text = format!("{}  ·  {} {}", v.name, v.scope, type_word(&v.ty).to_lowercase());
                    if ui.selectable_label(sel, text).clicked() {
                        *e = if v.array {
                            Expr::Element { variable: v.name.clone(), index: Box::new(Expr::int(0)) }
                        } else {
                            Expr::Variable(v.name.clone())
                        };
                        close(cx);
                    }
                }
            });
            if !slot.ty.is_empty() && ui.checkbox(&mut all, "Show all types").changed() {
                ui.data_mut(|d| d.insert_temp(all_id, all));
            }
        }
        VTab::Function => {
            if let Some(choice) = choice_list(ui, cx, &Want::Value(slot.ty.clone()), pop.with("fn"), true) {
                if let Some(new) = choice_expr(&choice, &cx.doc.triggers) {
                    *e = new;
                    cx.label = Some(format!("Use {}", choice_label(&choice, &cx.doc.triggers)));
                }
                close(cx);
            }
        }
        VTab::Object => {
            let q_id = pop.with("objq");
            let mut q: String = ui.data(|d| d.get_temp(q_id)).unwrap_or_default();
            ui.add(egui::TextEdit::singleline(&mut q).hint_text("Filter").desired_width(f32::INFINITY));
            ui.data_mut(|d| d.insert_temp(q_id, q.clone()));
            let doc = cx.doc;
            let mut names: Vec<(String, &str)> = Vec::new();
            let ty = slot.ty.as_str();
            if ty == "point" {
                names.extend(
                    doc.objects
                        .objects
                        .iter()
                        .filter(|o| o.kind == ObjectKind::Point && !o.name.is_empty())
                        .map(|o| (o.name.clone(), "point")),
                );
            }
            if ty == "region" {
                names.extend(doc.regions.iter().map(|r| (r.name.clone(), "region")));
            }
            if ty == "unit" || ty == "unitref" {
                names.extend(
                    doc.objects
                        .objects
                        .iter()
                        .filter(|o| o.kind == ObjectKind::Unit && !o.name.is_empty())
                        .map(|o| (o.name.clone(), "unit")),
                );
            }
            if ty == "camerainfo" {
                names.extend(doc.cameras.iter().map(|c| (c.name.clone(), "camera")));
            }
            names.retain(|(n, _)| matches_filter(n, &q));
            if names.is_empty() {
                let what = match ty {
                    "point" => "No named points. Place one in Points mode.",
                    "region" => "No regions. Draw one in Regions mode.",
                    "camerainfo" => "No cameras. Add one in Cameras mode.",
                    _ => "No named units. Give a placed unit a name in the inspector.",
                };
                ui.label(RichText::new(what).color(tk.text_dim));
            }
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for (n, kind) in names {
                    let sel = matches!(e, Expr::Placed(p) if *p == n);
                    if ui.selectable_label(sel, format!("{n}  ·  {kind}")).clicked() {
                        *e = Expr::Placed(n);
                        close(cx);
                    }
                }
            });
        }
        VTab::Trigger => {
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                for t in cx.doc.triggers.triggers() {
                    let sel = matches!(e, Expr::Trigger(i) if *i == t.id);
                    if ui.selectable_label(sel, &t.name).clicked() {
                        *e = Expr::Trigger(t.id);
                        close(cx);
                    }
                }
            });
        }
        VTab::Galaxy => {
            let mut code = match &*e {
                Expr::Raw(r) => r.clone(),
                other => galaxy_preview(cx, other),
            };
            let r = ui.add(
                egui::TextEdit::singleline(&mut code)
                    .code_editor()
                    .hint_text("Any Galaxy expression")
                    .desired_width(f32::INFINITY)
                    .id(pop.with("raw")),
            );
            if r.changed() {
                *e = Expr::Raw(code.clone());
                cx.coalesce = Some(r.id);
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                if let Some(parsed) = trigger_lib::parse_expr(&code) {
                    if !matches!(parsed, Expr::Raw(_)) {
                        *e = parsed;
                    }
                }
                close(cx);
            }
            ui.label(RichText::new("Raw Galaxy is passed through unchanged. Press Enter to apply.").color(tk.text_dim).small());
        }
    }
}

fn galaxy_preview(cx: &Cx, e: &Expr) -> String {
    if e.is_unset() {
        return String::new();
    }
    galaxy::expr(e, &cx.doc.triggers)
}

fn literal_editor(ui: &mut Ui, cx: &mut Cx, e: &mut Expr, slot: &Slot, pop: Id) {
    let tk = cx.tk;
    let ty = if slot.ty.is_empty() { infer(cx, e).unwrap_or_else(|| "int".into()) } else { slot.ty.clone() };
    let buf_id = pop.with("buf");
    let field_id = pop.with("field");
    match ty.as_str() {
        "bool" => {
            let (on, off) = slot.alts.split_once('/').unwrap_or(("True", "False"));
            let cur = matches!(e, Expr::Literal(Literal::Bool(true)));
            ui.horizontal(|ui| {
                if ui.selectable_label(cur && matches!(e, Expr::Literal(_)), on).clicked() {
                    *e = Expr::Literal(Literal::Bool(true));
                    egui::Popup::close_id(&cx.ctx, pop);
                }
                if ui.selectable_label(!cur && matches!(e, Expr::Literal(_)), off).clicked() {
                    *e = Expr::Literal(Literal::Bool(false));
                    egui::Popup::close_id(&cx.ctx, pop);
                }
            });
        }
        "text" => {
            let (key, mut value) = match e {
                Expr::Literal(Literal::Text(k)) => (Some(k.clone()), cx.text_value(k).unwrap_or_default()),
                _ => (None, String::new()),
            };
            let r = ui.add(
                egui::TextEdit::multiline(&mut value)
                    .hint_text("Text shown to players")
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .id(field_id),
            );
            if !ui.memory(|m| m.has_focus(field_id)) && ui.data(|d| d.get_temp::<bool>(buf_id)).is_none() {
                r.request_focus();
                ui.data_mut(|d| d.insert_temp(buf_id, true));
            }
            if r.changed() {
                let key = key.unwrap_or_else(|| cx.new_text_key());
                cx.texts.push((key.clone(), value));
                *e = Expr::Literal(Literal::Text(key));
                cx.coalesce = Some(field_id);
                cx.label = Some("Edit text".into());
            }
            if let Expr::Literal(Literal::Text(k)) = e {
                ui.label(RichText::new(format!("Stored as {k} ({DEFAULT_LOCALE})")).color(tk.text_dim).small());
            }
        }
        "int" | "fixed" | "byte" => {
            let current = match e {
                Expr::Literal(Literal::Int(v)) => v.to_string(),
                Expr::Literal(Literal::Fixed(v)) => galaxy::fixed_literal(*v),
                _ => String::new(),
            };
            let mut buf: (String, String) = ui.data(|d| d.get_temp(buf_id)).unwrap_or_else(|| (current.clone(), current.clone()));
            if buf.1 != current {
                buf = (current.clone(), current.clone());
            }
            let r = ui.add(
                egui::TextEdit::singleline(&mut buf.0)
                    .hint_text(if ty == "fixed" { "e.g. 2.5" } else { "e.g. 10" })
                    .desired_width(140.0)
                    .id(field_id),
            );
            if ui.data(|d| d.get_temp::<bool>(pop.with("focused"))).is_none() {
                r.request_focus();
                ui.data_mut(|d| d.insert_temp(pop.with("focused"), true));
            }
            if r.changed() {
                let t = buf.0.trim();
                let parsed = if ty == "fixed" {
                    t.parse::<f64>().ok().map(|v| if t.contains('.') { Expr::fixed(v) } else { Expr::int(v as i64) })
                } else {
                    t.parse::<i64>().ok().map(Expr::int)
                };
                match parsed {
                    Some(v) => {
                        *e = v;
                        buf.1 = match e {
                            Expr::Literal(Literal::Int(v)) => v.to_string(),
                            Expr::Literal(Literal::Fixed(v)) => galaxy::fixed_literal(*v),
                            _ => String::new(),
                        };
                        cx.coalesce = Some(field_id);
                        cx.label = Some("Edit value".into());
                    }
                    None if !t.is_empty() => {
                        ui.label(RichText::new("Not a number").color(tk.error).small());
                    }
                    None => {}
                }
            }
            ui.data_mut(|d| d.insert_temp(buf_id, buf));
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                egui::Popup::close_id(&cx.ctx, pop);
            }
        }
        _ => {
            // string, gamelink and anything else typed as text.
            let mut s = match e {
                Expr::Literal(Literal::String(s)) => s.clone(),
                _ => String::new(),
            };
            let hint = if slot.editor == "gamelink" { format!("{} id, e.g. Marine", slot.link) } else { "Text".into() };
            let r = ui.add(egui::TextEdit::singleline(&mut s).hint_text(hint).desired_width(f32::INFINITY).id(field_id));
            if ui.data(|d| d.get_temp::<bool>(pop.with("focused"))).is_none() {
                r.request_focus();
                ui.data_mut(|d| d.insert_temp(pop.with("focused"), true));
            }
            if r.changed() {
                *e = Expr::Literal(Literal::String(s.clone()));
                cx.coalesce = Some(field_id);
                cx.label = Some("Edit value".into());
            }
            if slot.editor == "gamelink" {
                let class = format!("C{}", slot.link);
                let ids: Vec<&str> = cx
                    .doc
                    .data
                    .entries
                    .iter()
                    .filter(|en| en.class() == class || en.class().starts_with(&class))
                    .map(|en| en.id())
                    .filter(|id| matches_filter(id, &s))
                    .take(12)
                    .collect();
                if !ids.is_empty() {
                    ui.label(RichText::new("From this map's data").color(tk.text_dim).small());
                    ui.horizontal_wrapped(|ui| {
                        for id in ids {
                            if ui.small_button(id).clicked() {
                                *e = Expr::Literal(Literal::String(id.into()));
                                egui::Popup::close_id(&cx.ctx, pop);
                            }
                        }
                    });
                }
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                egui::Popup::close_id(&cx.ctx, pop);
            }
        }
    }
}

/// Chip naming a variable (Set target, loop variable). Lets the user pick one
/// in scope or create a new local.
fn var_chip(ui: &mut Ui, cx: &mut Cx, name: &mut String, want: Option<&str>, id: Id) {
    let tk = cx.tk;
    let pop = id.with("pop");
    let open = egui::Popup::is_id_open(&cx.ctx, pop);
    let (text, color) = if name.is_empty() {
        ("(variable)".to_owned(), tk.warn)
    } else if cx.var(name).is_some() || want.is_some() {
        (name.clone(), tk.text)
    } else {
        (name.clone(), tk.error)
    };
    let r = chip(ui, &tk, &text, color, open, false).on_hover_text("Variable");
    if r.clicked() {
        egui::Popup::toggle_id(&cx.ctx, pop);
    }
    if !egui::Popup::is_id_open(&cx.ctx, pop) {
        return;
    }
    egui::Popup::new(pop, cx.ctx.clone(), &r, ui.layer_id())
        .open_memory(None)
        .close_behavior(PopupCloseBehavior::CloseOnClickOutside)
        .width(300.0)
        .show(|ui| {
            ui.label(RichText::new("Variable").strong());
            let vars: Vec<VarInfo> = cx.vars.iter().filter(|v| want.is_none_or(|w| type_ok(w, &v.ty))).cloned().collect();
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                for v in vars {
                    let text = format!("{}  ·  {} {}", v.name, v.scope, type_word(&v.ty).to_lowercase());
                    if ui.selectable_label(*name == v.name, text).clicked() {
                        *name = v.name.clone();
                        egui::Popup::close_id(&cx.ctx, pop);
                    }
                }
            });
            ui.separator();
            let nid = pop.with("new");
            let mut new: String = ui.data(|d| d.get_temp(nid)).unwrap_or_default();
            ui.horizontal(|ui| {
                let r = ui.add(egui::TextEdit::singleline(&mut new).hint_text("New local variable").desired_width(180.0));
                let create = ui.add_enabled(!new.trim().is_empty(), egui::Button::new("Create")).clicked()
                    || (r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) && !new.trim().is_empty());
                if create {
                    let ty = GalaxyType::from_name(want.unwrap_or("int"));
                    let n = new.trim().to_owned();
                    cx.new_locals.push(Variable::new(n.clone(), ty));
                    *name = n;
                    cx.label = Some("Add local variable".into());
                    new.clear();
                    egui::Popup::close_id(&cx.ctx, pop);
                }
            });
            ui.data_mut(|d| d.insert_temp(nid, new));
        });
}

// ---------------------------------------------------------------------------
// Library choices and the picker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Syn {
    If,
    While,
    For,
    ForEachUnit,
    ForEachPlayer,
    Switch,
    Set,
    Wait,
    Break,
    Continue,
    Return,
    ReturnValue,
    Comment,
    Raw,
    Compare,
    And,
    Or,
    Not,
    Arith,
}

impl Syn {
    fn label(self) -> &'static str {
        match self {
            Syn::If => "If then else",
            Syn::While => "While",
            Syn::For => "For each integer",
            Syn::ForEachUnit => "For each unit in unit group",
            Syn::ForEachPlayer => "For each player in player group",
            Syn::Switch => "Switch",
            Syn::Set => "Set variable",
            Syn::Wait => "Wait",
            Syn::Break => "Break",
            Syn::Continue => "Continue",
            Syn::Return => "Skip remaining actions",
            Syn::ReturnValue => "Return value",
            Syn::Comment => "Comment",
            Syn::Raw => "Custom Galaxy",
            Syn::Compare => "Comparison",
            Syn::And => "And",
            Syn::Or => "Or",
            Syn::Not => "Not",
            Syn::Arith => "Arithmetic",
        }
    }
    fn desc(self) -> &'static str {
        match self {
            Syn::If => "Run actions only when a condition is true, otherwise others.",
            Syn::While => "Repeat actions while a condition is true.",
            Syn::For => "Repeat actions for each integer in a range.",
            Syn::ForEachUnit => "Repeat actions for every unit in a unit group.",
            Syn::ForEachPlayer => "Repeat actions for every player in a player group.",
            Syn::Switch => "Run the actions of the first case that matches a value.",
            Syn::Set => "Store a value in a variable.",
            Syn::Wait => "Pause this trigger for some game time.",
            Syn::Break => "Stop the innermost loop.",
            Syn::Continue => "Skip to the next iteration of the innermost loop.",
            Syn::Return => "Stop running this trigger's actions.",
            Syn::ReturnValue => "Return a value from this function.",
            Syn::Comment => "A note for people reading the trigger.",
            Syn::Raw => "Galaxy code inserted as written.",
            Syn::Compare => "Compare two values.",
            Syn::And => "True when both conditions are true.",
            Syn::Or => "True when either condition is true.",
            Syn::Not => "True when a condition is false.",
            Syn::Arith => "Add, subtract, multiply or divide two numbers.",
        }
    }
}

#[derive(Debug, Clone)]
enum Choice {
    Lib(&'static FnSpec),
    Custom(TriggerId),
    Syn(Syn),
}

#[derive(Debug, Clone, PartialEq)]
enum Want {
    Event,
    Condition,
    Action { function: bool },
    Value(String),
}

fn choice_label(c: &Choice, lib: &TriggerLibrary) -> String {
    match c {
        Choice::Lib(f) => f.label.to_owned(),
        Choice::Custom(id) => lib.function(*id).map(|f| f.name.clone()).unwrap_or_default(),
        Choice::Syn(s) => s.label().to_owned(),
    }
}

fn gather(lib: &TriggerLibrary, want: &Want, query: &str) -> Vec<Choice> {
    let mut syn: Vec<Syn> = Vec::new();
    match want {
        Want::Event => {}
        Want::Condition => syn.extend([Syn::Compare, Syn::And, Syn::Or, Syn::Not]),
        Want::Action { function } => {
            syn.extend([
                Syn::If,
                Syn::Set,
                Syn::Wait,
                Syn::For,
                Syn::ForEachUnit,
                Syn::ForEachPlayer,
                Syn::While,
                Syn::Switch,
                Syn::Break,
                Syn::Continue,
                Syn::Return,
                Syn::Comment,
                Syn::Raw,
            ]);
            if *function {
                syn.push(Syn::ReturnValue);
            }
        }
        Want::Value(ty) => match ty.as_str() {
            "bool" => syn.extend([Syn::Compare, Syn::And, Syn::Or, Syn::Not]),
            "int" | "fixed" | "" => syn.push(Syn::Arith),
            _ => {}
        },
    }
    let mut scored: Vec<(i32, usize, Choice)> = Vec::new();
    let q = query.trim();
    let score_text = |text: &str| {
        if q.is_empty() {
            Some(0)
        } else {
            q.split_whitespace().map(|w| trigger_lib::fuzzy_score(w, text).map(|s| s * 3)).sum()
        }
    };
    for (n, s) in syn.iter().enumerate() {
        if let Some(sc) = score_text(s.label()) {
            scored.push((sc + 5, n, Choice::Syn(*s)));
        }
    }
    for (n, f) in lib.functions.iter().enumerate() {
        let ok = match want {
            Want::Action { .. } => true,
            Want::Condition => f.returns == Some(GalaxyType::Bool),
            Want::Value(ty) => f.returns.as_ref().is_some_and(|r| type_ok(ty, r.galaxy_name())),
            Want::Event => false,
        };
        if ok {
            if let Some(sc) = score_text(&f.name) {
                scored.push((sc + 4, 100 + n, Choice::Custom(f.id)));
            }
        }
    }
    let filter = |f: &FnSpec| match want {
        Want::Event => f.kind == FnKind::Event,
        Want::Condition => f.kind == FnKind::Condition,
        Want::Action { .. } => f.kind == FnKind::Action,
        Want::Value(ty) => f.kind != FnKind::Event && !f.ret.is_empty() && type_ok(ty, f.ret),
    };
    for (n, f) in trigger_lib::all().iter().enumerate() {
        if !filter(f) {
            continue;
        }
        if let Some(sc) = if q.is_empty() { Some(0) } else { trigger_lib::score(f, q) } {
            scored.push((sc, 1000 + n, Choice::Lib(f)));
        }
    }
    if !q.is_empty() {
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    scored.into_iter().map(|(_, _, c)| c).collect()
}

/// Searchable list of choices with keyboard navigation. Returns the pick.
fn choice_list(ui: &mut Ui, cx: &Cx, want: &Want, id: Id, autofocus: bool) -> Option<Choice> {
    let tk = cx.tk;
    let q_id = id.with("q");
    let sel_id = id.with("sel");
    let mut q: String = ui.data(|d| d.get_temp(q_id)).unwrap_or_default();
    let mut sel: usize = ui.data(|d| d.get_temp(sel_id)).unwrap_or(0);
    let r = ui.add(
        egui::TextEdit::singleline(&mut q)
            .hint_text("Search by name, phrase or category")
            .desired_width(f32::INFINITY)
            .id(id.with("field")),
    );
    if autofocus && ui.data(|d| d.get_temp::<bool>(id.with("focused"))).is_none() {
        r.request_focus();
        ui.data_mut(|d| d.insert_temp(id.with("focused"), true));
    }
    if r.changed() {
        sel = 0;
    }
    let items = gather(&cx.doc.triggers, want, &q);
    let mut picked = None;
    if r.has_focus() || r.lost_focus() {
        ui.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                sel = (sel + 1).min(items.len().saturating_sub(1));
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                sel = sel.saturating_sub(1);
            }
        });
        if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
            picked = items.get(sel).cloned();
        }
    }
    let sel_clamped = sel.min(items.len().saturating_sub(1));
    egui::ScrollArea::vertical().id_salt(id.with("scroll")).max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
        if items.is_empty() {
            ui.label(RichText::new("Nothing matches. Try fewer letters.").color(tk.text_dim));
        }
        for (n, c) in items.iter().enumerate().take(200) {
            let (label, detail, desc) = match c {
                Choice::Lib(f) => (f.label.to_owned(), f.category.to_owned(), f.desc.to_owned()),
                Choice::Custom(fid) => {
                    let name = cx.doc.triggers.function(*fid).map(|f| f.name.clone()).unwrap_or_default();
                    (name, "Custom".into(), "A function defined in this map.".into())
                }
                Choice::Syn(s) => (s.label().to_owned(), "General".into(), s.desc().to_owned()),
            };
            let selected = n == sel_clamped;
            let resp = ui
                .horizontal(|ui| {
                    let r = ui.selectable_label(selected, RichText::new(&label).color(tk.text));
                    ui.label(RichText::new(detail).color(tk.text_dim).small());
                    r
                })
                .inner;
            let resp = resp.on_hover_text(match c {
                Choice::Lib(f) => format!("{}\n\n{}", f.desc, f.name),
                _ => desc.clone(),
            });
            if selected && (r.changed() || ui.input(|i| i.key_pressed(Key::ArrowDown) || i.key_pressed(Key::ArrowUp))) {
                resp.scroll_to_me(None);
            }
            if resp.clicked() {
                picked = Some(c.clone());
            }
        }
    });
    if let Some(c) = items.get(sel_clamped) {
        let desc = match c {
            Choice::Lib(f) => f.desc.to_owned(),
            Choice::Custom(_) => "A function defined in this map.".into(),
            Choice::Syn(s) => s.desc().to_owned(),
        };
        ui.separator();
        ui.label(RichText::new(desc).color(tk.text_dim).small());
    }
    ui.data_mut(|d| {
        d.insert_temp(q_id, q);
        d.insert_temp(sel_id, sel_clamped);
    });
    if picked.is_some() {
        ui.data_mut(|d| {
            d.remove::<String>(q_id);
            d.remove::<bool>(id.with("focused"));
        });
    }
    picked
}

fn choice_expr(c: &Choice, lib: &TriggerLibrary) -> Option<Expr> {
    Some(match c {
        Choice::Lib(f) => Expr::Call(f.new_call()),
        Choice::Custom(id) => {
            let f = lib.function(*id)?;
            Expr::Call(Call { function: f.name.clone(), args: f.params.iter().map(|_| Expr::unset()).collect() })
        }
        Choice::Syn(Syn::Compare) => Expr::Binary { op: "==".into(), lhs: Box::new(Expr::unset()), rhs: Box::new(Expr::unset()) },
        Choice::Syn(Syn::And) => Expr::Binary { op: "&&".into(), lhs: Box::new(Expr::unset()), rhs: Box::new(Expr::unset()) },
        Choice::Syn(Syn::Or) => Expr::Binary { op: "||".into(), lhs: Box::new(Expr::unset()), rhs: Box::new(Expr::unset()) },
        Choice::Syn(Syn::Not) => Expr::Not(Box::new(Expr::unset())),
        Choice::Syn(Syn::Arith) => Expr::Binary { op: "+".into(), lhs: Box::new(Expr::unset()), rhs: Box::new(Expr::unset()) },
        Choice::Syn(_) => return None,
    })
}

fn choice_stmt(c: &Choice, lib: &TriggerLibrary) -> Option<Stmt> {
    Some(match c {
        Choice::Lib(f) => Stmt::Call(f.new_call()),
        Choice::Custom(id) => {
            let f = lib.function(*id)?;
            Stmt::Call(Call { function: f.name.clone(), args: f.params.iter().map(|_| Expr::unset()).collect() })
        }
        Choice::Syn(s) => match s {
            Syn::If => Stmt::If { cond: Expr::unset(), then: vec![], otherwise: vec![] },
            Syn::While => Stmt::While { cond: Expr::unset(), body: vec![] },
            Syn::For => Stmt::For { variable: "i".into(), from: Expr::int(1), to: Expr::int(10), body: vec![], step: None },
            Syn::ForEachUnit => Stmt::ForEachUnit { variable: "Picked unit".into(), group: Expr::unset(), body: vec![] },
            Syn::ForEachPlayer => {
                Stmt::ForEachPlayer { variable: "Picked player".into(), group: Expr::call("PlayerGroupAll", vec![]), body: vec![] }
            }
            Syn::Switch => {
                Stmt::Switch { value: Expr::unset(), cases: vec![SwitchCase { value: Expr::unset(), body: vec![] }], default: vec![] }
            }
            Syn::Set => Stmt::Set { variable: String::new(), value: Expr::unset(), index: None },
            Syn::Wait => Stmt::Wait(Expr::fixed(1.0)),
            Syn::Break => Stmt::Break,
            Syn::Continue => Stmt::Continue,
            Syn::Return => Stmt::Return,
            Syn::ReturnValue => Stmt::ReturnValue(Expr::unset()),
            Syn::Comment => Stmt::Comment(String::new()),
            Syn::Raw => Stmt::Raw(String::new()),
            Syn::Compare | Syn::And | Syn::Or | Syn::Not | Syn::Arith => return None,
        },
    })
}

/// Floating "add" picker. Returns the insert point and the choice made.
fn picker_ui(ctx: &egui::Context, state: &mut EditorState, item: TriggerId) -> Option<(InsertAt, Choice)> {
    let tk = theme::tokens(ctx);
    let picker = state.ui.triggers.picker.as_ref()?;
    if picker.item != item {
        state.ui.triggers.picker = None;
        return None;
    }
    let is_function = state.doc.triggers.function(item).is_some();
    let want = match picker.purpose {
        Purpose::Event => Want::Event,
        Purpose::Condition => Want::Condition,
        Purpose::Action => Want::Action { function: is_function },
    };
    let title = match picker.purpose {
        Purpose::Event => "Add event",
        Purpose::Condition => "Add condition",
        Purpose::Action => "Add action",
    };
    let pos = picker.pos;
    let fresh = picker.fresh;
    let id = Id::new(("trigger-picker", item));
    if fresh {
        ctx.data_mut(|d| {
            d.remove::<String>(id.with("q"));
            d.remove::<usize>(id.with("sel"));
            d.remove::<bool>(id.with("focused"));
        });
    }
    let screen = ctx.content_rect();
    let pos = egui::pos2(pos.x.min(screen.right() - 440.0).max(screen.left()), pos.y.min(screen.bottom() - 420.0).max(screen.top()));
    let mut picked = None;
    let mut close = false;
    let area = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.set_width(420.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("Close").clicked() {
                        close = true;
                    }
                });
            });
            let EditorState { doc, ui: panels, .. } = &mut *state;
            let cx = Cx {
                ctx: ctx.clone(),
                tk,
                doc,
                item,
                vars: Vec::new(),
                is_function,
                returns: None,
                cursor: None,
                collapsed: &mut panels.triggers.collapsed,
                lines: Vec::new(),
                ops: Vec::new(),
                open_picker: None,
                new_locals: Vec::new(),
                texts: Vec::new(),
                coalesce: None,
                label: None,
                clicked_line: None,
                cursor_rect: None,
            };
            picked = choice_list(ui, &cx, &want, id, true);
            ui.label(RichText::new("Up and down to choose, Enter to add, Esc to close").color(tk.text_dim).small());
        });
    });
    let rect = area.response.rect;
    let p = state.ui.triggers.picker.as_mut().expect("checked");
    p.fresh = false;
    let outside = ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|pt| !rect.contains(pt)));
    if close || ctx.input(|i| i.key_pressed(Key::Escape)) || (outside && p.rect.is_some()) {
        state.ui.triggers.picker = None;
        return None;
    }
    p.rect = Some(rect);
    let choice = picked?;
    let at = p.at.clone();
    state.ui.triggers.picker = None;
    Some((at, choice))
}

// ---------------------------------------------------------------------------
// Line operations
// ---------------------------------------------------------------------------

fn list_at<'a>(actions: &'a mut Vec<Stmt>, list_path: &[usize]) -> Option<&'a mut Vec<Stmt>> {
    let mut list = actions;
    let mut i = 0;
    while i + 1 < list_path.len() {
        let s = list.get_mut(list_path[i])?;
        let block = list_path[i + 1];
        list = s.children_mut().into_iter().nth(block)?;
        i += 2;
    }
    Some(list)
}

/// Where Ctrl+Enter / Add inserts for the cursor's section.
fn insert_point(work: &Work, cursor: Option<&LineRef>, section: Section) -> Option<(Purpose, InsertAt)> {
    match (work, section) {
        (Work::Trigger(t), Section::Events) => {
            let i = match cursor {
                Some(LineRef::Event(i)) => i + 1,
                _ => t.events.len(),
            };
            Some((Purpose::Event, InsertAt::Event(i)))
        }
        (Work::Trigger(t), Section::Conditions) => {
            let i = match cursor {
                Some(LineRef::Condition(i)) => i + 1,
                _ => t.conditions.len(),
            };
            Some((Purpose::Condition, InsertAt::Condition(i)))
        }
        (_, Section::Actions) => {
            let at = match cursor {
                Some(LineRef::Action(p)) if !p.is_empty() => InsertAt::Action(p[..p.len() - 1].to_vec(), p[p.len() - 1] + 1),
                _ => InsertAt::Action(
                    vec![],
                    match work {
                        Work::Trigger(t) => t.actions.len(),
                        Work::Function(f) => f.actions.len(),
                    },
                ),
            };
            Some((Purpose::Action, at))
        }
        (_, Section::Locals) => None,
        _ => Some((Purpose::Action, InsertAt::Action(vec![], 0))),
    }
}

fn insert_choice(work: &mut Work, at: &InsertAt, choice: &Choice, lib: &TriggerLibrary) -> Option<LineRef> {
    match (at, work) {
        (InsertAt::Event(i), Work::Trigger(t)) => {
            let Choice::Lib(f) = choice else { return None };
            let i = (*i).min(t.events.len());
            t.events.insert(i, f.new_call());
            Some(LineRef::Event(i))
        }
        (InsertAt::Condition(i), Work::Trigger(t)) => {
            let e = choice_expr(choice, lib)?;
            let i = (*i).min(t.conditions.len());
            t.conditions.insert(i, e);
            Some(LineRef::Condition(i))
        }
        (InsertAt::Action(list_path, i), work) => {
            let s = choice_stmt(choice, lib)?;
            let list = list_at(work.actions(), list_path)?;
            let i = (*i).min(list.len());
            list.insert(i, s);
            let mut p = list_path.clone();
            p.push(i);
            Some(LineRef::Action(p))
        }
        _ => None,
    }
}

fn vec_op<T: Clone>(v: &mut Vec<T>, i: usize, op: LineOp) -> Option<usize> {
    if i >= v.len() {
        return None;
    }
    match op {
        LineOp::Delete => {
            v.remove(i);
            Some(i.min(v.len().saturating_sub(1)))
        }
        LineOp::Duplicate => {
            let c = v[i].clone();
            v.insert(i + 1, c);
            Some(i + 1)
        }
        LineOp::MoveUp if i > 0 => {
            v.swap(i, i - 1);
            Some(i - 1)
        }
        LineOp::MoveDown if i + 1 < v.len() => {
            v.swap(i, i + 1);
            Some(i + 1)
        }
        _ => Some(i),
    }
}

fn line_op(work: &mut Work, line: &LineRef, op: LineOp) -> Option<LineRef> {
    match line {
        LineRef::Event(i) => match work {
            Work::Trigger(t) => vec_op(&mut t.events, *i, op).map(LineRef::Event),
            _ => None,
        },
        LineRef::Condition(i) => match work {
            Work::Trigger(t) => vec_op(&mut t.conditions, *i, op).map(LineRef::Condition),
            _ => None,
        },
        LineRef::Param(i) => match work {
            Work::Function(f) => vec_op(&mut f.params, *i, op).map(LineRef::Param),
            _ => None,
        },
        LineRef::Local(i) => vec_op(work.locals(), *i, op).map(LineRef::Local),
        LineRef::Action(path) => {
            let (last, parent) = path.split_last()?;
            let list = list_at(work.actions(), parent)?;
            if op == LineOp::ToggleDisabled {
                let s = list.get_mut(*last)?;
                *s = match std::mem::replace(s, Stmt::Break) {
                    Stmt::Disabled(inner) => *inner,
                    other => Stmt::Disabled(Box::new(other)),
                };
                return Some(line.clone());
            }
            let n = vec_op(list, *last, op)?;
            let mut p = parent.to_vec();
            if !list.is_empty() {
                p.push(n);
                Some(LineRef::Action(p))
            } else {
                None
            }
        }
    }
}

fn copy_line(work: &mut Work, line: &LineRef) -> Option<Clip> {
    Some(match (line, &mut *work) {
        (LineRef::Event(i), Work::Trigger(t)) => Clip::Events(vec![t.events.get(*i)?.clone()]),
        (LineRef::Condition(i), Work::Trigger(t)) => Clip::Conditions(vec![t.conditions.get(*i)?.clone()]),
        (LineRef::Param(i), Work::Function(f)) => Clip::Variables(vec![f.params.get(*i)?.clone()]),
        (LineRef::Local(i), w) => Clip::Variables(vec![w.locals().get(*i)?.clone()]),
        (LineRef::Action(p), w) => {
            let (last, parent) = p.split_last()?;
            Clip::Actions(vec![list_at(w.actions(), parent)?.get(*last)?.clone()])
        }
        _ => return None,
    })
}

fn paste_clip(work: &mut Work, cursor: Option<&LineRef>, clip: Clip) -> Option<LineRef> {
    match clip {
        Clip::Events(evs) => {
            let Work::Trigger(t) = work else { return None };
            let mut i = match cursor {
                Some(LineRef::Event(i)) => i + 1,
                _ => t.events.len(),
            };
            for e in evs {
                t.events.insert(i.min(t.events.len()), e);
                i += 1;
            }
            Some(LineRef::Event(i - 1))
        }
        Clip::Conditions(cs) => {
            let Work::Trigger(t) = work else { return None };
            let mut i = match cursor {
                Some(LineRef::Condition(i)) => i + 1,
                _ => t.conditions.len(),
            };
            for c in cs {
                t.conditions.insert(i.min(t.conditions.len()), c);
                i += 1;
            }
            Some(LineRef::Condition(i - 1))
        }
        Clip::Variables(vs) => {
            let locals = work.locals();
            let mut i = match cursor {
                Some(LineRef::Local(i)) => i + 1,
                _ => locals.len(),
            };
            for v in vs {
                locals.insert(i.min(locals.len()), v);
                i += 1;
            }
            Some(LineRef::Local(i - 1))
        }
        Clip::Actions(ss) => {
            let (parent, mut i) = match cursor {
                Some(LineRef::Action(p)) if !p.is_empty() => (p[..p.len() - 1].to_vec(), p[p.len() - 1] + 1),
                _ => (vec![], work.actions().len()),
            };
            let list = list_at(work.actions(), &parent)?;
            for s in ss {
                list.insert(i.min(list.len()), s);
                i += 1;
            }
            let mut p = parent;
            p.push(i - 1);
            Some(LineRef::Action(p))
        }
        Clip::Items(_) => None,
    }
}

// ---------------------------------------------------------------------------
// Custom script
// ---------------------------------------------------------------------------

fn custom_script_ui(ui: &mut Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let original = state.doc.triggers.custom_script.clone();
    let mut text = original.clone();
    ui.label(
        RichText::new("Galaxy code placed after the global variables and before custom functions and triggers. Triggers can call anything defined here.")
            .color(tk.text_dim),
    );
    // Lint the custom script on its own; names the generated script defines are fine.
    let generated: HashSet<String> = state
        .doc
        .triggers
        .functions
        .iter()
        .map(galaxy::function_name)
        .chain(state.doc.triggers.triggers().iter().map(|t| format!("gt_{}_Func", galaxy::ident(&t.name))))
        .chain(["InitLibs", "InitGlobals", "InitTriggers", "InitMap"].map(String::from))
        .collect();
    let diags: Vec<galaxy::Diagnostic> = galaxy::lint(&text)
        .into_iter()
        .filter(|d| !(d.message.starts_with("Unknown function") && generated.iter().any(|g| d.message.contains(&format!("\"{g}\"")))))
        .collect();
    if !diags.is_empty() {
        egui::Panel::bottom("custom-script-lint").resizable(true).default_size(90.0).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for d in &diags {
                    let color = if d.severity == galaxy::LintSeverity::Error { tk.error } else { tk.warn };
                    ui.label(RichText::new(format!("Line {}:{}  {}", d.line, d.col, d.message)).color(color));
                }
            });
        });
    }
    let dark = ui.visuals().dark_mode;
    let cache = &mut state.ui.triggers.script_job;
    let ctx = ui.ctx().clone();
    let mut layouter = |ui: &Ui, buf: &dyn egui::TextBuffer, _w: f32| {
        let s = buf.as_str();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        s.hash(&mut h);
        let key = h.finish();
        if cache.as_ref().is_none_or(|(k, d, _)| *k != key || *d != dark) {
            *cache = Some((key, dark, crate::panels::script::highlight_galaxy(&ctx, s)));
        }
        let job = cache.as_ref().expect("filled").2.clone();
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let mut r = None;
    egui::ScrollArea::both().id_salt("custom-script").auto_shrink([false, false]).show(ui, |ui| {
        r = Some(
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .code_editor()
                    .hint_text("// Write Galaxy here, e.g. helper functions or constants.")
                    .desired_width(f32::INFINITY)
                    .desired_rows(24)
                    .lock_focus(true)
                    .layouter(&mut layouter)
                    .id(Id::new("custom-script-text")),
            ),
        );
    });
    if text != original {
        let mut lib = state.doc.triggers.clone();
        lib.custom_script = text;
        commit(state, "Edit custom script".into(), lib, None, r.map(|r| r.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Work {
        let mut t = Trigger::new(TriggerId(1), "T");
        t.actions.push(Stmt::If { cond: Expr::unset(), then: vec![Stmt::Break, Stmt::Continue], otherwise: vec![] });
        t.actions.push(Stmt::Return);
        Work::Trigger(t)
    }

    #[test]
    fn nested_line_ops() {
        let mut w = sample();
        // Move the nested Continue up.
        let c = line_op(&mut w, &LineRef::Action(vec![0, 0, 1]), LineOp::MoveUp).unwrap();
        assert_eq!(c, LineRef::Action(vec![0, 0, 0]));
        let Work::Trigger(t) = &w else { unreachable!() };
        assert!(matches!(&t.actions[0], Stmt::If { then, .. } if then[0] == Stmt::Continue));
        // Disable and re-enable.
        line_op(&mut w, &LineRef::Action(vec![1]), LineOp::ToggleDisabled);
        let Work::Trigger(t) = &w else { unreachable!() };
        assert!(matches!(t.actions[1], Stmt::Disabled(_)));
        line_op(&mut w, &LineRef::Action(vec![1]), LineOp::ToggleDisabled);
        let Work::Trigger(t) = &w else { unreachable!() };
        assert_eq!(t.actions[1], Stmt::Return);
    }

    #[test]
    fn copy_paste_roundtrip() {
        let mut w = sample();
        let clip = copy_line(&mut w, &LineRef::Action(vec![0, 0, 0])).unwrap();
        let text = clip_text(clip);
        let back = parse_clip(&text).unwrap();
        let at = paste_clip(&mut w, Some(&LineRef::Action(vec![1])), back).unwrap();
        assert_eq!(at, LineRef::Action(vec![2]));
        let Work::Trigger(t) = &w else { unreachable!() };
        assert_eq!(t.actions[2], Stmt::Break);
        assert!(parse_clip("not json").is_none());
    }

    /// Render the panels headlessly over every statement kind to catch panics.
    #[test]
    fn panels_render_every_kind() {
        let mut doc = MapDocument::new("Smoke", 64, 64);
        let id = doc.triggers.items[0].id();
        let lib = doc.triggers.clone();
        let mut work = Work::Trigger(doc.triggers.trigger(id).unwrap().clone());
        let syns = [
            Syn::If,
            Syn::While,
            Syn::For,
            Syn::ForEachUnit,
            Syn::ForEachPlayer,
            Syn::Switch,
            Syn::Set,
            Syn::Wait,
            Syn::Break,
            Syn::Continue,
            Syn::Return,
            Syn::Comment,
            Syn::Raw,
        ];
        for s in syns {
            insert_choice(&mut work, &InsertAt::Action(vec![], 0), &Choice::Syn(s), &lib).unwrap();
        }
        for f in trigger_lib::all().iter().take(400) {
            let at = match f.kind {
                FnKind::Event => InsertAt::Event(0),
                FnKind::Condition => InsertAt::Condition(0),
                _ => InsertAt::Action(vec![0, 0], 0),
            };
            let _ = insert_choice(&mut work, &at, &Choice::Lib(f), &lib);
        }
        insert_choice(&mut work, &InsertAt::Condition(0), &Choice::Syn(Syn::Compare), &lib).unwrap();
        work.locals().push(Variable::new("Count", GalaxyType::Int));
        work.put(&mut doc.triggers);
        doc.triggers.variables.push(Variable::new("Score", GalaxyType::Fixed));
        doc.triggers.custom_script = "void helper () {\n    UnitKill(null);\n}\n".into();

        let mut state = EditorState::new(doc);
        state.selection.trigger = Some(id);
        let ctx = egui::Context::default();
        let frame = |state: &mut EditorState| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| {
                show(ui, state);
                crate::panels::script::show(ui, state);
            });
            out.textures_delta.clear();
        };
        frame(&mut state);
        state.ui.triggers.picker = Some(Picker {
            purpose: Purpose::Action,
            at: InsertAt::Action(vec![], 0),
            item: id,
            pos: egui::Pos2::ZERO,
            fresh: true,
            rect: None,
        });
        frame(&mut state);
        frame(&mut state);
        state.ui.triggers.tab = RightTab::Script;
        frame(&mut state);
        state.ui.triggers.tab = RightTab::Editor;
        state.ui.triggers.selected_var = Some(0);
        frame(&mut state);
        // Nothing above is an edit, so history must be untouched.
        assert!(!state.history.can_undo());
    }

    /// New trigger opens an inline rename box. Drawing it used to take the
    /// egui context lock twice, which hung the native app and panicked on wasm.
    #[test]
    fn new_trigger_rename_box_draws() {
        let mut state = EditorState::new(MapDocument::new("Rename", 64, 64));
        let ctx = egui::Context::default();
        let run = |state: &mut EditorState| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| show(ui, state));
            out.textures_delta.clear();
        };
        run(&mut state);
        apply_tree_actions(&ctx, &mut state, vec![TreeAction::NewTrigger]);
        assert!(state.ui.triggers.rename.is_some());
        for _ in 0..3 {
            run(&mut state);
        }
        assert!(state.doc.triggers.triggers().iter().any(|t| t.name.starts_with("New trigger")));
    }

    fn key(k: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key { key: k, physical_key: None, pressed: true, repeat: false, modifiers }
    }

    /// Drive the editor with keyboard input: pick from the library, then
    /// duplicate, move and delete lines.
    #[test]
    fn keyboard_flow() {
        let doc = MapDocument::new("Keys", 64, 64);
        let id = doc.triggers.items[0].id();
        let mut state = EditorState::new(doc);
        state.selection.trigger = Some(id);
        let ctx = egui::Context::default();
        let run = |state: &mut EditorState, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1400.0, 900.0))),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| show(ui, state));
            out.textures_delta.clear();
        };
        let actions = |state: &EditorState| state.doc.triggers.trigger(id).unwrap().actions.clone();
        run(&mut state, vec![]);
        state.ui.triggers.focus = Focus::Editor;
        state.ui.triggers.cursor = Some((id, LineRef::Action(vec![3])));
        // Ctrl+Enter opens the picker after the cursor; type and press Enter.
        run(&mut state, vec![key(Key::Enter, Modifiers::COMMAND)]);
        assert!(state.ui.triggers.picker.is_some());
        run(&mut state, vec![]);
        run(&mut state, vec![egui::Event::Text("wait".into())]);
        run(&mut state, vec![key(Key::Enter, Modifiers::NONE)]);
        run(&mut state, vec![]);
        let a = actions(&state);
        assert_eq!(a.len(), 5, "{a:?}");
        assert!(matches!(a[4], Stmt::Wait(_)), "{a:?}");
        assert_eq!(state.ui.triggers.cursor, Some((id, LineRef::Action(vec![4]))));
        // Duplicate, move up, delete.
        run(&mut state, vec![key(Key::D, Modifiers::COMMAND)]);
        assert_eq!(actions(&state).len(), 6);
        run(&mut state, vec![key(Key::ArrowUp, Modifiers::ALT)]);
        run(&mut state, vec![key(Key::ArrowUp, Modifiers::ALT)]);
        assert!(matches!(actions(&state)[3], Stmt::Wait(_)));
        run(&mut state, vec![key(Key::Delete, Modifiers::NONE)]);
        assert_eq!(actions(&state).len(), 5);
        // Arrow keys move the cursor between lines.
        run(&mut state, vec![key(Key::ArrowUp, Modifiers::NONE)]);
        assert!(state.ui.triggers.cursor.is_some());
        // One undo entry per change. Swapping the two identical Waits changed
        // nothing, so it made no entry.
        assert_eq!(state.history.undo_entries().len(), 4);
    }

    #[test]
    fn commits_coalesce_by_key() {
        let mut state = EditorState::new(MapDocument::new("C", 32, 32));
        let key = Id::new("field");
        for name in ["A", "AB", "ABC"] {
            let mut lib = state.doc.triggers.clone();
            lib.custom_script = name.into();
            commit(&mut state, "Edit custom script".into(), lib, None, Some(key));
        }
        assert_eq!(state.history.undo_entries().len(), 1);
        assert_eq!(state.doc.triggers.custom_script, "ABC");
        state.history.undo(&mut state.doc);
        assert_eq!(state.doc.triggers.custom_script, "");
        // A different key starts a new entry.
        let mut lib = state.doc.triggers.clone();
        lib.custom_script = "x".into();
        commit(&mut state, "a".into(), lib, None, Some(key));
        let mut lib = state.doc.triggers.clone();
        lib.custom_script = "xy".into();
        commit(&mut state, "b".into(), lib, None, Some(Id::new("other")));
        assert_eq!(state.history.undo_entries().len(), 2);
    }

    #[test]
    fn picker_inserts_where_asked() {
        let lib = TriggerLibrary::default();
        let mut w = sample();
        let at = InsertAt::Action(vec![0, 1], 0);
        let c = insert_choice(&mut w, &at, &Choice::Syn(Syn::Wait), &lib).unwrap();
        assert_eq!(c, LineRef::Action(vec![0, 1, 0]));
        let hits = gather(&lib, &Want::Action { function: false }, "create unit");
        assert!(hits.iter().take(5).any(|c| matches!(c, Choice::Lib(f) if f.name == "UnitCreate")));
        let hits = gather(&lib, &Want::Value("point".into()), "");
        assert!(hits.iter().all(|c| match c {
            Choice::Lib(f) => f.ret == "point",
            _ => true,
        }));
    }
}
