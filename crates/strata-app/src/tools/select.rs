//! Select mode: click / Shift-click / box select, drag to move, Delete,
//! Ctrl+D duplicate, R / Shift+R rotate.

use glam::{Vec2, Vec3};
use strata_core::history::Edit;
use strata_core::objects::{ObjectId, ObjectKind, PlacedObject};
use strata_core::regions::Region;

use super::{ToolInput, ToolState};
use crate::render::camera::View;
use crate::state::EditorState;

pub(crate) enum Drag {
    /// Rubber-band selection from a viewport-local point.
    Box { start: Vec2, additive: bool },
    /// Moving the selected objects (and/or the selected region).
    Move { start: Vec2, objects: Vec<PlacedObject>, regions: Option<Vec<Region>>, moved: bool },
    /// Drawing a new region (Regions mode).
    Region { start: Vec2, circle: bool },
}

/// Approximate footprint radius (world units) of a known unit type: small
/// infantry about half a cell, vehicles and large air units a cell or more,
/// structures half their footprint.
pub fn unit_radius(type_id: &str) -> Option<f32> {
    let r = match type_id {
        "SCV" | "MULE" | "Marine" | "Reaper" | "Ghost" | "Probe" | "Zealot" | "Sentry" | "Adept" | "HighTemplar"
        | "DarkTemplar" | "Observer" | "Larva" | "Drone" | "Zergling" | "Baneling" | "Hydralisk" | "Infestor"
        | "Queen" | "CreepTumorBurrowed" | "WidowMine" => 0.5,
        "Marauder" | "Hellion" | "HellionTank" | "Cyclone" | "Stalker" | "Roach" | "Ravager" | "Immortal"
        | "LurkerMP" | "SwarmHostMP" | "VikingFighter" | "VikingAssault" | "Medivac" | "Banshee" | "Raven"
        | "Phoenix" | "Oracle" | "WarpPrism" | "Disruptor" | "Mutalisk" | "Corruptor" | "Viper" | "Overlord"
        | "Overseer" => 0.75,
        "SiegeTank" | "SiegeTankSieged" | "Archon" | "Colossus" | "Ultralisk" | "Liberator" | "VoidRay"
        | "BroodLord" => 1.0,
        "Thor" | "Battlecruiser" | "Carrier" | "Tempest" => 1.25,
        "Mothership" => 1.5,
        // 5x5 town halls.
        "CommandCenter" | "OrbitalCommand" | "PlanetaryFortress" | "Nexus" | "Hatchery" | "Lair" | "Hive" => 2.5,
        // 2x2 structures.
        "SupplyDepot" | "SupplyDepotLowered" | "MissileTurret" | "SensorTower" | "Pylon" | "PhotonCannon"
        | "ShieldBattery" | "DarkShrine" | "SpineCrawler" | "SporeCrawler" | "BarracksTechLab" | "BarracksReactor"
        | "FactoryTechLab" | "FactoryReactor" | "StarportTechLab" | "StarportReactor" | "XelNagaTower" => 1.0,
        // 3x3 structures and geysers.
        "Barracks" | "EngineeringBay" | "Bunker" | "Factory" | "GhostAcademy" | "Starport" | "Armory"
        | "FusionCore" | "Refinery" | "Gateway" | "WarpGate" | "Forge" | "CyberneticsCore" | "TwilightCouncil"
        | "RoboticsFacility" | "Stargate" | "TemplarArchive" | "RoboticsBay" | "FleetBeacon" | "Assimilator"
        | "Extractor" | "SpawningPool" | "EvolutionChamber" | "RoachWarren" | "BanelingNest" | "HydraliskDen"
        | "LurkerDenMP" | "InfestationPit" | "Spire" | "GreaterSpire" | "NydusNetwork" | "UltraliskCavern" => 1.5,
        id if id.contains("MineralField") => 1.0,
        id if id.contains("Geyser") => 1.5,
        id if id.starts_with("Unbuildable") => 1.0,
        id if id.starts_with("Destructible") => {
            if id.contains("Huge") {
                3.0
            } else if id.contains("6x6") {
                2.5
            } else {
                2.0
            }
        }
        _ => return None,
    };
    Some(r)
}

