//! Document checks surfaced in the Problems panel and via MCP.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::base_data::{BaseCatalog, MELEE_MODS};
use crate::catalog::{CatalogKind, FieldKey};
use crate::catalog_schema::{self, FieldType};
use crate::document::MapDocument;
use crate::objects::{ObjectId, ObjectKind};
use crate::strings::DEFAULT_LOCALE;
use crate::PlayerControl;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// What a problem points at, so the UI can jump to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Target {
    Map,
    Object(ObjectId),
    Region(ObjectId),
    Trigger(String),
    Data { class: String, id: String },
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub severity: Severity,
    pub message: String,
    pub target: Target,
}

/// Unit types that fly, so placing them over unpathable ground is fine.
const AIR_UNITS: &[&str] = &[
    "Medivac",
    "VikingFighter",
    "Banshee",
    "Raven",
    "Battlecruiser",
    "Liberator",
    "LiberatorAG",
    "Observer",
    "ObserverSiegeMode",
    "WarpPrism",
    "WarpPrismPhasing",
    "Phoenix",
    "VoidRay",
    "Oracle",
    "Tempest",
    "Carrier",
    "Mothership",
    "Interceptor",
    "Overlord",
    "OverlordTransport",
    "Overseer",
    "OverseerSiegeMode",
    "Mutalisk",
    "Corruptor",
    "BroodLord",
    "Viper",
];

pub fn validate(doc: &MapDocument) -> Vec<Problem> {
    validate_with(doc, None)
}

