//! Browse and edit units, abilities, weapons and other game data.
//!
//! Three panes: catalog kinds on the left, a virtualized table of entries in
//! the middle, and the selected entry's fields on the right. Own fields are
//! editable; values inherited through `parent` are shown dimmed with where
//! they come from. Every change is one undoable `Edit::Data`.

use egui::{
    Align, Button, Color32, Id, Key, KeyboardShortcut, Label, Layout, Modifiers, RichText, Sense,
    TextEdit, Ui,
};
use egui_extras::{Column, TableBuilder};
use strata_core::catalog::{
    Catalog, CatalogEntry, CatalogIndex, CatalogKind, FieldKey, Reference, ResolvedField,
    SearchHit, XmlNode,
};
use strata_core::catalog_schema::{self as schema, FieldDef, FieldType};
use strata_core::history::Edit;
use strata_core::strings::DEFAULT_LOCALE;
use strata_core::MapDocument;

use crate::panels::Tab;
use crate::state::{AppAction, EditorState, ToastKind};
use crate::theme::{self, Tokens};

// ---------------------------------------------------------------------------
// Shared widgets (also used by the Text and Map properties panels).

/// Options for [`commit_text`].
#[derive(Clone, Copy)]
pub(crate) struct TextOpts<'a> {
    pub multiline: bool,
    pub width: f32,
    pub hint: &'a str,
    pub code: bool,
    pub rows: usize,
    pub color: Option<Color32>,
}

impl Default for TextOpts<'_> {
    fn default() -> Self {
        Self {
            multiline: false,
            width: 160.0,
            hint: "",
            code: false,
            rows: 1,
            color: None,
        }
    }
}

/// A text field that edits a copy of `current` and returns the new text once,
/// when the user presses Enter (single line) or moves focus away, if it
/// changed. Escape reverts. This turns typing into one undo step.
pub(crate) fn commit_text(
    ui: &mut Ui,
    id: Id,
    current: &str,
    o: TextOpts<'_>,
) -> (egui::Response, Option<String>) {
    let mut buf: String = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| current.to_owned());
    let mut te = if o.multiline {
        TextEdit::multiline(&mut buf).desired_rows(o.rows)
    } else {
        TextEdit::singleline(&mut buf)
    };
    te = te.id(id).desired_width(o.width);
    if !o.hint.is_empty() {
        te = te.hint_text(o.hint.to_owned());
    }
    if o.code {
        te = te.code_editor();
    }
    if let Some(c) = o.color {
        te = te.text_color(c);
    }
    let resp = ui.add(te);
    let mut out = None;
    if resp.has_focus() {
        ui.data_mut(|d| d.insert_temp(id, buf));
    } else {
        if resp.lost_focus() && !ui.input(|i| i.key_pressed(Key::Escape)) && buf != current {
            out = Some(buf);
        }
        ui.data_mut(|d| d.remove::<String>(id));
    }
    (resp, out)
}

/// A small dimmed label.
pub(crate) fn hint(ui: &mut Ui, tk: &Tokens, text: impl Into<String>) -> egui::Response {
    ui.label(RichText::new(text.into()).color(tk.text_dim))
}

// ---------------------------------------------------------------------------
// State.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortCol {
    Id,
    Parent,
    Class,
    Field(usize),
}

enum Dialog {
    New {
        kind: CatalogKind,
        class: String,
        id: String,
        parent: String,
    },
    Rename {
        index: usize,
        id: String,
        update_refs: bool,
    },
    Delete {
        index: usize,
        refs: Vec<Reference>,
    },
}

struct RawXml {
    index: usize,
    /// The entry's XML when the buffer was last reset.
    original: String,
    text: String,
    error: Option<String>,
}

struct Row {
    index: usize,
    id: String,
    parent: String,
    parent_local: bool,
    class: String,
    /// Key column values: (value, inherited).
    cols: Vec<Option<(String, bool)>>,
}

#[derive(Default)]
struct Cache {
    key: Option<(u64, usize, u64)>,
    index: CatalogIndex,
    counts: [usize; 11],
    rows_kind: Option<CatalogKind>,
    rows: Vec<Row>,
    mixed_classes: bool,
    order_key: Option<(SortCol, bool, String)>,
    /// Positions into `rows`, filtered and sorted.
    order: Vec<usize>,
    search_key: Option<String>,
    hits: Vec<SearchHit>,
}

pub struct DataState {
    kind: CatalogKind,
    search: String,
    filter: String,
    sort: SortCol,
    ascending: bool,
    raw: bool,
    raw_xml: Option<RawXml>,
    dialog: Option<Dialog>,
    /// True on the frame a dialog opens, so the key press that opened it
    /// (Enter, F2, Delete) does not also confirm it.
    dialog_fresh: bool,
    new_field: String,
    table_focus: bool,
    scroll_to_selected: bool,
    seen_selection: Option<usize>,
    cache: Cache,
}

impl Default for DataState {
    fn default() -> Self {
        Self {
            kind: CatalogKind::Unit,
            search: String::new(),
            filter: String::new(),
            sort: SortCol::Id,
            ascending: true,
            raw: false,
            raw_xml: None,
            dialog: None,
            dialog_fresh: false,
            new_field: String::new(),
            table_focus: false,
            scroll_to_selected: false,
            seen_selection: None,
            cache: Cache::default(),
        }
    }
}

impl DataState {
    fn open_dialog(&mut self, d: Dialog) {
        self.dialog = Some(d);
        self.dialog_fresh = true;
    }

    fn refresh(&mut self, doc: &MapDocument, rev: u64) {
        let cat = &doc.data;
        let key = (rev, cat.entries.len(), fingerprint(cat));
        if self.cache.key != Some(key) {
            self.cache = Cache {
                key: Some(key),
                index: cat.build_index(),
                counts: cat.counts(),
                ..Default::default()
            };
        }
        if self.cache.rows_kind != Some(self.kind) {
            self.build_rows(cat);
        }
        let ok = (self.sort, self.ascending, self.filter.clone());
        if self.cache.order_key.as_ref() != Some(&ok) {
            self.build_order();
            self.cache.order_key = Some(ok);
        }
        if !self.search.trim().is_empty()
            && self.cache.search_key.as_deref() != Some(self.search.as_str())
        {
            self.cache.hits = cat.search(&self.search);
            self.cache.search_key = Some(self.search.clone());
        }
    }

    fn build_rows(&mut self, cat: &Catalog) {
        let keys: Vec<FieldKey> = schema::key_columns(self.kind)
            .iter()
            .map(|k| FieldKey::parse(k))
            .collect();
        let idx = &self.cache.index;
        self.cache.rows = cat
            .of_kind(self.kind)
            .map(|(i, e)| Row {
                index: i,
                id: e.id().to_owned(),
                parent: e.parent().unwrap_or("").to_owned(),
                parent_local: idx.parent_of(cat, i).is_some(),
                class: e.class().to_owned(),
                cols: keys
                    .iter()
                    .map(|k| {
                        idx.resolve_value(cat, i, k)
                            .map(|(v, src)| (v.to_owned(), src != i))
                    })
                    .collect(),
            })
            .collect();
        let first = self.cache.rows.first().map(|r| r.class.clone());
        self.cache.mixed_classes = self
            .cache
            .rows
            .iter()
            .any(|r| Some(&r.class) != first.as_ref());
        self.cache.rows_kind = Some(self.kind);
        self.cache.order_key = None;
    }

    fn build_order(&mut self) {
        let q = self.filter.trim().to_lowercase();
        let rows = &self.cache.rows;
        let mut order: Vec<usize> = (0..rows.len())
            .filter(|&r| {
                q.is_empty() || {
                    let row = &rows[r];
                    row.id.to_lowercase().contains(&q)
                        || row.parent.to_lowercase().contains(&q)
                        || row.class.to_lowercase().contains(&q)
                }
            })
            .collect();
        let sort = self.sort;
        order.sort_by(|&a, &b| {
            let (a, b) = (&rows[a], &rows[b]);
            let o = match sort {
                SortCol::Id => natural_cmp(&a.id, &b.id),
                SortCol::Parent => natural_cmp(&a.parent, &b.parent),
                SortCol::Class => a.class.cmp(&b.class),
                SortCol::Field(c) => {
                    let va = a.cols.get(c).and_then(|v| v.as_ref()).map(|v| v.0.as_str());
                    let vb = b.cols.get(c).and_then(|v| v.as_ref()).map(|v| v.0.as_str());
                    match (va, vb) {
                        (Some(x), Some(y)) => match (x.parse::<f64>(), y.parse::<f64>()) {
                            (Ok(x), Ok(y)) => x.total_cmp(&y),
                            _ => natural_cmp(x, y),
                        },
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                }
            };
            o.then_with(|| a.index.cmp(&b.index))
        });
        if !self.ascending {
            order.reverse();
        }
        self.cache.order = order;
    }

    /// Entry indices in table order.
    fn ordered_indices(&self) -> impl Iterator<Item = usize> + '_ {
        self.cache.order.iter().map(|&r| self.cache.rows[r].index)
    }
}

