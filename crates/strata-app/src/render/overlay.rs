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

/// Tokens for anything drawn on the terrain. The terrain looks the same in
/// both themes, so markers, rings and labels on it use the dark palette
/// (bright strokes, dark halos) everywhere; panels keep the user's theme.
pub const MAP: Tokens = crate::theme::DARK;

/// Dark halo behind markers and strokes so they read on any terrain.
const HALO: Color32 = Color32::from_rgba_premultiplied(8, 11, 15, 200);
/// Fill for doodad markers: a neutral stone, distinct from player colors.
const DOODAD_FILL: Color32 = Color32::from_rgb(0xC9, 0xC0, 0xAE);
/// Hover ring.
const HOVER: Color32 = Color32::from_rgb(0xF2, 0xF5, 0xF8);

/// Markers grow labels once they are this big on screen (radius, points).
pub const LABEL_MIN_RADIUS: f32 = 14.0;

/// How to draw one object marker.
#[derive(Debug, Clone, Copy)]
pub struct MarkerStyle {
    pub color: Color32,
    pub selected: bool,
    pub hovered: bool,
    /// Placement preview: translucent, always labeled.
    pub ghost: bool,
}

/// A label a marker would like to show; placed later so labels never overlap.
#[derive(Debug, Clone)]
pub struct Label {
    /// Top-center of the label.
    pub at: Pos2,
    pub text: String,
    /// Higher wins when labels collide.
    pub priority: f32,
}

fn to_screen(origin: Pos2, p: Vec2) -> Pos2 {
    origin + egui::vec2(p.x, p.y)
}

/// Black or white, whichever reads better on `c`.
fn contrast_on(c: Color32) -> Color32 {
    let l = 0.299 * c.r() as f32 + 0.587 * c.g() as f32 + 0.114 * c.b() as f32;
    if l > 150.0 {
        Color32::from_rgb(0x10, 0x14, 0x1A)
    } else {
        Color32::WHITE
    }
}

/// Screen direction an object faces (unit vector), following perspective.
fn facing(view: &View, o: &PlacedObject, c: Pos2, origin: Pos2) -> egui::Vec2 {
    let dir = Vec3::new(o.rotation.cos(), o.rotation.sin(), 0.0);
    let d = view.project(o.position + dir).map(|t| to_screen(origin, t) - c).unwrap_or(egui::vec2(1.0, 0.0));
    if d.length() > 1e-3 {
        d.normalized()
    } else {
        egui::vec2(1.0, 0.0)
    }
}

/// Text for an object's label: its name, else the known display name, else the id.
pub fn object_label(o: &PlacedObject) -> &str {
    if !o.name.is_empty() {
        return &o.name;
    }
    strata_core::known::unit_name(&o.type_id)
        .or(strata_core::known::doodad_name(&o.type_id))
        .unwrap_or(o.display_name())
}