/// [`validate`], plus a reference audit when the base game's data is
/// loaded: parents, link fields and placed unit and doodad types must name
/// an entry in the map or the game, not just look plausible. Without it
/// (or for maps that depend on mods Strata doesn't load) only near-miss
/// typos are caught, and a note says the check was limited.
pub fn validate_with(doc: &MapDocument, base: Option<&BaseCatalog>) -> Vec<Problem> {
    let mut out = Vec::new();
    let refs = References::new(doc, base);
    if let Some(note) = refs.coverage_note() {
        out.push(Problem {
            severity: Severity::Info,
            message: note,
            target: Target::Map,
        });
    }
    let mut push = |severity, message: String, target| {
        out.push(Problem {
            severity,
            message,
            target,
        })
    };

    // Map info.
    let [l, b, r, t] = doc.info.playable;
    if l >= r || b >= t || r > doc.info.width || t > doc.info.height {
        push(
            Severity::Error,
            "Playable area is empty or extends past the map edge.".into(),
            Target::Map,
        );
    }
    if doc.info.name.trim().is_empty() {
        push(
            Severity::Warning,
            "Map has no name. Set one in Map properties.".into(),
            Target::Map,
        );
    }
    let user_slots = doc
        .info
        .players
        .iter()
        .filter(|p| p.control == PlayerControl::User)
        .count();
    if user_slots == 0 {
        push(
            Severity::Error,
            "No player slot is controlled by a user, so nobody can join.".into(),
            Target::Map,
        );
    }

    // Players and start locations.
    let mut ids = HashSet::new();
    for p in &doc.info.players {
        if !ids.insert(p.id) {
            push(
                Severity::Error,
                format!(
                    "Two player slots use id {}. Give each slot its own id.",
                    p.id
                ),
                Target::Map,
            );
        }
        if p.id == 0 || p.id > 15 {
            push(
                Severity::Error,
                format!("Player slot id {} is out of range; use 1 to 15.", p.id),
                Target::Map,
            );
        }
    }
    if doc.info.players.len() > 15 {
        push(
            Severity::Error,
            format!(
                "The map has {} player slots; SC2 allows at most 15.",
                doc.info.players.len()
            ),
            Target::Map,
        );
    }
    let playing: Vec<_> = doc
        .info
        .players
        .iter()
        .filter(|p| matches!(p.control, PlayerControl::User | PlayerControl::Computer))
        .collect();
    let starts_from_players = playing
        .iter()
        .filter(|p| p.start_location.is_some())
        .count();
    let starts_from_objects = doc
        .objects
        .objects
        .iter()
        .filter(|o| o.kind == ObjectKind::Point && o.type_id == "StartLoc")
        .count();
    let starts = starts_from_players.max(starts_from_objects);
    if !playing.is_empty() && starts < playing.len() {
        let slots = playing.len();
        let msg = if starts == 0 {
            format!("{slots} player slots but no start locations. Place a start location for each player.")
        } else {
            format!("{slots} player slots but only {starts} start location{}. Place one for each player.", plural(starts))
        };
        push(Severity::Warning, msg, Target::Map);
    }
    for p in &doc.info.players {
        if let Some(s) = p.start_location {
            if s.x < l as f32 || s.y < b as f32 || s.x > r as f32 || s.y > t as f32 {
                push(
                    Severity::Warning,
                    format!("Start location of {} is outside the playable area.", p.name),
                    Target::Map,
                );
            }
        }
    }

    // Objects.
    let (w, h) = (doc.terrain.width as f32, doc.terrain.height as f32);
    let mut point_names: HashMap<&str, usize> = HashMap::new();
    for o in &doc.objects.objects {
        let p = o.position;
        let outside = p.x < 0.0 || p.y < 0.0 || p.x > w || p.y > h;
        if outside {
            push(
                Severity::Error,
                format!("{} is outside the map.", o.display_name()),
                Target::Object(o.id),
            );
        }
        match o.kind {
            ObjectKind::Unit => {
                if o.type_id.is_empty() {
                    push(
                        Severity::Error,
                        "A unit has no unit type.".into(),
                        Target::Object(o.id),
                    );
                }
                if let Some(why) = refs.missing(CatalogKind::Unit, &o.type_id) {
                    push(
                        Severity::Error,
                        format!("{} is a \"{}\" unit, which {why}. It won't appear in the game.", o.display_name(), o.type_id),
                        Target::Object(o.id),
                    );
                }
                if o.player as usize > 15 {
                    push(
                        Severity::Error,
                        format!(
                            "{} belongs to player {}, which does not exist.",
                            o.display_name(),
                            o.player
                        ),
                        Target::Object(o.id),
                    );
                }
                if !outside
                    && !o.type_id.is_empty()
                    && is_unwalkable(doc, p.x, p.y)
                    && !is_air_unit(doc, &o.type_id)
                {
                    push(
                        Severity::Warning,
                        format!("{} stands on unpathable ground and may be stuck. Move it or clear the pathing.", o.display_name()),
                        Target::Object(o.id),
                    );
                }
            }
            ObjectKind::Point => {
                if !o.name.is_empty() {
                    *point_names.entry(o.name.as_str()).or_default() += 1;
                }
            }
            ObjectKind::Doodad => {
                if let Some(why) = refs.missing(CatalogKind::Actor, &o.type_id) {
                    push(
                        Severity::Warning,
                        format!("Doodad \"{}\" {why}. It won't appear in the game.", o.type_id),
                        Target::Object(o.id),
                    );
                }
            }
        }
    }
    let mut reported_points = HashSet::new();
    for o in &doc.objects.objects {
        if o.kind == ObjectKind::Point
            && point_names.get(o.name.as_str()).copied().unwrap_or(0) > 1
            && reported_points.insert(o.name.as_str())
        {
            push(
                Severity::Warning,
                format!("Several points are named \"{}\". Triggers can only find one of them; rename the others.", o.name),
                Target::Object(o.id),
            );
        }
    }

    // Triggers.
    let mut seen = HashSet::new();
    for t in doc.triggers.triggers() {
        let id = crate::galaxy::ident(&t.name);
        if !seen.insert(id.clone()) {
            push(
                Severity::Error,
                format!("Two triggers compile to the same name \"{id}\". Rename one."),
                Target::Trigger(t.name.clone()),
            );
        }
        if t.events.is_empty() && t.enabled {
            push(
                Severity::Info,
                format!(
                    "Trigger \"{}\" has no events, so it only runs when another trigger calls it.",
                    t.name
                ),
                Target::Trigger(t.name.clone()),
            );
        }
    }

    for (name, message) in crate::triggers::check(&doc.triggers) {
        push(Severity::Warning, format!("{name}: {message}"), Target::Trigger(name));
    }
    // Only lint the user's custom script: generated code is covered by
    // `triggers::check`, and linting it would report codegen details users can't fix.
    if !doc.triggers.custom_script.trim().is_empty() {
        for d in crate::galaxy::lint(&doc.triggers.custom_script) {
            let severity = match d.severity {
                crate::galaxy::LintSeverity::Error => Severity::Error,
                _ => Severity::Warning,
            };
            push(
                severity,
                format!("Custom script line {}: {}", d.line, d.message),
                Target::Trigger("Custom script".into()),
            );
        }
    }

    // Regions.
    let mut region_names = HashSet::new();
    for r in &doc.regions {
        if r.shapes.is_empty() {
            push(
                Severity::Warning,
                format!("Region \"{}\" has no shapes.", r.name),
                Target::Region(r.id),
            );
        }
        if !region_names.insert(r.name.as_str()) {
            push(
                Severity::Error,
                format!(
                    "Two regions are named \"{}\". Rename one so triggers can tell them apart.",
                    r.name
                ),
                Target::Region(r.id),
            );
        }
    }

    check_catalog(doc, &refs, &mut out);
    check_strings(doc, &mut out);

    out.sort_by_key(|p| p.severity);
    out
}

