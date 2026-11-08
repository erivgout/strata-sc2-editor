//! The base game's GameData (units, abilities, weapons...) read from the
//! user's own StarCraft II install, so pickers list real units and the Data
//! panel shows real inherited values ("LifeMax 45 from Marine (base game)").
//!
//! Nothing here ships Blizzard data: a [`BaseCatalog`] is built at runtime
//! from the install ([`load_from_casc`]) and cached by the app as compact
//! bytes ([`BaseCatalog::to_bytes`]).
//!
//! Mods are merged in dependency order ([`MELEE_MODS`]) the way the game
//! does it: a later mod's `<CUnit id="Marine">` modifies the existing
//! entry. Scalar fields are replaced, `index`ed array items replace the item
//! at that index, other array items are appended and `removed="1"` deletes.
//! Entries without a `parent` inherit from their class default
//! (`<CUnit default="1">`).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::catalog::{CatalogEntry, CatalogKind, Catalog, FieldKey, ResolvedField, XmlNode};
use crate::known::UnitGroup;

/// The dependency chain of a standard melee map, in load order.
pub const MELEE_MODS: &[&str] = &["Core", "Liberty", "Swarm", "Void", "LibertyMulti", "SwarmMulti", "VoidMulti"];

/// GameStrings prefixes kept for display names.
const NAME_PREFIXES: &[&str] = &[
    "Unit/Name/",
    "Abil/Name/",
    "Weapon/Name/",
    "Effect/Name/",
    "Behavior/Name/",
    "Upgrade/Name/",
    "Button/Name/",
    "Model/Name/",
];

/// Format version of [`BaseCatalog::to_bytes`]; bump when the layout changes.
const FORMAT: u32 = 1;
const MAGIC: &[u8; 4] = b"STBD";

/// A placeable unit type from the base game, for pickers.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseUnit {
    pub id: String,
    /// Localized (enUS) name, or the id when the game has none.
    pub name: String,
    pub race: UnitGroup,
    /// `ObjectType` from `EditorCategories`: Unit, Structure, Hero,
    /// Resource, Destructible, Item...
    pub object_type: String,
    /// `ObjectFamily` from `EditorCategories`: Melee, Campaign, Storymode...
    pub family: String,
    pub structure: bool,
    pub air: bool,
    pub radius: f32,
    pub footprint: Option<String>,
    /// The mod that introduced the unit ("Liberty", "Void"...).
    pub source_mod: String,
}

impl BaseUnit {
    pub fn is_melee(&self) -> bool {
        self.family == "Melee"
    }
}

/// The merged base-game catalog plus enUS display names.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BaseCatalog {
    /// Game version the data came from, e.g. "5.0.17.98310".
    pub version: String,
    /// Mods merged, in load order.
    pub mods: Vec<String>,
    /// Merged entries. Class defaults keep their (usually empty) id.
    pub entries: Vec<CatalogEntry>,
    /// Index into `mods` of the mod that first defined each entry.
    pub origin: Vec<u8>,
    /// Display names: "Unit/Name/Marine" -> "Marine".
    pub strings: HashMap<String, String>,
    #[serde(skip)]
    index: HashMap<(CatalogKind, String), usize>,
    /// Class name -> its default entry.
    #[serde(skip)]
    defaults: HashMap<String, usize>,
    #[serde(skip)]
    units: Vec<BaseUnit>,
    #[serde(skip)]
    doodads: Vec<String>,
}

/// One field seen through the base chain.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseField<'a> {
    pub key: FieldKey,
    pub node: &'a XmlNode,
    /// The base entry that sets it.
    pub source: usize,
}

fn is_default(e: &CatalogEntry) -> bool {
    e.node.attr("default") == Some("1")
}

fn kind_of_file(file: &str) -> CatalogKind {
    let stem = file.rsplit('/').next().unwrap_or(file);
    let stem = stem.strip_suffix(".xml").or_else(|| stem.strip_suffix(".XML")).unwrap_or(stem);
    CatalogKind::ALL
        .into_iter()
        .find(|k| *k != CatalogKind::Other && k.file().eq_ignore_ascii_case(stem))
        .unwrap_or(CatalogKind::Other)
}

