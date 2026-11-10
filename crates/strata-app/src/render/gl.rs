//! GPU side of the terrain viewport (glow: WebGL2 on web, GL 3.x native).
//!
//! The terrain is drawn into an offscreen framebuffer with a depth buffer
//! (multisampled when the driver allows it), resolved, and composited into
//! egui's framebuffer by a fullscreen triangle. This keeps depth testing and
//! MSAA independent of how eframe's own framebuffer was created.

use std::sync::{Arc, Mutex};

use eframe::egui_glow;
use glam::{Mat4, Vec3};
use glow::HasContext as _;

use super::textures::{LayerUpload, TEX_SIZE};

/// Direction toward the key light: from the west-north-west (upper left with
/// the default north-up camera) 40 degrees above the horizon, so slopes facing
/// the default camera still catch some light. Shared with the minimap so both
/// read the same relief.
pub const SUN: Vec3 = Vec3::new(-0.6943, 0.3237, 0.6428);

/// Pending uploads queued by the UI thread, consumed by the paint callback.
#[derive(Default)]
pub struct Uploads {
    /// Replace the whole mesh (first frame, load, resize).
    pub full: Option<FullUpload>,
    /// (byte offset, bytes) ranges of the vertex buffer.
    pub rows: Vec<(usize, Vec<u8>)>,
    /// Per-cell pathing flags (width, height, data).
    pub pathing: Option<(u32, u32, Vec<u8>)>,
    /// Terrain textures that arrived, for the texture arrays.
    pub layers: Vec<LayerUpload>,
}

pub struct FullUpload {
    pub vertices: Vec<u8>,
    pub indices: Vec<u32>,
}

/// Everything the shaders need for one frame. Colors are sRGB 0..1.
#[derive(Debug, Clone, Copy)]
pub struct FrameParams {
    pub view_proj: Mat4,
    pub eye: Vec3,
    pub map_size: [f32; 2],
    pub playable: [f32; 4],
    /// Lowest and highest terrain height, for height shading.
    pub height_range: [f32; 2],
    pub water: Option<f32>,
    pub show_grid: bool,
    pub show_contours: bool,
    pub cliff_tint: bool,
    pub show_pathing: bool,
    pub clear: [f32; 4],
    pub bounds_color: [f32; 4],
    pub unwalkable: [f32; 4],
    pub unbuildable: [f32; 4],
    pub nofly: [f32; 4],
    pub water_color: [f32; 4],
    /// Per texture layer: placeholder color (rgb) and flags (a), see
    /// [`super::textures::HAS_DIFFUSE`].
    pub layers: [[f32; 4]; 8],
    /// Texture repeats per cell.
    pub tex_scale: f32,
    /// Distance from the eye where the background haze starts. A huge value
    /// turns it off (map image exports).
    pub fog_start: f32,
}

/// A map image export waiting to be rendered by the next paint callback.
pub struct CaptureRequest {
    pub params: FrameParams,
    pub width: i32,
    pub height: i32,
    /// Render even if model meshes or textures are still queued for upload
    /// (the export stopped waiting).
    pub force: bool,
}

/// Pixels of a finished export: RGBA, rows top to bottom.
pub struct CapturedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Exports this large or smaller are anti-aliased; bigger ones would need
/// too much video memory for the multisampled buffers.
const CAPTURE_MSAA_PIXELS: i64 = 2048 * 2048;

/// State shared between the UI thread and the paint callback.
#[derive(Default)]
pub struct Shared {
    gpu: Option<Gpu>,
    pub uploads: Uploads,
    /// Set when GL setup failed; shown in the viewport.
    pub error: Option<String>,
    /// Number of MSAA samples in use (0 = none), for diagnostics.
    pub samples: i32,
    /// Placed units and doodads as 3D models, drawn after the terrain.
    pub models: super::models::ModelPass,
    /// A map image export to render (see [`CaptureRequest`]).
    pub capture: Option<CaptureRequest>,
    /// The finished export, or why it failed; taken by the UI.
    pub captured: Option<Result<CapturedImage, String>>,
    /// Longest image side the GPU can render; 0 until the renderer starts.
    pub max_capture_side: i32,
}

pub type SharedHandle = Arc<Mutex<Shared>>;

/// Build the paint callback for one frame.
pub fn callback(shared: SharedHandle, rect: egui::Rect, params: FrameParams) -> egui::PaintCallback {
    egui::PaintCallback {
        rect,
        callback: Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
            let Ok(mut s) = shared.lock() else { return };
            let gl = painter.gl();
            if s.gpu.is_none() && s.error.is_none() {
                match unsafe { Gpu::new(gl) } {
                    Ok(g) => {
                        s.samples = g.max_samples;
                        s.max_capture_side = g.max_side;
                        s.gpu = Some(g);
                    }
                    Err(e) => {
                        log::error!("terrain renderer: {e}");
                        s.error = Some(e);
                    }
                }
            }
            let Shared { gpu, uploads, models, capture, captured, .. } = &mut *s;
            if let Some(g) = gpu.as_mut() {
                unsafe {
                    g.upload(gl, uploads);
                    // An export renders first; the viewport then sets its
                    // own targets and egui's framebuffer back up.
                    if capture.as_ref().is_some_and(|c| c.force || !models.uploads_pending()) {
                        if let Some(req) = capture.take() {
                            *captured = Some(g.capture(gl, &req, models));
                        }
                    }
                    g.paint(gl, &info, &params, painter.intermediate_fbo(), models);
                }
            }
        })),
    }
}

