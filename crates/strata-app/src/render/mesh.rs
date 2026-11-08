//! CPU side of the terrain mesh: vertex building, texture palette, and
//! change detection so only dirty rows are re-uploaded to the GPU.

use strata_core::terrain::{Terrain, VertexRect};

/// One terrain vertex as uploaded to the GPU (36 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    /// Blended placeholder color (sRGB) in rgb; cliff level * 16 in alpha.
    pub color: [u8; 4],
    /// Effective weight of each texture layer (0..=255, summing to ~255),
    /// for blending the real terrain textures in the shader.
    pub weights: [u8; 8],
}

/// Deterministic display color for a terrain texture id. Without Blizzard's
/// textures we hash the id into an earthy palette, nudged by keywords in the
/// id so "Grass" reads green and "Rock" reads grey.
pub fn texture_color(id: &str) -> [u8; 3] {
    let h = fnv1a(id.as_bytes());
    let lower = id.to_ascii_lowercase();
    const GREENS: [[u8; 3]; 4] = [[104, 128, 62], [86, 112, 58], [128, 140, 70], [74, 98, 66]];
    const BROWNS: [[u8; 3]; 5] = [[138, 110, 80], [120, 94, 70], [156, 128, 92], [108, 86, 66], [146, 116, 84]];
    const GREYS: [[u8; 3]; 4] = [[118, 116, 112], [98, 100, 102], [134, 128, 120], [84, 86, 90]];
    const SANDS: [[u8; 3]; 3] = [[188, 166, 124], [176, 152, 110], [198, 180, 140]];
    const SNOWS: [[u8; 3]; 2] = [[214, 220, 226], [196, 206, 216]];
    const DARKS: [[u8; 3]; 3] = [[64, 58, 56], [78, 66, 60], [56, 54, 60]];
    const CROPS: [[u8; 3]; 2] = [[164, 150, 74], [150, 136, 66]];
    const EARTHY: [[u8; 3]; 10] = [
        [138, 110, 80],
        [104, 128, 62],
        [118, 116, 112],
        [176, 152, 110],
        [150, 96, 70],
        [96, 104, 88],
        [126, 98, 88],
        [160, 132, 96],
        [88, 110, 96],
        [140, 124, 104],
    ];
    let pick = |set: &[[u8; 3]]| set[(h as usize) % set.len()];
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    let base = if has(&["snow", "ice", "frost"]) {
        pick(&SNOWS)
    } else if has(&["crop"]) {
        pick(&CROPS)
    } else if has(&["grass", "foliage", "moss", "jungle", "leaf", "leaves"]) {
        pick(&GREENS)
    } else if has(&["sand", "beach", "dune"]) {
        pick(&SANDS)
    } else if has(&["lava", "ash", "burnt", "black", "tar", "creep"]) {
        pick(&DARKS)
    } else if has(&["rock", "stone", "cliff", "concrete", "brick", "metal", "plate", "tile", "road"]) {
        pick(&GREYS)
    } else if has(&["dirt", "mud", "soil", "ground"]) {
        pick(&BROWNS)
    } else {
        pick(&EARTHY)
    };
    // Small deterministic lightness jitter so sibling textures differ.
    let j = ((h >> 8) % 21) as i32 - 10;
    base.map(|c| (c as i32 + j).clamp(0, 255) as u8)
}

