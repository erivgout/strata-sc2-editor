//! Placed units and doodads drawn as their real `.m3` models, read from the
//! user's own StarCraft II install through [`crate::assets`].
//!
//! UI thread ([`ModelRenderer::prepare`], every frame):
//! 1. Placed types are resolved to model files through the catalogs
//!    ([`strata_core::models::ModelLookup`]: map data, then the data of the
//!    map's campaign / co-op dependencies, then the melee base game).
//! 2. Objects are frustum-culled; only types with something on screen start
//!    loading. Model bytes and textures come from the asset service without
//!    blocking; a few models are parsed per frame (`strata_m3`).
//! 3. Visible instances are grouped per model into one instanced draw per
//!    submesh, with the owner's team color, and handed to the GPU pass with
//!    any pending mesh / texture uploads.
//!
//! GPU side ([`ModelPass::paint`]): called from the terrain paint callback
//! into the same depth-tested offscreen target, opaque and cutout submeshes
//! first, then blended ones without depth writes.
//!
//! Objects without a model yet (loading, not in the data, no game install)
//! keep their 2D marker; [`ModelRenderer::drawn`] tells the overlay which
//! objects are covered by a model so it only adds rings and labels to them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use glam::{Mat4, Quat, Vec3, Vec4};
use glow::HasContext as _;
use strata_core::base_data::BaseCatalog;
use strata_core::models::{ModelLookup, ModelRef};
use strata_core::objects::{ObjectId, ObjectKind, PlacedObject};

use super::camera::View;
use super::gl::{FrameParams, SharedHandle};
use crate::assets::{Asset, Assets};
use crate::state::EditorState;

/// Longest texture side requested from the asset service.
const TEXTURE_SIDE: u32 = 512;
/// Models parsed per frame at most (parsing a large model takes ~1 ms).
const PARSES_PER_FRAME: usize = 3;
/// Draw budget: instances and triangles per frame, nearest first.
const MAX_INSTANCES: usize = 6000;
const MAX_TRIANGLES: usize = 6_000_000;
/// Models smaller than this on screen (radius in points) are skipped:
/// they would cover less than a pixel.
const MIN_SCREEN_RADIUS: f32 = 0.5;
/// How long to wait before asking again for a model whose file was not found
/// (the asset service may still be opening the install).
const RETRY_SECONDS: f64 = 3.0;

/// Where an object sits and how big it is on the ground, for the overlay.
#[derive(Debug, Clone, Copy)]
pub struct ModelMark {
    /// Base of the model (ground point under it).
    pub anchor: Vec3,
    /// Footprint radius in world units.
    pub radius: f32,
}

/// Per-frame statistics, for the status line / diagnostics.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModelStats {
    pub types: usize,
    pub types_ready: usize,
    pub types_missing: usize,
    pub instances: usize,
    pub triangles: usize,
    pub prepare_ms: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TypeKey {
    unit: bool,
    id: String,
    /// The placed object's `Variation` (doodads with numbered model files).
    variation: u32,
}

enum TypeState {
    /// Resolved to a model key (index into `models`).
    Model(usize),
    /// No model in the loaded data (keeps its marker).
    Missing,
}

/// One texture slot of a material.
#[derive(Default)]
struct TexSlot {
    /// Path as written in the model.
    path: Option<String>,
    /// Index into `textures` once a CASC path is being fetched.
    tex: Option<usize>,
}

struct Sub {
    first: u32,
    count: u32,
    material: strata_m3::Material,
    diffuse: TexSlot,
    emissive: TexSlot,
    alpha: TexSlot,
    /// Diffuse alpha is a team-color mask (opaque material whose cutout, if
    /// any, comes from another texture).
    team_color: bool,
}

enum ModelState {
    /// Trying CASC paths for the file, in order.
    Locating { candidates: Vec<String>, next: usize },
    /// Waiting for the bytes.
    Loading { path: String },
    /// Parsed and queued for upload.
    Ready { gpu: u32, bounds: strata_m3::Bounds, subs: Vec<Sub>, triangles: usize },
    /// Not found or unreadable; retried after a while.
    Failed { at: f64, why: String },
}

struct ModelEntry {
    model: ModelRef,
    /// Data paths to try, in order (a variation's file, then its fallbacks).
    files: Vec<String>,
    state: ModelState,
}

enum TexState {
    Locating {
        candidates: Vec<String>,
        next: usize,
    },
    Ready(u32),
    /// Not found (as of this time); retried later.
    Failed(f64),
}

/// Map dependencies' catalogs (campaign / co-op data), loaded once per
/// dependency list.
#[derive(Default)]
struct Supplement {
    key: Vec<String>,
    roots: Vec<String>,
    state: SupplementState,
}

#[derive(Default)]
enum SupplementState {
    #[default]
    Idle,
    /// Collecting `(root, file)` bytes from the asset service.
    Fetching {
        files: Vec<(String, String)>,
        got: Vec<Option<Arc<[u8]>>>,
    },
    #[cfg(not(target_arch = "wasm32"))]
    Building(std::sync::mpsc::Receiver<BaseCatalog>),
    Ready(Arc<BaseCatalog>),
}

/// CPU side of the model view. Lives in the viewport state.
#[derive(Default)]
pub struct ModelRenderer {
    types: HashMap<TypeKey, TypeState>,
    models: Vec<ModelEntry>,
    by_path: HashMap<String, usize>,
    textures: Vec<(String, TexState)>,
    tex_by_path: HashMap<String, usize>,
    next_gpu: u32,
    next_tex: u32,
    /// Fingerprint of the catalogs the type resolutions came from.
    stamp: Option<(usize, usize, usize, u64)>,
    /// Content hash of the map's actor / model entries, at a history revision.
    data_hash: (Option<(u64, usize, usize)>, u64),
    supplement: Supplement,
    /// Mesh uploads of models that became ready, not yet handed to the GPU pass.
    pending: Vec<MeshUpload>,
    /// UI time of the current frame, seconds.
    now: f64,
    logged_at: f64,
    logged: (usize, usize),
    /// Objects drawn as models this frame.
    pub drawn: HashMap<ObjectId, ModelMark>,
    pub stats: ModelStats,
}

/// SC2 player colors are stored sRGB; the shaders work in that space too.
fn srgb(c: [u8; 3]) -> [f32; 3] {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
}

