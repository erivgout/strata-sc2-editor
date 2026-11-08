//! The map: 3D terrain, overlays, camera controls and the active tool.
//!
//! Camera: middle-drag (or Alt+drag in most modes) orbits, right-drag pans,
//! the wheel zooms toward the cursor, WASD / arrow keys pan, Q / E turn.
//! F frames the selection, Home frames the map, T toggles top-down.

use egui::{Color32, Key, PointerButton, Sense};
use glam::{Vec2, Vec3};
use strata_core::objects::ObjectId;

use crate::render::camera::{OrbitCamera, View};
use crate::render::gl::{self, FrameParams};
use crate::render::{overlay, pick, TerrainRenderer};
use crate::state::{EditorState, Mode};
use crate::theme::{self, Tokens};
use crate::tools::{ToolInput, ToolState};

pub struct ViewportState {
    pub camera: OrbitCamera,
    pub renderer: TerrainRenderer,
    pub tools: ToolState,
    pub show_grid: bool,
    pub show_contours: bool,
    pub cliff_tint: bool,
    /// Pathing overlay outside Pathing mode (always on in Pathing mode).
    pub show_pathing: bool,
    /// Terrain point under the cursor.
    pub cursor: Option<Vec3>,
    /// Ground footprint of the view (for the minimap), counter-clockwise.
    pub footprint: Option<[Vec2; 4]>,
    pub fps: f32,
    kb_owned: bool,
    alt_orbit: bool,
    framed_for: Option<(u32, u32, Option<String>)>,
    sync_timer: f32,
    height_range: Option<(u64, (f32, f32))>,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            camera: OrbitCamera::default(),
            renderer: TerrainRenderer::default(),
            tools: ToolState::default(),
            show_grid: false,
            show_contours: false,
            cliff_tint: false,
            show_pathing: false,
            cursor: None,
            footprint: None,
            fps: 0.0,
            kb_owned: false,
            alt_orbit: false,
            framed_for: None,
            sync_timer: 0.0,
            height_range: None,
        }
    }
}

/// Modes where Alt is a tool modifier, so Alt+drag does not orbit.
fn alt_is_modifier(mode: Mode) -> bool {
    matches!(mode, Mode::Sculpt | Mode::Regions)
}

fn is_brush_mode(mode: Mode) -> bool {
    matches!(mode, Mode::Sculpt | Mode::Paint | Mode::Cliff | Mode::Pathing)
}

fn rgba(c: Color32) -> [f32; 4] {
    [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, c.a() as f32 / 255.0]
}