struct Targets {
    w: i32,
    h: i32,
    msaa_fbo: Option<glow::Framebuffer>,
    msaa_rbs: Vec<glow::Renderbuffer>,
    resolve_fbo: glow::Framebuffer,
    resolve_tex: glow::Texture,
    resolve_depth: Option<glow::Renderbuffer>,
    /// The driver reported the resolve framebuffer complete.
    complete: bool,
}

struct Gpu {
    terrain: glow::Program,
    u_view_proj: Option<glow::UniformLocation>,
    u_sun: Option<glow::UniformLocation>,
    u_eye: Option<glow::UniformLocation>,
    u_map: Option<glow::UniformLocation>,
    u_playable: Option<glow::UniformLocation>,
    u_height: Option<glow::UniformLocation>,
    u_toggles: Option<glow::UniformLocation>,
    u_path: Option<glow::UniformLocation>,
    u_bounds_color: Option<glow::UniformLocation>,
    u_unwalk: Option<glow::UniformLocation>,
    u_unbuild: Option<glow::UniformLocation>,
    u_nofly: Option<glow::UniformLocation>,
    u_fog: Option<glow::UniformLocation>,
    u_diffuse: Option<glow::UniformLocation>,
    u_normals: Option<glow::UniformLocation>,
    u_layers: Option<glow::UniformLocation>,
    u_tex: Option<glow::UniformLocation>,
    water: glow::Program,
    w_view_proj: Option<glow::UniformLocation>,
    w_rect: Option<glow::UniformLocation>,
    w_color: Option<glow::UniformLocation>,
    w_eye: Option<glow::UniformLocation>,
    blit: glow::Program,
    b_tex: Option<glow::UniformLocation>,
    vao: glow::VertexArray,
    empty_vao: glow::VertexArray,
    vbo: glow::Buffer,
    ibo: glow::Buffer,
    index_count: i32,
    vbo_bytes: usize,
    path_tex: glow::Texture,
    path_size: (u32, u32),
    /// Diffuse and normal maps of the texture layers, one slot per layer.
    diffuse_arr: glow::Texture,
    normal_arr: glow::Texture,
    targets: Option<Targets>,
    max_samples: i32,
    /// Largest render target side the driver allows.
    max_side: i32,
}

const TERRAIN_VS: &str = r#"
in vec3 a_pos;
in vec3 a_normal;
in vec4 a_color;
in vec4 a_w0;
in vec4 a_w1;
uniform mat4 u_view_proj;
out vec3 v_world;
out vec3 v_normal;
out vec3 v_color;
out float v_cliff;
out vec4 v_w0;
out vec4 v_w1;
void main() {
    v_world = a_pos;
    v_normal = a_normal;
    v_color = a_color.rgb;
    v_cliff = a_color.a * 255.0 / 16.0;
    v_w0 = a_w0;
    v_w1 = a_w1;
    gl_Position = u_view_proj * vec4(a_pos, 1.0);
}
"#;

const TERRAIN_FS: &str = r#"
in vec3 v_world;
in vec3 v_normal;
in vec3 v_color;
in float v_cliff;
in vec4 v_w0;
in vec4 v_w1;
uniform vec3 u_sun;
uniform vec3 u_eye;
uniform vec2 u_map;
uniform vec4 u_playable;
// Terrain height range (min, max) for height shading.
uniform vec2 u_height;
// x: grid, y: contours, z: cliff tint, w: pathing overlay
uniform vec4 u_toggles;
uniform sampler2D u_path;
uniform vec4 u_bounds_color;
uniform vec4 u_unwalk;
uniform vec4 u_unbuild;
uniform vec4 u_nofly;
// rgb: fog color, a: fog start distance
uniform vec4 u_fog;
// The game's terrain textures, one array slot per layer.
uniform sampler2DArray u_diffuse;
uniform sampler2DArray u_normals;
// Per layer: placeholder color (rgb), flags (a): 1 diffuse, 2 normal map.
uniform vec4 u_layers[8];
// x: texture repeats per cell, y: diffuse gain
uniform vec2 u_tex;
out vec4 o_color;

float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

