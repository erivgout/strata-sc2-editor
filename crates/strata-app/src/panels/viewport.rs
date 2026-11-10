//! The map: 3D terrain, overlays, camera controls and the active tool.
//!
//! Camera: middle-drag (or Alt+drag in most modes) orbits, right-drag pans,
//! the wheel zooms toward the cursor, WASD / arrow keys pan, Q / E turn.
//! F frames the selection, Home frames the map, T toggles top-down.

use egui::{Color32, Key, PointerButton, Sense};
use glam::{Vec2, Vec3};
use strata_core::objects::ObjectId;

use crate::render::camera::{OrbitCamera, Pose, View, MAP_PITCH};
use crate::render::gl::{self, FrameParams};
use crate::render::models::ModelRenderer;
use crate::render::{overlay, pick, TerrainRenderer};
use crate::state::{EditorState, Mode};
use crate::theme::{self, Tokens};
use crate::tools::{ToolInput, ToolState};

pub struct ViewportState {
    pub camera: OrbitCamera,
    pub renderer: TerrainRenderer,
    /// Units and doodads as 3D models.
    pub models: ModelRenderer,
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
    /// Viewport size in points (last frame).
    pub size: Vec2,
    /// Camera goal right after an automatic "frame the map", so the view can
    /// re-fit when the panel resizes until the user moves the camera.
    auto_framed: Option<Pose>,
    hud: HudFit,
    kb_owned: bool,
    alt_orbit: bool,
    framed_for: Option<(u32, u32, Option<String>)>,
    sync_timer: f32,
    height_range: Option<(u64, (f32, f32))>,
    /// A map image export in progress (see [`crate::export_image`]).
    pub export: Option<crate::export_image::ExportJob>,
}

