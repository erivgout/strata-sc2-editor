//! Custom files packed into a map: models, textures, sounds and the like.
//!
//! A map can carry its own asset files next to its data (what the official
//! editor's Import module manages). Game data points at them by path, as
//! written in data (`Assets\Textures\Hero.dds`): case-insensitive, with `\`
//! or `/`. Models point at their textures the same way, from inside the
//! `.m3` file.
//!
//! This module:
//! - lists the asset files in a map ([`list`]) and what uses each one
//!   ([`usage`]): data entries, models, triggers and UI layouts;
//! - finds paths that data or models use but the map doesn't contain
//!   ([`Usage::missing`]); the caller checks those against the game install;
//! - plans where imported files go ([`plan_import`]), putting textures where
//!   the models that use them expect them;
//! - builds the data for a custom sound ([`sound_entry`]) or a custom unit
//!   with its own model, portrait, icon and voice lines ([`build_unit`]).
//!
//! Files are added and removed with [`crate::history::Edit::File`], so
//! imports are undoable like any other change.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::catalog::{Catalog, CatalogEntry, FieldKey, XmlNode};
use crate::MapDocument;

/// What an asset file is, by extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FileKind {
    Model,
    /// `.m3a` animation sets that go with a model.
    Animation,
    Texture,
    Sound,
    Font,
    /// Movies (`.ogv`) and UI layouts (`.SC2Layout`, `.SC2Style`).
    Other,
}

impl FileKind {
    pub const ALL: [FileKind; 6] =
        [FileKind::Model, FileKind::Animation, FileKind::Texture, FileKind::Sound, FileKind::Font, FileKind::Other];

    /// The kind of an asset path, or `None` when it isn't one (map
    /// components, data, text, scripts).
    pub fn of(path: &str) -> Option<FileKind> {
        let ext = path.rsplit_once('.')?.1.to_ascii_lowercase();
        Some(match ext.as_str() {
            "m3" => FileKind::Model,
            "m3a" => FileKind::Animation,
            "dds" | "tga" | "png" | "jpg" | "jpeg" => FileKind::Texture,
            "mp3" | "ogg" | "wav" => FileKind::Sound,
            "ttf" | "otf" => FileKind::Font,
            "ogv" | "sc2layout" | "sc2style" => FileKind::Other,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            FileKind::Model => "Models",
            FileKind::Animation => "Animations",
            FileKind::Texture => "Textures",
            FileKind::Sound => "Sounds",
            FileKind::Font => "Fonts",
            FileKind::Other => "Other files",
        }
    }

    /// Lowercase singular noun for sentences.
    pub fn noun(self) -> &'static str {
        match self {
            FileKind::Model => "model",
            FileKind::Animation => "animation",
            FileKind::Texture => "texture",
            FileKind::Sound => "sound",
            FileKind::Font => "font",
            FileKind::Other => "file",
        }
    }

    /// Where imports of this kind go unless something says otherwise.
    pub fn default_folder(self) -> &'static str {
        match self {
            FileKind::Model | FileKind::Animation => "Assets/Models",
            FileKind::Texture => "Assets/Textures",
            FileKind::Sound => "Assets/Sounds",
            FileKind::Font => "Assets/Fonts",
            FileKind::Other => "Assets",
        }
    }
}

/// Files with asset extensions that belong to the map itself, not to its
/// imports (the editor regenerates them).
const COMPONENT_ASSETS: [&str; 1] = ["minimap.tga"];

/// Comparison form of a path: lowercase, `/` separators, no leading `./` or `/`.
pub fn norm(path: &str) -> String {
    let p = path.trim().replace('\\', "/").to_ascii_lowercase();
    let mut s = p.as_str();
    loop {
        if let Some(r) = s.strip_prefix("./") {
            s = r;
        } else if let Some(r) = s.strip_prefix('/') {
            s = r;
        } else {
            break;
        }
    }
    s.to_owned()
}

/// A file path as game data writes it (`\` separators).
pub fn data_path(file: &str) -> String {
    file.replace('/', "\\")
}

/// The file name part of a path.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// Is `path` (a map file key) an imported asset rather than part of the map?
pub fn is_asset(path: &str) -> bool {
    FileKind::of(path).is_some() && !COMPONENT_ASSETS.contains(&norm(path).as_str())
}

