//! The real StarCraft II terrain textures behind each texture layer.
//!
//! Each frame [`TerrainTextures::update`] maps the terrain's layer ids
//! ("Zerus3") to their diffuse and normal maps through the base game's
//! `CTerrainTex` data, asks the asset service for them, and queues the ones
//! that arrived for upload into two GPU texture arrays (one slot per layer).
//! Until a layer's texture is uploaded, or when there is no game install, the
//! shader paints that layer with its placeholder color instead.

use strata_core::terrain::{Terrain, MAX_TEXTURE_LAYERS};

use crate::assets::{Asset, Assets};

/// Side of every texture-array layer, in pixels.
pub const TEX_SIZE: u32 = 512;

/// Texture repeats per cell. SC2 terrain textures repeat every 32/3 cells
/// (maps store `uvtiling="24 24"` for 256 cells, `18.75 16.5` for 200x176).
pub const REPEATS_PER_CELL: f32 = 3.0 / 32.0;

/// Layer flags passed to the shader in `FrameParams::layers[i][3]`.
pub const HAS_DIFFUSE: f32 = 1.0;
pub const HAS_NORMAL: f32 = 2.0;

/// One image to copy into a texture-array slot.
pub struct LayerUpload {
    pub slot: u32,
    pub normal: bool,
    /// `TEX_SIZE` x `TEX_SIZE` RGBA8.
    pub rgba: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Waiting,
    Uploaded,
    Missing,
}

#[derive(Debug, Clone)]
struct Slot {
    id: String,
    diffuse: Part,
    normal: Part,
    /// Mean color of the diffuse map (for the minimap).
    mean: Option<[u8; 3]>,
}

impl Slot {
    fn new(id: &str) -> Self {
        Self { id: id.to_owned(), diffuse: Part::Waiting, normal: Part::Waiting, mean: None }
    }
}

#[derive(Default)]
pub struct TerrainTextures {
    slots: Vec<Slot>,
    /// Asset-service revision last seen with work still pending.
    seen: Option<u64>,
    available: bool,
    /// Bumped whenever a layer's mean color changes.
    pub colors_revision: u64,
}

impl TerrainTextures {
    /// Poll the asset service and queue uploads for textures that arrived.
    /// Returns the shader's per-layer parameters: placeholder color in rgb,
    /// `HAS_DIFFUSE | HAS_NORMAL` flags in a.
    pub fn update(
        &mut self,
        assets: &Assets,
        t: &Terrain,
        uploads: &mut Vec<LayerUpload>,
    ) -> [[f32; 4]; MAX_TEXTURE_LAYERS] {
        let n = t.layers.len().min(MAX_TEXTURE_LAYERS);
        let available = assets.available();
        if available != self.available {
            // Attached or detached: start over.
            self.available = available;
            self.slots.clear();
            self.seen = None;
            self.colors_revision += 1;
        }
        self.slots.truncate(n);
        for (i, layer) in t.layers.iter().take(n).enumerate() {
            match self.slots.get_mut(i) {
                Some(s) if s.id == layer.texture => {}
                Some(s) => {
                    *s = Slot::new(&layer.texture);
                    self.seen = None;
                    self.colors_revision += 1;
                }
                None => {
                    self.slots.push(Slot::new(&layer.texture));
                    self.seen = None;
                }
            }
        }
        let pending = self.slots.iter().any(|s| s.diffuse == Part::Waiting || s.normal == Part::Waiting);
        let rev = assets.revision();
        if available && pending && self.seen != Some(rev) {
            self.seen = Some(rev);
            for (i, s) in self.slots.iter_mut().enumerate() {
                if s.diffuse != Part::Waiting && s.normal != Part::Waiting {
                    continue;
                }
                let tt = match assets.terrain_texture(&s.id) {
                    Asset::Loading => continue,
                    Asset::Missing => {
                        if !s.id.is_empty() {
                            log::info!("terrain texture {:?} is not in the game data", s.id);
                        }
                        s.diffuse = Part::Missing;
                        s.normal = Part::Missing;
                        continue;
                    }
                    Asset::Ready(tt) => tt,
                };
                for normal in [false, true] {
                    let (part, path) =
                        if normal { (&mut s.normal, &tt.normal) } else { (&mut s.diffuse, &tt.diffuse) };
                    if *part != Part::Waiting {
                        continue;
                    }
                    let Some(path) = path.as_deref().and_then(|p| assets.resolve(p)) else {
                        *part = Part::Missing;
                        continue;
                    };
                    match assets.texture(&path, TEX_SIZE) {
                        Asset::Loading => {}
                        Asset::Missing => *part = Part::Missing,
                        Asset::Ready(img) => {
                            if !normal {
                                s.mean = Some(mean_color(&img));
                                self.colors_revision += 1;
                            }
                            uploads.push(LayerUpload { slot: i as u32, normal, rgba: fit(&img, TEX_SIZE) });
                            *part = Part::Uploaded;
                        }
                    }
                }
            }
        }
        let mut out = [[0.0; 4]; MAX_TEXTURE_LAYERS];
        for (i, layer) in t.layers.iter().take(n).enumerate() {
            let c = super::mesh::texture_color(&layer.texture);
            let mut flags = 0.0;
            if let Some(s) = self.slots.get(i) {
                if s.diffuse == Part::Uploaded {
                    flags += HAS_DIFFUSE;
                }
                if s.normal == Part::Uploaded {
                    flags += HAS_NORMAL;
                }
            }
            out[i] = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, flags];
        }
        out
    }

    /// Mean diffuse color of each layer whose texture is loaded.
    pub fn layer_colors(&self) -> Vec<Option<[u8; 3]>> {
        self.slots.iter().map(|s| s.mean).collect()
    }

    /// Whether some layer's texture is still on its way.
    pub fn pending(&self) -> bool {
        self.available && self.slots.iter().any(|s| s.diffuse == Part::Waiting)
    }

    /// Whether any layer shows a real texture.
    pub fn any_textured(&self) -> bool {
        self.slots.iter().any(|s| s.diffuse == Part::Uploaded)
    }
}

