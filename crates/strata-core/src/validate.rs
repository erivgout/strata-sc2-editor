//! Document checks surfaced in the Problems panel and via MCP.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

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
    let mut out = Vec::new();
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
        .filter(|o| o.type_id == "StartLocation")
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
            ObjectKind::Doodad => {}
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

    check_catalog(doc, &mut out);
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

fn check_catalog(doc: &MapDocument, out: &mut Vec<Problem>) {
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
                if let Some(near) = did_you_mean(p, same_kind.chain(base)) {
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
}