fn plural(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

fn is_unwalkable(doc: &MapDocument, x: f32, y: f32) -> bool {
    let t = &doc.terrain;
    let (cx, cy) = (x.floor() as i64, y.floor() as i64);
    if cx < 0 || cy < 0 || cx >= t.width as i64 || cy >= t.height as i64 {
        return false;
    }
    t.pathing
        .get(t.cell_index(cx as u32, cy as u32))
        .is_some_and(|f| f & crate::terrain::pathing::UNWALKABLE != 0)
}

fn is_air_unit(doc: &MapDocument, type_id: &str) -> bool {
    if AIR_UNITS.contains(&type_id) {
        return true;
    }
    let Some(i) = doc
        .data
        .entries
        .iter()
        .position(|e| e.kind() == CatalogKind::Unit && e.id() == type_id)
    else {
        // A map-local unit derived from a known air unit.
        return false;
    };
    let c = &doc.data;
    if c.resolve_value(i, &FieldKey::new("Mover"))
        .is_some_and(|(v, _)| v == "Fly")
        || c.resolve_value(i, &FieldKey::indexed("PlaneArray", "Air"))
            .is_some_and(|(v, _)| v == "1")
    {
        return true;
    }
    c.external_root(i).is_some_and(|p| AIR_UNITS.contains(&p))
}

/// A case-insensitive near miss for `id` among `candidates`, if `id` itself is absent.
fn did_you_mean<'a>(id: &str, candidates: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut near = None;
    for c in candidates {
        if c == id {
            return None;
        }
        if near.is_none() && c.eq_ignore_ascii_case(id) {
            near = Some(c);
        }
    }
    near
}

/// Map plus game data, for telling whether an id names a real entry.
struct References<'a> {
    doc: &'a MapDocument,
    /// The game's data, when it covers everything the map depends on.
    base: Option<&'a BaseCatalog>,
    /// Dependencies beyond the melee mods, which the game data lacks.
    uncovered: Vec<String>,
    loaded: bool,
}

impl<'a> References<'a> {
    fn new(doc: &'a MapDocument, base: Option<&'a BaseCatalog>) -> Self {
        let uncovered = extra_dependencies(&doc.info.dependencies);
        let full = base.filter(|_| uncovered.is_empty());
        Self { doc, base: full, uncovered, loaded: base.is_some() }
    }

    /// Why the check was limited, when the map has data that needed it.
    fn coverage_note(&self) -> Option<String> {
        if self.base.is_some() || self.doc.data.entries.is_empty() {
            return None;
        }
        if !self.loaded {
            return Some(
                "Data references were only checked for typos. Load the game data (from your StarCraft II install) to check that every parent and link exists.".to_owned(),
            );
        }
        Some(format!(
            "Data references were only checked for typos: this map depends on {}, whose data Strata doesn't load.",
            self.uncovered.join(", ")
        ))
    }

    /// `None` when `id` names an entry of `kind` (or can't be checked);
    /// otherwise why it's missing, e.g. "isn't in the map or the game's data".
    fn missing(&self, kind: CatalogKind, id: &str) -> Option<String> {
        let base = self.base?;
        if id.is_empty() || is_token(id) || base.find(kind, id).is_some() {
            return None;
        }
        let cat = &self.doc.data;
        if cat.entries.iter().any(|e| e.kind() == kind && e.id() == id) {
            return None;
        }
        let near = cat
            .entries
            .iter()
            .filter(|e| e.kind() == kind)
            .map(|e| e.id())
            .find(|c| c.eq_ignore_ascii_case(id))
            .or_else(|| base.find_ignore_case(kind, id));
        Some(match near {
            Some(n) => format!("isn't in the map or the game's data (did you mean \"{n}\"? Ids are case-sensitive)"),
            None => "isn't in the map or the game's data".to_owned(),
        })
    }
}

