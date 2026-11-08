//! Read-only questions about a document. The headless MCP server and the
//! live editor answer them with the same code, so results are identical.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::catalog::CatalogKind;
use crate::document::MapDocument;
use crate::history::{History, Origin};
use crate::objects::ObjectKind;
use crate::ops::OpError;
use crate::regions::Shape;
use crate::terrain::pathing;
use crate::validate::Severity;

/// One query. JSON form: `{"query": "list_objects", ...fields}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "query", rename_all = "snake_case")]
pub enum Query {
    /// Map metadata, players, counts, terrain overview, problem counts.
    Summary,
    ListObjects {
        #[serde(default)]
        kind: Option<ObjectKind>,
        /// Exact type id, e.g. "Marine".
        #[serde(default)]
        type_id: Option<String>,
        #[serde(default)]
        player: Option<u8>,
        /// [x0, y0, x1, y1] in world units.
        #[serde(default)]
        rect: Option<[f32; 4]>,
        #[serde(default)]
        ids: Option<Vec<u32>>,
        /// Case-insensitive substring of name or type id.
        #[serde(default)]
        search: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// Regions and cameras.
    ListRegions,
    /// Triggers (outline), global variables and custom script size.
    ListTriggers,
    /// One trigger as model JSON plus the Galaxy it compiles to.
    GetTrigger {
        name: String,
    },
    /// Generated MapScript.galaxy, optionally just one trigger's section.
    GalaxyScript {
        #[serde(default)]
        trigger: Option<String>,
    },
    Validate,
    /// Height / cliff / texture / pathing statistics, whole map or a rect.
    TerrainStats {
        #[serde(default)]
        rect: Option<[f32; 4]>,
    },
    SampleHeight {
        points: Vec<[f32; 2]>,
    },
    ListData {
        #[serde(default)]
        kind: Option<CatalogKind>,
        #[serde(default)]
        class: Option<String>,
        /// Case-insensitive substring of id, class or parent.
        #[serde(default)]
        search: Option<String>,
        /// Include the whole XML tree of each entry.
        #[serde(default)]
        full: bool,
        #[serde(default)]
        limit: Option<usize>,
    },
    GetStrings {
        #[serde(default)]
        locale: Option<String>,
        #[serde(default)]
        prefix: Option<String>,
        /// Case-insensitive substring of key or value.
        #[serde(default)]
        search: Option<String>,
        #[serde(default)]
        limit: Option<usize>,
    },
    History {
        #[serde(default)]
        limit: Option<usize>,
    },
    /// The whole document as JSON (used by the server to save a live map).
    Document,
}

const DEFAULT_LIMIT: usize = 200;
const MAX_LIMIT: usize = 5000;

fn round(v: f32) -> f64 {
    (v as f64 * 1000.0).round() / 1000.0
}

fn rect_contains(r: &[f32; 4], x: f32, y: f32) -> bool {
    x >= r[0].min(r[2]) && x <= r[0].max(r[2]) && y >= r[1].min(r[3]) && y <= r[1].max(r[3])
}

fn shape_json(s: &Shape) -> Value {
    match s {
        Shape::Rect { min, max } => {
            json!({"rect": [round(min.x), round(min.y), round(max.x), round(max.y)]})
        }
        Shape::Circle { center, radius } => {
            json!({"circle": [round(center.x), round(center.y), round(*radius)]})
        }
        Shape::Polygon { points } => {
            json!({"polygon": points.iter().map(|p| [round(p.x), round(p.y)]).collect::<Vec<_>>()})
        }
    }
}

impl Query {
    /// Short machine name, e.g. "list_objects".
    pub fn name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.get("query").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "query".into())
    }

    pub fn run(&self, doc: &MapDocument, history: &History) -> Result<Value, OpError> {
        let limit = |l: &Option<usize>| l.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        Ok(match self {
            Query::Summary => summary(doc, history),
            Query::ListObjects {
                kind,
                type_id,
                player,
                rect,
                ids,
                search,
                limit: l,
            } => {
                let search = search.as_ref().map(|s| s.to_lowercase());
                let matches: Vec<_> = doc
                    .objects
                    .objects
                    .iter()
                    .filter(|o| kind.is_none_or(|k| o.kind == k))
                    .filter(|o| type_id.as_ref().is_none_or(|t| &o.type_id == t))
                    .filter(|o| player.is_none_or(|p| o.player == p && o.kind == ObjectKind::Unit))
                    .filter(|o| {
                        rect.as_ref()
                            .is_none_or(|r| rect_contains(r, o.position.x, o.position.y))
                    })
                    .filter(|o| ids.as_ref().is_none_or(|ids| ids.contains(&o.id.0)))
                    .filter(|o| {
                        search.as_ref().is_none_or(|s| {
                            o.name.to_lowercase().contains(s)
                                || o.type_id.to_lowercase().contains(s)
                        })
                    })
                    .collect();
                let n = limit(l);
                let objects: Vec<Value> = matches
                    .iter()
                    .take(n)
                    .map(|o| {
                        json!({
                            "id": o.id.0,
                            "kind": o.kind,
                            "type_id": o.type_id,
                            "name": o.name,
                            "x": round(o.position.x),
                            "y": round(o.position.y),
                            "z": round(o.position.z),
                            "facing_degrees": round(o.rotation.to_degrees()),
                            "scale": [round(o.scale.x), round(o.scale.y), round(o.scale.z)],
                            "player": o.player,
                        })
                    })
                    .collect();
                json!({"total": matches.len(), "returned": objects.len(), "truncated": matches.len() > n, "objects": objects})
            }
            Query::ListRegions => json!({
                "regions": doc.regions.iter().map(|r| json!({
                    "id": r.id.0,
                    "name": r.name,
                    "color": r.color,
                    "shapes": r.shapes.iter().map(shape_json).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "cameras": doc.cameras.iter().map(|c| json!({
                    "id": c.id.0,
                    "name": c.name,
                    "x": round(c.target.x),
                    "y": round(c.target.y),
                    "distance": c.distance,
                    "pitch": c.pitch,
                    "yaw": c.yaw,
                    "fov": c.fov,
                })).collect::<Vec<_>>(),
            }),
            Query::ListTriggers => list_triggers(doc),
            Query::GetTrigger { name } => {
                let t = doc
                    .triggers
                    .triggers()
                    .into_iter()
                    .find(|t| &t.name == name)
                    .ok_or_else(|| OpError::NotFound(format!("trigger \"{name}\"")))?;
                json!({"trigger": t, "galaxy": trigger_section(doc, name).unwrap_or_default()})
            }
            Query::GalaxyScript { trigger } => match trigger {
                Some(name) => json!({
                    "script": trigger_section(doc, name).ok_or_else(|| OpError::NotFound(format!("trigger \"{name}\"")))?
                }),
                None => json!({"script": crate::galaxy::generate(doc)}),
            },
            Query::Validate => {
                let problems = crate::validate::validate(doc);
                let count = |s: Severity| problems.iter().filter(|p| p.severity == s).count();
                json!({
                    "errors": count(Severity::Error),
                    "warnings": count(Severity::Warning),
                    "infos": count(Severity::Info),
                    "problems": problems,
                })
            }
            Query::TerrainStats { rect } => terrain_stats(doc, rect.as_ref()),
            Query::SampleHeight { points } => {
                let samples: Vec<Value> = points
                    .iter()
                    .map(|[x, y]| {
                        let t = &doc.terrain;
                        let inside =
                            *x >= 0.0 && *y >= 0.0 && *x <= t.width as f32 && *y <= t.height as f32;
                        let vx = (x.round().max(0.0) as u32).min(t.width);
                        let vy = (y.round().max(0.0) as u32).min(t.height);
                        let cliff = t.cliff_levels[t.vertex_index(vx, vy)];
                        let cell = t.cell_index(vx.min(t.width - 1), vy.min(t.height - 1));
                        json!({
                            "x": x, "y": y,
                            "height": round(t.sample_height(*x, *y)),
                            "cliff_level": cliff,
                            "pathing": pathing_names(t.pathing[cell]),
                            "inside_map": inside,
                        })
                    })
                    .collect();
                json!({"samples": samples})
            }
            Query::ListData {
                kind,
                class,
                search,
                full,
                limit: l,
            } => {
                let search = search.as_ref().map(|s| s.to_lowercase());
                let matches: Vec<_> = doc
                    .data
                    .entries
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| kind.is_none_or(|k| CatalogKind::from_file(&e.file) == k))
                    .filter(|(_, e)| class.as_ref().is_none_or(|c| e.class() == c))
                    .filter(|(_, e)| {
                        search.as_ref().is_none_or(|s| {
                            e.id().to_lowercase().contains(s)
                                || e.class().to_lowercase().contains(s)
                                || e.parent().is_some_and(|p| p.to_lowercase().contains(s))
                        })
                    })
                    .collect();
                let n = limit(l);
                let entries: Vec<Value> = matches
                    .iter()
                    .take(n)
                    .map(|(i, e)| {
                        let fields: Map<String, Value> = e
                            .node
                            .children
                            .iter()
                            .filter_map(|c| {
                                c.attr("value")
                                    .map(|v| (c.name.clone(), Value::String(v.to_owned())))
                            })
                            .collect();
                        let mut v = json!({
                            "index": i,
                            "file": e.file,
                            "class": e.class(),
                            "id": e.id(),
                            "parent": e.parent(),
                            "fields": fields,
                        });
                        if *full {
                            v["node"] = serde_json::to_value(&e.node).unwrap_or_default();
                        }
                        v
                    })
                    .collect();
                json!({"total": matches.len(), "returned": entries.len(), "truncated": matches.len() > n, "entries": entries})
            }
            Query::GetStrings {
                locale,
                prefix,
                search,
                limit: l,
            } => {
                let loc = locale
                    .clone()
                    .unwrap_or_else(|| crate::strings::DEFAULT_LOCALE.to_owned());
                let search = search.as_ref().map(|s| s.to_lowercase());
                let empty = Default::default();
                let map = doc.strings.locales.get(&loc).unwrap_or(&empty);
                let matches: Vec<_> = map
                    .iter()
                    .filter(|(k, _)| prefix.as_ref().is_none_or(|p| k.starts_with(p.as_str())))
                    .filter(|(k, v)| {
                        search.as_ref().is_none_or(|s| {
                            k.to_lowercase().contains(s) || v.to_lowercase().contains(s)
                        })
                    })
                    .collect();
                let n = limit(l);
                let strings: Map<String, Value> = matches
                    .iter()
                    .take(n)
                    .map(|(k, v)| ((*k).clone(), Value::String((*v).clone())))
                    .collect();
                json!({
                    "locale": loc,
                    "locales": doc.strings.locales.keys().collect::<Vec<_>>(),
                    "total": matches.len(),
                    "truncated": matches.len() > n,
                    "strings": strings,
                })
            }
            Query::History { limit: l } => {
                let n = l.unwrap_or(50).clamp(1, 500);
                let entry = |e: &crate::history::HistoryEntry| json!({"label": e.label, "by": if e.origin == Origin::Mcp { "ai" } else { "user" }});
                json!({
                    "undo": history.undo_entries().iter().rev().take(n).map(entry).collect::<Vec<_>>(),
                    "redo": history.redo_entries().iter().rev().take(n).map(entry).collect::<Vec<_>>(),
                    "can_undo": history.can_undo(),
                    "can_redo": history.can_redo(),
                    "modified": history.is_modified(),
                })
            }
            Query::Document => {
                serde_json::to_value(doc).map_err(|e| OpError::Invalid(e.to_string()))?
            }
        })
    }
}

fn pathing_names(flags: u8) -> Vec<&'static str> {
    let mut v = Vec::new();
    if flags & pathing::UNWALKABLE != 0 {
        v.push("unwalkable");
    }
    if flags & pathing::UNBUILDABLE != 0 {
        v.push("unbuildable");
    }
    if flags & pathing::NO_FLY != 0 {
        v.push("no_fly");
    }
    v
}