fn fnv1a(bytes: &[u8]) -> u32 {
    let mut h: u32 = 0x811C_9DC5;
    for b in bytes {
        h ^= *b as u32;
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// Triangle indices for a `vx * vy` vertex grid (two triangles per cell).
pub fn grid_indices(vx: u32, vy: u32) -> Vec<u32> {
    let cells = (vx.saturating_sub(1) * vy.saturating_sub(1)) as usize;
    let mut out = Vec::with_capacity(cells * 6);
    for y in 0..vy.saturating_sub(1) {
        for x in 0..vx.saturating_sub(1) {
            let i = y * vx + x;
            let (a, b, c, d) = (i, i + 1, i + vx, i + vx + 1);
            out.extend_from_slice(&[a, b, d, a, d, c]);
        }
    }
    out
}

/// Snapshot of the terrain channels the mesh was built from.
#[derive(Default)]
struct Shadow {
    heights: Vec<f32>,
    cliffs: Vec<u8>,
    layer_ids: Vec<String>,
    layer_weights: Vec<Vec<u8>>,
    pathing: Vec<u8>,
}

/// What [`TerrainMesh::sync`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncResult {
    /// The grid size changed: rebuild everything (including indices).
    pub resized: bool,
    /// Vertex rows `[start, end)` that need re-uploading.
    pub rows: Option<(u32, u32)>,
    pub pathing_changed: bool,
}

pub struct TerrainMesh {
    pub verts_x: u32,
    pub verts_y: u32,
    pub vertices: Vec<Vertex>,
    layer_colors: Vec<[f32; 3]>,
    shadow: Shadow,
}

impl TerrainMesh {
    pub fn build(t: &Terrain) -> Self {
        let mut m =
            Self { verts_x: 0, verts_y: 0, vertices: Vec::new(), layer_colors: Vec::new(), shadow: Shadow::default() };
        m.rebuild(t);
        m
    }

    fn rebuild(&mut self, t: &Terrain) {
        self.verts_x = t.verts_x();
        self.verts_y = t.verts_y();
        self.vertices = vec![Vertex::zeroed_default(); (self.verts_x * self.verts_y) as usize];
        self.recolor_layers(t);
        self.shadow.heights = t.heights.clone();
        self.shadow.cliffs = t.cliff_levels.clone();
        self.shadow.pathing = t.pathing.clone();
        self.write_rect(t, t.full_rect());
    }

    fn recolor_layers(&mut self, t: &Terrain) {
        self.layer_colors = t.layers.iter().map(|l| texture_color(&l.texture).map(|c| c as f32)).collect();
        self.shadow.layer_ids = t.layers.iter().map(|l| l.texture.clone()).collect();
        self.shadow.layer_weights = t.layers.iter().map(|l| l.weights.clone()).collect();
    }

    /// Recompute vertices inside `rect` (vertex coords). Callers should pass
    /// a rect already grown by one vertex if neighbours' normals changed.
    fn write_rect(&mut self, t: &Terrain, rect: VertexRect) {
        let r = t.clamp_rect(rect);
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let vi = t.vertex_index(x, y);
                let w = t.blended_weights(vi);
                let weights = w.map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u8);
                let mut c = [0.0f32; 3];
                for (li, col) in self.layer_colors.iter().enumerate() {
                    let wi = w[li];
                    if wi > 0.0 {
                        c[0] += col[0] * wi;
                        c[1] += col[1] * wi;
                        c[2] += col[2] * wi;
                    }
                }
                let n = t.normal_at_vertex(x, y);
                self.vertices[vi] = Vertex {
                    pos: [x as f32, y as f32, t.heights[vi]],
                    normal: n.to_array(),
                    color: [
                        c[0].round().clamp(0.0, 255.0) as u8,
                        c[1].round().clamp(0.0, 255.0) as u8,
                        c[2].round().clamp(0.0, 255.0) as u8,
                        t.cliff_levels[vi].min(15) * 16,
                    ],
                    weights,
                };
            }
        }
    }

    /// Fast path during a brush stroke: rebuild the vertices around a rect the
    /// stroke reported and refresh the snapshot there. Returns rows to upload.
    pub fn update_rect(&mut self, t: &Terrain, rect: VertexRect) -> Option<(u32, u32)> {
        if t.verts_x() != self.verts_x || t.verts_y() != self.verts_y || rect.is_empty() {
            return None;
        }
        if t.layers.len() != self.shadow.layer_ids.len() {
            // A layer was added mid-stroke; fall back to a full diff.
            return self.sync(t).rows;
        }
        let r = t.clamp_rect(rect);
        let vx = self.verts_x as usize;
        for y in r.y0..r.y1 {
            let s = y as usize * vx + r.x0 as usize;
            let e = y as usize * vx + r.x1 as usize;
            self.shadow.heights[s..e].copy_from_slice(&t.heights[s..e]);
            self.shadow.cliffs[s..e].copy_from_slice(&t.cliff_levels[s..e]);
            for (sw, l) in self.shadow.layer_weights.iter_mut().zip(&t.layers) {
                sw[s..e].copy_from_slice(&l.weights[s..e]);
            }
        }
        let grown = grow(r, 1, self.verts_x, self.verts_y);
        self.write_rect(t, grown);
        Some((grown.y0, grown.y1))
    }

    /// Pathing changed under a stroke rect (cells); refresh the snapshot.
    pub fn take_pathing(&mut self, t: &Terrain) -> bool {
        if self.shadow.pathing != t.pathing {
            self.shadow.pathing.clone_from(&t.pathing);
            true
        } else {
            false
        }
    }

    /// Compare against the snapshot (after undo/redo/MCP/load) and rebuild
    /// whatever changed.
    pub fn sync(&mut self, t: &Terrain) -> SyncResult {
        if t.verts_x() != self.verts_x || t.verts_y() != self.verts_y {
            self.rebuild(t);
            return SyncResult { resized: true, rows: Some((0, self.verts_y)), pathing_changed: true };
        }
        let pathing_changed = self.take_pathing(t);
        let ids_changed = t.layers.len() != self.shadow.layer_ids.len()
            || t.layers.iter().zip(&self.shadow.layer_ids).any(|(l, id)| &l.texture != id);
        if ids_changed {
            self.recolor_layers(t);
            self.shadow.heights.clone_from(&t.heights);
            self.shadow.cliffs.clone_from(&t.cliff_levels);
            self.write_rect(t, t.full_rect());
            return SyncResult { resized: false, rows: Some((0, self.verts_y)), pathing_changed };
        }
        let vx = self.verts_x as usize;
        let mut dirty: Option<VertexRect> = None;
        let mut mark = |x0: usize, x1: usize, y: usize| {
            let r = VertexRect { x0: x0 as u32, y0: y as u32, x1: x1 as u32, y1: y as u32 + 1 };
            dirty = Some(dirty.map_or(r, |d| d.union(&r)));
        };
        for y in 0..self.verts_y as usize {
            let row = y * vx..(y + 1) * vx;
            let mut lo = usize::MAX;
            let mut hi = 0usize;
            let mut scan = |a: &dyn Fn(usize) -> bool| {
                for x in 0..vx {
                    if a(row.start + x) {
                        lo = lo.min(x);
                        hi = hi.max(x + 1);
                    }
                }
            };
            if t.heights[row.clone()] != self.shadow.heights[row.clone()] {
                scan(&|i| t.heights[i] != self.shadow.heights[i]);
            }
            if t.cliff_levels[row.clone()] != self.shadow.cliffs[row.clone()] {
                scan(&|i| t.cliff_levels[i] != self.shadow.cliffs[i]);
            }
            for (l, sw) in t.layers.iter().zip(&self.shadow.layer_weights) {
                if l.weights[row.clone()] != sw[row.clone()] {
                    scan(&|i| l.weights[i] != sw[i]);
                }
            }
            if lo < hi {
                mark(lo, hi, y);
            }
        }
        let rows = dirty.map(|d| {
            for y in d.y0..d.y1 {
                let s = (y * self.verts_x + d.x0) as usize;
                let e = (y * self.verts_x + d.x1) as usize;
                self.shadow.heights[s..e].copy_from_slice(&t.heights[s..e]);
                self.shadow.cliffs[s..e].copy_from_slice(&t.cliff_levels[s..e]);
                for (sw, l) in self.shadow.layer_weights.iter_mut().zip(&t.layers) {
                    sw[s..e].copy_from_slice(&l.weights[s..e]);
                }
            }
            let g = grow(d, 1, self.verts_x, self.verts_y);
            self.write_rect(t, g);
            (g.y0, g.y1)
        });
        SyncResult { resized: false, rows, pathing_changed }
    }

    /// Bytes of vertex rows `[y0, y1)` and their byte offset.
    pub fn row_bytes(&self, y0: u32, y1: u32) -> (usize, &[u8]) {
        let s = (y0 * self.verts_x) as usize;
        let e = (y1.min(self.verts_y) * self.verts_x) as usize;
        (s * std::mem::size_of::<Vertex>(), bytemuck::cast_slice(&self.vertices[s..e]))
    }
}

