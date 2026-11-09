//! Custom models, textures and sounds packed into the map.
//!
//! The panel lists every packed file by kind with what uses it, and flags
//! paths the map's data or models need that are neither in the map nor in
//! the game. It makes custom content quick:
//! - Import files or a folder (or drop them on the window). Textures land
//!   where their models expect them; files the data is missing land where
//!   the data looks for them.
//! - New sound: a `CSound` that plays one of a few audio files at random.
//! - New unit: a playable unit with its own model, portrait, icon and voice
//!   lines, built on any unit's stats, in one dialog.
//!
//! Every change is one undoable edit. The dialogs are drawn by the app
//! shell ([`dialogs`]) so imports dropped anywhere in the window work even
//! while this panel is hidden.

use std::collections::BTreeSet;

use egui::{Align, Button, Id, Label, Layout, RichText, Sense, TextEdit, Ui};
use strata_core::catalog::{CatalogEntry, CatalogKind};
use strata_core::history::Edit;
use strata_core::map_files::{self as mf, FileKind, MapFile, UnitRecipe, Usage, User, UNIT_SOUND_SLOTS};
use strata_core::strings::DEFAULT_LOCALE;

use crate::io::ImportFiles;
use crate::panels::Tab;
use crate::state::{AppAction, EditorState, Mode, ToastKind};
use crate::theme::{self, Tokens};

#[derive(Default)]
pub struct AssetsState {
    search: String,
    cache: Option<Cache>,
    dialog: Option<Dialog>,
    /// A dialog waiting for files it asked to import (its Import… button).
    resume: Option<Dialog>,
    /// Map file key of the model whose textures are listed.
    expanded: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    player: crate::audio::Player,
}

struct Cache {
    /// (doc generation, history revision) it was built at.
    key: (u64, u64),
    files: Vec<MapFile>,
    usage: Usage,
    missing: Vec<Missing>,
    /// `missing` was checked against the open game index.
    checked_game: bool,
}

struct Missing {
    path: String,
    users: Vec<User>,
    /// In the game install: `None` until the install's index is open.
    in_game: Option<bool>,
}

enum Dialog {
    Import(ImportForm),
    Sound(SoundForm),
    Unit(Box<UnitForm>),
    Remove { path: String, users: Vec<String> },
}

struct ImportForm {
    files: Vec<(String, Vec<u8>)>,
    rows: Vec<ImportRow>,
    skipped: Vec<String>,
    /// Make a `CSound` for each audio file nothing uses yet.
    make_sounds: bool,
    /// Open the new unit dialog with the imported model afterwards.
    then_unit: bool,
}

struct ImportRow {
    source: usize,
    path: String,
    kind: FileKind,
    include: bool,
    replaces: bool,
    wanted_by: Option<String>,
}

#[derive(Default)]
struct SoundForm {
    id: String,
    files: BTreeSet<String>,
    filter: String,
}

#[derive(Clone, PartialEq)]
enum Voice {
    None,
    /// An existing `CSound` id.
    Sound(String),
    /// An audio file in the map; a `CSound` is made for it.
    File(String),
}

struct UnitForm {
    name: String,
    id: String,
    id_edited: bool,
    base: String,
    base_filter: String,
    /// Map file keys.
    model: Option<String>,
    portrait: Option<String>,
    icon: Option<String>,
    scale: f32,
    voices: Vec<Voice>,
    place_after: bool,
}

impl Default for UnitForm {
    fn default() -> Self {
        Self {
            name: String::new(),
            id: String::new(),
            id_edited: false,
            base: "Marine".into(),
            base_filter: String::new(),
            model: None,
            portrait: None,
            icon: None,
            scale: 1.0,
            voices: vec![Voice::None; UNIT_SOUND_SLOTS.len()],
            place_after: true,
        }
    }
}

impl AssetsState {
    /// A dialog of this panel is open (global shortcuts pause).
    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }
}

// ---------------------------------------------------------------------------
// Entry points for the app shell.