pub fn show(ui: &mut egui::Ui, state: &mut EditorState) {
    let ctx = ui.ctx().clone();
    let tk = theme::tokens(&ctx);
    let rect = ui.available_rect_before_wrap();
    if rect.width() < 8.0 || rect.height() < 8.0 {
        return;
    }
    let response = ui.allocate_rect(rect, Sense::click_and_drag());
    let (dt, mods, pointer_pos, any_pressed) =
        ctx.input(|i| (i.stable_dt.clamp(0.0, 0.1), i.modifiers, i.pointer.latest_pos(), i.pointer.any_pressed()));

    // Frame the map the first time we see a document (and after loading one).
    let key = (state.doc.terrain.width, state.doc.terrain.height, state.file_name.clone());
    let new_doc = state.ui.viewport.framed_for.as_ref() != Some(&key);
    if new_doc {
        state.ui.viewport.framed_for = Some(key);
        frame_map(state, true);
    }

    {
        let vs = &mut state.ui.viewport;
        if dt > 0.0 {
            vs.fps = vs.fps * 0.9 + (1.0 / dt) * 0.1;
        }
        let over = pointer_pos.is_some_and(|p| rect.contains(p)) && response.hovered();
        if any_pressed {
            vs.kb_owned = over;
        }
    }
    let hovered = response.hovered();
    let keys = !ctx.egui_wants_keyboard_input()
        && (state.ui.viewport.kb_owned || ctx.pointer_hover_pos().is_some_and(|p| rect.contains(p)));

    camera_input(&ctx, state, &response, rect, keys, hovered, dt);
    if keys {
        view_keys(&ctx, state);
    }
    if state.ui.viewport.camera.update(dt) {
        ctx.request_repaint();
    }

    let view = state.ui.viewport.camera.view(Vec2::new(rect.width(), rect.height()));

    // Cursor on terrain.
    let busy = state.ui.viewport.tools.is_busy();
    let local =
        pointer_pos.filter(|p| rect.contains(*p) || busy).map(|p| Vec2::new(p.x - rect.min.x, p.y - rect.min.y));
    let range = height_range(state);
    let hover = local.and_then(|p| {
        let (o, d) = view.ray(p);
        pick::ray_terrain(&state.doc.terrain, o, d, range)
    });
    state.ui.viewport.cursor = hover;

    // Tools.
    let alt_orbit_now = state.ui.viewport.alt_orbit;
    let (pressed, down, released) =
        ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.primary_released()));
    let input = ToolInput {
        view,
        rect,
        pointer: local,
        hover,
        pressed: pressed && hovered && !alt_orbit_now,
        down: down && !alt_orbit_now,
        released,
        mods,
        dt,
        keys,
    };
    let mut tools = std::mem::take(&mut state.ui.viewport.tools);
    tools.handle(&ctx, state, &input);
    let stroke_rect = tools.frame_dirty.take();
    if tools.is_busy() {
        ctx.request_repaint();
    }
    state.ui.viewport.tools = tools;

    // Keep the GPU mesh in step with the document.
    {
        let vs = &mut state.ui.viewport;
        vs.sync_timer += dt;
        let force = new_doc || (vs.sync_timer > 0.5 && !vs.tools.stroking());
        if force {
            vs.sync_timer = 0.0;
        }
        vs.renderer.prepare(&state.doc.terrain, state.history.revision(), stroke_rect, force);
    }

    // Terrain.
    let painter = ui.painter_at(rect);
    let vs = &state.ui.viewport;
    let t = &state.doc.terrain;
    let p = &state.doc.info.playable;
    let params = FrameParams {
        view_proj: view.view_proj,
        eye: view.eye,
        map_size: [t.width as f32, t.height as f32],
        playable: [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32],
        water: t.water_height,
        show_grid: vs.show_grid,
        show_contours: vs.show_contours,
        cliff_tint: vs.cliff_tint || state.mode == Mode::Cliff,
        show_pathing: vs.show_pathing || state.mode == Mode::Pathing,
        clear: rgba(tk.bg_deep),
        bounds_color: rgba(tk.text_dim),
        unwalkable: rgba(tk.error),
        unbuildable: rgba(tk.warn),
        nofly: rgba(tk.text),
        water_color: [0.16, 0.36, 0.52, 1.0],
    };
    painter.add(gl::callback(vs.renderer.shared.clone(), rect, params));
    if let Some(err) = vs.renderer.error() {
        painter.rect_filled(rect, 0.0, tk.bg_deep);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, err, egui::FontId::proportional(14.0), tk.error);
    }

    // Overlays.
    let hovered_obj: Option<ObjectId> = if state.mode == Mode::Select && !busy {
        local.and_then(|p| crate::tools::select::pick_object(state, &view, p))
    } else {
        None
    };
    overlay::draw_document(&painter, state, &view, rect, hovered_obj, &tk);
    state.ui.viewport.tools.paint(&painter, state, &input);

    state.ui.viewport.footprint = Some(footprint(&view, state.ui.viewport.camera.current.target.z));

    toolbar(&ctx, ui.id(), rect, state, &tk);
    if is_brush_mode(state.mode) {
        brush_hud(&ctx, ui.id(), rect, state);
    } else {
        mode_hint(&painter, rect, state, &tk);
    }
}