fn summary(doc: &MapDocument, history: &History) -> Value {
    let count = |k: ObjectKind| doc.objects.objects.iter().filter(|o| o.kind == k).count();
    let problems = crate::validate::validate(doc);
    let sev = |s: Severity| problems.iter().filter(|p| p.severity == s).count();
    let (lo, hi) = doc.terrain.min_max_height();
    json!({
        "name": doc.info.name,
        "description": doc.info.description,
        "author": doc.info.author,
        "tileset": doc.info.tileset,
        "size": [doc.info.width, doc.info.height],
        "playable": doc.info.playable,
        "players": doc.info.players.iter().map(|p| json!({
            "id": p.id,
            "name": p.name,
            "race": p.race,
            "control": p.control,
            "team": p.team,
            "color": p.color,
            "start_location": p.start_location.map(|s| [round(s.x), round(s.y)]),
        })).collect::<Vec<_>>(),
        "counts": {
            "units": count(ObjectKind::Unit),
            "doodads": count(ObjectKind::Doodad),
            "points": count(ObjectKind::Point),
            "regions": doc.regions.len(),
            "cameras": doc.cameras.len(),
            "triggers": doc.triggers.triggers().len(),
            "variables": doc.triggers.variables.len(),
            "data_entries": doc.data.entries.len(),
            "strings": doc.strings.all_keys().len(),
        },
        "terrain": {
            "min_height": round(lo),
            "max_height": round(hi),
            "texture_layers": doc.terrain.layers.iter().map(|l| l.texture.as_str()).collect::<Vec<_>>(),
            "water_height": doc.terrain.water_height,
        },
        "dependencies": doc.info.dependencies,
        "problems": {"errors": sev(Severity::Error), "warnings": sev(Severity::Warning), "infos": sev(Severity::Info)},
        "history": {"can_undo": history.can_undo(), "can_redo": history.can_redo(), "modified": history.is_modified()},
        "coordinates": "World units = cells; (0,0) is the south-west corner, +x east, +y north.",
    })
}