fn fingerprint(cat: &Catalog) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    cat.entries
        .first()
        .map(|e| (e.id(), e.class()))
        .hash(&mut h);
    cat.entries.last().map(|e| (e.id(), e.class())).hash(&mut h);
    cat.entries
        .iter()
        .map(|e| e.node.children.len())
        .sum::<usize>()
        .hash(&mut h);
    h.finish()
}

/// Case-insensitive compare that orders "Unit2" before "Unit10".
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    fn split(s: &str) -> (String, u64) {
        let digits = s.len() - s.trim_end_matches(|c: char| c.is_ascii_digit()).len();
        let (head, tail) = s.split_at(s.len() - digits);
        (head.to_lowercase(), tail.parse().unwrap_or(0))
    }
    let (ha, na) = split(a);
    let (hb, nb) = split(b);
    ha.cmp(&hb).then(na.cmp(&nb)).then_with(|| a.cmp(b))
}

// ---------------------------------------------------------------------------
// Frame plumbing: panels collect changes here and they are applied at the end.

#[derive(Default)]
struct Pending {
    edits: Vec<(String, Edit)>,
    select: Option<Option<usize>>,
    toasts: Vec<(ToastKind, String)>,
    actions: Vec<AppAction>,
    focus_string: Option<String>,
}

impl Pending {
    fn edit(&mut self, label: impl Into<String>, e: Edit) {
        self.edits.push((label.into(), e));
    }

    /// Modify entry `i` with `f`; records nothing if it ends up unchanged.
    fn modify(
        &mut self,
        cat: &Catalog,
        i: usize,
        label: impl Into<String>,
        f: impl FnOnce(&mut CatalogEntry),
    ) {
        let Some(before) = cat.entries.get(i) else {
            return;
        };
        let mut after = before.clone();
        f(&mut after);
        if after != *before {
            self.edit(
                label,
                Edit::Data {
                    index: i,
                    before: Some(before.clone()),
                    after: Some(after),
                },
            );
        }
    }

    fn toast(&mut self, kind: ToastKind, text: impl Into<String>) {
        self.toasts.push((kind, text.into()));
    }
}

struct Cx<'a> {
    doc: &'a MapDocument,
    ds: &'a mut DataState,
    selected: Option<usize>,
    tk: Tokens,
    out: Pending,
}

pub fn show(ui: &mut Ui, state: &mut EditorState) {
    let tk = theme::tokens(ui.ctx());
    let rev = state.history.revision();
    let selection = state
        .selection
        .data_entry
        .filter(|&i| i < state.doc.data.entries.len());

    let ds = &mut state.ui.data;
    ds.refresh(&state.doc, rev);
    if selection != ds.seen_selection {
        if let Some(e) = selection.and_then(|i| state.doc.data.entries.get(i)) {
            if ds.kind != e.kind() {
                ds.kind = e.kind();
                ds.refresh(&state.doc, rev);
            }
            ds.scroll_to_selected = true;
        }
        ds.seen_selection = selection;
    }

    let mut cx = Cx {
        doc: &state.doc,
        ds,
        selected: selection,
        tk,
        out: Pending::default(),
    };

    if ui.ui_contains_pointer()
        && ui.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::F)))
    {
        ui.memory_mut(|m| m.request_focus(Id::new("data_search")));
    }

    if cx.doc.data.entries.is_empty() {
        egui::CentralPanel::default().show(ui, |ui| empty_catalog(ui, &mut cx));
    } else {
        egui::Panel::top("data_toolbar").show(ui, |ui| toolbar(ui, &mut cx));
        egui::Panel::left("data_kinds")
            .default_size(170.0)
            .size_range(120.0..=320.0)
            .show(ui, |ui| kinds(ui, &mut cx));
        if let Some(i) = cx.selected {
            egui::Panel::right("data_fields")
                .default_size(400.0)
                .size_range(260.0..=900.0)
                .show(ui, |ui| inspector(ui, &mut cx, i));
        }
        egui::CentralPanel::default().show(ui, |ui| {
            if cx.ds.search.trim().is_empty() {
                table(ui, &mut cx);
            } else {
                search_results(ui, &mut cx);
            }
        });
    }
    dialogs(ui.ctx(), &mut cx);

    let Cx { out, .. } = cx;
    let changed = !out.edits.is_empty();
    for (label, e) in out.edits {
        state.commit(label, e);
    }
    if let Some(s) = out.select {
        state.selection.data_entry = s;
        state.ui.data.scroll_to_selected = true;
        state.ui.data.raw_xml = None;
    }
    if changed {
        state.prune_selection();
    }
    for (k, t) in out.toasts {
        state.toast(k, t);
    }
    for a in out.actions {
        state.request(a);
    }
    if let Some(k) = out.focus_string {
        state.ui.strings.focus_key(&k);
        state.request(AppAction::FocusTab(Tab::Strings));
    }
}

// ---------------------------------------------------------------------------
// Creating entries.

fn insert_entries(
    cx: &mut Cx,
    label: String,
    entries: Vec<CatalogEntry>,
    strings: Vec<(String, String)>,
) {
    let base = cx.doc.data.entries.len();
    let mut edits: Vec<Edit> = entries
        .into_iter()
        .enumerate()
        .map(|(n, e)| Edit::Data {
            index: base + n,
            before: None,
            after: Some(e),
        })
        .collect();
    let new_strings: Vec<_> = strings
        .into_iter()
        .filter(|(k, _)| cx.doc.strings.get(DEFAULT_LOCALE, k).is_none())
        .collect();
    if !new_strings.is_empty() {
        let before = Box::new(cx.doc.strings.clone());
        let mut after = before.clone();
        for (k, v) in new_strings {
            after.set(DEFAULT_LOCALE, &k, &v);
        }
        edits.push(Edit::Strings { before, after });
    }
    let edit = if edits.len() == 1 {
        edits.remove(0)
    } else {
        Edit::Batch(edits)
    };
    cx.out.edit(label, edit);
    cx.out.select = Some(Some(base));
}

/// Words of an id for display text: "MyMarine" -> "My Marine".
fn spaced(id: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in id.chars() {
        if c.is_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        out.push(c);
    }
    out
}

/// Create `My<base>` (unit + actor + name text) inheriting from a base-game unit.
fn quick_unit(cx: &mut Cx, base: &str) {
    let id = cx.doc.data.unique_id("CUnit", &format!("My{base}"));
    let unit = CatalogEntry::new("UnitData", "CUnit", &id, Some(base));
    let mut actor = CatalogEntry::new("ActorData", "CActorUnit", &id, Some(base));
    actor.node.set_attr("unitName", id.as_str());
    let strings = vec![(format!("Unit/Name/{id}"), spaced(&id))];
    cx.ds.kind = CatalogKind::Unit;
    cx.ds.search.clear();
    insert_entries(cx, format!("Create unit {id}"), vec![unit, actor], strings);
    cx.out.toast(
        ToastKind::Success,
        format!("Created {id}. It plays like {base} until you override fields."),
    );
}

fn create_entry(cx: &mut Cx, kind: CatalogKind, class: &str, id: &str, parent: &str) {
    let e = CatalogEntry::new(kind.file(), class, id, Some(parent.trim()));
    let strings = if class == "CUnit" {
        vec![(format!("Unit/Name/{id}"), spaced(id))]
    } else {
        Vec::new()
    };
    cx.ds.kind = kind;
    cx.ds.search.clear();
    insert_entries(cx, format!("Create {} {id}", kind.noun()), vec![e], strings);
}

fn duplicate(cx: &mut Cx, i: usize) {
    let doc = cx.doc;
    let e = &doc.data.entries[i];
    let mut n = e.clone();
    let id = doc.data.unique_id(e.class(), &format!("{}Copy", e.id()));
    n.set_id(&id);
    cx.out.edit(
        format!("Duplicate {} as {id}", e.id()),
        Edit::Data {
            index: i + 1,
            before: None,
            after: Some(n),
        },
    );
    cx.out.select = Some(Some(i + 1));
}

fn derive(cx: &mut Cx, i: usize) {
    let doc = cx.doc;
    let e = &doc.data.entries[i];
    let base = if e.id().starts_with("My") {
        e.id().to_owned()
    } else {
        format!("My{}", e.id())
    };
    let id = doc.data.unique_id(e.class(), &base);
    let n = CatalogEntry::new(e.file.clone(), e.class(), &id, Some(e.id()));
    cx.out.edit(
        format!("Create {id} from {}", e.id()),
        Edit::Data {
            index: i + 1,
            before: None,
            after: Some(n),
        },
    );
    cx.out.select = Some(Some(i + 1));
}

fn open_new_dialog(cx: &mut Cx, kind: CatalogKind) {
    let class = schema::classes_for_kind(kind)[0].to_owned();
    let parent = schema::default_parent(&class).to_owned();
    let base = if parent.is_empty() {
        format!("My{}", kind.noun().replace(' ', ""))
    } else {
        format!("My{parent}")
    };
    let id = cx.doc.data.unique_id(&class, &capitalize(&base));
    cx.ds.open_dialog(Dialog::New {
        kind,
        class,
        id,
        parent,
    });
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Renaming and deleting.

/// Text key prefixes that follow an entry's id, per kind.
fn string_prefixes(kind: CatalogKind) -> &'static [&'static str] {
    match kind {
        CatalogKind::Unit => &["Unit/Name/", "Unit/Description/"],
        CatalogKind::Weapon => &["Weapon/Name/"],
        CatalogKind::Ability => &["Abil/Name/"],
        CatalogKind::Upgrade => &["Upgrade/Name/"],
        CatalogKind::Behavior => &["Behavior/Name/", "Behavior/Tooltip/"],
        CatalogKind::Button => &["Button/Name/", "Button/Tooltip/", "Button/Hotkey/"],
        _ => &[],
    }
}

