//! Terrain heightfield.
//!
//! Layout: the map is `width x height` cells. Per-vertex data has
//! `(width + 1) * (height + 1)` entries in row-major order, row 0 at y = 0
//! (south edge). Per-cell data has `width * height` entries. World units
//! equal cells, matching SC2.

use serde::{Deserialize, Serialize};

/// SC2 tilesets allow at most 8 blended texture layers.
pub const MAX_TEXTURE_LAYERS: usize = 8;
/// Height of one cliff level in world units (SC2 uses 2.0).
pub const CLIFF_LEVEL_HEIGHT: f32 = 2.0;

/// Per-cell pathing flags.
pub mod pathing {
    pub const UNWALKABLE: u8 = 1 << 0;
    pub const UNBUILDABLE: u8 = 1 << 1;
    pub const NO_FLY: u8 = 1 << 2;
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextureLayer {
    /// Terrain texture id, e.g. "CharDirt" or "AgriaGrass".
    pub texture: String,
    /// Per-vertex weight 0..=255. Layer 0 is the base and implicitly fills
    /// whatever the layers above it leave uncovered.
    pub weights: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Terrain {
    pub width: u32,
    pub height: u32,
    /// Per-vertex height in world units, *including* cliff level offset.
    pub heights: Vec<f32>,
    /// Per-vertex cliff level (0..=15). Height above is
    /// `cliff * CLIFF_LEVEL_HEIGHT + detail`.
    pub cliff_levels: Vec<u8>,
    pub layers: Vec<TextureLayer>,
    /// Per-cell flags from [`pathing`].
    pub pathing: Vec<u8>,
    pub water_height: Option<f32>,
}

/// Inclusive-exclusive rectangle in vertex coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VertexRect {
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl VertexRect {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> u32 {
        self.y1 - self.y0
    }
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn union(&self, o: &VertexRect) -> VertexRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        VertexRect {
            x0: self.x0.min(o.x0),
            y0: self.y0.min(o.y0),
            x1: self.x1.max(o.x1),
            y1: self.y1.max(o.y1),
        }
    }
}

/// Which per-vertex/per-cell channel a [`TerrainPatch`] covers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TerrainChannel {
    Heights(Vec<f32>),
    CliffLevels(Vec<u8>),
    /// Weights for every layer, layer-major: `layers[i]` is that layer's rect.
    Layers(Vec<Vec<u8>>),
    /// Per-cell rect (x1/y1 exclusive, in cells).
    Pathing(Vec<u8>),
}

/// A rectangular copy of one terrain channel. Undo stores one of these
/// "before" and one "after" per brush stroke.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainPatch {
    pub rect: VertexRect,
    pub data: TerrainChannel,
}

impl Terrain {
    pub fn new(width: u32, height: u32) -> Self {
        let verts = ((width + 1) * (height + 1)) as usize;
        let base_cliff = 2u8;
        Self {
            width,
            height,
            heights: vec![base_cliff as f32 * CLIFF_LEVEL_HEIGHT + 8.0; verts],
            cliff_levels: vec![base_cliff; verts],
            layers: vec![TextureLayer {
                texture: "CharDirt".into(),
                weights: vec![255; verts],
            }],
            pathing: vec![0; (width * height) as usize],
            water_height: None,
        }
    }

    #[inline]
    pub fn verts_x(&self) -> u32 {
        self.width + 1
    }
    #[inline]
    pub fn verts_y(&self) -> u32 {
        self.height + 1
    }
    #[inline]
    pub fn vertex_index(&self, x: u32, y: u32) -> usize {
        (y * self.verts_x() + x) as usize
    }
    #[inline]
    pub fn cell_index(&self, x: u32, y: u32) -> usize {
        (y * self.width + x) as usize
    }

    pub fn full_rect(&self) -> VertexRect {
        VertexRect { x0: 0, y0: 0, x1: self.verts_x(), y1: self.verts_y() }
    }

    /// Clamp a rect to the vertex grid.
    pub fn clamp_rect(&self, r: VertexRect) -> VertexRect {
        VertexRect {
            x0: r.x0.min(self.verts_x()),
            y0: r.y0.min(self.verts_y()),
            x1: r.x1.min(self.verts_x()),
            y1: r.y1.min(self.verts_y()),
        }
    }

    pub fn height_at_vertex(&self, x: u32, y: u32) -> f32 {
        self.heights[self.vertex_index(x.min(self.width), y.min(self.height))]
    }

    /// Bilinear height at a world position (clamped to the map).
    pub fn sample_height(&self, wx: f32, wy: f32) -> f32 {
        let fx = wx.clamp(0.0, self.width as f32);
        let fy = wy.clamp(0.0, self.height as f32);
        let x0 = (fx.floor() as u32).min(self.width.saturating_sub(1));
        let y0 = (fy.floor() as u32).min(self.height.saturating_sub(1));
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let h00 = self.height_at_vertex(x0, y0);
        let h10 = self.height_at_vertex(x0 + 1, y0);
        let h01 = self.height_at_vertex(x0, y0 + 1);
        let h11 = self.height_at_vertex(x0 + 1, y0 + 1);
        let a = h00 + (h10 - h00) * tx;
        let b = h01 + (h11 - h01) * tx;
        a + (b - a) * ty
    }

