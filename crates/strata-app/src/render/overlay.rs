//! Object, region and camera overlays drawn with the egui painter on top of
//! the GL terrain. Everything is culled to the viewport so thousands of
//! objects stay cheap.

use egui::{Color32, Pos2, Stroke};
use glam::{Vec2, Vec3};
use strata_core::objects::{ObjectId, ObjectKind, PlacedObject};
use strata_core::regions::{Camera, Shape};
use strata_core::terrain::Terrain;
use strata_core::MapDocument;

use super::camera::View;
use crate::state::EditorState;
use crate::theme::Tokens;
use crate::tools::select::object_screen_radius;

/// Display color for a player slot; neutral and unknown players use `text_dim`.
pub fn player_color(doc: &MapDocument, player: u8, tk: &Tokens) -> Color32 {
    if player == 0 {
        return tk.text_dim;
    }
    doc.info
        .players
        .iter()
        .find(|p| p.id == player)
        .map(|p| Color32::from_rgb(p.color[0], p.color[1], p.color[2]))
        .unwrap_or(tk.text_dim)
}

fn to_screen(origin: Pos2, p: Vec2) -> Pos2 {
    origin + egui::vec2(p.x, p.y)
}

/// Draw one placed object. `highlight` draws a ring (selection / hover).
#[allow(clippy::too_many_arguments)]
pub fn draw_object(
    painter: &egui::Painter,
    view: &View,
    origin: Pos2,
    o: &PlacedObject,
    color: Color32,
    highlight: Option<Color32>,
    label: bool,
    tk: &Tokens,
) {
    let Some(s) = view.project(o.position) else { return };
    let c = to_screen(origin, s);
    let r = object_screen_radius(o, view);
    let ppu = view.points_per_unit(o.position);
    let outline = Stroke::new(1.0, tk.bg_deep.gamma_multiply(0.85));
    match o.kind {
        ObjectKind::Unit => {
            painter.circle_filled(c, r, color.gamma_multiply(0.55));
            painter.circle_stroke(c, r, Stroke::new(1.5, color));
            // Facing tick, projected so it points the right way in perspective.
            let dir = Vec3::new(o.rotation.cos(), o.rotation.sin(), 0.0);
            let tip =
                view.project(o.position + dir * (r / ppu.max(1e-3)) * 1.35).map(|t| to_screen(origin, t)).unwrap_or(c);
            painter.line_segment([c, tip], Stroke::new(2.0, color));
            if let Some(h) = highlight {
                painter.circle_stroke(c, r + 3.0, Stroke::new(2.0, h));
            }
            if label && ppu > 16.0 && !o.type_id.is_empty() {
                let name = strata_core::known::unit_name(&o.type_id).unwrap_or(o.display_name());
                text(painter, c + egui::vec2(0.0, r + 8.0), name, tk);
            }
        }
        ObjectKind::Doodad => {
            let d = r * 0.85;
            let pts =
                vec![c + egui::vec2(0.0, -d), c + egui::vec2(d, 0.0), c + egui::vec2(0.0, d), c + egui::vec2(-d, 0.0)];
            painter.add(egui::Shape::convex_polygon(pts, color.gamma_multiply(0.75), outline));
            if let Some(h) = highlight {
                let e = d + 3.0;
                let ring = vec![
                    c + egui::vec2(0.0, -e),
                    c + egui::vec2(e, 0.0),
                    c + egui::vec2(0.0, e),
                    c + egui::vec2(-e, 0.0),
                ];
                painter.add(egui::Shape::closed_line(ring, Stroke::new(2.0, h)));
            }
            if label && ppu > 22.0 {
                text(painter, c + egui::vec2(0.0, d + 8.0), o.display_name(), tk);
            }
        }
        ObjectKind::Point => {
            let start = o.type_id == strata_core::known::POINT_START_LOCATION;
            let col = highlight.unwrap_or(if start { tk.ok } else { color });
            if start {
                painter.circle_stroke(c, 7.0, Stroke::new(2.0, col));
                painter.circle_filled(c, 2.5, col);
            } else {
                let top = c + egui::vec2(0.0, -16.0);
                painter.line_segment([c, top], Stroke::new(1.5, col));
                let flag = vec![top, top + egui::vec2(10.0, 3.5), top + egui::vec2(0.0, 7.0)];
                painter.add(egui::Shape::convex_polygon(flag, col, Stroke::NONE));
                painter.circle_filled(c, 2.0, col);
            }
            if label && (ppu > 3.0 || highlight.is_some()) {
                let at = if start { c + egui::vec2(0.0, 15.0) } else { c + egui::vec2(14.0, -12.0) };
                let align = if start { egui::Align2::CENTER_CENTER } else { egui::Align2::LEFT_CENTER };
                text_aligned(painter, at, align, o.display_name(), tk);
            }
        }
    }
}

