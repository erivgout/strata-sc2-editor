//! Serializable document operations: the command API shared by the command
//! palette, the live MCP link and the headless MCP server.
//!
//! [`Op::run`] validates and applies an op, returning an [`Applied`] whose
//! `edit` the caller pushes onto its [`crate::history::History`] (already applied).
//! Every op validates its input first and leaves the document untouched on
//! error; error messages say what is wrong and what the valid range is, since
//! they are read by people and by AI clients alike.
//!
//! Submodules:
//! - [`query`]: read-only questions about a document ([`query::Query`]).
//! - [`link`]: the wire protocol between `strata-mcp` and a running editor.

pub mod link;
pub mod query;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::brush::{Brush, BrushShape, BrushTool, Falloff, Stroke};
use crate::catalog::{CatalogEntry, XmlNode};
use crate::document::{MapDocument, Player, PlayerControl, Race};
use crate::galaxy::ident;
use crate::history::Edit;
use crate::objects::{ObjectId, ObjectKind, PlacedObject};
use crate::regions::{Camera, Region, Shape};
use crate::strings::LOCALES;
use crate::terrain::{TextureLayer, MAX_TEXTURE_LAYERS};
use crate::triggers::{Expr, GalaxyType, Trigger, TriggerItem, Variable};

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("{0}")]
    Invalid(String),
    #[error("nothing named {0} exists")]
    NotFound(String),
}

/// Highest player slot id SC2 supports (0 is neutral).
pub const MAX_PLAYER: u8 = 15;
/// Height range the terrain brushes clamp to; also the valid water range.
pub const HEIGHT_RANGE: (f32, f32) = (-16.0, 64.0);

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
        /// Facing in radians, counter-clockwise from +X.
        #[serde(default)]
        rotation: f32,
        #[serde(default)]
        name: String,
    },
    MoveObject {
        id: u32,
        x: f32,
        y: f32,
    },
    /// Change some properties of a placed object; omitted fields are kept.
    SetObjectProperty {
        id: u32,
        #[serde(default)]
        player: Option<u8>,
        /// Radians, counter-clockwise from +X.
        #[serde(default)]
        rotation: Option<f32>,
        #[serde(default)]
        scale: Option<[f32; 3]>,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        type_id: Option<String>,
    },
    DeleteObjects {
        ids: Vec<u32>,
    },
    CreateRegion {
        name: String,
        shape: Shape,
        #[serde(default)]
        color: Option<[u8; 3]>,
    },
    DeleteRegion {
        name: String,
    },
    CreateCamera {
        name: String,
        x: f32,
        y: f32,
        #[serde(default)]
        distance: Option<f32>,
        #[serde(default)]
        pitch: Option<f32>,
        #[serde(default)]
        yaw: Option<f32>,
        #[serde(default)]
        fov: Option<f32>,
    },
    DeleteCamera {
        name: String,
    },
    /// Add a trigger with events/conditions/actions given in model JSON.
    CreateTrigger {
        trigger: Trigger,
    },
    /// Replace the trigger called `name` (keeping its id and folder). The
    /// new trigger may carry a different name, which renames it.
    UpdateTrigger {
        name: String,
        trigger: Trigger,
    },
    DeleteTrigger {
        name: String,
    },
    /// Add a global variable to the trigger library.
    CreateVariable {
        name: String,
        ty: GalaxyType,
        #[serde(default)]
        array_len: Option<u32>,
        #[serde(default)]
        initial: Option<Expr>,
        #[serde(default)]
        comment: String,
    },
    DeleteVariable {
        name: String,
    },
    /// Replace (or append to) the free-form custom Galaxy script.
    SetCustomScript {
        script: String,
        #[serde(default)]
        append: bool,
    },
    SetString {
        key: String,
        value: String,
        #[serde(default = "default_locale")]
        locale: String,
    },
    /// Set a scalar catalog field, creating the entry if needed. `file`
    /// may be left empty for well-known classes (see [`data_file_for_class`]).
    SetDataField {
        #[serde(default)]
        file: String,
        class: String,
        id: String,
        field: String,
        value: String,
        #[serde(default)]
        parent: Option<String>,
    },
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
    /// Create or update player slot `id` (1..=15); omitted fields are kept.
    SetPlayer {
        id: u8,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        race: Option<Race>,
        #[serde(default)]
        control: Option<PlayerControl>,
        #[serde(default)]
        color: Option<[u8; 3]>,
        #[serde(default)]
        team: Option<u8>,
        #[serde(default)]
        start_location: Option<[f32; 2]>,
        /// Remove the start location.
        #[serde(default)]
        clear_start_location: bool,
    },
    /// Append a texture layer; paint it with a `Paint { layer }` brush.
    AddTextureLayer {
        texture: String,
        /// Cover the whole map with the new layer.
        #[serde(default)]
        fill: bool,
    },
    /// Set the water plane height, or remove water with `None`.
    SetWater {
        #[serde(default)]
        height: Option<f32>,
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

/// GameData file that holds entries of a catalog class, e.g. "CUnit" -> "UnitData".
pub fn data_file_for_class(class: &str) -> Option<&'static str> {
    const PREFIXES: [(&str, &str); 10] = [
        ("CUnit", "UnitData"),
        ("CAbil", "AbilData"),
        ("CWeapon", "WeaponData"),
        ("CEffect", "EffectData"),
        ("CBehavior", "BehaviorData"),
        ("CUpgrade", "UpgradeData"),
        ("CActor", "ActorData"),
        ("CModel", "ModelData"),
        ("CButton", "ButtonData"),
        ("CRequirement", "RequirementData"),
    ];
    PREFIXES
        .iter()
        .find(|(p, _)| class.starts_with(p))
        .map(|(_, f)| *f)
}

fn invalid<T>(msg: impl Into<String>) -> Result<T, OpError> {
    Err(OpError::Invalid(msg.into()))
}