/// One asset file in the map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapFile {
    /// The key in [`MapDocument::passthrough`], with `/` separators.
    pub path: String,
    pub kind: FileKind,
    pub size: usize,
}

/// Every asset file in the map, sorted by kind then path.
pub fn list(doc: &MapDocument) -> Vec<MapFile> {
    let mut out: Vec<MapFile> = doc
        .passthrough
        .iter()
        .filter(|(k, _)| is_asset(k))
        .filter_map(|(k, v)| Some(MapFile { path: k.clone(), kind: FileKind::of(k)?, size: v.len() }))
        .collect();
    out.sort_by(|a, b| a.kind.cmp(&b.kind).then_with(|| norm(&a.path).cmp(&norm(&b.path))));
    out
}

/// The map file key for a path as written in data or a model, if the map has it.
pub fn find<'a>(doc: &'a MapDocument, path: &str) -> Option<&'a str> {
    let want = norm(path);
    doc.passthrough.keys().find(|k| norm(k) == want).map(String::as_str)
}

/// Paths of asset files embedded in a binary file (the textures an `.m3`
/// model uses), in order of first appearance. Models store them as plain
/// null-terminated strings.
pub fn embedded_paths(bytes: &[u8]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen = BTreeSet::new();
    for run in bytes.split(|b| !(0x20..0x7f).contains(b)) {
        if run.len() < 5 {
            continue;
        }
        let Ok(s) = std::str::from_utf8(run) else { continue };
        let s = s.trim();
        if FileKind::of(s).is_some_and(|k| k == FileKind::Texture) && s.contains(['/', '\\']) && seen.insert(norm(s)) {
            out.push(s.to_owned());
        }
    }
    out
}

/// Who uses a path.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum User {
    /// A data field: entry index in `doc.data.entries`, class, id and the
    /// element (`Model`, `AssetArray`, `Wireframe.Image`).
    Data { entry: usize, class: String, id: String, field: String },
    /// A model in the map (its map file key).
    Model(String),
    /// Trigger, script or UI layout text that mentions the path.
    Text(String),
}

impl User {
    pub fn label(&self) -> String {
        match self {
            User::Data { class, id, field, .. } => format!("{class} {id} ({field})"),
            User::Model(m) => format!("model {}", file_name(m)),
            User::Text(f) => file_name(f).to_owned(),
        }
    }
}

/// Which paths the map uses and who uses them.
#[derive(Debug, Clone, Default)]
pub struct Usage {
    /// Normalized path -> (path as first written, users).
    by_path: BTreeMap<String, (String, Vec<User>)>,
    /// Normalized paths of the map's own asset files.
    present: BTreeSet<String>,
}

impl Usage {
    /// Users of a map file (by key or data path).
    pub fn users(&self, path: &str) -> &[User] {
        self.by_path.get(&norm(path)).map(|(_, u)| u.as_slice()).unwrap_or(&[])
    }

    /// Paths that data or the map's models use but the map doesn't contain:
    /// (path as written, users). Many are game files (base-game models,
    /// textures); check them against the install before calling them missing.
    pub fn missing(&self) -> Vec<(&str, &[User])> {
        self.by_path
            .iter()
            .filter(|(n, (_, users))| {
                !self.present.contains(*n) && users.iter().any(|u| !matches!(u, User::Text(_)))
            })
            .map(|(_, (p, users))| (p.as_str(), users.as_slice()))
            .collect()
    }

    fn add(&mut self, path: &str, user: User) {
        let e = self.by_path.entry(norm(path)).or_insert_with(|| (path.to_owned(), Vec::new()));
        if !e.1.contains(&user) {
            e.1.push(user);
        }
    }
}

/// Text files whose contents may name assets (triggers, scripts, layouts).
fn is_text_user(path: &str) -> bool {
    let n = norm(path);
    n == "triggers"
        || n.ends_with(".galaxy")
        || n.ends_with(".sc2layout")
        || n.ends_with(".sc2style")
        || n == "documentheader"
        || n == "mapinfo"
}