/// CASC folders whose `Base.SC2Data` and `Base.SC2Assets` a map's
/// dependencies add beyond the melee mods, in load order.
///
/// Dependencies are written like `bnet:Co-op Mission/0.0/999,file:Mods/StarCoop/StarCoop.SC2Mod`.
/// The co-op mod builds on the three campaigns' data, which CASC does not
/// list as its own dependencies, so those come first for it.
pub fn dependency_roots(deps: &[String]) -> Vec<String> {
    let melee: Vec<String> =
        strata_core::base_data::MELEE_MODS.iter().map(|m| format!("mods/{}.sc2mod", m.to_ascii_lowercase())).collect();
    let mut out: Vec<String> = Vec::new();
    let mut push = |r: &str| {
        if !melee.iter().any(|m| m == r) && !out.iter().any(|x| x == r) {
            out.push(r.to_owned());
        }
    };
    for d in deps {
        let path = d.rsplit_once("file:").map(|(_, p)| p).unwrap_or(d);
        let path = strata_casc::normalize_path(path.trim());
        if path.is_empty() {
            continue;
        }
        if path == "mods/starcoop/starcoop.sc2mod" {
            for c in ["liberty", "swarm", "void"] {
                push(&format!("campaigns/{c}.sc2campaign"));
            }
        }
        push(&path);
    }
    out
}

impl ModelRenderer {
    /// Resolve, load and lay out this frame's models, and hand the draws to
    /// the GPU pass in `shared`.
    pub fn prepare(&mut self, ctx: &egui::Context, state: &EditorState, view: &View, shared: &SharedHandle) {
        let t0 = web_time_now();
        self.drawn.clear();
        let assets = state.assets.clone();
        if !assets.available() {
            self.stats = ModelStats::default();
            if let Ok(mut s) = shared.lock() {
                s.models.frame.clear();
            }
            return;
        }
        let now = ctx.input(|i| i.time);
        self.now = now;
        self.update_supplement(&assets, &state.doc.info.dependencies);
        self.refresh_types(state);

        // Visible objects, grouped by model.
        let planes = frustum(view.view_proj);
        let mut wants: Vec<(usize, &PlacedObject, f32)> = Vec::new();
        let mut parses = 0usize;
        let mut busy = false;
        let mut lookup: Option<ModelLookup> = None;
        let extra = self.supplement_catalog();
        for o in &state.doc.objects.objects {
            if o.kind == ObjectKind::Point || o.type_id.is_empty() {
                continue;
            }
            let variation =
                o.extra.iter().find(|(k, _)| k == "Variation").and_then(|(_, v)| v.trim().parse().ok()).unwrap_or(0u32);
            let key = TypeKey { unit: o.kind == ObjectKind::Unit, id: o.type_id.clone(), variation };
            if !self.types.contains_key(&key) {
                let l = lookup.get_or_insert_with(|| {
                    let ex: Vec<&BaseCatalog> = extra.iter().map(|c| c.as_ref()).collect();
                    ModelLookup::new(Some(&state.doc.data), &ex, state.base.as_deref())
                });
                let r = if key.unit { l.unit_model(&key.id) } else { l.doodad_model(&key.id) };
                let st = match r {
                    Ok(m) => TypeState::Model(self.model_index(m, variation)),
                    Err(e) => {
                        log::debug!("no model for {}: {e}", key.id);
                        TypeState::Missing
                    }
                };
                self.types.insert(key.clone(), st);
            }
            let Some(TypeState::Model(mi)) = self.types.get(&key) else { continue };
            let mi = *mi;
            // Cull with the model's bounds when known, else a generous sphere.
            let (center, radius) = match &self.models[mi].state {
                ModelState::Ready { bounds, .. } => {
                    let (m, s) = object_matrix(state, o, &self.models[mi].model);
                    let c = Vec3::from(bounds.min).lerp(Vec3::from(bounds.max), 0.5);
                    (m.transform_point3(c), bounds.radius.max(0.25) * s.max_element())
                }
                _ => (object_base(state, o), 4.0 * o.scale.max_element().max(0.1)),
            };
            if !sphere_visible(&planes, center, radius) {
                continue;
            }
            let screen_r = radius * view.points_per_unit(center);
            if !matches!(self.models[mi].state, ModelState::Ready { .. }) {
                busy |= self.advance_model(mi, &assets, now, &mut parses);
                if !matches!(self.models[mi].state, ModelState::Ready { .. }) {
                    continue;
                }
            }
            // Effect-only models (particles, lights) have nothing to draw: keep the marker.
            if screen_r < MIN_SCREEN_RADIUS || matches!(self.models[mi].state, ModelState::Ready { triangles: 0, .. }) {
                continue;
            }
            let depth = (center - view.eye).length();
            wants.push((mi, o, depth));
        }

        // Textures of models that are on screen.
        let mut on_screen: Vec<usize> = wants.iter().map(|w| w.0).collect();
        on_screen.sort_unstable();
        on_screen.dedup();
        let mut tex_uploads = Vec::new();
        for &mi in &on_screen {
            busy |= self.advance_textures(mi, &assets, &mut tex_uploads);
        }

        // Budget: nearest first.
        wants.sort_by(|a, b| a.2.total_cmp(&b.2));
        let mut tris = 0usize;
        let mut kept = 0usize;
        for w in &wants {
            let t = match &self.models[w.0].state {
                ModelState::Ready { triangles, .. } => *triangles,
                _ => 0,
            };
            if kept >= MAX_INSTANCES || tris + t > MAX_TRIANGLES {
                break;
            }
            tris += t;
            kept += 1;
        }
        wants.truncate(kept);

        // Instances.
        let sel: HashSet<ObjectId> = state.selection.objects.iter().copied().collect();
        let accent = crate::render::overlay::MAP.accent;
        let accent = [accent.r() as f32 / 255.0, accent.g() as f32 / 255.0, accent.b() as f32 / 255.0];
        let mut per_model: HashMap<usize, Vec<f32>> = HashMap::new();
        for (mi, o, _) in &wants {
            let entry = &self.models[*mi];
            let ModelState::Ready { bounds, .. } = &entry.state else { continue };
            let (m, s) = object_matrix(state, o, &entry.model);
            let team = if o.kind == ObjectKind::Unit && o.player != 0 {
                state.doc.info.players.iter().find(|p| p.id == o.player).map(|p| srgb(p.color))
            } else {
                None
            };
            let team = match team {
                Some(c) => [c[0], c[1], c[2], 1.0],
                None => [0.0; 4],
            };
            let extra = if sel.contains(&o.id) { [accent[0], accent[1], accent[2], 0.45] } else { [0.0; 4] };
            let v = per_model.entry(*mi).or_default();
            v.extend_from_slice(&m.to_cols_array());
            v.extend_from_slice(&team);
            v.extend_from_slice(&extra);
            let base = object_base(state, o);
            let half = Vec3::from(bounds.max) - Vec3::from(bounds.min);
            let radius = (half.x.max(half.y) * 0.5 * s.x.max(s.y)).max(0.3);
            self.drawn.insert(o.id, ModelMark { anchor: base, radius });
        }

        let mut frame = FrameDraws::default();
        for (mi, inst) in per_model {
            let ModelState::Ready { gpu, subs, .. } = &self.models[mi].state else { continue };
            let count = (inst.len() / INSTANCE_FLOATS) as i32;
            let first = (frame.instances.len() / INSTANCE_FLOATS) as i32;
            frame.instances.extend_from_slice(&inst);
            for s in subs {
                if s.material.kind != strata_m3::MaterialKind::Standard {
                    continue;
                }
                let tex = |slot: &TexSlot| {
                    slot.tex.and_then(|t| match &self.textures[t].1 {
                        TexState::Ready(id) => Some(*id),
                        _ => None,
                    })
                };
                let mat = &s.material;
                let alpha_channel = mat.alpha_mask.as_ref().map(|l| l.channels).unwrap_or(2);
                frame.batches.push(Batch {
                    mesh: *gpu,
                    first_index: s.first,
                    index_count: s.count as i32,
                    first_instance: first,
                    instance_count: count,
                    diffuse: tex(&s.diffuse),
                    emissive: tex(&s.emissive),
                    alpha: tex(&s.alpha),
                    blend: mat.blend,
                    alpha_test: if mat.alpha_test > 0 && s.alpha.path.is_some() {
                        (mat.alpha_test as f32 / 255.0).max(0.02)
                    } else {
                        0.0
                    },
                    alpha_channel,
                    team_color: s.team_color,
                    team_emissive: mat.emissive_blend == 4,
                    emissive_mult: if mat.emissive_mult.is_finite() { mat.emissive_mult.clamp(0.0, 4.0) } else { 1.0 },
                    unshaded: mat.unshaded(),
                    has_diffuse_layer: mat.diffuse.is_some(),
                    diffuse_color: mat.diffuse.as_ref().map(|l| l.color).unwrap_or([255; 4]),
                });
            }
        }
        frame.batches.sort_by_key(|b| (!b.blend.is_opaque(), b.mesh, b.diffuse));

        // Hand over. Keep frames coming while uploads are queued (the pass
        // spreads them over several frames).
        if let Ok(mut s) = shared.lock() {
            let pass = &mut s.models;
            pass.mesh_uploads.append(&mut self.pending);
            pass.texture_uploads.extend(tex_uploads);
            pass.frame = frame;
            busy |= !pass.mesh_uploads.is_empty() || !pass.texture_uploads.is_empty();
        }

        self.stats = ModelStats {
            types: self.types.len(),
            types_ready: self
                .types
                .values()
                .filter(
                    |t| matches!(t, TypeState::Model(i) if matches!(self.models[*i].state, ModelState::Ready { .. })),
                )
                .count(),
            types_missing: self.types.values().filter(|t| matches!(t, TypeState::Missing)).count(),
            instances: kept,
            triangles: tris,
            prepare_ms: ((web_time_now() - t0) * 1000.0) as f32,
        };
        let summary = (self.stats.types_ready, self.stats.instances);
        if now - self.logged_at > 5.0 || (summary != self.logged && !busy) {
            self.logged_at = now;
            self.logged = summary;
            let s = &self.stats;
            log::debug!(
                "models: {} types ({} ready, {} without a model), {} instances, {} triangles, prepared in {:.2} ms",
                s.types,
                s.types_ready,
                s.types_missing,
                s.instances,
                s.triangles,
                s.prepare_ms
            );
        }
        if busy || matches!(self.supplement.state, SupplementState::Fetching { .. }) || self.supplement_building() {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
        }
    }