impl Default for ViewportState {
    fn default() -> Self {
        Self {
            camera: OrbitCamera::default(),
            renderer: TerrainRenderer::default(),
            models: ModelRenderer::default(),
            tools: ToolState::default(),
            show_grid: true,
            show_contours: false,
            cliff_tint: false,
            show_pathing: false,
            cursor: None,
            footprint: None,
            size: Vec2::ZERO,
            auto_framed: None,
            hud: HudFit::default(),
            kb_owned: false,
            alt_orbit: false,
            framed_for: None,
            sync_timer: 0.0,
            height_range: None,
            export: None,
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
    let size = Vec2::new(rect.width(), rect.height());
    let resized = state.ui.viewport.size != size;
    state.ui.viewport.size = size;
    let key = (state.doc.terrain.width, state.doc.terrain.height, state.file_name.clone());
    let new_doc = state.ui.viewport.framed_for.as_ref() != Some(&key);
    if new_doc {
        state.ui.viewport.framed_for = Some(key);
        frame_map(state, true);
        debug_view(state);
    } else if resized {
        // Keep an untouched auto-framed view fitted while the panel resizes.
        let vs = &state.ui.viewport;
        if vs.auto_framed.is_some_and(|p| p == vs.camera.goal) {
            frame_map(state, true);
        }
    }

    {
        let vs = &mut state.ui.viewport;
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
    let layers = state.ui.viewport.renderer.prepare_textures(&state.assets, &state.doc.terrain);
    let mut models = std::mem::take(&mut state.ui.viewport.models);
    if models.generation != Some(state.doc_generation) {
        models.reset(&state.ui.viewport.renderer.shared);
        models.generation = Some(state.doc_generation);
    }
    let mut export = state.ui.viewport.export.take();
    if export.as_ref().is_some_and(|j| j.generation != state.doc_generation) {
        // Another map was opened: this export no longer applies.
        crate::export_image::ExportJob::cancel(&state.ui.viewport.renderer.shared);
        if let Some(j) = export.as_mut() {
            j.result = Some(Err("Another map was opened, so the map image export stopped.".into()));
        }
    }
    let capture = export.as_mut().filter(|j| j.result.is_none()).map(|j| &mut j.capture);
    models.prepare(&ctx, state, &view, &state.ui.viewport.renderer.shared, capture);
    state.ui.viewport.models = models;

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
        height_range: [range.0, range.1],
        water: t.water_height,
        show_grid: vs.show_grid,
        show_contours: vs.show_contours,
        cliff_tint: vs.cliff_tint || state.mode == Mode::Cliff,
        show_pathing: vs.show_pathing || state.mode == Mode::Pathing,
        clear: rgba(tk.bg_deep),
        bounds_color: rgba(overlay::MAP.text_dim),
        unwalkable: rgba(overlay::MAP.error),
        unbuildable: rgba(overlay::MAP.warn),
        nofly: rgba(overlay::MAP.text),
        water_color: [0.16, 0.36, 0.52, 1.0],
        layers,
        tex_scale: crate::render::textures::REPEATS_PER_CELL,
        fog_start: t.width.max(t.height) as f32 * 1.2,
    };
    if let Some(job) = export.as_mut() {
        let p = crate::export_image::frame_params(&params, job.rect, range);
        job.drive(ctx.input(|i| i.time), &vs.renderer.shared, p);
        ctx.request_repaint();
    }
    painter.add(gl::callback(vs.renderer.shared.clone(), rect, params));
    if let Some(err) = vs.renderer.error() {
        painter.rect_filled(rect, 0.0, tk.bg_deep);
        painter.text(rect.center(), egui::Align2::CENTER_CENTER, err, egui::FontId::proportional(14.0), tk.error);
    }
    state.ui.viewport.export = export;

    // Overlays.
    let hovered_obj: Option<ObjectId> = if state.mode == Mode::Select && !busy {
        local.and_then(|p| crate::tools::select::pick_object(state, &view, p))
    } else {
        None
    };
    overlay::draw_document(&painter, state, &view, rect, hovered_obj, &overlay::MAP, &state.ui.viewport.models.drawn);
    state.ui.viewport.tools.paint(&painter, state, &input);
    if let Some(o) = hovered_obj.and_then(|id| state.doc.objects.get(id)) {
        response.clone().on_hover_ui_at_pointer(|ui| object_tooltip(ui, state, o, &tk));
    }

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

/// Room kept free around the framed map: left, top (the view toolbar),
/// right, bottom.
const FRAME_PAD: [f32; 4] = [16.0, 56.0, 16.0, 52.0];

/// Frame the whole map so it fills the viewport. `instant` skips the easing
/// (on load and while the panel resizes).
pub fn frame_map(state: &mut EditorState, instant: bool) {
    let w = state.doc.world_size();
    let (lo, hi) = state.doc.terrain.min_max_height();
    let vs = &mut state.ui.viewport;
    let cam = &mut vs.camera;
    let yaw = crate::render::camera::DEFAULT_YAW;
    if vs.size.x >= 64.0 && vs.size.y >= 64.0 {
        // The map's outline at its real heights: interior hills may poke
        // above the far edge, but the whole map area stays in view.
        let t = &state.doc.terrain;
        let corners = [Vec2::ZERO, Vec2::new(w.x, 0.0), w, Vec2::new(0.0, w.y)];
        let pts: Vec<Vec3> = (0..4)
            .flat_map(|e| {
                let (a, b) = (corners[e], corners[(e + 1) % 4]);
                (0..16).map(move |i| a.lerp(b, i as f32 / 16.0))
            })
            .map(|p| p.extend(t.sample_height(p.x, p.y)))
            .collect();
        cam.fit(&pts, vs.size, FRAME_PAD, yaw, MAP_PITCH, (lo + hi) * 0.5);
    } else {
        cam.goal.yaw = yaw;
        cam.frame(Vec3::new(w.x * 0.5, w.y * 0.5, (lo + hi) * 0.5), w.x.max(w.y) * 0.5);
    }
    if instant {
        cam.snap();
    }
    vs.auto_framed = Some(cam.goal);
}

/// `STRATA_VIEW="x,y,distance,pitch°,yaw°"` (native): start every map from
/// this camera, for checking rendering against the game. Missing values
/// keep the framed view's.
fn debug_view(state: &mut EditorState) {
    #[cfg(not(target_arch = "wasm32"))]
    if let Ok(v) = std::env::var("STRATA_VIEW") {
        let n: Vec<f32> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        let t = &state.doc.terrain;
        let cam = &mut state.ui.viewport.camera;
        if let [x, y, ..] = n[..] {
            cam.goal.target = Vec3::new(x, y, t.sample_height(x, y));
        }
        if let Some(d) = n.get(2) {
            cam.goal.distance = *d;
        }
        if let Some(p) = n.get(3) {
            cam.goal.pitch = p.to_radians();
        }
        if let Some(y) = n.get(4) {
            cam.goal.yaw = y.to_radians();
        }
        cam.snap();
        state.ui.viewport.auto_framed = None;
    }
    #[cfg(target_arch = "wasm32")]
    let _ = state;
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

/// Raised floating panel used by the view toolbar and the brush HUD:
/// 6px radius, hairline stroke and a soft shadow.
fn float_frame(ctx: &egui::Context, tk: &Tokens) -> egui::Frame {
    let dark = ctx.global_style().visuals.dark_mode;
    egui::Frame::new()
        .fill(tk.bg_raised.gamma_multiply(0.96))
        .stroke(egui::Stroke::new(1.0, tk.stroke))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(4))
        .shadow(egui::Shadow {
            offset: [0, 2],
            blur: 10,
            spread: 0,
            color: Color32::from_black_alpha(if dark { 90 } else { 36 }),
        })
}

fn toolbar(ctx: &egui::Context, id: egui::Id, rect: egui::Rect, state: &mut EditorState, tk: &Tokens) {
    egui::Area::new(id.with("viewport-toolbar"))
        .fixed_pos(rect.min + egui::vec2(8.0, 8.0))
        .constrain_to(rect.shrink(4.0))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            float_frame(ctx, tk).show(ui, |ui| {
                // Wrap onto a second row rather than run off a narrow viewport.
                ui.set_max_width((rect.width() - 24.0).max(120.0));
                ui.horizontal_wrapped(|ui| {
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
                    textures_note(ui, state, tk);
                });
            });
        });
}

/// Says why the terrain shows placeholder colors instead of the game's
/// textures, when it does.
fn textures_note(ui: &mut egui::Ui, state: &EditorState, tk: &Tokens) {
    let assets = &state.assets;
    let textures = &state.ui.viewport.renderer.textures;
    if textures.any_textured() || state.doc.terrain.layers.is_empty() {
        return;
    }
    let (text, tip) = if let Some(problem) = assets.problem() {
        ("Placeholder colors", problem)
    } else if textures.pending() {
        ("Loading textures…", "Reading the terrain textures from your StarCraft II install.".to_owned())
    } else if assets.available() {
        return;
    } else {
        (
            "Placeholder colors",
            "Terrain shows placeholder colors. Map → Load game data… reads the real textures from your StarCraft II install."
                .to_owned(),
        )
    };
    ui.separator();
    ui.label(egui::RichText::new(text).small().color(tk.text_dim)).on_hover_text(tip);
}

/// Brush HUD layouts, from roomiest to most compact.
const HUD_LEVELS: usize = 4;

/// Remembers how wide each HUD layout was for the current mode, so the HUD
/// can pick the roomiest one that fits without flickering between layouts.
#[derive(Default)]
struct HudFit {
    key: Option<(Mode, bool, u32)>,
    widths: [Option<f32>; HUD_LEVELS],
}

impl HudFit {
    /// Roomiest level not known to overflow `avail`.
    fn level(&self, avail: f32) -> usize {
        (0..HUD_LEVELS).find(|&l| self.widths[l].is_none_or(|w| w <= avail)).unwrap_or(HUD_LEVELS - 1)
    }
}

fn brush_hud(ctx: &egui::Context, id: egui::Id, rect: egui::Rect, state: &mut EditorState) {
    let tk = theme::tokens(ctx);
    let margin = 12.0;
    let avail = rect.width() - 2.0 * margin;
    let key = (
        state.mode,
        state.ui.viewport.tools.settings.cliff_op == crate::tools::CliffOp::Set,
        (ctx.pixels_per_point() * 100.0) as u32,
    );
    let fit = &mut state.ui.viewport.hud;
    if fit.key != Some(key) {
        *fit = HudFit { key: Some(key), ..HudFit::default() };
    }
    let level = fit.level(avail);
    let resp = egui::Area::new(id.with("brush-hud"))
        .pivot(egui::Align2::CENTER_BOTTOM)
        .fixed_pos(rect.center_bottom() - egui::vec2(0.0, margin))
        .constrain_to(rect.shrink(4.0))
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            float_frame(ctx, &tk).inner_margin(egui::Margin::symmetric(6, 4)).show(ui, |ui| {
                ui.horizontal(|ui| hud_contents(ui, state, level, &tk));
            });
        });
    let w = resp.response.rect.width();
    let fit = &mut state.ui.viewport.hud;
    fit.widths[level] = Some(w);
    if w > avail && level + 1 < HUD_LEVELS {
        ctx.request_repaint();
    }
}

