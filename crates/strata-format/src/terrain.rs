//! Terrain: `t3Terrain.xml` plus the binary grids next to it.
//!
//! Layouts (all little-endian, verified against editor 5.0.x output):
//!
//! | file | header | data |
//! |---|---|---|
//! | `t3HeightMap` | `HMAP` v101, u32 vertsX, u32 vertsY, 16 zero bytes | per vertex: u16 base, u16 cliffOffset, u16 cliffLevel |
//! | `t3SyncHeightMap` | `SMAP` v102, vertsX, vertsY, 12 zero bytes, 3 x u32 0x1000, pad to 64 | per vertex: u16 height*256, i16 cliffLevel |
//! | `t3SyncCliffLevel` | `CLIF` v100, cellsX, cellsY, pad to 32 | per cell: u16 64*cliffLevel (ramps hold intermediate values) |
//! | `t3CellFlags` | `LFCT` v102, 16 zero bytes, cellsX, cellsY | per cell: u8 flags (0x02 on every cell seen; 0x01 on cliff/blocked cells) |
//! | `t3TextureMasks` | `MASK` v102, u32 0, maskW, maskH, pad to 64 | 8 layers of 4-bit weights, 64x64-pixel tiles of 2048 bytes; mask = 8 px per cell |
//! | `t3SyncTextureInfo` | `RTXT` v101, cellsX, cellsY, u32 sets, 8*sets cstr names | 8 bytes per cell |
//! | `t3VertCol` | `VTCL` v104, u32 blocks, pad to 32 | per 4x4-cell block: u32 0, u32 1, f32 x0 y0 x1 y1, u32 0, u32 0, u32 25, 25 colour bytes |
//! | `t3Water` / `t3HardTile` / `t3FluffDoodad` | `WATR` v110 / `HRDT` v102 / `DLFT` v103 | empty maps: zero padding to 32 / 28 / 28 bytes |
//!
//! Height of a vertex: `(base*qs - qb)*scaleZ + offsetZ + (cliffOffset*qs - qb)`
//! with `qb`/`qs` from `<vertData quantizeBias quantizeScale>`. The editor
//! keeps the cliff offset at `(level - 1) * 2` and the sculpted detail in
//! `base`. The cliff level of a vertex is the low byte of the third word.
//!
//! On save of an existing map only the vertices / cells / mask pixels whose
//! model value differs from the decoded original are rewritten; everything
//! else stays byte-identical. Heights outside the file's quantization range
//! trigger a re-quantization (and an updated `vertData`). Pathing maps only
//! bit 0 of `t3CellFlags` to [`pathing::UNWALKABLE`] (inferred). Texture
//! layers are the eight textures of texture set 0. Cliff meshes
//! (`cliffCellList`), ramps, water, vertex colours and hard tiles are kept
//! but not modeled; a brand-new map gets cliff levels in the grids but no
//! cliff cells.

use std::collections::HashSet;

use strata_core::terrain::{pathing, Terrain, TextureLayer, MAX_TEXTURE_LAYERS};
use strata_core::MapDocument;

use crate::bin::{read_u16, read_u32, write_u16, Put};
use crate::xml::{self, fmt_sci, Doc, Edits, INDENT};
use crate::{lookup, put, FileMap};

pub const XML: &str = "t3Terrain.xml";
pub const HEIGHT: &str = "t3HeightMap";
pub const SYNC_HEIGHT: &str = "t3SyncHeightMap";
pub const CLIFF: &str = "t3SyncCliffLevel";
pub const FLAGS: &str = "t3CellFlags";
pub const MASKS: &str = "t3TextureMasks";
pub const SYNC_TEX: &str = "t3SyncTextureInfo";
pub const VERTCOL: &str = "t3VertCol";
pub const WATER: &str = "t3Water";
pub const HARDTILE: &str = "t3HardTile";
pub const FLUFF: &str = "t3FluffDoodad";

const FLAG_BLOCKED: u8 = 0x01;
const FLAG_DEFAULT: u8 = 0x02;

pub struct Desc {
    pub doc: Doc,
    pub tileset: String,
    pub verts: (u32, u32),
    pub qb: f64,
    pub qs: f64,
    pub scale_z: f64,
    pub offset_z: f64,
    /// Texture names by index (set * 8 + layer).
    pub textures: Vec<String>,
}

fn floats(s: Option<&str>) -> Vec<f64> {
    s.map(|s| s.split_whitespace().filter_map(|p| p.parse().ok()).collect()).unwrap_or_default()
}