fn text(painter: &egui::Painter, at: Pos2, s: &str, tk: &Tokens) {
    text_aligned(painter, at, egui::Align2::CENTER_CENTER, s, tk);
}

fn text_aligned(painter: &egui::Painter, at: Pos2, align: egui::Align2, s: &str, tk: &Tokens) {
    let font = egui::FontId::proportional(12.0);
    // Cheap shadow for legibility over bright terrain.
    painter.text(at + egui::vec2(1.0, 1.0), align, s, font.clone(), tk.bg_deep);
    painter.text(at, align, s, font, tk.text);
}

/// Boundary of a shape sampled roughly every `step` world units.
pub fn shape_outline(shape: &Shape, step: f32) -> Vec<Vec2> {
    match shape {
        Shape::Rect { min, max } => {
            let corners = [*min, Vec2::new(max.x, min.y), *max, Vec2::new(min.x, max.y)];
            let mut out = Vec::new();
            for i in 0..4 {
                let (a, b) = (corners[i], corners[(i + 1) % 4]);
                let n = ((a.distance(b) / step).ceil() as usize).clamp(1, 64);
                out.extend((0..n).map(|k| a.lerp(b, k as f32 / n as f32)));
            }
            out
        }
        Shape::Circle { center, radius } => {
            let n = ((radius * std::f32::consts::TAU / step).ceil() as usize).clamp(24, 128);
            (0..n)
                .map(|k| {
                    let a = k as f32 / n as f32 * std::f32::consts::TAU;
                    *center + Vec2::new(a.cos(), a.sin()) * *radius
                })
                .collect()
        }
        Shape::Polygon { points } => {
            let mut out = Vec::new();
            for i in 0..points.len() {
                let (a, b) = (points[i], points[(i + 1) % points.len()]);
                let n = ((a.distance(b) / step).ceil() as usize).clamp(1, 64);
                out.extend((0..n).map(|k| a.lerp(b, k as f32 / n as f32)));
            }
            out
        }
    }
}

fn lift(t: &Terrain, p: Vec2) -> Vec3 {
    Vec3::new(p.x, p.y, t.sample_height(p.x, p.y) + 0.08)
}