impl BaseCatalog {
    /// Starts building a catalog for game `version`.
    pub fn builder(version: impl Into<String>) -> BaseBuilder {
        BaseBuilder {
            cat: BaseCatalog { version: version.into(), ..Default::default() },
            by_key: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn rebuild(&mut self) {
        self.index.clear();
        self.defaults.clear();
        for (i, e) in self.entries.iter().enumerate() {
            if is_default(e) && e.id().is_empty() {
                self.defaults.entry(e.class().to_owned()).or_insert(i);
            } else {
                self.index.entry((e.kind(), e.id().to_owned())).or_insert(i);
            }
        }
        self.units = self.compute_units();
        let mut doodads: Vec<String> = self
            .entries
            .iter()
            .filter(|e| e.class() == "CActorDoodad" && !is_default(e) && !e.id().is_empty())
            .map(|e| e.id().to_owned())
            .collect();
        doodads.sort();
        doodads.dedup();
        self.doodads = doodads;
    }

    /// The entry of `kind` with this id.
    pub fn find(&self, kind: CatalogKind, id: &str) -> Option<usize> {
        self.index.get(&(kind, id.to_owned())).copied()
    }

    pub fn entry(&self, i: usize) -> Option<&CatalogEntry> {
        self.entries.get(i)
    }

    /// The mod that first defined entry `i`.
    pub fn origin_of(&self, i: usize) -> &str {
        self.origin.get(i).and_then(|&m| self.mods.get(m as usize)).map(String::as_str).unwrap_or("")
    }

    /// The parent of entry `i`: its `parent`, or its class default.
    pub fn parent_of(&self, i: usize) -> Option<usize> {
        let e = self.entries.get(i)?;
        match e.parent() {
            Some(p) => self.find(e.kind(), p).filter(|&j| j != i),
            None if is_default(e) && e.id().is_empty() => None,
            None => self.defaults.get(e.class()).copied().filter(|&j| j != i),
        }
    }

    /// `i` followed by its ancestors, nearest first. Stops at cycles.
    pub fn chain(&self, i: usize) -> Vec<usize> {
        let mut out = vec![i];
        let mut cur = i;
        while let Some(p) = self.parent_of(cur) {
            if out.contains(&p) || out.len() > 64 {
                break;
            }
            out.push(p);
            cur = p;
        }
        out
    }

    /// The element `key` as seen from entry `i`, with the entry that sets it.
    pub fn resolve(&self, i: usize, key: &FieldKey) -> Option<(&XmlNode, usize)> {
        self.chain(i).into_iter().find_map(|j| self.entries[j].get(key).map(|n| (n, j)))
    }

    /// The scalar `value` of `key` as seen from entry `i`.
    pub fn resolve_value(&self, i: usize, key: &FieldKey) -> Option<&str> {
        self.resolve(i, key).and_then(|(n, _)| n.attr("value"))
    }

    /// Scalar field `name` of the entry `kind`/`id`, through its parents.
    pub fn field(&self, kind: CatalogKind, id: &str, name: &str) -> Option<&str> {
        self.resolve_value(self.find(kind, id)?, &FieldKey::new(name))
    }

    /// Every field visible on entry `i`, nearest setter first.
    pub fn fields(&self, i: usize) -> Vec<BaseField<'_>> {
        let mut out: Vec<BaseField> = Vec::new();
        for j in self.chain(i) {
            for (key, node) in self.entries[j].node.keyed_children() {
                if !out.iter().any(|f| f.key == key) {
                    out.push(BaseField { key, node, source: j });
                }
            }
        }
        out
    }

    /// The base entry a map entry builds on: the base entry it names as
    /// parent at the top of its map-local chain, or the base entry of the
    /// same id that it modifies.
    pub fn underlying(&self, cat: &Catalog, index: usize) -> Option<usize> {
        let e = cat.entries.get(index)?;
        let kind = e.kind();
        if let Some(root) = cat.external_root(index) {
            return self.find(kind, root);
        }
        if cat.has_parent_cycle(index) {
            return None;
        }
        let top = cat.parent_chain(index).last().copied().unwrap_or(index);
        let te = &cat.entries[top];
        if te.parent().is_none() {
            return self.find(kind, te.id()).or_else(|| self.defaults.get(te.class()).copied());
        }
        None
    }

    /// Appends to `fields` (from [`Catalog::resolved_fields`]) the values
    /// map entry `index` inherits from the base game. Returns, per appended
    /// field, the base entry that sets it (same order).
    pub fn extend_resolved(&self, cat: &Catalog, index: usize, fields: &mut Vec<ResolvedField>) -> Vec<usize> {
        let Some(base) = self.underlying(cat, index) else { return Vec::new() };
        let mut sources = Vec::new();
        for f in self.fields(base) {
            if !fields.iter().any(|x| x.key == f.key) {
                fields.push(ResolvedField { key: f.key, node: f.node.clone(), source: index, inherited: true });
                sources.push(f.source);
            }
        }
        sources
    }

    /// The scalar value map entry `index` inherits from the base game.
    pub fn inherited_value(&self, cat: &Catalog, index: usize, key: &FieldKey) -> Option<(&str, usize)> {
        let base = self.underlying(cat, index)?;
        self.resolve(base, key).and_then(|(n, j)| n.attr("value").map(|v| (v, j)))
    }

    /// A localized string by key.
    pub fn string(&self, key: &str) -> Option<&str> {
        self.strings.get(key).map(String::as_str)
    }

    /// The display name of an entry (its `Name` field's string, or the
    /// conventional `<Kind>/Name/<id>` key).
    pub fn display_name(&self, kind: CatalogKind, id: &str) -> Option<&str> {
        if let Some(i) = self.find(kind, id) {
            if let Some(key) = self.resolve_value(i, &FieldKey::new("Name")) {
                if let Some(s) = self.string(key) {
                    return Some(s);
                }
            }
        }
        let prefix = match kind {
            CatalogKind::Unit => "Unit",
            CatalogKind::Ability => "Abil",
            CatalogKind::Weapon => "Weapon",
            CatalogKind::Effect => "Effect",
            CatalogKind::Behavior => "Behavior",
            CatalogKind::Upgrade => "Upgrade",
            CatalogKind::Button => "Button",
            CatalogKind::Model => "Model",
            _ => return None,
        };
        self.string(&format!("{prefix}/Name/{id}"))
    }

    /// Placeable units (units, structures, heroes, resources,
    /// destructibles, items), sorted by race then name.
    pub fn units(&self) -> &[BaseUnit] {
        &self.units
    }

    pub fn unit(&self, id: &str) -> Option<&BaseUnit> {
        self.units.iter().find(|u| u.id == id)
    }

    /// `CActorDoodad` ids, sorted.
    pub fn doodads(&self) -> &[String] {
        &self.doodads
    }

    /// Whether an id names a base-game unit (placeable or not).
    pub fn has_unit(&self, id: &str) -> bool {
        self.find(CatalogKind::Unit, id).is_some()
    }

    fn compute_units(&self) -> Vec<BaseUnit> {
        const PLACEABLE: &[&str] = &["Unit", "Structure", "Hero", "Resource", "Destructible", "Item", "Character"];
        let mut out = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if e.kind() != CatalogKind::Unit || e.class() != "CUnit" || is_default(e) || e.id().is_empty() {
                continue;
            }
            let get = |k: &FieldKey| self.resolve_value(i, k);
            let cats = get(&FieldKey::new("EditorCategories")).unwrap_or("");
            let mut object_type = "";
            let mut family = "";
            for part in cats.split(',') {
                match part.split_once(':') {
                    Some(("ObjectType", v)) => object_type = v,
                    Some(("ObjectFamily", v)) => family = v,
                    _ => {}
                }
            }
            if !PLACEABLE.contains(&object_type) {
                continue;
            }
            let race = match get(&FieldKey::new("Race")) {
                Some("Terr") => UnitGroup::Terran,
                Some("Prot") => UnitGroup::Protoss,
                Some("Zerg") => UnitGroup::Zerg,
                _ => UnitGroup::Neutral,
            };
            let id = e.id().to_owned();
            let name = self.display_name(CatalogKind::Unit, &id).unwrap_or(&id).to_owned();
            out.push(BaseUnit {
                name,
                race,
                structure: object_type == "Structure"
                    || get(&FieldKey::indexed("Attributes", "Structure")) == Some("1"),
                air: get(&FieldKey::indexed("PlaneArray", "Air")) == Some("1"),
                radius: get(&FieldKey::new("Radius")).and_then(|r| r.parse().ok()).unwrap_or(0.5),
                footprint: get(&FieldKey::new("Footprint")).map(str::to_owned),
                object_type: object_type.to_owned(),
                family: family.to_owned(),
                source_mod: self.origin_of(i).to_owned(),
                id,
            });
        }
        let race_order = |g: UnitGroup| UnitGroup::ALL.iter().position(|x| *x == g).unwrap_or(9);
        out.sort_by(|a, b| {
            race_order(a.race)
                .cmp(&race_order(b.race))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.id.cmp(&b.id))
        });
        out
    }

    /// Case-insensitive search by id or display name, best matches first.
    pub fn search(&self, query: &str, kind: Option<CatalogKind>, limit: usize) -> Vec<usize> {
        let q = query.trim().to_lowercase();
        let mut hits: Vec<(u8, usize)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            if kind.is_some_and(|k| k != e.kind()) || e.id().is_empty() {
                continue;
            }
            let id = e.id().to_lowercase();
            let name = self.display_name(e.kind(), e.id()).map(str::to_lowercase).unwrap_or_default();
            let rank = if q.is_empty() {
                3
            } else if id == q || name == q {
                0
            } else if id.starts_with(&q) || name.starts_with(&q) {
                1
            } else if id.contains(&q) || name.contains(&q) {
                2
            } else {
                continue;
            };
            hits.push((rank, i));
        }
        hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| self.entries[a.1].id().cmp(self.entries[b.1].id())));
        hits.into_iter().take(limit).map(|(_, i)| i).collect()
    }

    /// Compact bytes for caching: a header and deflated JSON.
    pub fn to_bytes(&self) -> Vec<u8> {
        let json = serde_json::to_vec(self).unwrap_or_default();
        let mut out = MAGIC.to_vec();
        out.extend_from_slice(&FORMAT.to_le_bytes());
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(&json, 6));
        out
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<BaseCatalog, String> {
        if bytes.len() < 8 || &bytes[..4] != MAGIC {
            return Err("not a Strata base-data cache".into());
        }
        if u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != FORMAT {
            return Err("base-data cache is from another Strata version".into());
        }
        let json = miniz_oxide::inflate::decompress_to_vec(&bytes[8..]).map_err(|e| format!("corrupt cache: {e:?}"))?;
        let mut cat: BaseCatalog = serde_json::from_slice(&json).map_err(|e| format!("corrupt cache: {e}"))?;
        cat.rebuild();
        Ok(cat)
    }
}