/// World radius used for picking and drawing an object.
pub fn object_world_radius(o: &PlacedObject) -> f32 {
    let s = o.scale.x.max(0.25);
    match o.kind {
        ObjectKind::Unit => unit_radius(&o.type_id).unwrap_or(0.75) * s,
        ObjectKind::Doodad => 0.75 * s,
        ObjectKind::Point => 0.5,
    }
}

/// Smallest on-screen radius (points) per kind, so markers stay readable
/// when zoomed out.
pub fn min_screen_radius(kind: ObjectKind) -> f32 {
    match kind {
        ObjectKind::Unit => 6.0,
        ObjectKind::Doodad => 5.5,
        ObjectKind::Point => 6.0,
    }
}

/// Screen radius (points) of an object at its depth.
pub fn object_screen_radius(o: &PlacedObject, view: &View) -> f32 {
    (object_world_radius(o) * view.points_per_unit(o.position)).clamp(min_screen_radius(o.kind), 160.0)
}

/// Topmost object under a viewport-local point.
pub fn pick_object(state: &EditorState, view: &View, p: Vec2) -> Option<ObjectId> {
    let mut best: Option<(f32, ObjectId)> = None;
    for o in &state.doc.objects.objects {
        let Some(s) = view.project(o.position) else { continue };
        let r = object_screen_radius(o, view).max(7.0);
        let d = s.distance(p);
        if d <= r && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, o.id));
        }
    }
    best.map(|(_, id)| id)
}

