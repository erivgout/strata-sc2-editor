//! Generated Galaxy script (read-only preview) with syntax highlighting,
//! line numbers, search, lint results and jump-to-trigger.
//!
//! [`highlight_galaxy`] is the shared Galaxy highlighter; the trigger
//! editor's custom script tab uses it too.

use std::ops::Range;

use egui::text::{CCursor, LayoutJob, TextFormat};
use egui::{Color32, FontId, Key, KeyboardShortcut, Modifiers};
use strata_core::galaxy::{self, Diagnostic, Generated, LintSeverity};
use strata_core::triggers::TriggerId;

use crate::panels::Tab;
use crate::state::{AppAction, EditorState};
use crate::theme::{self, Tokens};

pub const CODE_SIZE: f32 = 13.0;

const KEYWORDS: &[&str] = &[
    "if", "else", "while", "for", "return", "break", "continue", "const", "static", "native", "include", "struct", "typedef", "true",
    "false", "null", "do",
];

const TYPES: &[&str] = &[
    "void",
    "int",
    "fixed",
    "bool",
    "byte",
    "string",
    "text",
    "point",
    "region",
    "unit",
    "unitgroup",
    "unitref",
    "unitfilter",
    "playergroup",
    "timer",
    "trigger",
    "sound",
    "soundlink",
    "color",
    "abilcmd",
    "actor",
    "actorscope",
    "aifilter",
    "bank",
    "bitmask",
    "camerainfo",
    "datetime",
    "doodad",
    "effecthistory",
    "generichandle",
    "marker",
    "order",
    "revealer",
    "transmissionsource",
    "wave",
    "waveinfo",
    "wavetarget",
    "handle",
    "arrayref",
    "structref",
    "funcref",
];

/// Syntax colors derived from the theme tokens.
#[derive(Clone, Copy)]
struct Palette {
    text: Color32,
    comment: Color32,
    keyword: Color32,
    ty: Color32,
    string: Color32,
    number: Color32,
    constant: Color32,
    call: Color32,
}

fn palette(tk: &Tokens) -> Palette {
    Palette {
        text: tk.text,
        comment: tk.text_dim,
        keyword: tk.text.lerp_to_gamma(tk.accent, 0.85),
        ty: tk.text.lerp_to_gamma(tk.accent, 0.55),
        string: tk.ok,
        number: tk.warn,
        constant: tk.text.lerp_to_gamma(tk.warn, 0.6),
        call: tk.text.lerp_to_gamma(tk.ok, 0.25),
    }
}

/// Highlight Galaxy source as a monospace [`LayoutJob`] (no wrapping).
pub fn highlight_galaxy(ctx: &egui::Context, text: &str) -> LayoutJob {
    highlight_galaxy_marked(ctx, text, &[])
}

