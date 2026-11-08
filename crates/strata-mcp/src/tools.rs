//! The MCP tool catalog: names, descriptions and JSON Schemas written for an
//! AI reader, plus translation from tool arguments to [`Op`]s and [`Query`]s.

use std::path::PathBuf;
use std::sync::OnceLock;

use serde_json::{json, Map, Value};
use strata_core::brush::{BrushShape, BrushTool, Falloff};
use strata_core::catalog::CatalogKind;
use strata_core::document::{PlayerControl, Race};
use strata_core::objects::ObjectKind;
use strata_core::ops::link::LinkKind;
use strata_core::ops::query::Query;
use strata_core::ops::Op;
use strata_core::regions::Shape;
use strata_core::terrain::pathing;
use strata_core::MapDocument;

use crate::server::{Outcome, Strata};
use crate::trigger_json;

pub struct ToolDef {
    pub name: &'static str,
    pub title: &'static str,
    pub description: String,
    pub schema: Value,
    pub read_only: bool,
    pub destructive: bool,
}

#[derive(Debug, PartialEq)]
pub enum CallError {
    /// No such tool (a protocol error, not a tool failure).
    UnknownTool(String),
    /// The tool ran and failed; shown to the model with `isError: true`.
    Failed(String),
}

impl From<String> for CallError {
    fn from(s: String) -> Self {
        CallError::Failed(s)
    }
}

impl From<&str> for CallError {
    fn from(s: &str) -> Self {
        CallError::Failed(s.to_owned())
    }
}

