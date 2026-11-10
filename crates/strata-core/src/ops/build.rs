//! Larger map-building ops: placing many objects at once, scattering
//! doodads, mirroring a map for symmetric melee play and cutting ramps.
//! Each one is a single undoable step.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{check_finite, check_in_map, check_player, invalid, Applied, OpError};
use crate::document::MapDocument;
use crate::history::Edit;
use crate::objects::{ObjectKind, PlacedObject};
use crate::terrain::{pathing, ChannelKind, VertexRect, CLIFF_LEVEL_HEIGHT};

/// One object for [`super::Op::PlaceObjects`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObjectSpec {
    /// Default unit.
    #[serde(default)]
    pub kind: Option<ObjectKind>,
    #[serde(default)]
    pub type_id: String,
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub player: Option<u8>,
    /// Radians, counter-clockwise from +X.
    #[serde(default)]
    pub rotation: f32,
    /// Uniform scale (default 1).
    #[serde(default)]
    pub scale: Option<f32>,
    #[serde(default)]
    pub name: String,
}

/// A new object at `(x, y)` on the terrain, validated like `place_object`.
/// The id is allocated only when everything is valid.
pub(super) fn new_object(doc: &mut MapDocument, s: ObjectSpec) -> Result<PlacedObject, OpError> {
    let kind = s.kind.unwrap_or(ObjectKind::Unit);
    if kind != ObjectKind::Point && s.type_id.trim().is_empty() {
        return invalid("type_id is required for units and doodads, e.g. \"Marine\" or \"CharRock01\"");
    }
    check_in_map(doc, s.x, s.y)?;
    check_finite("rotation", s.rotation)?;
    if let Some(p) = s.player {
        check_player(p)?;
    }
    if let Some(sc) = s.scale {
        if !(0.01..=100.0).contains(&sc) {
            return invalid(format!("scale must be 0.01..=100, got {sc}"));
        }
    }
    let id = doc.objects.alloc_id();
    let z = doc.terrain.sample_height(s.x, s.y);
    let mut o = PlacedObject::new(id, kind, s.type_id, glam::Vec3::new(s.x, s.y, z));
    o.rotation = s.rotation;
    o.name = if kind == ObjectKind::Point && s.name.trim().is_empty() { format!("Point {}", id.0) } else { s.name };
    if let Some(p) = s.player {
        o.player = p;
    }
    if let Some(sc) = s.scale {
        o.scale = glam::Vec3::splat(sc);
    }
    Ok(o)
}

/// Insert objects as one batch edit.
fn insert_all(doc: &mut MapDocument, objects: Vec<PlacedObject>) -> Edit {
    let edit = Edit::Batch(objects.into_iter().map(|o| Edit::Object { id: o.id, before: None, after: Some(o) }).collect());
    edit.apply(doc, true);
    edit
}

pub(super) fn place_objects(doc: &mut MapDocument, specs: Vec<ObjectSpec>) -> Result<Applied, OpError> {
    if specs.is_empty() {
        return invalid("objects must list at least one object");
    }
    if specs.len() > 5000 {
        return invalid(format!("at most 5000 objects per call, got {}", specs.len()));
    }
    let next = doc.objects.next_id;
    let mut objects = Vec::with_capacity(specs.len());
    for (i, s) in specs.into_iter().enumerate() {
        match new_object(doc, s) {
            Ok(o) => objects.push(o),
            Err(e) => {
                doc.objects.next_id = next;
                return invalid(format!("objects[{i}]: {e}; nothing was placed"));
            }
        }
    }
    let ids: Vec<u32> = objects.iter().map(|o| o.id.0).collect();
    let label = format!("Place {} objects", ids.len());
    let edit = insert_all(doc, objects);
    Ok(Applied { label, edit, result: json!({"ids": ids}) })
}

// ---------------------------------------------------------------------------
// Scatter

/// Small deterministic generator (SplitMix64), so a seed reproduces a scatter.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(seed)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in [0, 1).
    pub(crate) fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// Where to scatter: a region by name, or a rectangle.
pub(super) enum Area<'a> {
    Region(&'a crate::regions::Region),
    Rect([f32; 4]),
}

