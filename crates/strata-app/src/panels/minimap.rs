//! Overview of the whole map. Click to move the camera, drag to scrub.

use egui::{Color32, Pos2, Rect, Sense, Stroke};
use glam::Vec2;
use strata_core::objects::ObjectKind;
use strata_core::terrain::Terrain;

use crate::render::mesh::texture_color;
use crate::render::overlay::player_color;
use crate::state::EditorState;
use crate::theme;

/// Seconds between rebuilds while the map keeps changing.
const THROTTLE: f64 = 0.25;
/// Longest image side; larger maps are downsampled.
const MAX_SIDE: u32 = 512;

#[derive(Default)]
pub struct MinimapState {
    texture: Option<egui::TextureHandle>,
    revision: Option<u64>,
    built_at: f64,
}

/// Shaded color image of the terrain, north at the top.
pub fn terrain_image(t: &Terrain) -> egui::ColorImage {
    let step = t.width.max(t.height).div_ceil(MAX_SIDE).max(1);
    let w = (t.width / step).max(1);
    let h = (t.height / step).max(1);
    let colors: Vec<[f32; 3]> = t.layers.iter().map(|l| texture_color(&l.texture).map(|c| c as f32 / 255.0)).collect();
    let (lo, hi) = t.min_max_height();
    let span = (hi - lo).max(1.0);
    // Same key light as the 3D view so relief reads the same way.
    let sun = crate::render::gl::SUN;
    let mut pixels = Vec::with_capacity((w * h) as usize);
    for row in 0..h {
        let y = (h - 1 - row) * step;
        for col in 0..w {
            let x = col * step;
            let vi = t.vertex_index(x, y);
            let weights = t.blended_weights(vi);
            let mut c = [0.0f32; 3];
            for (li, lc) in colors.iter().enumerate() {
                for k in 0..3 {
                    c[k] += lc[k] * weights[li];
                }
            }
            let n = t.normal_at_vertex(x, y);
            let light = 0.42 + 0.9 * n.dot(sun).max(0.0);
            let hgt = 0.86 + 0.22 * ((t.heights[vi] - lo) / span);
            let mut rgb = c.map(|v| v * light * hgt);
            if let Some(wz) = t.water_height {
                if t.heights[vi] < wz {
                    let water = [0.16, 0.36, 0.52];
                    for k in 0..3 {
                        rgb[k] = rgb[k] * 0.35 + water[k] * 0.65;
                    }
                }
            }
            let px = rgb.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
            pixels.push(Color32::from_rgb(px[0], px[1], px[2]));
        }
    }
    egui::ColorImage::new([w as usize, h as usize], pixels)
}

/// Fit a `w x h` map into `avail`, keeping its aspect ratio, centered.
pub fn fit_rect(avail: Rect, w: f32, h: f32) -> Rect {
    let s = (avail.width() / w).min(avail.height() / h).max(0.0);
    Rect::from_center_size(avail.center(), egui::vec2(w * s, h * s))
}