fn list_triggers(doc: &MapDocument) -> Value {
    use crate::triggers::TriggerItem;
    fn walk(items: &[TriggerItem], folder: &str, out: &mut Vec<Value>) {
        for i in items {
            match i {
                TriggerItem::Folder { name, children, .. } => {
                    let path = if folder.is_empty() {
                        name.clone()
                    } else {
                        format!("{folder}/{name}")
                    };
                    walk(children, &path, out);
                }
                TriggerItem::Trigger(t) => out.push(json!({
                    "name": t.name,
                    "folder": folder,
                    "enabled": t.enabled,
                    "initially_on": t.initially_on,
                    "events": t.events.iter().map(|e| e.function.as_str()).collect::<Vec<_>>(),
                    "conditions": t.conditions.len(),
                    "actions": t.actions.len(),
                    "comment": t.comment,
                })),
            }
        }
    }
    let mut triggers = Vec::new();
    walk(&doc.triggers.items, "", &mut triggers);
    let lib = &doc.triggers;
    json!({
        "triggers": triggers,
        "variables": lib.variables.iter().map(|v| json!({
            "name": v.name,
            "galaxy_name": crate::galaxy::global_var(&v.name),
            "type": v.ty.galaxy_name(),
            "array_len": v.array_len,
            "initial": v.initial.as_ref().map(|e| crate::galaxy::expr(e, lib)),
            "comment": v.comment,
        })).collect::<Vec<_>>(),
        "custom_script_lines": lib.custom_script.lines().count(),
    })
}