    // -- type resolution ------------------------------------------------------

    /// Drop cached type resolutions when the catalogs they came from change.
    fn refresh_types(&mut self, state: &EditorState) {
        let base = state.base.as_ref().map(|b| Arc::as_ptr(b) as usize).unwrap_or(0);
        let extra = self.supplement_catalog().map(|c| Arc::as_ptr(&c) as usize).unwrap_or(0);
        let data = &state.doc.data;
        // Hash the map's actor and model entries only after an edit.
        let revision = (state.history.revision(), data.entries.as_ptr() as usize, data.entries.len());
        if self.data_hash.0 != Some(revision) {
            let mut h: u64 = data.entries.len() as u64;
            for e in &data.entries {
                if matches!(
                    e.kind(),
                    strata_core::catalog::CatalogKind::Actor | strata_core::catalog::CatalogKind::Model
                ) {
                    for b in e.node.to_xml().bytes() {
                        h = h.wrapping_mul(0x100000001b3).wrapping_add(b as u64);
                    }
                }
            }
            self.data_hash = (Some(revision), h);
        }
        let stamp = (base, extra, data.entries.len(), self.data_hash.1);
        if self.stamp != Some(stamp) {
            self.stamp = Some(stamp);
            self.types.clear();
        }
    }

    /// The model entry for a resolved type: one per file and model scale.
    fn model_index(&mut self, m: ModelRef, variation: u32) -> usize {
        let mut files = vec![m.variation_path(variation)];
        for f in [m.variation_path(0), m.path.clone()] {
            if !files.contains(&f) {
                files.push(f);
            }
        }
        let key = format!("{}#{:?}", strata_casc::normalize_path(&files[0]), m.scale);
        if let Some(&i) = self.by_path.get(&key) {
            return i;
        }
        let candidates = self.model_candidates(&files);
        self.models.push(ModelEntry { model: m, files, state: ModelState::Locating { candidates, next: 0 } });
        let i = self.models.len() - 1;
        self.by_path.insert(key, i);
        i
    }

    fn model_candidates(&self, files: &[String]) -> Vec<String> {
        files.iter().flat_map(|f| self.candidates(f)).collect()
    }

    /// CASC paths to try for a data path: dependency roots (last loaded
    /// first), then the melee mods through the asset service.
    fn candidates(&self, data_path: &str) -> Vec<String> {
        let rel = strata_casc::normalize_path(data_path);
        let mut out: Vec<String> =
            self.supplement.roots.iter().rev().map(|r| format!("{r}/base.sc2assets/{rel}")).collect();
        out.push(format!("@resolve:{data_path}"));
        out
    }

    // -- loading --------------------------------------------------------------

