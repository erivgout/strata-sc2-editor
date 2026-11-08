//! Errors and warnings in this map. Click one to jump to what it is about.

use egui::{Color32, Label, RichText, Sense, Ui};
use strata_core::validate::{Problem, Severity, Target};

use crate::panels::Tab;
use crate::state::{AppAction, EditorState};
use crate::theme::{self, Tokens};

#[derive(Default)]
pub struct ProblemsState {
    /// Index of the last problem the user jumped to, for highlighting.
    selected: Option<usize>,
}

const GROUPS: [(Severity, &str, &str); 3] = [
    (Severity::Error, "error", "errors"),
    (Severity::Warning, "warning", "warnings"),
    (Severity::Info, "note", "notes"),
];

fn color(tk: &Tokens, s: Severity) -> Color32 {
    match s {
        Severity::Error => tk.error,
        Severity::Warning => tk.warn,
        Severity::Info => tk.text_dim,
    }
}

fn marker(ui: &mut Ui, c: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 14.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, c);
}

fn check_mark(ui: &mut Ui, c: Color32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(28.0, 28.0), Sense::hover());
    let p = |x: f32, y: f32| rect.lerp_inside(egui::vec2(x, y));
    let stroke = egui::Stroke::new(2.5, c);
    ui.painter()
        .line_segment([p(0.18, 0.52), p(0.42, 0.76)], stroke);
    ui.painter()
        .line_segment([p(0.42, 0.76), p(0.84, 0.26)], stroke);
}

fn target_hint(state: &EditorState, t: &Target) -> String {
    match t {
        Target::Map => "Map properties".into(),
        Target::Object(id) => state
            .doc
            .objects
            .get(*id)
            .map(|o| format!("Object {}", o.display_name()))
            .unwrap_or_else(|| "Object".into()),
        Target::Region(id) => state
            .doc
            .regions
            .iter()
            .find(|r| r.id == *id)
            .map(|r| format!("Region {}", r.name))
            .unwrap_or_else(|| "Region".into()),
        Target::Trigger(name) => format!("Trigger {name}"),
        Target::Data { class, id } => format!("{class} {id}"),
        Target::String(key) => format!("Text {key}"),
    }
}

/// Select what a problem points at and bring its panel forward.
pub fn navigate(state: &mut EditorState, target: &Target) {
    match target {
        Target::Map => state.request(AppAction::FocusTab(Tab::MapProps)),
        Target::Object(id) => {
            if let Some(o) = state.doc.objects.get(*id) {
                let pos = o.position.truncate();
                state.selection.objects.clear();
                state.selection.objects.insert(*id);
                state.request(AppAction::FocusTab(Tab::Viewport));
                state.request(AppAction::FrameWorld(pos));
            }
        }
        Target::Region(id) => {
            if let Some(r) = state.doc.regions.iter().find(|r| r.id == *id) {
                let center = r
                    .shapes
                    .iter()
                    .map(|s| s.bounds())
                    .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)))
                    .map(|(lo, hi)| (lo + hi) * 0.5);
                state.selection.region = Some(*id);
                state.request(AppAction::FocusTab(Tab::Viewport));
                if let Some(c) = center {
                    state.request(AppAction::FrameWorld(c));
                }
            }
        }
        Target::Trigger(name) => {
            if let Some(t) = state
                .doc
                .triggers
                .triggers()
                .into_iter()
                .find(|t| &t.name == name)
            {
                state.selection.trigger = Some(t.id);
            }
            state.request(AppAction::FocusTab(Tab::Triggers));
        }
        Target::Data { class, id } => {
            if let Some(i) = state.doc.data.index_of(class, id) {
                state.selection.data_entry = Some(i);
            }
            state.request(AppAction::FocusTab(Tab::Data));
        }
        Target::String(key) => {
            state.ui.strings.focus_key(key);
            state.request(AppAction::FocusTab(Tab::Strings));
        }
    }
}

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let problems: Vec<Problem> = state.problems().to_vec();

    if problems.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            check_mark(ui, tk.ok);
            ui.label("No problems found.");
            ui.label(RichText::new("Strata checks the map as you edit.").color(tk.text_dim));
        });
        return;
    }

    // Summary line.
    ui.horizontal(|ui| {
        for (sev, one, many) in GROUPS {
            let n = problems.iter().filter(|p| p.severity == sev).count();
            if n == 0 {
                continue;
            }
            marker(ui, color(&tk, sev));
            ui.label(format!("{n} {}", if n == 1 { one } else { many }));
            ui.add_space(8.0);
        }
    });
    ui.separator();

    let mut go: Option<(usize, Target)> = None;
    for (sev, _, many) in GROUPS {
        let items: Vec<(usize, &Problem)> = problems
            .iter()
            .enumerate()
            .filter(|(_, p)| p.severity == sev)
            .collect();
        if items.is_empty() {
            continue;
        }
        let title = format!("{} ({})", capitalize(many), items.len());
        egui::CollapsingHeader::new(RichText::new(title).strong())
            .id_salt(("problems", many))
            .default_open(true)
            .show(ui, |ui| {
                for (n, p) in items {
                    let selected = state.ui.problems.selected == Some(n);
                    let r = ui
                        .horizontal(|ui| {
                            marker(ui, color(&tk, p.severity));
                            let text = if selected {
                                RichText::new(&p.message).color(tk.accent)
                            } else {
                                RichText::new(&p.message)
                            };
                            // Message above its target so neither is clipped in narrow docks.
                            ui.vertical(|ui| {
                                let r = ui.add(
                                    Label::new(text)
                                        .wrap()
                                        .sense(Sense::click())
                                        .selectable(false),
                                );
                                ui.label(
                                    RichText::new(target_hint(state, &p.target))
                                        .color(tk.text_dim)
                                        .small(),
                                );
                                r
                            })
                            .inner
                        })
                        .inner;
                    let r = r
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text("Go to it");
                    if r.clicked() {
                        go = Some((n, p.target.clone()));
                    }
                }
            });
    }

    if let Some((n, t)) = go {
        state.ui.problems.selected = Some(n);
        navigate(state, &t);
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