impl Area<'_> {
    fn bounds(&self) -> (glam::Vec2, glam::Vec2) {
        match self {
            Area::Region(r) => {
                let mut lo = glam::Vec2::splat(f32::MAX);
                let mut hi = glam::Vec2::splat(f32::MIN);
                for s in &r.shapes {
                    let (a, b) = s.bounds();
                    lo = lo.min(a);
                    hi = hi.max(b);
                }
                (lo, hi)
            }
            Area::Rect(r) => (glam::Vec2::new(r[0].min(r[2]), r[1].min(r[3])), glam::Vec2::new(r[0].max(r[2]), r[1].max(r[3]))),
        }
    }
    fn contains(&self, p: glam::Vec2) -> bool {
        match self {
            Area::Region(r) => r.contains(p),
            Area::Rect(_) => {
                let (lo, hi) = self.bounds();
                p.cmpge(lo).all() && p.cmple(hi).all()
            }
        }
    }
}

pub(super) struct ScatterArgs {
    pub kind: ObjectKind,
    pub type_ids: Vec<String>,
    pub count: Option<u32>,
    pub density: Option<f32>,
    pub min_spacing: f32,
    pub scale: [f32; 2],
    pub random_rotation: bool,
    pub avoid_unpathable: bool,
    pub player: Option<u8>,
    pub seed: u64,
}

pub(super) fn scatter(doc: &mut MapDocument, area: Area, a: ScatterArgs) -> Result<Applied, OpError> {
    let types: Vec<String> = a.type_ids.iter().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).collect();
    if types.is_empty() && a.kind != ObjectKind::Point {
        return invalid("type_ids must name at least one doodad or unit type, e.g. [\"CharRock01\", \"CharRock02\"]");
    }
    if !(a.min_spacing.is_finite() && (0.0..=64.0).contains(&a.min_spacing)) {
        return invalid(format!("min_spacing must be 0..=64, got {}", a.min_spacing));
    }
    let [s0, s1] = a.scale;
    if !(0.01..=100.0).contains(&s0) || !(0.01..=100.0).contains(&s1) || s0 > s1 {
        return invalid(format!("scale must be [min, max] with 0.01 <= min <= max <= 100, got [{s0}, {s1}]"));
    }
    if let Some(p) = a.player {
        check_player(p)?;
    }
    let (lo, hi) = area.bounds();
    let (w, h) = (doc.terrain.width as f32, doc.terrain.height as f32);
    let lo = lo.max(glam::Vec2::ZERO);
    let hi = hi.min(glam::Vec2::new(w, h));
    if !(lo.x < hi.x && lo.y < hi.y) {
        return invalid("the area is empty or outside the map");
    }
    // Estimate the area's size by sampling, for density.
    let mut rng = Rng::new(a.seed);
    let inside = (0..2000)
        .filter(|_| area.contains(glam::Vec2::new(rng.range(lo.x, hi.x), rng.range(lo.y, hi.y))))
        .count();
    let size = (hi - lo).x * (hi - lo).y * inside as f32 / 2000.0;
    let target = match (a.count, a.density) {
        (Some(c), None) => c,
        (None, Some(d)) if d.is_finite() && d > 0.0 => (size * d / 100.0).round() as u32,
        (None, Some(d)) => return invalid(format!("density must be greater than 0, got {d}")),
        (Some(_), Some(_)) => return invalid("give either count or density, not both"),
        (None, None) => return invalid("give count (how many) or density (objects per 100 square cells)"),
    };
    if target == 0 {
        return invalid("that would place nothing; raise count or density");
    }
    if target > 2000 {
        return invalid(format!("at most 2000 objects per scatter, got {target}; scatter a smaller area or lower the density"));
    }

    let grid = (a.avoid_unpathable).then(|| crate::walk::WalkGrid::new(doc));
    // Existing objects count for spacing too, so nothing lands on a unit.
    let mut taken: Vec<glam::Vec2> = doc
        .objects
        .objects
        .iter()
        .filter(|o| o.kind != ObjectKind::Point)
        .map(|o| o.position.truncate())
        .filter(|p| p.cmpge(lo - a.min_spacing).all() && p.cmple(hi + a.min_spacing).all())
        .collect();
    let next = doc.objects.next_id;
    let mut objects = Vec::new();
    let mut tries = 0u32;
    while objects.len() < target as usize && tries < target * 60 {
        tries += 1;
        let p = glam::Vec2::new(rng.range(lo.x, hi.x), rng.range(lo.y, hi.y));
        if !area.contains(p) {
            continue;
        }
        if let Some(g) = &grid {
            if g.cell_at(p.x, p.y).and_then(|c| g.blocked(c)).is_some_and(|b| b != crate::walk::Blocked::OutsidePlayable)
            {
                continue;
            }
        }
        if a.min_spacing > 0.0 && taken.iter().any(|q| q.distance_squared(p) < a.min_spacing * a.min_spacing) {
            continue;
        }
        let type_id = if types.is_empty() {
            String::new()
        } else {
            types[(rng.unit() * types.len() as f32) as usize % types.len()].clone()
        };
        let spec = ObjectSpec {
            kind: Some(a.kind),
            type_id,
            x: p.x,
            y: p.y,
            player: a.player,
            rotation: if a.random_rotation { rng.range(0.0, std::f32::consts::TAU) } else { 0.0 },
            scale: (s0 != 1.0 || s1 != 1.0).then(|| rng.range(s0, s1)),
            name: String::new(),
        };
        match new_object(doc, spec) {
            Ok(o) => {
                taken.push(p);
                objects.push(o);
            }
            Err(e) => {
                doc.objects.next_id = next;
                return Err(e);
            }
        }
    }
    if objects.is_empty() {
        return invalid("no free spot was found; lower min_spacing, or pass avoid_unpathable: false");
    }
    let placed = objects.len();
    let ids: Vec<u32> = objects.iter().map(|o| o.id.0).collect();
    let edit = insert_all(doc, objects);
    let mut result = json!({"placed": placed, "requested": target, "ids": ids});
    if placed < target as usize {
        result["note"] = json!(format!(
            "only {placed} of {target} fit with min_spacing {} (and pathing); lower min_spacing to fit more",
            a.min_spacing
        ));
    }
    Ok(Applied { label: format!("Scatter {placed} objects"), edit, result })
}