    /// Move a model along its loading states. Returns true while it is
    /// still in progress (so the caller keeps repainting).
    fn advance_model(&mut self, mi: usize, assets: &Assets, now: f64, parses: &mut usize) -> bool {
        loop {
            match &self.models[mi].state {
                ModelState::Ready { .. } => return false,
                ModelState::Failed { at, .. } => {
                    if now - *at < RETRY_SECONDS {
                        return false;
                    }
                    let candidates = self.model_candidates(&self.models[mi].files);
                    self.models[mi].state = ModelState::Locating { candidates, next: 0 };
                    // One attempt per retry period.
                    let r = self.locate_model(mi, assets, now);
                    if let ModelState::Failed { why, .. } = &self.models[mi].state {
                        log::debug!("{}: {why}", self.models[mi].model.path);
                    }
                    return r;
                }
                ModelState::Locating { .. } => {
                    if self.locate_model(mi, assets, now) {
                        return true;
                    }
                    if !matches!(self.models[mi].state, ModelState::Loading { .. }) {
                        return false;
                    }
                }
                ModelState::Loading { path } => {
                    let path = path.clone();
                    let bytes = match assets.file(&path) {
                        Asset::Loading => return true,
                        Asset::Missing => {
                            self.models[mi].state =
                                ModelState::Failed { at: now, why: format!("{path} could not be read") };
                            return false;
                        }
                        Asset::Ready(b) => b,
                    };
                    if *parses >= PARSES_PER_FRAME {
                        return true;
                    }
                    *parses += 1;
                    match strata_m3::parse(&bytes) {
                        Ok(mesh) => {
                            log::debug!("model {path}: {} triangles", mesh.triangle_count());
                            let gpu = self.next_gpu;
                            self.next_gpu += 1;
                            let ready = self.ready_model(gpu, mesh);
                            self.models[mi].state = ready;
                        }
                        Err(e) => {
                            log::warn!("{path}: {e}");
                            self.models[mi].state = ModelState::Failed { at: f64::INFINITY, why: e.to_string() };
                        }
                    }
                    return false;
                }
            }
        }
    }

    /// Walk the candidate paths of a locating model until one exists.
    /// Returns true while the asset service is still answering.
    fn locate_model(&mut self, mi: usize, assets: &Assets, now: f64) -> bool {
        loop {
            let ModelState::Locating { candidates, next } = &mut self.models[mi].state else { return false };
            let Some(c) = candidates.get(*next).cloned() else {
                self.models[mi].state = ModelState::Failed { at: now, why: "file not found".into() };
                return false;
            };
            let path = match c.strip_prefix("@resolve:") {
                Some(p) => match assets.resolve(p) {
                    Some(r) => r,
                    None => {
                        *next += 1;
                        continue;
                    }
                },
                None => c,
            };
            match assets.file(&path) {
                Asset::Missing => *next += 1,
                Asset::Loading => {
                    self.models[mi].state = ModelState::Loading { path };
                    return true;
                }
                Asset::Ready(_) => {
                    self.models[mi].state = ModelState::Loading { path };
                    return false;
                }
            }
        }
    }

    /// Queue the mesh upload and build the submesh table.
    fn ready_model(&mut self, gpu: u32, mesh: strata_m3::Mesh) -> ModelState {
        let mut vertices = Vec::with_capacity(mesh.positions.len() * 8);
        for i in 0..mesh.positions.len() {
            vertices.extend_from_slice(&mesh.positions[i]);
            vertices.extend_from_slice(&mesh.normals[i]);
            vertices.extend_from_slice(&mesh.uvs[i]);
        }
        let mut subs = Vec::new();
        let mut triangles = 0;
        for s in mesh.submeshes {
            if s.material.kind != strata_m3::MaterialKind::Standard {
                continue;
            }
            let path_of = |l: &Option<strata_m3::Layer>| l.as_ref().and_then(|l| l.path.clone());
            let diffuse = path_of(&s.material.diffuse);
            let alpha = path_of(&s.material.alpha_mask);
            // Team color lives in the diffuse alpha when the layer reads RGBA
            // (an RGB-only layer leaves alpha unused), unless that alpha is
            // the material's cutout mask.
            let team_color = s.material.blend.is_opaque()
                && s.material.diffuse.as_ref().is_some_and(|l| l.channels == 1)
                && diffuse.is_some()
                && alpha.as_ref().is_none_or(|a| !a.eq_ignore_ascii_case(diffuse.as_deref().unwrap_or("")));
            triangles += s.index_count as usize / 3;
            subs.push(Sub {
                first: s.first_index,
                count: s.index_count,
                diffuse: TexSlot { path: diffuse, tex: None },
                emissive: TexSlot { path: path_of(&s.material.emissive), tex: None },
                alpha: TexSlot { path: alpha, tex: None },
                team_color,
                material: s.material,
            });
        }
        self.pending.push(MeshUpload { id: gpu, vertices, indices: mesh.indices });
        ModelState::Ready { gpu, bounds: mesh.bounds, subs, triangles }
    }

    /// Request and track the textures of model `mi`. Returns true while any
    /// are still loading.
    fn advance_textures(&mut self, mi: usize, assets: &Assets, uploads: &mut Vec<TextureUpload>) -> bool {
        // Collect slot paths first to avoid borrowing `self.models` across the texture table.
        let mut wanted: Vec<(usize, usize, String)> = Vec::new();
        if let ModelState::Ready { subs, .. } = &self.models[mi].state {
            for (si, s) in subs.iter().enumerate() {
                for (k, slot) in [&s.diffuse, &s.emissive, &s.alpha].into_iter().enumerate() {
                    if slot.tex.is_none() {
                        if let Some(p) = &slot.path {
                            wanted.push((si, k, p.clone()));
                        }
                    }
                }
            }
        }
        for (si, k, path) in wanted {
            let t = self.texture_index(&path);
            if let ModelState::Ready { subs, .. } = &mut self.models[mi].state {
                let s = &mut subs[si];
                let slot = match k {
                    0 => &mut s.diffuse,
                    1 => &mut s.emissive,
                    _ => &mut s.alpha,
                };
                slot.tex = Some(t);
            }
        }
        let mut busy = false;
        let mut used: Vec<usize> = Vec::new();
        if let ModelState::Ready { subs, .. } = &self.models[mi].state {
            for s in subs {
                used.extend([s.diffuse.tex, s.emissive.tex, s.alpha.tex].into_iter().flatten());
            }
        }
        used.sort_unstable();
        used.dedup();
        for t in used {
            busy |= self.advance_texture(t, assets, uploads);
        }
        busy
    }