fn rename(cx: &mut Cx, i: usize, new_id: &str, update_refs: bool) {
    let doc = cx.doc;
    let cat = &doc.data;
    let e = &cat.entries[i];
    let old = e.id().to_owned();
    let mut edits = Vec::new();
    let mut changed_refs = 0;
    let mut own = e.clone();
    if update_refs {
        for (j, after) in cat.renamed_references(&old, new_id) {
            if j == i {
                own = after;
            } else {
                changed_refs += 1;
                edits.push(Edit::Data {
                    index: j,
                    before: Some(cat.entries[j].clone()),
                    after: Some(after),
                });
            }
        }
    }
    own.set_id(new_id);
    edits.insert(
        0,
        Edit::Data {
            index: i,
            before: Some(e.clone()),
            after: Some(own),
        },
    );

    let mut strings = doc.strings.clone();
    let mut renamed_strings = 0;
    for p in string_prefixes(e.kind()) {
        if strings.rename_key(&format!("{p}{old}"), &format!("{p}{new_id}")) {
            renamed_strings += 1;
        }
    }
    if renamed_strings > 0 {
        edits.push(Edit::Strings {
            before: Box::new(doc.strings.clone()),
            after: Box::new(strings),
        });
    }
    cx.out.edit(
        format!("Rename {old} to {new_id}"),
        if edits.len() == 1 {
            edits.remove(0)
        } else {
            Edit::Batch(edits)
        },
    );
    let mut msg = format!("Renamed {old} to {new_id}");
    if changed_refs > 0 {
        msg += &format!(
            " and updated {changed_refs} other entr{}",
            if changed_refs == 1 { "y" } else { "ies" }
        );
    }
    cx.out.toast(ToastKind::Success, msg + ".");
}

/// Start a rename: straight away if nothing refers to the entry, else ask.
fn request_rename(cx: &mut Cx, i: usize, new_id: String) {
    let new_id = new_id.trim().to_owned();
    if let Err(msg) = check_id(
        &cx.doc.data,
        cx.doc.data.entries[i].class(),
        &new_id,
        Some(i),
    ) {
        cx.out.toast(ToastKind::Error, msg);
        return;
    }
    let old = cx.doc.data.entries[i].id().to_owned();
    if cx.doc.data.references_to(&old).iter().any(|r| r.index != i) {
        cx.ds.open_dialog(Dialog::Rename {
            index: i,
            id: new_id,
            update_refs: true,
        });
    } else {
        rename(cx, i, &new_id, false);
    }
}

/// Validate a new id for `class`; `except` is the entry being renamed.
fn check_id(cat: &Catalog, class: &str, id: &str, except: Option<usize>) -> Result<(), String> {
    if id.is_empty() {
        return Err("Enter an id.".into());
    }
    if id
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '<' | '>' | '&' | ',' | ';'))
    {
        return Err("Ids can't contain spaces or the characters \" < > & , ;".into());
    }
    match cat.index_of(class, id) {
        Some(j) if Some(j) != except => Err(format!("Another {class} is already named {id}.")),
        _ => Ok(()),
    }
}

fn delete(cx: &mut Cx, i: usize) {
    let e = cx.doc.data.entries[i].clone();
    let order: Vec<usize> = cx.ds.ordered_indices().collect();
    let next = order
        .iter()
        .position(|&x| x == i)
        .and_then(|p| {
            order
                .get(p + 1)
                .or_else(|| p.checked_sub(1).and_then(|q| order.get(q)))
        })
        .copied();
    let next = next.map(|n| if n > i { n - 1 } else { n });
    cx.out.edit(
        format!("Delete {}", e.id()),
        Edit::Data {
            index: i,
            before: Some(e),
            after: None,
        },
    );
    cx.out.select = Some(next);
}

fn request_delete(cx: &mut Cx, i: usize) {
    let id = cx.doc.data.entries[i].id().to_owned();
    let refs: Vec<Reference> = cx
        .doc
        .data
        .references_to(&id)
        .into_iter()
        .filter(|r| r.index != i)
        .collect();
    if refs.is_empty() {
        delete(cx, i);
    } else {
        cx.ds.open_dialog(Dialog::Delete { index: i, refs });
    }
}

// ---------------------------------------------------------------------------
// Empty states, toolbar and kinds.

fn empty_catalog(ui: &mut Ui, cx: &mut Cx) {
    let tk = cx.tk;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.22).max(16.0));
        ui.label(RichText::new("This map doesn't change any game data yet.").size(16.0).color(tk.text));
        ui.add_space(4.0);
        hint(ui, &tk, "Create a unit based on Marine to get started. It plays like a Marine until you change its fields.");
        ui.add_space(12.0);
        if ui.add(Button::new("Create unit based on Marine").min_size(egui::vec2(220.0, 28.0))).clicked() {
            quick_unit(cx, "Marine");
        }
        ui.add_space(6.0);
        for base in ["Zealot", "Zergling"] {
            if ui.button(format!("Create unit based on {base}")).clicked() {
                quick_unit(cx, base);
            }
        }
        ui.add_space(6.0);
        if ui.link("Create a different kind of entry…").clicked() {
            open_new_dialog(cx, CatalogKind::Unit);
        }
    });
}

fn empty_kind(ui: &mut Ui, cx: &mut Cx) {
    let tk = cx.tk;
    let kind = cx.ds.kind;
    ui.vertical_centered(|ui| {
        ui.add_space(32.0);
        ui.label(
            RichText::new(format!(
                "No {} in this map yet.",
                kind.label().to_lowercase()
            ))
            .size(15.0),
        );
        ui.add_space(4.0);
        hint(
            ui,
            &tk,
            format!(
                "A new {} can start blank or inherit from a base-game {}.",
                kind.noun(),
                kind.noun()
            ),
        );
        ui.add_space(10.0);
        if ui.button(format!("New {}…", kind.noun())).clicked() {
            open_new_dialog(cx, kind);
        }
        if kind == CatalogKind::Unit && ui.button("Create unit based on Marine").clicked() {
            quick_unit(cx, "Marine");
        }
    });
}

fn toolbar(ui: &mut Ui, cx: &mut Cx) {
    ui.horizontal(|ui| {
        let r = ui.add(
            TextEdit::singleline(&mut cx.ds.search)
                .id(Id::new("data_search"))
                .hint_text("Search all data: ids, parents, values (Ctrl+F)")
                .desired_width(300.0),
        );
        if r.has_focus() && ui.input(|i| i.key_pressed(Key::Escape)) {
            cx.ds.search.clear();
        }
        if !cx.ds.search.is_empty() && ui.small_button("Clear").clicked() {
            cx.ds.search.clear();
        }
        ui.separator();
        if ui
            .button("New…")
            .on_hover_text(format!("Create a new {}", cx.ds.kind.noun()))
            .clicked()
        {
            let kind = cx.ds.kind;
            open_new_dialog(cx, kind);
        }
        let sel = cx.selected;
        let has = sel.is_some();
        if ui
            .add_enabled(has, Button::new("Duplicate"))
            .on_hover_text("Copy this entry under a new id")
            .clicked()
        {
            duplicate(cx, sel.unwrap_or_default());
        }
        if ui
            .add_enabled(has, Button::new("Derive"))
            .on_hover_text("Create a new entry that inherits from this one")
            .clicked()
        {
            derive(cx, sel.unwrap_or_default());
        }
        if ui
            .add_enabled(has, Button::new("Rename…"))
            .on_hover_text("Rename and update references (F2)")
            .clicked()
        {
            if let Some(i) = sel {
                let id = cx.doc.data.entries[i].id().to_owned();
                cx.ds.open_dialog(Dialog::Rename {
                    index: i,
                    id,
                    update_refs: true,
                });
            }
        }
        if ui
            .add_enabled(has, Button::new("Delete"))
            .on_hover_text("Delete this entry (Del)")
            .clicked()
        {
            request_delete(cx, sel.unwrap_or_default());
        }
    });
}

fn kinds(ui: &mut Ui, cx: &mut Cx) {
    let tk = cx.tk;
    ui.add_space(4.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (n, kind) in CatalogKind::ALL.into_iter().enumerate() {
                let count = cx.ds.cache.counts[n];
                let selected = cx.ds.kind == kind && cx.ds.search.trim().is_empty();
                let color = if count == 0 { tk.text_dim } else { tk.text };
                let b = Button::selectable(selected, RichText::new(kind.label()).color(color))
                    .right_text(RichText::new(count.to_string()).color(tk.text_dim));
                if ui.add_sized([ui.available_width(), 24.0], b).clicked() {
                    cx.ds.kind = kind;
                    cx.ds.search.clear();
                    cx.ds.filter.clear();
                    cx.ds.cache.rows_kind = None;
                }
            }
        });
}