/// Builds a [`BaseCatalog`] mod by mod.
pub struct BaseBuilder {
    cat: BaseCatalog,
    by_key: HashMap<(CatalogKind, String, String), usize>,
}

impl BaseBuilder {
    /// Starts the next mod in load order.
    pub fn begin_mod(&mut self, name: &str) {
        self.cat.mods.push(name.to_owned());
    }

    fn current_mod(&self) -> u8 {
        self.cat.mods.len().saturating_sub(1) as u8
    }

    /// Merges one `GameData/*.xml` file of the current mod. Files that are
    /// not one of the catalog kinds the editor models are ignored. Returns
    /// the number of entries read.
    pub fn add_catalog(&mut self, file: &str, bytes: &[u8]) -> Result<usize, String> {
        let kind = kind_of_file(file);
        if kind == CatalogKind::Other {
            return Ok(0);
        }
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim_start_matches('\u{feff}');
        let root = XmlNode::parse(text).map_err(|e| format!("{file}: {e}"))?;
        if root.name != "Catalog" {
            return Ok(0);
        }
        let mut n = 0;
        for node in root.children {
            if !node.name.starts_with('C') {
                continue; // <const>, <Include>...
            }
            self.add_entry(kind, node);
            n += 1;
        }
        Ok(n)
    }