/// Sub-tool, size and strength. `level` trades labels and slider length for
/// width: 0 labeled, 1 unlabeled, 2 short sliders, 3 numbers only.
fn hud_contents(ui: &mut egui::Ui, state: &mut EditorState, level: usize, tk: &Tokens) {
    ui.spacing_mut().item_spacing.x = 6.0;
    crate::panels::inspector::sub_tool_picker(ui, state, true);
    let uses_strength = matches!(state.mode, Mode::Sculpt | Mode::Paint);
    let b = &mut state.brush;
    ui.spacing_mut().slider_width = if level >= 2 { 56.0 } else { 96.0 };
    let label = |ui: &mut egui::Ui, text: &str| {
        if level == 0 {
            ui.label(egui::RichText::new(text).color(tk.text_dim));
        }
    };
    let size_tip = "Brush size in cells ( [ and ] )";
    let strength_tip = "Brush strength (Shift+[ and Shift+])";
    ui.separator();
    label(ui, "Size");
    if level >= 3 {
        ui.add(egui::DragValue::new(&mut b.radius).range(0.5..=64.0).speed(0.1).max_decimals(1).prefix("Size "))
            .on_hover_text(size_tip);
    } else {
        ui.add(egui::Slider::new(&mut b.radius, 0.5..=64.0).logarithmic(true).max_decimals(1)).on_hover_text(size_tip);
    }
    if uses_strength {
        label(ui, "Strength");
        if level >= 3 {
            ui.add(
                egui::DragValue::new(&mut b.strength).range(0.01..=1.0).speed(0.01).max_decimals(2).prefix("Strength "),
            )
            .on_hover_text(strength_tip);
        } else {
            ui.add(egui::Slider::new(&mut b.strength, 0.01..=1.0).max_decimals(2)).on_hover_text(strength_tip);
        }
    }
}

