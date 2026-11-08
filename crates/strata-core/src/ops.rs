//! Serializable document operations: the command API shared by the command
//! palette, the live MCP link and the headless MCP server.
//!
//! [`Op::run`] validates and applies an op, returning an [`Applied`] whose
//! `edit` the caller pushes onto its [`crate::history::History`] (already applied).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::brush::{Brush, BrushShape, BrushTool, Falloff, Stroke};
use crate::catalog::{CatalogEntry, XmlNode};
use crate::document::MapDocument;
use crate::history::Edit;
use crate::objects::{ObjectId, ObjectKind, PlacedObject};
use crate::regions::{Region, Shape};
use crate::triggers::{Trigger, TriggerItem};

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("{0}")]
    Invalid(String),
    #[error("nothing named {0} exists")]
    NotFound(String),
}

/// One operation. JSON form: `{"op": "place_object", ...fields}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    /// Brush dabs along a path of world-space points.
    Brush {
        tool: BrushTool,
        points: Vec<[f32; 2]>,
        #[serde(default = "default_radius")]
        radius: f32,
        #[serde(default = "default_strength")]
        strength: f32,
        #[serde(default)]
        shape: BrushShape,
        #[serde(default)]
        falloff: Falloff,
    },
    PlaceObject {
        kind: ObjectKind,
        type_id: String,
        x: f32,
        y: f32,
        #[serde(default)]
        player: Option<u8>,
        #[serde(default)]
        rotation: f32,
        #[serde(default)]
        name: String,
    },
    MoveObject { id: u32, x: f32, y: f32 },
    DeleteObjects { ids: Vec<u32> },
    CreateRegion { name: String, shape: Shape, #[serde(default)] color: Option<[u8; 3]> },
    DeleteRegion { name: String },
    /// Add a trigger with events/conditions/actions given in model JSON.
    CreateTrigger { trigger: Trigger },
    DeleteTrigger { name: String },
    SetString { key: String, value: String, #[serde(default = "default_locale")] locale: String },
    /// Set a scalar catalog field, creating the entry if needed.
    SetDataField { file: String, class: String, id: String, field: String, value: String, #[serde(default)] parent: Option<String> },
    SetMapInfo {
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        author: Option<String>,
        #[serde(default)]
        playable: Option<[u32; 4]>,
    },
}

fn default_radius() -> f32 {
    6.0
}
fn default_strength() -> f32 {
    0.35
}
fn default_locale() -> String {
    crate::strings::DEFAULT_LOCALE.to_owned()
}

/// Result of a successful op. `edit` is already applied to the document.
#[derive(Debug, Clone)]
pub struct Applied {
    pub label: String,
    pub edit: Edit,
    /// Small JSON payload for the caller, e.g. `{"id": 12}`.
    pub result: Value,
}

impl Op {
    pub fn run(self, doc: &mut MapDocument) -> Result<Applied, OpError> {
        let ok = |label: String, edit: Edit, result: Value| Ok(Applied { label, edit, result });
        match self {
            Op::Brush { tool, points, radius, strength, shape, falloff } => {
                if points.is_empty() {
                    return Err(OpError::Invalid("brush needs at least one point".into()));
                }
                if !(0.5..=64.0).contains(&radius) {
                    return Err(OpError::Invalid("radius must be between 0.5 and 64".into()));
                }
                let brush = Brush { tool, shape, falloff, radius, strength: strength.clamp(0.0, 1.0) };
                let mut stroke = Stroke::begin(&doc.terrain, brush);
                for p in &points {
                    stroke.dab(&mut doc.terrain, glam::Vec2::from(*p));
                }
                let r = stroke.rect();
                match stroke.finish(&doc.terrain) {
                    Some(edit) => {
                        doc.dirty.terrain = true;
                        ok(tool.label().into(), edit, json!({"rect": [r.x0, r.y0, r.x1, r.y1]}))
                    }
                    None => Err(OpError::Invalid("brush changed nothing (outside map or zero strength)".into())),
                }
            }
            Op::PlaceObject { kind, type_id, x, y, player, rotation, name } => {
                if kind != ObjectKind::Point && type_id.trim().is_empty() {
                    return Err(OpError::Invalid("type_id is required for units and doodads".into()));
                }
                if x < 0.0 || y < 0.0 || x > doc.terrain.width as f32 || y > doc.terrain.height as f32 {
                    return Err(OpError::Invalid(format!("({x}, {y}) is outside the {}x{} map", doc.terrain.width, doc.terrain.height)));
                }
                let id = doc.objects.alloc_id();
                let z = doc.terrain.sample_height(x, y);
                let mut o = PlacedObject::new(id, kind, type_id, glam::Vec3::new(x, y, z));
                o.rotation = rotation;
                o.name = name;
                if let Some(p) = player {
                    o.player = p;
                }
                let label = format!("Place {}", o.display_name());
                let edit = Edit::Object { id, before: None, after: Some(o) };
                edit.apply(doc, true);
                ok(label, edit, json!({"id": id.0}))
            }
            Op::MoveObject { id, x, y } => {
                let before = doc.objects.get(ObjectId(id)).cloned().ok_or_else(|| OpError::NotFound(format!("object {id}")))?;
                let mut after = before.clone();
                after.position = glam::Vec3::new(x, y, doc.terrain.sample_height(x, y));
                let edit = Edit::Object { id: ObjectId(id), before: Some(before), after: Some(after) };
                edit.apply(doc, true);
                ok("Move object".into(), edit, json!({"id": id}))
            }
            Op::DeleteObjects { ids } => {
                let mut edits = Vec::new();
                for id in &ids {
                    let o = doc.objects.get(ObjectId(*id)).cloned().ok_or_else(|| OpError::NotFound(format!("object {id}")))?;
                    edits.push(Edit::Object { id: o.id, before: Some(o), after: None });
                }
                let edit = Edit::Batch(edits);
                edit.apply(doc, true);
                ok(format!("Delete {} objects", ids.len()), edit, json!({"deleted": ids.len()}))
            }
            Op::CreateRegion { name, shape, color } => {
                if doc.regions.iter().any(|r| r.name == name) {
                    return Err(OpError::Invalid(format!("a region named \"{name}\" already exists")));
                }
                let before = doc.regions.clone();
                let id = doc.objects.alloc_id();
                doc.regions.push(Region { id, name: name.clone(), color: color.unwrap_or([98, 205, 235]), shapes: vec![shape] });
                let edit = Edit::Regions { before, after: doc.regions.clone() };
                doc.dirty.regions = true;
                ok(format!("Create region {name}"), edit, json!({"id": id.0}))
            }
            Op::DeleteRegion { name } => {
                let before = doc.regions.clone();
                let i = doc.regions.iter().position(|r| r.name == name).ok_or_else(|| OpError::NotFound(format!("region \"{name}\"")))?;
                doc.regions.remove(i);
                doc.dirty.regions = true;
                ok(format!("Delete region {name}"), Edit::Regions { before, after: doc.regions.clone() }, json!({}))
            }
            Op::CreateTrigger { mut trigger } => {
                if doc.triggers.triggers().iter().any(|t| t.name == trigger.name) {
                    return Err(OpError::Invalid(format!("a trigger named \"{}\" already exists", trigger.name)));
                }
                let before = Box::new(doc.triggers.clone());
                trigger.id = doc.triggers.alloc_id();
                let label = format!("Create trigger {}", trigger.name);
                doc.triggers.items.push(TriggerItem::Trigger(trigger));
                doc.dirty.triggers = true;
                ok(label, Edit::Triggers { before, after: Box::new(doc.triggers.clone()) }, json!({}))
            }
            Op::DeleteTrigger { name } => {
                let before = Box::new(doc.triggers.clone());
                let id = doc.triggers.triggers().iter().find(|t| t.name == name).map(|t| t.id).ok_or_else(|| OpError::NotFound(format!("trigger \"{name}\"")))?;
                doc.triggers.remove(id);
                doc.dirty.triggers = true;
                ok(format!("Delete trigger {name}"), Edit::Triggers { before, after: Box::new(doc.triggers.clone()) }, json!({}))
            }
            Op::SetString { key, value, locale } => {
                let before = Box::new(doc.strings.clone());
                doc.strings.set(&locale, &key, &value);
                doc.dirty.strings = true;
                ok(format!("Set string {key}"), Edit::Strings { before, after: Box::new(doc.strings.clone()) }, json!({}))
            }
            Op::SetDataField { file, class, id, field, value, parent } => {
                let edit = match doc.data.entries.iter().position(|e| e.class() == class && e.id() == id) {
                    Some(index) => {
                        let before = doc.data.entries[index].clone();
                        let mut after = before.clone();
                        after.set_field(&field, value);
                        Edit::Data { index, before: Some(before), after: Some(after) }
                    }
                    None => {
                        let mut node = XmlNode::new(&class);
                        node.set_attr("id", &id);
                        if let Some(p) = parent {
                            node.set_attr("parent", p);
                        }
                        let mut e = CatalogEntry { file, node };
                        e.set_field(&field, value);
                        Edit::Data { index: doc.data.entries.len(), before: None, after: Some(e) }
                    }
                };
                edit.apply(doc, true);
                ok(format!("Set {class} {id}.{field}"), edit, json!({}))
            }
            Op::SetMapInfo { name, description, author, playable } => {
                let before = Box::new(doc.info.clone());
                if let Some(n) = name {
                    doc.info.name = n;
                }
                if let Some(d) = description {
                    doc.info.description = d;
                }
                if let Some(a) = author {
                    doc.info.author = a;
                }
                if let Some(p) = playable {
                    if p[0] >= p[2] || p[1] >= p[3] || p[2] > doc.info.width || p[3] > doc.info.height {
                        doc.info = *before;
                        return Err(OpError::Invalid("playable bounds must be [left, bottom, right, top] inside the map".into()));
                    }
                    doc.info.playable = p;
                }
                doc.dirty.info = true;
                ok("Edit map properties".into(), Edit::Info { before, after: Box::new(doc.info.clone()) }, json!({}))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_json_roundtrip_and_run() {
        let mut doc = MapDocument::new("t", 64, 64);
        let op: Op = serde_json::from_value(json!({
            "op": "place_object", "kind": "Unit", "type_id": "Marine", "x": 10.0, "y": 12.0
        }))
        .unwrap();
        let a = op.run(&mut doc).unwrap();
        assert_eq!(doc.objects.objects.len(), 1);
        a.edit.apply(&mut doc, false);
        assert!(doc.objects.objects.is_empty());

        let op: Op = serde_json::from_value(json!({
            "op": "brush", "tool": "Raise", "points": [[32.0, 32.0]]
        }))
        .unwrap();
        op.run(&mut doc).unwrap();
    }
}