/// Draw a region shape draped over the terrain: translucent fill + outline.
pub fn draw_shape(
    painter: &egui::Painter,
    view: &View,
    origin: Pos2,
    t: &Terrain,
    shape: &Shape,
    color: Color32,
    strong: bool,
) {
    let fill = color.gamma_multiply(if strong { 0.28 } else { 0.16 });
    let mut mesh = egui::Mesh::default();
    let proj = |p: Vec2| view.project(lift(t, p)).map(|s| to_screen(origin, s));
    let mut push_grid = |pts: Vec<Option<Pos2>>, cols: usize, rows: usize| {
        let base = mesh.vertices.len() as u32;
        for p in &pts {
            mesh.colored_vertex(p.unwrap_or(Pos2::ZERO), fill);
        }
        for y in 0..rows.saturating_sub(1) {
            for x in 0..cols.saturating_sub(1) {
                let i = y * cols + x;
                let q = [i, i + 1, i + cols, i + cols + 1];
                if q.iter().all(|&k| pts[k].is_some()) {
                    let [a, b, c, d] = q.map(|k| base + k as u32);
                    mesh.add_triangle(a, b, d);
                    mesh.add_triangle(a, d, c);
                }
            }
        }
    };
    match shape {
        Shape::Rect { min, max } => {
            let size = *max - *min;
            let nx = ((size.x / 3.0).ceil() as usize).clamp(1, 24) + 1;
            let ny = ((size.y / 3.0).ceil() as usize).clamp(1, 24) + 1;
            let mut pts = Vec::with_capacity(nx * ny);
            for y in 0..ny {
                for x in 0..nx {
                    let f = Vec2::new(x as f32 / (nx - 1) as f32, y as f32 / (ny - 1) as f32);
                    pts.push(proj(*min + size * f));
                }
            }
            push_grid(pts, nx, ny);
        }
        Shape::Circle { center, radius } => {
            let seg = 48;
            let rings = ((radius / 3.0).ceil() as usize).clamp(2, 12) + 1;
            let mut pts = Vec::with_capacity(seg * rings);
            for ring in 0..rings {
                let rr = *radius * ring as f32 / (rings - 1) as f32;
                for k in 0..=seg {
                    let a = k as f32 / seg as f32 * std::f32::consts::TAU;
                    pts.push(proj(*center + Vec2::new(a.cos(), a.sin()) * rr));
                }
            }
            push_grid(pts, seg + 1, rings);
        }
        Shape::Polygon { points } => {
            // Fan fill; exact for convex polygons, approximate otherwise.
            if points.len() >= 3 {
                let pts: Vec<Option<Pos2>> = points.iter().map(|p| proj(*p)).collect();
                if pts.iter().all(Option::is_some) {
                    let base = mesh.vertices.len() as u32;
                    for p in &pts {
                        mesh.colored_vertex(p.unwrap_or(Pos2::ZERO), fill);
                    }
                    for i in 1..pts.len() as u32 - 1 {
                        mesh.add_triangle(base, base + i, base + i + 1);
                    }
                }
            }
        }
    }
    if !mesh.indices.is_empty() {
        painter.add(egui::Shape::mesh(mesh));
    }
    let outline: Vec<Pos2> = shape_outline(shape, 1.0).into_iter().filter_map(proj).collect();
    if outline.len() > 2 {
        painter.add(egui::Shape::closed_line(outline, Stroke::new(if strong { 2.0 } else { 1.5 }, color)));
    }
}

/// Eye position of a placed SC2 camera.
pub fn camera_eye(c: &Camera, t: &Terrain) -> Vec3 {
    let target = Vec3::new(c.target.x, c.target.y, t.sample_height(c.target.x, c.target.y));
    let (yaw, pitch) = (c.yaw.to_radians(), c.pitch.to_radians());
    let f = Vec2::new(yaw.cos(), yaw.sin());
    target + c.distance * Vec3::new(-f.x * pitch.cos(), -f.y * pitch.cos(), pitch.sin())
}

fn draw_camera(
    painter: &egui::Painter,
    view: &View,
    origin: Pos2,
    t: &Terrain,
    c: &Camera,
    color: Color32,
    tk: &Tokens,
) {
    let target = Vec3::new(c.target.x, c.target.y, t.sample_height(c.target.x, c.target.y));
    let Some(ts) = view.project(target).map(|s| to_screen(origin, s)) else { return };
    let eye = camera_eye(c, t);
    let yaw = c.yaw.to_radians();
    let right = Vec3::new(yaw.sin(), -yaw.cos(), 0.0);
    let half = c.distance * (c.fov.to_radians() * 0.5).tan();
    let stroke = Stroke::new(1.5, color);
    if let (Some(e), Some(l), Some(r)) = (
        view.project(eye).map(|s| to_screen(origin, s)),
        view.project(target - right * half).map(|s| to_screen(origin, s)),
        view.project(target + right * half).map(|s| to_screen(origin, s)),
    ) {
        painter.line_segment([e, l], stroke);
        painter.line_segment([e, r], stroke);
        painter.line_segment([l, r], stroke);
        painter.circle_filled(e, 3.0, color);
    }
    painter.circle_stroke(ts, 5.0, stroke);
    painter.line_segment([ts - egui::vec2(8.0, 0.0), ts + egui::vec2(8.0, 0.0)], Stroke::new(1.0, color));
    painter.line_segment([ts - egui::vec2(0.0, 8.0), ts + egui::vec2(0.0, 8.0)], Stroke::new(1.0, color));
    text_aligned(painter, ts + egui::vec2(10.0, 10.0), egui::Align2::LEFT_CENTER, &c.name, tk);
}