float vnoise(vec2 p) {
    vec2 i = floor(p);
    vec2 f = fract(p);
    vec2 u = f * f * (3.0 - 2.0 * f);
    float a = hash12(i);
    float b = hash12(i + vec2(1.0, 0.0));
    float c = hash12(i + vec2(0.0, 1.0));
    float d = hash12(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Anti-aliased line at integer values of `coord`, `px` pixels wide.
float grid_line(float coord, float px) {
    float w = max(fwidth(coord), 1e-5);
    float d = abs(fract(coord - 0.5) - 0.5) / w;
    return 1.0 - clamp(d - px * 0.5 + 0.5, 0.0, 1.0);
}

vec3 hsv(float h, float s, float v) {
    vec3 k = clamp(abs(fract(h + vec3(0.0, 2.0 / 3.0, 1.0 / 3.0)) * 6.0 - 3.0) - 1.0, 0.0, 1.0);
    return v * mix(vec3(1.0), k, s);
}

vec3 cliff_color(float level) {
    // Neighbouring levels get well-separated hues (golden-ratio steps) so
    // terraces read at a glance; higher levels are a little brighter.
    return hsv(fract(0.58 + level * 0.381966), 0.55, 0.55 + 0.03 * level);
}

void main() {
    vec3 n = normalize(v_normal);

    // Blend the layers: real textures where loaded, placeholder colors
    // elsewhere. Gradients are taken once, outside the per-layer branches.
    vec2 uv = v_world.xy * u_tex.x;
    vec2 gx = dFdx(uv);
    vec2 gy = dFdy(uv);
    // Steep faces (cliff walls, drawn without the game's cliff meshes) also
    // sample side projections so the textures don't smear down the wall.
    vec2 uvx = v_world.yz * u_tex.x;
    vec2 uvy = v_world.xz * u_tex.x;
    vec2 gxx = dFdx(uvx);
    vec2 gyx = dFdy(uvx);
    vec2 gxy = dFdx(uvy);
    vec2 gyy = dFdy(uvy);
    vec3 tri = pow(abs(n), vec3(4.0));
    tri /= tri.x + tri.y + tri.z;
    tri = mix(tri, vec3(0.0), step(tri, vec3(0.03)));
    tri /= max(tri.x + tri.y + tri.z, 1e-4);
    float w[8];
    w[0] = v_w0.x; w[1] = v_w0.y; w[2] = v_w0.z; w[3] = v_w0.w;
    w[4] = v_w1.x; w[5] = v_w1.y; w[6] = v_w1.z; w[7] = v_w1.w;
    vec3 tex = vec3(0.0);
    vec3 flat_col = vec3(0.0);
    float flat_w = 0.0;
    vec2 bump = vec2(0.0);
    float total = 0.0;
    for (int i = 0; i < 8; i++) {
        float wi = w[i];
        if (wi < 0.002) continue;
        total += wi;
        vec4 layer = u_layers[i];
        int flags = int(layer.a + 0.5);
        if ((flags & 1) != 0) {
            float s = float(i);
            vec3 c = vec3(0.0);
            if (tri.z > 0.0) c += textureGrad(u_diffuse, vec3(uv, s), gx, gy).rgb * tri.z;
            if (tri.x > 0.0) c += textureGrad(u_diffuse, vec3(uvx, s), gxx, gyx).rgb * tri.x;
            if (tri.y > 0.0) c += textureGrad(u_diffuse, vec3(uvy, s), gxy, gyy).rgb * tri.y;
            tex += c * wi;
        } else {
            flat_col += layer.rgb * wi;
            flat_w += wi;
        }
        if ((flags & 2) != 0 && tri.z > 0.0) {
            vec4 nm = textureGrad(u_normals, vec3(uv, float(i)), gx, gy);
            bump += (vec2(nm.a, nm.g) * 2.0 - 1.0) * wi * tri.z;
        }
    }
    total = max(total, 1e-4);
    // Share of the surface still painted with placeholder colors.
    float placeholder = flat_w / total;

    // Procedural grain so placeholder colors do not read as plastic.
    float g = vnoise(v_world.xy * 1.3) * 0.6 + vnoise(v_world.xy * 5.1) * 0.4;
    vec3 base = (tex * u_tex.y + flat_col * (0.92 + 0.16 * g)) / total;

    if (u_toggles.z > 0.5) {
        base = mix(base, cliff_color(floor(v_cliff + 0.5)), 0.6);
    }

    // Slope tint: gentle slopes pick up a little warmth, steep faces turn
    // cooler and greyer, like exposed rock. Real textures get only a hint.
    float slope = 1.0 - n.z;
    float lum = dot(base, vec3(0.299, 0.587, 0.114));
    float rock = smoothstep(0.18, 0.62, slope);
    float tint = mix(0.25, 1.0, placeholder);
    base = mix(base, base * vec3(1.06, 1.0, 0.92), smoothstep(0.02, 0.18, slope) * (1.0 - rock) * tint);
    base = mix(base, vec3(lum * 1.08) * vec3(0.97, 0.96, 0.95), rock * 0.6 * tint);

    // Normal maps tilt the shading normal in the ground plane (DirectX
    // convention: +x along u, +y along v, which follow world x and y).
    if (bump != vec2(0.0)) {
        vec3 t = normalize(vec3(1.0, 0.0, 0.0) - n * n.x);
        vec3 b = cross(n, t);
        bump /= total;
        n = normalize(n + t * bump.x + b * bump.y);
    }

    // Subtle height shading: valleys a touch darker, peaks a touch lighter.
    float span = u_height.y - u_height.x;
    if (span > 0.5) {
        float h01 = clamp((v_world.z - u_height.x) / span, 0.0, 1.0);
        base *= mix(0.86, 1.08, h01);
    }

    // Key light from the upper left at ~40 degrees, cool sky / warm ground
    // hemisphere fill. The fill keeps faces turned away from the sun readable.
    float hemi = n.z * 0.5 + 0.5;
    vec3 ambient = mix(vec3(0.30, 0.25, 0.21), vec3(0.47, 0.52, 0.61), hemi);
    float ndl = dot(n, u_sun);
    // Slightly wrapped diffuse softens the terminator without flattening relief.
    float diff = clamp((ndl + 0.1) / 1.1, 0.0, 1.0);
    vec3 lit = base * (ambient + vec3(1.0, 0.94, 0.84) * diff * 0.86);
    // Soft shoulder: bright textures (Elsecaro, snow) keep their detail
    // instead of clipping to white.
    vec3 hi = 0.75 + 0.25 * (1.0 - exp(-(lit - 0.75) * 4.0));
    lit = mix(lit, hi, step(vec3(0.75), lit));

    if (u_toggles.y > 0.5) {
        float minor = grid_line(v_world.z, 1.0);
        float major = grid_line(v_world.z * 0.5, 1.5);
        lit = mix(lit, lit * 0.5, minor * 0.35);
        lit = mix(lit, lit * 0.35, major * 0.55);
    }

    if (u_toggles.x > 0.5) {
        // Cell lines fade out as cells shrink on screen (zoomed out or far
        // away); every eighth line stays a little longer to keep scale.
        float density = max(max(fwidth(v_world.x), fwidth(v_world.y)), 1e-4);
        float cell_px = 1.0 / density;
        float minor = max(grid_line(v_world.x, 1.0), grid_line(v_world.y, 1.0));
        float major = max(grid_line(v_world.x / 8.0, 1.0), grid_line(v_world.y / 8.0, 1.0));
        float minor_a = 0.10 * smoothstep(5.0, 14.0, cell_px);
        float major_a = 0.16 * smoothstep(8.0, 24.0, cell_px * 8.0);
        // Lines thin out on steep faces, where they would bunch into a mesh.
        float a = max(minor * minor_a, major * major_a) * (1.0 - 0.75 * rock);
        lit = mix(lit, vec3(1.0, 0.98, 0.95), a);
    }

    if (u_toggles.w > 0.5) {
        vec2 cell = clamp(floor(v_world.xy), vec2(0.0), u_map - 1.0);
        int f = int(texelFetch(u_path, ivec2(cell), 0).r * 255.0 + 0.5);
        if ((f & 1) != 0) {
            lit = mix(lit, u_unwalk.rgb, 0.55);
        } else if ((f & 2) != 0) {
            lit = mix(lit, u_unbuild.rgb, 0.45);
        }
        if ((f & 4) != 0) {
            float stripe = step(0.5, fract((v_world.x + v_world.y) * 1.5));
            lit = mix(lit, u_nofly.rgb, 0.35 * stripe);
        }
    }

    // Darken outside the playable area and outline its edge.
    vec2 p = v_world.xy;
    vec2 c = (u_playable.xy + u_playable.zw) * 0.5;
    vec2 h = (u_playable.zw - u_playable.xy) * 0.5;
    vec2 q = abs(p - c) - h;
    float sd = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0);
    if (sd > 0.0) {
        lit *= 0.42;
    }
    float edge = 1.0 - clamp(abs(sd) / max(fwidth(sd), 1e-5) - 0.75, 0.0, 1.0);
    lit = mix(lit, u_bounds_color.rgb, edge * 0.85);

    // Gentle distance haze toward the background.
    float dist = length(u_eye - v_world);
    float fog = clamp((dist - u_fog.a) / (u_fog.a * 2.0 + 1.0), 0.0, 0.45);
    lit = mix(lit, u_fog.rgb, fog);

    o_color = vec4(lit, 1.0);
}
"#;

const WATER_VS: &str = r#"
uniform mat4 u_view_proj;
// x0, y0, x1, y1 with z in a separate slot
uniform vec4 u_rect;
uniform vec4 u_color;
out vec3 v_world;
void main() {
    int id = gl_VertexID;
    vec2 corner = vec2((id == 1 || id == 2 || id == 4) ? 1.0 : 0.0, (id == 2 || id == 4 || id == 5) ? 1.0 : 0.0);
    vec2 xy = mix(u_rect.xy, u_rect.zw, corner);
    v_world = vec3(xy, u_color.a);
    gl_Position = u_view_proj * vec4(v_world, 1.0);
}
"#;

const WATER_FS: &str = r#"
in vec3 v_world;
uniform vec4 u_color;
uniform vec3 u_eye;
out vec4 o_color;
void main() {
    vec3 v = normalize(u_eye - v_world);
    float fresnel = pow(1.0 - clamp(v.z, 0.0, 1.0), 3.0);
    vec3 col = mix(u_color.rgb, vec3(0.75, 0.85, 0.95), fresnel * 0.6);
    o_color = vec4(col, 0.55 + fresnel * 0.3);
}
"#;

const BLIT_VS: &str = r#"
out vec2 v_uv;
void main() {
    vec2 p = vec2(gl_VertexID == 1 ? 3.0 : -1.0, gl_VertexID == 2 ? 3.0 : -1.0);
    v_uv = p * 0.5 + 0.5;
    gl_Position = vec4(p, 0.0, 1.0);
}
"#;

const BLIT_FS: &str = r#"
in vec2 v_uv;
uniform sampler2D u_tex;
out vec4 o_color;
void main() {
    o_color = vec4(texture(u_tex, v_uv).rgb, 1.0);
}
"#;

/// SC2's terrain textures are authored dark and brightened by the game's
/// lighting; this lifts them to read like the editor's default lighting.
const TEXTURE_GAIN: f32 = 1.0;

/// A `TEX_SIZE`² x 8 RGBA texture array with mipmaps, repeating, filtered
/// anisotropically where the driver allows.
/// Reverse the row order of an image with `stride` bytes per row (GL reads
/// bottom-up; images are top-down).
fn flip_rows(px: &mut [u8], stride: usize) {
    let rows = px.len() / stride.max(1);
    for y in 0..rows / 2 {
        let (top, bottom) = px.split_at_mut((rows - 1 - y) * stride);
        top[y * stride..(y + 1) * stride].swap_with_slice(&mut bottom[..stride]);
    }
}

unsafe fn delete_targets(gl: &glow::Context, t: Targets) {
    unsafe {
        if let Some(f) = t.msaa_fbo {
            gl.delete_framebuffer(f);
        }
        for rb in t.msaa_rbs {
            gl.delete_renderbuffer(rb);
        }
        if let Some(rb) = t.resolve_depth {
            gl.delete_renderbuffer(rb);
        }
        gl.delete_framebuffer(t.resolve_fbo);
        gl.delete_texture(t.resolve_tex);
    }
}

/// A color + depth target of `w` x `h` pixels: multisampled with `samples`
/// when the driver allows (set to 0 when it doesn't), resolved into a texture.
unsafe fn create_targets(gl: &glow::Context, w: i32, h: i32, samples: &mut i32) -> Result<Targets, String> {
    unsafe {
        let resolve_tex = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D, Some(resolve_tex));
        gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            w,
            h,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        for (p, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
            (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
            (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
            (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
        }
        gl.bind_texture(glow::TEXTURE_2D, None);
        let resolve_fbo = gl.create_framebuffer()?;
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolve_fbo));
        gl.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(resolve_tex), 0);

        let mut targets = Targets {
            w,
            h,
            msaa_fbo: None,
            msaa_rbs: Vec::new(),
            resolve_fbo,
            resolve_tex,
            resolve_depth: None,
            complete: false,
        };
        let mut msaa_ok = false;
        if *samples > 1 {
            let fbo = gl.create_framebuffer()?;
            let color = gl.create_renderbuffer()?;
            let depth = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(color));
            gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, *samples, glow::RGBA8, w, h);
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            gl.renderbuffer_storage_multisample(glow::RENDERBUFFER, *samples, glow::DEPTH_COMPONENT24, w, h);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(color));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(depth));
            msaa_ok = gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE;
            if msaa_ok {
                targets.msaa_fbo = Some(fbo);
                targets.msaa_rbs = vec![color, depth];
            } else {
                log::warn!("MSAA framebuffer incomplete; rendering without anti-aliasing");
                gl.delete_framebuffer(fbo);
                gl.delete_renderbuffer(color);
                gl.delete_renderbuffer(depth);
                *samples = 0;
            }
        }
        if !msaa_ok {
            let depth = gl.create_renderbuffer()?;
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::DEPTH_COMPONENT24, w, h);
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolve_fbo));
            gl.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(depth));
            targets.resolve_depth = Some(depth);
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, Some(resolve_fbo));
        let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
        gl.bind_renderbuffer(glow::RENDERBUFFER, None);
        targets.complete = status == glow::FRAMEBUFFER_COMPLETE;
        if !targets.complete {
            log::error!("offscreen framebuffer {w}x{h} incomplete (0x{status:x})");
        }
        Ok(targets)
    }
}