// ---------------------------------------------------------------------------
// The entries table.

fn table(ui: &mut Ui, cx: &mut Cx) {
    let tk = cx.tk;
    // Refresh in case the kind changed earlier this frame.
    if cx.ds.cache.rows_kind != Some(cx.ds.kind) || cx.ds.cache.order_key.is_none() {
        let rev = cx.ds.cache.key.map(|k| k.0).unwrap_or(0);
        cx.ds.refresh(cx.doc, rev);
    }
    if cx.ds.cache.rows.is_empty() {
        empty_kind(ui, cx);
        return;
    }
    let kind = cx.ds.kind;
    ui.horizontal(|ui| {
        ui.add(
            TextEdit::singleline(&mut cx.ds.filter)
                .id(Id::new("data_filter"))
                .hint_text(format!("Filter {}", kind.label().to_lowercase()))
                .desired_width(200.0),
        );
        let (shown, total) = (cx.ds.cache.order.len(), cx.ds.cache.rows.len());
        let text = if shown == total {
            format!("{total} {}", kind.label().to_lowercase())
        } else {
            format!("{shown} of {total}")
        };
        hint(ui, &tk, text);
    });
    // Keep the cache in sync with the filter typed this frame.
    let rev = cx.ds.cache.key.map(|k| k.0).unwrap_or(0);
    cx.ds.refresh(cx.doc, rev);

    // Keyboard navigation.
    let order: Vec<usize> = cx.ds.ordered_indices().collect();
    if cx.ds.table_focus && !ui.ctx().egui_wants_keyboard_input() && !order.is_empty() {
        let pos = cx.selected.and_then(|s| order.iter().position(|&x| x == s));
        let (up, down, home, end, pgup, pgdn, del, f2) = ui.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Home),
                i.key_pressed(Key::End),
                i.key_pressed(Key::PageUp),
                i.key_pressed(Key::PageDown),
                i.key_pressed(Key::Delete),
                i.key_pressed(Key::F2),
            )
        });
        let last = order.len() - 1;
        let target = match pos {
            _ if home => Some(0),
            _ if end => Some(last),
            Some(p) if up => Some(p.saturating_sub(1)),
            Some(p) if down => Some((p + 1).min(last)),
            Some(p) if pgup => Some(p.saturating_sub(15)),
            Some(p) if pgdn => Some((p + 15).min(last)),
            None if up || down || pgup || pgdn => Some(0),
            _ => None,
        };
        if let Some(t) = target {
            if Some(order[t]) != cx.selected {
                cx.out.select = Some(Some(order[t]));
            }
        }
        if let Some(s) = cx.selected {
            if del {
                request_delete(cx, s);
            } else if f2 {
                let id = cx.doc.data.entries[s].id().to_owned();
                cx.ds.open_dialog(Dialog::Rename {
                    index: s,
                    id,
                    update_refs: true,
                });
            }
        }
    }

    let cols: Vec<&str> = schema::key_columns(kind).to_vec();
    let mixed = cx.ds.cache.mixed_classes;
    let scroll_to = if cx.ds.scroll_to_selected {
        cx.selected.and_then(|s| order.iter().position(|&x| x == s))
    } else {
        None
    };
    cx.ds.scroll_to_selected = false;

    let mut tb = TableBuilder::new(ui)
        .id_salt("data_table")
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(180.0).at_least(80.0).clip(true))
        .column(Column::initial(140.0).at_least(60.0).clip(true));
    if mixed {
        tb = tb.column(Column::initial(130.0).at_least(60.0).clip(true));
    }
    tb = tb
        .columns(Column::initial(96.0).at_least(48.0).clip(true), cols.len())
        .column(Column::remainder());
    if let Some(r) = scroll_to {
        tb = tb.scroll_to_row(r, None);
    }

    let mut sort_click = None;
    let mut clicked = None;
    let (sort, asc) = (cx.ds.sort, cx.ds.ascending);
    let header = |ui: &mut Ui, label: &str, col: SortCol, click: &mut Option<SortCol>| {
        let arrow = if sort == col {
            if asc {
                " ⏶"
            } else {
                " ⏷"
            }
        } else {
            ""
        };
        let text = RichText::new(format!("{label}{arrow}")).strong();
        if ui
            .add(Label::new(text).sense(Sense::click()).selectable(false))
            .on_hover_text("Sort")
            .clicked()
        {
            *click = Some(col);
        }
    };
    let rows = &cx.ds.cache.rows;
    let order_rows = &cx.ds.cache.order;
    let selected = cx.selected;
    let response = tb
        .header(22.0, |mut h| {
            h.col(|ui| header(ui, "Id", SortCol::Id, &mut sort_click));
            h.col(|ui| header(ui, "Parent", SortCol::Parent, &mut sort_click));
            if mixed {
                h.col(|ui| header(ui, "Class", SortCol::Class, &mut sort_click));
            }
            for (n, c) in cols.iter().enumerate() {
                h.col(|ui| header(ui, c, SortCol::Field(n), &mut sort_click));
            }
            h.col(|_| {});
        })
        .body(|body| {
            body.rows(22.0, order_rows.len(), |mut row| {
                let r = &rows[order_rows[row.index()]];
                row.set_selected(selected == Some(r.index));
                row.col(|ui| {
                    ui.add(Label::new(&r.id).truncate().selectable(false));
                });
                row.col(|ui| {
                    let c = if r.parent_local { tk.text } else { tk.text_dim };
                    ui.add(
                        Label::new(RichText::new(&r.parent).color(c))
                            .truncate()
                            .selectable(false),
                    );
                });
                if mixed {
                    row.col(|ui| {
                        ui.add(
                            Label::new(RichText::new(&r.class).color(tk.text_dim))
                                .truncate()
                                .selectable(false),
                        );
                    });
                }
                for v in &r.cols {
                    row.col(|ui| {
                        if let Some((v, inherited)) = v {
                            let c = if *inherited { tk.text_dim } else { tk.text };
                            let label = Label::new(RichText::new(v).color(c))
                                .truncate()
                                .selectable(false);
                            if v.parse::<f64>().is_ok() {
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    ui.add(label)
                                });
                            } else {
                                ui.add(label);
                            }
                        }
                    });
                }
                row.col(|_| {});
                if row.response().clicked() {
                    clicked = Some(r.index);
                }
            });
        });
    let table_rect = response.inner_rect;
    if let Some(c) = sort_click {
        if cx.ds.sort == c {
            cx.ds.ascending = !cx.ds.ascending;
        } else {
            cx.ds.sort = c;
            cx.ds.ascending = true;
        }
    }
    if let Some(i) = clicked {
        cx.out.select = Some(Some(i));
        cx.ds.table_focus = true;
    } else if ui.input(|i| {
        i.pointer.any_pressed()
            && i.pointer
                .interact_pos()
                .is_some_and(|p| !table_rect.contains(p))
    }) {
        cx.ds.table_focus = false;
    }
}

fn search_results(ui: &mut Ui, cx: &mut Cx) {
    let tk = cx.tk;
    let hits = &cx.ds.cache.hits;
    if hits.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.label(format!(
                "Nothing matches \u{201c}{}\u{201d}.",
                cx.ds.search.trim()
            ));
            hint(
                ui,
                &tk,
                "Search looks at ids, parents, classes and every field value.",
            );
        });
        return;
    }
    hint(
        ui,
        &tk,
        format!(
            "{} match{}",
            hits.len(),
            if hits.len() == 1 { "" } else { "es" }
        ),
    );
    let cat = &cx.doc.data;
    let mut clicked = None;
    TableBuilder::new(ui)
        .id_salt("data_search_table")
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(100.0).clip(true))
        .column(Column::initial(180.0).clip(true))
        .column(Column::initial(150.0).clip(true))
        .column(Column::remainder().clip(true))
        .header(22.0, |mut h| {
            for t in ["Kind", "Id", "Matched", "Value"] {
                h.col(|ui| {
                    ui.strong(t);
                });
            }
        })
        .body(|body| {
            body.rows(22.0, hits.len(), |mut row| {
                let h = &hits[row.index()];
                let Some(e) = cat.entries.get(h.index) else {
                    return;
                };
                row.set_selected(cx.selected == Some(h.index));
                row.col(|ui| {
                    ui.add(
                        Label::new(RichText::new(e.kind().label()).color(tk.text_dim))
                            .selectable(false),
                    );
                });
                row.col(|ui| {
                    ui.add(Label::new(e.id()).truncate().selectable(false));
                });
                row.col(|ui| {
                    ui.add(
                        Label::new(RichText::new(&h.field).color(tk.text_dim))
                            .truncate()
                            .selectable(false),
                    );
                });
                row.col(|ui| {
                    ui.add(Label::new(&h.value).truncate().selectable(false));
                });
                if row.response().clicked() {
                    clicked = Some(h.index);
                }
            });
        });
    if let Some(i) = clicked {
        cx.out.select = Some(Some(i));
    }
}

// ---------------------------------------------------------------------------
// The selected entry.