/// Mode hint along the bottom of the viewport, on a quiet pill so it reads
/// over any terrain in either theme. Falls back to a short form when narrow.
fn mode_hint(painter: &egui::Painter, rect: egui::Rect, state: &EditorState, tk: &Tokens) {
    let (full, short) = match state.mode {
        Mode::Select => (
            "Click to select · Shift adds · drag a box · drag to move · R rotates · Delete removes · Ctrl+D duplicates",
            "Click to select · drag a box · drag to move",
        ),
        Mode::Units => (
            "Click to place · Shift keeps placing · Shift+wheel or R rotates · Esc returns to Select",
            "Click to place · Shift keeps placing",
        ),
        Mode::Doodads => (
            "Click to place a doodad · Shift keeps placing · R rotates · Esc returns to Select",
            "Click to place a doodad",
        ),
        Mode::Points => ("Click to place a point", "Click to place a point"),
        Mode::Regions => ("Drag to draw a rectangle · Alt+drag draws a circle", "Drag to draw a region"),
        Mode::Cameras => ("Click to add a camera looking from the current angle", "Click to add a camera"),
        _ => return,
    };
    let font = egui::FontId::proportional(12.0);
    let max_w = rect.width() - 32.0;
    let mut galley = painter.layout_no_wrap(full.to_owned(), font.clone(), tk.text);
    if galley.size().x > max_w {
        galley = painter.layout_no_wrap(short.to_owned(), font, tk.text);
    }
    if galley.size().x > max_w {
        return;
    }
    let pad = egui::vec2(8.0, 3.0);
    let size = galley.size() + pad * 2.0;
    let pill = egui::Rect::from_min_size(rect.center_bottom() - egui::vec2(size.x * 0.5, size.y + 10.0), size);
    painter.rect_filled(pill, 6.0, tk.bg_raised.gamma_multiply(0.82));
    painter.galley(pill.min + pad, galley, tk.text);
}