unsafe fn layer_array(gl: &glow::Context, aniso: Option<f32>) -> Result<glow::Texture, String> {
    unsafe {
        let t = gl.create_texture()?;
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(t));
        let side = TEX_SIZE as i32;
        gl.tex_image_3d(
            glow::TEXTURE_2D_ARRAY,
            0,
            glow::RGBA8 as i32,
            side,
            side,
            8,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(None),
        );
        for (p, v) in [
            (glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR),
            (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
            (glow::TEXTURE_WRAP_S, glow::REPEAT),
            (glow::TEXTURE_WRAP_T, glow::REPEAT),
        ] {
            gl.tex_parameter_i32(glow::TEXTURE_2D_ARRAY, p, v as i32);
        }
        if let Some(a) = aniso {
            gl.tex_parameter_f32(glow::TEXTURE_2D_ARRAY, glow::TEXTURE_MAX_ANISOTROPY_EXT, a);
        }
        gl.generate_mipmap(glow::TEXTURE_2D_ARRAY);
        gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
        Ok(t)
    }
}

pub(super) fn header(gl: &glow::Context) -> Result<&'static str, String> {
    match egui_glow::ShaderVersion::get(gl) {
        egui_glow::ShaderVersion::Es300 => {
            Ok("#version 300 es\nprecision highp float;\nprecision highp int;\nprecision highp sampler2D;\nprecision highp sampler2DArray;\n")
        }
        egui_glow::ShaderVersion::Gl140 => Ok("#version 140\n"),
        v => Err(format!("The 3D view needs WebGL 2 or OpenGL 3.1 (found {v:?}).")),
    }
}