pub fn show(ui: &mut egui::Ui, state: &mut EditorState) {
    let ctx = ui.ctx().clone();
    let tk = theme::tokens(&ctx);
    let now = ctx.input(|i| i.time);
    let rev = state.history.revision();
    let t = &state.doc.terrain;
    let ms = &mut state.ui.minimap;
    let stale = ms.revision != Some(rev);
    if ms.texture.is_none() || (stale && now - ms.built_at >= THROTTLE) {
        let img = terrain_image(t);
        match ms.texture.as_mut() {
            Some(tex) => tex.set(img, egui::TextureOptions::LINEAR),
            None => ms.texture = Some(ctx.load_texture("strata-minimap", img, egui::TextureOptions::LINEAR)),
        }
        ms.revision = Some(rev);
        ms.built_at = now;
    } else if stale {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(THROTTLE));
    }

    let avail = ui.available_rect_before_wrap().shrink(6.0);
    if avail.width() < 16.0 || avail.height() < 16.0 {
        return;
    }
    let (mw, mh) = (t.width as f32, t.height as f32);
    let rect = fit_rect(avail, mw, mh);
    let response = ui.allocate_rect(avail, Sense::click_and_drag());
    let painter = ui.painter_at(avail);
    painter.rect_filled(rect.expand(1.0), 2.0, tk.stroke);
    if let Some(tex) = &state.ui.minimap.texture {
        painter.image(tex.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
    }
    let to_px = |p: Vec2| Pos2::new(rect.min.x + p.x / mw * rect.width(), rect.max.y - p.y / mh * rect.height());
    let to_world = |p: Pos2| {
        Vec2::new(
            ((p.x - rect.min.x) / rect.width() * mw).clamp(0.0, mw),
            ((rect.max.y - p.y) / rect.height() * mh).clamp(0.0, mh),
        )
    };

    // Darken outside the playable bounds and outline them.
    let pb = state.doc.info.playable;
    let play =
        Rect::from_two_pos(to_px(Vec2::new(pb[0] as f32, pb[1] as f32)), to_px(Vec2::new(pb[2] as f32, pb[3] as f32)));
    // Darken (in both themes) like the 3D view does outside the bounds.
    let shade = Color32::from_black_alpha(120);
    for r in [
        Rect::from_min_max(rect.min, Pos2::new(rect.max.x, play.min.y)),
        Rect::from_min_max(Pos2::new(rect.min.x, play.max.y), rect.max),
        Rect::from_min_max(Pos2::new(rect.min.x, play.min.y), Pos2::new(play.min.x, play.max.y)),
        Rect::from_min_max(Pos2::new(play.max.x, play.min.y), Pos2::new(rect.max.x, play.max.y)),
    ] {
        if r.is_positive() {
            painter.rect_filled(r, 0.0, shade);
        }
    }
    painter.rect_stroke(play, 0.0, Stroke::new(1.0, tk.text_dim), egui::StrokeKind::Middle);

    // Objects, with a dark rim so they read on any terrain.
    let dot = (rect.width() / mw * 0.9).clamp(2.0, 3.5);
    let rim = Stroke::new(1.0, Color32::from_black_alpha(160));
    for o in &state.doc.objects.objects {
        let p = to_px(o.position.truncate());
        let selected = state.selection.objects.contains(&o.id);
        let c = if selected {
            tk.accent
        } else {
            match o.kind {
                ObjectKind::Unit => player_color(&state.doc, o.player, &tk),
                ObjectKind::Doodad => continue,
                ObjectKind::Point if o.type_id == strata_core::known::POINT_START_LOCATION => tk.ok,
                ObjectKind::Point => tk.text,
            }
        };
        let r = if selected { dot + 1.0 } else { dot };
        painter.circle_filled(p, r, c);
        painter.circle_stroke(p, r + 0.5, rim);
    }

    // Camera footprint, clipped to the map image (far corners of a low
    // camera land well beyond the map).
    if let Some(fp) = state.ui.viewport.footprint {
        let pts: Vec<Pos2> = fp.iter().map(|p| to_px(*p)).collect();
        let clipped = painter.with_clip_rect(rect);
        clipped.add(egui::Shape::closed_line(pts, Stroke::new(1.5, tk.text)));
    }

    if let Some(p) = response.interact_pointer_pos() {
        let w = to_world(p);
        if response.dragged() {
            crate::panels::viewport::jump_to(state, w);
        } else if response.clicked() {
            crate::panels::viewport::frame_world(state, w);
        }
    }
    response.on_hover_text("Click to move the camera. Drag to scrub.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_is_north_up_and_sized() {
        let mut t = Terrain::new(8, 4);
        // Paint grass on the north rows only.
        let mut w = vec![0u8; 9 * 5];
        for y in 3..=4 {
            for x in 0..=8 {
                w[t.vertex_index(x, y)] = 255;
            }
        }
        t.layers.push(strata_core::terrain::TextureLayer { texture: "AgriaGrassGreen".into(), weights: w });
        let img = terrain_image(&t);
        assert_eq!(img.size, [8, 4]);
        let top = img.pixels[0];
        let bottom = img.pixels[img.pixels.len() - 1];
        assert!(top.g() > top.r(), "north row is grass: {top:?}");
        assert_ne!(top, bottom);
    }

    #[test]
    fn large_maps_are_downsampled() {
        let t = Terrain::new(1024, 256);
        let img = terrain_image(&t);
        assert_eq!(img.size, [512, 128]);
    }

    #[test]
    fn fit_keeps_aspect() {
        let r = fit_rect(Rect::from_min_size(Pos2::ZERO, egui::vec2(200.0, 100.0)), 64.0, 64.0);
        assert_eq!(r.size(), egui::vec2(100.0, 100.0));
        assert_eq!(r.center(), Pos2::new(100.0, 50.0));
    }
}