/// Work out who uses every path the map's data and models name, plus which
/// of the map's own asset files are mentioned by triggers, scripts or layouts.
pub fn usage(doc: &MapDocument) -> Usage {
    let mut u = Usage::default();
    for (i, e) in doc.data.entries.iter().enumerate() {
        e.node.walk_attrs(&mut |elem, _key, value| {
            if value.contains("##") || FileKind::of(value).is_none() {
                return;
            }
            let field = if elem.is_empty() { e.class().to_owned() } else { elem.to_owned() };
            u.add(value, User::Data { entry: i, class: e.class().to_owned(), id: e.id().to_owned(), field });
        });
    }
    for (k, bytes) in &doc.passthrough {
        if FileKind::of(k) == Some(FileKind::Model) {
            for t in embedded_paths(bytes) {
                u.add(&t, User::Model(k.clone()));
            }
        }
    }
    let assets: Vec<(String, String)> =
        doc.passthrough.keys().filter(|k| is_asset(k)).map(|k| (k.clone(), norm(k))).collect();
    u.present = assets.iter().map(|(_, n)| n.clone()).collect();
    for (k, bytes) in &doc.passthrough {
        if !is_text_user(k) {
            continue;
        }
        let text = String::from_utf8_lossy(bytes).to_ascii_lowercase().replace("\\\\", "/").replace('\\', "/");
        for (asset, n) in &assets {
            if text.contains(n.as_str()) {
                u.add(asset, User::Text(k.clone()));
            }
        }
    }
    u
}

// ---------------------------------------------------------------------------
// Importing

/// Where one imported file goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    /// Index into the files handed to [`plan_import`].
    pub source: usize,
    /// Map file key (`/` separators).
    pub path: String,
    pub kind: FileKind,
    /// Replaces a file the map already has.
    pub replaces: bool,
    /// Who expects the file at this path: a model in the map or in this
    /// import ("model hero.m3"), or a user from the `missing` list.
    pub wanted_by: Option<String>,
}

/// A file name safe to use inside a map: no separators or characters
/// Windows refuses, never empty.
pub fn safe_file_name(name: &str) -> String {
    let n: String = file_name(name)
        .chars()
        .map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { '_' } else { c })
        .collect();
    let n = n.trim().trim_end_matches('.').to_owned();
    if n.is_empty() { "file".into() } else { n }
}

/// A relative path safe to use inside a map: each part made safe with
/// [`safe_file_name`], `.` and `..` parts dropped.
pub fn safe_rel_path(name: &str) -> String {
    let parts: Vec<String> =
        name.split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..").map(safe_file_name).collect();
    if parts.is_empty() { "file".into() } else { parts.join("/") }
}

/// Decide where each file goes. `files` are (name, bytes), where the name
/// is a file name or a path relative to an imported folder
/// (`Voices/hello.mp3`); files that aren't assets are skipped (the caller
/// reports them).
///
/// Textures go where a model expects them: a model in the map or in this
/// import naming `Assets/Textures/Hero_Diff.dds` puts an imported
/// `hero_diff.dds` at exactly that path. Everything else goes to its kind's
/// [`FileKind::default_folder`] (or `folder` when given), keeping the
/// subfolders of its name.
///
/// `missing` lists paths that data uses but neither the map nor the game
/// has, with who uses them (the caller knows the game): a file with the
/// same name goes there, whatever its kind.
pub fn plan_import(
    doc: &MapDocument,
    files: &[(String, &[u8])],
    folder: Option<&str>,
    missing: &[(String, String)],
) -> Vec<Planned> {
    // File name (lowercase) -> (path as written, who wants it, any kind:
    // false for model textures, which only place textures).
    let mut wanted: HashMap<String, (String, String, bool)> = HashMap::new();
    for (path, who) in missing {
        wanted.entry(file_name(path).to_ascii_lowercase()).or_insert_with(|| (path.replace('\\', "/"), who.clone(), true));
    }
    let mut want_from = |model: &str, bytes: &[u8]| {
        for t in embedded_paths(bytes) {
            wanted
                .entry(file_name(&t).to_ascii_lowercase())
                .or_insert_with(|| (t.replace('\\', "/"), format!("model {model}"), false));
        }
    };
    for (name, bytes) in files {
        if FileKind::of(name) == Some(FileKind::Model) {
            want_from(file_name(name), bytes);
        }
    }
    for (k, bytes) in &doc.passthrough {
        if FileKind::of(k) == Some(FileKind::Model) {
            want_from(file_name(k), bytes);
        }
    }
    let folder = folder.map(|f| f.trim().trim_matches(['/', '\\']).replace('\\', "/")).filter(|f| !f.is_empty());
    let mut taken: BTreeSet<String> = BTreeSet::new();
    let mut out = Vec::new();
    for (i, (name, _)) in files.iter().enumerate() {
        let Some(kind) = FileKind::of(name) else { continue };
        let clean = safe_rel_path(name);
        let (mut path, wanted_by) = match wanted.get(&file_name(&clean).to_ascii_lowercase()) {
            Some((p, who, any)) if *any || kind == FileKind::Texture => (p.clone(), Some(who.clone())),
            _ => {
                let dir = folder.clone().unwrap_or_else(|| kind.default_folder().to_owned());
                (format!("{dir}/{clean}"), None)
            }
        };
        // Reuse the spelling of a file being replaced.
        if let Some(existing) = find(doc, &path) {
            path = existing.to_owned();
        }
        if !taken.insert(norm(&path)) {
            continue; // the same name twice in one import: first wins
        }
        out.push(Planned { source: i, replaces: find(doc, &path).is_some(), path, kind, wanted_by });
    }
    out
}