// ---------------------------------------------------------------------------
// Mirror

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Symmetry {
    /// Point symmetry around the playable area's center (most 1v1 maps).
    Rotate180,
    /// Flip across the vertical center line (west <-> east).
    MirrorX,
    /// Flip across the horizontal center line (south <-> north).
    MirrorY,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    West,
    East,
    South,
    North,
}

pub(super) struct MirrorArgs {
    pub symmetry: Symmetry,
    pub keep: Side,
    pub players: Vec<[u8; 2]>,
    pub terrain: bool,
    pub objects: bool,
}

pub(super) fn mirror(doc: &mut MapDocument, a: MirrorArgs) -> Result<Applied, OpError> {
    match (a.symmetry, a.keep) {
        (Symmetry::MirrorX, Side::South | Side::North) => {
            return invalid("mirror_x copies west <-> east; keep must be \"west\" or \"east\"")
        }
        (Symmetry::MirrorY, Side::West | Side::East) => {
            return invalid("mirror_y copies south <-> north; keep must be \"south\" or \"north\"")
        }
        _ => {}
    }
    if !a.terrain && !a.objects {
        return invalid("nothing to mirror: terrain and objects are both off");
    }
    let mut map_player = [0u8; 16];
    for (i, m) in map_player.iter_mut().enumerate() {
        *m = i as u8;
    }
    for [p, q] in &a.players {
        check_player(*p)?;
        check_player(*q)?;
        map_player[*p as usize] = *q;
        map_player[*q as usize] = *p;
    }

    let [l, b, r, t] = doc.info.playable;
    // Vertex coordinates of the mirror: v' = s - v.
    let (sx, sy) = ((l + r) as i64, (b + t) as i64);
    let center = glam::Vec2::new(sx as f32 / 2.0, sy as f32 / 2.0);
    let flip_x = matches!(a.symmetry, Symmetry::Rotate180 | Symmetry::MirrorX);
    let flip_y = matches!(a.symmetry, Symmetry::Rotate180 | Symmetry::MirrorY);
    let map_pt = |p: glam::Vec2| {
        glam::Vec2::new(if flip_x { sx as f32 - p.x } else { p.x }, if flip_y { sy as f32 - p.y } else { p.y })
    };
    // Which side of the center a point is on; 0 on the dividing line.
    let side = |p: glam::Vec2| -> i8 {
        let d = match a.keep {
            Side::West => center.x - p.x,
            Side::East => p.x - center.x,
            Side::South => center.y - p.y,
            Side::North => p.y - center.y,
        };
        if d.abs() < 1e-4 {
            0
        } else {
            d.signum() as i8
        }
    };

    let mut edits = Vec::new();
    let mut copied_objects = 0usize;
    let mut removed_objects = 0usize;
    if a.terrain {
        let tr = doc.terrain.clone();
        let full = tr.full_rect();
        let cells = VertexRect { x0: 0, y0: 0, x1: tr.width, y1: tr.height };
        let before: Vec<_> = [ChannelKind::Heights, ChannelKind::CliffLevels, ChannelKind::Layers]
            .into_iter()
            .map(|k| tr.capture(full, k))
            .chain(std::iter::once(tr.capture(cells, ChannelKind::Pathing)))
            .collect();
        let te = &mut doc.terrain;
        for y in 0..tr.verts_y() {
            for x in 0..tr.verts_x() {
                if side(glam::Vec2::new(x as f32, y as f32)) >= 0 {
                    continue;
                }
                let (fx, fy) =
                    (if flip_x { sx - x as i64 } else { x as i64 }, if flip_y { sy - y as i64 } else { y as i64 });
                if fx < 0 || fy < 0 || fx >= tr.verts_x() as i64 || fy >= tr.verts_y() as i64 {
                    continue;
                }
                let (to, from) = (tr.vertex_index(x, y), tr.vertex_index(fx as u32, fy as u32));
                te.heights[to] = tr.heights[from];
                te.cliff_levels[to] = tr.cliff_levels[from];
                for (layer, src) in te.layers.iter_mut().zip(&tr.layers) {
                    layer.weights[to] = src.weights[from];
                }
            }
        }
        for y in 0..tr.height {
            for x in 0..tr.width {
                if side(glam::Vec2::new(x as f32 + 0.5, y as f32 + 0.5)) >= 0 {
                    continue;
                }
                let (fx, fy) = (
                    if flip_x { sx - 1 - x as i64 } else { x as i64 },
                    if flip_y { sy - 1 - y as i64 } else { y as i64 },
                );
                if fx < 0 || fy < 0 || fx >= tr.width as i64 || fy >= tr.height as i64 {
                    continue;
                }
                te.pathing[tr.cell_index(x, y)] = tr.pathing[tr.cell_index(fx as u32, fy as u32)];
            }
        }
        doc.dirty.terrain = true;
        for b in before {
            let after = doc.terrain.capture(b.rect, kind_of(&b.data));
            edits.push(Edit::Terrain { before: b, after });
        }
    }

    if a.objects {
        let old: Vec<PlacedObject> = doc.objects.objects.clone();
        for o in old.iter().filter(|o| side(o.position.truncate()) < 0) {
            edits.push(Edit::Object { id: o.id, before: Some(o.clone()), after: None });
            removed_objects += 1;
        }
        let mut names: std::collections::HashSet<String> =
            old.iter().filter(|o| side(o.position.truncate()) >= 0).map(|o| o.name.clone()).collect();
        let mut new = Vec::new();
        for o in old.iter().filter(|o| side(o.position.truncate()) > 0) {
            let p = map_pt(o.position.truncate());
            if p.x < 0.0 || p.y < 0.0 || p.x > doc.terrain.width as f32 || p.y > doc.terrain.height as f32 {
                continue;
            }
            let id = doc.objects.alloc_id();
            let mut c = o.clone();
            c.id = id;
            c.position = glam::Vec3::new(p.x, p.y, o.position.z);
            c.rotation = mirror_angle(o.rotation, a.symmetry);
            if o.kind == ObjectKind::Unit && (o.player as usize) < map_player.len() {
                c.player = map_player[o.player as usize];
            }
            if !o.name.is_empty() {
                let base = if o.type_id == "StartLoc" { "Start Location".to_owned() } else { format!("{} (mirrored)", o.name) };
                c.name = free_name(&base, o.type_id == "StartLoc", &mut names);
            }
            new.push(c);
        }
        copied_objects = new.len();
        for e in &edits {
            if let Edit::Object { .. } = e {
                e.apply(doc, true);
            }
        }
        for c in new {
            let e = Edit::Object { id: c.id, before: None, after: Some(c) };
            e.apply(doc, true);
            edits.push(e);
        }

        // Players' own start locations follow the player mapping.
        let before = doc.info.clone();
        let mut info = doc.info.clone();
        let starts: Vec<(u8, glam::Vec2)> =
            info.players.iter().filter_map(|p| p.start_location.map(|s| (p.id, s))).collect();
        for (id, s) in &starts {
            if side(*s) < 0 {
                if let Some(p) = info.players.iter_mut().find(|p| p.id == *id) {
                    p.start_location = None;
                }
            }
        }
        for (id, s) in starts {
            if side(s) > 0 {
                let to = map_player.get(id as usize).copied().unwrap_or(id);
                if let Some(p) = info.players.iter_mut().find(|p| p.id == to && p.id != id) {
                    p.start_location = Some(map_pt(s));
                }
            }
        }
        if info != before {
            doc.info = info.clone();
            doc.dirty.info = true;
            edits.push(Edit::Info { before: Box::new(before), after: Box::new(info) });
        }
    }
    let what = match a.symmetry {
        Symmetry::Rotate180 => "Rotate",
        Symmetry::MirrorX | Symmetry::MirrorY => "Mirror",
    };
    let side_name = match a.keep {
        Side::West => "west",
        Side::East => "east",
        Side::South => "south",
        Side::North => "north",
    };
    Ok(Applied {
        label: format!("{what} the {side_name} half"),
        edit: Edit::Batch(edits),
        result: json!({
            "center": [center.x, center.y],
            "objects_copied": copied_objects,
            "objects_replaced": removed_objects,
            "terrain": a.terrain,
            "note": "regions, cameras and triggers are not mirrored",
        }),
    })
}