/// Average color of an image (sampled on a sparse grid).
fn mean_color(img: &strata_dds::Image) -> [u8; 3] {
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    let step = (img.width.max(img.height) / 64).max(1);
    for y in (0..img.height).step_by(step as usize) {
        for x in (0..img.width).step_by(step as usize) {
            let i = ((y * img.width + x) * 4) as usize;
            for (acc, &v) in sum.iter_mut().zip(&img.rgba[i..i + 3]) {
                *acc += v as u64;
            }
            n += 1;
        }
    }
    sum.map(|s| (s / n.max(1)) as u8)
}

/// The image scaled to `side` x `side` (box filter when shrinking, nearest
/// when growing; textures are square powers of two in practice).
pub fn fit(img: &strata_dds::Image, side: u32) -> Vec<u8> {
    if img.width == side && img.height == side {
        return img.rgba.clone();
    }
    let mut out = vec![0u8; (side * side * 4) as usize];
    let fx = img.width as f32 / side as f32;
    let fy = img.height as f32 / side as f32;
    let (bx, by) = (fx.max(1.0).round() as u32, fy.max(1.0).round() as u32);
    for y in 0..side {
        for x in 0..side {
            let sx0 = ((x as f32 * fx) as u32).min(img.width - 1);
            let sy0 = ((y as f32 * fy) as u32).min(img.height - 1);
            let mut acc = [0u32; 4];
            let mut cnt = 0;
            for sy in sy0..(sy0 + by).min(img.height) {
                for sx in sx0..(sx0 + bx).min(img.width) {
                    let i = ((sy * img.width + sx) * 4) as usize;
                    for (a, &v) in acc.iter_mut().zip(&img.rgba[i..i + 4]) {
                        *a += v as u32;
                    }
                    cnt += 1;
                }
            }
            let o = ((y * side + x) * 4) as usize;
            for (d, a) in out[o..o + 4].iter_mut().zip(acc) {
                *d = (a / cnt.max(1)) as u8;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> strata_dds::Image {
        strata_dds::Image { width: w, height: h, rgba: c.repeat((w * h) as usize) }
    }

    #[test]
    fn fit_scales_both_ways() {
        let big = solid(1024, 1024, [10, 20, 30, 40]);
        let out = fit(&big, 512);
        assert_eq!(out.len(), 512 * 512 * 4);
        assert_eq!(&out[..4], &[10, 20, 30, 40]);
        let small = solid(128, 64, [1, 2, 3, 4]);
        let out = fit(&small, 512);
        assert_eq!(out.len(), 512 * 512 * 4);
        assert_eq!(&out[out.len() - 4..], &[1, 2, 3, 4]);
    }

    #[test]
    fn mean_of_solid_image() {
        assert_eq!(mean_color(&solid(64, 64, [100, 50, 25, 255])), [100, 50, 25]);
    }

    #[test]
    fn without_game_data_layers_use_placeholders() {
        let mut tt = TerrainTextures::default();
        let t = Terrain::new(8, 8);
        let mut up = Vec::new();
        let p = tt.update(&Assets::default(), &t, &mut up);
        assert!(up.is_empty());
        assert_eq!(p[0][3], 0.0, "no texture flags");
        let c = super::super::mesh::texture_color("CharDirt");
        assert_eq!(p[0][0], c[0] as f32 / 255.0);
        assert!(!tt.any_textured());
    }
}