/// Draw every region, camera and object, culled to `rect`.
pub fn draw_document(
    painter: &egui::Painter,
    state: &EditorState,
    view: &View,
    rect: egui::Rect,
    hovered: Option<ObjectId>,
    tk: &Tokens,
) {
    let doc = &state.doc;
    let origin = rect.min;
    let cull = rect.expand(48.0);
    let t = &doc.terrain;

    for r in &doc.regions {
        let selected = state.selection.region == Some(r.id);
        let color = if selected { tk.accent } else { Color32::from_rgb(r.color[0], r.color[1], r.color[2]) };
        for s in &r.shapes {
            let (lo, hi) = s.bounds();
            let c = (lo + hi) * 0.5;
            let corners = [lo, hi, Vec2::new(lo.x, hi.y), Vec2::new(hi.x, lo.y), c];
            let visible =
                corners.iter().any(|p| view.project(lift(t, *p)).is_some_and(|q| cull.contains(to_screen(origin, q))))
                    || (lo.x <= 0.0 && hi.x >= doc.terrain.width as f32);
            if visible {
                draw_shape(painter, view, origin, t, s, color, selected);
            }
        }
        if let Some(first) = r.shapes.first() {
            let (lo, hi) = first.bounds();
            if let Some(p) = view.project(lift(t, (lo + hi) * 0.5)) {
                let p = to_screen(origin, p);
                if cull.contains(p) {
                    text(painter, p, &r.name, tk);
                }
            }
        }
    }

    for c in &doc.cameras {
        let color = if state.selection.camera == Some(c.id) { tk.accent } else { tk.text };
        draw_camera(painter, view, origin, t, c, color, tk);
    }

    // Labels only when few enough objects are on screen to stay readable.
    let on_screen: Vec<&PlacedObject> = doc
        .objects
        .objects
        .iter()
        .filter(|o| view.project(o.position).is_some_and(|s| cull.contains(to_screen(origin, s))))
        .collect();
    let labels = on_screen.len() <= 400;
    for o in on_screen {
        let selected = state.selection.objects.contains(&o.id);
        let color = match o.kind {
            ObjectKind::Unit => player_color(doc, o.player, tk),
            ObjectKind::Doodad => tk.text_dim,
            ObjectKind::Point => tk.text,
        };
        let highlight = if selected {
            Some(tk.accent)
        } else if hovered == Some(o.id) {
            Some(tk.text)
        } else {
            None
        };
        draw_object(painter, view, origin, o, color, highlight, labels, tk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_outline_is_closed_and_on_edges() {
        let s = Shape::Rect { min: Vec2::ZERO, max: Vec2::new(4.0, 2.0) };
        let o = shape_outline(&s, 1.0);
        assert_eq!(o.len(), 4 + 2 + 4 + 2);
        assert!(o.iter().all(|p| p.x == 0.0 || p.x == 4.0 || p.y == 0.0 || p.y == 2.0));
    }

    #[test]
    fn camera_eye_matches_pitch() {
        let t = Terrain::new(16, 16);
        let mut c = Camera::new(ObjectId(1), "c", Vec2::new(8.0, 8.0));
        c.pitch = 90.0;
        let e = camera_eye(&c, &t);
        assert!((e.truncate() - Vec2::new(8.0, 8.0)).length() < 1e-3);
        assert!((e.z - (t.sample_height(8.0, 8.0) + c.distance)).abs() < 1e-3);
    }
}