    fn texture_index(&mut self, data_path: &str) -> usize {
        let key = strata_casc::normalize_path(data_path);
        if let Some(&i) = self.tex_by_path.get(&key) {
            return i;
        }
        let candidates = self.candidates(data_path);
        self.textures.push((key.clone(), TexState::Locating { candidates, next: 0 }));
        let i = self.textures.len() - 1;
        self.tex_by_path.insert(key, i);
        i
    }

    fn advance_texture(&mut self, t: usize, assets: &Assets, uploads: &mut Vec<TextureUpload>) -> bool {
        if let TexState::Failed(at) = self.textures[t].1 {
            if self.now - at < RETRY_SECONDS {
                return false;
            }
            let candidates = self.candidates(&self.textures[t].0);
            self.textures[t].1 = TexState::Locating { candidates, next: 0 };
        }
        loop {
            let now = self.now;
            let (_, st) = &mut self.textures[t];
            let TexState::Locating { candidates, next } = st else { return false };
            let Some(c) = candidates.get(*next).cloned() else {
                *st = TexState::Failed(now);
                return false;
            };
            let path = match c.strip_prefix("@resolve:") {
                Some(p) => match assets.resolve(p) {
                    Some(r) => r,
                    None => {
                        *next += 1;
                        continue;
                    }
                },
                None => c,
            };
            match assets.texture(&path, TEXTURE_SIDE) {
                Asset::Missing => *next += 1,
                Asset::Loading => return true,
                Asset::Ready(img) => {
                    let id = self.next_tex;
                    self.next_tex += 1;
                    uploads.push(TextureUpload { id, image: img });
                    self.textures[t].1 = TexState::Ready(id);
                    return false;
                }
            }
        }
    }

    // -- dependency data --------------------------------------------------------

    fn supplement_catalog(&self) -> Option<Arc<BaseCatalog>> {
        match &self.supplement.state {
            SupplementState::Ready(c) => Some(c.clone()),
            _ => None,
        }
    }

    fn supplement_building(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if matches!(self.supplement.state, SupplementState::Building(_)) {
            return true;
        }
        false
    }

    fn update_supplement(&mut self, assets: &Assets, deps: &[String]) {
        let sup = &mut self.supplement;
        if sup.key != deps {
            sup.key = deps.to_vec();
            sup.roots = dependency_roots(deps);
            let files: Vec<(String, String)> = sup
                .roots
                .iter()
                .flat_map(|r| {
                    ["actordata.xml", "modeldata.xml"].map(|f| (r.clone(), format!("{r}/base.sc2data/gamedata/{f}")))
                })
                .collect();
            sup.state = if files.is_empty() {
                SupplementState::Idle
            } else {
                SupplementState::Fetching { got: vec![None; files.len()], files }
            };
            // Loaded models and textures stay; anything not found yet is
            // looked up again under the new roots.
            self.types.clear();
            for i in 0..self.models.len() {
                if !matches!(self.models[i].state, ModelState::Ready { .. }) {
                    let candidates = self.model_candidates(&self.models[i].files);
                    self.models[i].state = ModelState::Locating { candidates, next: 0 };
                }
            }
            for i in 0..self.textures.len() {
                if !matches!(self.textures[i].1, TexState::Ready(_)) {
                    let candidates = self.candidates(&self.textures[i].0);
                    self.textures[i].1 = TexState::Locating { candidates, next: 0 };
                }
            }
            return;
        }
        match &mut sup.state {
            SupplementState::Fetching { files, got } => {
                let mut pending = false;
                for (i, (_, path)) in files.iter().enumerate() {
                    if got[i].is_some() {
                        continue;
                    }
                    match assets.file(path) {
                        Asset::Loading => pending = true,
                        Asset::Ready(b) => got[i] = Some(b),
                        Asset::Missing => got[i] = Some(Arc::from(Vec::new().into_boxed_slice())),
                    }
                }
                if pending {
                    return;
                }
                let inputs: Vec<(String, String, Arc<[u8]>)> = files
                    .iter()
                    .zip(got.iter())
                    .filter_map(|((root, path), b)| b.clone().map(|b| (root.clone(), path.clone(), b)))
                    .collect();
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(build_supplement(&inputs));
                    });
                    sup.state = SupplementState::Building(rx);
                }
                #[cfg(target_arch = "wasm32")]
                {
                    sup.state = SupplementState::Ready(Arc::new(build_supplement(&inputs)));
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            SupplementState::Building(rx) => {
                if let Ok(c) = rx.try_recv() {
                    log::info!("model data from map dependencies: {} entries", c.len());
                    sup.state = SupplementState::Ready(Arc::new(c));
                }
            }
            _ => {}
        }
    }
}

/// Merge the dependencies' actor and model catalogs in load order.
fn build_supplement(inputs: &[(String, String, Arc<[u8]>)]) -> BaseCatalog {
    let mut b = BaseCatalog::builder("dependencies");
    let mut current = String::new();
    for (root, path, bytes) in inputs {
        if *root != current {
            b.begin_mod(root);
            current = root.clone();
        }
        if !bytes.is_empty() {
            if let Err(e) = b.add_catalog(path, bytes) {
                log::warn!("{e}");
            }
        }
    }
    b.finish()
}

#[cfg(not(target_arch = "wasm32"))]
fn web_time_now() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

#[cfg(target_arch = "wasm32")]
fn web_time_now() -> f64 {
    web_sys::window().and_then(|w| w.performance()).map(|p| p.now() / 1000.0).unwrap_or(0.0)
}

/// Ground point under an object: units stand on the terrain (their stored
/// height is an offset above it), doodads keep their absolute height.
pub fn object_base(state: &EditorState, o: &PlacedObject) -> Vec3 {
    let t = &state.doc.terrain;
    let ground = t.sample_height(o.position.x, o.position.y);
    let z = match o.kind {
        ObjectKind::Unit => ground.max(o.position.z),
        _ => o.position.z,
    };
    Vec3::new(o.position.x, o.position.y, z)
}

/// Model-to-world transform and its scale.
fn object_matrix(state: &EditorState, o: &PlacedObject, m: &ModelRef) -> (Mat4, Vec3) {
    let s = o.scale.abs().max(Vec3::splat(0.01)) * Vec3::from(m.scale);
    // Models face -Y in their own space; a facing of 0 points east (+X).
    let facing = if o.rotation.is_finite() { o.rotation } else { 0.0 };
    let rot = Quat::from_rotation_z(facing + std::f32::consts::FRAC_PI_2);
    (Mat4::from_scale_rotation_translation(s, rot, object_base(state, o)), s)
}