impl Vertex {
    fn zeroed_default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

/// Grow a rect by `n` on every side, clamped to `[0, w) x [0, h)`.
pub fn grow(r: VertexRect, n: u32, w: u32, h: u32) -> VertexRect {
    VertexRect { x0: r.x0.saturating_sub(n), y0: r.y0.saturating_sub(n), x1: (r.x1 + n).min(w), y1: (r.y1 + n).min(h) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::brush::{Brush, BrushTool, Stroke};

    #[test]
    fn vertex_is_36_bytes() {
        assert_eq!(std::mem::size_of::<Vertex>(), 36);
    }

    #[test]
    fn weights_follow_layers() {
        let mut t = Terrain::new(4, 4);
        let n = t.heights.len();
        let mut w = vec![0u8; n];
        w[t.vertex_index(2, 2)] = 255;
        t.layers.push(strata_core::terrain::TextureLayer { texture: "Grass".into(), weights: w });
        let m = TerrainMesh::build(&t);
        assert_eq!(m.vertices[t.vertex_index(2, 2)].weights[..2], [0, 255]);
        assert_eq!(m.vertices[0].weights[..2], [255, 0]);
    }

    #[test]
    fn indices_cover_every_cell() {
        let idx = grid_indices(3, 3);
        assert_eq!(idx.len(), 4 * 6);
        assert!(idx.iter().all(|&i| i < 9));
    }

    #[test]
    fn mesh_matches_terrain() {
        let t = Terrain::new(8, 4);
        let m = TerrainMesh::build(&t);
        assert_eq!(m.vertices.len(), 9 * 5);
        let v = m.vertices[t.vertex_index(3, 2)];
        assert_eq!(v.pos, [3.0, 2.0, t.heights[0]]);
        assert_eq!(v.normal, [0.0, 0.0, 1.0]);
        assert_eq!(&v.color[..3], &texture_color("CharDirt"));
        assert_eq!(v.color[3], t.cliff_levels[0] * 16);
    }

    #[test]
    fn sync_finds_only_the_changed_rows() {
        let mut t = Terrain::new(64, 64);
        let mut m = TerrainMesh::build(&t);
        assert_eq!(m.sync(&t), SyncResult::default());
        let mut s = Stroke::begin(&t, Brush { radius: 2.0, ..Default::default() });
        s.dab(&mut t, glam::Vec2::new(20.0, 30.0));
        let r = m.sync(&t);
        let (y0, y1) = r.rows.unwrap();
        assert!(y0 >= 26 && y1 <= 35, "{y0}..{y1}");
        assert_eq!(m.sync(&t), SyncResult::default(), "second sync is clean");
        // Mesh equals a fresh build.
        assert_eq!(m.vertices, TerrainMesh::build(&t).vertices);
    }

    #[test]
    fn stroke_rect_fast_path_matches_full_build() {
        let mut t = Terrain::new(32, 32);
        let mut m = TerrainMesh::build(&t);
        let mut s = Stroke::begin(&t, Brush { radius: 3.0, tool: BrushTool::Raise, ..Default::default() });
        let r = s.dab(&mut t, glam::Vec2::new(10.0, 12.0));
        m.update_rect(&t, r).unwrap();
        assert_eq!(m.vertices, TerrainMesh::build(&t).vertices);
        assert_eq!(m.sync(&t).rows, None, "snapshot was refreshed");
    }

    #[test]
    fn texture_rename_recolors() {
        let mut t = Terrain::new(4, 4);
        let mut m = TerrainMesh::build(&t);
        t.layers[0].texture = "AgriaGrassGreen".into();
        let r = m.sync(&t);
        assert_eq!(r.rows, Some((0, 5)));
        assert_eq!(&m.vertices[0].color[..3], &texture_color("AgriaGrassGreen"));
    }

    #[test]
    fn palette_is_deterministic_and_keyword_aware() {
        assert_eq!(texture_color("AgriaGrassGreen"), texture_color("AgriaGrassGreen"));
        let g = texture_color("AgriaGrassGreen");
        assert!(g[1] > g[0] && g[1] > g[2], "grass is green: {g:?}");
        let r = texture_color("AgriaRock");
        assert!((r[0] as i32 - r[2] as i32).abs() < 30, "rock is grey: {r:?}");
    }

    #[test]
    fn resize_rebuilds() {
        let t = Terrain::new(8, 8);
        let mut m = TerrainMesh::build(&t);
        let t2 = Terrain::new(16, 8);
        let r = m.sync(&t2);
        assert!(r.resized);
        assert_eq!(m.vertices.len(), 17 * 9);
    }
}