    fn add_entry(&mut self, kind: CatalogKind, node: XmlNode) {
        let id = node.attr("id").unwrap_or("").to_owned();
        // Class defaults are per class; named entries share one id space per kind.
        let class_key = if id.is_empty() { node.name.clone() } else { String::new() };
        let key = (kind, id, class_key);
        match self.by_key.get(&key) {
            Some(&i) => merge(&mut self.cat.entries[i].node, &node),
            None => {
                self.by_key.insert(key, self.cat.entries.len());
                self.cat.entries.push(CatalogEntry { file: kind.file().to_owned(), node });
                self.cat.origin.push(self.current_mod());
            }
        }
    }

    /// Adds a `GameStrings.txt` (`key=value` lines), keeping display names.
    pub fn add_strings(&mut self, bytes: &[u8]) {
        let text = String::from_utf8_lossy(bytes);
        for line in text.lines() {
            let line = line.trim_start_matches('\u{feff}');
            let Some((k, v)) = line.split_once('=') else { continue };
            if NAME_PREFIXES.iter().any(|p| k.starts_with(p)) {
                self.cat.strings.insert(k.to_owned(), v.trim_end_matches('\r').to_owned());
            }
        }
    }

    pub fn finish(mut self) -> BaseCatalog {
        self.cat.rebuild();
        self.cat
    }
}