/// The textures a model needs, each with whether the map has it.
pub fn model_needs(doc: &MapDocument, model: &[u8]) -> Vec<(String, bool)> {
    embedded_paths(model).into_iter().map(|t| { let have = find(doc, &t).is_some(); (t, have) }).collect()
}

// ---------------------------------------------------------------------------
// Building data

/// A valid data id from free text: letters, digits and `_`, starting with a
/// letter ("Hero unit 2" -> "HeroUnit2").
pub fn id_from(text: &str) -> String {
    let mut out = String::new();
    for word in text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_') {
        let mut cs = word.chars();
        if let Some(f) = cs.next() {
            out.push(f.to_ascii_uppercase());
            out.push_str(cs.as_str());
        }
    }
    let out: String = out.chars().skip_while(|c| !c.is_ascii_alphabetic()).collect();
    if out.is_empty() { "Custom".into() } else { out }
}

/// `base`, or `base2`, `base3`... whichever `taken` says is free.
pub fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    (2..).map(|n| format!("{base}{n}")).find(|id| !taken(id)).unwrap_or_default()
}

/// `<CSound id="...">` playing one of `files` (data paths) at random.
pub fn sound_entry(id: &str, files: &[String]) -> CatalogEntry {
    let mut e = CatalogEntry::new("SoundData", "CSound", id, None);
    for f in files {
        let mut a = XmlNode::new("AssetArray");
        a.set_attr("File", data_path(f));
        e.node.children.push(a);
    }
    e
}

/// Voice and sound slots of a unit actor (`SoundArray` indexes).
pub const UNIT_SOUND_SLOTS: [(&str, &str); 6] = [
    ("Ready", "Trained or spawned"),
    ("What", "Selected"),
    ("Yes", "Given an order"),
    ("Attack", "Ordered to attack"),
    ("Pissed", "Clicked repeatedly"),
    ("Help", "Under attack"),
];

/// Slots that also have a group variant (`GroupSoundArray`), played when
/// several units are selected or ordered.
const GROUP_SLOTS: [&str; 3] = ["What", "Yes", "Attack"];

/// Everything needed to make a custom unit.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnitRecipe {
    /// Data id for the unit, actor, models and button.
    pub id: String,
    /// Name shown in game.
    pub name: String,
    /// Unit to inherit stats, abilities and weapons from (e.g. "Marine").
    pub base: String,
    /// Data path of the unit's `.m3` model; `None` keeps the base unit's look.
    pub model: Option<String>,
    /// Data path of the portrait `.m3`.
    pub portrait: Option<String>,
    /// Data path of the icon image (command card, selection panel).
    pub icon: Option<String>,
    /// Model scale; `None` or 1 leaves it alone.
    pub scale: Option<f32>,
    /// (slot, sound id), slots from [`UNIT_SOUND_SLOTS`].
    pub sounds: Vec<(String, String)>,
}