fn check_finite(what: &str, v: f32) -> Result<(), OpError> {
    if v.is_finite() {
        Ok(())
    } else {
        invalid(format!("{what} must be a finite number, got {v}"))
    }
}

fn check_in_map(doc: &MapDocument, x: f32, y: f32) -> Result<(), OpError> {
    check_finite("x", x)?;
    check_finite("y", y)?;
    let (w, h) = (doc.terrain.width as f32, doc.terrain.height as f32);
    if x < 0.0 || y < 0.0 || x > w || y > h {
        return invalid(format!(
            "({x}, {y}) is outside the {w}x{h} map; x must be 0..={w} and y 0..={h}"
        ));
    }
    Ok(())
}

fn check_player(p: u8) -> Result<(), OpError> {
    if p > MAX_PLAYER {
        return invalid(format!(
            "player {p} does not exist; use 0 (neutral) to {MAX_PLAYER}"
        ));
    }
    Ok(())
}

fn check_name(what: &str, name: &str) -> Result<(), OpError> {
    if name.trim().is_empty() {
        return invalid(format!("{what} name must not be empty"));
    }
    Ok(())
}

fn check_shape(shape: &Shape) -> Result<(), OpError> {
    match shape {
        Shape::Rect { min, max } => {
            if !min.is_finite() || !max.is_finite() || min.x >= max.x || min.y >= max.y {
                return invalid(
                    "rect needs min < max on both axes, e.g. min [10, 10] and max [20, 30]",
                );
            }
        }
        Shape::Circle { center, radius } => {
            if !center.is_finite() || !radius.is_finite() || *radius <= 0.0 {
                return invalid(format!(
                    "circle radius must be greater than 0, got {radius}"
                ));
            }
        }
        Shape::Polygon { points } => {
            if points.len() < 3 || points.iter().any(|p| !p.is_finite()) {
                return invalid(format!(
                    "polygon needs at least 3 finite points, got {}",
                    points.len()
                ));
            }
        }
    }
    Ok(())
}

fn check_trigger(doc: &MapDocument, t: &Trigger, replacing: Option<&str>) -> Result<(), OpError> {
    check_name("trigger", &t.name)?;
    let id = ident(&t.name);
    if let Some(other) = doc
        .triggers
        .triggers()
        .iter()
        .find(|o| Some(o.name.as_str()) != replacing && ident(&o.name) == id)
    {
        return invalid(format!(
            "trigger \"{}\" would compile to gt_{id}, which trigger \"{}\" already uses; pick another name",
            t.name, other.name
        ));
    }
    for e in &t.events {
        if e.function.trim().is_empty() {
            return invalid("every event needs a function name, e.g. TriggerAddEventMapInit");
        }
    }
    Ok(())
}