/// Merges a later mod's declaration of an element into the existing one.
fn merge(base: &mut XmlNode, over: &XmlNode) {
    for (k, v) in &over.attrs {
        if k != "id" {
            base.set_attr(k, v.as_str());
        }
    }
    if !over.text.trim().is_empty() {
        base.text = over.text.clone();
    }
    for c in &over.children {
        let removed = c.attr("removed") == Some("1");
        let target = match c.attr("index") {
            Some(ix) => base
                .children
                .iter()
                .position(|b| b.name == c.name && b.attr("index") == Some(ix))
                .or_else(|| {
                    // A numeric index into an array written without indexes.
                    let n: usize = ix.parse().ok()?;
                    base.children
                        .iter()
                        .enumerate()
                        .filter(|(_, b)| b.name == c.name && b.attr("index").is_none())
                        .nth(n)
                        .map(|(i, _)| i)
                }),
            None => {
                let same: Vec<usize> = (0..base.children.len()).filter(|&i| base.children[i].name == c.name).collect();
                let is_array = c.name.ends_with("Array") || same.len() > 1;
                if is_array && !removed {
                    None
                } else {
                    same.first().copied()
                }
            }
        };
        match (target, removed) {
            (Some(i), true) => {
                base.children.remove(i);
            }
            (None, true) => {}
            (Some(i), false) => {
                let mut c2 = c.clone();
                // Keep the original's addressing (unindexed arrays stay unindexed).
                if base.children[i].attr("index").is_none() {
                    c2.remove_attr("index");
                }
                if c2.children.is_empty() && base.children[i].children.is_empty() {
                    let b = &mut base.children[i];
                    for (k, v) in &c2.attrs {
                        b.set_attr(k, v.as_str());
                    }
                } else {
                    merge(&mut base.children[i], &c2);
                }
            }
            (None, false) => base.children.push(c.clone()),
        }
    }
}

/// Paths a [`strata_casc::Storage`] must keep for [`load_from_casc`].
pub fn casc_filter(path: &str) -> bool {
    path.starts_with("mods/")
        && (path.contains(".sc2mod/base.sc2data/gamedata/") || path.ends_with("/enus.sc2data/localizeddata/gamestrings.txt"))
}

/// Builds the base catalog from an opened CASC storage. `progress` is told
/// which mod is being read.
pub async fn load_from_casc<S: strata_casc::Source>(
    storage: &strata_casc::Storage<S>,
    mods: &[&str],
    mut progress: impl FnMut(&str),
) -> Result<BaseCatalog, String> {
    let mut b = BaseCatalog::builder(storage.build.version.clone());
    for m in mods {
        progress(m);
        let lower = m.to_ascii_lowercase();
        let dir = format!("mods/{lower}.sc2mod/base.sc2data/gamedata/");
        let mut files: Vec<&str> = storage
            .files()
            .iter()
            .map(|f| f.path.as_str())
            .filter(|p| {
                p.len() > dir.len()
                    && p[..dir.len()].eq_ignore_ascii_case(&dir)
                    && !p[dir.len()..].contains('/')
                    && kind_of_file(p) != CatalogKind::Other
            })
            .collect();
        if files.is_empty() {
            return Err(format!("{m}.SC2Mod is not in this install"));
        }
        files.sort();
        b.begin_mod(m);
        for f in files {
            let bytes = storage.read(f).await.map_err(|e| format!("{f}: {e}"))?;
            b.add_catalog(f, &bytes)?;
        }
        let strings = format!("mods/{lower}.sc2mod/enus.sc2data/localizeddata/gamestrings.txt");
        if storage.find(&strings).is_some() {
            if let Ok(bytes) = storage.read(&strings).await {
                b.add_strings(&bytes);
            }
        }
    }
    Ok(b.finish())
}