pub fn parse_desc(bytes: &[u8]) -> Option<Desc> {
    let doc = xml::parse_bytes(bytes).ok()?;
    if doc.root.name != "terrain" {
        return None;
    }
    let hm = doc.root.child("heightMap")?;
    let dim = floats(hm.attr("dim"));
    if dim.len() < 2 || dim[0] < 2.0 || dim[1] < 2.0 {
        return None;
    }
    let vd = hm.child("vertData");
    let num = |k: &str, d: f64| vd.and_then(|v| v.attr(k)).and_then(|s| s.trim().parse().ok()).unwrap_or(d);
    let mut textures = vec![String::new(); 64];
    if let Some(tl) = hm.child("textureList") {
        for t in tl.children_named("texture") {
            if let Some(i) = t.attr("i").and_then(|i| i.parse::<usize>().ok()).filter(|&i| i < 64) {
                textures[i] = t.attr("name").unwrap_or("").to_owned();
            }
        }
    }
    let scale = floats(hm.attr("scale"));
    let offset = floats(hm.attr("offset"));
    Some(Desc {
        tileset: hm.attr("tileSet").unwrap_or("").to_owned(),
        verts: (dim[0] as u32, dim[1] as u32),
        qb: num("quantizeBias", 0.0),
        qs: num("quantizeScale", 0.1),
        scale_z: scale.get(2).copied().filter(|v| *v != 0.0).unwrap_or(1.0),
        offset_z: offset.get(2).copied().unwrap_or(0.0),
        textures,
        doc,
    })
}

// ---- decoding ----------------------------------------------------------------

struct Heights {
    world: Vec<f32>,
    cliff: Vec<u8>,
}

fn check(d: &[u8], magic: &[u8; 4], min: usize) -> bool {
    d.len() >= min && &d[..4] == magic
}

fn decode_heights(d: &[u8], desc: &Desc) -> Option<Heights> {
    let (vx, vy) = desc.verts;
    let n = (vx * vy) as usize;
    if !check(d, b"HMAP", 32) || read_u32(d, 8) != vx || read_u32(d, 12) != vy || d.len() != 32 + 6 * n {
        return None;
    }
    let mut world = Vec::with_capacity(n);
    let mut cliff = Vec::with_capacity(n);
    for i in 0..n {
        let o = 32 + 6 * i;
        world.push(height_of(desc, read_u16(d, o), read_u16(d, o + 2)));
        cliff.push((read_u16(d, o + 4) & 0xFF) as u8);
    }
    Some(Heights { world, cliff })
}

fn height_of(desc: &Desc, base: u16, adj: u16) -> f32 {
    ((base as f64 * desc.qs - desc.qb) * desc.scale_z + desc.offset_z + (adj as f64 * desc.qs - desc.qb)) as f32
}

fn decode_flags(d: &[u8], cells: (u32, u32)) -> Option<&[u8]> {
    let n = (cells.0 * cells.1) as usize;
    if !check(d, b"LFCT", 32) || read_u32(d, 24) != cells.0 || read_u32(d, 28) != cells.1 || d.len() != 32 + n {
        return None;
    }
    Some(&d[32..])
}

#[derive(Clone, Copy)]
struct Mask {
    w: u32,
    h: u32,
    layer_size: usize,
}

impl Mask {
    fn header(d: &[u8]) -> Option<Mask> {
        if !check(d, b"MASK", 64) || read_u32(d, 4) != 102 {
            return None;
        }
        let (w, h) = (read_u32(d, 12), read_u32(d, 16));
        if w == 0 || h == 0 || w % 64 != 0 || h % 64 != 0 || w > 8192 || h > 8192 {
            return None;
        }
        let layer_size = (w / 64 * h / 64) as usize * 2048;
        (d.len() == 64 + 8 * layer_size).then_some(Mask { w, h, layer_size })
    }

    fn offset(&self, layer: usize, x: u32, y: u32) -> (usize, bool) {
        let tiles_x = self.w / 64;
        let (tx, ty, ix, iy) = (x / 64, y / 64, x % 64, y % 64);
        let off = 64 + layer * self.layer_size + ((ty * tiles_x + tx) as usize) * 2048 + (iy * 32 + ix / 2) as usize;
        (off, ix % 2 == 0)
    }

    fn get(&self, d: &[u8], layer: usize, x: u32, y: u32) -> u8 {
        let (o, hi) = self.offset(layer, x, y);
        if hi { d[o] >> 4 } else { d[o] & 0x0F }
    }

    fn set(&self, d: &mut [u8], layer: usize, x: u32, y: u32, v: u8) {
        let (o, hi) = self.offset(layer, x, y);
        d[o] = if hi { (d[o] & 0x0F) | (v << 4) } else { (d[o] & 0xF0) | (v & 0x0F) };
    }

    /// Mask pixels per cell on each axis.
    fn scale(&self, cells: (u32, u32)) -> (f32, f32) {
        (self.w as f32 / cells.0 as f32, self.h as f32 / cells.1 as f32)
    }
}