fn inspector(ui: &mut Ui, cx: &mut Cx, i: usize) {
    let tk = cx.tk;
    let doc = cx.doc;
    let cat = &doc.data;
    let Some(e) = cat.entries.get(i) else { return };
    let class = e.class().to_owned();
    let id = e.id().to_owned();

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new(&id).size(16.0).strong());
        let desc = schema::class_schema(&class).map(|s| s.desc).unwrap_or("");
        hint(ui, &tk, &class).on_hover_text(desc);
    });

    egui::Grid::new(("data_header", i))
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            hint(ui, &tk, "Id");
            let (_, new) = commit_text(
                ui,
                Id::new(("data_id", i)),
                &id,
                TextOpts {
                    width: 200.0,
                    ..Default::default()
                },
            );
            if let Some(n) = new {
                request_rename(cx, i, n);
            }
            ui.end_row();
            hint(ui, &tk, "Parent").on_hover_text("The entry this one inherits its values from");
            let parent = e.parent().unwrap_or("").to_owned();
            if let Some(n) = link_editor(
                ui,
                cx,
                Id::new(("data_parent", i)),
                &parent,
                Some(e.kind()),
                "None",
            ) {
                let n = n.trim().to_owned();
                if n == id {
                    cx.out
                        .toast(ToastKind::Error, "An entry can't be its own parent.");
                } else {
                    cx.out.modify(cat, i, format!("Set parent of {id}"), |e| {
                        e.set_parent(Some(&n))
                    });
                }
            }
            ui.end_row();
        });

    // Inheritance chain and derived entries.
    let chain = cat.parent_chain(i);
    let external = cat.external_root(i).map(str::to_owned);
    if !chain.is_empty() || external.is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            hint(ui, &tk, "Inherits from");
            for (n, &p) in chain.iter().enumerate() {
                if n > 0 {
                    hint(ui, &tk, "›");
                }
                if ui
                    .link(cat.entries[p].id())
                    .on_hover_text("Select this entry")
                    .clicked()
                {
                    cx.out.select = Some(Some(p));
                }
            }
            if let Some(x) = &external {
                if !chain.is_empty() {
                    hint(ui, &tk, "›");
                }
                ui.label(x)
                    .on_hover_text("Defined by the base game or a dependency, not in this map");
                hint(ui, &tk, "(base game)");
            }
        });
    }
    let derived: Vec<usize> = (0..cat.entries.len())
        .filter(|&j| j != i && cat.parent_of(j) == Some(i))
        .collect();
    if !derived.is_empty() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            hint(ui, &tk, "Inherited by");
            for &d in derived.iter().take(12) {
                if ui.link(cat.entries[d].id()).clicked() {
                    cx.out.select = Some(Some(d));
                }
            }
            if derived.len() > 12 {
                hint(ui, &tk, format!("and {} more", derived.len() - 12));
            }
        });
    }
    let refs: Vec<Reference> = cat
        .references_to(&id)
        .into_iter()
        .filter(|r| r.index != i && r.path != "parent")
        .collect();
    if !refs.is_empty() {
        egui::CollapsingHeader::new(
            RichText::new(format!(
                "Used by {} other field{}",
                refs.len(),
                if refs.len() == 1 { "" } else { "s" }
            ))
            .color(tk.text_dim),
        )
        .id_salt(("data_refs", i))
        .show(ui, |ui| {
            for r in refs.iter().take(50) {
                let other = &cat.entries[r.index];
                if ui
                    .link(format!("{}.{}", other.id(), r.path))
                    .on_hover_text(other.class())
                    .clicked()
                {
                    cx.out.select = Some(Some(r.index));
                }
            }
        });
    }

    ui.separator();
    ui.horizontal(|ui| {
        if ui.selectable_label(!cx.ds.raw, "Fields").clicked() {
            cx.ds.raw = false;
        }
        if ui
            .selectable_label(cx.ds.raw, "Raw XML")
            .on_hover_text("Edit this entry as XML")
            .clicked()
        {
            cx.ds.raw = true;
        }
    });
    ui.add_space(2.0);
    if cx.ds.raw {
        raw_editor(ui, cx, i);
    } else {
        egui::ScrollArea::vertical()
            .id_salt("data_fields_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                fields(ui, cx, i, external.as_deref());
            });
    }
}

fn fields(ui: &mut Ui, cx: &mut Cx, i: usize, external: Option<&str>) {
    let tk = cx.tk;
    let doc = cx.doc;
    let cat = &doc.data;
    let resolved = cat.resolved_fields(i);
    if resolved.is_empty() {
        let text = match external {
            Some(p) => format!(
                "No fields set. Every value comes from {p}. Add a field below to change it."
            ),
            None => "No fields set yet. Add one below.".to_owned(),
        };
        hint(ui, &tk, text);
    } else {
        egui::Grid::new(("data_field_grid", i))
            .num_columns(3)
            .striped(true)
            .spacing([8.0, 4.0])
            .min_col_width(60.0)
            .show(ui, |ui| {
                for f in &resolved {
                    field_row(ui, cx, i, f, external);
                    ui.end_row();
                }
            });
        if let Some(p) = external {
            ui.add_space(4.0);
            hint(ui, &tk, format!("Fields not listed come from {p}."));
        }
    }
    ui.add_space(10.0);
    add_field(ui, cx, i, &resolved);
}

fn summary(node: &XmlNode) -> String {
    let mut parts: Vec<String> = node
        .attrs
        .iter()
        .filter(|(k, _)| k != "index")
        .take(3)
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    if !node.children.is_empty() {
        parts.push(format!(
            "{} item{}",
            node.children.len(),
            if node.children.len() == 1 { "" } else { "s" }
        ));
    }
    if parts.is_empty() {
        "(empty)".to_owned()
    } else {
        parts.join("  ")
    }
}

/// `<WeaponArray Link="X"/>`: an array element that only names another entry.
fn is_link_item(n: &XmlNode) -> bool {
    n.children.is_empty()
        && n.attr("Link").is_some()
        && n.attrs.iter().all(|(k, _)| k == "Link" || k == "index")
}

fn def_hover(def: Option<&FieldDef>) -> Option<String> {
    def.map(|d| {
        let default = if d.default.is_empty() {
            String::new()
        } else {
            format!(", default {}", d.default)
        };
        format!("{}\n{}{default}", d.desc, d.ty.label())
    })
}

fn field_row(ui: &mut Ui, cx: &mut Cx, i: usize, f: &ResolvedField, external: Option<&str>) {
    let tk = cx.tk;
    let doc = cx.doc;
    let cat = &doc.data;
    let e = &cat.entries[i];
    let def = schema::field_def(e.class(), &f.key.name);
    let label = f.key.to_string();

    let r = ui.label(RichText::new(&label).color(if f.inherited { tk.text_dim } else { tk.text }));
    if let Some(h) = def_hover(def) {
        r.on_hover_text(h);
    }

    if f.inherited {
        ui.add(Label::new(RichText::new(scalar_or_summary(&f.node)).color(tk.text_dim)).truncate());
    } else {
        ui.vertical(|ui| value_editor(ui, cx, i, f, def));
    }

    ui.horizontal(|ui| {
        if f.inherited {
            let src = cat.entries[f.source].id().to_owned();
            if ui
                .link(RichText::new(format!("from {src}")).small())
                .on_hover_text("Select the entry that sets this value")
                .clicked()
            {
                cx.out.select = Some(Some(f.source));
            }
            if ui
                .small_button("Override")
                .on_hover_text("Copy this value into this entry so you can change it")
                .clicked()
            {
                let node = f.node.clone();
                cx.out
                    .modify(cat, i, format!("Override {label}"), |e| e.put(&f.key, node));
            }
        } else {
            let from = cat
                .parent_chain(i)
                .into_iter()
                .find(|&p| cat.entries[p].get(&f.key).is_some());
            match from {
                Some(p) => {
                    let pid = cat.entries[p].id().to_owned();
                    if ui
                        .small_button("Reset")
                        .on_hover_text(format!("Remove this override and use the value from {pid}"))
                        .clicked()
                    {
                        cx.out.modify(cat, i, format!("Reset {label}"), |e| {
                            e.remove(&f.key);
                        });
                    }
                }
                None => {
                    let tip = match external {
                        Some(x) => format!("Remove this field so the value comes from {x}"),
                        None => "Remove this field".to_owned(),
                    };
                    if ui.small_button("Remove").on_hover_text(tip).clicked() {
                        cx.out.modify(cat, i, format!("Remove {label}"), |e| {
                            e.remove(&f.key);
                        });
                    }
                }
            }
        }
    });
}

fn scalar_or_summary(n: &XmlNode) -> String {
    if n.is_scalar() {
        n.attr("value").unwrap_or("").to_owned()
    } else if is_link_item(n) {
        n.attr("Link").unwrap_or("").to_owned()
    } else {
        summary(n)
    }
}