fn height_range(state: &mut EditorState) -> (f32, f32) {
    let rev = state.history.revision();
    let stroking = state.ui.viewport.tools.stroking();
    match state.ui.viewport.height_range {
        Some((r, range)) if r == rev && !stroking => range,
        _ => {
            let range = state.doc.terrain.min_max_height();
            state.ui.viewport.height_range = Some((rev, range));
            range
        }
    }
}

fn camera_input(
    ctx: &egui::Context,
    state: &mut EditorState,
    response: &egui::Response,
    rect: egui::Rect,
    keys: bool,
    hovered: bool,
    dt: f32,
) {
    let mode = state.mode;
    let size = Vec2::new(rect.width(), rect.height());
    let (delta, mods, primary_pressed, primary_down, latest) = ctx.input(|i| {
        (i.pointer.delta(), i.modifiers, i.pointer.primary_pressed(), i.pointer.primary_down(), i.pointer.latest_pos())
    });
    let vs = &mut state.ui.viewport;

    // Alt + left drag orbits where Alt is not a tool modifier.
    if primary_pressed && hovered && mods.alt && !alt_is_modifier(mode) {
        vs.alt_orbit = true;
    }
    if !primary_down {
        vs.alt_orbit = false;
    }
    let d = Vec2::new(delta.x, delta.y);
    if response.dragged_by(PointerButton::Middle) || (vs.alt_orbit && primary_down) {
        vs.camera.orbit(d);
        ctx.request_repaint();
    }

    // Right drag: grab the ground and pull it.
    if response.dragged_by(PointerButton::Secondary) {
        if let Some(p) = latest {
            let view = vs.camera.view(size);
            let cur = Vec2::new(p.x - rect.min.x, p.y - rect.min.y);
            let z = vs.camera.current.target.z;
            if let (Some(a), Some(b)) = (view.ray_plane(cur - d, z), view.ray_plane(cur, z)) {
                let mv = (a - b).truncate().extend(0.0);
                vs.camera.goal.target += mv;
                vs.camera.current.target += mv;
                ctx.request_repaint();
            }
        }
    }

    // Wheel: zoom toward the cursor; Shift+wheel rotates the placement ghost.
    if hovered {
        let (notches, pinch) = ctx.input(|i| {
            let mut n = 0.0f32;
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e {
                    if modifiers.command {
                        continue; // reported through zoom_delta
                    }
                    let v = if delta.y != 0.0 { delta.y } else { delta.x };
                    n += match unit {
                        egui::MouseWheelUnit::Point => v / 50.0,
                        egui::MouseWheelUnit::Line => v,
                        egui::MouseWheelUnit::Page => v * 10.0,
                    };
                }
            }
            (n, i.zoom_delta())
        });
        if notches != 0.0 && mods.shift && matches!(mode, Mode::Units | Mode::Doodads) {
            let s = &mut vs.tools.settings;
            s.ghost_rotation = (s.ghost_rotation + notches * 15f32.to_radians()).rem_euclid(std::f32::consts::TAU);
        } else if notches != 0.0 || pinch != 1.0 {
            let factor = 0.87f32.powf(notches) / pinch.max(0.01);
            let view = vs.camera.view(size);
            let anchor = latest.and_then(|p| {
                let local = Vec2::new(p.x - rect.min.x, p.y - rect.min.y);
                let (o, dir) = view.ray(local);
                pick::ray_terrain(&state.doc.terrain, o, dir, state.doc.terrain.min_max_height())
                    .or_else(|| view.ray_plane(local, vs.camera.current.target.z))
            });
            vs.camera.zoom_at(factor, anchor);
            ctx.request_repaint();
        }
    }

    // Keyboard pan.
    if keys && !mods.command && !mods.alt {
        let (fwd, right) = ctx.input(|i| {
            let k = |a: Key, b: Key| (i.key_down(a) || i.key_down(b)) as i32 as f32;
            (
                k(Key::W, Key::ArrowUp) - k(Key::S, Key::ArrowDown),
                k(Key::D, Key::ArrowRight) - k(Key::A, Key::ArrowLeft),
            )
        });
        if fwd != 0.0 || right != 0.0 {
            let speed = vs.camera.goal.distance * 1.1 * if mods.shift { 2.5 } else { 1.0 };
            vs.camera.pan_relative(Vec2::new(right, fwd).normalize_or_zero() * speed * dt.max(1.0 / 120.0));
            ctx.request_repaint();
        }
    }

    // Keep the pivot on the map.
    let w = state.doc.world_size();
    let g = &mut vs.camera.goal.target;
    g.x = g.x.clamp(-16.0, w.x + 16.0);
    g.y = g.y.clamp(-16.0, w.y + 16.0);
}