/// Per-vertex weights (0..=255) for all 8 mask layers.
fn decode_layers(d: &[u8], m: Mask, cells: (u32, u32)) -> Vec<Vec<u8>> {
    let (sx, sy) = m.scale(cells);
    let (vx, vy) = (cells.0 + 1, cells.1 + 1);
    (0..8)
        .map(|l| {
            let mut w = Vec::with_capacity((vx * vy) as usize);
            for y in 0..vy {
                let py = ((y as f32 * sy).round() as u32).min(m.h - 1);
                for x in 0..vx {
                    let px = ((x as f32 * sx).round() as u32).min(m.w - 1);
                    w.push(m.get(d, l, px, py) * 17);
                }
            }
            w
        })
        .collect()
}

/// Build the model terrain from the files. Missing or undecodable parts fall
/// back to defaults (the files themselves are preserved by passthrough).
pub fn load(files: &FileMap, size: Option<(u32, u32)>) -> Option<(Terrain, String)> {
    let desc = lookup(files, XML).and_then(parse_desc);
    let (w, h) = match &desc {
        Some(d) => (d.verts.0 - 1, d.verts.1 - 1),
        None => size?,
    };
    let mut t = Terrain::new(w, h);
    let Some(desc) = desc else {
        return Some((t, String::new()));
    };
    if let Some(hs) = lookup(files, HEIGHT).and_then(|b| decode_heights(b, &desc)) {
        t.heights = hs.world;
        t.cliff_levels = hs.cliff;
    }
    if let Some(f) = lookup(files, FLAGS).and_then(|b| decode_flags(b, (w, h))) {
        t.pathing = f.iter().map(|&b| if b & FLAG_BLOCKED != 0 { pathing::UNWALKABLE } else { 0 }).collect();
    }
    let mut weights: Vec<Vec<u8>> = Vec::new();
    if let Some(d) = lookup(files, MASKS) {
        if let Some(m) = Mask::header(d) {
            weights = decode_layers(d, m, (w, h));
        }
    }
    let verts = ((w + 1) * (h + 1)) as usize;
    let used = (0..MAX_TEXTURE_LAYERS)
        .filter(|&l| !desc.textures[l].is_empty() || weights.get(l).is_some_and(|w| w.iter().any(|&v| v != 0)))
        .max()
        .unwrap_or(0);
    t.layers = (0..=used)
        .map(|l| TextureLayer {
            texture: desc.textures[l].clone(),
            weights: weights.get(l).cloned().unwrap_or_else(|| vec![if l == 0 { 255 } else { 0 }; verts]),
        })
        .collect();
    Some((t, desc.tileset))
}

// ---- encoding ----------------------------------------------------------------

/// Quantization covering every value with a 1-unit margin, like the editor
/// (`qb = 1 - min`, `qs = (max - min + 2) / 65535`), rounded through the
/// `%e` text form so encode and decode agree exactly.
fn quantization(values: impl Iterator<Item = f64>) -> (f64, f64, String, String) {
    let (mut lo, mut hi) = (f64::MAX, f64::MIN);
    for v in values {
        lo = lo.min(v);
        hi = hi.max(v);
    }
    if lo > hi {
        lo = 0.0;
        hi = 0.0;
    }
    let (qb_s, qs_s) = (fmt_sci(1.0 - lo), fmt_sci((hi - lo + 2.0) / 65535.0));
    (qb_s.parse().unwrap_or(1.0), qs_s.parse().unwrap_or(1.0), qb_s, qs_s)
}

fn enc(v: f64, qb: f64, qs: f64) -> Option<u16> {
    let r = ((v + qb) / qs).round();
    (0.0..=65535.0).contains(&r).then_some(r as u16)
}

fn cliff_offset(level: u8) -> f64 {
    (level as f64 - 1.0) * 2.0
}