/// Camera glyph under a point (the target marker).
pub fn pick_camera(state: &EditorState, view: &View, p: Vec2) -> Option<ObjectId> {
    let t = &state.doc.terrain;
    state
        .doc
        .cameras
        .iter()
        .filter_map(|c| {
            let s = view.project(Vec3::new(c.target.x, c.target.y, t.sample_height(c.target.x, c.target.y)))?;
            let d = s.distance(p);
            (d <= 9.0).then_some((d, c.id))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// Smallest region containing a world point.
pub fn pick_region(state: &EditorState, w: Vec2) -> Option<ObjectId> {
    state.doc.regions.iter().filter(|r| r.contains(w)).min_by(|a, b| area(a).total_cmp(&area(b))).map(|r| r.id)
}

fn area(r: &Region) -> f32 {
    r.shapes
        .iter()
        .map(|s| {
            let (lo, hi) = s.bounds();
            (hi - lo).x * (hi - lo).y
        })
        .sum()
}

impl ToolState {
    pub(crate) fn select_tool(&mut self, ctx: &egui::Context, state: &mut EditorState, input: &ToolInput) {
        let shift = input.mods.shift;
        if input.pressed && self.drag.is_none() {
            if let Some(p) = input.pointer {
                let world = input.hover.map(|h| h.truncate());
                if let Some(id) = pick_object(state, &input.view, p) {
                    let sel = &mut state.selection;
                    if shift {
                        if !sel.objects.remove(&id) {
                            sel.objects.insert(id);
                        }
                    } else if !sel.objects.contains(&id) {
                        sel.objects.clear();
                        sel.objects.insert(id);
                        sel.region = None;
                        sel.camera = None;
                    }
                    if let (true, Some(w)) = (state.selection.objects.contains(&id), world) {
                        let objects = selected_objects(state);
                        self.drag = Some(Drag::Move { start: w, objects, regions: None, moved: false });
                    }
                } else if let Some(cam) = pick_camera(state, &input.view, p) {
                    if !shift {
                        state.selection.objects.clear();
                        state.selection.region = None;
                    }
                    state.selection.camera = Some(cam);
                } else if let Some(rid) = world.and_then(|w| pick_region(state, w)).filter(|_| !shift) {
                    state.selection.objects.clear();
                    state.selection.camera = None;
                    state.selection.region = Some(rid);
                    if let Some(w) = world {
                        self.drag = Some(Drag::Move {
                            start: w,
                            objects: Vec::new(),
                            regions: Some(state.doc.regions.clone()),
                            moved: false,
                        });
                    }
                } else {
                    self.drag = Some(Drag::Box { start: p, additive: shift });
                }
            }
        }

        match self.drag.as_mut() {
            Some(Drag::Move { start, objects, regions, moved }) => {
                if let Some(h) = input.hover {
                    let raw = h.truncate() - *start;
                    let delta = if self.settings.snap {
                        (raw / self.settings.snap_step).round() * self.settings.snap_step
                    } else {
                        raw
                    };
                    if delta != Vec2::ZERO || *moved {
                        *moved = true;
                        let t = &state.doc.terrain;
                        let size = state.doc.world_size();
                        for b in objects.iter() {
                            if let Some(o) = state.doc.objects.get_mut(b.id) {
                                let xy = (b.position.truncate() + delta).clamp(Vec2::ZERO, size);
                                o.position = Vec3::new(xy.x, xy.y, t.sample_height(xy.x, xy.y));
                            }
                        }
                        if let (Some(before), Some(rid)) = (regions.as_ref(), state.selection.region) {
                            if let (Some(src), Some(dst)) = (
                                before.iter().find(|r| r.id == rid),
                                state.doc.regions.iter_mut().find(|r| r.id == rid),
                            ) {
                                dst.shapes = src.shapes.clone();
                                dst.shapes.iter_mut().for_each(|s| s.translate(delta));
                            }
                        }
                        if !objects.is_empty() {
                            state.doc.dirty.objects = true;
                        }
                    }
                }
                if !input.down {
                    if let Some(Drag::Move { objects, regions, moved, .. }) = self.drag.take() {
                        if moved {
                            commit_move(state, objects, regions);
                        }
                    }
                }
            }
            Some(Drag::Box { start, additive }) if !input.down => {
                let (start, additive) = (*start, *additive);
                self.drag = None;
                let end = input.pointer.unwrap_or(start);
                if !additive {
                    state.selection.objects.clear();
                    state.selection.region = None;
                    state.selection.camera = None;
                }
                // A click without a real drag only clears the selection.
                if start.distance(end) >= 4.0 {
                    let lo = start.min(end);
                    let hi = start.max(end);
                    for o in &state.doc.objects.objects {
                        if let Some(s) = input.view.project(o.position) {
                            if s.x >= lo.x && s.y >= lo.y && s.x <= hi.x && s.y <= hi.y {
                                state.selection.objects.insert(o.id);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        if !input.keys {
            return;
        }
        use egui::{Key, Modifiers};
        let (delete, dup, all) = ctx.input_mut(|i| {
            let delete = i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace);
            let dup = i.consume_key(Modifiers::COMMAND, Key::D);
            let all = i.consume_key(Modifiers::COMMAND, Key::A);
            (delete, dup, all)
        });
        if delete && self.drag.is_none() {
            delete_selection(state);
        }
        if dup && self.drag.is_none() {
            duplicate_selection(state);
        }
        if all {
            state.selection.objects = state.doc.objects.objects.iter().map(|o| o.id).collect();
        }
    }

    pub(crate) fn cancel_drag(&mut self, state: &mut EditorState) {
        if let Some(Drag::Move { objects, regions, .. }) = self.drag.take() {
            for b in objects {
                if let Some(o) = state.doc.objects.get_mut(b.id) {
                    *o = b;
                }
            }
            if let Some(r) = regions {
                state.doc.regions = r;
            }
        }
    }

    /// R / Shift+R rotate the selection (Select) or the placement ghost.
    pub(crate) fn rotate_keys(&mut self, ctx: &egui::Context, state: &mut EditorState) {
        let (cw, ccw) = ctx.input(|i| {
            let r = i.key_pressed(egui::Key::R) && !i.modifiers.command && !i.modifiers.alt;
            (r && i.modifiers.shift, r && !i.modifiers.shift)
        });
        if !(cw || ccw) {
            return;
        }
        let step = 15f32.to_radians() * if cw { -1.0 } else { 1.0 };
        if state.mode == crate::state::Mode::Select {
            if self.drag.is_some() || state.selection.objects.is_empty() {
                return;
            }
            let before = selected_objects(state);
            let edits: Vec<Edit> = before
                .into_iter()
                .map(|b| {
                    let mut a = b.clone();
                    a.rotation = (a.rotation + step).rem_euclid(std::f32::consts::TAU);
                    Edit::Object { id: b.id, before: Some(b), after: Some(a) }
                })
                .collect();
            let n = edits.len();
            state.commit(format!("Rotate {}", plural(n, "object")), Edit::Batch(edits));
        } else {
            self.settings.ghost_rotation = (self.settings.ghost_rotation + step).rem_euclid(std::f32::consts::TAU);
        }
    }

    pub(crate) fn paint_select(&self, painter: &egui::Painter, input: &ToolInput) {
        if let Some(Drag::Box { start, .. }) = &self.drag {
            let Some(end) = input.pointer else { return };
            let tk = crate::render::overlay::MAP;
            let r = egui::Rect::from_two_pos(input.to_screen(*start), input.to_screen(end));
            painter.rect_filled(r, 0.0, tk.accent.gamma_multiply(0.12));
            painter.rect_stroke(r, 0.0, egui::Stroke::new(1.0, tk.accent), egui::StrokeKind::Inside);
        }
    }
}

pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

pub fn selected_objects(state: &EditorState) -> Vec<PlacedObject> {
    state.selection.objects.iter().filter_map(|id| state.doc.objects.get(*id).cloned()).collect()
}

/// Record a move whose result is already applied to the document.
fn commit_move(state: &mut EditorState, before: Vec<PlacedObject>, regions: Option<Vec<Region>>) {
    let mut edits: Vec<Edit> = before
        .into_iter()
        .filter_map(|b| {
            let after = state.doc.objects.get(b.id)?.clone();
            (after != b).then_some(Edit::Object { id: b.id, before: Some(b), after: Some(after) })
        })
        .collect();
    let n = edits.len();
    let mut label = format!("Move {}", plural(n, "object"));
    if let Some(r) = regions {
        if r != state.doc.regions {
            edits.push(Edit::Regions { before: r, after: state.doc.regions.clone() });
            label = if n == 0 { "Move region".into() } else { format!("{label} and a region") };
        }
    }
    for e in &edits {
        state.doc.dirty.merge(e.dirty());
    }
    match edits.len() {
        0 => {}
        1 => state.push_edit(label, edits.pop().expect("one edit")),
        _ => state.push_edit(label, Edit::Batch(edits)),
    }
}

/// Delete selected objects, region and camera as one undoable step.
pub fn delete_selection(state: &mut EditorState) {
    let mut edits: Vec<Edit> =
        selected_objects(state).into_iter().map(|o| Edit::Object { id: o.id, before: Some(o), after: None }).collect();
    let n = edits.len();
    let mut what = Vec::new();
    if n > 0 {
        what.push(plural(n, "object"));
    }
    if let Some(rid) = state.selection.region {
        let before = state.doc.regions.clone();
        let after: Vec<Region> = before.iter().filter(|r| r.id != rid).cloned().collect();
        if after.len() != before.len() {
            edits.push(Edit::Regions { before, after });
            what.push("region".into());
        }
    }
    if let Some(cid) = state.selection.camera {
        let before = state.doc.cameras.clone();
        let after: Vec<_> = before.iter().filter(|c| c.id != cid).cloned().collect();
        if after.len() != before.len() {
            edits.push(Edit::Cameras { before, after });
            what.push("camera".into());
        }
    }
    if edits.is_empty() {
        return;
    }
    state.commit(format!("Delete {}", what.join(", ")), Edit::Batch(edits));
    state.selection.objects.clear();
    state.selection.region = None;
    state.selection.camera = None;
}

/// Copy selected objects one cell to the north-east and select the copies.
pub fn duplicate_selection(state: &mut EditorState) {
    let src = selected_objects(state);
    if src.is_empty() {
        return;
    }
    let size = state.doc.world_size();
    let mut edits = Vec::new();
    let mut ids = Vec::new();
    for o in src {
        let id = state.doc.objects.alloc_id();
        let mut c = o.clone();
        c.id = id;
        let xy = (o.position.truncate() + Vec2::new(1.0, 1.0)).min(size);
        c.position = Vec3::new(xy.x, xy.y, state.doc.terrain.sample_height(xy.x, xy.y));
        edits.push(Edit::Object { id, before: None, after: Some(c) });
        ids.push(id);
    }
    let n = ids.len();
    state.commit(format!("Duplicate {}", plural(n, "object")), Edit::Batch(edits));
    state.selection.objects = ids.into_iter().collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::MapDocument;

    fn state_with_units(n: usize) -> EditorState {
        let mut s = EditorState::new(MapDocument::new("t", 32, 32));
        for i in 0..n {
            let id = s.doc.objects.alloc_id();
            let o = PlacedObject::new(id, ObjectKind::Unit, "Marine", Vec3::new(4.0 + i as f32 * 3.0, 4.0, 10.0));
            s.commit("Place", Edit::Object { id, before: None, after: Some(o) });
        }
        s
    }

    #[test]
    fn delete_and_undo() {
        let mut s = state_with_units(3);
        s.selection.objects = s.doc.objects.objects.iter().take(2).map(|o| o.id).collect();
        delete_selection(&mut s);
        assert_eq!(s.doc.objects.objects.len(), 1);
        s.undo();
        assert_eq!(s.doc.objects.objects.len(), 3);
    }

    #[test]
    fn duplicate_selects_copies() {
        let mut s = state_with_units(2);
        s.selection.objects = s.doc.objects.objects.iter().map(|o| o.id).collect();
        let before: Vec<_> = s.selection.objects.iter().copied().collect();
        duplicate_selection(&mut s);
        assert_eq!(s.doc.objects.objects.len(), 4);
        assert!(s.selection.objects.iter().all(|id| !before.contains(id)));
        s.undo();
        assert_eq!(s.doc.objects.objects.len(), 2);
    }

    #[test]
    fn move_commit_is_one_undo_step() {
        let mut s = state_with_units(2);
        let before = s.doc.objects.objects.clone();
        for o in &mut s.doc.objects.objects {
            o.position.x += 2.0;
        }
        commit_move(&mut s, before.clone(), None);
        s.undo();
        assert_eq!(s.doc.objects.objects, before);
    }

    #[test]
    fn unit_sizes_follow_type() {
        assert_eq!(unit_radius("Marine"), Some(0.5));
        assert!(unit_radius("CommandCenter").unwrap() > unit_radius("Thor").unwrap());
        assert_eq!(unit_radius("RichMineralField750"), Some(1.0));
        assert_eq!(unit_radius("SomeCustomUnit"), None);
        let o = PlacedObject::new(ObjectId(1), ObjectKind::Unit, "SomeCustomUnit", Vec3::ZERO);
        assert_eq!(object_world_radius(&o), 0.75);
    }

    #[test]
    fn markers_keep_a_minimum_screen_size() {
        let o = PlacedObject::new(ObjectId(1), ObjectKind::Unit, "Marine", Vec3::new(64.0, 64.0, 8.0));
        let far = crate::render::camera::Pose {
            target: Vec3::new(64.0, 64.0, 8.0),
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 0.9,
            distance: 400.0,
        };
        let v = View::new(&far, false, Vec2::new(800.0, 600.0));
        assert_eq!(object_screen_radius(&o, &v), 6.0, "12px across when zoomed out");
        let near = crate::render::camera::Pose { distance: 10.0, ..far };
        let v = View::new(&near, false, Vec2::new(800.0, 600.0));
        assert!(object_screen_radius(&o, &v) > 14.0, "grows with zoom");
    }

    #[test]
    fn picks_nearest_on_screen() {
        let s = state_with_units(2);
        let pose = crate::render::camera::Pose {
            target: Vec3::new(5.5, 4.0, 10.0),
            yaw: std::f32::consts::FRAC_PI_2,
            pitch: 1.2,
            distance: 20.0,
        };
        let v = View::new(&pose, false, Vec2::new(800.0, 600.0));
        let p = v.project(s.doc.objects.objects[1].position).unwrap();
        assert_eq!(pick_object(&s, &v, p + Vec2::new(1.0, 0.0)), Some(s.doc.objects.objects[1].id));
        assert_eq!(pick_object(&s, &v, Vec2::new(5.0, 5.0)), None);
    }
}
