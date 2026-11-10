//! A schematic overhead picture of the map, drawn on the CPU so the
//! headless MCP server and the editor produce the same image: terrain
//! colored by texture and lit by its slopes, cliffs outlined, water, the
//! playable area, a coordinate grid, regions, and objects as markers
//! (units in their owner's color, resources, doodads, start locations).
//!
//! It is meant for reading the layout (an AI client checking its work, a
//! quick preview), not as a faithful render; the app's "Export map image"
//! draws real textures and models.

use serde_json::{json, Value};

use crate::document::MapDocument;
use crate::objects::ObjectKind;
use crate::regions::Shape;
use crate::terrain::{pathing, MAX_TEXTURE_LAYERS};
use crate::walk::{Blocked, WalkGrid};

#[derive(Debug, Clone)]
pub struct Options {
    /// World rect [x0, y0, x1, y1]; the whole map when `None`.
    pub rect: Option<[f32; 4]>,
    /// Image width in pixels (height follows the rect's aspect).
    pub width: u32,
    pub objects: bool,
    pub regions: bool,
    /// Tint cells ground units can't walk on.
    pub pathing: bool,
    /// Grid line every this many cells (0 = none).
    pub grid: u32,
}

impl Default for Options {
    fn default() -> Self {
        Self { rect: None, width: 768, objects: true, regions: true, pathing: false, grid: 16 }
    }
}

pub const MAX_WIDTH: u32 = 2048;

/// An RGB image, rows top (north) to bottom.
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    /// What was drawn, for the caller to describe the picture.
    pub info: Value,
}