/// The part of the generated script for one trigger (its two functions).
fn trigger_section(doc: &MapDocument, name: &str) -> Option<String> {
    doc.triggers.triggers().iter().find(|t| t.name == name)?;
    let script = crate::galaxy::generate(doc);
    let marker = format!("// Trigger: {name}\n");
    let at = script.find(&marker)?;
    let line_start = |pos: usize| script[..pos].rfind('\n').map_or(0, |p| p + 1);
    // Include the separator line above the marker.
    let start = line_start(at.saturating_sub(1));
    let body = at + marker.len();
    let end = match script[body..].find("\n// Trigger: ") {
        // Stop before the separator line above the next trigger's marker.
        Some(e) => line_start(body + e),
        None => script[body..]
            .find("void InitTriggers")
            .map_or(script.len(), |e| body + e),
    };
    Some(script[start..end].trim_end().to_owned())
}

fn terrain_stats(doc: &MapDocument, rect: Option<&[f32; 4]>) -> Value {
    let t = &doc.terrain;
    let (x0, y0, x1, y1) = match rect {
        Some(r) => (
            r[0].min(r[2]),
            r[1].min(r[3]),
            r[0].max(r[2]),
            r[1].max(r[3]),
        ),
        None => (0.0, 0.0, t.width as f32, t.height as f32),
    };
    let clamp_v = |v: f32, max: u32| (v.max(0.0) as u32).min(max);
    let (vx0, vy0) = (clamp_v(x0.ceil(), t.width), clamp_v(y0.ceil(), t.height));
    let (vx1, vy1) = (clamp_v(x1.floor(), t.width), clamp_v(y1.floor(), t.height));
    let mut n = 0usize;
    let (mut lo, mut hi, mut sum) = (f32::MAX, f32::MIN, 0.0f64);
    let mut cliffs = [0usize; 16];
    let mut coverage = vec![0.0f64; t.layers.len()];
    for y in vy0..=vy1 {
        for x in vx0..=vx1 {
            let i = t.vertex_index(x, y);
            let h = t.heights[i];
            lo = lo.min(h);
            hi = hi.max(h);
            sum += h as f64;
            cliffs[t.cliff_levels[i].min(15) as usize] += 1;
            let w = t.blended_weights(i);
            for (c, w) in coverage.iter_mut().zip(w) {
                *c += w as f64;
            }
            n += 1;
        }
    }
    let (mut unwalkable, mut unbuildable, mut no_fly, mut cells) = (0usize, 0usize, 0usize, 0usize);
    for y in vy0..vy1.max(vy0 + 1).min(t.height) {
        for x in vx0..vx1.max(vx0 + 1).min(t.width) {
            let f = t.pathing[t.cell_index(x, y)];
            cells += 1;
            unwalkable += (f & pathing::UNWALKABLE != 0) as usize;
            unbuildable += (f & pathing::UNBUILDABLE != 0) as usize;
            no_fly += (f & pathing::NO_FLY != 0) as usize;
        }
    }
    let pct = |v: f64, total: usize| {
        if total == 0 {
            0.0
        } else {
            (v / total as f64 * 1000.0).round() / 10.0
        }
    };
    json!({
        "rect": [vx0, vy0, vx1, vy1],
        "vertices": n,
        "height": {
            "min": if n > 0 { round(lo) } else { 0.0 },
            "max": if n > 0 { round(hi) } else { 0.0 },
            "mean": if n > 0 { ((sum / n as f64) * 1000.0).round() / 1000.0 } else { 0.0 },
        },
        "cliff_levels": cliffs.iter().enumerate().filter(|(_, c)| **c > 0)
            .map(|(l, c)| json!({"level": l, "percent": pct(*c as f64, n)})).collect::<Vec<_>>(),
        "textures": t.layers.iter().zip(&coverage).enumerate()
            .map(|(i, (l, c))| json!({"layer": i, "texture": l.texture, "percent": pct(*c, n)})).collect::<Vec<_>>(),
        "pathing": {
            "cells": cells,
            "unwalkable_percent": pct(unwalkable as f64, cells),
            "unbuildable_percent": pct(unbuildable as f64, cells),
            "no_fly_percent": pct(no_fly as f64, cells),
        },
        "water_height": t.water_height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::Op;

    fn setup() -> (MapDocument, History) {
        let mut d = MapDocument::new("Q", 32, 32);
        let mut h = History::default();
        for (x, ty) in [(4.0, "Marine"), (8.0, "Marine"), (20.0, "Zergling")] {
            let a = Op::PlaceObject {
                kind: ObjectKind::Unit,
                type_id: ty.into(),
                x,
                y: 4.0,
                player: Some(1),
                rotation: 0.0,
                name: String::new(),
            }
            .run(&mut d)
            .unwrap();
            h.push(a.label, Origin::Mcp, a.edit);
        }
        (d, h)
    }

    #[test]
    fn query_json_shape() {
        let q: Query =
            serde_json::from_value(json!({"query": "list_objects", "type_id": "Marine"})).unwrap();
        assert_eq!(q.name(), "list_objects");
        let q2: Query = serde_json::from_value(json!({"query": "summary"})).unwrap();
        assert_eq!(q2, Query::Summary);
    }

    #[test]
    fn list_objects_filters() {
        let (d, h) = setup();
        let all = Query::ListObjects {
            kind: None,
            type_id: None,
            player: None,
            rect: None,
            ids: None,
            search: None,
            limit: None,
        }
        .run(&d, &h)
        .unwrap();
        assert_eq!(all["total"], 3);
        let marines = Query::ListObjects {
            kind: None,
            type_id: Some("Marine".into()),
            player: None,
            rect: None,
            ids: None,
            search: None,
            limit: None,
        }
        .run(&d, &h)
        .unwrap();
        assert_eq!(marines["total"], 2);
        let in_rect = Query::ListObjects {
            kind: None,
            type_id: None,
            player: None,
            rect: Some([0.0, 0.0, 10.0, 10.0]),
            ids: None,
            search: None,
            limit: Some(1),
        }
        .run(&d, &h)
        .unwrap();
        assert_eq!(
            (
                in_rect["total"].as_u64(),
                in_rect["returned"].as_u64(),
                in_rect["truncated"].as_bool()
            ),
            (Some(2), Some(1), Some(true))
        );
        let z = Query::ListObjects {
            kind: None,
            type_id: None,
            player: None,
            rect: None,
            ids: None,
            search: Some("zerg".into()),
            limit: None,
        }
        .run(&d, &h)
        .unwrap();
        assert_eq!(z["objects"][0]["type_id"], "Zergling");
    }

    #[test]
    fn summary_history_validate_terrain() {
        let (d, h) = setup();
        let s = Query::Summary.run(&d, &h).unwrap();
        assert_eq!(s["counts"]["units"], 3);
        assert_eq!(s["size"], json!([32, 32]));
        let hist = Query::History { limit: None }.run(&d, &h).unwrap();
        assert_eq!(hist["undo"][0]["label"], "Place Zergling");
        assert_eq!(hist["undo"][0]["by"], "ai");
        let v = Query::Validate.run(&d, &h).unwrap();
        assert_eq!(v["errors"], 0);
        let ts = Query::TerrainStats { rect: None }.run(&d, &h).unwrap();
        assert_eq!(ts["vertices"], 33 * 33);
        assert_eq!(ts["textures"][0]["percent"], 100.0);
        let sh = Query::SampleHeight {
            points: vec![[16.0, 16.0], [100.0, 0.0]],
        }
        .run(&d, &h)
        .unwrap();
        assert_eq!(sh["samples"][0]["height"], 12.0);
        assert_eq!(sh["samples"][1]["inside_map"], false);
    }

    #[test]
    fn triggers_and_script() {
        let (d, h) = setup();
        let lt = Query::ListTriggers.run(&d, &h).unwrap();
        assert_eq!(lt["triggers"][0]["name"], "Melee Initialization");
        let gt = Query::GetTrigger {
            name: "Melee Initialization".into(),
        }
        .run(&d, &h)
        .unwrap();
        let g = gt["galaxy"].as_str().unwrap();
        assert!(
            g.contains("bool gt_MeleeInitialization_Func")
                && g.contains("void gt_MeleeInitialization_Init"),
            "{g}"
        );
        assert!(!g.contains("InitTriggers"));
        assert!(Query::GetTrigger {
            name: "Nope".into()
        }
        .run(&d, &h)
        .is_err());
        let full = Query::GalaxyScript { trigger: None }.run(&d, &h).unwrap();
        assert!(full["script"].as_str().unwrap().contains("void InitMap ()"));
    }

    #[test]
    fn data_and_strings() {
        let (mut d, h) = setup();
        Op::SetDataField {
            file: String::new(),
            class: "CUnit".into(),
            id: "Marine".into(),
            field: "LifeMax".into(),
            value: "55".into(),
            parent: None,
        }
        .run(&mut d)
        .unwrap();
        let ld = Query::ListData {
            kind: Some(CatalogKind::Unit),
            class: None,
            search: Some("mar".into()),
            full: true,
            limit: None,
        }
        .run(&d, &h)
        .unwrap();
        assert_eq!(ld["entries"][0]["fields"]["LifeMax"], "55");
        assert!(ld["entries"][0]["node"].is_object());
        let gs = Query::GetStrings {
            locale: None,
            prefix: Some("DocInfo/".into()),
            search: None,
            limit: None,
        }
        .run(&d, &h)
        .unwrap();
        assert_eq!(gs["strings"]["DocInfo/Name"], "Q");
        let doc = Query::Document.run(&d, &h).unwrap();
        let back: MapDocument = serde_json::from_value(doc).unwrap();
        assert_eq!(back.objects.objects.len(), 3);
    }
}