/// Hover card for a placed object: what it is, who owns it, where it is.
fn object_tooltip(ui: &mut egui::Ui, state: &EditorState, o: &strata_core::objects::PlacedObject, tk: &Tokens) {
    use strata_core::known;
    use strata_core::objects::ObjectKind;
    let type_name = match o.kind {
        ObjectKind::Unit => known::unit_name(&o.type_id),
        ObjectKind::Doodad => known::doodad_name(&o.type_id),
        ObjectKind::Point => known::POINT_TYPES.iter().find(|k| k.id == o.type_id).map(|k| k.name),
    };
    let title = if !o.name.is_empty() { o.name.as_str() } else { type_name.unwrap_or(o.type_id.as_str()) };
    ui.label(egui::RichText::new(title).family(theme::semibold()).color(tk.text));
    let kind = match o.kind {
        ObjectKind::Unit => "Unit",
        ObjectKind::Doodad => "Doodad",
        ObjectKind::Point => "Point",
    };
    let ty = match type_name {
        Some(n) if n != title => format!("{kind} · {n} ({})", o.type_id),
        _ if o.type_id != title => format!("{kind} · {}", o.type_id),
        _ => kind.to_string(),
    };
    ui.label(egui::RichText::new(ty).small().color(tk.text_dim));
    if o.kind == ObjectKind::Unit {
        let player = if o.player == 0 {
            "Neutral".to_string()
        } else {
            state
                .doc
                .info
                .players
                .iter()
                .find(|p| p.id == o.player)
                .map(|p| {
                    let default = format!("Player {}", p.id);
                    if p.name.is_empty() || p.name == default {
                        default
                    } else {
                        format!("{default} · {}", p.name)
                    }
                })
                .unwrap_or_else(|| format!("Player {}", o.player))
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, 2.0, overlay::player_color(&state.doc, o.player, tk));
            ui.label(egui::RichText::new(player).small().color(tk.text_dim));
        });
    }
    ui.label(egui::RichText::new(format!("x {:.1}, y {:.1}", o.position.x, o.position.y)).small().color(tk.text_dim));
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

/// Cursor readout for the status bar: position, height and cliff level under
/// the cursor, plus the brush size in brush modes. The shell shows the mode
/// and frame rate itself.
pub fn status_text(state: &EditorState) -> String {
    let vs = &state.ui.viewport;
    let mut parts = Vec::new();
    match vs.cursor {
        Some(c) => {
            let t = &state.doc.terrain;
            let vx = (c.x.round().max(0.0) as u32).min(t.width);
            let vy = (c.y.round().max(0.0) as u32).min(t.height);
            let cliff = t.cliff_levels[t.vertex_index(vx, vy)];
            parts.push(format!("x {:.1}, y {:.1}", c.x, c.y));
            parts.push(format!("height {:.1}", c.z));
            parts.push(format!("cliff {cliff}"));
        }
        None => parts.push("Cursor off map".into()),
    }
    if is_brush_mode(state.mode) {
        parts.push(format!("brush {:.1}", state.brush.radius));
    }
    parts.join(" · ")
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
        s.mode = Mode::Select;
        assert_eq!(status_text(&s), "Cursor off map");
        s.ui.viewport.cursor = Some(Vec3::new(3.0, 4.0, 12.0));
        assert_eq!(status_text(&s), "x 3.0, y 4.0 · height 12.0 · cliff 2");
        s.mode = Mode::Sculpt;
        let t = status_text(&s);
        assert!(t.ends_with(" · brush 6.0") && !t.contains("fps") && !t.contains("mode"), "{t}");
    }

    #[test]
    fn hud_picks_roomiest_layout_that_fits() {
        let mut f = HudFit::default();
        assert_eq!(f.level(500.0), 0, "unknown widths are tried first");
        f.widths[0] = Some(620.0);
        assert_eq!(f.level(500.0), 1);
        f.widths[1] = Some(520.0);
        f.widths[2] = Some(430.0);
        assert_eq!(f.level(500.0), 2);
        assert_eq!(f.level(700.0), 0, "widening goes back to the labeled layout");
        f.widths[3] = Some(380.0);
        assert_eq!(f.level(100.0), HUD_LEVELS - 1);
    }

    #[test]
    fn frame_map_fills_the_viewport() {
        let mut s = EditorState::new(MapDocument::new("t", 128, 128));
        s.ui.viewport.size = Vec2::new(930.0, 460.0);
        frame_map(&mut s, true);
        let v = s.ui.viewport.camera.view(s.ui.viewport.size);
        let corners = [Vec2::ZERO, Vec2::new(128.0, 0.0), Vec2::new(0.0, 128.0), Vec2::splat(128.0)];
        let z = s.doc.terrain.sample_height(0.0, 0.0);
        let pts: Vec<Vec2> = corners.iter().map(|c| v.project(c.extend(z)).unwrap()).collect();
        let lo = pts.iter().fold(Vec2::splat(f32::MAX), |a, b| a.min(*b));
        let hi = pts.iter().fold(Vec2::splat(f32::MIN), |a, b| a.max(*b));
        assert!(lo.x >= 0.0 && lo.y >= 0.0 && hi.x <= 930.0 && hi.y <= 460.0, "{lo:?} {hi:?}");
        assert!(hi.y - lo.y > 460.0 * 0.75, "map fills the height: {lo:?} {hi:?}");
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
