//! Terrain brushes, shared by the viewport tools and MCP ops.
//!
//! A brush stroke is: [`Stroke::begin`], one or more [`Stroke::dab`]s while the
//! pointer moves, then [`Stroke::finish`] which yields one undoable [`Edit`].

use serde::{Deserialize, Serialize};

use crate::history::Edit;
use crate::terrain::{ChannelKind, Terrain, VertexRect, CLIFF_LEVEL_HEIGHT};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BrushTool {
    Raise,
    Lower,
    Smooth,
    /// Pull heights toward `target`.
    Flatten { target: f32 },
    /// Deterministic value noise.
    Noise { seed: u32 },
    /// Paint texture layer `layer` (index into `Terrain::layers`).
    Paint { layer: usize },
    /// Remove texture layer `layer`'s weight.
    Erase { layer: usize },
    /// Set cliff level; heights shift by whole cliff levels.
    Cliff { level: u8 },
    /// Set (`on = true`) or clear pathing flag bits on cells.
    Pathing { flags: u8, on: bool },
}

impl BrushTool {
    pub fn channels(&self) -> &'static [ChannelKind] {
        match self {
            BrushTool::Raise | BrushTool::Lower | BrushTool::Smooth | BrushTool::Flatten { .. } | BrushTool::Noise { .. } => {
                &[ChannelKind::Heights]
            }
            BrushTool::Paint { .. } | BrushTool::Erase { .. } => &[ChannelKind::Layers],
            BrushTool::Cliff { .. } => &[ChannelKind::CliffLevels, ChannelKind::Heights],
            BrushTool::Pathing { .. } => &[ChannelKind::Pathing],
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            BrushTool::Raise => "Raise terrain",
            BrushTool::Lower => "Lower terrain",
            BrushTool::Smooth => "Smooth terrain",
            BrushTool::Flatten { .. } => "Flatten terrain",
            BrushTool::Noise { .. } => "Add noise",
            BrushTool::Paint { .. } => "Paint texture",
            BrushTool::Erase { .. } => "Erase texture",
            BrushTool::Cliff { .. } => "Set cliff level",
            BrushTool::Pathing { .. } => "Paint pathing",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum BrushShape {
    #[default]
    Circle,
    Square,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Falloff {
    Constant,
    Linear,
    #[default]
    Smooth,
    Sharp,
}

impl Falloff {
    /// Weight for normalized distance `t` in 0..=1.
    pub fn weight(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Falloff::Constant => 1.0,
            Falloff::Linear => 1.0 - t,
            Falloff::Smooth => {
                let s = 1.0 - t;
                s * s * (3.0 - 2.0 * s)
            }
            Falloff::Sharp => (1.0 - t).powi(3),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Brush {
    pub tool: BrushTool,
    pub shape: BrushShape,
    pub falloff: Falloff,
    /// Radius in world units (cells).
    pub radius: f32,
    /// 0..=1. Height tools move up to `strength * 4` units per dab.
    pub strength: f32,
}

impl Default for Brush {
    fn default() -> Self {
        Self { tool: BrushTool::Raise, shape: BrushShape::Circle, falloff: Falloff::Smooth, radius: 6.0, strength: 0.25 }
    }
}

impl Brush {
    /// Falloff weight of the vertex (or cell, `per_cell`) at `x, y` for a
    /// dab at `center`; 0 outside the brush.
    fn weight_at(&self, center: glam::Vec2, x: u32, y: u32, per_cell: bool) -> f32 {
        let off = if per_cell { 0.5 } else { 0.0 };
        let r = self.radius.max(0.5);
        let d = glam::Vec2::new(x as f32 + off, y as f32 + off) - center;
        let t = match self.shape {
            BrushShape::Circle => d.length() / r,
            BrushShape::Square => d.x.abs().max(d.y.abs()) / r,
        };
        if t > 1.0 { 0.0 } else { self.falloff.weight(t) }
    }
}

fn hash_noise(seed: u32, x: i32, y: i32) -> f32 {
    let mut h = seed ^ (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65535.0 * 2.0 - 1.0
}

/// Apply one dab at `center` (world units). Returns the vertex rect touched
/// (cells for pathing).
pub fn dab(terrain: &mut Terrain, brush: &Brush, center: glam::Vec2) -> VertexRect {
    let r = brush.radius.max(0.5);
    let per_cell = matches!(brush.tool, BrushTool::Pathing { .. });
    let (gw, gh) = if per_cell { (terrain.width, terrain.height) } else { (terrain.verts_x(), terrain.verts_y()) };
    let off = if per_cell { 0.5 } else { 0.0 };
    let x0 = ((center.x - r - off).floor().max(0.0) as u32).min(gw);
    let y0 = ((center.y - r - off).floor().max(0.0) as u32).min(gh);
    let x1 = ((center.x + r - off).ceil().max(0.0) as u32 + 1).min(gw);
    let y1 = ((center.y + r - off).ceil().max(0.0) as u32 + 1).min(gh);
    let rect = VertexRect { x0, y0, x1, y1 };
    if rect.is_empty() {
        return rect;
    }

    let weight_at = |x: u32, y: u32| brush.weight_at(center, x, y, per_cell);

    let s = brush.strength.clamp(0.0, 1.0);
    match brush.tool {
        BrushTool::Raise | BrushTool::Lower | BrushTool::Flatten { .. } | BrushTool::Noise { .. } => {
            for y in y0..y1 {
                for x in x0..x1 {
                    let w = weight_at(x, y);
                    if w <= 0.0 {
                        continue;
                    }
                    let i = terrain.vertex_index(x, y);
                    let h = &mut terrain.heights[i];
                    match brush.tool {
                        BrushTool::Raise => *h += 4.0 * s * w,
                        BrushTool::Lower => *h -= 4.0 * s * w,
                        BrushTool::Flatten { target } => *h += (target - *h) * (s * w).min(1.0),
                        BrushTool::Noise { seed } => *h += hash_noise(seed, x as i32, y as i32) * 2.0 * s * w,
                        _ => unreachable!(),
                    }
                    *h = h.clamp(-16.0, 64.0);
                }
            }
        }
        BrushTool::Smooth => {
            let src = terrain.heights.clone();
            let vx = terrain.verts_x() as i64;
            let vy = terrain.verts_y() as i64;
            for y in y0..y1 {
                for x in x0..x1 {
                    let w = weight_at(x, y);
                    if w <= 0.0 {
                        continue;
                    }
                    let mut sum = 0.0;
                    let mut n = 0.0;
                    for dy in -1i64..=1 {
                        for dx in -1i64..=1 {
                            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                            if nx >= 0 && ny >= 0 && nx < vx && ny < vy {
                                sum += src[(ny * vx + nx) as usize];
                                n += 1.0;
                            }
                        }
                    }
                    let i = terrain.vertex_index(x, y);
                    let avg = sum / n;
                    terrain.heights[i] += (avg - terrain.heights[i]) * (s * w).min(1.0);
                }
            }
        }
        BrushTool::Paint { layer } | BrushTool::Erase { layer } => {
            if layer >= terrain.layers.len() {
                return VertexRect { x0: 0, y0: 0, x1: 0, y1: 0 };
            }
            let erase = matches!(brush.tool, BrushTool::Erase { .. });
            let stride = terrain.verts_x();
            for y in y0..y1 {
                for x in x0..x1 {
                    let w = weight_at(x, y);
                    if w <= 0.0 {
                        continue;
                    }
                    let i = (y * stride + x) as usize;
                    let amount = (255.0 * s * w).round() as i32;
                    let cur = terrain.layers[layer].weights[i] as i32;
                    let next = if erase { cur - amount } else { cur + amount };
                    terrain.layers[layer].weights[i] = next.clamp(0, 255) as u8;
                    if !erase {
                        // Painting a layer pushes the layers above it down so it shows
                        // through (for the base layer this uncovers it).
                        for above in layer + 1..terrain.layers.len() {
                            let a = terrain.layers[above].weights[i] as i32;
                            terrain.layers[above].weights[i] = (a - amount).clamp(0, 255) as u8;
                        }
                    }
                }
            }
        }
        BrushTool::Cliff { level } => {
            let level = level.min(15);
            for y in y0..y1 {
                for x in x0..x1 {
                    if weight_at(x, y) <= 0.0 {
                        continue;
                    }
                    let i = terrain.vertex_index(x, y);
                    let old = terrain.cliff_levels[i];
                    if old != level {
                        terrain.cliff_levels[i] = level;
                        terrain.heights[i] += (level as f32 - old as f32) * CLIFF_LEVEL_HEIGHT;
                    }
                }
            }
        }
        BrushTool::Pathing { flags, on } => {
            for y in y0..y1 {
                for x in x0..x1 {
                    if weight_at(x, y) <= 0.0 {
                        continue;
                    }
                    let i = terrain.cell_index(x, y);
                    if on {
                        terrain.pathing[i] |= flags;
                    } else {
                        terrain.pathing[i] &= !flags;
                    }
                }
            }
        }
    }
    rect
}

/// An in-progress brush stroke.
pub struct Stroke {
    brush: Brush,
    before: Terrain,
    rect: VertexRect,
    /// Raise / Lower: how far one stroke may move a vertex from its height
    /// when the stroke began, at full brush weight.
    height_cap: Option<f32>,
    /// Per vertex: the cap times the highest brush weight it has seen this
    /// stroke (so a stroke shapes a hill following the falloff).
    allowance: Vec<f32>,
}

impl Stroke {
    pub fn begin(terrain: &Terrain, brush: Brush) -> Self {
        Self {
            brush,
            before: terrain.clone(),
            rect: VertexRect { x0: 0, y0: 0, x1: 0, y1: 0 },
            height_cap: None,
            allowance: Vec::new(),
        }
    }

    /// Limit Raise / Lower to `cap` units per stroke: heights approach the
    /// start height plus the cap (shaped by the falloff) instead of growing
    /// without bound while the button is held, so a stroke makes a hill, not
    /// a spike. Paint another stroke to go higher.
    pub fn with_height_cap(mut self, cap: f32) -> Self {
        self.height_cap = Some(cap.max(0.0));
        self
    }

    pub fn brush(&self) -> &Brush {
        &self.brush
    }

    /// Apply a dab and return the rect touched by it, for incremental remeshing.
    pub fn dab(&mut self, terrain: &mut Terrain, center: glam::Vec2) -> VertexRect {
        let r = dab(terrain, &self.brush, center);
        let sign = match self.brush.tool {
            BrushTool::Raise => 1.0,
            BrushTool::Lower => -1.0,
            _ => 0.0,
        };
        if let (Some(cap), true) = (self.height_cap, sign != 0.0) {
            if self.allowance.len() != terrain.heights.len() {
                self.allowance = vec![0.0; terrain.heights.len()];
            }
            for y in r.y0..r.y1 {
                for x in r.x0..r.x1 {
                    let i = terrain.vertex_index(x, y);
                    let a = &mut self.allowance[i];
                    *a = a.max(cap * self.brush.weight_at(center, x, y, false));
                    let limit = self.before.heights[i] + sign * *a;
                    let h = &mut terrain.heights[i];
                    *h = if sign > 0.0 { h.min(limit.max(self.before.heights[i])) } else { h.max(limit.min(self.before.heights[i])) };
                }
            }
        }
        self.rect = self.rect.union(&r);
        r
    }

    /// Accumulated rect touched so far.
    pub fn rect(&self) -> VertexRect {
        self.rect
    }

    /// Finish the stroke. Returns `None` if nothing changed.
    pub fn finish(self, terrain: &Terrain) -> Option<Edit> {
        if self.rect.is_empty() {
            return None;
        }
        let edits: Vec<Edit> = self
            .brush
            .tool
            .channels()
            .iter()
            .map(|&k| Edit::Terrain { before: self.before.capture(self.rect, k), after: terrain.capture(self.rect, k) })
            .filter(|e| match e {
                Edit::Terrain { before, after } => before != after,
                _ => true,
            })
            .collect();
        match edits.len() {
            0 => None,
            1 => edits.into_iter().next(),
            _ => Some(Edit::Batch(edits)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::MapDocument;

    #[test]
    fn stroke_is_undoable() {
        let mut doc = MapDocument::new("t", 32, 32);
        let original = doc.terrain.heights.clone();
        let mut s = Stroke::begin(&doc.terrain, Brush::default());
        s.dab(&mut doc.terrain, glam::Vec2::new(16.0, 16.0));
        s.dab(&mut doc.terrain, glam::Vec2::new(18.0, 16.0));
        let edit = s.finish(&doc.terrain).expect("changed");
        assert_ne!(doc.terrain.heights, original);
        edit.apply(&mut doc, false);
        assert_eq!(doc.terrain.heights, original);
    }

    #[test]
    fn capped_raise_makes_a_hill_not_a_spike() {
        let mut t = Terrain::new(32, 32);
        let i = t.vertex_index(16, 16);
        let h0 = t.heights[i];
        let b = Brush { radius: 4.0, strength: 1.0, ..Default::default() };
        let mut s = Stroke::begin(&t, b).with_height_cap(3.0);
        for _ in 0..200 {
            s.dab(&mut t, glam::Vec2::new(16.0, 16.0));
        }
        assert!((t.heights[i] - (h0 + 3.0)).abs() < 1e-4, "center reaches the cap: {}", t.heights[i] - h0);
        let j = t.vertex_index(18, 16);
        let rise = t.heights[j] - h0;
        assert!(rise > 0.5 && rise < 3.0, "slopes follow the falloff: {rise}");
        // Moving the brush keeps the higher allowance already reached.
        s.dab(&mut t, glam::Vec2::new(20.0, 16.0));
        assert!((t.heights[i] - (h0 + 3.0)).abs() < 1e-4);
        // An uncapped stroke keeps climbing.
        let mut u = Terrain::new(32, 32);
        let mut s = Stroke::begin(&u, b);
        for _ in 0..200 {
            s.dab(&mut u, glam::Vec2::new(16.0, 16.0));
        }
        assert!(u.heights[i] - h0 > 30.0);
    }

    #[test]
    fn capped_lower_digs_down_only() {
        let mut t = Terrain::new(16, 16);
        let i = t.vertex_index(8, 8);
        let h0 = t.heights[i];
        let b = Brush { tool: BrushTool::Lower, radius: 3.0, strength: 1.0, ..Default::default() };
        let mut s = Stroke::begin(&t, b).with_height_cap(2.0);
        for _ in 0..50 {
            s.dab(&mut t, glam::Vec2::new(8.0, 8.0));
        }
        assert!((t.heights[i] - (h0 - 2.0)).abs() < 1e-4);
    }

    #[test]
    fn cliff_shifts_height() {
        let mut t = Terrain::new(8, 8);
        let i = t.vertex_index(4, 4);
        let h = t.heights[i];
        let b = Brush { tool: BrushTool::Cliff { level: 3 }, radius: 1.0, ..Default::default() };
        dab(&mut t, &b, glam::Vec2::new(4.0, 4.0));
        assert_eq!(t.cliff_levels[i], 3);
        assert_eq!(t.heights[i], h + CLIFF_LEVEL_HEIGHT);
    }

    #[test]
    fn painting_base_layer_uncovers_it() {
        let mut t = Terrain::new(8, 8);
        let n = t.heights.len();
        t.layers.push(crate::terrain::TextureLayer { texture: "Grass".into(), weights: vec![255; n] });
        let i = t.vertex_index(4, 4);
        assert_eq!(t.blended_weights(i)[0], 0.0);
        let b = Brush { tool: BrushTool::Paint { layer: 0 }, radius: 2.0, strength: 1.0, ..Default::default() };
        dab(&mut t, &b, glam::Vec2::new(4.0, 4.0));
        assert!(t.blended_weights(i)[0] > 0.9);
    }
}