/// Catalog values that are filled in later: `##id##` tokens and parameters.
fn is_token(v: &str) -> bool {
    v.contains("##") || v.starts_with('$') || v.contains('{')
}

/// The map's dependencies that are not one of the melee mods, by file name
/// (`file:Mods/StarCoop/StarCoop.SC2Mod` -> "StarCoop.SC2Mod").
pub fn extra_dependencies(deps: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for d in deps {
        let path = d.rsplit_once("file:").map(|(_, p)| p).unwrap_or(d).trim().replace('\\', "/");
        let Some(file) = path.rsplit('/').next().filter(|f| !f.is_empty()) else { continue };
        let lower = path.to_ascii_lowercase();
        let melee = MELEE_MODS.iter().any(|m| lower == format!("mods/{}.sc2mod", m.to_ascii_lowercase()));
        if !melee && !out.iter().any(|o| o == file) {
            out.push(file.to_owned());
        }
    }
    out
}

fn check_catalog(doc: &MapDocument, refs: &References, out: &mut Vec<Problem>) {
    let cat = &doc.data;
    let target = |e: &crate::catalog::CatalogEntry| Target::Data {
        class: e.class().to_owned(),
        id: e.id().to_owned(),
    };
    let mut seen: HashSet<(&str, &str)> = HashSet::new();
    let mut by_kind: BTreeMap<CatalogKind, Vec<&str>> = BTreeMap::new();
    for e in &cat.entries {
        by_kind.entry(e.kind()).or_default().push(e.id());
    }
    for (i, e) in cat.entries.iter().enumerate() {
        let (class, id) = (e.class(), e.id());
        if id.trim().is_empty() {
            out.push(Problem {
                severity: Severity::Error,
                message: format!("A {class} entry has no id. Give it one in Data."),
                target: target(e),
            });
            continue;
        }
        if !seen.insert((class, id)) {
            out.push(Problem {
                severity: Severity::Error,
                message: format!("Two {class} entries are named \"{id}\". Only one of them takes effect; rename or delete the other."),
                target: target(e),
            });
        }
        let kind = e.kind();
        if let Some(p) = e.parent() {
            if p.trim().is_empty() {
                out.push(Problem {
                    severity: Severity::Warning,
                    message: format!(
                        "{id} has an empty parent. Remove the parent or name one, such as Marine."
                    ),
                    target: target(e),
                });
            } else if p == id && cat.parent_of(i).is_none() {
                out.push(Problem {
                    severity: Severity::Error,
                    message: format!("{id} is its own parent. Pick a different parent."),
                    target: target(e),
                });
            } else if cat.has_parent_cycle(i) {
                out.push(Problem {
                    severity: Severity::Error,
                    message: format!("{id} inherits from itself through its parents. Break the loop by changing a parent."),
                    target: target(e),
                });
            } else if cat.parent_of(i).is_none() {
                let same_kind = by_kind.get(&kind).into_iter().flatten().copied();
                let base = catalog_schema::base_ids(kind).iter().copied();
                let other_kind = cat.entries.iter().any(|o| o.id() == p && o.kind() != kind);
                let checked = refs.base.is_some() && !other_kind;
                if let Some(why) = refs.missing(kind, p).filter(|_| checked) {
                    out.push(Problem {
                        severity: Severity::Error,
                        message: format!("{id}'s parent \"{p}\" {why}. Pick an existing parent."),
                        target: target(e),
                    });
                } else if checked {
                    // Found in the game's data.
                } else if let Some(near) = did_you_mean(p, same_kind.chain(base)) {
                    out.push(Problem {
                        severity: Severity::Warning,
                        message: format!("{id}'s parent \"{p}\" was not found. Did you mean \"{near}\"? Ids are case-sensitive."),
                        target: target(e),
                    });
                } else if let Some(other) =
                    cat.entries.iter().find(|o| o.id() == p && o.kind() != kind)
                {
                    out.push(Problem {
                        severity: Severity::Warning,
                        message: format!("{id}'s parent \"{p}\" is a {}, not a {}. Parents must be the same kind of data.", other.kind().noun(), kind.noun()),
                        target: target(e),
                    });
                }
            }
        }
        // Link fields that look like typos.
        for (key, node) in e.node.keyed_children() {
            let Some(def) = catalog_schema::field_def(class, &key.name) else {
                continue;
            };
            let (link_kind, attr) = match def.ty {
                FieldType::Link(k) => (k, "value"),
                FieldType::LinkArray(k) => (k, "Link"),
                _ => continue,
            };
            let Some(v) = node
                .attr(attr)
                .or_else(|| node.attr("value"))
                .filter(|v| !v.is_empty())
            else {
                continue;
            };
            if refs.base.is_some() {
                if let Some(why) = refs.missing(link_kind, v) {
                    out.push(Problem {
                        severity: Severity::Warning,
                        message: format!("{id}.{key} links to {} \"{v}\", which {why}.", link_kind.noun()),
                        target: target(e),
                    });
                }
                continue;
            }
            let map_ids = by_kind.get(&link_kind).into_iter().flatten().copied();
            let base = catalog_schema::base_ids(link_kind).iter().copied();
            if let Some(near) = did_you_mean(v, map_ids.chain(base)) {
                out.push(Problem {
                    severity: Severity::Warning,
                    message: format!("{id}.{key} links to \"{v}\", which was not found. Did you mean \"{near}\"?"),
                    target: target(e),
                });
            }
        }
    }
}