fn value_editor(ui: &mut Ui, cx: &mut Cx, i: usize, f: &ResolvedField, def: Option<&FieldDef>) {
    let node = &f.node;
    let label = f.key.to_string();
    let id = Id::new(("data_val", i, label.as_str()));
    let ty = def.map(|d| d.ty).unwrap_or(FieldType::Text);
    if node.is_scalar() {
        let cur = node.attr("value").unwrap_or("");
        if let Some(new) = scalar_editor(ui, cx, id, &label, cur, ty) {
            let doc = cx.doc;
            let cat = &doc.data;
            cx.out.modify(
                cat,
                i,
                format!("Set {} {label}", cat.entries[i].id()),
                |e| e.set_value(&f.key, new),
            );
        }
    } else if is_link_item(node) {
        let cur = node.attr("Link").unwrap_or("");
        let kind = match ty {
            FieldType::Link(k) | FieldType::LinkArray(k) => Some(k),
            _ => None,
        };
        if let Some(new) = link_editor(ui, cx, id, cur, kind, "") {
            let mut n = node.clone();
            n.set_attr("Link", new);
            let doc = cx.doc;
            let cat = &doc.data;
            cx.out.modify(
                cat,
                i,
                format!("Set {} {label}", cat.entries[i].id()),
                |e| e.put(&f.key, n),
            );
        }
    } else {
        let mut edited = node.clone();
        let (changed, _) = tree_node(ui, &cx.tk, id, &mut edited, 0);
        if changed {
            let doc = cx.doc;
            let cat = &doc.data;
            cx.out.modify(
                cat,
                i,
                format!("Edit {} {label}", cat.entries[i].id()),
                |e| e.put(&f.key, edited),
            );
        }
    }
}

fn scalar_editor(
    ui: &mut Ui,
    cx: &mut Cx,
    id: Id,
    label: &str,
    cur: &str,
    ty: FieldType,
) -> Option<String> {
    let tk = cx.tk;
    let doc = cx.doc;
    match ty {
        FieldType::Bool | FieldType::Flags(_) => {
            let mut b = cur.trim() == "1";
            let text = if b { "Yes" } else { "No" };
            ui.checkbox(&mut b, text)
                .changed()
                .then(|| if b { "1" } else { "0" }.to_owned())
        }
        FieldType::Enum(choices) => {
            let mut v = cur.to_owned();
            egui::ComboBox::from_id_salt(id)
                .selected_text(if v.is_empty() { "None" } else { v.as_str() })
                .width(170.0)
                .show_ui(ui, |ui| {
                    for c in choices {
                        ui.selectable_value(&mut v, (*c).to_owned(), *c);
                    }
                    if !cur.is_empty() && !choices.contains(&cur) {
                        ui.selectable_value(&mut v, cur.to_owned(), cur);
                    }
                });
            (v != cur).then_some(v)
        }
        FieldType::Link(k) | FieldType::LinkArray(k) => link_editor(ui, cx, id, cur, Some(k), ""),
        FieldType::Int | FieldType::Real | FieldType::IndexedReal(_) => {
            let int = ty == FieldType::Int;
            let valid = |s: &str| {
                if int {
                    s.trim().parse::<i64>().is_ok()
                } else {
                    s.trim().parse::<f64>().is_ok()
                }
            };
            let color = (!valid(cur)).then_some(tk.error);
            let (_, new) = commit_text(
                ui,
                id,
                cur,
                TextOpts {
                    width: 120.0,
                    color,
                    ..Default::default()
                },
            );
            match new {
                Some(n) if valid(&n) => Some(n.trim().to_owned()),
                Some(n) => {
                    let what = if int { "a whole number" } else { "a number" };
                    cx.out.toast(
                        ToastKind::Error,
                        format!("{label} must be {what}; \"{n}\" isn't one."),
                    );
                    None
                }
                None => None,
            }
        }
        FieldType::StringKey => {
            let (_, new) = commit_text(
                ui,
                id,
                cur,
                TextOpts {
                    width: 200.0,
                    hint: "Text key",
                    ..Default::default()
                },
            );
            ui.horizontal(|ui| {
                match doc.strings.get(DEFAULT_LOCALE, cur) {
                    Some(t) => {
                        let short: String = t.chars().take(60).collect();
                        hint(ui, &tk, format!("\u{201c}{short}\u{201d}"));
                    }
                    None if !cur.is_empty() => {
                        ui.label(RichText::new("No text yet").color(tk.warn).small());
                    }
                    None => {}
                }
                if !cur.is_empty() && ui.link(RichText::new("Edit text").small()).clicked() {
                    cx.out.focus_string = Some(cur.to_owned());
                }
            });
            new
        }
        FieldType::Text | FieldType::Struct => {
            commit_text(
                ui,
                id,
                cur,
                TextOpts {
                    width: 200.0,
                    ..Default::default()
                },
            )
            .1
        }
    }
}

/// A text field for an entry id plus a searchable picker of `kind` entries.
fn link_editor(
    ui: &mut Ui,
    cx: &mut Cx,
    id: Id,
    cur: &str,
    kind: Option<CatalogKind>,
    empty_hint: &str,
) -> Option<String> {
    ui.horizontal(|ui| {
        let (_, mut new) = commit_text(
            ui,
            id,
            cur,
            TextOpts {
                width: 150.0,
                hint: empty_hint,
                ..Default::default()
            },
        );
        if let Some(k) = kind {
            let r = ui.menu_button("Pick", |ui| {
                if let Some(p) = link_menu(ui, cx, id, k) {
                    new = Some(p);
                }
            });
            r.response.on_hover_text(format!("Pick a {}", k.noun()));
            if let Some(&t) = cx.ds.cache.index.lookup(k, cur).first() {
                if ui
                    .small_button("Go")
                    .on_hover_text(format!("Select {cur}"))
                    .clicked()
                {
                    cx.out.select = Some(Some(t));
                }
            }
        }
        new
    })
    .inner
}

fn link_menu(ui: &mut Ui, cx: &mut Cx, id: Id, kind: CatalogKind) -> Option<String> {
    let tk = cx.tk;
    let fid = id.with("pick_filter");
    let mut q: String = ui.data(|d| d.get_temp::<String>(fid)).unwrap_or_default();
    let r = ui.add(
        TextEdit::singleline(&mut q)
            .hint_text(format!("Search {}", kind.label().to_lowercase()))
            .desired_width(220.0),
    );
    if !r.has_focus() && !ui.ctx().egui_wants_keyboard_input() {
        r.request_focus();
    }
    let ql = q.trim().to_lowercase();
    let mut cands: Vec<(String, bool)> = cx
        .doc
        .data
        .of_kind(kind)
        .map(|(_, e)| (e.id().to_owned(), true))
        .collect();
    for b in schema::base_ids(kind) {
        if !cands.iter().any(|(c, _)| c == b) {
            cands.push(((*b).to_owned(), false));
        }
    }
    cands.retain(|(c, _)| ql.is_empty() || c.to_lowercase().contains(&ql));
    cands.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| natural_cmp(&a.0, &b.0)));
    let mut picked = None;
    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        picked = cands
            .first()
            .map(|c| c.0.clone())
            .or_else(|| (!q.trim().is_empty()).then(|| q.trim().to_owned()));
    }
    egui::ScrollArea::vertical()
        .max_height(260.0)
        .show(ui, |ui| {
            if cands.is_empty() {
                hint(ui, &tk, "No matches. Type an id and press Enter.");
            }
            for (c, local) in cands.iter().take(300) {
                let text = if *local {
                    RichText::new(c)
                } else {
                    RichText::new(format!("{c}  · base game")).color(tk.text_dim)
                };
                if ui.selectable_label(false, text).clicked() {
                    picked = Some(c.clone());
                }
            }
        });
    if picked.is_some() {
        ui.data_mut(|d| d.remove::<String>(fid));
        ui.close();
    } else {
        ui.data_mut(|d| d.insert_temp(fid, q));
    }
    picked
}

/// Editable tree for a structured element. Returns (changed, remove requested).
fn tree_node(ui: &mut Ui, tk: &Tokens, id: Id, node: &mut XmlNode, depth: usize) -> (bool, bool) {
    let mut changed = false;
    let mut remove = false;
    let title = if depth == 0 {
        summary(node)
    } else {
        format!("{}  {}", node.name, summary(node))
    };
    egui::CollapsingHeader::new(RichText::new(title).color(if depth == 0 {
        tk.text_dim
    } else {
        tk.text
    }))
    .id_salt(id)
    .default_open(false)
    .show(ui, |ui| {
        if !node.attrs.is_empty() {
            egui::Grid::new(id.with("attrs"))
                .num_columns(2)
                .spacing([6.0, 2.0])
                .show(ui, |ui| {
                    for a in 0..node.attrs.len() {
                        let (k, v) = node.attrs[a].clone();
                        hint(ui, tk, &k);
                        if let (_, Some(n)) = commit_text(
                            ui,
                            id.with(("attr", a)),
                            &v,
                            TextOpts {
                                width: 150.0,
                                ..Default::default()
                            },
                        ) {
                            node.attrs[a].1 = n;
                            changed = true;
                        }
                        ui.end_row();
                    }
                });
        }
        if !node.text.is_empty() {
            let cur = node.text.clone();
            if let (_, Some(n)) = commit_text(
                ui,
                id.with("text"),
                &cur,
                TextOpts {
                    width: 200.0,
                    ..Default::default()
                },
            ) {
                node.text = n;
                changed = true;
            }
        }
        let mut remove_child = None;
        for c in 0..node.children.len() {
            let (ch, rm) = tree_node(
                ui,
                tk,
                id.with(("child", c)),
                &mut node.children[c],
                depth + 1,
            );
            changed |= ch;
            if rm {
                remove_child = Some(c);
            }
        }
        if let Some(c) = remove_child {
            node.children.remove(c);
            changed = true;
        }
        if depth > 0 && ui.small_button(format!("Remove {}", node.name)).clicked() {
            remove = true;
        }
        if node.children.is_empty() && node.attrs.is_empty() {
            hint(ui, tk, "Empty. Use Raw XML to add nested values.");
        }
    });
    (changed, remove)
}

