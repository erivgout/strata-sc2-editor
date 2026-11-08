//! Strata M3: reads Blizzard `.m3` models (StarCraft II units, buildings,
//! doodads) into a static, render-ready triangle mesh. Pure Rust, no I/O,
//! compiles for wasm32.
//!
//! Only what a map editor needs to *show* a model is read: vertices
//! (position, normal, first UV set), the faces of the first mesh division,
//! its regions and batches, and the standard materials those batches use
//! (diffuse, emissive and alpha-mask texture paths, blend mode, flags). No
//! animation: the mesh comes out in the model's default pose (each bone's
//! un-animated location/rotation/scale applied through the skin), or in the
//! raw bind pose with [`Pose::Bind`].
//!
//! # File layout (MD34)
//!
//! An M3 file is a header, a block of tagged arrays and an index of those
//! arrays at the end. The `MD34` header (stored reversed as `43DM`) gives the
//! index position and a reference to the `MODL` entry. Every reference is
//! `(count, index entry, flags)`; an index entry is `(tag, offset,
//! repetitions, version)`, the tag again stored reversed. Struct layouts
//! depend on each entry's version. Layouts follow the community format
//! description of the Blender add-on m3studio / m3addon (`structures.xml`).
//!
//! Every read is bounds-checked: malformed files return an [`M3Error`],
//! they never panic.

use glam::{Mat4, Quat, Vec3};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum M3Error {
    #[error("not an M3 model")]
    NotM3,
    #[error("truncated or corrupt M3 data: {0}")]
    Corrupt(String),
    #[error("unsupported M3 data: {0}")]
    Unsupported(String),
}

type Result<T> = std::result::Result<T, M3Error>;

fn corrupt(what: impl Into<String>) -> M3Error {
    M3Error::Corrupt(what.into())
}

/// Which pose the mesh is produced in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pose {
    /// The first frame of the model's `Stand` animation, the way the game
    /// and the editor show an idle unit: bones (and the batches they toggle)
    /// take their keyed values, everything else its default. Models without
    /// a `Stand` animation fall back to [`Pose::Default`].
    #[default]
    Stand,
    /// Every bone at its default (un-animated) location, rotation and scale,
    /// applied through the skin.
    Default,
    /// The vertex positions exactly as stored (the skin's bind pose).
    Bind,
}

/// How a material's output is combined with what is behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blend {
    Opaque,
    AlphaBlend,
    Add,
    AlphaAdd,
    Mod,
    Mod2x,
    /// A blend mode this crate does not know.
    Other(u32),
}

impl Blend {
    fn from_raw(v: u32) -> Blend {
        match v {
            0 => Blend::Opaque,
            1 => Blend::AlphaBlend,
            2 => Blend::Add,
            3 => Blend::AlphaAdd,
            4 => Blend::Mod,
            5 => Blend::Mod2x,
            v => Blend::Other(v),
        }
    }

    /// Whether the material writes depth and needs no sorting.
    pub fn is_opaque(self) -> bool {
        matches!(self, Blend::Opaque)
    }
}

/// One texture layer of a material.
#[derive(Debug, Clone, PartialEq)]
pub struct Layer {
    /// Texture path as written in the model (e.g. `Assets\Textures\Marine_Diffuse.dds`),
    /// `None` for a constant-color layer.
    pub path: Option<String>,
    /// Constant color (RGBA), used when there is no bitmap.
    pub color: [u8; 4],
    /// Raw `LAYR.flags` (0x4 wrap U, 0x8 wrap V, 0x10 invert, 0x400 constant color).
    pub flags: u32,
    /// UV source (0 = first UV set, 1 = second ...).
    pub uv_source: u32,
    /// Color channels: 0 RGB, 1 RGBA, 2 A, 3 R, 4 G, 5 B.
    pub channels: u32,
}

/// What kind of material a batch uses. Only standard materials (and
/// composites, resolved to their first standard section) are drawable here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialKind {
    Standard,
    /// Displacement (2), terrain (4), volume (5), creep (7), ... by M3 type id.
    Other(u32),
}

/// A material, reduced to what a viewer needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub name: String,
    pub kind: MaterialKind,
    pub diffuse: Option<Layer>,
    pub emissive: Option<Layer>,
    /// Cutout / transparency mask (often the diffuse texture's alpha).
    pub alpha_mask: Option<Layer>,
    pub blend: Blend,
    /// Raw `MAT_.flags` (0x8 two-sided, 0x10 unshaded, 0x4 unfogged ...).
    pub flags: u32,
    /// Alpha-test threshold (0..=255); 0 means no cutout.
    pub alpha_test: u8,
    /// HDR multiplier of the emissive layers.
    pub emissive_mult: f32,
    /// How the first emissive layer combines: 2 add, 4 team-color emissive add ...
    pub emissive_blend: u32,
}

impl Material {
    pub fn two_sided(&self) -> bool {
        self.flags & 0x8 != 0
    }

    pub fn unshaded(&self) -> bool {
        self.flags & 0x10 != 0
    }

