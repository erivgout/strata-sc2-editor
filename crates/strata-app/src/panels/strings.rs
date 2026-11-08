//! Localized text: every string key with its value per language.
//!
//! A virtualized table with one row per key. The selected row grows into
//! multiline editors, one per shown language. Line breaks are stored the SC2
//! way (`<n/>`) and shown as real line breaks while editing.

use std::collections::HashMap;

use egui::{Align, Button, Id, Key, Label, Layout, RichText, Sense, TextEdit, Ui};
use egui_extras::{Column, TableBuilder};
use strata_core::history::Edit;
use strata_core::strings::{Strings, DEFAULT_LOCALE, LOCALES};

use crate::panels::data::{commit_text, hint, TextOpts};
use crate::state::{EditorState, ToastKind};
use crate::theme;

pub struct StringsState {
    /// Languages shown as value columns.
    locales: Vec<String>,
    search: String,
    missing_only: bool,
    new_key: String,
    selected: Option<String>,
    table_focus: bool,
    scroll_to_selected: bool,
    cache_key: Option<(u64, String, bool, Vec<String>)>,
    keys: Vec<String>,
    used: HashMap<String, String>,
}

impl Default for StringsState {
    fn default() -> Self {
        Self {
            locales: vec![DEFAULT_LOCALE.to_owned()],
            search: String::new(),
            missing_only: false,
            new_key: String::new(),
            selected: None,
            table_focus: false,
            scroll_to_selected: false,
            cache_key: None,
            keys: Vec::new(),
            used: HashMap::new(),
        }
    }
}

impl StringsState {
    /// Select `key` and scroll to it (used by Data and Problems). If the key
    /// does not exist yet it is offered in the "Add key" field.
    pub fn focus_key(&mut self, key: &str) {
        self.selected = Some(key.to_owned());
        self.search.clear();
        self.missing_only = false;
        self.scroll_to_selected = true;
        self.cache_key = None;
    }

    fn refresh(&mut self, state_rev: u64, doc: &strata_core::MapDocument) {
        let key = (
            state_rev,
            self.search.clone(),
            self.missing_only,
            self.locales.clone(),
        );
        if self.cache_key.as_ref() == Some(&key) {
            return;
        }
        let s = &doc.strings;
        let mut search_locales = self.locales.clone();
        if !search_locales.iter().any(|l| l == DEFAULT_LOCALE) {
            search_locales.push(DEFAULT_LOCALE.to_owned());
        }
        let check: Vec<&String> = if self.locales.len() > 1 {
            self.locales
                .iter()
                .filter(|l| *l != DEFAULT_LOCALE)
                .collect()
        } else {
            self.locales.iter().collect()
        };
        self.keys = s
            .all_keys()
            .into_iter()
            .filter(|k| s.matches(k, &self.search, &search_locales))
            .filter(|k| {
                !self.missing_only
                    || check
                        .iter()
                        .any(|l| s.get(l, k).is_none_or(|v| v.is_empty()))
            })
            .collect();
        self.used = strata_core::validate::referenced_strings(doc)
            .into_iter()
            .collect();
        self.cache_key = Some(key);
    }
}

fn to_display(v: &str) -> String {
    v.replace("<n/>", "\n")
}

fn from_display(v: &str) -> String {
    v.replace("\r\n", "\n").replace('\n', "<n/>")
}

fn locale_name(l: &str) -> &'static str {
    match l {
        "enUS" => "English (US)",
        "deDE" => "German",
        "esES" => "Spanish (Spain)",
        "esMX" => "Spanish (Mexico)",
        "frFR" => "French",
        "itIT" => "Italian",
        "koKR" => "Korean",
        "plPL" => "Polish",
        "ptBR" => "Portuguese (Brazil)",
        "ruRU" => "Russian",
        "zhCN" => "Chinese (Simplified)",
        "zhTW" => "Chinese (Traditional)",
        _ => "",
    }
}