/// Draw one placed object and return the label it wants, if any.
///
/// Units are discs in the owner's color sized to their footprint (never
/// smaller than 12px across), with a dark outline and a facing notch.
/// Doodads are stone diamonds turned to their rotation; points are flags,
/// start locations are rings.
pub fn draw_object(
    painter: &egui::Painter,
    view: &View,
    origin: Pos2,
    o: &PlacedObject,
    style: MarkerStyle,
    tk: &Tokens,
) -> Option<Label> {
    let s = view.project(o.position)?;
    let c = to_screen(origin, s);
    let r = object_screen_radius(o, view);
    let alpha = if style.ghost { 0.55 } else { 1.0 };
    let halo = Stroke::new(1.5, HALO.gamma_multiply(alpha));
    let accent = tk.accent;
    let ring = |painter: &egui::Painter, rr: f32| {
        if style.selected {
            painter.circle_stroke(c, rr, Stroke::new(4.0, HALO.gamma_multiply(0.6)));
            painter.circle_stroke(c, rr, Stroke::new(2.0, accent));
        } else if style.hovered {
            painter.circle_stroke(c, rr, Stroke::new(3.5, HALO.gamma_multiply(0.5)));
            painter.circle_stroke(c, rr, Stroke::new(1.5, HOVER.gamma_multiply(0.9)));
        }
    };
    let mut label_offset = r + 4.0;
    match o.kind {
        ObjectKind::Unit => {
            let d = facing(view, o, c, origin);
            // Big markers (structures, close zoom) go translucent so the
            // terrain under them stays visible.
            let fill_a = if r < 18.0 { 0.92 } else { 0.55 };
            let fill = style.color.gamma_multiply(fill_a * alpha);
            // Facing notch: a small wedge just outside the rim.
            let n = egui::vec2(-d.y, d.x);
            let nub = (r * 0.45).clamp(3.5, 9.0);
            let wedge = vec![c + d * (r + nub), c + d * (r - 1.0) + n * nub * 0.85, c + d * (r - 1.0) - n * nub * 0.85];
            painter.add(egui::Shape::convex_polygon(wedge, style.color.gamma_multiply(alpha), halo));
            painter.circle_filled(c, r, fill);
            if r >= 18.0 {
                painter.circle_stroke(c, r - 1.5, Stroke::new(1.5, style.color.gamma_multiply(alpha)));
            }
            painter.circle_stroke(c, r, halo);
            // Facing tick inside the disc.
            let tick = if r < 18.0 { contrast_on(style.color) } else { style.color };
            painter.line_segment([c + d * (r * 0.2), c + d * (r - 1.0)], Stroke::new(1.75, tick.gamma_multiply(alpha)));
            ring(painter, r + 3.5);
            label_offset = r + 5.0;
        }
        ObjectKind::Doodad => {
            let d = facing(view, o, c, origin);
            let n = egui::vec2(-d.y, d.x);
            let pts = vec![c + d * r, c + n * r, c - d * r, c - n * r];
            let fill = DOODAD_FILL.gamma_multiply(if r < 18.0 { 0.9 } else { 0.5 } * alpha);
            painter.add(egui::Shape::convex_polygon(pts, fill, halo));
            if style.selected || style.hovered {
                let e = r + 4.0;
                let ring_pts = vec![c + d * e, c + n * e, c - d * e, c - n * e];
                let (w, col) = if style.selected { (2.0, accent) } else { (1.5, HOVER) };
                painter
                    .add(egui::Shape::closed_line(ring_pts.clone(), Stroke::new(w + 2.0, HALO.gamma_multiply(0.55))));
                painter.add(egui::Shape::closed_line(ring_pts, Stroke::new(w, col)));
            }
        }
        ObjectKind::Point => {
            let start = o.type_id == strata_core::known::POINT_START_LOCATION;
            let col = if style.selected {
                accent
            } else if start {
                tk.ok
            } else {
                style.color
            };
            if start {
                let rr = r.max(8.0);
                painter.circle_stroke(c, rr, Stroke::new(4.5, HALO));
                painter.circle_stroke(c, rr, Stroke::new(2.5, col));
                painter.circle_filled(c, 3.0, col);
                painter.circle_stroke(c, 3.0, Stroke::new(1.0, HALO));
                if style.hovered && !style.selected {
                    painter.circle_stroke(c, rr + 3.5, Stroke::new(1.5, HOVER));
                }
                label_offset = rr + 5.0;
            } else {
                let top = c + egui::vec2(0.0, -18.0);
                painter.line_segment([c, top], Stroke::new(3.5, HALO));
                painter.line_segment([c, top], Stroke::new(1.5, col));
                let flag = vec![top, top + egui::vec2(11.0, 4.0), top + egui::vec2(0.0, 8.0)];
                painter.add(egui::Shape::convex_polygon(flag, col, Stroke::new(1.0, HALO)));
                painter.circle_filled(c, 2.5, col);
                painter.circle_stroke(c, 2.5, Stroke::new(1.0, HALO));
                ring(painter, 7.0);
                label_offset = 8.0;
            }
        }
    }
    let point_label = o.kind == ObjectKind::Point && view.points_per_unit(o.position) > 3.0;
    let wants = style.ghost || style.selected || style.hovered || point_label || r > LABEL_MIN_RADIUS;
    if !wants || o.type_id.is_empty() && o.name.is_empty() {
        return None;
    }
    let priority = if style.selected {
        3000.0
    } else if style.hovered || style.ghost {
        2000.0
    } else if o.kind == ObjectKind::Point {
        1000.0 + r
    } else {
        r
    };
    Some(Label { at: c + egui::vec2(0.0, label_offset), text: object_label(o).to_owned(), priority })
}

/// Place labels highest priority first, skipping any that would overlap one
/// already placed or run off `clip`.
pub fn draw_labels(painter: &egui::Painter, mut labels: Vec<Label>, clip: egui::Rect, tk: &Tokens) {
    labels.sort_by(|a, b| b.priority.total_cmp(&a.priority));
    let font = egui::FontId::proportional(12.0);
    let mut taken: Vec<egui::Rect> = Vec::new();
    for l in labels {
        let galley = painter.layout_no_wrap(l.text, font.clone(), tk.text);
        let pad = egui::vec2(5.0, 1.0);
        let size = galley.size() + pad * 2.0;
        let rect = egui::Rect::from_min_size(l.at - egui::vec2(size.x * 0.5, 0.0), size);
        if !clip.contains_rect(rect) || taken.iter().any(|t| t.intersects(rect)) {
            continue;
        }
        taken.push(rect.expand(2.0));
        painter.rect_filled(rect, 4.0, tk.bg_deep.gamma_multiply(0.82));
        painter.galley(rect.min + pad, galley, tk.text);
    }
}