    fn other(kind: u32) -> Material {
        Material {
            name: String::new(),
            kind: MaterialKind::Other(kind),
            diffuse: None,
            emissive: None,
            alpha_mask: None,
            blend: Blend::Opaque,
            flags: 0,
            alpha_test: 0,
            emissive_mult: 1.0,
            emissive_blend: 2,
        }
    }
}

/// A run of [`Mesh::indices`] drawn with one material.
#[derive(Debug, Clone, PartialEq)]
pub struct Submesh {
    pub first_index: u32,
    pub index_count: u32,
    pub material: Material,
}

/// Axis-aligned bounds in model space (SC2 world units, Z up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub radius: f32,
}

/// A static triangle mesh. All per-vertex arrays have the same length.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    /// First UV set, DirectX convention (v grows downward, so it matches
    /// image rows stored top to bottom).
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list into the vertex arrays.
    pub indices: Vec<u32>,
    pub submeshes: Vec<Submesh>,
    /// Bounds of the vertices the submeshes use.
    pub bounds: Bounds,
    /// `MODL` struct version, for diagnostics.
    pub version: u32,
}

impl Mesh {
    pub fn triangle_count(&self) -> usize {
        self.submeshes.iter().map(|s| s.index_count as usize / 3).sum()
    }
}

/// Parse an `.m3` file in its idle pose ([`Pose::Stand`]).
pub fn parse(bytes: &[u8]) -> Result<Mesh> {
    parse_with(bytes, Pose::Stand)
}

/// Parse an `.m3` file in the given pose.
pub fn parse_with(bytes: &[u8], pose: Pose) -> Result<Mesh> {
    let file = File::open(bytes)?;
    file.mesh(pose)
}

// ---------------------------------------------------------------------------
// Low-level reading

/// Tags as stored in the file (reversed).
const fn tag(s: &[u8; 4]) -> [u8; 4] {
    [s[3], s[2], s[1], s[0]]
}

const T_MODL: [u8; 4] = tag(b"MODL");
const T_U8: [u8; 4] = tag(b"U8__");
const T_U16: [u8; 4] = tag(b"U16_");
const T_U32: [u8; 4] = tag(b"U32_");
const T_CHAR: [u8; 4] = tag(b"CHAR");
const T_DIV: [u8; 4] = tag(b"DIV_");
const T_REGN: [u8; 4] = tag(b"REGN");
const T_BAT: [u8; 4] = tag(b"BAT_");
const T_MATM: [u8; 4] = tag(b"MATM");
const T_MAT: [u8; 4] = tag(b"MAT_");
const T_LAYR: [u8; 4] = tag(b"LAYR");
const T_CMP: [u8; 4] = tag(b"CMP_");
const T_CMS: [u8; 4] = tag(b"CMS_");
const T_BONE: [u8; 4] = tag(b"BONE");
const T_IREF: [u8; 4] = tag(b"IREF");

/// Upper bound for any element count, against absurd allocations.
const MAX_ITEMS: usize = 1 << 24;
/// Cap on emitted triangle indices across all regions (SC2's largest models
/// have well under a million). Tiny under test so the limit is testable.
#[cfg(not(test))]
const MAX_INDICES: usize = 3 << 20;
#[cfg(test)]
const MAX_INDICES: usize = 600;

#[derive(Debug, Clone, Copy)]
struct Entry {
    tag: [u8; 4],
    offset: usize,
    count: usize,
    version: u32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Ref {
    entries: u32,
    index: u32,
}

/// A located array: `count` items of `stride` bytes at `offset`.
#[derive(Debug, Clone, Copy)]
struct Items {
    offset: usize,
    count: usize,
    stride: usize,
    version: u32,
}

impl Items {
    fn at(&self, i: usize) -> usize {
        self.offset + i * self.stride
    }
}

struct File<'a> {
    b: &'a [u8],
    index: Vec<Entry>,
}

fn tag_name(t: [u8; 4]) -> String {
    let r = [t[3], t[2], t[1], t[0]];
    String::from_utf8_lossy(&r).into_owned()
}

impl<'a> File<'a> {
    fn open(b: &'a [u8]) -> Result<File<'a>> {
        match b.get(0..4) {
            Some(t) if t == tag(b"MD34") => {}
            Some(t) if t == tag(b"MD33") => {
                return Err(M3Error::Unsupported("MD33 (StarCraft II beta) models".into()));
            }
            _ => return Err(M3Error::NotM3),
        }
        let mut f = File { b, index: Vec::new() };
        let index_offset = f.u32(4)? as usize;
        let index_size = f.u32(8)? as usize;
        let end = index_size
            .checked_mul(16)
            .and_then(|n| n.checked_add(index_offset))
            .ok_or_else(|| corrupt("index size"))?;
        if end > b.len() || index_size > MAX_ITEMS {
            return Err(corrupt("index runs past the end of the file"));
        }
        let mut index = Vec::with_capacity(index_size);
        for i in 0..index_size {
            let o = index_offset + i * 16;
            index.push(Entry {
                tag: [b[o], b[o + 1], b[o + 2], b[o + 3]],
                offset: f.u32(o + 4)? as usize,
                count: f.u32(o + 8)? as usize,
                version: f.u32(o + 12)?,
            });
        }
        f.index = index;
        Ok(f)
    }

    fn bytes(&self, off: usize, n: usize) -> Result<&'a [u8]> {
        off.checked_add(n)
            .and_then(|end| self.b.get(off..end))
            .ok_or_else(|| corrupt(format!("read of {n} bytes at {off} is out of range")))
    }