/// Clip planes (a, b, c, d) with inside where `dot(abc, p) + d >= 0`.
fn frustum(vp: Mat4) -> [Vec4; 6] {
    let r = [vp.row(0), vp.row(1), vp.row(2), vp.row(3)];
    [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[3] + r[2], r[3] - r[2]]
}

fn sphere_visible(planes: &[Vec4; 6], c: Vec3, r: f32) -> bool {
    planes.iter().all(|p| p.truncate().dot(c) + p.w >= -r * p.truncate().length())
}

// ---------------------------------------------------------------------------
// GPU side

/// Floats per instance: model matrix (16), team color (4), highlight (4).
const INSTANCE_FLOATS: usize = 24;

struct MeshUpload {
    id: u32,
    vertices: Vec<f32>,
    indices: Vec<u32>,
}

struct TextureUpload {
    id: u32,
    image: Arc<strata_dds::Image>,
}

struct Batch {
    mesh: u32,
    first_index: u32,
    index_count: i32,
    first_instance: i32,
    instance_count: i32,
    diffuse: Option<u32>,
    emissive: Option<u32>,
    alpha: Option<u32>,
    blend: strata_m3::Blend,
    alpha_test: f32,
    alpha_channel: u32,
    team_color: bool,
    team_emissive: bool,
    emissive_mult: f32,
    unshaded: bool,
    has_diffuse_layer: bool,
    diffuse_color: [u8; 4],
}

#[derive(Default)]
struct FrameDraws {
    instances: Vec<f32>,
    batches: Vec<Batch>,
}

impl FrameDraws {
    fn clear(&mut self) {
        self.instances.clear();
        self.batches.clear();
    }
}

struct GpuMesh {
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ibo: glow::Buffer,
}

struct Programs {
    program: glow::Program,
    u_view_proj: Option<glow::UniformLocation>,
    u_sun: Option<glow::UniformLocation>,
    u_eye: Option<glow::UniformLocation>,
    u_fog: Option<glow::UniformLocation>,
    u_mat: Option<glow::UniformLocation>,
    u_mat2: Option<glow::UniformLocation>,
    u_alpha_ch: Option<glow::UniformLocation>,
    u_base: Option<glow::UniformLocation>,
    u_diffuse: Option<glow::UniformLocation>,
    u_emissive: Option<glow::UniformLocation>,
    u_alpha: Option<glow::UniformLocation>,
    white: glow::Texture,
    instance_vbo: glow::Buffer,
    instance_bytes: usize,
}

/// GPU state of the model view, owned by the terrain renderer's shared
/// state and painted from its callback.
#[derive(Default)]
pub struct ModelPass {
    programs: Option<Programs>,
    failed: bool,
    meshes: HashMap<u32, GpuMesh>,
    textures: HashMap<u32, glow::Texture>,
    mesh_uploads: Vec<MeshUpload>,
    texture_uploads: Vec<TextureUpload>,
    frame: FrameDraws,
}

/// Uploads done per paint at most, so a burst of new models spreads over frames.
const UPLOADS_PER_FRAME: usize = 12;

const MODEL_VS: &str = r#"
in vec3 a_pos;
in vec3 a_normal;
in vec2 a_uv;
in vec4 i_m0;
in vec4 i_m1;
in vec4 i_m2;
in vec4 i_m3;
in vec4 i_team;
in vec4 i_extra;
uniform mat4 u_view_proj;
out vec3 v_world;
out vec3 v_normal;
out vec2 v_uv;
out vec4 v_team;
out vec4 v_extra;
void main() {
    mat4 m = mat4(i_m0, i_m1, i_m2, i_m3);
    vec4 w = m * vec4(a_pos, 1.0);
    v_world = w.xyz;
    // Scales are near uniform, so the upper 3x3 is a fine normal matrix.
    v_normal = mat3(m) * a_normal;
    v_uv = a_uv;
    v_team = i_team;
    v_extra = i_extra;
    gl_Position = u_view_proj * w;
}
"#;

const MODEL_FS: &str = r#"
in vec3 v_world;
in vec3 v_normal;
in vec2 v_uv;
in vec4 v_team;
in vec4 v_extra;
uniform sampler2D u_diffuse;
uniform sampler2D u_emissive;
uniform sampler2D u_alpha;
uniform vec3 u_sun;
uniform vec3 u_eye;
uniform vec4 u_fog;
// x: emissive texture, y: alpha-test threshold (0 = none), z: alpha texture, w: unshaded
uniform vec4 u_mat;
// x: emissive multiplier, y: team-color emissive, z: blend (0 opaque, 1 alpha, 2 add, 3 alpha add, 4 mod), w: team-color diffuse
uniform vec4 u_mat2;
// Alpha-mask channel selector (one-hot RGBA).
uniform vec4 u_alpha_ch;
// Base color when there is no diffuse texture.
uniform vec4 u_base;
out vec4 o_color;
void main() {
    vec4 d = texture(u_diffuse, v_uv) * u_base;
    vec3 base = d.rgb;
    if (u_mat2.w > 0.5 && v_team.a > 0.5) {
        // Diffuse alpha masks team color: 0 is fully the owner's color.
        base = mix(v_team.rgb, d.rgb, d.a);
    }
    float alpha = 1.0;
    if (u_mat.z > 0.5) {
        alpha = dot(texture(u_alpha, v_uv), u_alpha_ch);
    } else if (u_mat2.z > 0.5) {
        alpha = d.a;
    }
    if (u_mat.y > 0.0 && alpha < u_mat.y) {
        discard;
    }

    vec3 n = normalize(v_normal);
    if (!gl_FrontFacing) {
        n = -n;
    }
    vec3 lit;
    if (u_mat.w > 0.5) {
        lit = base;
    } else {
        // Same light rig as the terrain: warm key from the upper left,
        // cool sky / warm ground fill.
        float hemi = n.z * 0.5 + 0.5;
        vec3 ambient = mix(vec3(0.30, 0.25, 0.21), vec3(0.47, 0.52, 0.61), hemi);
        float diff = clamp((dot(n, u_sun) + 0.1) / 1.1, 0.0, 1.0);
        lit = base * (ambient * 1.15 + vec3(1.0, 0.94, 0.84) * diff * 0.95);
        vec3 v = normalize(u_eye - v_world);
        vec3 h = normalize(v + u_sun);
        lit += vec3(0.12) * pow(max(dot(n, h), 0.0), 24.0);
    }
    if (u_mat.x > 0.5) {
        vec4 e = texture(u_emissive, v_uv);
        vec3 em = u_mat2.y > 0.5 ? e.a * max(v_team.rgb, vec3(0.25)) : e.rgb;
        lit += em * u_mat2.x;
    }
    if (v_extra.a > 0.0) {
        // Selection: a cyan rim plus a light wash so it reads at any size.
        vec3 v = normalize(u_eye - v_world);
        float rim = pow(1.0 - abs(dot(n, v)), 2.0);
        lit = mix(lit, v_extra.rgb, v_extra.a * (0.35 + 0.65 * rim));
    }
    float dist = length(u_eye - v_world);
    float fog = clamp((dist - u_fog.a) / (u_fog.a * 2.0 + 1.0), 0.0, 0.45);
    if (u_mat2.z < 1.5) {
        lit = mix(lit, u_fog.rgb, fog);
    } else {
        lit *= 1.0 - fog;
    }
    o_color = vec4(lit, alpha);
}
"#;