fn add_field(ui: &mut Ui, cx: &mut Cx, i: usize, resolved: &[ResolvedField]) {
    let tk = cx.tk;
    let class = cx.doc.data.entries[i].class().to_owned();
    ui.label(RichText::new("Add field").strong());
    let id = Id::new("data_add_field");
    let r = ui.add(
        TextEdit::singleline(&mut cx.ds.new_field)
            .id(id)
            .hint_text("Field name, e.g. LifeMax or CostResource[Minerals]")
            .desired_width(f32::INFINITY),
    );
    let own: Vec<String> = resolved
        .iter()
        .filter(|f| !f.inherited)
        .map(|f| f.key.to_string())
        .collect();
    let sugg: Vec<String> = schema::suggest_fields(&class, &cx.ds.new_field)
        .into_iter()
        .filter(|s| !own.contains(s))
        .collect();
    let mut add = None;
    if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
        let typed = cx.ds.new_field.trim().to_owned();
        if !typed.is_empty() {
            add = Some(
                sugg.iter()
                    .find(|s| s.eq_ignore_ascii_case(&typed))
                    .or(sugg.first())
                    .cloned()
                    .unwrap_or(typed),
            );
        }
    }
    if sugg.is_empty() {
        if schema::class_schema(&class).is_none() {
            hint(
                ui,
                &tk,
                format!("No field suggestions for {class}; type any SC2 field name."),
            );
        }
    } else {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            let typing = !cx.ds.new_field.trim().is_empty();
            for s in sugg.iter().take(if typing { 16 } else { 10 }) {
                let name = s.split('[').next().unwrap_or(s);
                let tip = def_hover(schema::field_def(&class, name)).unwrap_or_default();
                if ui.small_button(s).on_hover_text(tip).clicked() {
                    add = Some(s.clone());
                }
            }
        });
    }
    if let Some(name) = add {
        add_field_named(cx, i, &name);
        cx.ds.new_field.clear();
    }
}

fn add_field_named(cx: &mut Cx, i: usize, name: &str) {
    let key = FieldKey::parse(name);
    let doc = cx.doc;
    if key.name.is_empty()
        || key
            .name
            .contains(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"')
    {
        cx.out.toast(
            ToastKind::Error,
            format!("\"{name}\" isn't a valid field name."),
        );
        return;
    }
    let cat = &doc.data;
    let e = &cat.entries[i];
    if e.get(&key).is_some() {
        cx.out.toast(
            ToastKind::Info,
            format!("{key} is already set on {}.", e.id()),
        );
        return;
    }
    let def = schema::field_def(e.class(), &key.name);
    let inherited = cat.resolve(i, &key).map(|(n, _)| n.clone());
    let node = match (inherited, def.map(|d| d.ty)) {
        (Some(n), _) => n,
        (None, Some(FieldType::Struct)) => XmlNode::new(&key.name),
        (None, Some(FieldType::LinkArray(_))) => {
            let mut n = XmlNode::new(&key.name);
            n.set_attr("Link", "");
            n
        }
        (None, _) => {
            let mut n = XmlNode::new(&key.name);
            if let Some(ix) = &key.index {
                n.set_attr("index", ix.as_str());
            }
            let default = def.map(|d| d.default).unwrap_or("");
            n.set_attr("value", default.replace("<id>", e.id()));
            n
        }
    };
    cx.out
        .modify(cat, i, format!("Add {key} to {}", e.id()), |e| {
            e.put(&key, node)
        });
}

fn raw_editor(ui: &mut Ui, cx: &mut Cx, i: usize) {
    let tk = cx.tk;
    let doc = cx.doc;
    let e = &doc.data.entries[i];
    let xml = e.to_xml();
    let reset = match &cx.ds.raw_xml {
        Some(r) => r.index != i || (r.text == r.original && r.original != xml),
        None => true,
    };
    if reset {
        cx.ds.raw_xml = Some(RawXml {
            index: i,
            original: xml.clone(),
            text: xml.clone(),
            error: None,
        });
    }
    let editor_id = Id::new("data_raw_xml");
    let focused = ui.memory(|m| m.has_focus(editor_id));
    let shortcut = focused
        && ui.input_mut(|inp| {
            inp.consume_shortcut(&KeyboardShortcut::new(Modifiers::COMMAND, Key::Enter))
        });
    let Some(raw) = cx.ds.raw_xml.as_mut() else {
        return;
    };
    let dirty = raw.text != xml;
    let mut apply = shortcut && dirty;
    ui.horizontal(|ui| {
        apply |= ui
            .add_enabled(dirty, Button::new("Apply XML"))
            .on_hover_text("Parse and apply (Ctrl+Enter)")
            .clicked();
        if ui.add_enabled(dirty, Button::new("Revert")).clicked() {
            raw.text = xml.clone();
            raw.original = xml.clone();
            raw.error = None;
        }
        if dirty {
            ui.label(RichText::new("Unapplied changes").color(tk.warn).small());
        } else {
            hint(ui, &tk, "Comments are not kept.");
        }
    });
    if let Some(err) = &raw.error {
        ui.colored_label(tk.error, err);
    }
    egui::ScrollArea::both()
        .id_salt("data_raw_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add(
                TextEdit::multiline(&mut raw.text)
                    .id(editor_id)
                    .code_editor()
                    .desired_width(f32::INFINITY)
                    .desired_rows(24),
            );
        });
    if !apply {
        return;
    }
    match CatalogEntry::from_xml(e.file.clone(), &raw.text) {
        Err(err) => raw.error = Some(format!("{err}. Nothing was changed.")),
        Ok(new) => {
            let cat = &doc.data;
            if let Some(j) = cat.index_of(new.class(), new.id()).filter(|&j| j != i) {
                raw.error = Some(format!(
                    "Another {} is already named {} (entry {}). Choose a different id.",
                    new.class(),
                    new.id(),
                    j + 1
                ));
                return;
            }
            raw.error = None;
            let text = new.to_xml();
            raw.text = text.clone();
            raw.original = text;
            if new.id() != e.id() {
                cx.out.toast(
                    ToastKind::Info,
                    format!(
                        "Renamed {} to {}. References were not updated; use Rename to update them.",
                        e.id(),
                        new.id()
                    ),
                );
            }
            let id = new.id().to_owned();
            cx.out
                .modify(cat, i, format!("Edit {id} as XML"), |x| *x = new);
        }
    }
}

// ---------------------------------------------------------------------------
// Dialogs.