    /// Surface normal at a vertex via central differences.
    pub fn normal_at_vertex(&self, x: u32, y: u32) -> glam::Vec3 {
        let l = self.height_at_vertex(x.saturating_sub(1), y);
        let r = self.height_at_vertex((x + 1).min(self.width), y);
        let d = self.height_at_vertex(x, y.saturating_sub(1));
        let u = self.height_at_vertex(x, (y + 1).min(self.height));
        glam::Vec3::new(l - r, d - u, 2.0).normalize_or_zero()
    }

    pub fn min_max_height(&self) -> (f32, f32) {
        self.heights
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &h| (lo.min(h), hi.max(h)))
    }

    /// Effective weight of every layer at a vertex, normalized to sum 1.
    /// Upper layers paint over lower ones.
    pub fn blended_weights(&self, vi: usize) -> [f32; MAX_TEXTURE_LAYERS] {
        let mut out = [0.0; MAX_TEXTURE_LAYERS];
        let mut remaining = 1.0f32;
        for (i, layer) in self.layers.iter().enumerate().skip(1).rev() {
            let w = layer.weights[vi] as f32 / 255.0 * remaining;
            out[i] = w;
            remaining -= w;
        }
        if !self.layers.is_empty() {
            out[0] = remaining.max(0.0);
        }
        out
    }

    /// Copy one channel out of `rect` (clamped). For [`TerrainChannel::Pathing`]
    /// the rect is interpreted in cells.
    pub fn capture(&self, rect: VertexRect, kind: ChannelKind) -> TerrainPatch {
        let (gw, gh) = match kind {
            ChannelKind::Pathing => (self.width, self.height),
            _ => (self.verts_x(), self.verts_y()),
        };
        let r = VertexRect { x0: rect.x0.min(gw), y0: rect.y0.min(gh), x1: rect.x1.min(gw), y1: rect.y1.min(gh) };
        fn grab<T: Copy>(src: &[T], stride: u32, r: VertexRect) -> Vec<T> {
            let mut v = Vec::with_capacity((r.width() * r.height()) as usize);
            for y in r.y0..r.y1 {
                let s = (y * stride + r.x0) as usize;
                v.extend_from_slice(&src[s..s + r.width() as usize]);
            }
            v
        }
        let data = match kind {
            ChannelKind::Heights => TerrainChannel::Heights(grab(&self.heights, gw, r)),
            ChannelKind::CliffLevels => TerrainChannel::CliffLevels(grab(&self.cliff_levels, gw, r)),
            ChannelKind::Layers => {
                TerrainChannel::Layers(self.layers.iter().map(|l| grab(&l.weights, gw, r)).collect())
            }
            ChannelKind::Pathing => TerrainChannel::Pathing(grab(&self.pathing, gw, r)),
        };
        TerrainPatch { rect: r, data }
    }

    /// Write a patch back. Layer patches that reference more layers than
    /// exist are applied to the existing layers only.
    pub fn apply(&mut self, patch: &TerrainPatch) {
        let r = patch.rect;
        fn put<T: Copy>(dst: &mut [T], stride: u32, r: VertexRect, src: &[T]) {
            let w = r.width() as usize;
            for (row, y) in (r.y0..r.y1).enumerate() {
                let d = (y * stride + r.x0) as usize;
                dst[d..d + w].copy_from_slice(&src[row * w..row * w + w]);
            }
        }
        let vx = self.verts_x();
        match &patch.data {
            TerrainChannel::Heights(v) => put(&mut self.heights, vx, r, v),
            TerrainChannel::CliffLevels(v) => put(&mut self.cliff_levels, vx, r, v),
            TerrainChannel::Layers(ls) => {
                for (layer, v) in self.layers.iter_mut().zip(ls) {
                    put(&mut layer.weights, vx, r, v);
                }
            }
            TerrainChannel::Pathing(v) => put(&mut self.pathing, self.width, r, v),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelKind {
    Heights,
    CliffLevels,
    Layers,
    Pathing,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_apply_roundtrip() {
        let mut t = Terrain::new(16, 16);
        let r = VertexRect { x0: 2, y0: 3, x1: 6, y1: 9 };
        let before = t.capture(r, ChannelKind::Heights);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let i = t.vertex_index(x, y);
                t.heights[i] = 99.0;
            }
        }
        t.apply(&before);
        assert!(t.heights.iter().all(|&h| h == t.heights[0]));
    }

    #[test]
    fn sample_is_bilinear() {
        let mut t = Terrain::new(2, 2);
        let i = t.vertex_index(1, 1);
        t.heights.iter_mut().for_each(|h| *h = 0.0);
        t.heights[i] = 4.0;
        assert_eq!(t.sample_height(1.0, 1.0), 4.0);
        assert_eq!(t.sample_height(0.5, 1.0), 2.0);
    }
}