/// Data entries and text for a unit made from `r`: a `CUnit` inheriting
/// from the base unit, its own `CActorUnit`, `CModel`s for the model and
/// portrait, and a `CButton` with the icon.
///
/// With a custom model the actor starts from `GenericUnitStandard`, so the
/// unit plays its own model's death animation instead of the base unit's
/// death effects. Without one it inherits the base unit's actor.
pub fn build_unit(r: &UnitRecipe) -> (Vec<CatalogEntry>, Vec<(String, String)>) {
    let id = r.id.as_str();
    let mut entries = Vec::new();
    let mut strings = vec![(format!("Unit/Name/{id}"), r.name.clone())];

    entries.push(CatalogEntry::new("UnitData", "CUnit", id, Some(&r.base)));

    let fmt_scale = |s: f32| format!("{s:.6},{s:.6},{s:.6}");
    if let Some(m) = &r.model {
        let mut model = CatalogEntry::new("ModelData", "CModel", id, None);
        model.set_field("Model", data_path(m));
        if let Some(s) = r.scale.filter(|s| s.is_finite() && *s > 0.0 && (*s - 1.0).abs() > 1e-4) {
            model.set_field("ScaleMin", fmt_scale(s));
            model.set_field("ScaleMax", fmt_scale(s));
        }
        entries.push(model);
    }
    let portrait_id = format!("{id}Portrait");
    if let Some(p) = &r.portrait {
        let mut model = CatalogEntry::new("ModelData", "CModel", &portrait_id, None);
        model.set_field("Model", data_path(p));
        entries.push(model);
    }

    let actor_parent = if r.model.is_some() { "GenericUnitStandard" } else { r.base.as_str() };
    let mut actor = CatalogEntry::new("ActorData", "CActorUnit", id, Some(actor_parent));
    actor.node.set_attr("unitName", id);
    if r.model.is_some() {
        actor.set_field("Model", id);
    }
    if r.portrait.is_some() {
        actor.set_field("PortraitModel", &portrait_id);
    }
    if let Some(icon) = &r.icon {
        actor.set_field("UnitIcon", data_path(icon));
    }
    for (slot, sound) in &r.sounds {
        actor.set_value(&FieldKey::indexed("SoundArray", slot.as_str()), sound.as_str());
        if GROUP_SLOTS.contains(&slot.as_str()) {
            actor.set_value(&FieldKey::indexed("GroupSoundArray", slot.as_str()), sound.as_str());
        }
    }
    entries.push(actor);

    if let Some(icon) = &r.icon {
        let mut button = CatalogEntry::new("ButtonData", "CButton", id, None);
        button.set_field("Icon", data_path(icon));
        entries.push(button);
        strings.push((format!("Button/Name/{id}"), r.name.clone()));
    }
    (entries, strings)
}