/// Like [`highlight_galaxy`], with extra background marks (byte ranges), e.g.
/// search matches. Marks must not overlap.
pub fn highlight_galaxy_marked(ctx: &egui::Context, text: &str, marks: &[(Range<usize>, Color32)]) -> LayoutJob {
    let tk = theme::tokens(ctx);
    let pal = palette(&tk);
    let font = FontId::monospace(CODE_SIZE);
    let mut job = LayoutJob { text: String::new(), ..Default::default() };
    job.wrap.max_width = f32::INFINITY;
    let mut spans: Vec<(Range<usize>, Color32)> = Vec::new();
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let start = i;
        let color = if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            pal.comment
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len());
            pal.comment
        } else if c == b'"' || c == b'\'' {
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            pal.string
        } else if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            pal.number
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            let mut j = i;
            while j < b.len() && b[j] == b' ' {
                j += 1;
            }
            if KEYWORDS.contains(&word) {
                pal.keyword
            } else if TYPES.contains(&word) {
                pal.ty
            } else if word.starts_with("c_") || word.contains("_ge_") {
                pal.constant
            } else if b.get(j) == Some(&b'(') {
                pal.call
            } else {
                pal.text
            }
        } else {
            // Advance one UTF-8 character.
            i += 1;
            while i < b.len() && (b[i] & 0xC0) == 0x80 {
                i += 1;
            }
            pal.text
        };
        match spans.last_mut() {
            Some((r, col)) if *col == color && r.end == start => r.end = i,
            _ => spans.push((start..i, color)),
        }
    }
    job.text = text.to_owned();
    // Split spans at mark boundaries.
    let mut marks: Vec<&(Range<usize>, Color32)> = marks.iter().filter(|(r, _)| r.start < r.end && r.end <= text.len()).collect();
    marks.sort_by_key(|(r, _)| r.start);
    let mut cuts: Vec<usize> = marks.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
    cuts.sort_unstable();
    for (range, color) in spans {
        let mut s = range.start;
        let inner: Vec<usize> = cuts.iter().copied().filter(|c| *c > range.start && *c < range.end).collect();
        for e in inner.into_iter().chain(std::iter::once(range.end)) {
            let bg = marks.iter().find(|(r, _)| r.start <= s && s < r.end).map(|(_, c)| *c).unwrap_or(Color32::TRANSPARENT);
            job.sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: egui::text::ByteIndex(s)..egui::text::ByteIndex(e),
                format: TextFormat { font_id: font.clone(), color, background: bg, ..Default::default() },
            });
            s = e;
        }
    }
    job
}

struct Cache {
    key: (u64, u32, usize, usize, String),
    gen: Generated,
    lint: Vec<Diagnostic>,
    /// Byte offset of each line start.
    line_starts: Vec<usize>,
    job_key: Option<(bool, String, Option<usize>, Option<usize>)>,
    job: LayoutJob,
    numbers: String,
}

#[derive(Default)]
pub struct ScriptState {
    cache: Option<Cache>,
    search: String,
    focus_search: bool,
    /// Index into the current match list.
    match_index: usize,
    /// Scroll so this byte offset is visible next frame.
    scroll_to: Option<usize>,
    /// Line (1-based) to flash, e.g. after clicking a diagnostic.
    marked_line: Option<usize>,
    show_problems: bool,
    /// Line (1-based) of the text cursor in the preview.
    cursor_line: Option<usize>,
    /// Trigger to scroll to once the script is generated.
    pending_reveal: Option<TriggerId>,
}

impl ScriptState {
    /// Show the generated code for a trigger (used by the trigger editor).
    pub fn reveal(&mut self, id: TriggerId) {
        self.pending_reveal = Some(id);
    }
}

fn cache_key(state: &EditorState) -> (u64, u32, usize, usize, String) {
    let lib = &state.doc.triggers;
    (state.history.revision(), lib.next_id, lib.items.len(), lib.variables.len(), state.doc.info.name.clone())
}

fn matches(text: &str, query: &str) -> Vec<Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let lower = text.to_ascii_lowercase();
    let q = query.to_ascii_lowercase();
    lower.match_indices(&q).map(|(i, m)| i..i + m.len()).collect()
}

fn line_of(starts: &[usize], byte: usize) -> usize {
    match starts.binary_search(&byte) {
        Ok(i) => i + 1,
        Err(i) => i,
    }
}

