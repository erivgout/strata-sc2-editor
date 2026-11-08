//! Undo history, newest first. Click an entry to go back (or forward) to it.

use egui::{Button, Color32, Key, KeyboardShortcut, Modifiers, RichText, Sense, Ui};
use strata_core::history::{HistoryEntry, Origin};

use crate::state::{AppAction, EditorState};
use crate::theme::{self, Tokens};

struct Entry {
    label: String,
    origin: Origin,
}

fn ai_badge(ui: &mut Ui, tk: &Tokens) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, tk.text_dim))
        .corner_radius(4.0)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.label(RichText::new("AI").small().color(tk.text_dim));
        })
        .response
        .on_hover_text("Made by an AI client over the AI link");
}

/// One clickable history row. Returns true when clicked.
fn row(
    ui: &mut Ui,
    tk: &Tokens,
    label: &str,
    origin: Option<Origin>,
    current: bool,
    color: Color32,
    tip: &str,
) -> bool {
    let r = ui
        .horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 16.0), Sense::hover());
            if current {
                ui.painter().circle_filled(rect.center(), 4.0, tk.accent);
            }
            let text = RichText::new(label).color(if current { tk.accent } else { color });
            let r = ui.add(
                egui::Label::new(text)
                    .truncate()
                    .sense(Sense::click())
                    .selectable(false),
            );
            if origin == Some(Origin::Mcp) {
                ai_badge(ui, tk);
            }
            r
        })
        .inner;
    let r = if current {
        r
    } else {
        r.on_hover_text(tip)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
    };
    r.clicked() && !current
}

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let undo_sc = ui
        .ctx()
        .format_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Z));
    let redo_sc = ui.ctx().format_shortcut(&KeyboardShortcut::new(
        Modifiers::COMMAND | Modifiers::SHIFT,
        Key::Z,
    ));

    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                state.history.can_undo(),
                Button::new("Undo").shortcut_text(undo_sc),
            )
            .clicked()
        {
            state.request(AppAction::Undo);
        }
        if ui
            .add_enabled(
                state.history.can_redo(),
                Button::new("Redo").shortcut_text(redo_sc),
            )
            .clicked()
        {
            state.request(AppAction::Redo);
        }
    });
    ui.add_space(4.0);

    // Labels only: entries can hold large terrain patches.
    let summarize = |es: &[HistoryEntry]| {
        es.iter()
            .map(|e| Entry {
                label: e.label.clone(),
                origin: e.origin,
            })
            .collect::<Vec<_>>()
    };
    let undo = summarize(state.history.undo_entries());
    let redo = summarize(state.history.redo_entries());
    if undo.is_empty() && redo.is_empty() {
        ui.add_space(16.0);
        ui.vertical_centered(|ui| {
            ui.label("No changes yet.");
            ui.label(
                RichText::new("Your edits, and edits from AI clients, show up here.")
                    .color(tk.text_dim),
            );
        });
        return;
    }

    let mut jump: Option<usize> = None;
    for (n, e) in undo.iter().enumerate().rev() {
        let current = n + 1 == undo.len();
        if row(
            ui,
            &tk,
            &e.label,
            Some(e.origin),
            current,
            tk.text,
            "Go back to after this change",
        ) {
            jump = Some(n + 1);
        }
    }
    if row(
        ui,
        &tk,
        "Original map",
        None,
        undo.is_empty(),
        tk.text_dim,
        "Undo every change",
    ) {
        jump = Some(0);
    }

    if !redo.is_empty() {
        ui.add_space(8.0);
        ui.label(
            RichText::new(format!("Undone ({}), click to redo", redo.len()))
                .color(tk.text_dim)
                .small(),
        );
        // The next redo is the last element; list it first.
        for (r, e) in redo.iter().enumerate().rev() {
            if row(
                ui,
                &tk,
                &e.label,
                Some(e.origin),
                false,
                tk.text_dim,
                "Redo up to this change",
            ) {
                jump = Some(undo.len() + (redo.len() - r));
            }
        }
    }

    if let Some(target) = jump {
        state.history.jump_to(&mut state.doc, target);
        state.prune_selection();
    }
}