/// Is `id` free for every entry [`build_unit`] makes, in `cat`?
pub fn unit_id_free(cat: &Catalog, id: &str) -> bool {
    let portrait = format!("{id}Portrait");
    [("CUnit", id), ("CActorUnit", id), ("CModel", id), ("CButton", id), ("CModel", portrait.as_str())]
        .iter()
        .all(|(c, i)| cat.find(c, i).is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Edit, History, Origin};

    fn doc_with(files: &[(&str, &[u8])]) -> MapDocument {
        let mut d = MapDocument::new("t", 16, 16);
        for (k, v) in files {
            d.passthrough.insert((*k).to_owned(), v.to_vec());
        }
        d
    }

    fn fake_model(textures: &[&str]) -> Vec<u8> {
        let mut b = b"MD34\0\0\x01\x02".to_vec();
        for t in textures {
            b.extend_from_slice(t.as_bytes());
            b.extend_from_slice(&[0, 0xff, 3]);
        }
        b
    }

    #[test]
    fn kinds_and_paths() {
        assert_eq!(FileKind::of("Assets/Units/Hero.M3"), Some(FileKind::Model));
        assert_eq!(FileKind::of("a\\b\\voice.mp3"), Some(FileKind::Sound));
        assert_eq!(FileKind::of("Base.SC2Data/GameData/UnitData.xml"), None);
        assert_eq!(FileKind::of("MapInfo"), None);
        assert!(!is_asset("Minimap.tga"));
        assert!(is_asset("Assets/Textures/minimap.tga"));
        assert_eq!(norm(".\\Assets\\Textures\\X.dds"), "assets/textures/x.dds");
        assert_eq!(data_path("Assets/Sounds/a.ogg"), "Assets\\Sounds\\a.ogg");
        assert_eq!(safe_file_name("C:\\dl\\we?ird:name.mp3"), "we_ird_name.mp3");
        assert_eq!(id_from("my hero unit 2"), "MyHeroUnit2");
        assert_eq!(id_from("2 fast"), "Fast");
        assert_eq!(id_from("!!"), "Custom");
    }

    #[test]
    fn finds_textures_inside_models() {
        let m = fake_model(&["Assets/Textures/Hero_Diff.dds", "Assets\\Textures\\hero_norm.dds", "Assets/Textures/Hero_Diff.dds"]);
        assert_eq!(embedded_paths(&m), vec!["Assets/Textures/Hero_Diff.dds", "Assets\\Textures\\hero_norm.dds"]);
    }

    #[test]
    fn usage_links_data_models_and_triggers() {
        let model = fake_model(&["Assets/Textures/Hero_Diff.dds", "Assets/Textures/GameOnly.dds"]);
        let mut doc = doc_with(&[
            ("Hero/Hero.m3", &model),
            ("Assets/Textures/Hero_Diff.dds", b"dds"),
            ("Assets/Sounds/hi.mp3", b"mp3"),
            ("Assets/Textures/loading.dds", b"dds"),
            ("Triggers", br#"<Param><Value>Assets\Textures\loading.dds</Value></Param>"#),
        ]);
        let mut m = CatalogEntry::new("ModelData", "CModel", "Hero", None);
        m.set_field("Model", "Hero\\Hero.m3");
        doc.data.entries.push(m);
        doc.data.entries.push(sound_entry("Hi", &["Assets/Sounds/hi.mp3".into(), "Assets/Sounds/gone.mp3".into()]));

        let u = usage(&doc);
        assert!(matches!(&u.users("hero/hero.m3")[0], User::Data { id, field, .. } if id == "Hero" && field == "Model"));
        assert_eq!(u.users("Assets\\Textures\\Hero_Diff.dds"), &[User::Model("Hero/Hero.m3".into())]);
        assert_eq!(u.users("Assets/Textures/loading.dds"), &[User::Text("Triggers".into())]);
        let missing: Vec<&str> = u.missing().iter().map(|m| m.0).collect();
        assert_eq!(missing, vec!["Assets\\Sounds\\gone.mp3", "Assets/Textures/GameOnly.dds"]);
        let files = list(&doc);
        assert_eq!(files.iter().map(|f| f.kind).collect::<Vec<_>>(), [FileKind::Model, FileKind::Texture, FileKind::Texture, FileKind::Sound]);
    }

    #[test]
    fn imports_put_textures_where_models_want_them() {
        let doc = doc_with(&[("Assets/Sounds/old.mp3", b"x")]);
        let model = fake_model(&["Assets/Textures/Hero_Diff.dds"]);
        let files: Vec<(String, &[u8])> = vec![
            ("hero.m3".into(), &model),
            ("stuff\\hero_diff.DDS".into(), b"d"),
            ("other.dds".into(), b"o"),
            ("old.mp3".into(), b"new"),
            ("notes.txt".into(), b"skip"),
            ("Voices/Trump/../bye?.mp3".into(), b"v"),
            ("FIRED.mp3".into(), b"f"),
        ];
        let missing = [("GestaltHeavyBomber\\Sounds\\Trump\\fired.mp3".to_owned(), "CSound TrumpFired".to_owned())];
        let plan = plan_import(&doc, &files, None, &missing);
        let paths: Vec<(&str, bool)> = plan.iter().map(|p| (p.path.as_str(), p.replaces)).collect();
        assert_eq!(
            paths,
            vec![
                ("Assets/Models/hero.m3", false),
                ("Assets/Textures/Hero_Diff.dds", false),
                ("Assets/Textures/other.dds", false),
                ("Assets/Sounds/old.mp3", true),
                ("Assets/Sounds/Voices/Trump/bye_.mp3", false),
                ("GestaltHeavyBomber/Sounds/Trump/fired.mp3", false),
            ]
        );
        assert_eq!(plan[1].wanted_by.as_deref(), Some("model hero.m3"));
        assert_eq!(plan[5].wanted_by.as_deref(), Some("CSound TrumpFired"));
        let custom = plan_import(&doc, &files[2..3], Some("\\UI\\Icons\\"), &[]);
        assert_eq!(custom[0].path, "UI/Icons/other.dds");
    }

    #[test]
    fn file_edits_undo_and_track_removals() {
        let mut doc = doc_with(&[("Assets/a.ogv", b"a")]);
        let mut h = History::default();
        h.commit(&mut doc, "Import", Origin::User, Edit::File { path: "Assets/b.mp3".into(), before: None, after: Some(b"b".to_vec()) });
        h.commit(&mut doc, "Remove", Origin::User, Edit::File { path: "Assets/a.ogv".into(), before: Some(b"a".to_vec()), after: None });
        assert!(doc.passthrough.contains_key("Assets/b.mp3"));
        assert!(!doc.passthrough.contains_key("Assets/a.ogv"));
        assert!(doc.removed_files.contains("Assets/a.ogv"));
        h.undo(&mut doc);
        h.undo(&mut doc);
        assert!(doc.passthrough.contains_key("Assets/a.ogv"));
        assert!(!doc.passthrough.contains_key("Assets/b.mp3"));
        assert!(doc.removed_files.contains("Assets/b.mp3"));
        assert!(!doc.removed_files.contains("Assets/a.ogv"));
    }

    #[test]
    fn custom_unit_links_model_portrait_icon_and_sounds() {
        let r = UnitRecipe {
            id: "Hero".into(),
            name: "The Hero".into(),
            base: "Marine".into(),
            model: Some("Assets/Models/Hero.m3".into()),
            portrait: Some("Assets/Models/HeroPortrait.m3".into()),
            icon: Some("Assets/Textures/btn-hero.dds".into()),
            scale: Some(1.5),
            sounds: vec![("What".into(), "HeroWhat".into()), ("Ready".into(), "HeroReady".into())],
        };
        let (es, strings) = build_unit(&r);
        let xml: Vec<String> = es.iter().map(|e| e.to_xml()).collect();
        let all = xml.join("\n");
        assert!(all.contains(r#"<CUnit id="Hero" parent="Marine"/>"#), "{all}");
        assert!(all.contains(r#"<Model value="Assets\Models\Hero.m3"/>"#));
        assert!(all.contains(r#"<ScaleMax value="1.500000,1.500000,1.500000"/>"#));
        assert!(all.contains(r#"<CActorUnit id="Hero" parent="GenericUnitStandard" unitName="Hero">"#), "{all}");
        assert!(all.contains(r#"<PortraitModel value="HeroPortrait"/>"#));
        assert!(all.contains(r#"<SoundArray index="What" value="HeroWhat"/>"#));
        assert!(all.contains(r#"<GroupSoundArray index="What" value="HeroWhat"/>"#));
        assert!(!all.contains(r#"<GroupSoundArray index="Ready""#));
        assert!(all.contains(r#"<Icon value="Assets\Textures\btn-hero.dds"/>"#));
        assert_eq!(strings[0], ("Unit/Name/Hero".into(), "The Hero".into()));

        // Without a model the unit keeps the base unit's actor.
        let plain = UnitRecipe { id: "Hero2".into(), name: "x".into(), base: "Marine".into(), ..Default::default() };
        let (es, _) = build_unit(&plain);
        assert_eq!(es.len(), 2);
        assert_eq!(es[1].parent(), Some("Marine"));

        let mut cat = Catalog::default();
        assert!(unit_id_free(&cat, "Hero"));
        cat.entries.extend(build_unit(&r).0);
        assert!(!unit_id_free(&cat, "Hero"));
        assert_eq!(unique("Hero", |i| !unit_id_free(&cat, i)), "Hero2");
    }
}