/// Write terrain files for the document, patching originals where possible.
pub fn save(doc: &MapDocument, orig: &FileMap, out: &mut FileMap) {
    let t = &doc.terrain;
    let cells = (t.width, t.height);
    let desc = lookup(orig, XML).and_then(parse_desc).filter(|d| d.verts == (t.width + 1, t.height + 1));
    let heights = desc.as_ref().and_then(|d| lookup(orig, HEIGHT).and_then(|b| decode_heights(b, d)));
    let (Some(desc), Some(h0)) = (desc, heights) else {
        fresh(doc, out);
        return;
    };
    let mut edits = Edits::default();
    let hm_el = desc.doc.root.child("heightMap").expect("checked in parse_desc");

    // Tileset
    if !doc.info.tileset.is_empty() && doc.info.tileset != desc.tileset {
        xml::set_attr_edit(&desc.doc, hm_el, "tileSet", &doc.info.tileset, &mut edits);
    }

    // Heights and cliff levels
    let n = h0.world.len();
    let changed: Vec<usize> =
        (0..n).filter(|&i| t.heights.get(i) != Some(&h0.world[i]) || t.cliff_levels.get(i) != Some(&h0.cliff[i])).collect();
    if !changed.is_empty() {
        let orig_hm = lookup(orig, HEIGHT).expect("decoded above");
        let mut hm = orig_hm.to_vec();
        let level = |i: usize| t.cliff_levels.get(i).copied().unwrap_or(h0.cliff[i]);
        let world = |i: usize| t.heights.get(i).copied().unwrap_or(h0.world[i]) as f64;
        // World-space (base, offset) pair for vertex i.
        let pair = |i: usize, qb: f64, qs: f64| -> (f64, f64) {
            let o = 32 + 6 * i;
            let adj_raw = read_u16(orig_hm, o + 2);
            let adj = if level(i) == h0.cliff[i] { adj_raw as f64 * qs - qb } else { cliff_offset(level(i)) };
            ((world(i) - desc.offset_z - adj) / desc.scale_z, adj)
        };
        let fits = changed.iter().all(|&i| {
            let (b, a) = pair(i, desc.qb, desc.qs);
            enc(b, desc.qb, desc.qs).is_some() && enc(a, desc.qb, desc.qs).is_some()
        });
        let changed_set: HashSet<usize> = changed.iter().copied().collect();
        let (qb, qs, todo): (f64, f64, Vec<usize>) = if fits {
            (desc.qb, desc.qs, changed.clone())
        } else {
            // Re-quantize every vertex.
            let pairs: Vec<(f64, f64)> = (0..n)
                .map(|i| {
                    if changed_set.contains(&i) {
                        pair(i, desc.qb, desc.qs)
                    } else {
                        let o = 32 + 6 * i;
                        let b = read_u16(orig_hm, o) as f64 * desc.qs - desc.qb;
                        let a = read_u16(orig_hm, o + 2) as f64 * desc.qs - desc.qb;
                        (b, a)
                    }
                })
                .collect();
            let (qb, qs, qb_s, qs_s) = quantization(pairs.iter().flat_map(|&(b, a)| [b, a]));
            if let Some(vd) = hm_el.child("vertData") {
                xml::set_attr_edit(&desc.doc, vd, "quantizeBias", &qb_s, &mut edits);
                xml::set_attr_edit(&desc.doc, vd, "quantizeScale", &qs_s, &mut edits);
            }
            for (i, (b, a)) in pairs.iter().enumerate() {
                let o = 32 + 6 * i;
                write_u16(&mut hm, o, enc(*b, qb, qs).unwrap_or(0));
                write_u16(&mut hm, o + 2, enc(*a, qb, qs).unwrap_or(0));
            }
            (qb, qs, Vec::new())
        };
        for &i in &todo {
            let (b, a) = pair(i, qb, qs);
            let o = 32 + 6 * i;
            let adj_raw = read_u16(orig_hm, o + 2);
            write_u16(&mut hm, o, enc(b, qb, qs).unwrap_or(0));
            if level(i) != h0.cliff[i] {
                write_u16(&mut hm, o + 2, enc(a, qb, qs).unwrap_or(adj_raw));
            }
        }
        for &i in &changed {
            let o = 32 + 6 * i + 4;
            let m = read_u16(&hm, o);
            write_u16(&mut hm, o, (m & 0xFF00) | level(i) as u16);
        }
        put(out, HEIGHT, hm);

        if let Some(sm0) = lookup(orig, SYNC_HEIGHT).filter(|d| check(d, b"SMAP", 64) && d.len() == 64 + 4 * n) {
            let mut sm = sm0.to_vec();
            for &i in &changed {
                let o = 64 + 4 * i;
                write_u16(&mut sm, o, (world(i) * 256.0).round().clamp(0.0, 65535.0) as u16);
                write_u16(&mut sm, o + 2, level(i) as u16);
            }
            put(out, SYNC_HEIGHT, sm);
        }
        let cliff_changed: Vec<usize> = changed.iter().copied().filter(|&i| level(i) != h0.cliff[i]).collect();
        let cell_n = (cells.0 * cells.1) as usize;
        if let Some(c0) = lookup(orig, CLIFF).filter(|d| check(d, b"CLIF", 32) && d.len() == 32 + 2 * cell_n) {
            if !cliff_changed.is_empty() {
                let mut c = c0.to_vec();
                let vx = t.width + 1;
                let mut cells_done = HashSet::new();
                for &i in &cliff_changed {
                    let (x, y) = (i as u32 % vx, i as u32 / vx);
                    for (cx, cy) in [(x.wrapping_sub(1), y.wrapping_sub(1)), (x, y.wrapping_sub(1)), (x.wrapping_sub(1), y), (x, y)] {
                        if cx >= cells.0 || cy >= cells.1 || !cells_done.insert((cx, cy)) {
                            continue;
                        }
                        let corners = [(cx, cy), (cx + 1, cy), (cx, cy + 1), (cx + 1, cy + 1)];
                        let lv = corners.iter().map(|&(a, b)| level((b * vx + a) as usize)).min().unwrap_or(0);
                        write_u16(&mut c, 32 + 2 * (cy * cells.0 + cx) as usize, 64 * lv as u16);
                    }
                }
                put(out, CLIFF, c);
            }
        }
    }

    // Pathing (bit 0 only)
    if let Some(f0) = lookup(orig, FLAGS) {
        if let Some(raw) = decode_flags(f0, cells) {
            let mut f: Option<Vec<u8>> = None;
            for (i, &b) in raw.iter().enumerate() {
                let want = t.pathing.get(i).is_some_and(|p| p & pathing::UNWALKABLE != 0);
                if (b & FLAG_BLOCKED != 0) != want {
                    let f = f.get_or_insert_with(|| f0.to_vec());
                    f[32 + i] = if want { b | FLAG_BLOCKED } else { b & !FLAG_BLOCKED };
                }
            }
            if let Some(f) = f {
                put(out, FLAGS, f);
            }
        }
    }

    // Texture weights
    if let Some(m0) = lookup(orig, MASKS) {
        if let Some(m) = Mask::header(m0) {
            let w0 = decode_layers(m0, m, cells);
            let verts = ((cells.0 + 1) * (cells.1 + 1)) as usize;
            let cur = |l: usize, v: usize| t.layers.get(l).and_then(|ly| ly.weights.get(v)).copied().unwrap_or(0);
            let mut buf: Option<Vec<u8>> = None;
            let (sx, sy) = m.scale(cells);
            let vx = cells.0 + 1;
            for (l, layer0) in w0.iter().enumerate() {
                for v in 0..verts {
                    if cur(l, v) == layer0[v] {
                        continue;
                    }
                    let d = buf.get_or_insert_with(|| m0.to_vec());
                    let (x, y) = (v as u32 % vx, v as u32 / vx);
                    let px0 = (((x as f32 - 1.0) * sx).floor() + 1.0).max(0.0) as u32;
                    let px1 = (((x as f32 + 1.0) * sx).ceil() as u32).min(m.w);
                    let py0 = (((y as f32 - 1.0) * sy).floor() + 1.0).max(0.0) as u32;
                    let py1 = (((y as f32 + 1.0) * sy).ceil() as u32).min(m.h);
                    for py in py0..py1 {
                        for px in px0..px1 {
                            let w = bilinear(|a, b| cur(l, (b * vx + a) as usize), px as f32 / sx, py as f32 / sy, cells);
                            m.set(d, l, px, py, (w / 17.0).round().clamp(0.0, 15.0) as u8);
                        }
                    }
                }
            }
            if let Some(d) = buf {
                put(out, MASKS, d);
            }
        }
    }

    // Texture names (texture set 0)
    if let Some(tl) = hm_el.child("textureList") {
        let nl = desc.doc.newline();
        let mut inserts = String::new();
        for (l, layer) in t.layers.iter().enumerate().take(MAX_TEXTURE_LAYERS) {
            if layer.texture == desc.textures[l] {
                continue;
            }
            let found = tl.children_named("texture").find(|e| e.attr("i") == Some(&l.to_string()));
            match found {
                Some(e) => xml::set_attr_edit(&desc.doc, e, "name", &layer.texture, &mut edits),
                None => {
                    let ind = format!("{}{INDENT}", xml::indent_before(&desc.doc.src, tl.span.start));
                    inserts.push_str(&format!(
                        "{nl}{ind}<texture i=\"{l}\" name=\"{}\"/>",
                        xml::esc_attr(&layer.texture)
                    ));
                }
            }
        }
        if !inserts.is_empty() {
            match tl.children.last() {
                Some(last) => edits.insert(last.span.end, inserts),
                None if tl.self_closing => {
                    let ind = xml::indent_before(&desc.doc.src, tl.span.start).to_owned();
                    edits.replace(
                        tl.span.clone(),
                        format!("{}{inserts}{nl}{ind}</textureList>", xml::start_tag("textureList", &tl.attrs, false)),
                    );
                }
                None => {
                    let ind = xml::indent_before(&desc.doc.src, tl.span.start).to_owned();
                    edits.replace(tl.inner.clone(), format!("{inserts}{nl}{ind}"));
                }
            }
        }
    }

    if !edits.is_empty() {
        put(out, XML, desc.doc.finish(edits.apply(&desc.doc.src)));
    }
}