impl Op {
    /// Short machine name of the op, e.g. "place_object".
    pub fn name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.get("op").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "op".into())
    }

    pub fn run(self, doc: &mut MapDocument) -> Result<Applied, OpError> {
        let ok = |label: String, edit: Edit, result: Value| {
            Ok(Applied {
                label,
                edit,
                result,
            })
        };
        match self {
            Op::Brush {
                tool,
                points,
                radius,
                strength,
                shape,
                falloff,
            } => {
                if let BrushTool::Pathing { flags, .. } = tool {
                    if flags & !crate::terrain::pathing::UNWALKABLE != 0 {
                        return invalid("only the unwalkable pathing flag can be saved to .SC2Map files today");
                    }
                }
                if points.is_empty() {
                    return invalid("brush needs at least one point, e.g. points [[32, 32]]");
                }
                if !(0.5..=64.0).contains(&radius) {
                    return invalid(format!("radius must be between 0.5 and 64, got {radius}"));
                }
                if !strength.is_finite() {
                    return invalid("strength must be a number between 0 and 1");
                }
                if points
                    .iter()
                    .any(|p| !p[0].is_finite() || !p[1].is_finite())
                {
                    return invalid("every brush point must be two finite numbers [x, y]");
                }
                match tool {
                    BrushTool::Paint { layer } | BrushTool::Erase { layer }
                        if layer >= doc.terrain.layers.len() =>
                    {
                        return invalid(format!(
                            "texture layer {layer} does not exist; this map has layers 0..={} ({}). Add one with add_texture_layer",
                            doc.terrain.layers.len().saturating_sub(1),
                            doc.terrain.layers.iter().map(|l| l.texture.as_str()).collect::<Vec<_>>().join(", ")
                        ));
                    }
                    BrushTool::Cliff { level } if level > 15 => {
                        return invalid(format!("cliff level must be 0..=15, got {level}"));
                    }
                    BrushTool::Flatten { target }
                        if !(HEIGHT_RANGE.0..=HEIGHT_RANGE.1).contains(&target) =>
                    {
                        return invalid(format!(
                            "flatten target must be {}..={}, got {target}",
                            HEIGHT_RANGE.0, HEIGHT_RANGE.1
                        ));
                    }
                    _ => {}
                }
                let brush = Brush {
                    tool,
                    shape,
                    falloff,
                    radius,
                    strength: strength.clamp(0.0, 1.0),
                };
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
                    None => invalid(format!(
                        "brush changed nothing: points must be inside the {}x{} map and strength above 0 (or the terrain already matches)",
                        doc.terrain.width, doc.terrain.height
                    )),
                }
            }
            Op::PlaceObject {
                kind,
                type_id,
                x,
                y,
                player,
                rotation,
                name,
            } => {
                if kind != ObjectKind::Point && type_id.trim().is_empty() {
                    return invalid("type_id is required for units and doodads, e.g. \"Marine\" or \"CharRock01\"");
                }
                check_in_map(doc, x, y)?;
                check_finite("rotation", rotation)?;
                if let Some(p) = player {
                    check_player(p)?;
                }
                let id = doc.objects.alloc_id();
                let z = doc.terrain.sample_height(x, y);
                let mut o = PlacedObject::new(id, kind, type_id, glam::Vec3::new(x, y, z));
                o.rotation = rotation;
                o.name = if kind == ObjectKind::Point && name.trim().is_empty() {
                    format!("Point {}", id.0)
                } else {
                    name
                };
                if let Some(p) = player {
                    o.player = p;
                }
                let label = format!("Place {}", o.display_name());
                let edit = Edit::Object {
                    id,
                    before: None,
                    after: Some(o),
                };
                edit.apply(doc, true);
                ok(label, edit, json!({"id": id.0}))
            }
            Op::MoveObject { id, x, y } => {
                let before = doc
                    .objects
                    .get(ObjectId(id))
                    .cloned()
                    .ok_or_else(|| OpError::NotFound(format!("object {id}")))?;
                check_in_map(doc, x, y)?;
                let mut after = before.clone();
                after.position = glam::Vec3::new(x, y, doc.terrain.sample_height(x, y));
                let label = format!("Move {}", before.display_name());
                let edit = Edit::Object {
                    id: ObjectId(id),
                    before: Some(before),
                    after: Some(after),
                };
                edit.apply(doc, true);
                ok(label, edit, json!({"id": id}))
            }
            Op::SetObjectProperty {
                id,
                player,
                rotation,
                scale,
                name,
                type_id,
            } => {
                let before = doc
                    .objects
                    .get(ObjectId(id))
                    .cloned()
                    .ok_or_else(|| OpError::NotFound(format!("object {id}")))?;
                if player.is_none()
                    && rotation.is_none()
                    && scale.is_none()
                    && name.is_none()
                    && type_id.is_none()
                {
                    return invalid("nothing to change: give at least one of player, rotation, scale, name, type_id");
                }
                let mut after = before.clone();
                if let Some(p) = player {
                    check_player(p)?;
                    after.player = p;
                }
                if let Some(r) = rotation {
                    check_finite("rotation", r)?;
                    after.rotation = r;
                }
                if let Some(s) = scale {
                    if s.iter()
                        .any(|v| !v.is_finite() || !(0.01..=100.0).contains(v))
                    {
                        return invalid(format!("scale components must be 0.01..=100, got {s:?}"));
                    }
                    after.scale = glam::Vec3::from(s);
                }
                if let Some(n) = name {
                    after.name = n;
                }
                if let Some(t) = type_id {
                    if after.kind != ObjectKind::Point && t.trim().is_empty() {
                        return invalid("type_id must not be empty for units and doodads");
                    }
                    after.type_id = t;
                }
                let label = format!("Edit {}", after.display_name());
                let edit = Edit::Object {
                    id: ObjectId(id),
                    before: Some(before),
                    after: Some(after),
                };
                edit.apply(doc, true);
                ok(label, edit, json!({"id": id}))
            }
            Op::DeleteObjects { ids } => {
                if ids.is_empty() {
                    return invalid(
                        "ids is empty; list the object ids to delete (see list_objects)",
                    );
                }
                let mut edits = Vec::new();
                let mut seen = std::collections::BTreeSet::new();
                for id in &ids {
                    if !seen.insert(*id) {
                        continue;
                    }
                    let o = doc
                        .objects
                        .get(ObjectId(*id))
                        .cloned()
                        .ok_or_else(|| OpError::NotFound(format!("object {id}")))?;
                    edits.push(Edit::Object {
                        id: o.id,
                        before: Some(o),
                        after: None,
                    });
                }
                let n = edits.len();
                let edit = Edit::Batch(edits);
                edit.apply(doc, true);
                let label = if n == 1 {
                    "Delete 1 object".to_owned()
                } else {
                    format!("Delete {n} objects")
                };
                ok(label, edit, json!({"deleted": n}))
            }
            Op::CreateRegion { name, shape, color } => {
                check_name("region", &name)?;
                check_shape(&shape)?;
                if doc.regions.iter().any(|r| r.name == name) {
                    return invalid(format!("a region named \"{name}\" already exists; pick another name or delete it first"));
                }
                let before = doc.regions.clone();
                let id = doc.objects.alloc_id();
                doc.regions.push(Region {
                    id,
                    name: name.clone(),
                    color: color.unwrap_or([98, 205, 235]),
                    shapes: vec![shape],
                });
                let edit = Edit::Regions {
                    before,
                    after: doc.regions.clone(),
                };
                doc.dirty.regions = true;
                ok(format!("Create region {name}"), edit, json!({"id": id.0}))
            }
            Op::DeleteRegion { name } => {
                let i = doc
                    .regions
                    .iter()
                    .position(|r| r.name == name)
                    .ok_or_else(|| OpError::NotFound(format!("region \"{name}\"")))?;
                let before = doc.regions.clone();
                doc.regions.remove(i);
                doc.dirty.regions = true;
                ok(
                    format!("Delete region {name}"),
                    Edit::Regions {
                        before,
                        after: doc.regions.clone(),
                    },
                    json!({}),
                )
            }
            Op::CreateCamera {
                name,
                x,
                y,
                distance,
                pitch,
                yaw,
                fov,
            } => {
                check_name("camera", &name)?;
                check_in_map(doc, x, y)?;
                if doc.cameras.iter().any(|c| c.name == name) {
                    return invalid(format!(
                        "a camera named \"{name}\" already exists; pick another name"
                    ));
                }
                let id = doc.objects.alloc_id();
                let mut cam = Camera::new(id, name.clone(), glam::Vec2::new(x, y));
                if let Some(d) = distance {
                    if !(1.0..=500.0).contains(&d) {
                        return invalid(format!("distance must be 1..=500, got {d}"));
                    }
                    cam.distance = d;
                }
                if let Some(p) = pitch {
                    if !(-90.0..=90.0).contains(&p) {
                        return invalid(format!("pitch must be -90..=90 degrees, got {p}"));
                    }
                    cam.pitch = p;
                }
                if let Some(y) = yaw {
                    check_finite("yaw", y)?;
                    cam.yaw = y.rem_euclid(360.0);
                }
                if let Some(f) = fov {
                    if !(1.0..=170.0).contains(&f) {
                        return invalid(format!("fov must be 1..=170 degrees, got {f}"));
                    }
                    cam.fov = f;
                }
                let before = doc.cameras.clone();
                doc.cameras.push(cam);
                doc.dirty.cameras = true;
                ok(
                    format!("Create camera {name}"),
                    Edit::Cameras {
                        before,
                        after: doc.cameras.clone(),
                    },
                    json!({"id": id.0}),
                )
            }
            Op::DeleteCamera { name } => {
                let i = doc
                    .cameras
                    .iter()
                    .position(|c| c.name == name)
                    .ok_or_else(|| OpError::NotFound(format!("camera \"{name}\"")))?;
                let before = doc.cameras.clone();
                doc.cameras.remove(i);
                doc.dirty.cameras = true;
                ok(
                    format!("Delete camera {name}"),
                    Edit::Cameras {
                        before,
                        after: doc.cameras.clone(),
                    },
                    json!({}),
                )
            }
            Op::CreateTrigger { mut trigger } => {
                if doc
                    .triggers
                    .triggers()
                    .iter()
                    .any(|t| t.name == trigger.name)
                {
                    return invalid(format!(
                        "a trigger named \"{}\" already exists; use update_trigger to replace it",
                        trigger.name
                    ));
                }
                check_trigger(doc, &trigger, None)?;
                let before = Box::new(doc.triggers.clone());
                trigger.id = doc.triggers.alloc_id();
                let id = trigger.id.0;
                let label = format!("Create trigger {}", trigger.name);
                doc.triggers.items.push(TriggerItem::Trigger(trigger));
                doc.dirty.triggers = true;
                ok(
                    label,
                    Edit::Triggers {
                        before,
                        after: Box::new(doc.triggers.clone()),
                    },
                    json!({"id": id}),
                )
            }
            Op::UpdateTrigger { name, mut trigger } => {
                let id = doc
                    .triggers
                    .triggers()
                    .iter()
                    .find(|t| t.name == name)
                    .map(|t| t.id)
                    .ok_or_else(|| OpError::NotFound(format!("trigger \"{name}\"")))?;
                if trigger.name.trim().is_empty() {
                    trigger.name = name.clone();
                }
                if trigger.name != name
                    && doc
                        .triggers
                        .triggers()
                        .iter()
                        .any(|t| t.name == trigger.name)
                {
                    return invalid(format!(
                        "cannot rename to \"{}\": another trigger already has that name",
                        trigger.name
                    ));
                }
                check_trigger(doc, &trigger, Some(&name))?;
                let before = Box::new(doc.triggers.clone());
                trigger.id = id;
                let label = format!("Edit trigger {}", trigger.name);
                if let Some(slot) = doc.triggers.trigger_mut(id) {
                    *slot = trigger;
                }
                doc.dirty.triggers = true;
                ok(
                    label,
                    Edit::Triggers {
                        before,
                        after: Box::new(doc.triggers.clone()),
                    },
                    json!({"id": id.0}),
                )
            }
            Op::DeleteTrigger { name } => {
                let id = doc
                    .triggers
                    .triggers()
                    .iter()
                    .find(|t| t.name == name)
                    .map(|t| t.id)
                    .ok_or_else(|| OpError::NotFound(format!("trigger \"{name}\"")))?;
                let before = Box::new(doc.triggers.clone());
                doc.triggers.remove(id);
                doc.dirty.triggers = true;
                ok(
                    format!("Delete trigger {name}"),
                    Edit::Triggers {
                        before,
                        after: Box::new(doc.triggers.clone()),
                    },
                    json!({}),
                )
            }
            Op::CreateVariable {
                name,
                ty,
                array_len,
                initial,
                comment,
            } => {
                check_name("variable", &name)?;
                let id = ident(&name);
                if let Some(v) = doc.triggers.variables.iter().find(|v| ident(&v.name) == id) {
                    return invalid(format!(
                        "variable \"{}\" already compiles to gv_{id}; pick another name",
                        v.name
                    ));
                }
                if let Some(n) = array_len {
                    if !(1..=65_536).contains(&n) {
                        return invalid(format!(
                            "array_len must be 1..=65536, got {n}; omit it for a plain variable"
                        ));
                    }
                }
                if let GalaxyType::Other(t) = &ty {
                    if t.trim().is_empty()
                        || !t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        return invalid(format!(
                            "\"{t}\" is not a Galaxy type name, e.g. int, fixed, bool, unit, actor"
                        ));
                    }
                }
                let before = Box::new(doc.triggers.clone());
                doc.triggers.variables.push(Variable {
                    name: name.clone(),
                    ty,
                    array_len,
                    initial,
                    comment,
                });
                doc.dirty.triggers = true;
                ok(
                    format!("Create variable {name}"),
                    Edit::Triggers {
                        before,
                        after: Box::new(doc.triggers.clone()),
                    },
                    json!({"galaxy_name": crate::galaxy::global_var(&name)}),
                )
            }
            Op::DeleteVariable { name } => {
                let i = doc
                    .triggers
                    .variables
                    .iter()
                    .position(|v| v.name == name)
                    .ok_or_else(|| OpError::NotFound(format!("variable \"{name}\"")))?;
                let before = Box::new(doc.triggers.clone());
                doc.triggers.variables.remove(i);
                doc.dirty.triggers = true;
                ok(
                    format!("Delete variable {name}"),
                    Edit::Triggers {
                        before,
                        after: Box::new(doc.triggers.clone()),
                    },
                    json!({}),
                )
            }
            Op::SetCustomScript { script, append } => {
                let before = Box::new(doc.triggers.clone());
                if append {
                    let cs = &mut doc.triggers.custom_script;
                    if !cs.is_empty() && !cs.ends_with('\n') {
                        cs.push('\n');
                    }
                    cs.push_str(&script);
                } else {
                    doc.triggers.custom_script = script;
                }
                if before.custom_script == doc.triggers.custom_script {
                    return invalid("custom script is already exactly that; nothing changed");
                }
                doc.dirty.triggers = true;
                let lines = doc.triggers.custom_script.lines().count();
                ok(
                    "Edit custom script".into(),
                    Edit::Triggers {
                        before,
                        after: Box::new(doc.triggers.clone()),
                    },
                    json!({"lines": lines}),
                )
            }
            Op::SetString { key, value, locale } => {
                if key.trim().is_empty() || key.contains('=') || key.contains('\n') {
                    return invalid("string key must be non-empty without '=' or newlines, e.g. \"Param/Value/lib_1_00001\"");
                }
                if !LOCALES.contains(&locale.as_str()) {
                    return invalid(format!(
                        "unknown locale \"{locale}\"; use one of {}",
                        LOCALES.join(", ")
                    ));
                }
                let before = Box::new(doc.strings.clone());
                doc.strings.set(&locale, &key, &value);
                doc.dirty.strings = true;
                ok(
                    format!("Set text {key}"),
                    Edit::Strings {
                        before,
                        after: Box::new(doc.strings.clone()),
                    },
                    json!({}),
                )
            }
            Op::SetDataField {
                file,
                class,
                id,
                field,
                value,
                parent,
            } => {
                for (what, v) in [("class", &class), ("id", &id), ("field", &field)] {
                    if v.trim().is_empty() {
                        return invalid(format!("{what} must not be empty (e.g. class \"CUnit\", id \"Marine\", field \"LifeMax\")"));
                    }
                }
                let edit = match doc
                    .data
                    .entries
                    .iter()
                    .position(|e| e.class() == class && e.id() == id)
                {
                    Some(index) => {
                        let before = doc.data.entries[index].clone();
                        let mut after = before.clone();
                        after.set_field(&field, value);
                        if let Some(p) = parent {
                            after.node.set_attr("parent", p);
                        }
                        Edit::Data {
                            index,
                            before: Some(before),
                            after: Some(after),
                        }
                    }
                    None => {
                        let file = if file.trim().is_empty() {
                            match data_file_for_class(&class) {
                                Some(f) => f.to_owned(),
                                None => {
                                    return invalid(format!(
                                        "cannot tell which GameData file holds class {class}; pass file, e.g. \"UnitData\""
                                    ))
                                }
                            }
                        } else {
                            file.trim_end_matches(".xml").to_owned()
                        };
                        let mut node = XmlNode::new(&class);
                        node.set_attr("id", &id);
                        if let Some(p) = parent {
                            node.set_attr("parent", p);
                        }
                        let mut e = CatalogEntry { file, node };
                        e.set_field(&field, value);
                        Edit::Data {
                            index: doc.data.entries.len(),
                            before: None,
                            after: Some(e),
                        }
                    }
                };
                let created = matches!(edit, Edit::Data { before: None, .. });
                edit.apply(doc, true);
                ok(
                    format!("Set {class} {id}.{field}"),
                    edit,
                    json!({"created_entry": created}),
                )
            }
            Op::SetMapInfo {
                name,
                description,
                author,
                playable,
            } => {
                if let Some(p) = playable {
                    if p[0] >= p[2]
                        || p[1] >= p[3]
                        || p[2] > doc.info.width
                        || p[3] > doc.info.height
                    {
                        return invalid(format!(
                            "playable bounds must be [left, bottom, right, top] with left < right <= {} and bottom < top <= {}",
                            doc.info.width, doc.info.height
                        ));
                    }
                }
                if name.is_none() && description.is_none() && author.is_none() && playable.is_none()
                {
                    return invalid("nothing to change: give at least one of name, description, author, playable");
                }
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
                    doc.info.playable = p;
                }
                doc.dirty.info = true;
                ok(
                    "Edit map properties".into(),
                    Edit::Info {
                        before,
                        after: Box::new(doc.info.clone()),
                    },
                    json!({}),
                )
            }
            Op::SetPlayer {
                id,
                name,
                race,
                control,
                color,
                team,
                start_location,
                clear_start_location,
            } => {
                if !(1..=MAX_PLAYER).contains(&id) {
                    return invalid(format!("player id must be 1..={MAX_PLAYER}, got {id}"));
                }
                if let Some([x, y]) = start_location {
                    check_in_map(doc, x, y)?;
                }
                if let Some(t) = team {
                    if !(1..=MAX_PLAYER).contains(&t) {
                        return invalid(format!("team must be 1..={MAX_PLAYER}, got {t}"));
                    }
                }
                let before = Box::new(doc.info.clone());
                let created = !doc.info.players.iter().any(|p| p.id == id);
                if created {
                    doc.info.players.push(Player {
                        id,
                        name: format!("Player {id}"),
                        race: Race::Random,
                        control: PlayerControl::User,
                        color: [180, 180, 180],
                        team: id,
                        start_location: None,
                    });
                    doc.info.players.sort_by_key(|p| p.id);
                }
                let p = doc
                    .info
                    .players
                    .iter_mut()
                    .find(|p| p.id == id)
                    .expect("player exists");
                if let Some(n) = name {
                    p.name = n;
                }
                if let Some(r) = race {
                    p.race = r;
                }
                if let Some(c) = control {
                    p.control = c;
                }
                if let Some(c) = color {
                    p.color = c;
                }
                if let Some(t) = team {
                    p.team = t;
                }
                if clear_start_location {
                    p.start_location = None;
                }
                if let Some(s) = start_location {
                    p.start_location = Some(glam::Vec2::from(s));
                }
                doc.dirty.info = true;
                let label = if created {
                    format!("Add player {id}")
                } else {
                    format!("Edit player {id}")
                };
                ok(
                    label,
                    Edit::Info {
                        before,
                        after: Box::new(doc.info.clone()),
                    },
                    json!({"created": created}),
                )
            }
            Op::AddTextureLayer { texture, fill } => {
                let texture = texture.trim().to_owned();
                if texture.is_empty() {
                    return invalid(
                        "texture must not be empty, e.g. \"CharDirtDark\" or \"AgriaGrass\"",
                    );
                }
                if doc.terrain.layers.len() >= MAX_TEXTURE_LAYERS {
                    return invalid(format!(
                        "this map already has the maximum of {MAX_TEXTURE_LAYERS} texture layers"
                    ));
                }
                if let Some(i) = doc.terrain.layers.iter().position(|l| l.texture == texture) {
                    return invalid(format!(
                        "texture {texture} is already layer {i}; paint with that layer instead"
                    ));
                }
                let before = doc.terrain.layers.clone();
                let n = ((doc.terrain.width + 1) * (doc.terrain.height + 1)) as usize;
                doc.terrain.layers.push(TextureLayer {
                    texture: texture.clone(),
                    weights: vec![if fill { 255 } else { 0 }; n],
                });
                doc.dirty.terrain = true;
                let layer = doc.terrain.layers.len() - 1;
                ok(
                    format!("Add texture {texture}"),
                    Edit::TextureLayers {
                        before,
                        after: doc.terrain.layers.clone(),
                    },
                    json!({"layer": layer}),
                )
            }
            Op::SetWater { .. } => invalid(
                "water can't be saved to .SC2Map files yet, so Strata refuses to change it rather than lose the edit",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::triggers::{Call, Stmt, TriggerId};

    fn doc() -> MapDocument {
        MapDocument::new("t", 64, 64)
    }

    /// Run an op, check it changed something, then undo and check the
    /// document matches the original exactly.
    fn run_and_undo(d: &mut MapDocument, op: Op) -> Applied {
        let before = serde_json::to_value(&*d).unwrap();
        let a = op.run(d).unwrap();
        assert_ne!(
            serde_json::to_value(&*d).unwrap(),
            before,
            "op changed nothing"
        );
        a.edit.apply(d, false);
        let mut after = serde_json::to_value(&*d).unwrap();
        // alloc_id bumps the counter; that is not part of the undo contract.
        after["objects"]["next_id"] = before["objects"]["next_id"].clone();
        after["triggers"]["next_id"] = before["triggers"]["next_id"].clone();
        assert_eq!(after, before, "undo did not restore the document");
        a.edit.apply(d, true);
        a
    }

    fn place(d: &mut MapDocument) -> u32 {
        let a = Op::PlaceObject {
            kind: ObjectKind::Unit,
            type_id: "Marine".into(),
            x: 10.0,
            y: 12.0,
            player: None,
            rotation: 0.0,
            name: String::new(),
        }
        .run(d)
        .unwrap();
        a.result["id"].as_u64().unwrap() as u32
    }

    #[test]
    fn op_json_roundtrip_and_run() {
        let mut doc = doc();
        let op: Op = serde_json::from_value(json!({
            "op": "place_object", "kind": "Unit", "type_id": "Marine", "x": 10.0, "y": 12.0
        }))
        .unwrap();
        assert_eq!(op.name(), "place_object");
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

    #[test]
    fn brush_validates_and_undoes() {
        let mut d = doc();
        run_and_undo(
            &mut d,
            Op::Brush {
                tool: BrushTool::Raise,
                points: vec![[32.0, 32.0]],
                radius: 6.0,
                strength: 0.5,
                shape: BrushShape::Circle,
                falloff: Falloff::Smooth,
            },
        );
        let bad_layer = Op::Brush {
            tool: BrushTool::Paint { layer: 3 },
            points: vec![[1.0, 1.0]],
            radius: 4.0,
            strength: 1.0,
            shape: BrushShape::Circle,
            falloff: Falloff::Smooth,
        };
        let e = bad_layer.run(&mut d).unwrap_err().to_string();
        assert!(e.contains("layer 3 does not exist"), "{e}");
        let bad_radius = Op::Brush {
            tool: BrushTool::Raise,
            points: vec![[1.0, 1.0]],
            radius: 100.0,
            strength: 1.0,
            shape: BrushShape::Circle,
            falloff: Falloff::Smooth,
        };
        assert!(bad_radius
            .run(&mut d)
            .unwrap_err()
            .to_string()
            .contains("between 0.5 and 64"));
        let outside = Op::Brush {
            tool: BrushTool::Raise,
            points: vec![[500.0, 500.0]],
            radius: 4.0,
            strength: 1.0,
            shape: BrushShape::Circle,
            falloff: Falloff::Smooth,
        };
        assert!(outside
            .run(&mut d)
            .unwrap_err()
            .to_string()
            .contains("changed nothing"));
    }

    #[test]
    fn place_move_edit_delete_objects() {
        let mut d = doc();
        let a = run_and_undo(
            &mut d,
            Op::PlaceObject {
                kind: ObjectKind::Point,
                type_id: String::new(),
                x: 5.0,
                y: 5.0,
                player: None,
                rotation: 0.0,
                name: String::new(),
            },
        );
        let pid = a.result["id"].as_u64().unwrap() as u32;
        assert_eq!(
            d.objects.get(ObjectId(pid)).unwrap().name,
            format!("Point {pid}")
        );
        let e = Op::PlaceObject {
            kind: ObjectKind::Unit,
            type_id: "Marine".into(),
            x: 99.0,
            y: 5.0,
            player: None,
            rotation: 0.0,
            name: String::new(),
        }
        .run(&mut d)
        .unwrap_err();
        assert!(e.to_string().contains("outside the 64x64 map"));
        let e = Op::PlaceObject {
            kind: ObjectKind::Unit,
            type_id: "Marine".into(),
            x: 9.0,
            y: 5.0,
            player: Some(16),
            rotation: 0.0,
            name: String::new(),
        }
        .run(&mut d)
        .unwrap_err();
        assert!(e.to_string().contains("player 16 does not exist"));

        let id = place(&mut d);
        run_and_undo(
            &mut d,
            Op::MoveObject {
                id,
                x: 20.0,
                y: 20.0,
            },
        );
        assert_eq!(d.objects.get(ObjectId(id)).unwrap().position.x, 20.0);
        assert!(Op::MoveObject {
            id,
            x: -1.0,
            y: 0.0
        }
        .run(&mut d)
        .is_err());
        assert!(matches!(
            Op::MoveObject {
                id: 999,
                x: 1.0,
                y: 1.0
            }
            .run(&mut d),
            Err(OpError::NotFound(_))
        ));

        run_and_undo(
            &mut d,
            Op::SetObjectProperty {
                id,
                player: Some(2),
                rotation: Some(1.5),
                scale: Some([2.0, 2.0, 2.0]),
                name: Some("Hero".into()),
                type_id: Some("Marauder".into()),
            },
        );
        let o = d.objects.get(ObjectId(id)).unwrap();
        assert_eq!(
            (o.player, o.name.as_str(), o.type_id.as_str()),
            (2, "Hero", "Marauder")
        );
        let e = Op::SetObjectProperty {
            id,
            player: None,
            rotation: None,
            scale: Some([0.0, 1.0, 1.0]),
            name: None,
            type_id: None,
        }
        .run(&mut d)
        .unwrap_err();
        assert!(e.to_string().contains("0.01..=100"));
        assert!(Op::SetObjectProperty {
            id,
            player: None,
            rotation: None,
            scale: None,
            name: None,
            type_id: None
        }
        .run(&mut d)
        .is_err());

        run_and_undo(
            &mut d,
            Op::DeleteObjects {
                ids: vec![id, pid, id],
            },
        );
        assert!(d.objects.objects.is_empty());
        assert!(Op::DeleteObjects { ids: vec![] }.run(&mut d).is_err());
        assert!(Op::DeleteObjects { ids: vec![12345] }.run(&mut d).is_err());
    }

    #[test]
    fn regions_and_cameras() {
        let mut d = doc();
        let rect = Shape::Rect {
            min: glam::Vec2::new(1.0, 1.0),
            max: glam::Vec2::new(9.0, 9.0),
        };
        run_and_undo(
            &mut d,
            Op::CreateRegion {
                name: "Base".into(),
                shape: rect.clone(),
                color: None,
            },
        );
        assert!(Op::CreateRegion {
            name: "Base".into(),
            shape: rect,
            color: None
        }
        .run(&mut d)
        .is_err());
        let bad = Shape::Rect {
            min: glam::Vec2::new(9.0, 1.0),
            max: glam::Vec2::new(1.0, 9.0),
        };
        assert!(Op::CreateRegion {
            name: "Bad".into(),
            shape: bad,
            color: None
        }
        .run(&mut d)
        .unwrap_err()
        .to_string()
        .contains("min < max"));
        run_and_undo(
            &mut d,
            Op::DeleteRegion {
                name: "Base".into(),
            },
        );
        assert!(d.regions.is_empty());

        run_and_undo(
            &mut d,
            Op::CreateCamera {
                name: "Intro".into(),
                x: 32.0,
                y: 32.0,
                distance: Some(40.0),
                pitch: None,
                yaw: Some(-90.0),
                fov: None,
            },
        );
        assert_eq!(d.cameras[0].yaw, 270.0);
        assert!(Op::CreateCamera {
            name: "X".into(),
            x: 32.0,
            y: 32.0,
            distance: Some(0.0),
            pitch: None,
            yaw: None,
            fov: None
        }
        .run(&mut d)
        .is_err());
        run_and_undo(
            &mut d,
            Op::DeleteCamera {
                name: "Intro".into(),
            },
        );
        assert!(matches!(
            Op::DeleteCamera {
                name: "Intro".into()
            }
            .run(&mut d),
            Err(OpError::NotFound(_))
        ));
    }

    #[test]
    fn triggers_variables_and_script() {
        let mut d = doc();
        let mut t = Trigger::new(TriggerId(0), "Spawn");
        t.events.push(Call {
            function: "TriggerAddEventTimeElapsed".into(),
            args: vec![],
        });
        t.actions.push(Stmt::Comment("go".into()));
        run_and_undo(&mut d, Op::CreateTrigger { trigger: t.clone() });
        assert!(Op::CreateTrigger { trigger: t.clone() }
            .run(&mut d)
            .unwrap_err()
            .to_string()
            .contains("update_trigger"));
        let clash = Trigger::new(TriggerId(0), "Sp awn");
        assert!(Op::CreateTrigger { trigger: clash }
            .run(&mut d)
            .unwrap_err()
            .to_string()
            .contains("gt_Spawn"));

        let mut t2 = t.clone();
        t2.name = "Spawn waves".into();
        t2.actions.push(Stmt::Return);
        run_and_undo(
            &mut d,
            Op::UpdateTrigger {
                name: "Spawn".into(),
                trigger: t2,
            },
        );
        let names: Vec<_> = d
            .triggers
            .triggers()
            .iter()
            .map(|t| t.name.clone())
            .collect();
        assert_eq!(names, ["Melee Initialization", "Spawn waves"]);
        assert!(Op::UpdateTrigger {
            name: "Nope".into(),
            trigger: t.clone()
        }
        .run(&mut d)
        .is_err());
        let mut rename_clash = t.clone();
        rename_clash.name = "Melee Initialization".into();
        assert!(Op::UpdateTrigger {
            name: "Spawn waves".into(),
            trigger: rename_clash
        }
        .run(&mut d)
        .is_err());
        run_and_undo(
            &mut d,
            Op::DeleteTrigger {
                name: "Spawn waves".into(),
            },
        );

        run_and_undo(
            &mut d,
            Op::CreateVariable {
                name: "Score".into(),
                ty: GalaxyType::Int,
                array_len: Some(16),
                initial: None,
                comment: String::new(),
            },
        );
        assert!(Op::CreateVariable {
            name: "Score".into(),
            ty: GalaxyType::Int,
            array_len: None,
            initial: None,
            comment: String::new()
        }
        .run(&mut d)
        .is_err());
        assert!(Op::CreateVariable {
            name: "Big".into(),
            ty: GalaxyType::Int,
            array_len: Some(0),
            initial: None,
            comment: String::new()
        }
        .run(&mut d)
        .is_err());
        assert!(Op::CreateVariable {
            name: "Odd".into(),
            ty: GalaxyType::Other("not a type".into()),
            array_len: None,
            initial: None,
            comment: String::new()
        }
        .run(&mut d)
        .is_err());
        assert!(crate::galaxy::generate(&d).contains("int[16] gv_score;"));
        run_and_undo(
            &mut d,
            Op::DeleteVariable {
                name: "Score".into(),
            },
        );

        run_and_undo(
            &mut d,
            Op::SetCustomScript {
                script: "void Foo() {}".into(),
                append: false,
            },
        );
        run_and_undo(
            &mut d,
            Op::SetCustomScript {
                script: "void Bar() {}".into(),
                append: true,
            },
        );
        assert_eq!(d.triggers.custom_script, "void Foo() {}\nvoid Bar() {}");
    }

    #[test]
    fn strings_data_info_players() {
        let mut d = doc();
        run_and_undo(
            &mut d,
            Op::SetString {
                key: "A/B".into(),
                value: "hi".into(),
                locale: "enUS".into(),
            },
        );
        assert!(Op::SetString {
            key: "A".into(),
            value: "x".into(),
            locale: "xxXX".into()
        }
        .run(&mut d)
        .unwrap_err()
        .to_string()
        .contains("enUS"));
        assert!(Op::SetString {
            key: "".into(),
            value: "x".into(),
            locale: "enUS".into()
        }
        .run(&mut d)
        .is_err());

        let a = run_and_undo(
            &mut d,
            Op::SetDataField {
                file: String::new(),
                class: "CUnit".into(),
                id: "Marine".into(),
                field: "LifeMax".into(),
                value: "55".into(),
                parent: None,
            },
        );
        assert_eq!(a.result["created_entry"], true);
        assert_eq!(d.data.entries[0].file, "UnitData");
        run_and_undo(
            &mut d,
            Op::SetDataField {
                file: String::new(),
                class: "CUnit".into(),
                id: "Marine".into(),
                field: "LifeMax".into(),
                value: "60".into(),
                parent: None,
            },
        );
        assert_eq!(
            d.data.find("CUnit", "Marine").unwrap().field("LifeMax"),
            Some("60")
        );
        assert!(Op::SetDataField {
            file: String::new(),
            class: "CFoo".into(),
            id: "x".into(),
            field: "f".into(),
            value: "1".into(),
            parent: None
        }
        .run(&mut d)
        .is_err());

        run_and_undo(
            &mut d,
            Op::SetMapInfo {
                name: Some("New".into()),
                description: None,
                author: None,
                playable: Some([4, 4, 60, 60]),
            },
        );
        assert!(Op::SetMapInfo {
            name: None,
            description: None,
            author: None,
            playable: Some([4, 4, 70, 60])
        }
        .run(&mut d)
        .is_err());
        assert_eq!(d.info.playable, [4, 4, 60, 60]);

        let a = run_and_undo(
            &mut d,
            Op::SetPlayer {
                id: 3,
                name: Some("Ally".into()),
                race: Some(Race::Zerg),
                control: Some(PlayerControl::Computer),
                color: None,
                team: Some(1),
                start_location: Some([10.0, 10.0]),
                clear_start_location: false,
            },
        );
        assert_eq!(a.result["created"], true);
        assert_eq!(
            d.info.players.iter().map(|p| p.id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        run_and_undo(
            &mut d,
            Op::SetPlayer {
                id: 3,
                name: None,
                race: None,
                control: None,
                color: None,
                team: None,
                start_location: None,
                clear_start_location: true,
            },
        );
        assert!(Op::SetPlayer {
            id: 16,
            name: None,
            race: None,
            control: None,
            color: None,
            team: None,
            start_location: None,
            clear_start_location: false
        }
        .run(&mut d)
        .is_err());
    }

    #[test]
    fn texture_layers_and_water() {
        let mut d = doc();
        let a = run_and_undo(
            &mut d,
            Op::AddTextureLayer {
                texture: "CharGrass".into(),
                fill: false,
            },
        );
        assert_eq!(a.result["layer"], 1);
        assert!(Op::AddTextureLayer {
            texture: "CharGrass".into(),
            fill: false
        }
        .run(&mut d)
        .unwrap_err()
        .to_string()
        .contains("already layer 1"));
        run_and_undo(
            &mut d,
            Op::Brush {
                tool: BrushTool::Paint { layer: 1 },
                points: vec![[10.0, 10.0]],
                radius: 4.0,
                strength: 1.0,
                shape: BrushShape::Circle,
                falloff: Falloff::Smooth,
            },
        );
        for i in 0..6 {
            Op::AddTextureLayer {
                texture: format!("T{i}"),
                fill: i == 0,
            }
            .run(&mut d)
            .unwrap();
        }
        assert!(Op::AddTextureLayer {
            texture: "More".into(),
            fill: false
        }
        .run(&mut d)
        .unwrap_err()
        .to_string()
        .contains("maximum of 8"));

        assert!(Op::SetWater { height: Some(9.0) }.run(&mut d).is_err());
        assert_eq!(d.terrain.water_height, None);
    }
}