/// Typical color of a terrain texture, guessed from its id.
pub fn texture_color(id: &str) -> [f32; 3] {
    let l = id.to_ascii_lowercase();
    let pick: &[(&str, [f32; 3])] = &[
        ("lava", [0.80, 0.33, 0.10]),
        ("magma", [0.80, 0.33, 0.10]),
        ("creep", [0.42, 0.25, 0.45]),
        ("snow", [0.86, 0.89, 0.93]),
        ("ice", [0.72, 0.82, 0.90]),
        ("water", [0.20, 0.38, 0.55]),
        ("grass", [0.33, 0.47, 0.22]),
        ("moss", [0.30, 0.42, 0.24]),
        ("jungle", [0.28, 0.42, 0.20]),
        ("sand", [0.76, 0.66, 0.45]),
        ("dune", [0.76, 0.66, 0.45]),
        ("mud", [0.38, 0.30, 0.22]),
        ("dirt", [0.50, 0.40, 0.29]),
        ("rock", [0.47, 0.45, 0.42]),
        ("cliff", [0.40, 0.38, 0.36]),
        ("gravel", [0.52, 0.50, 0.46]),
        ("ash", [0.36, 0.35, 0.35]),
        ("asphalt", [0.30, 0.31, 0.33]),
        ("metal", [0.42, 0.45, 0.50]),
        ("plate", [0.42, 0.45, 0.50]),
        ("tile", [0.50, 0.50, 0.52]),
        ("brick", [0.55, 0.36, 0.30]),
        ("crystal", [0.45, 0.60, 0.75]),
    ];
    if let Some((_, c)) = pick.iter().find(|(k, _)| l.contains(k)) {
        return *c;
    }
    // Something stable and muted for anything else.
    let mut h: u32 = 2166136261;
    for b in l.bytes() {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    let f = |s: u32| 0.30 + ((h >> s) & 0xFF) as f32 / 255.0 * 0.30;
    [f(0), f(8), f(16)]
}

fn player_color(doc: &MapDocument, p: u8) -> [u8; 3] {
    if p == 0 {
        return [200, 200, 200];
    }
    doc.info.players.iter().find(|x| x.id == p).map(|x| x.color).unwrap_or_else(|| {
        const DEFAULT: [[u8; 3]; 8] = [
            [180, 20, 30],
            [0, 66, 255],
            [28, 167, 234],
            [84, 0, 129],
            [235, 225, 41],
            [254, 138, 14],
            [22, 128, 0],
            [204, 166, 252],
        ];
        DEFAULT[(p as usize - 1) % DEFAULT.len()]
    })
}

pub fn render(doc: &MapDocument, o: &Options) -> Result<Image, String> {
    let t = &doc.terrain;
    let (mw, mh) = (t.width as f32, t.height as f32);
    let [x0, y0, x1, y1] = o.rect.unwrap_or([0.0, 0.0, mw, mh]);
    let (x0, x1) = (x0.min(x1).max(0.0), x0.max(x1).min(mw));
    let (y0, y1) = (y0.min(y1).max(0.0), y0.max(y1).min(mh));
    if !(x1 - x0 >= 1.0 && y1 - y0 >= 1.0) {
        return Err(format!("rect must cover at least one cell inside the {mw}x{mh} map"));
    }
    let width = o.width.clamp(64, MAX_WIDTH);
    let ppc = width as f32 / (x1 - x0);
    let height = (((y1 - y0) * ppc).round() as u32).clamp(16, MAX_WIDTH * 2);
    let (w, h) = (width as usize, height as usize);
    let world = |px: f32, py: f32| (x0 + px / ppc, y1 - py / ppc);
    let to_px = |x: f32, y: f32| ((x - x0) * ppc, (y1 - y) * ppc);

    let (lo, hi) = t.min_max_height();
    let span = (hi - lo).max(1.0);
    let colors: Vec<[f32; 3]> = t.layers.iter().map(|l| texture_color(&l.texture)).collect();
    let g = WalkGrid::new(doc);
    let [pl, pb, pr, pt] = doc.info.playable.map(|v| v as f32);
    let sun = glam::Vec3::new(-0.6943, 0.3237, 0.6428);

    let mut rgb = vec![0u8; w * h * 3];
    for py in 0..h {
        for px in 0..w {
            let (x, y) = world(px as f32 + 0.5, py as f32 + 0.5);
            let (vx, vy) = (x.clamp(0.0, mw), y.clamp(0.0, mh));
            // Texture blend at the nearest vertex.
            let vi = t.vertex_index(vx.round() as u32, vy.round() as u32);
            let wts = t.blended_weights(vi);
            let mut c = [0.0f32; 3];
            for (i, col) in colors.iter().enumerate().take(MAX_TEXTURE_LAYERS) {
                for k in 0..3 {
                    c[k] += col[k] * wts[i];
                }
            }
            if colors.is_empty() {
                c = [0.45, 0.42, 0.38];
            }
            // Slope light and height tint.
            let z = t.sample_height(vx, vy);
            let e = 0.5;
            let n = glam::Vec3::new(
                t.sample_height((vx - e).max(0.0), vy) - t.sample_height((vx + e).min(mw), vy),
                t.sample_height(vx, (vy - e).max(0.0)) - t.sample_height(vx, (vy + e).min(mh)),
                2.0 * e,
            )
            .normalize_or_zero();
            let light = 0.55 + 0.6 * n.dot(sun).max(0.0);
            let lift = 0.75 + 0.45 * (z - lo) / span;
            let mut f = [c[0] * light * lift, c[1] * light * lift, c[2] * light * lift];
            if let Some(wz) = t.water_height.filter(|wz| z < *wz) {
                let d = ((wz - z) / 3.0).clamp(0.3, 0.8);
                f = [f[0] * (1.0 - d) + 0.12 * d, f[1] * (1.0 - d) + 0.32 * d, f[2] * (1.0 - d) + 0.55 * d];
            }
            let cell = (x.floor().clamp(0.0, mw - 1.0) as u32, y.floor().clamp(0.0, mh - 1.0) as u32);
            match g.blocked(cell) {
                Some(Blocked::Cliff) => f = [f[0] * 0.35, f[1] * 0.35, f[2] * 0.35],
                Some(Blocked::Unpathable) if o.pathing => f = [f[0] * 0.6 + 0.35, f[1] * 0.5, f[2] * 0.5],
                _ => {}
            }
            if !o.pathing && t.pathing[t.cell_index(cell.0, cell.1)] & pathing::UNWALKABLE != 0 {
                f = [f[0] * 0.75, f[1] * 0.75, f[2] * 0.75];
            }
            if x < pl || x > pr || y < pb || y > pt {
                f = [f[0] * 0.45, f[1] * 0.45, f[2] * 0.5];
            }
            if o.grid > 0 {
                let gx = (x / o.grid as f32).round() * o.grid as f32;
                let gy = (y / o.grid as f32).round() * o.grid as f32;
                if (x - gx).abs() * ppc < 0.5 || (y - gy).abs() * ppc < 0.5 {
                    f = [f[0] * 0.8 + 0.1, f[1] * 0.8 + 0.1, f[2] * 0.8 + 0.1];
                }
            }
            let i = (py * w + px) * 3;
            for k in 0..3 {
                rgb[i + k] = (f[k].clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }

    let mut canvas = Canvas { w, h, rgb };
    // Playable bounds.
    let (a, b) = (to_px(pl, pt), to_px(pr, pb));
    canvas.rect_outline(a, b, [235, 235, 235]);

    let mut drawn_regions = Vec::new();
    if o.regions {
        for r in &doc.regions {
            for s in &r.shapes {
                match s {
                    Shape::Rect { min, max } => canvas.rect_outline(to_px(min.x, max.y), to_px(max.x, min.y), r.color),
                    Shape::Circle { center, radius } => {
                        let c = to_px(center.x, center.y);
                        canvas.ring(c, radius * ppc, r.color);
                    }
                    Shape::Polygon { points } => {
                        for (i, p) in points.iter().enumerate() {
                            let q = points[(i + 1) % points.len()];
                            canvas.line(to_px(p.x, p.y), to_px(q.x, q.y), r.color);
                        }
                    }
                }
            }
            drawn_regions.push(r.name.clone());
        }
    }

    let (mut units, mut doodads, mut resources, mut starts) = (0, 0, 0, Vec::new());
    if o.objects {
        let mut objs: Vec<_> = doc.objects.objects.iter().collect();
        // Doodads under units, start locations on top.
        objs.sort_by_key(|o| match o.kind {
            ObjectKind::Doodad => 0,
            ObjectKind::Unit => 1,
            ObjectKind::Point => 2,
        });
        for ob in objs {
            let p = to_px(ob.position.x, ob.position.y);
            if p.0 < -8.0 || p.1 < -8.0 || p.0 > w as f32 + 8.0 || p.1 > h as f32 + 8.0 {
                continue;
            }
            let ty = ob.type_id.to_ascii_lowercase();
            match ob.kind {
                ObjectKind::Doodad => {
                    doodads += 1;
                    canvas.disc(p, (0.45 * ppc * ob.scale.x.max(0.3)).max(1.0), [70, 66, 60]);
                }
                ObjectKind::Unit if ty.contains("mineralfield") => {
                    resources += 1;
                    canvas.disc(p, (0.9 * ppc).max(1.5), [90, 190, 255]);
                }
                ObjectKind::Unit if ty.contains("geyser") => {
                    resources += 1;
                    canvas.disc(p, (1.5 * ppc).max(2.0), [70, 220, 110]);
                }
                ObjectKind::Unit => {
                    units += 1;
                    let c = player_color(doc, ob.player);
                    canvas.disc(p, (0.7 * ppc * ob.scale.x.max(0.5)).max(2.0), [20, 20, 20]);
                    canvas.disc(p, (0.7 * ppc * ob.scale.x.max(0.5) - 1.0).max(1.0), c);
                }
                ObjectKind::Point if ob.type_id == "StartLoc" => {
                    canvas.ring(p, (2.5 * ppc).max(5.0), [255, 255, 255]);
                    canvas.ring(p, (2.5 * ppc).max(5.0) - 1.0, [20, 20, 20]);
                    canvas.disc(p, (0.6 * ppc).max(2.0), [255, 255, 255]);
                    starts.push(json!({"name": ob.name, "x": ob.position.x, "y": ob.position.y}));
                }
                ObjectKind::Point => canvas.disc(p, (0.4 * ppc).max(1.5), [250, 220, 90]),
            }
        }
        for p in &doc.info.players {
            if let Some(s) = p.start_location {
                let q = to_px(s.x, s.y);
                canvas.ring(q, (2.5 * ppc).max(5.0), player_color(doc, p.id));
            }
        }
    }

    let info = json!({
        "width": width,
        "height": height,
        "rect": [x0, y0, x1, y1],
        "pixels_per_cell": (ppc * 100.0).round() / 100.0,
        "orientation": "north (higher y) is up; x grows to the right",
        "grid_every_cells": o.grid,
        "legend": {
            "terrain": "colored by texture, lit from the north-west; brighter is higher",
            "dark outlines": "cliff edges",
            "dimmed area": "outside the playable area (white frame)",
            "blue": "water",
            "colored dots with dark rims": "units, in their owner's color",
            "light blue dots": "mineral fields",
            "green dots": "vespene geysers",
            "dark brown dots": "doodads",
            "white rings": "start locations",
            "colored outlines": "regions",
            "red tint": if o.pathing { "cells ground units can't walk on" } else { "(pathing off)" },
        },
        "counts": {"units": units, "resources": resources, "doodads": doodads, "regions": drawn_regions.len()},
        "start_locations": starts,
        "textures": t.layers.iter().map(|l| l.texture.clone()).collect::<Vec<_>>(),
    });
    Ok(Image { width, height, rgb: canvas.rgb, info })
}

struct Canvas {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Canvas {
    fn put(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let i = (y as usize * self.w + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }
    fn disc(&mut self, c: (f32, f32), r: f32, col: [u8; 3]) {
        let ri = r.ceil() as i64;
        for dy in -ri..=ri {
            for dx in -ri..=ri {
                if (dx * dx + dy * dy) as f32 <= r * r {
                    self.put(c.0 as i64 + dx, c.1 as i64 + dy, col);
                }
            }
        }
    }
    fn ring(&mut self, c: (f32, f32), r: f32, col: [u8; 3]) {
        let steps = ((r * 8.0) as usize).clamp(16, 4096);
        for i in 0..steps {
            let a = i as f32 / steps as f32 * std::f32::consts::TAU;
            self.put((c.0 + r * a.cos()) as i64, (c.1 + r * a.sin()) as i64, col);
        }
    }
    fn line(&mut self, a: (f32, f32), b: (f32, f32), col: [u8; 3]) {
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).clamp(1, 16384);
        for i in 0..=n {
            let t = i as f32 / n as f32;
            self.put((a.0 + (b.0 - a.0) * t) as i64, (a.1 + (b.1 - a.1) * t) as i64, col);
        }
    }
    fn rect_outline(&mut self, a: (f32, f32), b: (f32, f32), col: [u8; 3]) {
        self.line(a, (b.0, a.1), col);
        self.line((b.0, a.1), b, col);
        self.line(b, (a.0, b.1), col);
        self.line((a.0, b.1), a, col);
    }
}

/// Render and encode as PNG.
pub fn render_png(doc: &MapDocument, o: &Options) -> Result<(Vec<u8>, Value), String> {
    let img = render(doc, o)?;
    let png = crate::png::encode(img.width, img.height, 3, &img.rgb)?;
    Ok((png, img.info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{ObjectId, PlacedObject};

    #[test]
    fn draws_terrain_objects_and_regions() {
        let mut d = MapDocument::new("img", 64, 48);
        let mut u = PlacedObject::new(ObjectId(5), ObjectKind::Unit, "Marine", glam::Vec3::new(32.0, 24.0, 0.0));
        u.player = 1;
        d.objects.objects.push(u);
        let s = PlacedObject::new(ObjectId(6), ObjectKind::Point, "StartLoc", glam::Vec3::new(16.0, 16.0, 0.0));
        d.objects.objects.push(s);
        let img = render(&d, &Options { width: 256, ..Default::default() }).unwrap();
        assert_eq!((img.width, img.height), (256, 192));
        // The unit's pixel is player 1's color.
        let px = |x: f32, y: f32| {
            let (px, py) = ((x * 4.0) as usize, ((48.0 - y) * 4.0) as usize);
            let i = (py * 256 + px) * 3;
            [img.rgb[i], img.rgb[i + 1], img.rgb[i + 2]]
        };
        assert_eq!(px(32.0, 24.0), d.info.players[0].color);
        assert_eq!(px(16.0, 16.0), [255, 255, 255]);
        assert_eq!(img.info["counts"]["units"], 1);

        let crop = render(&d, &Options { rect: Some([24.0, 16.0, 40.0, 32.0]), width: 128, ..Default::default() }).unwrap();
        assert_eq!((crop.width, crop.height), (128, 128));
        assert!(render(&d, &Options { rect: Some([70.0, 0.0, 80.0, 5.0]), ..Default::default() }).is_err());
        let (png, _) = render_png(&d, &Options::default()).unwrap();
        assert_eq!(&png[1..4], b"PNG");
    }
}