fn valid_key(k: &str) -> Result<(), &'static str> {
    if k.is_empty() {
        Err("Enter a key, such as Unit/Name/MyMarine.")
    } else if k.contains('=') || k.contains('\n') || k.contains('\r') {
        Err("Keys can't contain \"=\" or line breaks.")
    } else if k.trim() != k {
        Err("Keys can't start or end with spaces.")
    } else {
        Ok(())
    }
}

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let rev = state.history.revision();
    let ss = &mut state.ui.strings;
    ss.refresh(rev, &state.doc);
    let strings = &state.doc.strings;

    let mut edit: Option<(String, Strings)> = None;
    let mut toast: Option<(ToastKind, String)> = None;

    // A key requested from elsewhere that doesn't exist yet: offer to add it.
    if let Some(sel) = &ss.selected {
        if ss.scroll_to_selected && !strings.has_key(sel) {
            ss.new_key = sel.clone();
        }
    }

    // Toolbar.
    ui.horizontal(|ui| {
        let r = ui.add(
            TextEdit::singleline(&mut ss.search)
                .hint_text("Search keys and text")
                .desired_width(240.0),
        );
        if r.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            ss.search.clear();
        }
        let missing_tip = if ss.locales.len() > 1 {
            "Only show keys without text in one of the shown languages"
        } else {
            "Only show keys without text. Show another language to find missing translations."
        };
        ui.checkbox(&mut ss.missing_only, "Missing translations")
            .on_hover_text(missing_tip);
        ui.menu_button(format!("Languages ({})", ss.locales.len()), |ui| {
            for l in LOCALES {
                let mut on = ss.locales.iter().any(|x| x == l);
                let missing = strings.missing_in(l).len();
                let label = if missing > 0 && strings.locales.contains_key(l) {
                    format!("{l}  {}  ({missing} missing)", locale_name(l))
                } else {
                    format!("{l}  {}", locale_name(l))
                };
                if ui.checkbox(&mut on, label).changed() {
                    if on {
                        ss.locales.push(l.to_owned());
                        ss.locales
                            .sort_by_key(|x| LOCALES.iter().position(|y| y == x));
                    } else if ss.locales.len() > 1 {
                        ss.locales.retain(|x| x != l);
                    }
                }
            }
        });
        ui.separator();
        let add_id = Id::new("strings_new_key");
        let r = ui.add(
            TextEdit::singleline(&mut ss.new_key)
                .id(add_id)
                .hint_text("New key, e.g. Unit/Name/MyMarine")
                .desired_width(220.0),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        let k = ss.new_key.trim().to_owned();
        let exists = strings.has_key(&k);
        let ok = valid_key(&k).is_ok() && !exists;
        let b = ui.add_enabled(ok, Button::new("Add key"));
        let b = if exists {
            b.on_disabled_hover_text("That key already exists")
        } else {
            b.on_disabled_hover_text(valid_key(&k).err().unwrap_or(""))
        };
        if b.clicked() || (enter && ok) {
            let mut after = strings.clone();
            after.set(DEFAULT_LOCALE, &k, "");
            edit = Some((format!("Add text {k}"), after));
            ss.selected = Some(k);
            ss.scroll_to_selected = true;
            ss.new_key.clear();
        }
    });
    ss.refresh(rev, &state.doc);

    if strings.all_keys().is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label("This map has no text yet.");
            hint(
                ui,
                &tk,
                "Add a key above. Units, buttons and the map name all look up their text here.",
            );
        });
    } else if ss.keys.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(if ss.missing_only {
                "Every shown key has text in every shown language."
            } else {
                "No keys match your search."
            });
        });
    } else {
        let n = ss.keys.len();
        let total = strings.all_keys().len();
        hint(
            ui,
            &tk,
            if n == total {
                format!("{total} keys")
            } else {
                format!("{n} of {total} keys")
            },
        );

        // Keyboard navigation.
        let pos = ss
            .selected
            .as_ref()
            .and_then(|s| ss.keys.iter().position(|k| k == s));
        if ss.table_focus && !ui.ctx().egui_wants_keyboard_input() {
            let (up, down) =
                ui.input(|i| (i.key_pressed(Key::ArrowUp), i.key_pressed(Key::ArrowDown)));
            let t = match pos {
                Some(p) if up => Some(p.saturating_sub(1)),
                Some(p) if down => Some((p + 1).min(n - 1)),
                None if up || down => Some(0),
                _ => None,
            };
            if let Some(t) = t {
                ss.selected = Some(ss.keys[t].clone());
                ss.scroll_to_selected = true;
            }
        }
        let pos = ss
            .selected
            .as_ref()
            .and_then(|s| ss.keys.iter().position(|k| k == s));

        let locales = ss.locales.clone();
        let row_h = 22.0;
        let sel_lines = pos
            .map(|p| {
                let k = &ss.keys[p];
                locales
                    .iter()
                    .map(|l| {
                        strings
                            .get(l, k)
                            .map(|v| to_display(v).lines().count())
                            .unwrap_or(1)
                    })
                    .max()
                    .unwrap_or(1)
            })
            .unwrap_or(1);
        let sel_h = (sel_lines.clamp(2, 10) as f32) * 17.0 + 34.0;
        let mut tb = TableBuilder::new(ui)
            .id_salt("strings_table")
            .striped(true)
            .resizable(true)
            .sense(Sense::click())
            .cell_layout(Layout::left_to_right(Align::Center))
            .column(Column::initial(240.0).at_least(80.0).clip(true))
            .column(Column::initial(130.0).at_least(40.0).clip(true));
        for (n, _) in locales.iter().enumerate() {
            let c = if n + 1 == locales.len() {
                Column::remainder().at_least(160.0)
            } else {
                Column::initial(280.0).at_least(100.0)
            };
            tb = tb.column(c.clip(true));
        }
        tb = tb.column(Column::exact(110.0));
        if ss.scroll_to_selected {
            if let Some(p) = pos {
                tb = tb.scroll_to_row(p, None);
            }
            ss.scroll_to_selected = false;
        }
        let mut clicked: Option<String> = None;
        let mut delete: Option<String> = None;
        let mut copy: Option<String> = None;
        let keys = &ss.keys;
        let used = &ss.used;
        let selected = ss.selected.clone();
        let heights = keys.iter().map(|k| {
            if Some(k) == selected.as_ref() {
                sel_h
            } else {
                row_h
            }
        });
        let out = tb
            .header(22.0, |mut h| {
                h.col(|ui| {
                    ui.strong("Key");
                });
                h.col(|ui| {
                    ui.strong("Used by");
                });
                for l in &locales {
                    h.col(|ui| {
                        ui.strong(l).on_hover_text(locale_name(l));
                    });
                }
                h.col(|_| {});
            })
            .body(|body| {
                body.heterogeneous_rows(heights, |mut row| {
                    let k = &keys[row.index()];
                    let is_sel = Some(k) == selected.as_ref();
                    row.set_selected(is_sel);
                    row.col(|ui| {
                        ui.add(
                            Label::new(RichText::new(k).monospace())
                                .truncate()
                                .selectable(false),
                        );
                    });
                    row.col(|ui| {
                        let why = used.get(k).cloned().or_else(|| Strings::reference_hint(k));
                        if let Some(w) = why {
                            ui.add(
                                Label::new(RichText::new(&w).color(tk.text_dim).small())
                                    .truncate()
                                    .selectable(false),
                            )
                            .on_hover_text(w);
                        }
                    });
                    for l in &locales {
                        row.col(|ui| {
                            let v = strings.get(l, k);
                            if is_sel {
                                let cur = v.map(to_display).unwrap_or_default();
                                let o = TextOpts {
                                    multiline: true,
                                    width: ui.available_width(),
                                    rows: sel_lines.clamp(2, 10),
                                    ..Default::default()
                                };
                                let hint_text = format!("Text in {}", locale_name(l));
                                let (_, new) = commit_text(
                                    ui,
                                    Id::new(("strings_edit", k.as_str(), l.as_str())),
                                    &cur,
                                    TextOpts {
                                        hint: &hint_text,
                                        ..o
                                    },
                                );
                                if let Some(n) = new {
                                    let mut after = strings.clone();
                                    after.set(l, k, &from_display(&n));
                                    edit = Some((format!("Edit text {k}"), after));
                                }
                            } else {
                                match v {
                                    Some(v) if !v.is_empty() => {
                                        let first = to_display(v);
                                        let line = first.lines().next().unwrap_or("");
                                        let more = if first.lines().count() > 1 {
                                            " …"
                                        } else {
                                            ""
                                        };
                                        ui.add(
                                            Label::new(format!("{line}{more}"))
                                                .truncate()
                                                .selectable(false),
                                        );
                                    }
                                    _ => {
                                        let c = if l == DEFAULT_LOCALE {
                                            tk.text_dim
                                        } else {
                                            tk.warn
                                        };
                                        ui.add(
                                            Label::new(RichText::new("Missing").color(c).italics())
                                                .selectable(false),
                                        );
                                    }
                                }
                            }
                        });
                    }
                    row.col(|ui| {
                        ui.with_layout(
                            Layout::left_to_right(if is_sel { Align::Min } else { Align::Center }),
                            |ui| {
                                if ui
                                    .small_button("Copy")
                                    .on_hover_text("Copy the key")
                                    .clicked()
                                {
                                    copy = Some(k.clone());
                                }
                                if ui
                                    .small_button("Delete")
                                    .on_hover_text("Delete this key in every language")
                                    .clicked()
                                {
                                    delete = Some(k.clone());
                                }
                            },
                        );
                    });
                    if row.response().clicked() {
                        clicked = Some(k.clone());
                    }
                });
            });
        let rect = out.inner_rect;
        if let Some(c) = clicked {
            ss.selected = Some(c);
            ss.table_focus = true;
        } else if ui.input(|i| {
            i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
        }) {
            ss.table_focus = false;
        }
        if let Some(k) = copy {
            ui.ctx().copy_text(k.clone());
            toast = Some((ToastKind::Info, format!("Copied {k}")));
        }
        if let Some(k) = delete {
            let mut after = strings.clone();
            after.remove(&k);
            edit = Some((format!("Delete text {k}"), after));
            if ss.selected.as_ref() == Some(&k) {
                ss.selected = None;
            }
        }
    }

    if let Some((label, after)) = edit {
        let before = Box::new(state.doc.strings.clone());
        state.commit(
            label,
            Edit::Strings {
                before,
                after: Box::new(after),
            },
        );
    }
    if let Some((k, t)) = toast {
        state.toast(k, t);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_breaks_round_trip() {
        assert_eq!(to_display("a<n/>b"), "a\nb");
        assert_eq!(from_display("a\r\nb\nc"), "a<n/>b<n/>c");
        assert!(valid_key("Unit/Name/A").is_ok());
        assert!(valid_key("A=B").is_err());
        assert!(valid_key(" A").is_err());
    }
}