/// Single-key view toggles and brush hotkeys.
fn view_keys(ctx: &egui::Context, state: &mut EditorState) {
    let pressed = |k: Key| ctx.input(|i| i.key_pressed(k) && !i.modifiers.command && !i.modifiers.alt);
    let shift = ctx.input(|i| i.modifiers.shift);
    let vs = &mut state.ui.viewport;
    if pressed(Key::Q) {
        vs.camera.goal.yaw += std::f32::consts::FRAC_PI_4;
    }
    if pressed(Key::E) {
        vs.camera.goal.yaw -= std::f32::consts::FRAC_PI_4;
    }
    if pressed(Key::T) {
        let on = !vs.camera.ortho;
        vs.camera.set_ortho(on);
    }
    if pressed(Key::G) {
        vs.show_grid = !vs.show_grid;
    }
    if pressed(Key::C) {
        vs.show_contours = !vs.show_contours;
    }
    if pressed(Key::L) {
        vs.cliff_tint = !vs.cliff_tint;
    }
    if pressed(Key::P) {
        vs.show_pathing = !vs.show_pathing;
    }
    if pressed(Key::N) {
        vs.tools.settings.snap = !vs.tools.settings.snap;
    }
    if pressed(Key::Home) {
        frame_map(state, false);
    }
    if pressed(Key::F) {
        frame_selection(state);
    }
    if is_brush_mode(state.mode) {
        let smaller = pressed(Key::OpenBracket) || pressed(Key::OpenCurlyBracket);
        let bigger = pressed(Key::CloseBracket) || pressed(Key::CloseCurlyBracket);
        let curly = ctx.input(|i| i.key_pressed(Key::OpenCurlyBracket) || i.key_pressed(Key::CloseCurlyBracket));
        let b = &mut state.brush;
        if smaller || bigger {
            if shift || curly {
                let step = if bigger { 0.05 } else { -0.05 };
                b.strength = (b.strength + step).clamp(0.01, 1.0);
            } else {
                let f = if bigger { 1.2 } else { 1.0 / 1.2 };
                b.radius = (b.radius * f).clamp(0.5, 64.0);
            }
        }
    }
}

/// Frame the whole map. `instant` skips the easing (on load).
pub fn frame_map(state: &mut EditorState, instant: bool) {
    let w = state.doc.world_size();
    let (lo, hi) = state.doc.terrain.min_max_height();
    let cam = &mut state.ui.viewport.camera;
    cam.goal.yaw = crate::render::camera::DEFAULT_YAW;
    cam.frame(Vec3::new(w.x * 0.5, w.y * 0.5, (lo + hi) * 0.5), w.x.max(w.y) * 0.5);
    if instant {
        cam.snap();
    }
}

/// Frame the selected objects, region or camera.
pub fn frame_selection(state: &mut EditorState) {
    let mut lo = Vec2::splat(f32::MAX);
    let mut hi = Vec2::splat(f32::MIN);
    let mut any = false;
    let mut add = |p: Vec2| {
        lo = lo.min(p);
        hi = hi.max(p);
        any = true;
    };
    for id in &state.selection.objects {
        if let Some(o) = state.doc.objects.get(*id) {
            add(o.position.truncate());
        }
    }
    if let Some(r) = state.selection.region.and_then(|id| state.doc.regions.iter().find(|r| r.id == id)) {
        for s in &r.shapes {
            let (a, b) = s.bounds();
            add(a);
            add(b);
        }
    }
    if let Some(c) = state.selection.camera.and_then(|id| state.doc.cameras.iter().find(|c| c.id == id)) {
        add(c.target);
    }
    if !any {
        state.toast(crate::state::ToastKind::Info, "Select something to frame it. Home frames the whole map.");
        return;
    }
    let c = (lo + hi) * 0.5;
    let radius = ((hi - lo).length() * 0.5).max(6.0);
    let z = state.doc.terrain.sample_height(c.x, c.y);
    state.ui.viewport.camera.frame(Vec3::new(c.x, c.y, z), radius);
}