/// Opens the install at `root` and builds the melee base catalog
/// (native only).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_install(root: &std::path::Path) -> Result<BaseCatalog, String> {
    let storage = strata_casc::block_on(strata_casc::Storage::open_filtered(strata_casc::DirSource::new(root), casc_filter))
        .map_err(|e| e.to_string())?;
    strata_casc::block_on(load_from_casc(&storage, MELEE_MODS, |_| {}))
}

/// The installed game's version from `.build.info`, cheap enough to check
/// whether a cached catalog is stale (native only).
#[cfg(not(target_arch = "wasm32"))]
pub fn install_version(root: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join(".build.info")).ok()?;
    strata_casc::BuildInfo::parse(&text).ok().map(|b| b.version)
}

/// Likely install folders on this machine (native only), existing ones first.
#[cfg(not(target_arch = "wasm32"))]
pub fn default_install_dirs() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(p) = std::env::var("STRATA_SC2_DIR") {
        out.push(std::path::PathBuf::from(p));
    }
    for var in ["ProgramFiles(x86)", "ProgramFiles", "ProgramW6432"] {
        if let Ok(p) = std::env::var(var) {
            out.push(std::path::Path::new(&p).join("StarCraft II"));
        }
    }
    for p in [
        "C:/Program Files (x86)/StarCraft II",
        "C:/Program Files/StarCraft II",
        "/Applications/StarCraft II",
    ] {
        out.push(std::path::PathBuf::from(p));
    }
    out.dedup();
    out.into_iter().filter(|p| p.join(".build.info").is_file()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CORE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Catalog>
    <!-- class default -->
    <CUnit default="1">
        <LifeMax value="10"/>
        <Radius value="0.5"/>
    </CUnit>
    <CUnit default="1" id="DESTRUCTIBLE">
        <Race value="Neut"/>
        <EditorCategories value="ObjectType:Destructible,ObjectFamily:Melee"/>
    </CUnit>
    <const id="$x" value="1"/>
</Catalog>"#;

    const LIBERTY: &str = r#"<Catalog>
    <CUnit id="Marine">
        <Race value="Terr"/>
        <LifeMax value="45"/>
        <EditorCategories value="ObjectType:Unit,ObjectFamily:Melee"/>
        <AbilArray Link="stop"/>
        <AbilArray Link="attack"/>
        <CostResource index="Minerals" value="50"/>
        <Name value="Unit/Name/Marine"/>
    </CUnit>
    <CUnit id="Rocks" parent="DESTRUCTIBLE">
        <Radius value="2"/>
        <Attributes index="Structure" value="1"/>
    </CUnit>
    <CUnit id="Missile">
        <EditorCategories value="ObjectType:Projectile"/>
    </CUnit>
</Catalog>"#;

    const MULTI: &str = r#"<Catalog>
    <CUnit id="Marine">
        <LifeMax value="55"/>
        <AbilArray index="1" Link="AttackBetter"/>
        <AbilArray Link="Stimpack"/>
        <CostResource index="Minerals" removed="1"/>
        <PlaneArray index="Air" value="1"/>
    </CUnit>
</Catalog>"#;

    fn sample() -> BaseCatalog {
        let mut b = BaseCatalog::builder("1.2.3");
        b.begin_mod("Core");
        assert_eq!(b.add_catalog("mods/core.sc2mod/base.sc2data/gamedata/unitdata.xml", CORE.as_bytes()), Ok(2));
        assert_eq!(b.add_catalog("ignored/sounddata.xml", CORE.as_bytes()), Ok(0));
        b.begin_mod("Liberty");
        b.add_catalog("UnitData.xml", LIBERTY.as_bytes()).unwrap();
        b.add_strings(b"\xef\xbb\xbfUnit/Name/Marine=Marine\r\nUnit/Name/Rocks=Destructible Rocks\nButton/Tooltip/X=skip\n");
        b.begin_mod("VoidMulti");
        b.add_catalog("unitdata.xml", MULTI.as_bytes()).unwrap();
        b.finish()
    }

    #[test]
    fn merges_mods_in_order() {
        let c = sample();
        let m = c.find(CatalogKind::Unit, "Marine").unwrap();
        assert_eq!(c.origin_of(m), "Liberty");
        assert_eq!(c.field(CatalogKind::Unit, "Marine", "LifeMax"), Some("55"));
        let abils: Vec<&str> = c.entries[m]
            .node
            .children
            .iter()
            .filter(|n| n.name == "AbilArray")
            .filter_map(|n| n.attr("Link"))
            .collect();
        assert_eq!(abils, ["stop", "AttackBetter", "Stimpack"]);
        assert!(c.entries[m].get(&FieldKey::indexed("CostResource", "Minerals")).is_none());
        assert_eq!(c.strings.len(), 2);
        assert_eq!(c.display_name(CatalogKind::Unit, "Rocks"), Some("Destructible Rocks"));
    }

    #[test]
    fn inherits_from_parents_and_class_defaults() {
        let c = sample();
        let rocks = c.find(CatalogKind::Unit, "Rocks").unwrap();
        // Rocks -> DESTRUCTIBLE -> CUnit default.
        assert_eq!(c.chain(rocks).len(), 3);
        assert_eq!(c.field(CatalogKind::Unit, "Rocks", "LifeMax"), Some("10"));
        assert_eq!(c.field(CatalogKind::Unit, "Rocks", "Race"), Some("Neut"));
        let fields = c.fields(rocks);
        assert_eq!(fields[0].key.name, "Radius");
        assert_eq!(fields[0].node.attr("value"), Some("2"));
    }

    #[test]
    fn unit_list() {
        let c = sample();
        let ids: Vec<&str> = c.units().iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["Marine", "Rocks"]);
        let marine = c.unit("Marine").unwrap();
        assert_eq!((marine.race, marine.air, marine.structure), (UnitGroup::Terran, true, false));
        let rocks = c.unit("Rocks").unwrap();
        assert_eq!((rocks.race, rocks.structure, rocks.radius), (UnitGroup::Neutral, true, 2.0));
        assert_eq!(rocks.name, "Destructible Rocks");
        assert_eq!(c.search("rock", Some(CatalogKind::Unit), 10), vec![rocks_index(&c)]);
    }

    fn rocks_index(c: &BaseCatalog) -> usize {
        c.find(CatalogKind::Unit, "Rocks").unwrap()
    }

    #[test]
    fn map_entries_fall_through_to_base() {
        let c = sample();
        let mut map = Catalog::default();
        let mut mine = CatalogEntry::new("UnitData", "CUnit", "MyMarine", Some("Marine"));
        mine.set_field("Speed", "3");
        map.entries.push(mine);
        // A map entry that edits the base Marine directly.
        let mut edit = CatalogEntry::new("UnitData", "CUnit", "Marine", None);
        edit.set_field("LifeMax", "100");
        map.entries.push(edit);

        let mut fields = map.resolved_fields(0);
        let sources = c.extend_resolved(&map, 0, &mut fields);
        let own = map.resolved_fields(0).len();
        // LifeMax comes from the map's own Marine edit, Race from the base game.
        let life = fields.iter().find(|f| f.key.name == "LifeMax").unwrap();
        assert_eq!((life.node.attr("value"), life.source), (Some("100"), 1));
        let race = fields.iter().position(|f| f.key.name == "Race").unwrap();
        assert!(race >= own && fields[race].inherited);
        assert_eq!(c.entries[sources[race - own]].id(), "Marine");
        assert_eq!(fields.iter().filter(|f| f.key.name == "LifeMax").count(), 1);
        assert_eq!(
            c.inherited_value(&map, 1, &FieldKey::new("Race")).map(|(v, _)| v),
            Some("Terr")
        );
    }

    #[test]
    fn cache_roundtrip() {
        let c = sample();
        let bytes = c.to_bytes();
        let back = BaseCatalog::from_bytes(&bytes).unwrap();
        assert_eq!(back.len(), c.len());
        assert_eq!(back.units(), c.units());
        assert_eq!(back.field(CatalogKind::Unit, "Rocks", "LifeMax"), Some("10"));
        assert!(BaseCatalog::from_bytes(b"nope").is_err());
    }
}