/// Review files picked or dropped for importing.
pub fn begin_import(state: &mut EditorState, picked: ImportFiles) {
    refresh(state);
    // Files dropped while the unit or sound dialog is open fill it in afterwards.
    if state.ui.assets.resume.is_none() && matches!(state.ui.assets.dialog, Some(Dialog::Unit(_) | Dialog::Sound(_))) {
        state.ui.assets.resume = state.ui.assets.dialog.take();
    }
    if picked.files.is_empty() {
        let what = if picked.skipped.is_empty() {
            "Nothing to import.".to_owned()
        } else {
            format!(
                "None of those can go in a map ({}). Strata imports models (.m3), textures (.dds, .tga), sounds (.mp3, .ogg, .wav) and fonts.",
                short_list(&picked.skipped)
            )
        };
        state.toast(ToastKind::Error, what);
        import_cancelled(state);
        return;
    }
    let missing: Vec<(String, String)> = state
        .ui
        .assets
        .cache
        .as_ref()
        .map(|c| {
            c.missing
                .iter()
                .filter(|m| m.in_game == Some(false))
                .map(|m| (m.path.clone(), m.users.first().map(User::label).unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    let named: Vec<(String, &[u8])> = picked.files.iter().map(|(n, b)| (n.clone(), b.as_slice())).collect();
    let plan = mf::plan_import(&state.doc, &named, None, &missing);
    let rows: Vec<ImportRow> = plan
        .into_iter()
        .map(|p| ImportRow {
            source: p.source,
            path: p.path,
            kind: p.kind,
            include: true,
            replaces: p.replaces,
            wanted_by: p.wanted_by,
        })
        .collect();
    let has_free_sounds = rows.iter().any(|r| r.kind == FileKind::Sound && r.wanted_by.is_none() && !r.replaces);
    let one_model = rows.iter().any(|r| r.kind == FileKind::Model);
    let resuming = state.ui.assets.resume.is_some();
    state.ui.assets.dialog = Some(Dialog::Import(ImportForm {
        files: picked.files,
        rows,
        skipped: picked.skipped,
        make_sounds: has_free_sounds,
        then_unit: one_model && !resuming,
    }));
}

/// The file picker was closed without choosing anything.
pub fn import_cancelled(state: &mut EditorState) {
    if let Some(d) = state.ui.assets.resume.take() {
        state.ui.assets.dialog = Some(d);
    }
}

pub fn open_new_unit(state: &mut EditorState, model: Option<String>) {
    let mut f = UnitForm::default();
    let paths: Vec<String> = model.into_iter().collect();
    autofill(state, &mut f, &paths);
    state.ui.assets.dialog = Some(Dialog::Unit(Box::new(f)));
}

pub fn open_new_sound(state: &mut EditorState, files: Vec<String>) {
    let id = files
        .first()
        .map(|f| mf::id_from(stem(f)))
        .unwrap_or_else(|| "MySound".into());
    let id = mf::unique(&id, |i| sound_taken(state, i));
    state.ui.assets.dialog = Some(Dialog::Sound(SoundForm { id, files: files.into_iter().collect(), filter: String::new() }));
}

// ---------------------------------------------------------------------------
// Helpers.

fn refresh(state: &mut EditorState) {
    let key = (state.doc_generation, state.history.revision());
    let st = &mut state.ui.assets;
    if st.cache.as_ref().is_none_or(|c| c.key != key) {
        let usage = mf::usage(&state.doc);
        // Game files all live under Assets\, so anything else the map lacks
        // is missing without asking the install.
        let missing = usage
            .missing()
            .into_iter()
            .map(|(p, u)| {
                let in_game = (!mf::norm(p).starts_with("assets/")).then_some(false);
                Missing { path: p.to_owned(), users: u.to_vec(), in_game }
            })
            .collect();
        st.cache = Some(Cache { key, files: mf::list(&state.doc), usage, missing, checked_game: false });
    }
    if let Some(c) = &mut st.cache {
        if !c.checked_game && state.assets.ready() {
            let roots = crate::render::models::dependency_roots(&state.doc.info.dependencies);
            for m in c.missing.iter_mut().filter(|m| m.in_game.is_none()) {
                m.in_game = state.assets.exists(&m.path, &roots);
            }
            c.checked_game = true;
        }
    }
}

fn stem(path: &str) -> &str {
    let n = mf::file_name(path);
    n.rsplit_once('.').map(|(s, _)| s).unwrap_or(n)
}

fn human_size(n: usize) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.0} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

fn short_list(names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(3).map(|n| mf::file_name(n)).collect();
    let more = names.len().saturating_sub(3);
    if more > 0 { format!("{} and {more} more", shown.join(", ")) } else { shown.join(", ") }
}

fn files_of(state: &EditorState, kind: FileKind) -> Vec<String> {
    let mut v: Vec<String> =
        state.doc.passthrough.keys().filter(|k| mf::is_asset(k) && FileKind::of(k) == Some(kind)).cloned().collect();
    v.sort_by_key(|k| mf::norm(k));
    v
}

fn sound_ids(state: &EditorState) -> Vec<String> {
    let mut v: Vec<String> =
        state.doc.data.entries.iter().filter(|e| e.class() == "CSound").map(|e| e.id().to_owned()).collect();
    v.sort();
    v.dedup();
    v
}

fn sound_taken(state: &EditorState, id: &str) -> bool {
    state.doc.data.find("CSound", id).is_some()
}

fn unit_taken(state: &EditorState, id: &str) -> bool {
    if !mf::unit_id_free(&state.doc.data, id) {
        return true;
    }
    state.base.as_ref().is_some_and(|b| {
        b.has_unit(id) || [CatalogKind::Actor, CatalogKind::Model, CatalogKind::Button].iter().any(|k| b.find(*k, id).is_some())
    })
}

fn valid_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("Enter an id.".into());
    }
    if !id.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err("Ids start with a letter and use only letters, digits and _.".into());
    }
    Ok(())
}

/// Insert catalog entries (at the end) and missing default-locale text as
/// one undoable step, plus any extra edits first (file imports).
fn commit_data(state: &mut EditorState, label: String, mut edits: Vec<Edit>, entries: Vec<CatalogEntry>, strings: Vec<(String, String)>) {
    let base = state.doc.data.entries.len();
    edits.extend(entries.into_iter().enumerate().map(|(n, e)| Edit::Data { index: base + n, before: None, after: Some(e) }));
    let new_strings: Vec<_> = strings.into_iter().filter(|(k, _)| state.doc.strings.get(DEFAULT_LOCALE, k).is_none()).collect();
    if !new_strings.is_empty() {
        let before = Box::new(state.doc.strings.clone());
        let mut after = before.clone();
        for (k, v) in new_strings {
            after.set(DEFAULT_LOCALE, &k, &v);
        }
        edits.push(Edit::Strings { before, after });
    }
    if edits.is_empty() {
        return;
    }
    let edit = if edits.len() == 1 { edits.remove(0) } else { Edit::Batch(edits) };
    state.commit(label, edit);
}