fn bilinear(w: impl Fn(u32, u32) -> u8, fx: f32, fy: f32, cells: (u32, u32)) -> f32 {
    let fx = fx.clamp(0.0, cells.0 as f32);
    let fy = fy.clamp(0.0, cells.1 as f32);
    let x0 = (fx.floor() as u32).min(cells.0.saturating_sub(1));
    let y0 = (fy.floor() as u32).min(cells.1.saturating_sub(1));
    let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
    let g = |a, b| w(a, b) as f32;
    let a = g(x0, y0) + (g(x0 + 1, y0) - g(x0, y0)) * tx;
    let b = g(x0, y0 + 1) + (g(x0 + 1, y0 + 1) - g(x0, y0 + 1)) * tx;
    a + (b - a) * ty
}

/// Generate every terrain file from the model (new map or resized terrain).
pub fn fresh(doc: &MapDocument, out: &mut FileMap) {
    let t = &doc.terrain;
    let (cw, ch) = (t.width, t.height);
    let (vx, vy) = (cw + 1, ch + 1);
    let n = (vx * vy) as usize;
    let level = |i: usize| t.cliff_levels.get(i).copied().unwrap_or(1);
    let world = |i: usize| t.heights.get(i).copied().unwrap_or(8.0) as f64;
    let pairs: Vec<(f64, f64)> = (0..n).map(|i| (world(i) - cliff_offset(level(i)), cliff_offset(level(i)))).collect();
    let (qb, qs, qb_s, qs_s) = quantization(pairs.iter().flat_map(|&(b, a)| [b, a]));
    let tileset = if doc.info.tileset.is_empty() { "Char".to_owned() } else { doc.info.tileset.clone() };

    // t3Terrain.xml
    let nl = "\r\n";
    let i1 = INDENT;
    let i2 = INDENT.repeat(2);
    let i3 = INDENT.repeat(3);
    let mut s = String::from(xml::DECL);
    s.push_str(nl);
    s.push_str("<terrain version=\"115\">");
    s.push_str(nl);
    s.push_str(&format!(
        "{i1}<heightMap tileSet=\"{}\" uvtiling=\"24.000000 24.000000 0.000000 0.000000 \" dim=\"{vx} {vy} \" offset=\"0.000000 0.000000 0.000000 \" scale=\"1.000000e+00 1.000000e+00 1.000000e+00 \">{nl}",
        xml::esc_attr(&tileset)
    ));
    s.push_str(&format!(
        "{i2}<vertData quantizeBias=\"{qb_s}\" quantizeScale=\"{qs_s}\" standardHeight=\"8.000000e+00\" name=\"{HEIGHT}\"/>{nl}"
    ));
    s.push_str(&format!("{i2}<masks name=\"{MASKS}\"/>{nl}"));
    s.push_str(&format!("{i2}<textureSetList num=\"8\">{nl}"));
    for i in 0..8 {
        let name = if i == 0 { tileset.as_str() } else { "" };
        s.push_str(&format!("{i3}<textureSet i=\"{i}\" name=\"{}\"/>{nl}", xml::esc_attr(name)));
    }
    s.push_str(&format!("{i2}</textureSetList>{nl}"));
    let tex_name = |i: usize| -> &str { t.layers.get(i).filter(|_| i < 8).map(|l| l.texture.as_str()).unwrap_or("") };
    s.push_str(&format!("{i2}<textureList num=\"64\">{nl}"));
    for i in 0..64 {
        s.push_str(&format!("{i3}<texture i=\"{i}\" name=\"{}\"/>{nl}", xml::esc_attr(tex_name(i))));
    }
    s.push_str(&format!("{i2}</textureList>{nl}"));
    let blocks = cw.div_ceil(8) * ch.div_ceil(8);
    s.push_str(&format!("{i2}<blockTextureSetList num=\"{blocks}\">{nl}"));
    for i in 0..blocks {
        s.push_str(&format!("{i3}<blockTextureSet i=\"{i}\" tileSet=\"0\"/>{nl}"));
    }
    s.push_str(&format!("{i2}</blockTextureSetList>{nl}"));
    s.push_str(&format!("{i2}<cliffCellList num=\"{}\" numOccupied=\"0\"/>{nl}", (cw / 2) * (ch / 2)));
    s.push_str(&format!("{i1}</heightMap>{nl}</terrain>"));
    put(out, XML, s.into_bytes());

    // t3HeightMap
    let mut hm = Vec::with_capacity(32 + 6 * n);
    hm.extend_from_slice(b"HMAP");
    hm.put_u32(101);
    hm.put_u32(vx);
    hm.put_u32(vy);
    hm.extend_from_slice(&[0; 16]);
    for (i, (b, a)) in pairs.iter().enumerate() {
        hm.put_u16(enc(*b, qb, qs).unwrap_or(0));
        hm.put_u16(enc(*a, qb, qs).unwrap_or(0));
        hm.put_u16(level(i) as u16);
    }
    put(out, HEIGHT, hm);

    // t3SyncHeightMap
    let mut sm = Vec::with_capacity(64 + 4 * n);
    sm.extend_from_slice(b"SMAP");
    sm.put_u32(102);
    sm.put_u32(vx);
    sm.put_u32(vy);
    sm.extend_from_slice(&[0; 12]);
    for _ in 0..3 {
        sm.put_u32(0x1000);
    }
    sm.resize(64, 0);
    for i in 0..n {
        sm.put_u16((world(i) * 256.0).round().clamp(0.0, 65535.0) as u16);
        sm.put_u16(level(i) as u16);
    }
    put(out, SYNC_HEIGHT, sm);

    // t3SyncCliffLevel
    let cells = (cw * ch) as usize;
    let mut cl = Vec::with_capacity(32 + 2 * cells);
    cl.extend_from_slice(b"CLIF");
    cl.put_u32(100);
    cl.put_u32(cw);
    cl.put_u32(ch);
    cl.resize(32, 0);
    for y in 0..ch {
        for x in 0..cw {
            let lv = [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)]
                .iter()
                .map(|&(a, b)| level((b * vx + a) as usize))
                .min()
                .unwrap_or(1);
            cl.put_u16(64 * lv as u16);
        }
    }
    put(out, CLIFF, cl);

    // t3CellFlags
    let mut cf = Vec::with_capacity(32 + cells);
    cf.extend_from_slice(b"LFCT");
    cf.put_u32(102);
    cf.extend_from_slice(&[0; 16]);
    cf.put_u32(cw);
    cf.put_u32(ch);
    for i in 0..cells {
        let blocked = t.pathing.get(i).is_some_and(|p| p & pathing::UNWALKABLE != 0);
        cf.push(FLAG_DEFAULT | if blocked { FLAG_BLOCKED } else { 0 });
    }
    put(out, FLAGS, cf);

    // t3TextureMasks
    let m = Mask { w: (cw * 8).div_ceil(64) * 64, h: (ch * 8).div_ceil(64) * 64, layer_size: 0 };
    let m = Mask { layer_size: (m.w / 64 * m.h / 64) as usize * 2048, ..m };
    let mut mk = Vec::with_capacity(64 + 8 * m.layer_size);
    mk.extend_from_slice(b"MASK");
    mk.put_u32(102);
    mk.put_u32(0);
    mk.put_u32(m.w);
    mk.put_u32(m.h);
    mk.resize(64 + 8 * m.layer_size, 0);
    let (sx, sy) = m.scale((cw, ch));
    for (l, layer) in t.layers.iter().enumerate().take(MAX_TEXTURE_LAYERS) {
        let first = layer.weights.first().copied().unwrap_or(0);
        if layer.weights.iter().all(|&w| w == first) {
            let nib = (first as f32 / 17.0).round().clamp(0.0, 15.0) as u8;
            mk[64 + l * m.layer_size..64 + (l + 1) * m.layer_size].fill((nib << 4) | nib);
            continue;
        }
        for py in 0..m.h {
            for px in 0..m.w {
                let w = bilinear(|a, b| layer.weights.get((b * vx + a) as usize).copied().unwrap_or(0), px as f32 / sx, py as f32 / sy, (cw, ch));
                m.set(&mut mk, l, px, py, (w / 17.0).round().clamp(0.0, 15.0) as u8);
            }
        }
    }
    put(out, MASKS, mk);

    // t3SyncTextureInfo
    let mut tx = Vec::new();
    tx.extend_from_slice(b"RTXT");
    tx.put_u32(101);
    tx.put_u32(cw);
    tx.put_u32(ch);
    tx.put_u32(1);
    for i in 0..8 {
        tx.put_cstr(tex_name(i));
    }
    tx.resize(tx.len() + 8 * cells, 0);
    put(out, SYNC_TEX, tx);

    // t3VertCol: one all-white 4x4-cell block entry per block
    let (bx, by) = (cw.div_ceil(4), ch.div_ceil(4));
    let mut vc = Vec::new();
    vc.extend_from_slice(b"VTCL");
    vc.put_u32(104);
    vc.put_u32(bx * by);
    vc.resize(32, 0);
    for y in 0..by {
        for x in 0..bx {
            vc.put_u32(0);
            vc.put_u32(1);
            for v in [x * 4, y * 4, (x + 1) * 4, (y + 1) * 4] {
                vc.put_f32(v as f32);
            }
            vc.put_u32(0);
            vc.put_u32(0);
            vc.put_u32(25);
            vc.extend_from_slice(&[0xFF; 25]);
        }
    }
    put(out, VERTCOL, vc);

    let empty = |magic: &[u8; 4], version: u32, len: usize| {
        let mut v = magic.to_vec();
        v.put_u32(version);
        v.resize(len, 0);
        v
    };
    put(out, WATER, empty(b"WATR", 110, 32));
    put(out, HARDTILE, empty(b"HRDT", 102, 28));
    put(out, FLUFF, empty(b"DLFT", 103, 28));
}

