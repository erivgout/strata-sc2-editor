//! Placed objects: units, doodads and points.

use serde::{Deserialize, Serialize};

/// Stable id of a placed object, region or camera within one document.
/// Matches the `Id` attribute in the SC2 Objects file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ObjectId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectKind {
    Unit,
    Doodad,
    Point,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlacedObject {
    pub id: ObjectId,
    pub kind: ObjectKind,
    /// Catalog id: unit type ("Marine"), doodad model ("CharRock01"),
    /// or empty for points.
    pub type_id: String,
    /// Optional display name (points always have one).
    pub name: String,
    pub position: glam::Vec3,
    /// Facing in radians, counter-clockwise from +X.
    pub rotation: f32,
    pub scale: glam::Vec3,
    /// Owning player slot (0 = neutral). Ignored for doodads/points.
    pub player: u8,
    /// Extra attributes preserved verbatim from the source file
    /// (name, value) so round-tripping is lossless.
    pub extra: Vec<(String, String)>,
}

impl PlacedObject {
    pub fn new(id: ObjectId, kind: ObjectKind, type_id: impl Into<String>, position: glam::Vec3) -> Self {
        Self {
            id,
            kind,
            type_id: type_id.into(),
            name: String::new(),
            position,
            rotation: 0.0,
            scale: glam::Vec3::ONE,
            player: if kind == ObjectKind::Unit { 1 } else { 0 },
            extra: Vec::new(),
        }
    }

    pub fn display_name(&self) -> &str {
        if self.name.is_empty() { &self.type_id } else { &self.name }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ObjectLayer {
    pub objects: Vec<PlacedObject>,
    /// Next id handed out by [`ObjectLayer::alloc_id`]. Shared with regions
    /// and cameras so ids are unique document-wide.
    pub next_id: u32,
}

impl ObjectLayer {
    pub fn alloc_id(&mut self) -> ObjectId {
        let max = self.objects.iter().map(|o| o.id.0).max().unwrap_or(0);
        self.next_id = self.next_id.max(max + 1).max(1);
        let id = ObjectId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn get(&self, id: ObjectId) -> Option<&PlacedObject> {
        self.objects.iter().find(|o| o.id == id)
    }

    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut PlacedObject> {
        self.objects.iter_mut().find(|o| o.id == id)
    }

    pub fn remove(&mut self, id: ObjectId) -> Option<PlacedObject> {
        let i = self.objects.iter().position(|o| o.id == id)?;
        Some(self.objects.remove(i))
    }

    /// Objects whose XY lies within `radius` of `p`, nearest first.
    pub fn pick(&self, p: glam::Vec2, radius: f32) -> Vec<ObjectId> {
        let mut hits: Vec<(f32, ObjectId)> = self
            .objects
            .iter()
            .map(|o| (o.position.truncate().distance(p), o.id))
            .filter(|(d, _)| *d <= radius)
            .collect();
        hits.sort_by(|a, b| a.0.total_cmp(&b.0));
        hits.into_iter().map(|(_, id)| id).collect()
    }

    /// Objects whose XY lies inside the axis-aligned box.
    pub fn in_rect(&self, min: glam::Vec2, max: glam::Vec2) -> Vec<ObjectId> {
        self.objects
            .iter()
            .filter(|o| {
                let p = o.position.truncate();
                p.x >= min.x && p.y >= min.y && p.x <= max.x && p.y <= max.y
            })
            .map(|o| o.id)
            .collect()
    }
}