pub(super) unsafe fn compile(
    gl: &glow::Context,
    hdr: &str,
    vs: &str,
    fs: &str,
    attribs: &[&str],
) -> Result<glow::Program, String> {
    unsafe {
        let program = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
            let s = gl.create_shader(kind)?;
            gl.shader_source(s, &format!("{hdr}{src}"));
            gl.compile_shader(s);
            if !gl.get_shader_compile_status(s) {
                return Err(format!("shader compile failed: {}", gl.get_shader_info_log(s)));
            }
            gl.attach_shader(program, s);
            shaders.push(s);
        }
        for (i, a) in attribs.iter().enumerate() {
            gl.bind_attrib_location(program, i as u32, a);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            return Err(format!("shader link failed: {}", gl.get_program_info_log(program)));
        }
        for s in shaders {
            gl.detach_shader(program, s);
            gl.delete_shader(s);
        }
        Ok(program)
    }
}

impl Gpu {
    unsafe fn new(gl: &glow::Context) -> Result<Self, String> {
        unsafe {
            let hdr = header(gl)?;
            let terrain =
                compile(gl, hdr, TERRAIN_VS, TERRAIN_FS, &["a_pos", "a_normal", "a_color", "a_w0", "a_w1"])?;
            let water = compile(gl, hdr, WATER_VS, WATER_FS, &[])?;
            let blit = compile(gl, hdr, BLIT_VS, BLIT_FS, &[])?;
            let u = |p: glow::Program, n: &str| gl.get_uniform_location(p, n);

            let vao = gl.create_vertex_array()?;
            let empty_vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            let ibo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
            let stride = std::mem::size_of::<super::mesh::Vertex>() as i32;
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
            gl.enable_vertex_attrib_array(1);
            gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
            gl.enable_vertex_attrib_array(2);
            gl.vertex_attrib_pointer_f32(2, 4, glow::UNSIGNED_BYTE, true, stride, 24);
            gl.enable_vertex_attrib_array(3);
            gl.vertex_attrib_pointer_f32(3, 4, glow::UNSIGNED_BYTE, true, stride, 28);
            gl.enable_vertex_attrib_array(4);
            gl.vertex_attrib_pointer_f32(4, 4, glow::UNSIGNED_BYTE, true, stride, 32);
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);

            let path_tex = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(path_tex));
            for (p, v) in [
                (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
            }
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::R8 as i32,
                1,
                1,
                0,
                glow::RED,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&[0u8])),
            );
            gl.bind_texture(glow::TEXTURE_2D, None);

            let max_samples = gl.get_parameter_i32(glow::MAX_SAMPLES).clamp(0, 4);
            let max_side = gl
                .get_parameter_i32(glow::MAX_RENDERBUFFER_SIZE)
                .min(gl.get_parameter_i32(glow::MAX_TEXTURE_SIZE))
                .max(0);
            let aniso = gl
                .supported_extensions()
                .iter()
                .any(|e| e.ends_with("texture_filter_anisotropic"))
                .then(|| gl.get_parameter_f32(glow::MAX_TEXTURE_MAX_ANISOTROPY_EXT).min(8.0))
                .filter(|a| *a > 1.0);
            let diffuse_arr = layer_array(gl, aniso)?;
            let normal_arr = layer_array(gl, aniso)?;

            Ok(Self {
                u_view_proj: u(terrain, "u_view_proj"),
                u_sun: u(terrain, "u_sun"),
                u_eye: u(terrain, "u_eye"),
                u_map: u(terrain, "u_map"),
                u_playable: u(terrain, "u_playable"),
                u_height: u(terrain, "u_height"),
                u_toggles: u(terrain, "u_toggles"),
                u_path: u(terrain, "u_path"),
                u_bounds_color: u(terrain, "u_bounds_color"),
                u_unwalk: u(terrain, "u_unwalk"),
                u_unbuild: u(terrain, "u_unbuild"),
                u_nofly: u(terrain, "u_nofly"),
                u_fog: u(terrain, "u_fog"),
                u_diffuse: u(terrain, "u_diffuse"),
                u_normals: u(terrain, "u_normals"),
                u_layers: u(terrain, "u_layers"),
                u_tex: u(terrain, "u_tex"),
                terrain,
                w_view_proj: u(water, "u_view_proj"),
                w_rect: u(water, "u_rect"),
                w_color: u(water, "u_color"),
                w_eye: u(water, "u_eye"),
                water,
                b_tex: u(blit, "u_tex"),
                blit,
                vao,
                empty_vao,
                vbo,
                ibo,
                index_count: 0,
                vbo_bytes: 0,
                path_tex,
                path_size: (1, 1),
                diffuse_arr,
                normal_arr,
                targets: None,
                max_samples,
                max_side,
            })
        }
    }

    unsafe fn upload(&mut self, gl: &glow::Context, up: &mut Uploads) {
        unsafe {
            if let Some(full) = up.full.take() {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, &full.vertices, glow::DYNAMIC_DRAW);
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
                self.vbo_bytes = full.vertices.len();
                gl.bind_vertex_array(Some(self.vao));
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ibo));
                gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    bytemuck::cast_slice(&full.indices),
                    glow::STATIC_DRAW,
                );
                gl.bind_vertex_array(None);
                self.index_count = full.indices.len() as i32;
                up.rows.clear();
            }
            if !up.rows.is_empty() {
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
                for (off, bytes) in up.rows.drain(..) {
                    if off + bytes.len() <= self.vbo_bytes {
                        gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, off as i32, &bytes);
                    }
                }
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
            }
            if let Some((w, h, data)) = up.pathing.take() {
                gl.bind_texture(glow::TEXTURE_2D, Some(self.path_tex));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                if self.path_size == (w, h) {
                    gl.tex_sub_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        0,
                        0,
                        w as i32,
                        h as i32,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&data)),
                    );
                } else {
                    gl.tex_image_2d(
                        glow::TEXTURE_2D,
                        0,
                        glow::R8 as i32,
                        w as i32,
                        h as i32,
                        0,
                        glow::RED,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&data)),
                    );
                    self.path_size = (w, h);
                }
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            if !up.layers.is_empty() {
                let mut touched = [false; 2];
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 4);
                for l in up.layers.drain(..) {
                    let side = TEX_SIZE as i32;
                    if l.slot >= 8 || l.rgba.len() != (side * side * 4) as usize {
                        continue;
                    }
                    let arr = if l.normal { self.normal_arr } else { self.diffuse_arr };
                    touched[l.normal as usize] = true;
                    gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(arr));
                    gl.tex_sub_image_3d(
                        glow::TEXTURE_2D_ARRAY,
                        0,
                        0,
                        0,
                        l.slot as i32,
                        side,
                        side,
                        1,
                        glow::RGBA,
                        glow::UNSIGNED_BYTE,
                        glow::PixelUnpackData::Slice(Some(&l.rgba)),
                    );
                }
                for (i, arr) in [self.diffuse_arr, self.normal_arr].into_iter().enumerate() {
                    if touched[i] {
                        gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(arr));
                        gl.generate_mipmap(glow::TEXTURE_2D_ARRAY);
                    }
                }
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
            }
        }
    }

    unsafe fn ensure_targets(&mut self, gl: &glow::Context, w: i32, h: i32) -> Result<(), String> {
        unsafe {
            if self.targets.as_ref().is_some_and(|t| t.w == w && t.h == h) {
                return Ok(());
            }
            if let Some(t) = self.targets.take() {
                delete_targets(gl, t);
            }
            let t = create_targets(gl, w, h, &mut self.max_samples)?;
            let complete = t.complete;
            self.targets = Some(t);
            if !complete {
                return Err("offscreen framebuffer incomplete".into());
            }
            Ok(())
        }
    }

    /// Render `req` into its own offscreen target and read the pixels back.
    unsafe fn capture(
        &mut self,
        gl: &glow::Context,
        req: &CaptureRequest,
        models: &mut super::models::ModelPass,
    ) -> Result<CapturedImage, String> {
        unsafe {
            let (w, h) = (req.width, req.height);
            if w < 1 || h < 1 || w > self.max_side || h > self.max_side {
                return Err(format!("the graphics card can't render a {w}x{h} image (largest side {})", self.max_side));
            }
            let mut samples = if (w as i64) * (h as i64) <= CAPTURE_MSAA_PIXELS { self.max_samples } else { 0 };
            let t = create_targets(gl, w, h, &mut samples)?;
            if !t.complete {
                delete_targets(gl, t);
                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                return Err(format!("the graphics card couldn't make a {w}x{h} image; try a smaller width"));
            }
            self.draw_scene(gl, &req.params, &t, models, true);
            let mut rgba = vec![0u8; w as usize * h as usize * 4];
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(t.resolve_fbo));
            gl.pixel_store_i32(glow::PACK_ALIGNMENT, 4);
            gl.read_pixels(0, 0, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut rgba)));
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            delete_targets(gl, t);
            flip_rows(&mut rgba, w as usize * 4);
            Ok(CapturedImage { width: w as u32, height: h as u32, rgba })
        }
    }

    /// Draw terrain, models and water into `t` (and resolve its MSAA).
    unsafe fn draw_scene(
        &self,
        gl: &glow::Context,
        p: &FrameParams,
        t: &Targets,
        models: &mut super::models::ModelPass,
        capture: bool,
    ) {
        unsafe {
            let (w, h) = (t.w, t.h);
            let draw_fbo = t.msaa_fbo.unwrap_or(t.resolve_fbo);

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(draw_fbo));
            gl.viewport(0, 0, w, h);
            gl.disable(glow::SCISSOR_TEST);
            gl.disable(glow::BLEND);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.enable(glow::CULL_FACE);
            gl.cull_face(glow::BACK);
            gl.front_face(glow::CCW);
            gl.color_mask(true, true, true, true);
            gl.clear_color(p.clear[0], p.clear[1], p.clear[2], 1.0);
            gl.clear_depth_f32(1.0);
            gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);

            let vp_cols = p.view_proj.to_cols_array();
            if self.index_count > 0 {
                gl.use_program(Some(self.terrain));
                gl.uniform_matrix_4_f32_slice(self.u_view_proj.as_ref(), false, &vp_cols);
                let sun = SUN;
                gl.uniform_3_f32(self.u_sun.as_ref(), sun.x, sun.y, sun.z);
                gl.uniform_2_f32(self.u_height.as_ref(), p.height_range[0], p.height_range[1]);
                gl.uniform_3_f32(self.u_eye.as_ref(), p.eye.x, p.eye.y, p.eye.z);
                gl.uniform_2_f32(self.u_map.as_ref(), p.map_size[0], p.map_size[1]);
                let [a, b, c, d] = p.playable;
                gl.uniform_4_f32(self.u_playable.as_ref(), a, b, c, d);
                let f = |b: bool| if b { 1.0 } else { 0.0 };
                gl.uniform_4_f32(
                    self.u_toggles.as_ref(),
                    f(p.show_grid),
                    f(p.show_contours),
                    f(p.cliff_tint),
                    f(p.show_pathing),
                );
                let v4 = |l: Option<&glow::UniformLocation>, c: [f32; 4]| gl.uniform_4_f32(l, c[0], c[1], c[2], c[3]);
                v4(self.u_bounds_color.as_ref(), p.bounds_color);
                v4(self.u_unwalk.as_ref(), p.unwalkable);
                v4(self.u_unbuild.as_ref(), p.unbuildable);
                v4(self.u_nofly.as_ref(), p.nofly);
                gl.uniform_4_f32(self.u_fog.as_ref(), p.clear[0], p.clear[1], p.clear[2], p.fog_start);
                let layers: Vec<f32> = p.layers.iter().flatten().copied().collect();
                gl.uniform_4_f32_slice(self.u_layers.as_ref(), &layers);
                gl.uniform_2_f32(self.u_tex.as_ref(), p.tex_scale, TEXTURE_GAIN);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.path_tex));
                gl.uniform_1_i32(self.u_path.as_ref(), 0);
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.diffuse_arr));
                gl.uniform_1_i32(self.u_diffuse.as_ref(), 1);
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, Some(self.normal_arr));
                gl.uniform_1_i32(self.u_normals.as_ref(), 2);
                gl.bind_vertex_array(Some(self.vao));
                gl.draw_elements(glow::TRIANGLES, self.index_count, glow::UNSIGNED_INT, 0);
                gl.bind_vertex_array(None);
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D_ARRAY, None);
                gl.active_texture(glow::TEXTURE0);
            }

            models.paint(gl, p, capture);

            if let Some(z) = p.water {
                gl.disable(glow::CULL_FACE);
                gl.enable(glow::BLEND);
                gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                gl.depth_mask(false);
                gl.use_program(Some(self.water));
                gl.uniform_matrix_4_f32_slice(self.w_view_proj.as_ref(), false, &vp_cols);
                gl.uniform_4_f32(self.w_rect.as_ref(), 0.0, 0.0, p.map_size[0], p.map_size[1]);
                let c = p.water_color;
                gl.uniform_4_f32(self.w_color.as_ref(), c[0], c[1], c[2], z);
                gl.uniform_3_f32(self.w_eye.as_ref(), p.eye.x, p.eye.y, p.eye.z);
                gl.bind_vertex_array(Some(self.empty_vao));
                gl.draw_arrays(glow::TRIANGLES, 0, 6);
                gl.bind_vertex_array(None);
                gl.depth_mask(true);
                gl.disable(glow::BLEND);
            }

            if let Some(msaa) = t.msaa_fbo {
                gl.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(msaa));
                gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(t.resolve_fbo));
                gl.blit_framebuffer(0, 0, w, h, 0, 0, w, h, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            }
        }
    }

    unsafe fn paint(
        &mut self,
        gl: &glow::Context,
        info: &egui::PaintCallbackInfo,
        p: &FrameParams,
        out_fbo: Option<glow::Framebuffer>,
        models: &mut super::models::ModelPass,
    ) {
        unsafe {
            let vp = info.viewport_in_pixels();
            let (w, h) = (vp.width_px.max(1), vp.height_px.max(1));
            if let Err(e) = self.ensure_targets(gl, w, h) {
                log::error!("{e}");
                gl.bind_framebuffer(glow::FRAMEBUFFER, out_fbo);
                return;
            }
            let t = self.targets.as_ref().expect("targets");
            self.draw_scene(gl, p, t, models, false);

            // Composite into egui's framebuffer, clipped to the panel.
            gl.bind_framebuffer(glow::FRAMEBUFFER, out_fbo);
            gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
            let clip = info.clip_rect_in_pixels();
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(clip.left_px, clip.from_bottom_px, clip.width_px, clip.height_px);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.disable(glow::BLEND);
            gl.use_program(Some(self.blit));
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(t.resolve_tex));
            gl.uniform_1_i32(self.b_tex.as_ref(), 0);
            gl.bind_vertex_array(Some(self.empty_vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            gl.use_program(None);
        }
    }
}

#[cfg(test)]
mod capture_tests {
    #[test]
    fn rows_flip_top_to_bottom() {
        let mut px = vec![1, 1, 2, 2, 3, 3];
        super::flip_rows(&mut px, 2);
        assert_eq!(px, vec![3, 3, 2, 2, 1, 1]);
        let mut even = vec![1, 2, 3, 4];
        super::flip_rows(&mut even, 2);
        assert_eq!(even, vec![3, 4, 1, 2]);
    }
}