/// Ground quad seen by the camera (viewport corners cast onto `z`).
fn footprint(view: &View, z: f32) -> [Vec2; 4] {
    let s = view.size;
    let far = (view.eye.z - z).abs() * 6.0 + 50.0;
    [Vec2::new(0.0, s.y), Vec2::new(s.x, s.y), Vec2::new(s.x, 0.0), Vec2::ZERO].map(|c| match view.ray_plane(c, z) {
        Some(p) if (p - view.eye).length() < far => p.truncate(),
        _ => {
            let (o, d) = view.ray(c);
            (o.truncate() + d.truncate().normalize_or_zero() * far)
                .clamp(o.truncate() - Vec2::splat(far), o.truncate() + Vec2::splat(far))
        }
    })
}

fn toolbar(ctx: &egui::Context, id: egui::Id, rect: egui::Rect, state: &mut EditorState, tk: &Tokens) {
    egui::Area::new(id.with("viewport-toolbar"))
        .fixed_pos(rect.min + egui::vec2(8.0, 8.0))
        .constrain_to(rect)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(tk.bg_panel.gamma_multiply(0.92)).inner_margin(4.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    let vs = &mut state.ui.viewport;
                    let toggle = |ui: &mut egui::Ui, on: &mut bool, label: &str, tip: &str| {
                        if ui.add(egui::Button::selectable(*on, label)).on_hover_text(tip).clicked() {
                            *on = !*on;
                        }
                    };
                    toggle(ui, &mut vs.show_grid, "Grid", "Cell grid (G)");
                    toggle(ui, &mut vs.show_contours, "Contours", "Height contour lines (C)");
                    toggle(ui, &mut vs.cliff_tint, "Cliffs", "Tint terrain by cliff level (L)");
                    toggle(ui, &mut vs.show_pathing, "Pathing", "Show unwalkable and unbuildable cells (P)");
                    ui.separator();
                    let mut ortho = vs.camera.ortho;
                    if ui
                        .add(egui::Button::selectable(ortho, "Top-down"))
                        .on_hover_text("Orthographic top-down view (T)")
                        .clicked()
                    {
                        ortho = !ortho;
                        vs.camera.set_ortho(ortho);
                    }
                    toggle(
                        ui,
                        &mut vs.tools.settings.snap,
                        "Snap",
                        "Snap moves and placement to the half-cell grid (N)",
                    );
                    ui.separator();
                    if ui
                        .button("Frame map")
                        .on_hover_text("Frame the whole map (Home). F frames the selection.")
                        .clicked()
                    {
                        frame_map(state, false);
                    }
                });
            });
        });
}

fn brush_hud(ctx: &egui::Context, id: egui::Id, rect: egui::Rect, state: &mut EditorState) {
    let tk = theme::tokens(ctx);
    egui::Area::new(id.with("brush-hud"))
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(rect.center_bottom() - egui::vec2(0.0, 12.0))
        .constrain_to(rect)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).fill(tk.bg_panel.gamma_multiply(0.95)).inner_margin(6.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    crate::panels::inspector::sub_tool_picker(ui, state, true);
                    ui.separator();
                    crate::panels::inspector::brush_controls(ui, state, true);
                });
            });
        });
}