impl ModelPass {
    unsafe fn init(&mut self, gl: &glow::Context) -> Result<(), String> {
        unsafe {
            let hdr = super::gl::header(gl)?;
            let program = super::gl::compile(
                gl,
                hdr,
                MODEL_VS,
                MODEL_FS,
                &["a_pos", "a_normal", "a_uv", "i_m0", "i_m1", "i_m2", "i_m3", "i_team", "i_extra"],
            )?;
            let u = |n: &str| gl.get_uniform_location(program, n);
            let white = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(white));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                1,
                1,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&[255, 255, 255, 255])),
            );
            for (p, v) in [(glow::TEXTURE_MIN_FILTER, glow::NEAREST), (glow::TEXTURE_MAG_FILTER, glow::NEAREST)] {
                gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
            }
            gl.bind_texture(glow::TEXTURE_2D, None);
            let instance_vbo = gl.create_buffer()?;
            self.programs = Some(Programs {
                u_view_proj: u("u_view_proj"),
                u_sun: u("u_sun"),
                u_eye: u("u_eye"),
                u_fog: u("u_fog"),
                u_mat: u("u_mat"),
                u_mat2: u("u_mat2"),
                u_alpha_ch: u("u_alpha_ch"),
                u_base: u("u_base"),
                u_diffuse: u("u_diffuse"),
                u_emissive: u("u_emissive"),
                u_alpha: u("u_alpha"),
                program,
                white,
                instance_vbo,
                instance_bytes: 0,
            });
            Ok(())
        }
    }

    unsafe fn upload(&mut self, gl: &glow::Context) {
        unsafe {
            let mut budget = UPLOADS_PER_FRAME;
            while budget > 0 {
                let Some(m) = self.mesh_uploads.pop() else { break };
                budget -= 1;
                let Ok(vao) = gl.create_vertex_array() else { continue };
                let (Ok(vbo), Ok(ibo)) = (gl.create_buffer(), gl.create_buffer()) else { continue };
                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytemuck::cast_slice(&m.vertices), glow::STATIC_DRAW);
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
                gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    bytemuck::cast_slice(&m.indices),
                    glow::STATIC_DRAW,
                );
                let stride = 32;
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
                gl.enable_vertex_attrib_array(1);
                gl.vertex_attrib_pointer_f32(1, 3, glow::FLOAT, false, stride, 12);
                gl.enable_vertex_attrib_array(2);
                gl.vertex_attrib_pointer_f32(2, 2, glow::FLOAT, false, stride, 24);
                for a in 3..=8 {
                    gl.enable_vertex_attrib_array(a);
                    gl.vertex_attrib_divisor(a, 1);
                }
                gl.bind_vertex_array(None);
                gl.bind_buffer(glow::ARRAY_BUFFER, None);
                if let Some(old) = self.meshes.insert(m.id, GpuMesh { vao, vbo, ibo }) {
                    gl.delete_vertex_array(old.vao);
                    gl.delete_buffer(old.vbo);
                    gl.delete_buffer(old.ibo);
                }
            }
            while budget > 0 {
                let Some(t) = self.texture_uploads.pop() else { break };
                budget -= 1;
                let img = &t.image;
                if img.width == 0 || img.height == 0 || img.rgba.len() < (img.width * img.height * 4) as usize {
                    continue;
                }
                let Ok(tex) = gl.create_texture() else { continue };
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    img.width as i32,
                    img.height as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&img.rgba)),
                );
                gl.generate_mipmap(glow::TEXTURE_2D);
                for (p, v) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::REPEAT),
                    (glow::TEXTURE_WRAP_T, glow::REPEAT),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, p, v as i32);
                }
                gl.bind_texture(glow::TEXTURE_2D, None);
                if let Some(old) = self.textures.insert(t.id, tex) {
                    gl.delete_texture(old);
                }
            }
        }
    }

    /// Draw this frame's models into the currently bound (depth-tested)
    /// target. Leaves the terrain pass's GL state as it found it.
    ///
    /// # Safety
    /// Must run on the GL thread inside the paint callback.
    pub unsafe fn paint(&mut self, gl: &glow::Context, p: &FrameParams) {
        unsafe {
            if self.failed {
                return;
            }
            if self.programs.is_none() {
                if let Err(e) = self.init(gl) {
                    log::error!("model renderer: {e}");
                    self.failed = true;
                    return;
                }
            }
            self.upload(gl);
            if self.frame.batches.is_empty() {
                return;
            }
            let Some(pr) = self.programs.as_mut() else { return };

            // Instance data for the whole frame.
            let bytes: &[u8] = bytemuck::cast_slice(&self.frame.instances);
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(pr.instance_vbo));
            if bytes.len() > pr.instance_bytes {
                let size = bytes.len().next_power_of_two();
                gl.buffer_data_size(glow::ARRAY_BUFFER, size as i32, glow::DYNAMIC_DRAW);
                pr.instance_bytes = size;
            }
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes);

            gl.use_program(Some(pr.program));
            gl.uniform_matrix_4_f32_slice(pr.u_view_proj.as_ref(), false, &p.view_proj.to_cols_array());
            let sun = super::gl::SUN;
            gl.uniform_3_f32(pr.u_sun.as_ref(), sun.x, sun.y, sun.z);
            gl.uniform_3_f32(pr.u_eye.as_ref(), p.eye.x, p.eye.y, p.eye.z);
            let fog_start = p.map_size[0].max(p.map_size[1]) * 1.2;
            gl.uniform_4_f32(pr.u_fog.as_ref(), p.clear[0], p.clear[1], p.clear[2], fog_start);
            gl.uniform_1_i32(pr.u_diffuse.as_ref(), 0);
            gl.uniform_1_i32(pr.u_emissive.as_ref(), 1);
            gl.uniform_1_i32(pr.u_alpha.as_ref(), 2);
            gl.disable(glow::CULL_FACE);

            let stride = (INSTANCE_FLOATS * 4) as i32;
            let mut bound_mesh = None;
            let mut blending = false;
            for b in &self.frame.batches {
                let Some(mesh) = self.meshes.get(&b.mesh) else { continue };
                if bound_mesh != Some(b.mesh) {
                    gl.bind_vertex_array(Some(mesh.vao));
                    bound_mesh = Some(b.mesh);
                }
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(pr.instance_vbo));
                let base = b.first_instance * stride;
                for k in 0..6 {
                    gl.vertex_attrib_pointer_f32(3 + k as u32, 4, glow::FLOAT, false, stride, base + k * 16);
                }
                let blend_code = match b.blend {
                    strata_m3::Blend::Opaque => 0.0,
                    strata_m3::Blend::AlphaBlend => 1.0,
                    strata_m3::Blend::Add => 2.0,
                    strata_m3::Blend::AlphaAdd => 3.0,
                    _ => 4.0,
                };
                if !b.blend.is_opaque() && !blending {
                    blending = true;
                    gl.enable(glow::BLEND);
                    gl.depth_mask(false);
                }
                match b.blend {
                    strata_m3::Blend::Opaque => {}
                    strata_m3::Blend::AlphaBlend => gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA),
                    strata_m3::Blend::Add => gl.blend_func(glow::ONE, glow::ONE),
                    strata_m3::Blend::AlphaAdd => gl.blend_func(glow::SRC_ALPHA, glow::ONE),
                    strata_m3::Blend::Mod => gl.blend_func(glow::DST_COLOR, glow::ZERO),
                    _ => gl.blend_func(glow::DST_COLOR, glow::SRC_COLOR),
                }
                let tex = |t: Option<u32>| t.and_then(|t| self.textures.get(&t).copied());
                let diffuse = tex(b.diffuse);
                let emissive = tex(b.emissive);
                let alpha = tex(b.alpha);
                gl.active_texture(glow::TEXTURE0);
                gl.bind_texture(glow::TEXTURE_2D, Some(diffuse.unwrap_or(pr.white)));
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(emissive.unwrap_or(pr.white)));
                gl.active_texture(glow::TEXTURE2);
                gl.bind_texture(glow::TEXTURE_2D, Some(alpha.unwrap_or(pr.white)));
                // Until its texture arrives (or without one) a surface is a neutral grey.
                let c = if diffuse.is_some() {
                    [1.0; 4]
                } else if b.has_diffuse_layer && b.diffuse.is_none() {
                    let d = b.diffuse_color;
                    [d[0] as f32 / 255.0, d[1] as f32 / 255.0, d[2] as f32 / 255.0, 1.0]
                } else {
                    [0.62, 0.62, 0.64, 1.0]
                };
                gl.uniform_4_f32(pr.u_base.as_ref(), c[0], c[1], c[2], c[3]);
                let alpha_test = if alpha.is_some() { b.alpha_test } else { 0.0 };
                gl.uniform_4_f32(
                    pr.u_mat.as_ref(),
                    if emissive.is_some() { 1.0 } else { 0.0 },
                    alpha_test,
                    if alpha.is_some() { 1.0 } else { 0.0 },
                    if b.unshaded { 1.0 } else { 0.0 },
                );
                gl.uniform_4_f32(
                    pr.u_mat2.as_ref(),
                    b.emissive_mult,
                    if b.team_emissive { 1.0 } else { 0.0 },
                    blend_code,
                    if b.team_color && diffuse.is_some() { 1.0 } else { 0.0 },
                );
                let ch = match b.alpha_channel {
                    3 => [1.0, 0.0, 0.0, 0.0],
                    4 => [0.0, 1.0, 0.0, 0.0],
                    5 => [0.0, 0.0, 1.0, 0.0],
                    _ => [0.0, 0.0, 0.0, 1.0],
                };
                gl.uniform_4_f32(pr.u_alpha_ch.as_ref(), ch[0], ch[1], ch[2], ch[3]);
                gl.draw_elements_instanced(
                    glow::TRIANGLES,
                    b.index_count,
                    glow::UNSIGNED_INT,
                    (b.first_index * 4) as i32,
                    b.instance_count,
                );
            }

            // Restore what the terrain pass expects.
            gl.bind_vertex_array(None);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            for unit in [glow::TEXTURE2, glow::TEXTURE1, glow::TEXTURE0] {
                gl.active_texture(unit);
                gl.bind_texture(glow::TEXTURE_2D, None);
            }
            gl.disable(glow::BLEND);
            gl.depth_mask(true);
            gl.enable(glow::CULL_FACE);
            gl.cull_face(glow::BACK);
            gl.front_face(glow::CCW);
            gl.use_program(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dependency_roots_skip_melee_and_expand_coop() {
        let deps = [
            "bnet:Void Multi (Mod)/0.0/999,file:Mods/VoidMulti.SC2Mod",
            "bnet:Nova Covert Ops (Art Mod)/0.0/999,file:Mods/NovaStoryAssets.SC2Mod",
            "bnet:Co-op Mission/0.0/999,file:Mods/StarCoop/StarCoop.SC2Mod",
            "file:Campaigns/Void.SC2Campaign",
        ]
        .map(String::from);
        assert_eq!(
            dependency_roots(&deps),
            [
                "mods/novastoryassets.sc2mod",
                "campaigns/liberty.sc2campaign",
                "campaigns/swarm.sc2campaign",
                "campaigns/void.sc2campaign",
                "mods/starcoop/starcoop.sc2mod",
            ]
        );
        assert!(dependency_roots(&["file:Mods/Core.SC2Mod".to_owned()]).is_empty());
    }

    #[test]
    fn frustum_culls_spheres() {
        let view = Mat4::perspective_rh_gl(1.0, 1.0, 0.1, 100.0) * Mat4::look_at_rh(Vec3::ZERO, Vec3::X, Vec3::Z);
        let p = frustum(view);
        assert!(sphere_visible(&p, Vec3::new(10.0, 0.0, 0.0), 1.0));
        assert!(!sphere_visible(&p, Vec3::new(-10.0, 0.0, 0.0), 1.0));
        assert!(sphere_visible(&p, Vec3::new(10.0, 6.0, 0.0), 1.5), "touching the edge counts");
        assert!(!sphere_visible(&p, Vec3::new(200.0, 0.0, 0.0), 1.0), "past the far plane");
    }
}