/// A 256x256 24-bit TGA preview shaded by height (used only when a map has none).
pub fn minimap(t: &Terrain) -> Vec<u8> {
    const S: u32 = 256;
    let mut o = vec![0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    o.put_u16(S as u16);
    o.put_u16(S as u16);
    o.push(24);
    o.push(0x20); // top-left origin
    let (lo, hi) = t.min_max_height();
    let span = (hi - lo).max(1e-3);
    for row in 0..S {
        for col in 0..S {
            let wx = (col as f32 + 0.5) / S as f32 * t.width as f32;
            let wy = (1.0 - (row as f32 + 0.5) / S as f32) * t.height as f32;
            let k = (t.sample_height(wx, wy) - lo) / span;
            let g = (60.0 + k * 150.0) as u8;
            o.extend_from_slice(&[g / 2, g, g]); // BGR, warm grey ramp
        }
    }
    o.extend_from_slice(&[0; 8]);
    o.extend_from_slice(b"TRUEVISION-XFILE.\0");
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc_with(w: u32, h: u32) -> MapDocument {
        let mut d = MapDocument::new("T", w, h);
        d.info.tileset = "Agria".into();
        d
    }

    #[test]
    fn fresh_sizes_match_editor_layout() {
        let d = doc_with(256, 256);
        let mut out = FileMap::new();
        fresh(&d, &mut out);
        assert_eq!(out[HEIGHT].len(), 396326);
        assert_eq!(out[SYNC_HEIGHT].len(), 264260);
        assert_eq!(out[CLIFF].len(), 131104);
        assert_eq!(out[FLAGS].len(), 65568);
        assert_eq!(out[MASKS].len(), 16777280);
        assert_eq!(out[VERTCOL].len(), 249888);
        assert_eq!(out[WATER].len(), 32);
        assert_eq!(out[HARDTILE].len(), 28);
    }

    #[test]
    fn fresh_roundtrips_through_load() {
        let mut d = doc_with(64, 32);
        let vx = d.terrain.verts_x();
        let i = d.terrain.vertex_index(10, 7);
        d.terrain.heights[i] = 15.5;
        d.terrain.cliff_levels[i] = 3;
        d.terrain.pathing[5] = pathing::UNWALKABLE;
        d.terrain.layers.push(TextureLayer { texture: "AgriaGrass".into(), weights: vec![0; (vx * 33) as usize] });
        d.terrain.layers[1].weights[i] = 255;
        let mut files = FileMap::new();
        fresh(&d, &mut files);
        let (t, ts) = load(&files, None).unwrap();
        assert_eq!(ts, "Agria");
        assert_eq!((t.width, t.height), (64, 32));
        assert!((t.heights[i] - 15.5).abs() < 1e-3);
        assert!((t.heights[0] - d.terrain.heights[0]).abs() < 1e-3);
        assert_eq!(t.cliff_levels, d.terrain.cliff_levels);
        assert_eq!(t.pathing, d.terrain.pathing);
        assert_eq!(t.layers.len(), 2);
        assert_eq!(t.layers[1].texture, "AgriaGrass");
        assert_eq!(t.layers[1].weights[i], 255);
        assert_eq!(t.layers[0].weights[i], 255);
    }

    #[test]
    fn patch_only_touches_changed_data() {
        let d = doc_with(64, 64);
        let mut files = FileMap::new();
        fresh(&d, &mut files);
        let (t, _) = load(&files, None).unwrap();
        let mut d2 = d.clone();
        d2.terrain = t;
        // Unchanged -> nothing written.
        let mut out = FileMap::new();
        save(&d2, &files, &mut out);
        assert!(out.is_empty(), "unexpected writes: {:?}", out.keys().collect::<Vec<_>>());
        // One vertex raised within range, one far outside (forces re-quantization).
        let a = d2.terrain.vertex_index(3, 3);
        let b = d2.terrain.vertex_index(40, 40);
        d2.terrain.heights[a] += 0.5;
        d2.terrain.heights[b] = 30.0;
        d2.terrain.cliff_levels[b] = 5;
        d2.terrain.layers[0].weights[a] = 0;
        let mut out = FileMap::new();
        save(&d2, &files, &mut out);
        assert!(out.contains_key(HEIGHT) && out.contains_key(XML) && out.contains_key(MASKS) && out.contains_key(CLIFF));
        let mut merged = files.clone();
        merged.extend(out);
        let (t3, _) = load(&merged, None).unwrap();
        assert!((t3.heights[a] - d2.terrain.heights[a]).abs() < 1e-2);
        assert!((t3.heights[b] - 30.0).abs() < 1e-2);
        assert_eq!(t3.cliff_levels[b], 5);
        assert_eq!(t3.layers[0].weights[a], 0);
        let c = d2.terrain.vertex_index(20, 20);
        assert!((t3.heights[c] - d2.terrain.heights[c]).abs() < 1e-2);
    }

    #[test]
    fn minimap_is_a_tga() {
        let t = Terrain::new(32, 32);
        assert_eq!(minimap(&t).len(), 196652);
    }
}