/// Fill empty unit form fields from newly imported files.
fn autofill(state: &EditorState, f: &mut UnitForm, imported: &[String]) {
    let is_portrait = |p: &str| p.to_ascii_lowercase().contains("portrait");
    let models: Vec<&String> = imported.iter().filter(|p| FileKind::of(p) == Some(FileKind::Model)).collect();
    if f.model.is_none() {
        f.model = models.iter().find(|p| !is_portrait(p)).map(|p| (*p).clone());
    }
    if f.portrait.is_none() {
        f.portrait = models.iter().find(|p| is_portrait(p)).map(|p| (*p).clone());
        // A portrait next to the model in the map, sharing its folder.
        if f.portrait.is_none() {
            if let Some(m) = &f.model {
                let dir = mf::norm(m).rsplit_once('/').map(|(d, _)| d.to_owned()).unwrap_or_default();
                f.portrait = files_of(state, FileKind::Model)
                    .into_iter()
                    .find(|k| is_portrait(k) && mf::norm(k).rsplit_once('/').map(|(d, _)| d) == Some(dir.as_str()));
            }
        }
    }
    if f.icon.is_none() {
        f.icon = imported
            .iter()
            .filter(|p| FileKind::of(p) == Some(FileKind::Texture))
            .find(|p| {
                let n = mf::file_name(p).to_ascii_lowercase();
                n.starts_with("btn-") || n.contains("icon")
            })
            .cloned();
    }
    if f.name.is_empty() {
        if let Some(m) = &f.model {
            f.name = stem(m).replace(['_', '-'], " ");
        }
    }
    if !f.id_edited {
        f.id = mf::unique(&mf::id_from(&f.name), |i| unit_taken(state, i));
    }
}

fn navigate(state: &mut EditorState, user: &User) {
    match user {
        User::Data { entry, .. } => {
            state.selection.data_entry = Some(*entry);
            state.request(AppAction::FocusTab(Tab::Data));
        }
        User::Model(m) => state.ui.assets.expanded = Some(m.clone()),
        User::Text(_) => state.request(AppAction::FocusTab(Tab::Triggers)),
    }
}

// ---------------------------------------------------------------------------
// The panel.

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    refresh(state);

    ui.horizontal_wrapped(|ui| {
        if ui.button("Import files…").on_hover_text("Add models, textures and sounds to this map").clicked() {
            state.request(AppAction::ImportFiles { folder: false });
        }
        #[cfg(not(target_arch = "wasm32"))]
        if ui.button("Import folder…").on_hover_text("Add every model, texture and sound in a folder").clicked() {
            state.request(AppAction::ImportFiles { folder: true });
        }
        ui.separator();
        if ui.button("New unit…").on_hover_text("Make a unit with its own model, icon and voice").clicked() {
            open_new_unit(state, None);
        }
        if ui.button("New sound…").on_hover_text("Make a sound from audio files in this map").clicked() {
            open_new_sound(state, Vec::new());
        }
    });
    ui.add(TextEdit::singleline(&mut state.ui.assets.search).hint_text("Search files").desired_width(f32::INFINITY));

    let Some(cache) = state.ui.assets.cache.take() else { return };
    let q = state.ui.assets.search.to_lowercase();
    let total: usize = cache.files.iter().map(|f| f.size).sum();
    let not_found: Vec<&Missing> = cache.missing.iter().filter(|m| m.in_game == Some(false)).collect();
    let unchecked: Vec<&Missing> = cache.missing.iter().filter(|m| m.in_game.is_none()).collect();

    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(format!("{} files · {}", cache.files.len(), human_size(total))).color(tk.text_dim));
        if !not_found.is_empty() {
            ui.label(RichText::new(format!("· {} missing", not_found.len())).color(tk.warn));
        }
    });
    ui.separator();

    let mut action: Option<RowAction> = None;
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if cache.files.is_empty() && cache.missing.is_empty() {
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                ui.label("No custom files in this map yet.");
                ui.label(
                    RichText::new("Drop models (.m3), textures (.dds) or sounds (.mp3, .ogg, .wav) anywhere in the window, or use Import files.")
                        .color(tk.text_dim),
                );
            });
        }
        if !not_found.is_empty() {
            missing_section(ui, &tk, "Missing", "Used by this map's data or models, but not in the map or the game. Import a file with the same name and it goes where it's needed.", &not_found, true, &q, &mut action);
        }
        if !unchecked.is_empty() {
            let note = if state.assets.problem().is_some() || !state.assets.available() {
                "Not in the map. Most are probably game files; load game data (Map menu) to check which are really missing."
            } else {
                "Not in the map. Checking the game install…"
            };
            missing_section(ui, &tk, "Not in the map", note, &unchecked, false, &q, &mut action);
        }
        for kind in FileKind::ALL {
            let items: Vec<&MapFile> =
                cache.files.iter().filter(|f| f.kind == kind && (q.is_empty() || f.path.to_lowercase().contains(&q))).collect();
            if items.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(RichText::new(format!("{} ({})", kind.label(), items.len())).strong())
                .id_salt(("assets", kind.label()))
                .default_open(kind != FileKind::Animation)
                .open(if q.is_empty() { None } else { Some(true) })
                .show(ui, |ui| {
                    for f in items {
                        file_row(ui, state, &tk, f, cache.usage.users(&f.path), &mut action);
                    }
                });
        }
    });
    state.ui.assets.cache = Some(cache);
    if let Some(a) = action {
        run_action(ui.ctx(), state, a);
    }
}

enum RowAction {
    Go(User),
    Copy(String),
    Expand(String),
    #[cfg(not(target_arch = "wasm32"))]
    Play(String),
    #[cfg(not(target_arch = "wasm32"))]
    Stop,
    Remove(String),
    UnitFrom(String),
    SoundFrom(String),
    Import,
}

#[allow(clippy::too_many_arguments)]
fn missing_section(
    ui: &mut Ui,
    tk: &Tokens,
    title: &str,
    note: &str,
    items: &[&Missing],
    open: bool,
    q: &str,
    action: &mut Option<RowAction>,
) {
    let shown: Vec<&&Missing> = items.iter().filter(|m| q.is_empty() || m.path.to_lowercase().contains(q)).collect();
    if shown.is_empty() {
        return;
    }
    let color = if open { tk.warn } else { tk.text_dim };
    egui::CollapsingHeader::new(RichText::new(format!("{title} ({})", shown.len())).strong().color(color))
        .id_salt(("assets-missing", title))
        .default_open(open)
        .show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(note).color(tk.text_dim).small());
                if open && ui.small_button("Import…").clicked() {
                    *action = Some(RowAction::Import);
                }
            });
            for m in shown {
                ui.horizontal(|ui| {
                    ui.add(Label::new(RichText::new(mf::file_name(&m.path))).truncate()).on_hover_text(&m.path);
                    if let Some(u) = m.users.first() {
                        let more = if m.users.len() > 1 { format!(" +{}", m.users.len() - 1) } else { String::new() };
                        let all: Vec<String> = m.users.iter().map(User::label).collect();
                        let r = ui
                            .add(Label::new(RichText::new(format!("{}{more}", u.label())).color(tk.text_dim).small()).truncate().sense(Sense::click()))
                            .on_hover_text(all.join("\n"));
                        if r.clicked() {
                            *action = Some(RowAction::Go(u.clone()));
                        }
                    }
                });
            }
        });
}