/// Text keys the map refers to, with what refers to them.
pub fn referenced_strings(doc: &MapDocument) -> Vec<(String, String)> {
    let mut out = vec![("DocInfo/Name".to_owned(), "Map name".to_owned())];
    for e in &doc.data.entries {
        for (key, node) in e.node.keyed_children() {
            let is_key = catalog_schema::field_def(e.class(), &key.name)
                .is_some_and(|d| d.ty == FieldType::StringKey);
            if let Some(v) = node.attr("value").filter(|v| is_key && v.contains('/')) {
                out.push((v.to_owned(), format!("{}.{key}", e.id())));
            }
        }
        if e.kind() == CatalogKind::Unit && e.field("Name").is_none() && !e.id().is_empty() {
            out.push((
                format!("Unit/Name/{}", e.id()),
                format!("Name of unit {}", e.id()),
            ));
        }
    }
    out
}

fn check_strings(doc: &MapDocument, out: &mut Vec<Problem>) {
    let base = doc.strings.locales.get(DEFAULT_LOCALE);
    let has = |k: &str| base.is_some_and(|m| m.get(k).is_some_and(|v| !v.trim().is_empty()));
    if !doc.info.name.trim().is_empty() && !has("DocInfo/Name") {
        out.push(Problem {
            severity: Severity::Warning,
            message: "The map name text (DocInfo/Name) is missing, so the lobby shows no name. Set the name in Map properties.".into(),
            target: Target::String("DocInfo/Name".into()),
        });
    }
    let mut reported = HashSet::new();
    for (key, why) in referenced_strings(doc) {
        if key == "DocInfo/Name" || has(&key) || !reported.insert(key.clone()) {
            continue;
        }
        let base_unit = key
            .strip_prefix("Unit/Name/")
            .is_some_and(|id| catalog_schema::base_ids(CatalogKind::Unit).contains(&id));
        if base_unit {
            continue; // the base game provides it
        }
        let generated = why.starts_with("Name of unit");
        out.push(Problem {
            severity: if generated {
                Severity::Info
            } else {
                Severity::Warning
            },
            message: if generated {
                format!("{why} has no text ({key}), so the game shows the raw key. Add it in Text.")
            } else {
                format!("{why} uses text {key}, which does not exist. Add it in Text.")
            },
            target: Target::String(key),
        });
    }

    if let Some(base) = base {
        for (loc, map) in &doc.strings.locales {
            if loc == DEFAULT_LOCALE {
                continue;
            }
            let missing = base.keys().filter(|k| !map.contains_key(*k)).count();
            if missing > 0 {
                out.push(Problem {
                    severity: Severity::Info,
                    message: format!(
                        "{loc} is missing {missing} translated string{}.",
                        plural(missing)
                    ),
                    target: Target::Map,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogEntry;
    use crate::objects::PlacedObject;
    use crate::regions::{Region, Shape};

    fn has(problems: &[Problem], needle: &str) -> bool {
        problems.iter().any(|p| p.message.contains(needle))
    }

    #[test]
    fn fresh_map_only_warns_about_start_locations() {
        let doc = MapDocument::new("Test", 64, 64);
        let p = validate(&doc);
        assert!(p.iter().all(|p| p.severity != Severity::Error), "{p:?}");
        assert!(has(&p, "no start locations"));
    }

    #[test]
    fn duplicate_names_and_pathing() {
        let mut doc = MapDocument::new("Test", 32, 32);
        let shape = Shape::Rect {
            min: glam::Vec2::ZERO,
            max: glam::Vec2::ONE,
        };
        for _ in 0..2 {
            let id = doc.objects.alloc_id();
            doc.regions.push(Region {
                id,
                name: "Base".into(),
                color: [0, 0, 0],
                shapes: vec![shape.clone()],
            });
        }
        for _ in 0..2 {
            let id = doc.objects.alloc_id();
            let mut o =
                PlacedObject::new(id, ObjectKind::Point, "", glam::Vec3::new(3.0, 3.0, 0.0));
            o.name = "Spawn".into();
            doc.objects.objects.push(o);
        }
        let i = doc.terrain.cell_index(10, 10);
        doc.terrain.pathing[i] |= crate::terrain::pathing::UNWALKABLE;
        let id = doc.objects.alloc_id();
        doc.objects.objects.push(PlacedObject::new(
            id,
            ObjectKind::Unit,
            "Marine",
            glam::Vec3::new(10.5, 10.5, 0.0),
        ));
        let id = doc.objects.alloc_id();
        doc.objects.objects.push(PlacedObject::new(
            id,
            ObjectKind::Unit,
            "Medivac",
            glam::Vec3::new(10.5, 10.5, 0.0),
        ));
        let p = validate(&doc);
        assert!(has(&p, "Two regions are named \"Base\""));
        assert!(has(&p, "Several points are named \"Spawn\""));
        assert_eq!(
            p.iter()
                .filter(|p| p.message.contains("unpathable"))
                .count(),
            1
        );
    }

    #[test]
    fn catalog_parent_checks() {
        let mut doc = MapDocument::new("Test", 32, 32);
        doc.data
            .entries
            .push(CatalogEntry::new("UnitData", "CUnit", "A", Some("marine")));
        doc.data
            .entries
            .push(CatalogEntry::new("UnitData", "CUnit", "B", Some("C")));
        doc.data
            .entries
            .push(CatalogEntry::new("UnitData", "CUnit", "C", Some("B")));
        doc.data.entries.push(CatalogEntry::new(
            "UnitData",
            "CUnit",
            "D",
            Some("SomeModUnit"),
        ));
        doc.data
            .entries
            .push(CatalogEntry::new("UnitData", "CUnit", "D", Some("Marine")));
        let mut w = CatalogEntry::new("WeaponData", "CWeaponLegacy", "W", None);
        w.set_field("Effect", "gaussrifle");
        doc.data.entries.push(w);
        let p = validate(&doc);
        assert!(has(&p, "Did you mean \"Marine\""));
        assert!(has(&p, "inherits from itself"));
        assert!(has(&p, "Two CUnit entries are named \"D\""));
        assert!(has(&p, "Did you mean \"GaussRifle\""));
        // Unknown external parents are not flagged: they may come from a dependency.
        assert!(!has(&p, "SomeModUnit"));
    }

    #[test]
    fn string_checks() {
        let mut doc = MapDocument::new("Test", 32, 32);
        doc.strings.remove("DocInfo/Name");
        let mut u = CatalogEntry::new("UnitData", "CUnit", "MyMarine", Some("Marine"));
        u.set_field("Description", "Unit/Description/Missing");
        doc.data.entries.push(u);
        doc.data
            .entries
            .push(CatalogEntry::new("UnitData", "CUnit", "Marine", None));
        let p = validate(&doc);
        assert!(has(&p, "DocInfo/Name"));
        assert!(has(&p, "Unit/Description/Missing, which does not exist"));
        assert!(has(&p, "Unit/Name/MyMarine"));
        assert!(!has(&p, "Unit/Name/Marine)"));
        assert!(p.windows(2).all(|w| w[0].severity <= w[1].severity));
    }

    fn game() -> BaseCatalog {
        let mut b = BaseCatalog::builder("test");
        b.begin_mod("Liberty");
        b.add_catalog(
            "UnitData.xml",
            br#"<Catalog><CUnit default="1"/><CUnit id="Marine"/><CUnit id="SiegeTank"/></Catalog>"#,
        )
        .unwrap();
        b.add_catalog(
            "WeaponData.xml",
            br#"<Catalog><CWeaponLegacy id="GuassRifle"/></Catalog>"#,
        )
        .unwrap();
        b.add_catalog(
            "ActorData.xml",
            br#"<Catalog><CActorDoodad id="AgriaRock"/></Catalog>"#,
        )
        .unwrap();
        b.finish()
    }

    fn link_node(weapon: &str) -> crate::catalog::XmlNode {
        let mut n = crate::catalog::XmlNode::new("WeaponArray");
        n.set_attr("Link", weapon);
        n
    }

    fn audit_doc() -> MapDocument {
        let mut doc = MapDocument::new("Audit", 64, 64);
        let mut ok = CatalogEntry::new("UnitData", "CUnit", "MyMarine", Some("Marine"));
        ok.node.children.push(link_node("GuassRifle"));
        let mut bad = CatalogEntry::new("UnitData", "CUnit", "MyTank", Some("SeigeTank"));
        bad.node.children.push(link_node("NoSuchGun"));
        let lower = CatalogEntry::new("UnitData", "CUnit", "MyOther", Some("marine"));
        doc.data.entries = vec![ok, bad, lower];
        let mut u = PlacedObject::new(ObjectId(50), ObjectKind::Unit, "Marien", glam::Vec3::new(10.0, 10.0, 0.0));
        u.player = 1;
        let d = PlacedObject::new(ObjectId(51), ObjectKind::Doodad, "AgriaRock", glam::Vec3::new(12.0, 12.0, 0.0));
        let d2 = PlacedObject::new(ObjectId(52), ObjectKind::Doodad, "NotARock", glam::Vec3::new(14.0, 12.0, 0.0));
        let mine = PlacedObject::new(ObjectId(53), ObjectKind::Unit, "MyMarine", glam::Vec3::new(16.0, 12.0, 0.0));
        doc.objects.objects.extend([u, d, d2, mine]);
        doc
    }

    #[test]
    fn game_data_turns_typo_checks_into_a_reference_audit() {
        let doc = audit_doc();
        let base = game();
        let p = validate_with(&doc, Some(&base));
        assert!(has(&p, "MyTank's parent \"SeigeTank\" isn't in the map or the game's data"), "{p:#?}");
        assert!(has(&p, "links to weapon \"NoSuchGun\""), "{p:#?}");
        assert!(has(&p, "did you mean \"Marine\""), "{p:#?}");
        assert!(has(&p, "\"Marien\" unit"), "{p:#?}");
        assert!(has(&p, "Doodad \"NotARock\""), "{p:#?}");
        assert!(!has(&p, "GuassRifle"), "{p:#?}");
        assert!(!has(&p, "\"MyMarine\" unit"), "{p:#?}");
        assert!(!has(&p, "Doodad \"AgriaRock\""), "{p:#?}");
        assert!(!has(&p, "only checked for typos"), "{p:#?}");

        // Without game data: no definite claims, and a note says so.
        let p = validate(&doc);
        assert!(!has(&p, "isn't in the map or the game's data"), "{p:#?}");
        assert!(has(&p, "Load the game data"), "{p:#?}");
    }

    #[test]
    fn maps_on_other_mods_are_not_audited_against_melee_data() {
        let mut doc = audit_doc();
        doc.info.dependencies.push("bnet:Co-op Mission/0.0/999,file:Mods/StarCoop/StarCoop.SC2Mod".into());
        let p = validate_with(&doc, Some(&game()));
        assert!(!has(&p, "isn't in the map or the game's data"), "{p:#?}");
        assert!(has(&p, "depends on StarCoop.SC2Mod"), "{p:#?}");
        assert_eq!(
            extra_dependencies(&["bnet:Liberty.SC2Mod/0.0/999,file:Mods/Liberty.SC2Mod".into()]),
            Vec::<String>::new()
        );
    }
}