pub fn show(ui: &mut egui::Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let key = cache_key(state);
    if state.ui.script.cache.as_ref().is_none_or(|c| c.key != key) {
        let gen = galaxy::generate_full(&state.doc);
        let lint = galaxy::lint(&gen.script);
        let line_starts: Vec<usize> =
            std::iter::once(0).chain(gen.script.match_indices('\n').map(|(i, _)| i + 1)).filter(|i| *i <= gen.script.len()).collect();
        let n = gen.script.lines().count().max(1);
        let w = n.to_string().len();
        let numbers = (1..=n).map(|i| format!("{i:>w$}")).collect::<Vec<_>>().join("\n");
        state.ui.script.cache = Some(Cache { key, gen, lint, line_starts, job_key: None, job: LayoutJob::default(), numbers });
    }

    let ctx = ui.ctx().clone();
    let st = &mut state.ui.script;
    let panel_rect = ui.max_rect();
    let hovered = ui.rect_contains_pointer(panel_rect);
    if hovered && ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F))) {
        st.focus_search = true;
    }

    let cache = st.cache.as_mut().expect("cache filled above");
    if let Some(id) = st.pending_reveal.take() {
        if let Some(line) = cache.gen.line_of(id) {
            st.scroll_to = cache.line_starts.get(line - 1).copied();
            st.marked_line = Some(line);
        }
    }
    let found = matches(&cache.gen.script, &st.search);
    if st.match_index >= found.len() {
        st.match_index = 0;
    }

    let mut jump: Option<TriggerId> = None;
    // Toolbar. Wraps in narrow docks so the lint summary is never clipped.
    ui.horizontal_wrapped(|ui| {
        if ui.button("Copy all").on_hover_text("Copy the whole script to the clipboard").clicked() {
            ui.ctx().copy_text(cache.gen.script.clone());
        }
        ui.separator();
        let search = ui.add(
            egui::TextEdit::singleline(&mut st.search).hint_text("Find in script (Ctrl+F)").desired_width(200.0).id_salt("script-search"),
        );
        if st.focus_search {
            search.request_focus();
            st.focus_search = false;
        }
        if search.changed() {
            st.match_index = 0;
            if let Some(r) = matches(&cache.gen.script, &st.search).first() {
                st.scroll_to = Some(r.start);
            }
        }
        let enter = search.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if !found.is_empty() {
            ui.label(egui::RichText::new(format!("{} of {}", st.match_index + 1, found.len())).color(tk.text_dim));
            let back = ui.small_button("Previous").clicked() || (enter && ui.input(|i| i.modifiers.shift));
            let next = ui.small_button("Next").clicked() || (enter && !ui.input(|i| i.modifiers.shift));
            if back {
                st.match_index = (st.match_index + found.len() - 1) % found.len();
            } else if next {
                st.match_index = (st.match_index + 1) % found.len();
            }
            if back || next {
                st.scroll_to = Some(found[st.match_index].start);
                search.request_focus();
            }
        } else if !st.search.is_empty() {
            ui.label(egui::RichText::new("No matches").color(tk.text_dim));
        }
        ui.separator();
        let errors = cache.lint.iter().filter(|d| d.severity == LintSeverity::Error).count();
        let warnings = cache.lint.len() - errors;
        let summary = match (errors, warnings) {
            (0, 0) => egui::RichText::new("No lint problems").color(tk.ok),
            (0, w) => egui::RichText::new(format!("{w} warnings")).color(tk.warn),
            (e, w) => egui::RichText::new(format!("{e} errors, {w} warnings")).color(tk.error),
        };
        if ui.selectable_label(st.show_problems, summary).on_hover_text("Show lint results").clicked() {
            st.show_problems = !st.show_problems;
        }
        if let Some(line) = st.cursor_line {
            if let Some(id) = cache.gen.item_at_line(line) {
                if let Some((_, _, name)) = cache.gen.sections.iter().find(|(_, i, _)| *i == id) {
                    ui.separator();
                    if ui.button(format!("Open {name}")).on_hover_text("Show this in the trigger editor").clicked() {
                        jump = Some(id);
                    }
                }
            }
        }
    });

    if st.show_problems && !cache.lint.is_empty() {
        egui::Panel::bottom("script-lint").resizable(true).default_size(120.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("script-lint-scroll").auto_shrink([false, false]).show(ui, |ui| {
                for d in &cache.lint {
                    let color = if d.severity == LintSeverity::Error { tk.error } else { tk.warn };
                    let text = egui::RichText::new(format!("Line {}:{}  {}", d.line, d.col, d.message)).color(color);
                    if ui.add(egui::Label::new(text).sense(egui::Sense::click())).on_hover_text("Go to line").clicked() {
                        st.scroll_to = cache.line_starts.get(d.line - 1).copied();
                        st.marked_line = Some(d.line);
                    }
                }
            });
        });
    }

    // Build (or reuse) the highlighted layout.
    let current = found.get(st.match_index).cloned();
    let job_key = (ctx.global_style().visuals.dark_mode, st.search.clone(), st.marked_line, current.as_ref().map(|r| r.start));
    if cache.job_key.as_ref() != Some(&job_key) {
        let mut marks: Vec<(Range<usize>, Color32)> = Vec::new();
        if let Some(l) = st.marked_line {
            if let (Some(&s), e) = (cache.line_starts.get(l - 1), cache.line_starts.get(l).copied()) {
                let e = e.map(|e| e.saturating_sub(1)).unwrap_or(cache.gen.script.len());
                if e > s {
                    marks.push((s..e, tk.warn.gamma_multiply(0.18)));
                }
            }
        }
        for (n, r) in found.iter().enumerate() {
            if marks.iter().any(|(m, _)| m.start < r.end && r.start < m.end) {
                continue;
            }
            let strength = if Some(n) == Some(st.match_index) { 0.55 } else { 0.25 };
            marks.push((r.clone(), tk.accent.gamma_multiply(strength)));
        }
        cache.job = highlight_galaxy_marked(&ctx, &cache.gen.script, &marks);
        cache.job_key = Some(job_key);
    }

    let scroll_to = st.scroll_to.take();
    egui::ScrollArea::both().id_salt("script-code").auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let numbers = egui::RichText::new(&cache.numbers).font(FontId::monospace(CODE_SIZE)).color(tk.text_dim);
            ui.add(egui::Label::new(numbers).selectable(false).extend());
            let job = cache.job.clone();
            let mut layouter = |ui: &egui::Ui, _buf: &dyn egui::TextBuffer, _w: f32| ui.fonts_mut(|f| f.layout_job(job.clone()));
            let mut text: &str = cache.gen.script.as_str();
            let out = egui::TextEdit::multiline(&mut text)
                .id_salt("script-code-text")
                .code_editor()
                .frame(egui::Frame::NONE)
                .desired_width(f32::INFINITY)
                .layouter(&mut layouter)
                .show(ui);
            if let Some(range) = out.cursor_range {
                let ci = range.primary.index.0;
                let byte = cache.gen.script.char_indices().nth(ci).map_or(cache.gen.script.len(), |(b, _)| b);
                st.cursor_line = Some(line_of(&cache.line_starts, byte));
            }
            if let Some(byte) = scroll_to {
                let chars = cache.gen.script[..byte.min(cache.gen.script.len())].chars().count();
                let r = out.galley.pos_from_cursor(CCursor::new(chars)).translate(out.galley_pos.to_vec2());
                ui.scroll_to_rect(r.expand2(egui::vec2(0.0, 40.0)), Some(egui::Align::Center));
            }
        });
    });

    if let Some(id) = jump {
        state.selection.trigger = Some(id);
        state.request(AppAction::FocusTab(Tab::Triggers));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlight_covers_text() {
        let ctx = egui::Context::default();
        let src = "// hi\nint gv_x = 1;\nvoid f () {\n    UnitKill(\"é\");\n}\n";
        let job = highlight_galaxy_marked(&ctx, src, &[(7..10, Color32::RED)]);
        let mut pos = 0;
        for s in &job.sections {
            assert_eq!(s.byte_range.start.0, pos);
            pos = s.byte_range.end.0;
        }
        assert_eq!(pos, src.len());
        assert!(job.sections.iter().any(|s| s.format.background == Color32::RED));
    }
}