fn file_row(ui: &mut Ui, state: &EditorState, tk: &Tokens, f: &MapFile, users: &[User], action: &mut Option<RowAction>) {
    let expanded = state.ui.assets.expanded.as_deref() == Some(f.path.as_str());
    ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 22.0), Layout::right_to_left(Align::Center), |ui| {
        ui.menu_button("⋯", |ui| {
            if ui.button("Copy path").on_hover_text("As written in data, with backslashes").clicked() {
                *action = Some(RowAction::Copy(mf::data_path(&f.path)));
                ui.close();
            }
            let make = match f.kind {
                FileKind::Model => Some(("Make a unit with this model…", RowAction::UnitFrom(f.path.clone()))),
                FileKind::Sound => Some(("Make a sound from this file…", RowAction::SoundFrom(f.path.clone()))),
                _ => None,
            };
            if let Some((label, a)) = make {
                if ui.button(label).clicked() {
                    *action = Some(a);
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("Remove from map…").clicked() {
                *action = Some(RowAction::Remove(f.path.clone()));
                ui.close();
            }
        });
        ui.label(RichText::new(human_size(f.size)).color(tk.text_dim).small());
        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
            #[cfg(not(target_arch = "wasm32"))]
            if f.kind == FileKind::Sound {
                let playing = state.ui.assets.player.playing() == Some(f.path.as_str());
                let (icon, tip) = if playing { ("■", "Stop") } else { ("▶", "Play") };
                if ui.small_button(icon).on_hover_text(tip).clicked() {
                    *action = Some(if playing { RowAction::Stop } else { RowAction::Play(f.path.clone()) });
                }
            }
            let name = RichText::new(mf::file_name(&f.path));
            let name = if expanded { name.color(tk.accent_text) } else { name };
            let r = ui.add(Label::new(name).truncate().sense(Sense::click())).on_hover_text(&f.path);
            if r.clicked() && f.kind == FileKind::Model {
                *action = Some(RowAction::Expand(f.path.clone()));
            }
            match users.first() {
                Some(u) => {
                    let more = if users.len() > 1 { format!(" +{}", users.len() - 1) } else { String::new() };
                    let all: Vec<String> = users.iter().map(User::label).collect();
                    let r = ui
                        .add(Label::new(RichText::new(format!("{}{more}", u.label())).color(tk.text_dim).small()).truncate().sense(Sense::click()))
                        .on_hover_text(format!("Used by:\n{}", all.join("\n")));
                    if r.clicked() {
                        *action = Some(RowAction::Go(u.clone()));
                    }
                }
                None => {
                    ui.add(Label::new(RichText::new("not used").color(tk.text_dim).small()))
                        .on_hover_text("Nothing in this map's data, models, triggers or layouts names this file.");
                }
            }
        });
    });
    if expanded {
        model_textures(ui, state, tk, &f.path);
    }
}

/// The textures a model in the map needs, and where each one is.
fn model_textures(ui: &mut Ui, state: &EditorState, tk: &Tokens, key: &str) {
    let Some(bytes) = state.doc.passthrough.get(key) else { return };
    let roots = crate::render::models::dependency_roots(&state.doc.info.dependencies);
    let needs = mf::model_needs(&state.doc, bytes);
    ui.indent(("model-tex", key), |ui| {
        if needs.is_empty() {
            ui.label(RichText::new("Uses no texture files.").color(tk.text_dim).small());
            return;
        }
        let mut missing = 0;
        for (t, in_map) in &needs {
            let (mark, color, tip) = if *in_map {
                ("✔", tk.ok, "In the map")
            } else {
                match state.assets.exists(t, &roots) {
                    Some(true) => ("✔", tk.text_dim, "From the game"),
                    Some(false) => {
                        missing += 1;
                        ("✖", tk.error, "Missing: the model shows untextured there")
                    }
                    None => ("?", tk.text_dim, "Not in the map; load game data to check the game"),
                }
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new(mark).color(color));
                ui.add(Label::new(RichText::new(mf::file_name(t)).small()).truncate()).on_hover_text(format!("{t}\n{tip}"));
            });
        }
        if missing > 0 {
            ui.label(
                RichText::new(format!("{missing} missing. Import files with these names and they go where the model looks for them."))
                    .color(tk.warn)
                    .small(),
            );
        }
    });
}