fn kind_of(c: &crate::terrain::TerrainChannel) -> ChannelKind {
    use crate::terrain::TerrainChannel as C;
    match c {
        C::Heights(_) => ChannelKind::Heights,
        C::CliffLevels(_) => ChannelKind::CliffLevels,
        C::Layers(_) => ChannelKind::Layers,
        C::Pathing(_) => ChannelKind::Pathing,
    }
}

fn mirror_angle(a: f32, s: Symmetry) -> f32 {
    use std::f32::consts::{PI, TAU};
    let m = match s {
        Symmetry::Rotate180 => a + PI,
        Symmetry::MirrorX => PI - a,
        Symmetry::MirrorY => -a,
    };
    m.rem_euclid(TAU)
}

/// `base` itself or with the lowest free number ("Start Location 003").
fn free_name(base: &str, numbered: bool, taken: &mut std::collections::HashSet<String>) -> String {
    let mut n = 1;
    let mut name = if numbered { format!("{base} {n:03}") } else { base.to_owned() };
    while taken.contains(&name) {
        n += 1;
        name = if numbered { format!("{base} {n:03}") } else { format!("{base} {n}") };
    }
    taken.insert(name.clone());
    name
}

// ---------------------------------------------------------------------------
// Ramp

pub(super) fn ramp(doc: &mut MapDocument, from: [f32; 2], to: [f32; 2], width: f32) -> Result<Applied, OpError> {
    check_in_map(doc, from[0], from[1])?;
    check_in_map(doc, to[0], to[1])?;
    if !(width.is_finite() && (1.0..=24.0).contains(&width)) {
        return invalid(format!("width must be 1..=24, got {width}"));
    }
    let (a, b) = (glam::Vec2::from(from), glam::Vec2::from(to));
    let len = a.distance(b);
    if len < 2.0 {
        return invalid("from and to must be at least 2 apart");
    }
    let t = &doc.terrain;
    let near_level = |p: glam::Vec2| {
        let (x, y) = (p.x.round().clamp(0.0, t.width as f32) as u32, p.y.round().clamp(0.0, t.height as f32) as u32);
        t.cliff_levels[t.vertex_index(x, y)]
    };
    let (ha, hb) = (t.sample_height(a.x, a.y), t.sample_height(b.x, b.y));
    let (la, lb) = (near_level(a), near_level(b));
    let slope = (hb - ha).abs() / len;
    if slope > crate::walk::MAX_STEP * 0.8 {
        let need = ((hb - ha).abs() / (crate::walk::MAX_STEP * 0.8)).ceil();
        return invalid(format!(
            "a ramp from height {ha:.2} to {hb:.2} over {len:.1} cells is too steep to walk; make it at least {need} cells long"
        ));
    }
    let dir = (b - a) / len;
    let half = width / 2.0;
    // Vertex rect covering the corridor plus a one-vertex margin.
    let lo = a.min(b) - glam::Vec2::splat(half + 1.0);
    let hi = a.max(b) + glam::Vec2::splat(half + 1.0);
    let rect = t.clamp_rect(VertexRect {
        x0: lo.x.floor().max(0.0) as u32,
        y0: lo.y.floor().max(0.0) as u32,
        x1: hi.x.ceil().max(0.0) as u32 + 1,
        y1: hi.y.ceil().max(0.0) as u32 + 1,
    });
    let cells = VertexRect { x0: rect.x0, y0: rect.y0, x1: rect.x1.min(t.width), y1: rect.y1.min(t.height) };
    let before = [t.capture(rect, ChannelKind::Heights), t.capture(rect, ChannelKind::CliffLevels), t.capture(cells, ChannelKind::Pathing)];

    // Distance along and across the ramp.
    let along_across = |p: glam::Vec2| {
        let d = p - a;
        (d.dot(dir) / len, (d - dir * d.dot(dir)).length())
    };
    let te = &mut doc.terrain;
    let mut changed = 0usize;
    for y in rect.y0..rect.y1 {
        for x in rect.x0..rect.x1 {
            let (s, across) = along_across(glam::Vec2::new(x as f32, y as f32));
            if !(0.0..=1.0).contains(&s) || across > half {
                continue;
            }
            let i = te.vertex_index(x, y);
            let level = if s < 0.5 { la } else { lb };
            te.cliff_levels[i] = level;
            te.heights[i] = ha + (hb - ha) * s;
            changed += 1;
        }
    }
    for y in cells.y0..cells.y1 {
        for x in cells.x0..cells.x1 {
            let (s, across) = along_across(glam::Vec2::new(x as f32 + 0.5, y as f32 + 0.5));
            if (0.0..=1.0).contains(&s) && across <= half {
                let i = te.cell_index(x, y);
                te.pathing[i] &= !pathing::UNWALKABLE;
            }
        }
    }
    if changed == 0 {
        return invalid("the ramp covers no terrain vertices; make it wider");
    }
    doc.dirty.terrain = true;
    let edits = before
        .into_iter()
        .map(|b| {
            let after = doc.terrain.capture(b.rect, kind_of(&b.data));
            Edit::Terrain { before: b, after }
        })
        .collect();
    let levels = (hb - ha) / CLIFF_LEVEL_HEIGHT;
    Ok(Applied {
        label: "Make ramp".into(),
        edit: Edit::Batch(edits),
        result: json!({
            "from_height": ha, "to_height": hb, "cliff_levels": [la, lb],
            "rise_in_levels": (levels * 100.0).round() / 100.0,
            "vertices": changed,
        }),
    })
}