// ---------------------------------------------------------------------------
// Schema helpers

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required, "additionalProperties": false})
}
fn num(d: &str) -> Value {
    json!({"type": "number", "description": d})
}
fn int(d: &str, min: i64, max: i64) -> Value {
    json!({"type": "integer", "description": d, "minimum": min, "maximum": max})
}
fn string(d: &str) -> Value {
    json!({"type": "string", "description": d})
}
fn boolean(d: &str) -> Value {
    json!({"type": "boolean", "description": d})
}
fn enumeration(d: &str, values: &[&str]) -> Value {
    json!({"type": "string", "description": d, "enum": values})
}
fn points(d: &str) -> Value {
    json!({"type": "array", "description": d, "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}, "minItems": 1})
}
fn rect(d: &str) -> Value {
    json!({"type": "array", "description": d, "items": {"type": "number"}, "minItems": 4, "maxItems": 4})
}
fn color(d: &str) -> Value {
    json!({"type": ["array", "string"], "description": format!("{d} Either [r, g, b] with 0..255 each or \"#rrggbb\"."), "items": {"type": "integer", "minimum": 0, "maximum": 255}, "minItems": 3, "maxItems": 3})
}
fn limit() -> Value {
    int(
        "Maximum results to return (default 200, max 5000).",
        1,
        5000,
    )
}
fn discard() -> Value {
    boolean("Replace the current map even if it has unsaved changes. Default false: the call fails instead, so you can save first.")
}

const ANY_EXPR: &str = "Expression: a number, true/false, a Galaxy source string (\"c_playerAny\", \"\\\"quoted text\\\"\", \"gv_score + 1\") or an object form such as {\"var\": \"Score\"}.";

const TRIGGER_FORMS: &str = r#"Simplified trigger JSON:
{"name": "Spawn wave",
 "comment": "optional",
 "enabled": true, "initially_on": true,
 "events": [{"function": "TriggerAddEventTimePeriodic", "args": [30.0, "c_timeGame"]}],
 "conditions": ["gv_wave < 10"],
 "actions": [
   {"function": "UnitCreate", "args": [5, "\"Zergling\"", 0, 2, {"point": "Spawn"}, 270.0]},
   {"set": "wave", "value": "gv_wave + 1"},
   {"if": "gv_wave == 10", "then": [{"function": "UIDisplayMessage", "args": ["PlayerGroupAll()", "c_messageAreaSubtitle", {"text": "Msg/Final"}]}], "else": []},
   {"for": "i", "from": 1, "to": 3, "do": [{"wait": 1.0}]},
   "UnitKill(gv_boss)"
 ]}
Events are TriggerAddEvent* natives; the trigger handle is inserted as their first argument automatically, so leave it out of args.
Expressions: numbers (integers -> int, decimals -> fixed), true/false, and strings holding Galaxy source. Simple tokens become structured values ("c_..." presets, "\"quoted\"" strings, numbers); anything else is kept verbatim. Object forms: {"string": "hi"}, {"text": "String/Key"} (localized text, StringExternal), {"int": 3}, {"fixed": 1.5}, {"var": "Score"} (global variable by editor name -> gv_Score), {"preset": "c_x"}, {"point": "Name"} (placed point by name), {"raw": "galaxy"}, {"function": "Fn", "args": [...]}, {"op": "==", "lhs": .., "rhs": ..}, {"not": expr}.
Actions: {"function": "Fn", "args": [...]}, {"set": "Var", "value": expr}, {"if": cond, "then": [...], "else": [...]}, {"while": cond, "do": [...]}, {"for": "i", "from": e, "to": e, "do": [...]}, {"wait": seconds}, {"comment": "..."}, {"raw": "Galaxy statements;"}, "return", or a string of Galaxy statements.
The full model JSON (as returned by get_trigger under "trigger") is also accepted."#;

fn trigger_props(extra: Value) -> Value {
    let mut p = json!({
        "trigger": {"type": "object", "description": "The whole trigger as one object (simplified or model form). Use this or the separate fields below."},
        "comment": string("Designer note shown in the trigger editor."),
        "enabled": boolean("Default true. Disabled triggers never run."),
        "initially_on": boolean("Default true. If false the trigger starts turned off until TriggerEnable is called."),
        "events": {"type": "array", "description": "Event registrations: [{\"function\": \"TriggerAddEventMapInit\", \"args\": []}]. Omit the trigger argument.", "items": {"type": ["object", "string"]}},
        "conditions": {"type": "array", "description": "Conditions, all must be true. Each is an expression.", "items": {}},
        "actions": {"type": "array", "description": "Actions run in order. See the forms in the tool description.", "items": {}},
    });
    if let (Some(p), Value::Object(e)) = (p.as_object_mut(), extra) {
        p.extend(e);
    }
    p
}

fn def(
    name: &'static str,
    title: &'static str,
    description: impl Into<String>,
    schema: Value,
) -> ToolDef {
    ToolDef {
        name,
        title,
        description: description.into(),
        schema,
        read_only: false,
        destructive: false,
    }
}
fn read(mut d: ToolDef) -> ToolDef {
    d.read_only = true;
    d
}
fn destructive(mut d: ToolDef) -> ToolDef {
    d.destructive = true;
    d
}

pub fn defs() -> &'static [ToolDef] {
    static DEFS: OnceLock<Vec<ToolDef>> = OnceLock::new();
    DEFS.get_or_init(build_defs)
}

fn build_defs() -> Vec<ToolDef> {
    let xy = "World units (1 unit = 1 terrain cell). (0, 0) is the south-west corner; x grows east, y grows north.";
    vec![
        read(def("editor_status", "Editor status",
            "Report where tool calls go: \"editor\" (a running Strata editor connected over the live link; edits appear there immediately and are undoable, labeled AI) or \"headless\" (an in-memory map in this server, saved with map_save). Also reports whether the user allows AI edits, the map name and save state. Call this first.",
            obj(json!({}), &[]))),
        read(def("map_summary", "Map summary",
            "Overview of the open map: name, size, playable bounds, players, counts of units/doodads/points/regions/cameras/triggers/variables/data entries/strings, terrain height range, texture layers, water, problem counts and undo state.",
            obj(json!({}), &[]))),
        destructive(def("map_new", "New map",
            "Replace the open map with a new flat map (default 128x128, tileset Char, two user players and the standard Melee Initialization trigger). Fails if the current map has unsaved changes unless discard_changes is true.",
            obj(json!({
                "name": string("Map name. Default \"Untitled map\"."),
                "width": int("Width in cells, default 128.", 16, 256),
                "height": int("Height in cells, default 128.", 16, 256),
                "tileset": string("Tileset id, e.g. \"Char\", \"Agria\", \"Bel'Shir\". Default \"Char\"."),
                "discard_changes": discard(),
            }), &[]))),
        destructive(def("map_open", "Open map",
            "Open a map from disk on this machine: a .SC2Map archive, an unpacked map folder, or a .strata.json document. Relative paths resolve against the server's working directory. In live mode the map opens in the editor.",
            obj(json!({
                "path": string("File or folder path, e.g. \"C:/Maps/Arena.SC2Map\" or \"arena.strata.json\"."),
                "discard_changes": discard(),
            }), &["path"]))),
        def("map_save", "Save map",
            "Save the open map to disk. Path must end in .SC2Map or .strata.json (Strata's lossless JSON form, always available), or be an existing map folder. Omit path to save where the map was opened or last saved. In live mode the editor's map is saved and marked as saved there.",
            obj(json!({"path": string("Where to save. Optional after the first save.")}), &[])),

        def("terrain_brush", "Sculpt or paint terrain",
            format!("Apply a terrain brush along a path, as one undoable stroke. {xy} Consecutive points are joined into a continuous stroke. \
Tools: raise / lower move height by up to 4*strength units at the brush center per pass; smooth averages; flatten pulls toward `height`; noise adds roughness (seed); \
paint / erase change texture `layer` weight (see map_summary texture_layers; add one with add_texture_layer); cliff sets cliff `level` 0..15 (each level is 2 height units; new maps are level 2 at height 12); \
pathing sets (on: true) or clears `flags` on cells. Heights clamp to -16..64. Use passes to repeat the stroke for stronger effects. Example: {{\"tool\": \"raise\", \"points\": [[40, 40], [60, 40]], \"radius\": 8, \"strength\": 0.6}}"),
            obj(json!({
                "tool": enumeration("Brush tool.", &["raise", "lower", "smooth", "flatten", "noise", "paint", "erase", "cliff", "pathing"]),
                "points": points("Stroke path as [[x, y], ...]. Or give x and y for a single dab."),
                "x": num("Single dab x (instead of points)."),
                "y": num("Single dab y (instead of points)."),
                "radius": json!({"type": "number", "description": "Brush radius in world units, 0.5..64 (default 6).", "minimum": 0.5, "maximum": 64}),
                "strength": json!({"type": "number", "description": "0..1 (default 0.35).", "minimum": 0, "maximum": 1}),
                "shape": enumeration("Default circle.", &["circle", "square"]),
                "falloff": enumeration("Edge softness; default smooth. constant gives hard edges (good for cliffs, pathing, flatten).", &["constant", "linear", "smooth", "sharp"]),
                "height": num("flatten: target height (-16..64)."),
                "seed": int("noise: random seed.", 0, u32::MAX as i64),
                "layer": int("paint / erase: texture layer index.", 0, 7),
                "level": int("cliff: cliff level 0..15.", 0, 15),
                "flags": {"type": "array", "description": "pathing: which flags to set or clear.", "items": {"type": "string", "enum": ["unwalkable"]}},
                "on": boolean("pathing: true sets the flags (default), false clears them."),
                "passes": int("Repeat the stroke this many times (default 1).", 1, 50),
            }), &["tool"])),
        read(def("terrain_stats", "Terrain statistics",
            format!("Height min/max/mean, cliff level and texture coverage percentages, pathing percentages and water height for the whole map or a rectangle. {xy}"),
            obj(json!({"rect": rect("Optional [x0, y0, x1, y1] area.")}), &[]))),
        read(def("sample_height", "Sample terrain",
            format!("Terrain height, cliff level and pathing flags at specific points. {xy}"),
            obj(json!({"points": points("Points as [[x, y], ...].")}), &["points"]))),
        def("add_texture_layer", "Add texture layer",
            "Append a terrain texture layer (maximum 8 per map). Returns its layer index; paint it with terrain_brush tool \"paint\". Texture ids depend on the tileset, e.g. CharDirt, CharDirtDark, CharGrassDead, AgriaGrass, AgriaDirt, BelShirJungleGrass.",
            obj(json!({
                "texture": string("Texture id, e.g. \"CharDirtDark\"."),
                "fill": boolean("Cover the whole map with it (default false: starts empty)."),
            }), &["texture"])),

        def("place_object", "Place object",
            format!("Place a unit, doodad or point. {xy} The z coordinate follows the terrain. Returns the new object's id. Unit types are SC2 catalog ids (Marine, SCV, Zergling, Hatchery, CommandCenter, MineralField, VespeneGeyser, XelNagaTower...); doodads are model ids (e.g. CharRock01)."),
            obj(json!({
                "kind": enumeration("Default unit.", &["unit", "doodad", "point"]),
                "type_id": string("Unit type or doodad model id. Not needed for points."),
                "x": num("X position."),
                "y": num("Y position."),
                "player": int("Owner for units: 0 neutral, 1..15 players (default 1).", 0, 15),
                "facing": num("Facing in degrees counter-clockwise from east (0 = east, 90 = north, 270 = south). Default 0."),
                "name": string("Optional name. Points are referenced by name in triggers ({\"point\": \"Name\"})."),
            }), &["x", "y"])),
        def("move_object", "Move object",
            format!("Move a placed object to a new position. {xy}"),
            obj(json!({"id": int("Object id from list_objects.", 1, u32::MAX as i64), "x": num("New x."), "y": num("New y.")}), &["id", "x", "y"])),
        def("set_object_property", "Edit object",
            "Change properties of a placed object. Omitted properties stay as they are.",
            obj(json!({
                "id": int("Object id from list_objects.", 1, u32::MAX as i64),
                "player": int("Owner: 0 neutral, 1..15.", 0, 15),
                "facing": num("Facing in degrees counter-clockwise from east."),
                "scale": {"type": ["number", "array"], "description": "Uniform scale (e.g. 1.5) or [x, y, z]; each 0.01..100.", "items": {"type": "number"}, "minItems": 3, "maxItems": 3},
                "name": string("Object name."),
                "type_id": string("New unit type or doodad model id."),
            }), &["id"])),
        destructive(def("delete_objects", "Delete objects",
            "Delete placed objects by id, as one undoable step.",
            obj(json!({"ids": {"type": "array", "description": "Object ids from list_objects.", "items": {"type": "integer", "minimum": 1}, "minItems": 1}}), &["ids"]))),
        read(def("list_objects", "List objects",
            format!("List placed units, doodads and points with id, type, name, position, facing, scale and owner. All filters combine. {xy}"),
            obj(json!({
                "kind": enumeration("Only this kind.", &["unit", "doodad", "point"]),
                "type_id": string("Only this exact type id, e.g. \"Marine\"."),
                "player": int("Only units owned by this player.", 0, 15),
                "rect": rect("Only objects inside [x0, y0, x1, y1]."),
                "ids": {"type": "array", "description": "Only these ids.", "items": {"type": "integer"}},
                "search": string("Case-insensitive substring of name or type id."),
                "limit": limit(),
            }), &[]))),

        def("create_region", "Create region",
            format!("Create a named region (trigger area). Give exactly one of rect, circle or polygon. {xy}"),
            obj(json!({
                "name": string("Unique region name, referenced by triggers."),
                "rect": rect("Rectangle [x0, y0, x1, y1]."),
                "circle": json!({"type": "array", "description": "Circle [center_x, center_y, radius].", "items": {"type": "number"}, "minItems": 3, "maxItems": 3}),
                "polygon": points("Polygon corners [[x, y], ...], at least 3, counter-clockwise."),
                "color": color("Display color in the editor."),
            }), &["name"])),
        destructive(def("delete_region", "Delete region", "Delete a region by name.", obj(json!({"name": string("Region name.")}), &["name"]))),
        read(def("list_regions", "List regions and cameras", "List regions (name, color, shapes) and cameras (name, target, distance, pitch, yaw, fov).", obj(json!({}), &[]))),
        def("create_camera", "Create camera",
            format!("Create a named camera object looking at (x, y). {xy}"),
            obj(json!({
                "name": string("Unique camera name."),
                "x": num("Target x."),
                "y": num("Target y."),
                "distance": num("Distance from target, 1..500 (default 34)."),
                "pitch": num("Pitch in degrees, -90..90 (default 56)."),
                "yaw": num("Yaw in degrees (default 90)."),
                "fov": num("Field of view in degrees, 1..170 (default 27)."),
            }), &["name", "x", "y"])),

        def("create_trigger", "Create trigger",
            format!("Create a trigger (event -> conditions -> actions) that compiles to Galaxy in MapScript.galaxy. Pass the fields directly or as one `trigger` object. Check the result with get_trigger or get_galaxy_script.\n\n{TRIGGER_FORMS}"),
            obj(trigger_props(json!({"name": string("Unique trigger name.")})), &[])),
        def("update_trigger", "Replace trigger",
            format!("Replace the trigger called `name` entirely (fields you omit become empty or default), keeping its place in the trigger list. Read it first with get_trigger. To rename, pass new_name (or a different name inside `trigger`).\n\n{TRIGGER_FORMS}"),
            obj(trigger_props(json!({
                "name": string("Name of the existing trigger to replace."),
                "new_name": string("Optional new name."),
            })), &["name"])),
        destructive(def("delete_trigger", "Delete trigger", "Delete a trigger by name.", obj(json!({"name": string("Trigger name.")}), &["name"]))),
        read(def("list_triggers", "List triggers",
            "List triggers (name, folder, enabled, event functions, condition and action counts), global variables (name, Galaxy name, type, initial value) and the custom script length.",
            obj(json!({}), &[]))),
        read(def("get_trigger", "Get trigger",
            "One trigger as model JSON (editable and accepted back by update_trigger) plus the Galaxy code it compiles to.",
            obj(json!({"name": string("Trigger name.")}), &["name"]))),
        read(def("get_galaxy_script", "Get Galaxy script",
            "The generated MapScript.galaxy as plain text: variables, custom script, trigger functions and InitMap. Pass trigger to get only that trigger's functions.",
            obj(json!({"trigger": string("Optional trigger name.")}), &[]))),
        def("create_variable", "Create variable",
            "Add a global trigger variable. In Galaxy it is named gv_<Name>; in trigger JSON reference it as {\"var\": \"Name\"} or \"gv_Name\".",
            obj(json!({
                "name": string("Variable name, e.g. \"Wave\"."),
                "type": string("Galaxy type: int, fixed, bool, string, text, point, region, unit, unitgroup, playergroup, timer, trigger, sound, color, or another Galaxy type name such as actor or dialog."),
                "array_len": int("Make it an array of this length.", 1, 65536),
                "initial": json!({"description": format!("Initial value (not for arrays). {ANY_EXPR}")}),
                "comment": string("Designer note."),
            }), &["name", "type"])),
        destructive(def("delete_variable", "Delete variable", "Delete a global trigger variable by name.", obj(json!({"name": string("Variable name.")}), &["name"]))),
        def("set_custom_script", "Set custom script",
            "Replace (or append to) the map's custom Galaxy script, which is emitted after the global variables and before the trigger functions. Use it for helper functions your triggers call.",
            obj(json!({
                "script": string("Galaxy source."),
                "append": boolean("Append instead of replacing (default false)."),
            }), &["script"])),

        def("set_string", "Set text",
            "Set a localized string (GameStrings.txt entry). Reference it from triggers with {\"text\": \"Key\"}.",
            obj(json!({
                "key": string("String key, e.g. \"Msg/Welcome\"."),
                "value": string("Text."),
                "locale": string("Locale, default enUS. One of enUS deDE esES esMX frFR itIT koKR plPL ptBR ruRU zhCN zhTW."),
            }), &["key", "value"])),
        read(def("get_strings", "Get text",
            "List localized strings for one locale (default enUS).",
            obj(json!({
                "locale": string("Locale, default enUS."),
                "prefix": string("Only keys starting with this."),
                "search": string("Case-insensitive substring of key or value."),
                "limit": limit(),
            }), &[]))),
        def("set_data_field", "Set data field",
            "Set a scalar field on a map GameData catalog entry (e.g. CUnit Marine LifeMax = 55), creating the entry if the map does not override it yet. Only simple value fields (<Field value=\"...\"/>) are supported. To make a new unit based on another, set parent.",
            obj(json!({
                "class": string("Catalog class, e.g. CUnit, CWeaponLegacy, CEffectDamage, CAbilEffectTarget, CBehaviorBuff, CUpgrade."),
                "id": string("Entry id, e.g. \"Marine\"."),
                "field": string("Field name, e.g. \"LifeMax\", \"Speed\", \"CostResource\"."),
                "value": string("New value as text, e.g. \"55\"."),
                "parent": string("Parent entry id for a new entry (inherits its fields)."),
                "file": string("GameData file, e.g. \"UnitData\". Only needed for classes Strata cannot place."),
            }), &["class", "id", "field", "value"])),
        read(def("list_base_units", "List base-game units",
            "List the real placeable units of StarCraft II (from the user's install: Core, Liberty, Swarm, Void and the multiplayer mods) with their display names, race and size. Use these ids for place_unit, set_data_field parents and triggers. editor_status.base_data says whether the data is loaded.",
            obj(json!({
                "search": string("Case-insensitive substring of id or display name, e.g. \"siege\"."),
                "race": enumeration("Only this race.", &["terran", "protoss", "zerg", "neutral"]),
                "melee_only": boolean("Only multiplayer (melee) units. Default true; false adds campaign, hero and story units."),
                "limit": limit(),
            }), &[]))),
        read(def("search_base_data", "Search base-game data",
            "Search the base game's catalogs (units, abilities, weapons, effects, behaviors, upgrades, actors, models, buttons, requirements) by id or display name.",
            obj(json!({
                "query": string("Case-insensitive substring of id or display name."),
                "kind": enumeration("Catalog family.", &["unit", "ability", "weapon", "effect", "behavior", "upgrade", "actor", "model", "button", "requirement"]),
                "limit": limit(),
            }), &["query"]))),
        read(def("get_base_data", "Get base-game entry",
            "One base-game catalog entry with every field it has after inheritance (its parents and class default), e.g. Marine: LifeMax 45, Speed 2.25. Map entries with the same id or this id as parent inherit these values.",
            obj(json!({
                "kind": enumeration("Catalog family.", &["unit", "ability", "weapon", "effect", "behavior", "upgrade", "actor", "model", "button", "requirement"]),
                "id": string("Entry id, e.g. \"Marine\"."),
                "full": boolean("Also return the entry's merged XML (default false)."),
            }), &["kind", "id"]))),
        read(def("list_data", "List data",
            "List the map's GameData catalog entries (only what this map defines or overrides, not the built-in game data; see search_base_data and get_base_data for that) with their scalar fields.",
            obj(json!({
                "kind": enumeration("Catalog family.", &["unit", "ability", "weapon", "effect", "behavior", "upgrade", "actor", "model", "button", "requirement", "other"]),
                "class": string("Only this class, e.g. \"CUnit\"."),
                "search": string("Case-insensitive substring of id, class or parent."),
                "full": boolean("Include each entry's whole XML tree (default false)."),
                "limit": limit(),
            }), &[]))),

        def("set_map_info", "Edit map properties",
            "Set the map name, description, author or playable bounds. Omitted fields stay as they are.",
            obj(json!({
                "name": string("Map name."),
                "description": string("Map description."),
                "author": string("Author."),
                "playable": json!({"type": "array", "description": "Playable bounds [left, bottom, right, top] in cells.", "items": {"type": "integer", "minimum": 0}, "minItems": 4, "maxItems": 4}),
            }), &[])),
        def("set_player", "Edit player",
            "Create or update a player slot. Omitted fields stay as they are (new slots default to a user-controlled Random player).",
            obj(json!({
                "id": int("Player slot 1..15.", 1, 15),
                "name": string("Player name."),
                "race": enumeration("Race.", &["terran", "protoss", "zerg", "random", "neutral"]),
                "control": enumeration("Who controls the slot.", &["user", "computer", "neutral", "hostile", "open"]),
                "color": color("Player color."),
                "team": int("Team number 1..15.", 1, 15),
                "start_location": {"type": ["array", "null"], "description": "Start location [x, y], or null to remove it.", "items": {"type": "number"}, "minItems": 2, "maxItems": 2},
            }), &["id"])),
        read(def("validate_map", "Validate map",
            "Check the map for problems (errors, warnings, info) such as objects outside the map, players that do not exist, trigger name clashes and missing translations.",
            obj(json!({}), &[]))),
        def("undo", "Undo",
            "Undo the most recent change(s) on the active target, including changes the user made in the editor. Check history first if unsure.",
            obj(json!({"steps": int("How many changes to undo (default 1).", 1, 50)}), &[])),
        def("redo", "Redo", "Redo changes undone with undo.", obj(json!({"steps": int("How many changes to redo (default 1).", 1, 50)}), &[])),
        read(def("history", "History",
            "The undo and redo stacks, newest first, each entry labeled with who made it (user or ai), plus whether the map has unsaved changes.",
            obj(json!({"limit": int("Entries per stack (default 50).", 1, 500)}), &[]))),
    ]
}

/// The tools/list payload.
pub fn list_json() -> Vec<Value> {
    defs()
        .iter()
        .map(|d| {
            json!({
                "name": d.name,
                "title": d.title,
                "description": d.description,
                "inputSchema": d.schema,
                "annotations": {
                    "title": d.title,
                    "readOnlyHint": d.read_only,
                    "destructiveHint": d.destructive,
                    "idempotentHint": d.read_only,
                    "openWorldHint": false,
                },
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Argument reading

struct Args<'a>(&'a Map<String, Value>);

fn type_err(k: &str, want: &str, got: &Value) -> String {
    format!("\"{k}\" must be {want}, got {got}")
}

impl<'a> Args<'a> {
    fn get(&self, k: &str) -> Option<&'a Value> {
        self.0.get(k).filter(|v| !v.is_null())
    }
    fn has(&self, k: &str) -> bool {
        self.get(k).is_some()
    }
    fn f32(&self, k: &str) -> Result<Option<f32>, String> {
        self.get(k)
            .map(|v| {
                v.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| type_err(k, "a number", v))
            })
            .transpose()
    }
    fn need_f32(&self, k: &str) -> Result<f32, String> {
        self.f32(k)?
            .ok_or_else(|| format!("\"{k}\" is required (a number)"))
    }
    fn int(&self, k: &str, min: i64, max: i64) -> Result<Option<i64>, String> {
        let Some(v) = self.get(k) else {
            return Ok(None);
        };
        let i = v
            .as_i64()
            .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64));
        let i = i.ok_or_else(|| type_err(k, "an integer", v))?;
        if !(min..=max).contains(&i) {
            return Err(format!("\"{k}\" must be {min}..={max}, got {i}"));
        }
        Ok(Some(i))
    }
    fn need_int(&self, k: &str, min: i64, max: i64) -> Result<i64, String> {
        self.int(k, min, max)?
            .ok_or_else(|| format!("\"{k}\" is required (an integer {min}..={max})"))
    }
    fn str(&self, k: &str) -> Result<Option<String>, String> {
        self.get(k)
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| type_err(k, "a string", v))
            })
            .transpose()
    }
    fn need_str(&self, k: &str) -> Result<String, String> {
        self.str(k)?
            .ok_or_else(|| format!("\"{k}\" is required (a string)"))
    }
    fn bool(&self, k: &str) -> Result<Option<bool>, String> {
        self.get(k)
            .map(|v| v.as_bool().ok_or_else(|| type_err(k, "true or false", v)))
            .transpose()
    }
    fn floats(&self, k: &str, n: usize) -> Result<Option<Vec<f32>>, String> {
        let Some(v) = self.get(k) else {
            return Ok(None);
        };
        let a = v
            .as_array()
            .filter(|a| a.len() == n)
            .ok_or_else(|| type_err(k, &format!("an array of {n} numbers"), v))?;
        a.iter()
            .map(|x| {
                x.as_f64()
                    .map(|f| f as f32)
                    .ok_or_else(|| type_err(k, &format!("an array of {n} numbers"), v))
            })
            .collect::<Result<_, _>>()
            .map(Some)
    }
    fn point(&self, k: &str) -> Result<Option<[f32; 2]>, String> {
        Ok(self.floats(k, 2)?.map(|v| [v[0], v[1]]))
    }
    fn rect(&self, k: &str) -> Result<Option<[f32; 4]>, String> {
        Ok(self.floats(k, 4)?.map(|v| [v[0], v[1], v[2], v[3]]))
    }
    fn points(&self, k: &str) -> Result<Option<Vec<[f32; 2]>>, String> {
        let Some(v) = self.get(k) else {
            return Ok(None);
        };
        let want = "an array of [x, y] pairs";
        let a = v.as_array().ok_or_else(|| type_err(k, want, v))?;
        a.iter()
            .map(|p| {
                match p
                    .as_array()
                    .map(|p| p.iter().map(Value::as_f64).collect::<Vec<_>>())
                {
                    Some(xy) if xy.len() == 2 && xy.iter().all(Option::is_some) => {
                        Ok([xy[0].unwrap_or(0.0) as f32, xy[1].unwrap_or(0.0) as f32])
                    }
                    _ => Err(type_err(k, want, v)),
                }
            })
            .collect::<Result<_, _>>()
            .map(Some)
    }
    fn ids(&self, k: &str) -> Result<Option<Vec<u32>>, String> {
        let Some(v) = self.get(k) else {
            return Ok(None);
        };
        let a = v
            .as_array()
            .ok_or_else(|| type_err(k, "an array of object ids", v))?;
        a.iter()
            .map(|x| {
                x.as_u64()
                    .filter(|i| *i <= u32::MAX as u64)
                    .map(|i| i as u32)
                    .ok_or_else(|| type_err(k, "an array of object ids", v))
            })
            .collect::<Result<_, _>>()
            .map(Some)
    }
    fn color(&self, k: &str) -> Result<Option<[u8; 3]>, String> {
        let Some(v) = self.get(k) else {
            return Ok(None);
        };
        let bad = || type_err(k, "[r, g, b] (0..255 each) or \"#rrggbb\"", v);
        if let Some(s) = v.as_str() {
            let h = s.trim().trim_start_matches('#');
            if h.len() != 6 {
                return Err(bad());
            }
            let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).map_err(|_| bad());
            return Ok(Some([byte(0)?, byte(2)?, byte(4)?]));
        }
        let a = v.as_array().filter(|a| a.len() == 3).ok_or_else(bad)?;
        let c: Vec<u8> = a
            .iter()
            .map(|x| {
                x.as_u64()
                    .filter(|c| *c <= 255)
                    .map(|c| c as u8)
                    .ok_or_else(bad)
            })
            .collect::<Result<_, _>>()?;
        Ok(Some([c[0], c[1], c[2]]))
    }
    fn choice<T: Copy>(&self, k: &str, options: &[(&str, T)]) -> Result<Option<T>, String> {
        let Some(s) = self.str(k)? else {
            return Ok(None);
        };
        options
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(s.trim()))
            .map(|(_, t)| Some(*t))
            .ok_or_else(|| {
                format!(
                    "\"{k}\" must be one of {}, got \"{s}\"",
                    options
                        .iter()
                        .map(|(n, _)| *n)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }
}

const KINDS: [(&str, ObjectKind); 3] = [
    ("unit", ObjectKind::Unit),
    ("doodad", ObjectKind::Doodad),
    ("point", ObjectKind::Point),
];

fn facing(a: &Args, k: &str) -> Result<Option<f32>, String> {
    Ok(a.f32(k)?.map(|d| d.rem_euclid(360.0).to_radians()))
}

/// Join consecutive points with dabs every `radius / 3` so strokes are continuous.
fn densify(points: &[[f32; 2]], radius: f32) -> Vec<[f32; 2]> {
    let step = (radius / 3.0).max(0.25);
    let mut out = Vec::new();
    for (i, p) in points.iter().enumerate() {
        if i > 0 {
            let q = points[i - 1];
            let d = ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
            let n = ((d / step).ceil() as usize).min(4096);
            for s in 1..n {
                let t = s as f32 / n as f32;
                out.push([q[0] + (p[0] - q[0]) * t, q[1] + (p[1] - q[1]) * t]);
            }
        }
        out.push(*p);
    }
    out
}

fn brush_op(a: &Args) -> Result<Op, String> {
    let radius = a.f32("radius")?.unwrap_or(6.0);
    let strength = a.f32("strength")?.unwrap_or(0.35);
    let mut pts = match (a.points("points")?, a.f32("x")?, a.f32("y")?) {
        (Some(p), _, _) if !p.is_empty() => p,
        (None, Some(x), Some(y)) => vec![[x, y]],
        _ => return Err("give points [[x, y], ...] or x and y".into()),
    };
    pts = densify(&pts, radius);
    let passes = a.int("passes", 1, 50)?.unwrap_or(1) as usize;
    let one = pts.clone();
    for _ in 1..passes {
        pts.extend_from_slice(&one);
    }
    let tool_name = a.need_str("tool")?;
    let tool = match tool_name.to_ascii_lowercase().as_str() {
        "raise" => BrushTool::Raise,
        "lower" => BrushTool::Lower,
        "smooth" => BrushTool::Smooth,
        "flatten" => BrushTool::Flatten {
            target: a.f32("height")?.ok_or("flatten needs \"height\", the target height (new maps are at 12)")?,
        },
        "noise" => BrushTool::Noise { seed: a.int("seed", 0, u32::MAX as i64)?.unwrap_or(1) as u32 },
        "paint" => BrushTool::Paint { layer: a.int("layer", 0, 7)?.ok_or("paint needs \"layer\" (see map_summary texture_layers)")? as usize },
        "erase" => BrushTool::Erase { layer: a.int("layer", 0, 7)?.ok_or("erase needs \"layer\"")? as usize },
        "cliff" => BrushTool::Cliff { level: a.int("level", 0, 15)?.ok_or("cliff needs \"level\" 0..15")? as u8 },
        "pathing" => {
            let names = a.get("flags").and_then(Value::as_array).ok_or("pathing needs \"flags\", e.g. [\"unwalkable\"]")?;
            let mut flags = 0u8;
            for n in names {
                flags |= match n.as_str() {
                    Some("unwalkable") => pathing::UNWALKABLE,
                    _ => return Err(format!("unknown pathing flag {n}; only \"unwalkable\" can be saved to .SC2Map files today")),
                };
            }
            if flags == 0 {
                return Err("pathing needs at least one flag".into());
            }
            BrushTool::Pathing { flags, on: a.bool("on")?.unwrap_or(true) }
        }
        other => return Err(format!("unknown brush tool \"{other}\"; use raise, lower, smooth, flatten, noise, paint, erase, cliff, pathing")),
    };
    let shape = a
        .choice(
            "shape",
            &[
                ("circle", BrushShape::Circle),
                ("square", BrushShape::Square),
            ],
        )?
        .unwrap_or_default();
    let falloff = a
        .choice(
            "falloff",
            &[
                ("constant", Falloff::Constant),
                ("linear", Falloff::Linear),
                ("smooth", Falloff::Smooth),
                ("sharp", Falloff::Sharp),
            ],
        )?
        .unwrap_or_default();
    Ok(Op::Brush {
        tool,
        points: pts,
        radius,
        strength,
        shape,
        falloff,
    })
}

fn region_shape(a: &Args) -> Result<Shape, String> {
    let given = ["rect", "circle", "polygon"]
        .iter()
        .filter(|k| a.has(k))
        .count();
    if given != 1 {
        return Err("give exactly one of rect [x0, y0, x1, y1], circle [cx, cy, r] or polygon [[x, y], ...]".into());
    }
    if let Some(r) = a.rect("rect")? {
        let (min, max) = (
            [r[0].min(r[2]), r[1].min(r[3])],
            [r[0].max(r[2]), r[1].max(r[3])],
        );
        return Ok(Shape::Rect {
            min: glam::Vec2::from(min),
            max: glam::Vec2::from(max),
        });
    }
    if let Some(c) = a.floats("circle", 3)? {
        return Ok(Shape::Circle {
            center: glam::Vec2::new(c[0], c[1]),
            radius: c[2],
        });
    }
    let p = a.points("polygon")?.unwrap_or_default();
    Ok(Shape::Polygon {
        points: p.into_iter().map(glam::Vec2::from).collect(),
    })
}

/// Trigger from either a `trigger` object or the top-level fields.
fn trigger_arg(
    a: &Args,
    default_name: Option<&str>,
    skip: &[&str],
) -> Result<strata_core::triggers::Trigger, String> {
    if let Some(t) = a.get("trigger") {
        let others: Vec<&String> =
            a.0.keys()
                .filter(|k| *k != "trigger" && !skip.contains(&k.as_str()))
                .collect();
        if !others.is_empty() {
            return Err(format!(
                "pass either `trigger` or the separate fields, not both (also got {others:?})"
            ));
        }
        return trigger_json::parse_trigger(t, default_name);
    }
    let fields: Map<String, Value> =
        a.0.iter()
            .filter(|(k, _)| !skip.contains(&k.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
    trigger_json::parse_trigger(&Value::Object(fields), default_name)
}

fn catalog_kind(a: &Args) -> Result<Option<CatalogKind>, String> {
    let opts: Vec<(String, CatalogKind)> = CatalogKind::ALL
        .iter()
        .map(|k| (format!("{k:?}").to_lowercase(), *k))
        .collect();
    let refs: Vec<(&str, CatalogKind)> = opts.iter().map(|(n, k)| (n.as_str(), *k)).collect();
    a.choice("kind", &refs)
}

// ---------------------------------------------------------------------------
// Dispatch

fn render(out: Outcome) -> String {
    let mut v = out.value;
    match v.as_object_mut() {
        Some(o) => {
            let mut m = Map::new();
            m.insert("target".into(), Value::String(out.target.as_str().into()));
            m.extend(std::mem::take(o));
            v = Value::Object(m);
        }
        None => v = json!({"target": out.target.as_str(), "result": v}),
    }
    serde_json::to_string_pretty(&v).unwrap_or_default()
}

/// Run a tool. Returns the text content for the MCP result.
pub fn call(s: &Strata, name: &str, args: &Value) -> Result<String, CallError> {
    let def = defs()
        .iter()
        .find(|d| d.name == name)
        .ok_or_else(|| CallError::UnknownTool(name.to_owned()))?;
    let empty = Map::new();
    let map = match args {
        Value::Object(m) => m,
        Value::Null => &empty,
        _ => return Err("arguments must be a JSON object".into()),
    };
    if let Some(props) = def.schema.get("properties").and_then(Value::as_object) {
        if let Some(k) = map.keys().find(|k| !props.contains_key(*k)) {
            let mut valid: Vec<&str> = props.keys().map(String::as_str).collect();
            valid.sort_unstable();
            return Err(CallError::Failed(format!(
                "unknown argument \"{k}\" for {name}; valid arguments: {}",
                if valid.is_empty() {
                    "none".to_owned()
                } else {
                    valid.join(", ")
                }
            )));
        }
    }
    let a = Args(map);
    let op = |op: Op| {
        s.run(name, LinkKind::Op(op))
            .map(render)
            .map_err(CallError::Failed)
    };
    let query = |q: Query| s.query(name, q).map(render).map_err(CallError::Failed);

    match name {
        "editor_status" => Ok(serde_json::to_string_pretty(&s.status()).unwrap_or_default()),
        "map_summary" => query(Query::Summary),
        "map_new" => {
            let w = a.int("width", 16, 256)?.unwrap_or(128) as u32;
            let h = a.int("height", 16, 256)?.unwrap_or(128) as u32;
            let mut doc = MapDocument::new(
                a.str("name")?.unwrap_or_else(|| "Untitled map".into()),
                w,
                h,
            );
            if let Some(t) = a.str("tileset")? {
                doc.info.tileset = t;
            }
            Ok(render(s.new_map(
                doc,
                a.bool("discard_changes")?.unwrap_or(false),
            )?))
        }
        "map_open" => Ok(render(s.open(
            &PathBuf::from(a.need_str("path")?),
            a.bool("discard_changes")?.unwrap_or(false),
        )?)),
        "map_save" => Ok(render(s.save(a.str("path")?.map(PathBuf::from))?)),

        "terrain_brush" => op(brush_op(&a)?),
        "terrain_stats" => query(Query::TerrainStats {
            rect: a.rect("rect")?,
        }),
        "sample_height" => query(Query::SampleHeight {
            points: a.points("points")?.ok_or("\"points\" is required")?,
        }),
        "add_texture_layer" => op(Op::AddTextureLayer {
            texture: a.need_str("texture")?,
            fill: a.bool("fill")?.unwrap_or(false),
        }),

        "place_object" => {
            let kind = a.choice("kind", &KINDS)?.unwrap_or(ObjectKind::Unit);
            op(Op::PlaceObject {
                kind,
                type_id: a.str("type_id")?.unwrap_or_default(),
                x: a.need_f32("x")?,
                y: a.need_f32("y")?,
                player: a.int("player", 0, 15)?.map(|p| p as u8),
                rotation: facing(&a, "facing")?.unwrap_or(0.0),
                name: a.str("name")?.unwrap_or_default(),
            })
        }
        "move_object" => op(Op::MoveObject {
            id: a.need_int("id", 1, u32::MAX as i64)? as u32,
            x: a.need_f32("x")?,
            y: a.need_f32("y")?,
        }),
        "set_object_property" => {
            let scale = match a.get("scale") {
                None => None,
                Some(v) if v.is_number() => {
                    let f = v.as_f64().unwrap_or(1.0) as f32;
                    Some([f, f, f])
                }
                Some(_) => a.floats("scale", 3)?.map(|v| [v[0], v[1], v[2]]),
            };
            op(Op::SetObjectProperty {
                id: a.need_int("id", 1, u32::MAX as i64)? as u32,
                player: a.int("player", 0, 15)?.map(|p| p as u8),
                rotation: facing(&a, "facing")?,
                scale,
                name: a.str("name")?,
                type_id: a.str("type_id")?,
            })
        }
        "delete_objects" => op(Op::DeleteObjects {
            ids: a.ids("ids")?.ok_or("\"ids\" is required")?,
        }),
        "list_objects" => query(Query::ListObjects {
            kind: a.choice("kind", &KINDS)?,
            type_id: a.str("type_id")?,
            player: a.int("player", 0, 15)?.map(|p| p as u8),
            rect: a.rect("rect")?,
            ids: a.ids("ids")?,
            search: a.str("search")?,
            limit: a.int("limit", 1, 5000)?.map(|l| l as usize),
        }),

        "create_region" => op(Op::CreateRegion {
            name: a.need_str("name")?,
            shape: region_shape(&a)?,
            color: a.color("color")?,
        }),
        "delete_region" => op(Op::DeleteRegion {
            name: a.need_str("name")?,
        }),
        "list_regions" => query(Query::ListRegions),
        "create_camera" => op(Op::CreateCamera {
            name: a.need_str("name")?,
            x: a.need_f32("x")?,
            y: a.need_f32("y")?,
            distance: a.f32("distance")?,
            pitch: a.f32("pitch")?,
            yaw: a.f32("yaw")?,
            fov: a.f32("fov")?,
        }),

        "create_trigger" => op(Op::CreateTrigger {
            trigger: trigger_arg(&a, None, &[])?,
        }),
        "update_trigger" => {
            let old = a.need_str("name")?;
            let mut t = trigger_arg(&a, Some(&old), &["name", "new_name"])?;
            if let Some(n) = a.str("new_name")? {
                t.name = n;
            } else if !a.has("trigger") {
                t.name = old.clone();
            }
            op(Op::UpdateTrigger {
                name: old,
                trigger: t,
            })
        }
        "delete_trigger" => op(Op::DeleteTrigger {
            name: a.need_str("name")?,
        }),
        "list_triggers" => query(Query::ListTriggers),
        "get_trigger" => query(Query::GetTrigger {
            name: a.need_str("name")?,
        }),
        "get_galaxy_script" => {
            let out = s.query(
                name,
                Query::GalaxyScript {
                    trigger: a.str("trigger")?,
                },
            )?;
            let script = out
                .value
                .get("script")
                .and_then(Value::as_str)
                .unwrap_or_default();
            Ok(format!("// target: {}\n{script}", out.target.as_str()))
        }
        "create_variable" => op(Op::CreateVariable {
            name: a.need_str("name")?,
            ty: trigger_json::parse_type(&a.need_str("type")?),
            array_len: a.int("array_len", 1, 65536)?.map(|n| n as u32),
            initial: a
                .get("initial")
                .map(trigger_json::parse_expr)
                .transpose()
                .map_err(|e| format!("initial: {e}"))?,
            comment: a.str("comment")?.unwrap_or_default(),
        }),
        "delete_variable" => op(Op::DeleteVariable {
            name: a.need_str("name")?,
        }),
        "set_custom_script" => op(Op::SetCustomScript {
            script: a.need_str("script")?,
            append: a.bool("append")?.unwrap_or(false),
        }),

        "set_string" => op(Op::SetString {
            key: a.need_str("key")?,
            value: a.need_str("value")?,
            locale: a
                .str("locale")?
                .unwrap_or_else(|| strata_core::strings::DEFAULT_LOCALE.into()),
        }),
        "get_strings" => query(Query::GetStrings {
            locale: a.str("locale")?,
            prefix: a.str("prefix")?,
            search: a.str("search")?,
            limit: a.int("limit", 1, 5000)?.map(|l| l as usize),
        }),
        "set_data_field" => op(Op::SetDataField {
            file: a.str("file")?.unwrap_or_default(),
            class: a.need_str("class")?,
            id: a.need_str("id")?,
            field: a.need_str("field")?,
            value: match a.get("value") {
                Some(Value::String(v)) => v.clone(),
                Some(v @ (Value::Number(_) | Value::Bool(_))) => v.to_string(),
                _ => return Err("\"value\" is required (text such as \"55\")".into()),
            },
            parent: a.str("parent")?,
        }),
        "list_data" => query(Query::ListData {
            kind: catalog_kind(&a)?,
            class: a.str("class")?,
            search: a.str("search")?,
            full: a.bool("full")?.unwrap_or(false),
            limit: a.int("limit", 1, 5000)?.map(|l| l as usize),
        }),

        "list_base_units" => {
            let race = a.str("race")?;
            let v = crate::base::list_units(
                a.str("search")?.as_deref(),
                race.as_deref(),
                a.bool("melee_only")?.unwrap_or(true),
                a.int("limit", 1, 5000)?.unwrap_or(200) as usize,
            )?;
            Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
        }
        "search_base_data" => {
            let v = crate::base::search(
                &a.need_str("query")?,
                catalog_kind(&a)?,
                a.int("limit", 1, 5000)?.unwrap_or(200) as usize,
            )?;
            Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
        }
        "get_base_data" => {
            let kind = catalog_kind(&a)?.ok_or("\"kind\" is required")?;
            let v = crate::base::get_entry(kind, &a.need_str("id")?, a.bool("full")?.unwrap_or(false))?;
            Ok(serde_json::to_string_pretty(&v).unwrap_or_default())
        }

        "set_map_info" => {
            let playable = match a.floats("playable", 4)? {
                Some(p) if p.iter().all(|v| *v >= 0.0 && v.fract() == 0.0) => {
                    Some([p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32])
                }
                Some(_) => {
                    return Err(
                        "\"playable\" must be 4 whole numbers [left, bottom, right, top]".into(),
                    )
                }
                None => None,
            };
            op(Op::SetMapInfo {
                name: a.str("name")?,
                description: a.str("description")?,
                author: a.str("author")?,
                playable,
            })
        }
        "set_player" => {
            let races = [
                ("terran", Race::Terran),
                ("protoss", Race::Protoss),
                ("zerg", Race::Zerg),
                ("random", Race::Random),
                ("neutral", Race::Neutral),
            ];
            let controls = [
                ("user", PlayerControl::User),
                ("computer", PlayerControl::Computer),
                ("neutral", PlayerControl::Neutral),
                ("hostile", PlayerControl::Hostile),
                ("open", PlayerControl::Open),
            ];
            let clear = a.0.get("start_location").is_some_and(Value::is_null);
            op(Op::SetPlayer {
                id: a.need_int("id", 1, 15)? as u8,
                name: a.str("name")?,
                race: a.choice("race", &races)?,
                control: a.choice("control", &controls)?,
                color: a.color("color")?,
                team: a.int("team", 1, 15)?.map(|t| t as u8),
                start_location: a.point("start_location")?,
                clear_start_location: clear,
            })
        }
        "validate_map" => query(Query::Validate),
        "undo" | "redo" => {
            let steps = a.int("steps", 1, 50)?.unwrap_or(1);
            let mut done = Vec::new();
            let mut target = s.target();
            for _ in 0..steps {
                let kind = if name == "undo" {
                    LinkKind::Undo
                } else {
                    LinkKind::Redo
                };
                match s.run(name, kind) {
                    Ok(out) => {
                        target = out.target;
                        if let Some(l) = out
                            .value
                            .as_object()
                            .and_then(|o| o.values().next())
                            .and_then(Value::as_str)
                        {
                            done.push(l.to_owned());
                        }
                    }
                    Err(e) if done.is_empty() => return Err(CallError::Failed(e)),
                    Err(_) => break,
                }
            }
            let key = if name == "undo" { "undid" } else { "redid" };
            Ok(render(Outcome {
                target,
                value: json!({ key: done }),
            }))
        }
        "history" => query(Query::History {
            limit: a.int("limit", 1, 500)?.map(|l| l as usize),
        }),
        _ => Err(CallError::UnknownTool(name.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_def_has_a_handler_and_valid_schema() {
        let s = Strata::headless_only(MapDocument::new("t", 32, 32));
        let mut names = std::collections::HashSet::new();
        for d in defs() {
            assert!(names.insert(d.name), "duplicate tool {}", d.name);
            assert_eq!(d.schema["type"], "object");
            assert!(d.schema["properties"].is_object());
            for r in d.schema["required"].as_array().unwrap() {
                assert!(
                    d.schema["properties"].get(r.as_str().unwrap()).is_some(),
                    "{}: required {r} missing",
                    d.name
                );
            }
            // Unknown tool names are the only way to get UnknownTool for a listed tool.
            let r = call(&s, d.name, &json!({"__nope": 1}));
            assert!(
                matches!(r, Err(CallError::Failed(ref e)) if e.contains("unknown argument")),
                "{}: {r:?}",
                d.name
            );
        }
        assert!(names.len() >= 35);
        assert_eq!(
            call(&s, "bogus", &json!({})),
            Err(CallError::UnknownTool("bogus".into()))
        );
    }

    #[test]
    fn base_data_tools() {
        let s = Strata::headless_only(MapDocument::new("t", 32, 32));
        let mut b = strata_core::base_data::BaseCatalog::builder("0.0.0.1");
        b.begin_mod("Liberty");
        b.add_catalog(
            "UnitData.xml",
            br#"<Catalog>
                <CUnit default="1"><Radius value="0.5"/></CUnit>
                <CUnit id="SiegeTank">
                    <Race value="Terr"/><LifeMax value="175"/>
                    <EditorCategories value="ObjectType:Unit,ObjectFamily:Melee"/>
                </CUnit>
                <CUnit id="Raynor" parent="SiegeTank">
                    <EditorCategories value="ObjectType:Hero,ObjectFamily:Campaign"/>
                </CUnit>
            </Catalog>"#,
        )
        .unwrap();
        b.add_strings(b"Unit/Name/SiegeTank=Siege Tank\n");
        crate::base::set(b.finish());

        let units: Value =
            serde_json::from_str(&call(&s, "list_base_units", &json!({"search": "siege"})).unwrap()).unwrap();
        assert_eq!(units["units"][0]["name"], "Siege Tank");
        assert_eq!(units["units"][0]["race"], "terran");
        assert_eq!(units["total"], 1);
        let all: Value = serde_json::from_str(
            &call(&s, "list_base_units", &json!({"melee_only": false, "limit": 1})).unwrap(),
        )
        .unwrap();
        assert_eq!(all["total"], 2);
        assert_eq!(all["units"].as_array().map(Vec::len), Some(1));
        let hits: Value =
            serde_json::from_str(&call(&s, "search_base_data", &json!({"query": "tank", "kind": "unit"})).unwrap())
                .unwrap();
        assert_eq!(hits["results"][0]["id"], "SiegeTank");
        let raynor: Value =
            serde_json::from_str(&call(&s, "get_base_data", &json!({"kind": "unit", "id": "Raynor"})).unwrap()).unwrap();
        assert_eq!(raynor["fields"]["LifeMax"], "175");
        assert_eq!(raynor["fields"]["Radius"], "0.5");
        assert_eq!(raynor["inherits_from"][0], "SiegeTank");
        assert!(call(&s, "get_base_data", &json!({"kind": "unit", "id": "Nope"})).is_err());
        let status = s.status();
        assert_eq!(status["base_data"]["units"], 2);
    }

    #[test]
    fn densify_joins_points() {
        let d = densify(&[[0.0, 0.0], [6.0, 0.0]], 6.0);
        assert_eq!(d.first(), Some(&[0.0, 0.0]));
        assert_eq!(d.last(), Some(&[6.0, 0.0]));
        assert!(d.len() >= 4);
    }

    #[test]
    fn tools_edit_headless_map() {
        let s = Strata::headless_only(MapDocument::new("t", 64, 64));
        let c = |n: &str, a: Value| call(&s, n, &a);
        let out: Value = serde_json::from_str(
            &c(
                "place_object",
                json!({"type_id": "Marine", "x": 10, "y": 10, "facing": 90}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(out["target"], "headless");
        let id = out["result"]["id"].as_u64().unwrap();
        c(
            "set_object_property",
            json!({"id": id, "scale": 2, "player": 2}),
        )
        .unwrap();
        let list: Value =
            serde_json::from_str(&c("list_objects", json!({"player": 2})).unwrap()).unwrap();
        assert_eq!(list["objects"][0]["facing_degrees"], 90.0);
        assert_eq!(list["objects"][0]["scale"], json!([2.0, 2.0, 2.0]));

        c("terrain_brush", json!({"tool": "raise", "points": [[20, 20], [40, 20]], "radius": 5, "strength": 1, "passes": 2})).unwrap();
        c("terrain_brush", json!({"tool": "pathing", "x": 5, "y": 5, "flags": ["unwalkable"], "falloff": "constant"})).unwrap();
        let e = c("terrain_brush", json!({"tool": "flatten", "x": 5, "y": 5})).unwrap_err();
        assert!(matches!(e, CallError::Failed(ref m) if m.contains("height")));
        let layer: Value =
            serde_json::from_str(&c("add_texture_layer", json!({"texture": "CharGrass"})).unwrap())
                .unwrap();
        assert_eq!(layer["result"]["layer"], 1);
        c(
            "terrain_brush",
            json!({"tool": "paint", "layer": 1, "x": 30, "y": 30}),
        )
        .unwrap();

        c(
            "create_region",
            json!({"name": "Arena", "rect": [30, 30, 10, 10], "color": "#ff8800"}),
        )
        .unwrap();
        assert!(c(
            "create_region",
            json!({"name": "B", "rect": [1, 1, 2, 2], "circle": [1, 1, 1]})
        )
        .is_err());
        c(
            "create_region",
            json!({"name": "Ring", "circle": [32, 32, 5]}),
        )
        .unwrap();
        c("create_camera", json!({"name": "Intro", "x": 32, "y": 32})).unwrap();
        let regions: Value = serde_json::from_str(&c("list_regions", json!({})).unwrap()).unwrap();
        assert_eq!(
            regions["regions"][0]["shapes"][0]["rect"],
            json!([10.0, 10.0, 30.0, 30.0])
        );
        assert_eq!(regions["regions"][0]["color"], json!([255, 136, 0]));
        assert_eq!(regions["cameras"][0]["name"], "Intro");

        c(
            "create_variable",
            json!({"name": "Wave", "type": "int", "initial": 1}),
        )
        .unwrap();
        c("create_trigger", json!({
            "name": "Waves",
            "events": [{"function": "TriggerAddEventTimePeriodic", "args": [30.0, "c_timeGame"]}],
            "actions": [{"set": "Wave", "value": {"op": "+", "lhs": {"var": "Wave"}, "rhs": 1}}]
        }))
        .unwrap();
        c(
            "update_trigger",
            json!({"name": "Waves", "new_name": "Wave timer", "actions": ["return"]}),
        )
        .unwrap();
        let script = c("get_galaxy_script", json!({})).unwrap();
        assert!(script.starts_with("// target: headless"));
        assert!(
            script.contains("int gv_wave;")
                && script.contains("gv_wave = 1;")
                && script.contains("gt_Wavetimer"),
            "{script}"
        );
        let t: Value =
            serde_json::from_str(&c("get_trigger", json!({"name": "Wave timer"})).unwrap())
                .unwrap();
        // The model form round-trips through update_trigger.
        c(
            "update_trigger",
            json!({"name": "Wave timer", "trigger": t["trigger"]}),
        )
        .unwrap();
        c("set_custom_script", json!({"script": "void Helper() {}"})).unwrap();
        c("delete_trigger", json!({"name": "Wave timer"})).unwrap();

        c("set_string", json!({"key": "Msg/Hi", "value": "Hello"})).unwrap();
        let strings: Value =
            serde_json::from_str(&c("get_strings", json!({"prefix": "Msg/"})).unwrap()).unwrap();
        assert_eq!(strings["strings"]["Msg/Hi"], "Hello");
        c(
            "set_data_field",
            json!({"class": "CUnit", "id": "Marine", "field": "LifeMax", "value": 55}),
        )
        .unwrap();
        let data: Value =
            serde_json::from_str(&c("list_data", json!({"kind": "unit"})).unwrap()).unwrap();
        assert_eq!(data["entries"][0]["fields"]["LifeMax"], "55");
        c(
            "set_map_info",
            json!({"name": "Arena", "playable": [4, 4, 60, 60]}),
        )
        .unwrap();
        c(
            "set_player",
            json!({"id": 3, "race": "zerg", "control": "computer", "start_location": [50, 50]}),
        )
        .unwrap();
        c("set_player", json!({"id": 3, "start_location": null})).unwrap();
        let summary: Value = serde_json::from_str(&c("map_summary", json!({})).unwrap()).unwrap();
        assert_eq!(summary["name"], "Arena");
        assert_eq!(summary["players"][2]["race"], "Zerg");
        assert!(summary["players"][2]["start_location"].is_null());

        let undo: Value = serde_json::from_str(&c("undo", json!({"steps": 2})).unwrap()).unwrap();
        assert_eq!(undo["undid"], json!(["Edit player 3", "Add player 3"]));
        c("redo", json!({})).unwrap();
        let hist: Value =
            serde_json::from_str(&c("history", json!({"limit": 1})).unwrap()).unwrap();
        assert_eq!(hist["undo"][0]["label"], "Add player 3");
        assert_eq!(hist["redo"][0]["label"], "Edit player 3");
        c("delete_objects", json!({"ids": [id]})).unwrap();
        let v: Value = serde_json::from_str(&c("validate_map", json!({})).unwrap()).unwrap();
        assert!(v["problems"].is_array());
        let st: Value =
            serde_json::from_str(&c("terrain_stats", json!({"rect": [0, 0, 10, 10]})).unwrap())
                .unwrap();
        assert!(st["pathing"]["unwalkable_percent"].as_f64().unwrap() > 0.0);
    }

    #[test]
    fn map_files_roundtrip_json() {
        let s = Strata::headless_only(MapDocument::new("t", 32, 32));
        let dir =
            std::env::temp_dir().join(format!("strata-mcp-files-{}", crate::token::generate()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("m.strata.json");
        call(
            &s,
            "place_object",
            &json!({"type_id": "SCV", "x": 3, "y": 3}),
        )
        .unwrap();
        // Unsaved changes block map_new.
        let e = call(&s, "map_new", &json!({})).unwrap_err();
        assert!(matches!(e, CallError::Failed(ref m) if m.contains("unsaved")));
        assert!(call(&s, "map_save", &json!({})).is_err());
        assert!(call(
            &s,
            "map_save",
            &json!({"path": dir.join("m.txt").to_string_lossy()})
        )
        .is_err());
        call(&s, "map_save", &json!({"path": path.to_string_lossy()})).unwrap();
        call(
            &s,
            "map_new",
            &json!({"name": "Fresh", "width": 64, "height": 48}),
        )
        .unwrap();
        let summary: Value =
            serde_json::from_str(&call(&s, "map_summary", &json!({})).unwrap()).unwrap();
        assert_eq!(summary["size"], json!([64, 48]));
        call(&s, "map_open", &json!({"path": path.to_string_lossy()})).unwrap();
        let list: Value =
            serde_json::from_str(&call(&s, "list_objects", &json!({})).unwrap()).unwrap();
        assert_eq!(list["objects"][0]["type_id"], "SCV");
        let e = call(
            &s,
            "map_open",
            &json!({"path": dir.join("missing.SC2Map").to_string_lossy()}),
        )
        .unwrap_err();
        assert!(matches!(e, CallError::Failed(ref m) if m.contains("does not exist")));
        let _ = std::fs::remove_dir_all(dir);
    }
}