fn run_action(ctx: &egui::Context, state: &mut EditorState, a: RowAction) {
    match a {
        RowAction::Go(u) => navigate(state, &u),
        RowAction::Copy(p) => {
            ctx.copy_text(p.clone());
            state.toast(ToastKind::Info, format!("Copied {p}"));
        }
        RowAction::Expand(p) => {
            let st = &mut state.ui.assets;
            st.expanded = if st.expanded.as_deref() == Some(p.as_str()) { None } else { Some(p) };
        }
        #[cfg(not(target_arch = "wasm32"))]
        RowAction::Play(p) => {
            if let Some(bytes) = state.doc.passthrough.get(&p).cloned() {
                if let Err(e) = state.ui.assets.player.play(&p, bytes) {
                    state.toast(ToastKind::Error, e);
                }
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        RowAction::Stop => state.ui.assets.player.stop(),
        RowAction::Remove(p) => {
            let users = state.ui.assets.cache.as_ref().map(|c| c.usage.users(&p).iter().map(User::label).collect()).unwrap_or_default();
            state.ui.assets.dialog = Some(Dialog::Remove { path: p, users });
        }
        RowAction::UnitFrom(p) => open_new_unit(state, Some(p)),
        RowAction::SoundFrom(p) => open_new_sound(state, vec![p]),
        RowAction::Import => state.request(AppAction::ImportFiles { folder: false }),
    }
}

// ---------------------------------------------------------------------------
// Dialogs.

/// Draw the open dialog, if any. Called by the app shell every frame.
pub fn dialogs(ctx: &egui::Context, state: &mut EditorState) {
    #[cfg(not(target_arch = "wasm32"))]
    if state.ui.assets.player.playing().is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
    }
    let Some(mut dialog) = state.ui.assets.dialog.take() else { return };
    refresh(state);
    let tk = theme::tokens(ctx);
    let keep = match &mut dialog {
        Dialog::Import(f) => import_dialog(ctx, state, &tk, f),
        Dialog::Sound(f) => sound_dialog(ctx, state, &tk, f),
        Dialog::Unit(f) => unit_dialog(ctx, state, &tk, f),
        Dialog::Remove { path, users } => remove_dialog(ctx, state, &tk, path, users),
    };
    // A dialog may have opened another one (or asked for an import).
    if keep == Keep::Open && state.ui.assets.dialog.is_none() {
        state.ui.assets.dialog = Some(dialog);
    } else if keep == Keep::Resume {
        state.ui.assets.resume = Some(dialog);
    }
}

#[derive(PartialEq, Eq)]
enum Keep {
    Open,
    Close,
    /// Close for now; reopen after the import it asked for.
    Resume,
}

fn modal(ctx: &egui::Context, id: &str) -> egui::Modal {
    egui::Modal::new(Id::new(("assets-dialog", id))).frame(theme::dialog_frame(ctx))
}

fn import_dialog(ctx: &egui::Context, state: &mut EditorState, tk: &Tokens, f: &mut ImportForm) -> Keep {
    let roots = crate::render::models::dependency_roots(&state.doc.info.dependencies);
    let mut go = false;
    let mut cancel = false;
    let resp = modal(ctx, "import").show(ctx, |ui| {
        ui.set_width(640.0_f32.min(ctx.content_rect().width() - 64.0));
        let n = f.rows.iter().filter(|r| r.include).count();
        ui.heading(format!("Import {} file{}", f.rows.len(), if f.rows.len() == 1 { "" } else { "s" }));
        ui.label(RichText::new("Each file goes into the map at the path on the right; game data refers to it by that path.").color(tk.text_dim));
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            egui::Grid::new("import-grid").num_columns(3).spacing([8.0, 4.0]).striped(true).show(ui, |ui| {
                for r in &mut f.rows {
                    ui.checkbox(&mut r.include, "");
                    ui.add_enabled(r.include, TextEdit::singleline(&mut r.path).desired_width(360.0));
                    let note = match (&r.wanted_by, r.replaces) {
                        (_, true) => RichText::new("replaces the map's copy").color(tk.warn),
                        (Some(w), false) => RichText::new(format!("where {w} looks for it")).color(tk.ok),
                        (None, false) => RichText::new(r.kind.noun()).color(tk.text_dim),
                    };
                    ui.label(note.small());
                    ui.end_row();
                }
            });
        });
        if !f.skipped.is_empty() {
            ui.label(
                RichText::new(format!("Skipped {} (not a model, texture, sound or font).", short_list(&f.skipped)))
                    .color(tk.text_dim)
                    .small(),
            );
        }
        // What each imported model still needs.
        let importing: BTreeSet<String> = f.rows.iter().filter(|r| r.include).map(|r| mf::norm(&r.path)).collect();
        for r in f.rows.iter().filter(|r| r.include && r.kind == FileKind::Model) {
            let Some((_, bytes)) = f.files.get(r.source) else { continue };
            let textures = mf::embedded_paths(bytes);
            if textures.is_empty() {
                continue;
            }
            let mut lacking = Vec::new();
            let mut unknown = 0;
            for t in &textures {
                if importing.contains(&mf::norm(t)) || mf::find(&state.doc, t).is_some() {
                    continue;
                }
                match state.assets.exists(t, &roots) {
                    Some(true) => {}
                    Some(false) => lacking.push(mf::file_name(t).to_owned()),
                    None => unknown += 1,
                }
            }
            let name = mf::file_name(&r.path);
            let line = if !lacking.is_empty() {
                RichText::new(format!(
                    "{name} uses {} textures; {} are missing: {}. Add them to this import or it shows untextured there.",
                    textures.len(),
                    lacking.len(),
                    short_list(&lacking)
                ))
                .color(tk.warn)
            } else if unknown > 0 {
                RichText::new(format!("{name} uses {} textures; {unknown} aren't in the map (they may come from the game).", textures.len()))
                    .color(tk.text_dim)
            } else {
                RichText::new(format!("{name}: all {} textures found.", textures.len())).color(tk.ok)
            };
            ui.label(line.small());
        }
        ui.add_space(6.0);
        if f.rows.iter().any(|r| r.kind == FileKind::Sound) {
            ui.checkbox(&mut f.make_sounds, "Make a sound for each new audio file")
                .on_hover_text("Creates a sound (CSound) you can play from triggers or use as a unit's voice line");
        }
        if f.rows.iter().any(|r| r.kind == FileKind::Model) {
            ui.checkbox(&mut f.then_unit, "Then make a unit with the model");
        }
        ui.add_space(8.0);
        // Paths stay asset files: a map component (MapInfo, data, text)
        // can't be overwritten from here.
        let bad = f.rows.iter().find(|r| r.include && !mf::is_asset(&mf::safe_rel_path(&r.path)));
        if let Some(r) = bad {
            ui.colored_label(
                tk.error,
                format!("{} must end in a model, texture, sound or font extension.", r.path.trim()),
            );
        }
        let bad = bad.is_some();
        ui.horizontal(|ui| {
            let label = format!("Import {n} file{}", if n == 1 { "" } else { "s" });
            go = ui.add_enabled(n > 0 && !bad, Button::new(label)).clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if go {
        let imported = commit_import(state, f);
        if let Some(Dialog::Unit(mut u)) = state.ui.assets.resume.take() {
            autofill(state, &mut u, &imported);
            state.ui.assets.dialog = Some(Dialog::Unit(u));
        } else if let Some(Dialog::Sound(mut s)) = state.ui.assets.resume.take() {
            s.files.extend(imported.iter().filter(|p| FileKind::of(p) == Some(FileKind::Sound)).cloned());
            state.ui.assets.dialog = Some(Dialog::Sound(s));
        } else if f.then_unit {
            let model = imported.iter().find(|p| FileKind::of(p) == Some(FileKind::Model)).cloned();
            let mut u = UnitForm::default();
            autofill(state, &mut u, &imported);
            if u.model.is_none() {
                u.model = model;
            }
            state.ui.assets.dialog = Some(Dialog::Unit(Box::new(u)));
        }
        state.request(AppAction::FocusTab(Tab::Assets));
        return Keep::Close;
    }
    if cancel || resp.should_close() {
        import_cancelled(state);
        return Keep::Close;
    }
    Keep::Open
}

/// Apply an import. Returns the map paths written.
fn commit_import(state: &mut EditorState, f: &mut ImportForm) -> Vec<String> {
    let mut edits = Vec::new();
    let mut written = Vec::new();
    let mut seen = BTreeSet::new();
    for r in f.rows.iter().filter(|r| r.include) {
        let Some((_, bytes)) = f.files.get_mut(r.source) else { continue };
        let mut path = mf::safe_rel_path(&r.path);
        if !mf::is_asset(&path) || !seen.insert(mf::norm(&path)) {
            continue;
        }
        if let Some(existing) = mf::find(&state.doc, &path) {
            path = existing.to_owned();
        }
        let before = state.doc.passthrough.get(&path).cloned();
        let after = std::mem::take(bytes);
        if before.as_deref() == Some(after.as_slice()) {
            written.push(path);
            continue;
        }
        edits.push(Edit::File { path: path.clone(), before, after: Some(after) });
        written.push(path);
    }
    let mut sounds = Vec::new();
    if f.make_sounds {
        let mut made: BTreeSet<String> = BTreeSet::new();
        for r in f.rows.iter().filter(|r| r.include && r.kind == FileKind::Sound && r.wanted_by.is_none() && !r.replaces) {
            let base = mf::id_from(stem(&r.path));
            let id = mf::unique(&base, |i| sound_taken(state, i) || made.contains(i));
            made.insert(id.clone());
            sounds.push(mf::sound_entry(&id, &[mf::safe_rel_path(&r.path)]));
        }
    }
    let n_files = written.len();
    let n_sounds = sounds.len();
    let label = format!("Import {n_files} file{}", if n_files == 1 { "" } else { "s" });
    commit_data(state, label, edits, sounds, Vec::new());
    let msg = if n_sounds > 0 {
        format!("Imported {n_files} file{} and made {n_sounds} sound{}", plural_s(n_files), plural_s(n_sounds))
    } else {
        format!("Imported {n_files} file{}", plural_s(n_files))
    };
    state.toast(ToastKind::Success, msg);
    written
}

fn plural_s(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

fn sound_dialog(ctx: &egui::Context, state: &mut EditorState, tk: &Tokens, f: &mut SoundForm) -> Keep {
    let audio = files_of(state, FileKind::Sound);
    let mut create = false;
    let mut cancel = false;
    let mut import = false;
    let resp = modal(ctx, "sound").show(ctx, |ui| {
        ui.set_width(480.0_f32.min(ctx.content_rect().width() - 64.0));
        ui.heading("New sound");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label("Id");
            ui.add(TextEdit::singleline(&mut f.id).desired_width(260.0));
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("Audio files");
            ui.add(TextEdit::singleline(&mut f.filter).hint_text("Search").desired_width(180.0));
            if ui.button("Import…").clicked() {
                import = true;
            }
        });
        let q = f.filter.to_lowercase();
        egui::ScrollArea::vertical().max_height(220.0).auto_shrink([false, true]).show(ui, |ui| {
            if audio.is_empty() {
                ui.label(RichText::new("This map has no audio files yet. Import some (.mp3, .ogg or .wav).").color(tk.text_dim));
            }
            for a in audio.iter().filter(|a| q.is_empty() || a.to_lowercase().contains(&q)) {
                let mut on = f.files.contains(a);
                if ui.checkbox(&mut on, mf::file_name(a)).on_hover_text(a).changed() {
                    if on {
                        f.files.insert(a.clone());
                    } else {
                        f.files.remove(a);
                    }
                }
            }
        });
        ui.label(RichText::new("Plays one of the checked files at random each time. Use it from triggers (Play Sound) or as a unit's voice line.").color(tk.text_dim).small());
        let check = valid_id(f.id.trim()).and_then(|()| {
            if sound_taken(state, f.id.trim()) {
                Err(format!("There is already a sound named {}.", f.id.trim()))
            } else if f.files.is_empty() {
                Err("Check at least one audio file.".into())
            } else {
                Ok(())
            }
        });
        if let Err(e) = &check {
            ui.colored_label(tk.error, e);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            create = ui.add_enabled(check.is_ok(), Button::new("Create sound")).clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if import {
        state.request(AppAction::ImportFiles { folder: false });
        return Keep::Resume;
    }
    if create {
        let id = f.id.trim().to_owned();
        let files: Vec<String> = f.files.iter().cloned().collect();
        commit_data(state, format!("Create sound {id}"), Vec::new(), vec![mf::sound_entry(&id, &files)], Vec::new());
        state.toast(ToastKind::Success, format!("Created sound {id}"));
        return Keep::Close;
    }
    if cancel || resp.should_close() { Keep::Close } else { Keep::Open }
}

fn file_combo(ui: &mut Ui, id: &str, value: &mut Option<String>, none: &str, choices: &[String]) {
    let text = value.as_deref().map(mf::file_name).unwrap_or(none).to_owned();
    egui::ComboBox::from_id_salt(id).selected_text(text).width(300.0).show_ui(ui, |ui| {
        ui.selectable_value(value, None, none);
        for c in choices {
            ui.selectable_value(value, Some(c.clone()), mf::file_name(c)).on_hover_text(c);
        }
    });
}

fn unit_dialog(ctx: &egui::Context, state: &mut EditorState, tk: &Tokens, f: &mut UnitForm) -> Keep {
    let models = files_of(state, FileKind::Model);
    let textures = files_of(state, FileKind::Texture);
    let audio = files_of(state, FileKind::Sound);
    let sounds = sound_ids(state);
    let roots = crate::render::models::dependency_roots(&state.doc.info.dependencies);
    let mut create = false;
    let mut cancel = false;
    let mut import = false;
    let resp = modal(ctx, "unit").show(ctx, |ui| {
        ui.set_width(560.0_f32.min(ctx.content_rect().width() - 64.0));
        ui.heading("New unit");
        ui.label(RichText::new("A unit with its own look and voice that plays like the unit it's based on. Change its stats later in the Data panel.").color(tk.text_dim));
        ui.add_space(6.0);
        egui::Grid::new("unit-grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label("Name");
            if ui.add(TextEdit::singleline(&mut f.name).hint_text("Shown in game").desired_width(300.0)).changed() && !f.id_edited {
                f.id = mf::unique(&mf::id_from(&f.name), |i| unit_taken(state, i));
            }
            ui.end_row();
            ui.label("Id");
            if ui.add(TextEdit::singleline(&mut f.id).desired_width(300.0)).changed() {
                f.id_edited = true;
            }
            ui.end_row();

            ui.label("Based on");
            ui.horizontal(|ui| {
                ui.add(TextEdit::singleline(&mut f.base).desired_width(200.0));
                ui.menu_button("Pick", |ui| {
                    ui.add(TextEdit::singleline(&mut f.base_filter).hint_text("Search units").desired_width(240.0));
                    let q = f.base_filter.to_lowercase();
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        let mine: Vec<String> = state.doc.data.entries.iter().filter(|e| e.class() == "CUnit").map(|e| e.id().to_owned()).collect();
                        let mut shown = 0;
                        for id in mine.iter().filter(|i| q.is_empty() || i.to_lowercase().contains(&q)) {
                            if ui.selectable_label(f.base == *id, format!("{id} (this map)")).clicked() {
                                f.base = id.clone();
                                ui.close();
                            }
                        }
                        let base_units: Vec<(String, String)> = match &state.base {
                            Some(b) => b
                                .units()
                                .iter()
                                .filter(|u| !u.structure && (u.is_melee() || !q.is_empty()))
                                .map(|u| (u.id.clone(), u.name.clone()))
                                .collect(),
                            None => strata_core::known::UnitGroup::ALL
                                .iter()
                                .flat_map(|g| g.units().iter().map(|u| (u.id.to_owned(), u.name.to_owned())))
                                .collect(),
                        };
                        for (id, name) in base_units {
                            if !(q.is_empty() || id.to_lowercase().contains(&q) || name.to_lowercase().contains(&q)) {
                                continue;
                            }
                            shown += 1;
                            if shown > 300 {
                                ui.label(RichText::new("Keep typing to narrow the list.").color(tk.text_dim));
                                break;
                            }
                            if ui.selectable_label(f.base == id, &name).on_hover_text(&id).clicked() {
                                f.base = id;
                                ui.close();
                            }
                        }
                    });
                });
            });
            ui.end_row();

            ui.label("Model");
            ui.horizontal(|ui| {
                let keep = format!("Keep {}'s look", if f.base.trim().is_empty() { "the base unit" } else { f.base.trim() });
                let before = f.model.clone();
                file_combo(ui, "unit-model", &mut f.model, &keep, &models);
                if f.model != before {
                    // A portrait beside the new model, a name from its file.
                    autofill(state, f, &[]);
                }
                if ui.button("Import…").on_hover_text("Import a model and its textures").clicked() {
                    import = true;
                }
            });
            ui.end_row();
            ui.label("Portrait");
            file_combo(ui, "unit-portrait", &mut f.portrait, "Base unit's portrait", &models);
            ui.end_row();
            ui.label("Icon");
            file_combo(ui, "unit-icon", &mut f.icon, "Base unit's icon", &textures);
            ui.end_row();
            ui.label("Size");
            ui.add(egui::DragValue::new(&mut f.scale).range(0.1..=10.0).speed(0.01).fixed_decimals(2).suffix("×"));
            ui.end_row();
        });

        if let Some(m) = &f.model {
            if let Some(bytes) = state.doc.passthrough.get(m) {
                let lacking: Vec<String> = mf::model_needs(&state.doc, bytes)
                    .into_iter()
                    .filter(|(t, have)| !have && state.assets.exists(t, &roots) == Some(false))
                    .map(|(t, _)| t)
                    .collect();
                if !lacking.is_empty() {
                    ui.label(
                        RichText::new(format!(
                            "This model is missing {} texture{}: {}. Import them (Import…) or it shows untextured.",
                            lacking.len(),
                            plural_s(lacking.len()),
                            short_list(&lacking)
                        ))
                        .color(tk.warn)
                        .small(),
                    );
                }
            }
        }

        ui.add_space(6.0);
        egui::CollapsingHeader::new("Voice lines").id_salt("unit-voices").default_open(true).show(ui, |ui| {
            egui::Grid::new("unit-voice-grid").num_columns(2).spacing([10.0, 4.0]).show(ui, |ui| {
                for (i, (slot, when)) in UNIT_SOUND_SLOTS.iter().enumerate() {
                    ui.label(*when).on_hover_text(format!("SoundArray {slot}"));
                    let v = &mut f.voices[i];
                    let text = match v {
                        Voice::None => "Base unit's".to_owned(),
                        Voice::Sound(s) => s.clone(),
                        Voice::File(p) => mf::file_name(p).to_owned(),
                    };
                    egui::ComboBox::from_id_salt(("unit-voice", i)).selected_text(text).width(300.0).show_ui(ui, |ui| {
                        ui.selectable_value(v, Voice::None, "Base unit's");
                        if !sounds.is_empty() {
                            ui.label(RichText::new("Sounds").small().color(tk.text_dim));
                            for s in &sounds {
                                ui.selectable_value(v, Voice::Sound(s.clone()), s);
                            }
                        }
                        if !audio.is_empty() {
                            ui.label(RichText::new("Audio files").small().color(tk.text_dim));
                            for a in &audio {
                                ui.selectable_value(v, Voice::File(a.clone()), mf::file_name(a)).on_hover_text(a);
                            }
                        }
                    });
                    ui.end_row();
                }
            });
        });
        ui.add_space(4.0);
        ui.checkbox(&mut f.place_after, "Place it on the map next");

        let id = f.id.trim();
        let check = valid_id(id).and_then(|()| {
            if unit_taken(state, id) {
                return Err(format!("{id} is already used by a unit, actor, model or button. Choose another id."));
            }
            if f.name.trim().is_empty() {
                return Err("Enter a name.".into());
            }
            let base = f.base.trim();
            if base.is_empty() {
                return Err("Choose a unit to base it on.".into());
            }
            let known = state.doc.data.find("CUnit", base).is_some() || state.base.as_ref().is_none_or(|b| b.has_unit(base));
            if !known {
                return Err(format!("There is no unit named {base} in this map or the game."));
            }
            Ok(())
        });
        if let Err(e) = &check {
            ui.colored_label(tk.error, e);
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            create = ui.add_enabled(check.is_ok(), Button::new("Create unit")).clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if import {
        state.request(AppAction::ImportFiles { folder: false });
        return Keep::Resume;
    }
    if create {
        create_unit(state, f);
        return Keep::Close;
    }
    if cancel || resp.should_close() { Keep::Close } else { Keep::Open }
}

fn create_unit(state: &mut EditorState, f: &UnitForm) {
    let id = f.id.trim().to_owned();
    let mut entries = Vec::new();
    let mut voices = Vec::new();
    let mut made: BTreeSet<String> = BTreeSet::new();
    for ((slot, _), v) in UNIT_SOUND_SLOTS.iter().zip(&f.voices) {
        match v {
            Voice::None => {}
            Voice::Sound(s) => voices.push(((*slot).to_owned(), s.clone())),
            Voice::File(p) => {
                let sid = mf::unique(&format!("{id}{slot}"), |i| sound_taken(state, i) || made.contains(i));
                made.insert(sid.clone());
                entries.push(mf::sound_entry(&sid, std::slice::from_ref(p)));
                voices.push(((*slot).to_owned(), sid));
            }
        }
    }
    let recipe = UnitRecipe {
        id: id.clone(),
        name: f.name.trim().to_owned(),
        base: f.base.trim().to_owned(),
        model: f.model.clone(),
        portrait: f.portrait.clone(),
        icon: f.icon.clone(),
        scale: Some(f.scale),
        sounds: voices,
    };
    let (unit_entries, strings) = mf::build_unit(&recipe);
    entries.extend(unit_entries);
    commit_data(state, format!("Create unit {id}"), Vec::new(), entries, strings);
    if f.place_after {
        state.place_type = id.clone();
        state.mode = Mode::Units;
        state.request(AppAction::FocusTab(Tab::Viewport));
        state.toast(ToastKind::Success, format!("Created {}. Click the map to place it.", recipe.name));
    } else {
        state.toast(ToastKind::Success, format!("Created {}", recipe.name));
    }
}

fn remove_dialog(ctx: &egui::Context, state: &mut EditorState, tk: &Tokens, path: &str, users: &[String]) -> Keep {
    let mut remove = false;
    let mut cancel = false;
    let resp = modal(ctx, "remove").show(ctx, |ui| {
        ui.set_width(440.0);
        ui.heading(format!("Remove {}?", mf::file_name(path)));
        ui.label(RichText::new(path).color(tk.text_dim).small());
        ui.add_space(6.0);
        if users.is_empty() {
            ui.label("Nothing in the map uses it.");
        } else {
            ui.label(RichText::new("Still used by:").color(tk.warn));
            for u in users.iter().take(8) {
                ui.label(format!("  {u}"));
            }
            if users.len() > 8 {
                ui.label(format!("  and {} more", users.len() - 8));
            }
            ui.label(RichText::new("They will show or play nothing in game.").color(tk.text_dim));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            remove = ui.button("Remove file").clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if remove {
        if let Some(bytes) = state.doc.passthrough.get(path).cloned() {
            #[cfg(not(target_arch = "wasm32"))]
            if state.ui.assets.player.playing() == Some(path) {
                state.ui.assets.player.stop();
            }
            state.commit(format!("Remove {}", mf::file_name(path)), Edit::File { path: path.to_owned(), before: Some(bytes), after: None });
            state.toast(ToastKind::Info, format!("Removed {}", mf::file_name(path)));
        }
        return Keep::Close;
    }
    if cancel || resp.should_close() { Keep::Close } else { Keep::Open }
}