fn mode_hint(painter: &egui::Painter, rect: egui::Rect, state: &EditorState, tk: &Tokens) {
    let text = match state.mode {
        Mode::Select => {
            "Click to select · Shift adds · drag a box · drag to move · R rotates · Delete removes · Ctrl+D duplicates"
        }
        Mode::Units => "Click to place · Shift keeps placing · Shift+wheel or R rotates · Esc returns to Select",
        Mode::Doodads => "Click to place a doodad · Shift keeps placing · R rotates · Esc returns to Select",
        Mode::Points => "Click to place a point",
        Mode::Regions => "Drag to draw a rectangle · Alt+drag draws a circle",
        Mode::Cameras => "Click to add a camera looking from the current angle",
        _ => return,
    };
    let at = rect.center_bottom() - egui::vec2(0.0, 14.0);
    let font = egui::FontId::proportional(12.0);
    painter.text(at + egui::vec2(1.0, 1.0), egui::Align2::CENTER_BOTTOM, text, font.clone(), tk.bg_deep);
    painter.text(at, egui::Align2::CENTER_BOTTOM, text, font, tk.text);
}

/// Center the camera on a world position (used by Problems, Minimap, MCP).
pub fn frame_world(state: &mut EditorState, p: glam::Vec2) {
    let z = state.doc.terrain.sample_height(p.x, p.y);
    state.ui.viewport.camera.goal.target = Vec3::new(p.x, p.y, z);
}

/// Move the camera to a world position immediately (minimap drag).
pub fn jump_to(state: &mut EditorState, p: glam::Vec2) {
    frame_world(state, p);
    let cam = &mut state.ui.viewport.camera;
    cam.current.target = cam.goal.target;
}

/// Right side of the status bar: cursor position, height, brush.
pub fn status_text(state: &EditorState) -> String {
    let vs = &state.ui.viewport;
    let mut parts = Vec::new();
    match vs.cursor {
        Some(c) => {
            let t = &state.doc.terrain;
            let vx = (c.x.round().max(0.0) as u32).min(t.width);
            let vy = (c.y.round().max(0.0) as u32).min(t.height);
            let cliff = t.cliff_levels[t.vertex_index(vx, vy)];
            parts.push(format!("x {:.1}  y {:.1}", c.x, c.y));
            parts.push(format!("height {:.2}", c.z));
            parts.push(format!("cliff {cliff}"));
        }
        None => parts.push("cursor off map".into()),
    }
    if is_brush_mode(state.mode) {
        parts.push(format!("brush {:.1}", state.brush.radius));
    }
    if vs.fps > 0.0 {
        parts.push(format!("{:.0} fps", vs.fps.min(999.0)));
    }
    parts.join("  •  ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::MapDocument;

    #[test]
    fn frame_world_targets_terrain_height() {
        let mut s = EditorState::new(MapDocument::new("t", 64, 64));
        frame_world(&mut s, Vec2::new(10.0, 20.0));
        let g = s.ui.viewport.camera.goal.target;
        assert_eq!((g.x, g.y), (10.0, 20.0));
        assert_eq!(g.z, s.doc.terrain.sample_height(10.0, 20.0));
    }

    #[test]
    fn status_text_reports_cursor() {
        let mut s = EditorState::new(MapDocument::new("t", 64, 64));
        assert!(status_text(&s).contains("off map"));
        s.ui.viewport.cursor = Some(Vec3::new(3.0, 4.0, 12.0));
        let t = status_text(&s);
        assert!(t.contains("x 3.0") && t.contains("height 12.00") && t.contains("cliff 2"), "{t}");
    }

    #[test]
    fn footprint_surrounds_target() {
        let cam = OrbitCamera::default();
        let v = cam.view(Vec2::new(800.0, 600.0));
        let f = footprint(&v, cam.current.target.z);
        let c = cam.current.target.truncate();
        let lo = f.iter().fold(Vec2::splat(f32::MAX), |a, b| a.min(*b));
        let hi = f.iter().fold(Vec2::splat(f32::MIN), |a, b| a.max(*b));
        assert!(lo.x < c.x && lo.y < c.y && hi.x > c.x && hi.y > c.y);
    }
}