/// A free-standing label (regions, cameras): a quiet pill centered on `at`.
fn text(painter: &egui::Painter, at: Pos2, s: &str, tk: &Tokens) {
    text_aligned(painter, at, egui::Align2::CENTER_CENTER, s, tk);
}

fn text_aligned(painter: &egui::Painter, at: Pos2, align: egui::Align2, s: &str, tk: &Tokens) {
    let galley = painter.layout_no_wrap(s.to_owned(), egui::FontId::proportional(12.0), tk.text);
    let pad = egui::vec2(5.0, 1.0);
    let rect = align.anchor_size(at, galley.size() + pad * 2.0);
    painter.rect_filled(rect, 4.0, tk.bg_deep.gamma_multiply(0.72));
    painter.galley(rect.min + pad, galley, tk.text);
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

/// Selection and hover feedback for an object drawn as a 3D model: a ring
/// on the ground around its footprint (like the game's selection circle)
/// and the label below it. The model itself replaces the marker.
fn draw_model_mark(
    painter: &egui::Painter,
    view: &View,
    origin: Pos2,
    o: &PlacedObject,
    mark: &super::models::ModelMark,
    style: MarkerStyle,
    tk: &Tokens,
) -> Option<Label> {
    let r = mark.radius * 1.08 + 0.15;
    let ring: Vec<Pos2> = (0..40)
        .filter_map(|k| {
            let a = k as f32 / 40.0 * std::f32::consts::TAU;
            view.project(mark.anchor + Vec3::new(a.cos() * r, a.sin() * r, 0.05)).map(|s| to_screen(origin, s))
        })
        .collect();
    if ring.len() == 40 {
        if style.selected {
            painter.add(egui::Shape::closed_line(ring.clone(), Stroke::new(4.5, HALO.gamma_multiply(0.6))));
            painter.add(egui::Shape::closed_line(ring.clone(), Stroke::new(2.0, tk.accent)));
        } else if style.hovered {
            painter.add(egui::Shape::closed_line(ring.clone(), Stroke::new(3.5, HALO.gamma_multiply(0.5))));
            painter.add(egui::Shape::closed_line(ring.clone(), Stroke::new(1.5, HOVER.gamma_multiply(0.9))));
        }
    }
    if !(style.selected || style.hovered) || o.type_id.is_empty() && o.name.is_empty() {
        return None;
    }
    let bottom = ring.iter().map(|p| p.y).fold(f32::MIN, f32::max);
    let c = view.project(mark.anchor).map(|s| to_screen(origin, s))?;
    let priority = if style.selected { 3000.0 } else { 2000.0 };
    Some(Label { at: Pos2::new(c.x, bottom.max(c.y) + 5.0), text: object_label(o).to_owned(), priority })
}

/// Draw every region, camera and object, culled to `rect`. Objects in
/// `models` are drawn as 3D models already and only get rings and labels.
pub fn draw_document(
    painter: &egui::Painter,
    state: &EditorState,
    view: &View,
    rect: egui::Rect,
    hovered: Option<ObjectId>,
    tk: &Tokens,
    models: &std::collections::HashMap<ObjectId, super::models::ModelMark>,
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

    // Doodads first so units and points sit on top; selected and hovered
    // objects last so their rings are never covered.
    let mut on_screen: Vec<&PlacedObject> = doc
        .objects
        .objects
        .iter()
        .filter(|o| view.project(o.position).is_some_and(|s| cull.contains(to_screen(origin, s))))
        .collect();
    let rank = |o: &PlacedObject| {
        let lift = if state.selection.objects.contains(&o.id) || hovered == Some(o.id) { 10 } else { 0 };
        lift + match o.kind {
            ObjectKind::Doodad => 0,
            ObjectKind::Unit => 1,
            ObjectKind::Point => 2,
        }
    };
    on_screen.sort_by_key(|o| rank(o));
    // Only selected / hovered objects keep labels when the view is crowded.
    let crowded = on_screen.len() > 400;
    let mut labels = Vec::new();
    for o in on_screen {
        let selected = state.selection.objects.contains(&o.id);
        let style = MarkerStyle {
            color: match o.kind {
                ObjectKind::Unit => player_color(doc, o.player, tk),
                ObjectKind::Doodad => DOODAD_FILL,
                ObjectKind::Point => tk.text,
            },
            selected,
            hovered: hovered == Some(o.id),
            ghost: false,
        };
        let label = match models.get(&o.id) {
            Some(mark) => draw_model_mark(painter, view, origin, o, mark, style, tk),
            None => draw_object(painter, view, origin, o, style, tk),
        };
        if let Some(l) = label {
            if !crowded || selected || style.hovered {
                labels.push(l);
            }
        }
    }
    draw_labels(painter, labels, rect, tk);
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