fn dialogs(ctx: &egui::Context, cx: &mut Cx) {
    let Some(mut dialog) = cx.ds.dialog.take() else {
        return;
    };
    let tk = cx.tk;
    let fresh = std::mem::take(&mut cx.ds.dialog_fresh);
    let enter = |ui: &Ui| !fresh && ui.input(|i| i.key_pressed(Key::Enter));
    let mut close = false;
    match &mut dialog {
        Dialog::New {
            kind,
            class,
            id,
            parent,
        } => {
            let resp = egui::Modal::new(Id::new("data_new_dialog")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading(format!("New {}", kind.noun()));
                ui.add_space(6.0);
                egui::Grid::new("data_new_grid").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
                    ui.label("Kind");
                    let before = *kind;
                    egui::ComboBox::from_id_salt("data_new_kind").selected_text(kind.label()).width(220.0).show_ui(ui, |ui| {
                        for k in CatalogKind::ALL {
                            ui.selectable_value(kind, k, k.label());
                        }
                    });
                    if *kind != before {
                        *class = schema::classes_for_kind(*kind)[0].to_owned();
                        *parent = schema::default_parent(class).to_owned();
                    }
                    ui.end_row();
                    ui.label("Class");
                    ui.horizontal(|ui| {
                        ui.add(TextEdit::singleline(class).desired_width(170.0));
                        ui.menu_button("Pick", |ui| {
                            for c in schema::classes_for_kind(*kind) {
                                let desc = schema::class_schema(c).map(|s| s.desc).unwrap_or("");
                                if ui.selectable_label(class == c, c).on_hover_text(desc).clicked() {
                                    *class = c.to_owned();
                                    if parent.is_empty() {
                                        *parent = schema::default_parent(c).to_owned();
                                    }
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.end_row();
                    ui.label("Id");
                    let r = ui.add(TextEdit::singleline(id).desired_width(220.0));
                    if !ui.ctx().egui_wants_keyboard_input() {
                        r.request_focus();
                    }
                    ui.end_row();
                    ui.label("Parent");
                    ui.horizontal(|ui| {
                        ui.add(TextEdit::singleline(parent).hint_text("None: start blank").desired_width(170.0));
                        ui.menu_button("Pick", |ui| {
                            let mut c: Vec<String> = cx.doc.data.of_kind(*kind).map(|(_, e)| e.id().to_owned()).collect();
                            c.extend(schema::base_ids(*kind).iter().map(|s| (*s).to_owned()));
                            c.dedup();
                            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                                for p in c {
                                    if ui.selectable_label(*parent == p, &p).clicked() {
                                        *parent = p;
                                        ui.close();
                                    }
                                }
                            });
                        });
                    });
                    ui.end_row();
                });
                let check = check_id(&cx.doc.data, class.trim(), id.trim(), None).and_then(|_| {
                    if class.trim().is_empty() { Err("Enter a class such as CUnit.".to_owned()) } else { Ok(()) }
                });
                ui.add_space(4.0);
                match &check {
                    Err(m) => {
                        ui.colored_label(tk.error, m);
                    }
                    Ok(()) if parent.trim().is_empty() => {
                        hint(ui, &tk, "Starts blank. Units without a parent need many fields before they work in game.");
                    }
                    Ok(()) => {
                        hint(ui, &tk, format!("Starts as a copy of {} through inheritance.", parent.trim()));
                    }
                }
                ui.add_space(8.0);
                let mut create = false;
                ui.horizontal(|ui| {
                    create = ui.add_enabled(check.is_ok(), Button::new("Create")).clicked()
                        || (check.is_ok() && enter(ui));
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                create
            });
            if resp.inner {
                let (k, c, i, p) = (
                    *kind,
                    class.trim().to_owned(),
                    id.trim().to_owned(),
                    parent.trim().to_owned(),
                );
                create_entry(cx, k, &c, &i, &p);
                close = true;
            }
            close |= resp.should_close();
        }
        Dialog::Rename {
            index,
            id,
            update_refs,
        } => {
            let Some(e) = cx.doc.data.entries.get(*index) else {
                return;
            };
            let old = e.id().to_owned();
            let class = e.class().to_owned();
            let refs: Vec<Reference> = cx
                .doc
                .data
                .references_to(&old)
                .into_iter()
                .filter(|r| r.index != *index)
                .collect();
            let resp = egui::Modal::new(Id::new("data_rename_dialog")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading(format!("Rename {old}"));
                ui.add_space(6.0);
                let r = ui.add(TextEdit::singleline(id).desired_width(f32::INFINITY));
                if !ui.ctx().egui_wants_keyboard_input() {
                    r.request_focus();
                }
                let check = check_id(&cx.doc.data, &class, id.trim(), Some(*index));
                if let Err(m) = &check {
                    ui.colored_label(tk.error, m);
                }
                if refs.is_empty() {
                    hint(ui, &tk, "Nothing else in this map refers to it.");
                } else {
                    ui.checkbox(
                        update_refs,
                        format!(
                            "Also update {} reference{}",
                            refs.len(),
                            if refs.len() == 1 { "" } else { "s" }
                        ),
                    );
                    for r in refs.iter().take(8) {
                        hint(
                            ui,
                            &tk,
                            format!("  {}.{}", cx.doc.data.entries[r.index].id(), r.path),
                        );
                    }
                    if refs.len() > 8 {
                        hint(ui, &tk, format!("  and {} more", refs.len() - 8));
                    }
                    if !*update_refs {
                        ui.colored_label(
                            tk.warn,
                            "Those references will point at an id that no longer exists.",
                        );
                    }
                }
                ui.add_space(8.0);
                let mut go = false;
                ui.horizontal(|ui| {
                    let ok = check.is_ok() && id.trim() != old;
                    go = ui.add_enabled(ok, Button::new("Rename")).clicked() || (ok && enter(ui));
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                go
            });
            if resp.inner {
                let (i, n, u) = (*index, id.trim().to_owned(), *update_refs);
                rename(cx, i, &n, u);
                close = true;
            }
            close |= resp.should_close();
        }
        Dialog::Delete { index, refs } => {
            let Some(e) = cx.doc.data.entries.get(*index) else {
                return;
            };
            let id = e.id().to_owned();
            let resp = egui::Modal::new(Id::new("data_delete_dialog")).show(ctx, |ui| {
                ui.set_width(400.0);
                ui.heading(format!("Delete {id}?"));
                ui.add_space(6.0);
                ui.label(format!(
                    "{id} is used in {} place{}:",
                    refs.len(),
                    if refs.len() == 1 { "" } else { "s" }
                ));
                for r in refs.iter().take(10) {
                    hint(
                        ui,
                        &tk,
                        format!(
                            "  {}.{}",
                            cx.doc
                                .data
                                .entries
                                .get(r.index)
                                .map(|e| e.id())
                                .unwrap_or("?"),
                            r.path
                        ),
                    );
                }
                if refs.len() > 10 {
                    hint(ui, &tk, format!("  and {} more", refs.len() - 10));
                }
                ui.label("Deleting it leaves those links pointing at nothing. You can undo this.");
                ui.add_space(8.0);
                let mut go = false;
                ui.horizontal(|ui| {
                    go = ui
                        .add(Button::new(RichText::new("Delete anyway").color(tk.error)))
                        .clicked();
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
                go
            });
            if resp.inner {
                let i = *index;
                delete(cx, i);
                close = true;
            }
            close |= resp.should_close();
        }
    }
    if !close {
        cx.ds.dialog = Some(dialog);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn natural_order_and_spacing() {
        let mut v = vec!["Unit10", "unit2", "Unit1", "Alpha"];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["Alpha", "Unit1", "unit2", "Unit10"]);
        assert_eq!(spaced("MyMarine2"), "My Marine2");
        assert_eq!(spaced("MySCV"), "My SCV");
    }

    /// Drive every panel this module family owns through a few headless frames.
    #[test]
    fn panels_render_without_panicking() {
        use crate::panels::{history, map_props, problems, strings};

        let mut doc = MapDocument::new("Smoke", 64, 64);
        let mut u = CatalogEntry::new("UnitData", "CUnit", "MyMarine", Some("Marine"));
        u.set_field("LifeMax", "55");
        u.set_value(&FieldKey::indexed("CostResource", "Minerals"), "40");
        let mut arr = XmlNode::new("WeaponArray");
        arr.set_attr("Link", "MyGun");
        u.node.children.push(arr);
        let mut cost = XmlNode::new("Cost");
        let mut vital = XmlNode::new("Vital");
        vital.set_attr("index", "Energy");
        vital.set_attr("value", "50");
        cost.children.push(vital);
        u.node.children.push(cost);
        let child = CatalogEntry::new("UnitData", "CUnit", "MyMarine2", Some("MyMarine"));
        let w = CatalogEntry::new("WeaponData", "CWeaponLegacy", "MyGun", Some("GaussRifle"));
        doc.data.entries = vec![u, child, w];
        doc.strings.set("deDE", "DocInfo/Name", "Rauch");
        let mut state = EditorState::new(doc);
        state.selection.data_entry = Some(1);

        let ctx = egui::Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 900.0),
            )),
            ..Default::default()
        };
        let frame = |state: &mut EditorState| {
            let mut out = ctx.run_ui(input(), |ui| {
                egui::Panel::left("smoke_left")
                    .default_size(800.0)
                    .show(ui, |ui| show(ui, state));
                egui::CentralPanel::default().show(ui, |ui| {
                    strings::show(ui, state);
                    map_props::show(ui, state);
                    problems::show(ui, state);
                    history::show(ui, state);
                });
            });
            out.textures_delta.clear();
        };
        for n in 0..8 {
            match n {
                2 => {
                    state.ui.data.raw = true;
                    state.ui.strings.focus_key("Unit/Name/Missing");
                }
                4 => {
                    let refs = state.doc.data.references_to("MyMarine");
                    state.ui.data.open_dialog(Dialog::Delete { index: 0, refs });
                }
                5 => {
                    state.ui.data.dialog = None;
                    state.ui.data.raw = false;
                    state.ui.data.search = "gun".into();
                }
                6 => {
                    state.ui.data.search.clear();
                    let kind = CatalogKind::Weapon;
                    state.ui.data.open_dialog(Dialog::New {
                        kind,
                        class: "CWeaponLegacy".into(),
                        id: "X".into(),
                        parent: String::new(),
                    });
                }
                _ => {}
            }
            frame(&mut state);
        }
        // An empty catalog shows the invitation instead of the table.
        state.ui.data.dialog = None;
        state.doc.data.entries.clear();
        state.selection.data_entry = None;
        frame(&mut state);
        frame(&mut state);
    }

    #[test]
    fn id_validation() {
        let mut c = Catalog::default();
        c.entries
            .push(CatalogEntry::new("UnitData", "CUnit", "A", None));
        assert!(check_id(&c, "CUnit", "A", None).is_err());
        assert!(check_id(&c, "CUnit", "A", Some(0)).is_ok());
        assert!(check_id(&c, "CUnit", "Has space", None).is_err());
        assert!(check_id(&c, "CWeaponLegacy", "A", None).is_ok());
    }
}