// ---------------------------------------------------------------------------
// Rename helpers

/// Rename trigger references to a placed object or region (`Expr::Placed`).
/// Returns how many references changed.
pub(super) fn rename_placed(lib: &mut crate::triggers::TriggerLibrary, old: &str, new: &str) -> usize {
    fn walk(v: &mut Value, old: &str, new: &str, n: &mut usize) {
        match v {
            Value::Object(m) => {
                if m.len() == 1 && m.get("Placed").and_then(Value::as_str) == Some(old) {
                    m.insert("Placed".into(), Value::String(new.to_owned()));
                    *n += 1;
                    return;
                }
                for x in m.values_mut() {
                    walk(x, old, new, n);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, old, new, n)),
            _ => {}
        }
    }
    let Ok(mut v) = serde_json::to_value(&*lib) else { return 0 };
    let mut n = 0;
    walk(&mut v, old, new, &mut n);
    if n > 0 {
        if let Ok(l) = serde_json::from_value(v) {
            *lib = l;
        } else {
            return 0;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::super::Op;
    use super::*;
    use crate::regions::{Region, Shape};

    fn doc() -> MapDocument {
        let mut d = MapDocument::new("b", 64, 64);
        d.info.playable = [0, 0, 64, 64];
        d
    }

    /// Run, check something changed, undo, check the document is back.
    fn run_and_undo(d: &mut MapDocument, op: Op) -> Applied {
        let before = serde_json::to_value(&*d).unwrap();
        let a = op.run(d).unwrap();
        assert_ne!(serde_json::to_value(&*d).unwrap(), before, "op changed nothing");
        a.edit.apply(d, false);
        let mut after = serde_json::to_value(&*d).unwrap();
        after["objects"]["next_id"] = before["objects"]["next_id"].clone();
        after["triggers"]["next_id"] = before["triggers"]["next_id"].clone();
        assert_eq!(after, before, "undo did not restore the document");
        a.edit.apply(d, true);
        a
    }

    fn spec(t: &str, x: f32, y: f32) -> ObjectSpec {
        ObjectSpec { kind: None, type_id: t.into(), x, y, player: Some(1), rotation: 0.0, scale: None, name: String::new() }
    }

    #[test]
    fn place_objects_is_one_step_and_all_or_nothing() {
        let mut d = doc();
        let a = run_and_undo(&mut d, Op::PlaceObjects { objects: vec![spec("Marine", 5.0, 5.0), spec("SCV", 6.0, 5.0)] });
        assert_eq!(a.result["ids"].as_array().unwrap().len(), 2);
        assert_eq!(d.objects.objects.len(), 2);
        let e = Op::PlaceObjects { objects: vec![spec("Marine", 5.0, 5.0), spec("Marine", 500.0, 5.0)] }
            .run(&mut d)
            .unwrap_err();
        assert!(e.to_string().contains("objects[1]"), "{e}");
        assert_eq!(d.objects.objects.len(), 2);
    }

    fn scatter_rect(seed: u64) -> Op {
        Op::Scatter {
            kind: None,
            type_ids: vec!["CharRock01".into(), "CharRock02".into()],
            region: None,
            rect: Some([10.0, 10.0, 30.0, 30.0]),
            count: Some(25),
            density: None,
            min_spacing: 2.0,
            scale: Some([0.8, 1.2]),
            random_rotation: true,
            avoid_unpathable: true,
            player: None,
            seed: Some(seed),
        }
    }

    #[test]
    fn scatter_respects_area_spacing_and_seed() {
        let mut d = doc();
        let a = run_and_undo(&mut d, scatter_rect(7));
        assert_eq!(a.result["placed"], 25);
        let pts: Vec<glam::Vec2> = d.objects.objects.iter().map(|o| o.position.truncate()).collect();
        for (i, p) in pts.iter().enumerate() {
            assert!(p.x >= 10.0 && p.x <= 30.0 && p.y >= 10.0 && p.y <= 30.0);
            assert!(pts[i + 1..].iter().all(|q| q.distance(*p) >= 2.0));
        }
        assert!(d.objects.objects.iter().all(|o| o.kind == ObjectKind::Doodad && (0.8..=1.2).contains(&o.scale.x)));
        // Same seed, same layout.
        let mut d2 = doc();
        scatter_rect(7).run(&mut d2).unwrap();
        let pts2: Vec<glam::Vec2> = d2.objects.objects.iter().map(|o| o.position.truncate()).collect();
        assert_eq!(pts, pts2);

        // Regions work too, and nothing lands outside a circle.
        let mut d = doc();
        d.regions.push(Region {
            id: crate::objects::ObjectId(900),
            name: "Grove".into(),
            color: [0, 0, 0],
            shapes: vec![Shape::Circle { center: glam::Vec2::new(32.0, 32.0), radius: 6.0 }],
        });
        let a = Op::Scatter {
            kind: None,
            type_ids: vec!["Tree".into()],
            region: Some("Grove".into()),
            rect: None,
            count: None,
            density: Some(10.0),
            min_spacing: 1.0,
            scale: None,
            random_rotation: false,
            avoid_unpathable: false,
            player: None,
            seed: Some(1),
        }
        .run(&mut d)
        .unwrap();
        assert!(a.result["requested"].as_u64().unwrap() >= 10);
        assert!(d.objects.objects.iter().all(|o| o.position.truncate().distance(glam::Vec2::splat(32.0)) <= 6.0));
    }

    #[test]
    fn mirror_copies_terrain_and_objects_across() {
        let mut d = doc();
        let i = d.terrain.vertex_index(10, 12);
        d.terrain.heights[i] = 20.0;
        let c = d.terrain.cell_index(5, 6);
        d.terrain.pathing[c] = pathing::UNWALKABLE;
        Op::PlaceObjects {
            objects: vec![
                ObjectSpec { rotation: 0.5, ..spec("CommandCenter", 12.5, 14.5) },
                ObjectSpec { kind: Some(ObjectKind::Point), name: "Start Location 001".into(), ..spec("StartLoc", 12.5, 14.5) },
                spec("Marine", 50.0, 50.0),
            ],
        }
        .run(&mut d)
        .unwrap();
        d.info.players[0].start_location = Some(glam::Vec2::new(12.5, 14.5));
        let a = run_and_undo(
            &mut d,
            Op::Mirror { symmetry: Symmetry::Rotate180, keep: Side::West, players: None, terrain: true, objects: true },
        );
        assert_eq!(a.result["objects_copied"], 2);
        assert_eq!(a.result["objects_replaced"], 1);
        assert!(!d.objects.objects.iter().any(|o| o.type_id == "Marine"));
        let cc: Vec<_> = d.objects.objects.iter().filter(|o| o.type_id == "CommandCenter").collect();
        assert_eq!(cc.len(), 2);
        let copy = cc.iter().find(|o| o.position.x > 32.0).unwrap();
        assert_eq!(copy.position.truncate(), glam::Vec2::new(51.5, 49.5));
        assert_eq!(copy.player, 2);
        assert!((copy.rotation - (0.5 + std::f32::consts::PI)).abs() < 1e-4);
        assert!(d.objects.objects.iter().any(|o| o.name == "Start Location 002"));
        assert_eq!(d.terrain.heights[d.terrain.vertex_index(54, 52)], 20.0);
        assert_eq!(d.terrain.pathing[d.terrain.cell_index(58, 57)], pathing::UNWALKABLE);
        assert_eq!(d.info.players[1].start_location, Some(glam::Vec2::new(51.5, 49.5)));

        assert!(Op::Mirror { symmetry: Symmetry::MirrorX, keep: Side::North, players: None, terrain: true, objects: true }
            .run(&mut d)
            .is_err());
    }

    #[test]
    fn ramp_joins_two_cliff_levels() {
        let mut d = doc();
        for y in 0..=64 {
            for x in 32..=64 {
                let i = d.terrain.vertex_index(x, y);
                d.terrain.cliff_levels[i] += 1;
                d.terrain.heights[i] += CLIFF_LEVEL_HEIGHT;
            }
        }
        let grid = crate::walk::WalkGrid::new(&d);
        assert!(grid.route([10.5, 20.5], [50.5, 20.5]).is_err());
        let a = run_and_undo(&mut d, Op::Ramp { from: [26.0, 20.0], to: [38.0, 20.0], width: 4.0 });
        assert_eq!(a.result["cliff_levels"], json!([2, 3]));
        let grid = crate::walk::WalkGrid::new(&d);
        assert!(grid.route([10.5, 20.5], [50.5, 20.5]).is_ok());
        assert!(Op::Ramp { from: [31.0, 40.0], to: [33.0, 40.0], width: 4.0 }.run(&mut d).is_err());
    }

    #[test]
    fn region_rename_updates_triggers_and_small_edits_undo() {
        use crate::triggers::{Call, Expr, Stmt, Trigger, TriggerId};
        let mut d = doc();
        Op::CreateRegion {
            name: "Base".into(),
            shape: Shape::Rect { min: glam::Vec2::new(1.0, 1.0), max: glam::Vec2::new(9.0, 9.0) },
            color: None,
        }
        .run(&mut d)
        .unwrap();
        let mut t = Trigger::new(TriggerId(0), "Enter");
        t.actions.push(Stmt::Call(Call { function: "RegionAddRegion".into(), args: vec![Expr::Placed("Base".into())] }));
        Op::CreateTrigger { trigger: t }.run(&mut d).unwrap();
        let a = run_and_undo(
            &mut d,
            Op::UpdateRegion {
                name: "Base".into(),
                new_name: Some("Home".into()),
                shape: None,
                color: None,
                offset: Some([2.0, 0.0]),
            },
        );
        assert_eq!(a.result["trigger_references_updated"], 1);
        assert!(serde_json::to_string(&d.triggers).unwrap().contains("\"Placed\":\"Home\""));
        assert_eq!(d.regions[0].shapes[0].bounds().0, glam::Vec2::new(3.0, 1.0));

        let a = run_and_undo(&mut d, Op::SetTriggerEnabled { name: "Enter".into(), enabled: None, initially_on: Some(false) });
        assert_eq!(a.result["initially_on"], false);

        Op::SetString { key: "A/B".into(), value: "hi".into(), locale: "enUS".into() }.run(&mut d).unwrap();
        run_and_undo(&mut d, Op::DeleteString { key: "A/B".into(), locale: None });
        assert!(Op::DeleteString { key: "Nope".into(), locale: None }.run(&mut d).is_err());

        for (class, id, field, value) in [("CWeaponLegacy", "Gun", "Range", "6"), ("CUnit", "Hero", "LifeMax", "200")] {
            Op::SetDataField {
                file: String::new(),
                class: class.into(),
                id: id.into(),
                field: field.into(),
                value: value.into(),
                parent: None,
            }
            .run(&mut d)
            .unwrap();
        }
        let hero = d.data.entries.iter().position(|e| e.id() == "Hero").unwrap();
        let mut w = crate::catalog::XmlNode::new("WeaponArray");
        w.set_attr("Link", "Gun");
        d.data.entries[hero].node.children.push(w);
        run_and_undo(&mut d, Op::DeleteDataField { class: "CUnit".into(), id: "Hero".into(), field: "LifeMax".into() });
        let err = Op::DeleteData { class: "CWeaponLegacy".into(), id: "Gun".into(), force: false }.run(&mut d).unwrap_err();
        assert!(err.to_string().contains("Hero.WeaponArray"), "{err}");
        run_and_undo(&mut d, Op::DeleteData { class: "CWeaponLegacy".into(), id: "Gun".into(), force: true });
    }
}