    fn u8(&self, off: usize) -> Result<u8> {
        Ok(self.bytes(off, 1)?[0])
    }

    fn u16(&self, off: usize) -> Result<u16> {
        let s = self.bytes(off, 2)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    fn i16(&self, off: usize) -> Result<i16> {
        Ok(self.u16(off)? as i16)
    }

    fn u32(&self, off: usize) -> Result<u32> {
        let s = self.bytes(off, 4)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    fn f32(&self, off: usize) -> Result<f32> {
        Ok(f32::from_bits(self.u32(off)?))
    }

    fn vec3(&self, off: usize) -> Result<Vec3> {
        Ok(Vec3::new(self.f32(off)?, self.f32(off + 4)?, self.f32(off + 8)?))
    }

    fn reference(&self, off: usize) -> Result<Ref> {
        Ok(Ref { entries: self.u32(off)?, index: self.u32(off + 4)? })
    }

    /// The array a reference points at, checked against the expected tag.
    /// `stride` maps the entry's version to its struct size (`None` =
    /// unsupported version). Returns `None` for an empty reference.
    fn items(&self, r: Ref, want: [u8; 4], stride: impl Fn(u32) -> Option<usize>) -> Result<Option<Items>> {
        if r.entries == 0 {
            return Ok(None);
        }
        let e = self
            .index
            .get(r.index as usize)
            .ok_or_else(|| corrupt(format!("reference to missing index entry {}", r.index)))?;
        if e.tag != want {
            return Err(corrupt(format!("expected {} but found {}", tag_name(want), tag_name(e.tag))));
        }
        let count = r.entries as usize;
        if count > e.count || count > MAX_ITEMS {
            return Err(corrupt(format!("{}: {count} items but the entry holds {}", tag_name(want), e.count)));
        }
        let stride = stride(e.version)
            .ok_or_else(|| M3Error::Unsupported(format!("{} version {}", tag_name(want), e.version)))?;
        let items = Items { offset: e.offset, count, stride, version: e.version };
        self.bytes(e.offset, count.checked_mul(stride).ok_or_else(|| corrupt("array size"))?)?;
        Ok(Some(items))
    }

    fn string(&self, r: Ref) -> String {
        match self.items(r, T_CHAR, |_| Some(1)) {
            Ok(Some(it)) => {
                let s = &self.b[it.offset..it.offset + it.count];
                let s = s.split(|&c| c == 0).next().unwrap_or(&[]);
                String::from_utf8_lossy(s).into_owned()
            }
            _ => String::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Model

    fn mesh(&self, pose: Pose) -> Result<Mesh> {
        let model_ref = self.reference(12)?;
        let modl = self
            .items(model_ref, T_MODL, |v| (20..=30).contains(&v).then_some(0))?
            .ok_or_else(|| corrupt("no MODL entry"))?;
        let m = Modl::layout(modl.offset, modl.version);
        self.bytes(modl.offset, m.size)?;

        let name = self.string(self.reference(m.at)?);
        let vflags = self.u32(m.at + 96)?;
        let layout = VertexLayout::new(vflags);
        let vref = self.reference(m.at + 100)?;
        // Effect-only models (particles, lights) have no vertices at all.
        let Some(vert) = self.items(vref, T_U8, |_| Some(1))? else {
            return Ok(Mesh::empty(name, modl.version));
        };
        if layout.stride == 0 || vert.count % layout.stride != 0 {
            return Err(M3Error::Unsupported(format!(
                "vertex format 0x{vflags:x} ({} bytes per vertex) does not divide {} bytes of vertex data",
                layout.stride, vert.count
            )));
        }
        let vcount = vert.count / layout.stride;

        let div = self
            .items(self.reference(m.at + 112)?, T_DIV, |v| (v == 2).then_some(52))?
            .ok_or_else(|| corrupt("model has no mesh division"))?;
        let d = div.at(0);
        let faces = self.items(self.reference(d)?, T_U16, |_| Some(2))?;
        let regions = self.items(self.reference(d + 12)?, T_REGN, |v| match v {
            2 => Some(28),
            3 => Some(36),
            4 => Some(40),
            5 => Some(48),
            _ => None,
        })?;
        let batches = self.items(self.reference(d + 24)?, T_BAT, |v| (v == 1).then_some(14))?;
        let (Some(faces), Some(regions), Some(batches)) = (faces, regions, batches) else {
            return Ok(Mesh::empty(name, modl.version));
        };

        let bone_lookup: Vec<u16> = match self.items(self.reference(m.at + 124)?, T_U16, |_| Some(2))? {
            Some(it) => (0..it.count).map(|i| self.u16(it.at(i))).collect::<Result<_>>()?,
            None => Vec::new(),
        };
        let mut bones = self.bones(&m)?;
        if pose == Pose::Stand {
            // Animation data is optional polish: a malformed track leaves the defaults.
            if let Ok(Some(tracks)) = self.stand_tracks(&m) {
                for b in &mut bones {
                    b.apply(self, &tracks);
                }
            }
        }
        let skin = match pose {
            Pose::Stand | Pose::Default => self.skin_matrices(&m, &bones),
            Pose::Bind => None,
        };

        let mut positions = Vec::with_capacity(vcount);
        let mut normals = Vec::with_capacity(vcount);
        let mut uvs = vec![[0.0f32; 2]; vcount];
        for i in 0..vcount {
            let o = vert.offset + i * layout.stride;
            positions.push(self.vec3(o)?);
            normals.push(layout.normal(self, o)?);
        }

        let mut regs = Vec::with_capacity(regions.count);
        for i in 0..regions.count {
            regs.push(Region::read(self, regions.at(i), regions.version)?);
        }

        // UVs, region by region (the int16 scale is per region), and skinning.
        let mut skinned = vec![false; vcount];
        for r in &regs {
            let (a, n) = (r.first_vertex as usize, r.vertex_count as usize);
            if a.checked_add(n).is_none_or(|e| e > vcount) {
                return Err(corrupt("region vertex range is out of bounds"));
            }
            for v in a..a + n {
                let o = vert.offset + v * layout.stride;
                uvs[v] = layout.uv(self, o, r.uv_multiply, r.uv_offset)?;
                if let Some(mats) = &skin {
                    if !skinned[v] {
                        skinned[v] = true;
                        let (p, nrm) = layout.skin(self, o, r, &bone_lookup, mats, positions[v], normals[v])?;
                        positions[v] = p;
                        normals[v] = nrm;
                    }
                }
            }
        }

        let face_at = |i: usize| self.u16(faces.at(i));
        let mut indices = Vec::new();
        let mut region_ranges: Vec<Option<(u32, u32)>> = vec![None; regs.len()];
        let mut submeshes = Vec::new();
        let mut used = vec![false; vcount];
        let mut matcache: Vec<Option<Material>> = Vec::new();
        for bi in 0..batches.count {
            let o = batches.at(bi);
            let region_index = self.u16(o + 4)? as usize;
            let matref = self.u16(o + 10)? as usize;
            let bone = self.i16(o + 12)?;
            let Some(r) = regs.get(region_index) else { continue };
            if r.hidden() {
                continue;
            }
            if bone >= 0 && bones.get(bone as usize).is_some_and(|b| !b.batch_visible) {
                continue;
            }
            let range = match region_ranges[region_index] {
                Some(rg) => rg,
                None => {
                    let first = indices.len() as u32;
                    let (fa, fn_) = (r.first_face as usize, r.face_count as usize);
                    if fa.checked_add(fn_).is_none_or(|e| e > faces.count) {
                        return Err(corrupt("region face range is out of bounds"));
                    }
                    let base = if regions.version <= 2 { 0 } else { r.first_vertex as usize };
                    let mut k = fa;
                    while k + 3 <= fa + fn_ {
                        let tri = [face_at(k)?, face_at(k + 1)?, face_at(k + 2)?].map(|x| base + x as usize);
                        k += 3;
                        if tri.iter().any(|&x| x >= vcount) {
                            continue;
                        }
                        // Parts the pose scales to nothing (hidden add-ons,
                        // construction scaffolds) collapse to a point.
                        let [a, b, c] = tri.map(|x| positions[x]);
                        if (b - a).cross(c - a).length_squared() < 1e-14 {
                            continue;
                        }
                        // Regions may overlap, so bound the total output
                        // rather than each region (hostile files could
                        // otherwise expand a small model without limit).
                        if indices.len() + 3 > MAX_INDICES {
                            return Err(corrupt("model has too many triangles"));
                        }
                        for &x in &tri {
                            used[x] = true;
                            indices.push(x as u32);
                        }
                    }
                    let rg = (first, indices.len() as u32 - first);
                    region_ranges[region_index] = Some(rg);
                    rg
                }
            };
            if range.1 == 0 {
                continue;
            }
            if matcache.len() <= matref {
                matcache.resize(matref + 1, None);
            }
            let material = match &matcache[matref] {
                Some(mt) => mt.clone(),
                None => {
                    let mt = self.material_ref(&m, matref, 0).unwrap_or_else(|_| Material::other(0));
                    matcache[matref] = Some(mt.clone());
                    mt
                }
            };
            submeshes.push(Submesh { first_index: range.0, index_count: range.1, material });
        }

        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for (p, _) in positions.iter().zip(&used).filter(|(_, u)| **u) {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let bounds = if lo.x <= hi.x && lo.is_finite() && hi.is_finite() {
            Bounds { min: lo.to_array(), max: hi.to_array(), radius: (hi - lo).length() * 0.5 }
        } else {
            let mn = self.vec3(m.at + 136)?;
            let mx = self.vec3(m.at + 148)?;
            Bounds { min: mn.to_array(), max: mx.to_array(), radius: self.f32(m.at + 160)? }
        };

        Ok(Mesh {
            name,
            positions: positions.into_iter().map(|p| p.to_array()).collect(),
            normals: normals.into_iter().map(|n| n.to_array()).collect(),
            uvs,
            indices,
            submeshes,
            bounds,
            version: modl.version,
        })
    }

    // -----------------------------------------------------------------------
    // Bones and skinning

    fn bones(&self, m: &Modl) -> Result<Vec<Bone>> {
        let Some(it) = self.items(self.reference(m.at + 80)?, T_BONE, |v| (v == 1).then_some(160))? else {
            return Ok(Vec::new());
        };
        let mut out = Vec::with_capacity(it.count);
        for i in 0..it.count {
            let o = it.at(i);
            let rot = Quat::from_xyzw(self.f32(o + 68)?, self.f32(o + 72)?, self.f32(o + 76)?, self.f32(o + 80)?);
            out.push(Bone {
                parent: self.i16(o + 20)?,
                location: self.vec3(o + 32)?,
                rotation: rot,
                scale: self.vec3(o + 112)?,
                batch_visible: self.u32(o + 148)? != 0,
                anim_ids: [self.u32(o + 28)?, self.u32(o + 64)?, self.u32(o + 108)?, self.u32(o + 144)?],
            });
        }
        Ok(out)
    }

    /// Animation tracks of the `Stand` sequence, by animation id, holding
    /// the offset of their first key.
    fn stand_tracks(&self, m: &Modl) -> Result<Option<std::collections::HashMap<u32, Track>>> {
        let Some(groups) = self.items(self.reference(m.at + 40)?, tag(b"STG_"), |v| (v == 0).then_some(24))? else {
            return Ok(None);
        };
        let Some(stcs) = self.items(self.reference(m.at + 28)?, tag(b"STC_"), |v| (v == 4).then_some(204))? else {
            return Ok(None);
        };
        // The plain "Stand" group, else the shortest "Stand ..." variant.
        let mut best: Option<(usize, usize)> = None;
        for g in 0..groups.count {
            let name = self.string(self.reference(groups.at(g))?);
            let lower = name.to_ascii_lowercase();
            if (lower == "stand" || lower.starts_with("stand ")) && best.is_none_or(|(_, len)| name.len() < len) {
                best = Some((g, name.len()));
            }
        }
        let Some((g, _)) = best else { return Ok(None) };
        let Some(indices) = self.items(self.reference(groups.at(g) + 12)?, T_U32, |_| Some(4))? else {
            return Ok(None);
        };
        let mut tracks = std::collections::HashMap::new();
        for k in 0..indices.count {
            let si = self.u32(indices.at(k))? as usize;
            if si >= stcs.count {
                continue;
            }
            let s = stcs.at(si);
            let (Some(ids), Some(refs)) = (
                self.items(self.reference(s + 20)?, T_U32, |_| Some(4))?,
                self.items(self.reference(s + 32)?, T_U32, |_| Some(4))?,
            ) else {
                continue;
            };
            for a in 0..ids.count.min(refs.count) {
                let id = self.u32(ids.at(a))?;
                let r = self.u32(refs.at(a))?;
                let (kind, index) = ((r >> 16) as usize, (r & 0xffff) as usize);
                let (block_tag, key_tag, key_size, track): TrackKind = match kind {
                    2 => (tag(b"SD3V"), tag(b"VEC3"), 12, Track::Vec3),
                    3 => (tag(b"SD4Q"), tag(b"QUAT"), 16, Track::Quat),
                    10 => (tag(b"SDU3"), T_U32, 4, Track::U32),
                    11 => (tag(b"SDFG"), tag(b"FLAG"), 4, Track::U32),
                    _ => continue,
                };
                // Kind k is the k-th block reference, starting at sdev (+48).
                let Some(blocks) = self.items(self.reference(s + 48 + kind * 12)?, block_tag, |_| Some(32))? else {
                    continue;
                };
                if index >= blocks.count {
                    continue;
                }
                let Some(keys) = self.items(self.reference(blocks.at(index) + 20)?, key_tag, |_| Some(key_size))?
                else {
                    continue;
                };
                tracks.entry(id).or_insert(track(keys.at(0)));
            }
        }
        Ok(Some(tracks))
    }

    /// `world(default pose) * inverse(bind)` per bone, or `None` when the
    /// model has no usable skeleton (then vertices stay in bind pose).
    fn skin_matrices(&self, m: &Modl, bones: &[Bone]) -> Option<Vec<Mat4>> {
        if bones.is_empty() {
            return None;
        }
        let irefs = self.items(self.reference(m.bone_rests).ok()?, T_IREF, |v| (v == 0).then_some(64)).ok()??;
        if irefs.count != bones.len() {
            return None;
        }
        let mut world: Vec<Mat4> = Vec::with_capacity(bones.len());
        for (i, b) in bones.iter().enumerate() {
            let rot = if b.rotation.is_finite() && b.rotation.length_squared() > 1e-8 {
                b.rotation.normalize()
            } else {
                Quat::IDENTITY
            };
            let scale = if b.scale.is_finite() { b.scale } else { Vec3::ONE };
            let loc = if b.location.is_finite() { b.location } else { Vec3::ZERO };
            let local = Mat4::from_scale_rotation_translation(scale, rot, loc);
            // Parents come first in M3 files; anything else is treated as a root.
            let w = match usize::try_from(b.parent).ok().filter(|&p| p < i) {
                Some(p) => world[p] * local,
                None => local,
            };
            world.push(w);
        }
        let mut out = Vec::with_capacity(bones.len());
        for (i, w) in world.iter().enumerate() {
            let o = irefs.at(i);
            let mut cols = [0.0f32; 16];
            for (k, c) in cols.iter_mut().enumerate() {
                *c = self.f32(o + k * 4).ok()?;
            }
            let m = *w * Mat4::from_cols_array(&cols);
            if !m.is_finite() {
                return None;
            }
            out.push(m);
        }
        Some(out)
    }

    // -----------------------------------------------------------------------
    // Materials

    /// Material reference `i` (a `MATM`), following composites.
    fn material_ref(&self, m: &Modl, i: usize, depth: u32) -> Result<Material> {
        let refs = self
            .items(self.reference(m.at + m.material_refs)?, T_MATM, |_| Some(8))?
            .ok_or_else(|| corrupt("no material references"))?;
        if i >= refs.count {
            return Err(corrupt("material reference out of range"));
        }
        let kind = self.u32(refs.at(i))?;
        let index = self.u32(refs.at(i) + 4)? as usize;
        match kind {
            1 => self.standard_material(m, index),
            3 if depth < 4 => {
                let cmps = self
                    .items(self.reference(m.at + m.materials_composite)?, T_CMP, |v| (v == 2).then_some(28))?
                    .ok_or_else(|| corrupt("missing composite material"))?;
                if index >= cmps.count {
                    return Err(corrupt("composite material out of range"));
                }
                let sections = self
                    .items(self.reference(cmps.at(index) + 16)?, T_CMS, |v| (v == 0).then_some(24))?
                    .ok_or_else(|| corrupt("empty composite material"))?;
                for s in 0..sections.count {
                    let r = self.u32(sections.at(s))? as usize;
                    if let Ok(mt) = self.material_ref(m, r, depth + 1) {
                        if mt.kind == MaterialKind::Standard {
                            return Ok(mt);
                        }
                    }
                }
                Ok(Material::other(3))
            }
            k => Ok(Material::other(k)),
        }
    }

    fn standard_material(&self, m: &Modl, index: usize) -> Result<Material> {
        let mats = self
            .items(self.reference(m.at + m.materials_standard)?, T_MAT, |v| match v {
                15 => Some(268),
                16..=18 => Some(280),
                19 => Some(340),
                20 => Some(352),
                _ => None,
            })?
            .ok_or_else(|| corrupt("missing standard material"))?;
        if index >= mats.count {
            return Err(corrupt("standard material out of range"));
        }
        let o = mats.at(index);
        let v = mats.version;
        let layers = if v >= 20 { o + 64 } else { o + 52 };
        // Layer slots in order; gloss only exists from version 16.
        let slot = |n: usize| layers + n * 12;
        let gloss = usize::from(v >= 16);
        let diffuse = slot(0);
        let emissive = slot(3 + gloss);
        let alpha = slot(7 + gloss);
        let layer_count = 13 + gloss + if v >= 19 { 4 } else { 0 };
        let after = slot(layer_count);
        Ok(Material {
            name: self.string(self.reference(o)?),
            kind: MaterialKind::Standard,
            flags: self.u32(o + 16)?,
            blend: Blend::from_raw(self.u32(o + 20)?),
            alpha_test: self.u8(o + 40)?,
            emissive_mult: self.f32(o + 48)?,
            diffuse: self.layer(self.reference(diffuse)?),
            emissive: self.layer(self.reference(emissive)?),
            alpha_mask: self.layer(self.reference(alpha)?),
            emissive_blend: self.u32(after + 8)?,
        })
    }

    fn layer(&self, r: Ref) -> Option<Layer> {
        let it = self.items(r, T_LAYR, |v| (20..=26).contains(&v).then_some(356)).ok()??;
        let o = it.at(0);
        let path = self.string(self.reference(o + 4).ok()?);
        let col = self.bytes(o + 16 + 8, 4).ok()?; // ColorAnimationReference default, BGRA
        let flags = self.u32(o + 36).ok()?;
        let layer = Layer {
            path: (!path.trim().is_empty()).then_some(path),
            color: [col[2], col[1], col[0], col[3]],
            flags,
            uv_source: self.u32(o + 40).ok()?,
            channels: self.u32(o + 44).ok()?,
        };
        // A layer with neither a bitmap nor a constant color does nothing.
        (layer.path.is_some() || flags & 0x400 != 0).then_some(layer)
    }
}

impl Mesh {
    fn empty(name: String, version: u32) -> Mesh {
        Mesh {
            name,
            positions: Vec::new(),
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            submeshes: Vec::new(),
            bounds: Bounds { min: [0.0; 3], max: [0.0; 3], radius: 0.0 },
            version,
        }
    }
}

/// Byte offsets of the `MODL` fields we read (relative to the struct start,
/// except `at`, which is absolute).
struct Modl {
    at: usize,
    size: usize,
    material_refs: usize,
    materials_standard: usize,
    materials_composite: usize,
    /// Absolute offset of the `bone_rests` (IREF) reference.
    bone_rests: usize,
}

impl Modl {
    fn layout(at: usize, v: u32) -> Modl {
        // Fixed up to the lights; shadow boxes appear in version 21.
        let mut c = 264 + if v >= 21 { 12 } else { 0 };
        c += 24; // cameras, cameras addon
        let material_refs = c;
        let materials_standard = c + 12;
        let materials_composite = c + 36;
        c += 12 * 8; // material refs, standard, displacement, composite, terrain, volume, hair, creep
        for since in [25, 26, 28, 29, 30] {
            if v >= since {
                c += 12; // volume noise, splat terrain bake, reflection, lens flare, buffer
            }
        }
        c += 12 * 10; // particles .. physics joints
        if v >= 28 {
            c += 12; // cloths
        }
        c += 12; // ik two joints
        if v >= 24 {
            c += 12; // ik ccd
        }
        c += 12 * 4; // ik joints, one-bone solvers, turret parts, turrets
        let bone_rests = c;
        c += 12 + 108 + 12 + 12; // bone rests, tight hit test, hit tests, attachment volumes
        if v >= 23 {
            c += 24;
        }
        c += 12 + 12 + 4 + 12; // billboards, tmd, m3a hash, m3a hashes
        Modl { at, size: c, material_refs, materials_standard, materials_composite, bone_rests: at + bone_rests }
    }
}

struct Bone {
    parent: i16,
    location: Vec3,
    rotation: Quat,
    scale: Vec3,
    /// The bone's batching flag: batches bound to a bone whose flag is off
    /// are hidden.
    batch_visible: bool,
    /// Animation ids of location, rotation, scale and batching.
    anim_ids: [u32; 4],
}

/// Block tag, key tag, key size and constructor of one keyframe type.
type TrackKind = ([u8; 4], [u8; 4], usize, fn(usize) -> Track);

/// First key of an animation track (absolute offset of the key data).
#[derive(Debug, Clone, Copy)]
enum Track {
    Vec3(usize),
    Quat(usize),
    U32(usize),
}

impl Bone {
    /// Take the keyed values of `tracks` where the bone is animated.
    fn apply(&mut self, f: &File, tracks: &std::collections::HashMap<u32, Track>) {
        let get = |id: u32| tracks.get(&id).copied();
        if let Some(Track::Vec3(o)) = get(self.anim_ids[0]) {
            if let Ok(v) = f.vec3(o) {
                self.location = v;
            }
        }
        if let Some(Track::Quat(o)) = get(self.anim_ids[1]) {
            if let (Ok(x), Ok(y), Ok(z), Ok(w)) = (f.f32(o), f.f32(o + 4), f.f32(o + 8), f.f32(o + 12)) {
                self.rotation = Quat::from_xyzw(x, y, z, w);
            }
        }
        if let Some(Track::Vec3(o)) = get(self.anim_ids[2]) {
            if let Ok(v) = f.vec3(o) {
                self.scale = v;
            }
        }
        if let Some(Track::U32(o)) = get(self.anim_ids[3]) {
            if let Ok(v) = f.u32(o) {
                self.batch_visible = v != 0;
            }
        }
    }
}

struct Region {
    first_vertex: u32,
    vertex_count: u32,
    first_face: u32,
    face_count: u32,
    first_bone_lookup: u16,
    lookups_used: u8,
    flags: u32,
    uv_multiply: f32,
    uv_offset: f32,
}

impl Region {
    fn read(f: &File, o: usize, version: u32) -> Result<Region> {
        if version <= 2 {
            return Ok(Region {
                first_vertex: f.u16(o + 4)? as u32,
                vertex_count: f.u16(o + 6)? as u32,
                first_face: f.u32(o + 8)?,
                face_count: f.u32(o + 12)?,
                first_bone_lookup: f.u16(o + 18)?,
                lookups_used: f.u8(o + 24)?,
                flags: 0,
                uv_multiply: 16.0,
                uv_offset: 0.0,
            });
        }
        let (uv_multiply, uv_offset) = if version >= 5 { (f.f32(o + 40)?, f.f32(o + 44)?) } else { (16.0, 0.0) };
        Ok(Region {
            first_vertex: f.u32(o + 8)?,
            vertex_count: f.u32(o + 12)?,
            first_face: f.u32(o + 16)?,
            face_count: f.u32(o + 20)?,
            first_bone_lookup: f.u16(o + 26)?,
            lookups_used: f.u8(o + 32)?,
            flags: if version >= 4 { f.u32(o + 36)? } else { 0 },
            uv_multiply: if uv_multiply.is_finite() { uv_multiply } else { 16.0 },
            uv_offset: if uv_offset.is_finite() { uv_offset } else { 0.0 },
        })
    }

    /// Hidden or placeholder regions are not drawn.
    fn hidden(&self) -> bool {
        self.flags & 0x3 != 0
    }
}

/// Byte offsets of the vertex components present for a `vertex_flags` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VertexLayout {
    stride: usize,
    /// Number of bone weight / lookup pairs (0, 2 or 4).
    skin_pairs: usize,
    weights: usize,
    lookups: usize,
    normal_f32: Option<usize>,
    normal_u8: Option<usize>,
    uv0_i16: Option<usize>,
    uv0_f32: Option<usize>,
}

impl VertexLayout {
    fn new(flags: u32) -> VertexLayout {
        let has = |m: u32| flags & m != 0;
        let mut o = 0usize;
        let mut l = VertexLayout {
            stride: 0,
            skin_pairs: 0,
            weights: 0,
            lookups: 0,
            normal_f32: None,
            normal_u8: None,
            uv0_i16: None,
            uv0_f32: None,
        };
        if has(0x1) {
            o += 12;
        }
        l.skin_pairs = 2 * usize::from(has(0x20)) + 2 * usize::from(has(0x40));
        l.weights = o;
        o += l.skin_pairs;
        l.lookups = o;
        o += l.skin_pairs;
        if has(0x80) {
            l.normal_f32 = Some(o);
            o += 12;
        }
        if has(0x800000) {
            l.normal_u8 = Some(o);
            o += 4;
        }
        for m in [0x100, 0x200, 0x400, 0x800, 0x1000] {
            if has(m) {
                o += 4;
            }
        }
        for m in [0x2000, 0x4000, 0x8000, 0x10000] {
            if has(m) {
                if l.uv0_f32.is_none() {
                    l.uv0_f32 = Some(o);
                }
                o += 8;
            }
        }
        for m in [0x20000, 0x40000, 0x80000, 0x100000, 0x40000000] {
            if has(m) {
                if l.uv0_i16.is_none() {
                    l.uv0_i16 = Some(o);
                }
                o += 4;
            }
        }
        if has(0x200000) {
            if l.normal_f32.is_none() {
                l.normal_f32 = Some(o);
            }
            o += 12;
        }
        if has(0x400000) {
            o += 12;
        }
        if has(0x1000000) {
            o += 4;
        }
        for (m, n) in [(0x2000000, 4), (0x4000000, 12), (0x8000000, 12), (0x10000000, 4), (0x20000000, 4)] {
            if has(m) {
                o += n;
            }
        }
        l.stride = o;
        l
    }

    fn normal(&self, f: &File, o: usize) -> Result<Vec3> {
        let n = if let Some(k) = self.normal_u8 {
            let b = f.bytes(o + k, 3)?;
            Vec3::new(b[0] as f32, b[1] as f32, b[2] as f32) / 255.0 * 2.0 - Vec3::ONE
        } else if let Some(k) = self.normal_f32 {
            f.vec3(o + k)?
        } else {
            Vec3::Z
        };
        Ok(n.try_normalize().unwrap_or(Vec3::Z))
    }

    fn uv(&self, f: &File, o: usize, mult: f32, off: f32) -> Result<[f32; 2]> {
        if let Some(k) = self.uv0_i16 {
            let s = mult / 32768.0;
            let (u, v) = (f.i16(o + k)? as f32, f.i16(o + k + 2)? as f32);
            Ok([u * s + off, v * s + off])
        } else if let Some(k) = self.uv0_f32 {
            Ok([f.f32(o + k)?, f.f32(o + k + 4)?])
        } else {
            Ok([0.0, 0.0])
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn skin(
        &self,
        f: &File,
        o: usize,
        r: &Region,
        lookup: &[u16],
        mats: &[Mat4],
        p: Vec3,
        n: Vec3,
    ) -> Result<(Vec3, Vec3)> {
        let pairs = self.skin_pairs.min(r.lookups_used as usize);
        if pairs == 0 {
            // Rigid regions follow their first bone lookup.
            let m = lookup.get(r.first_bone_lookup as usize).and_then(|&b| mats.get(b as usize));
            return Ok(match m {
                Some(m) => (m.transform_point3(p), m.transform_vector3(n).try_normalize().unwrap_or(n)),
                None => (p, n),
            });
        }
        let mut sp = Vec3::ZERO;
        let mut sn = Vec3::ZERO;
        let mut total = 0.0;
        for k in 0..pairs {
            let w = f.u8(o + self.weights + k)? as f32 / 255.0;
            if w <= 0.0 {
                continue;
            }
            let li = r.first_bone_lookup as usize + f.u8(o + self.lookups + k)? as usize;
            let Some(m) = lookup.get(li).and_then(|&b| mats.get(b as usize)) else { continue };
            sp += m.transform_point3(p) * w;
            sn += m.transform_vector3(n) * w;
            total += w;
        }
        if total <= 1e-4 {
            return Ok((p, n));
        }
        Ok((sp / total, sn.try_normalize().unwrap_or(n)))
    }
}

#[cfg(test)]
mod tests;
