//! GPU rendering (glow / WebGL2) for the terrain viewport.
//!
//! - [`camera`]: orbit camera, projection and rays.
//! - [`pick`]: ray vs heightfield.
//! - [`mesh`]: CPU mesh, texture palette and dirty-row tracking.
//! - [`gl`]: shaders, buffers and the egui paint callback.
//! - [`models`]: placed units and doodads as their real `.m3` models.
//! - [`overlay`]: objects, regions and cameras drawn with the egui painter.

pub mod camera;
pub mod gl;
pub mod mesh;
pub mod models;
pub mod overlay;
pub mod pick;

use strata_core::terrain::{Terrain, VertexRect};

use self::gl::{FullUpload, SharedHandle};
use self::mesh::TerrainMesh;

/// Keeps the GPU copy of the terrain in step with the document.
#[derive(Default)]
pub struct TerrainRenderer {
    pub shared: SharedHandle,
    mesh: Option<TerrainMesh>,
    revision: Option<u64>,
}

impl TerrainRenderer {
    /// Queue whatever changed since last frame. `stroke_rect` is the rect a
    /// brush dab touched this frame (fast path, no full diff needed). `force`
    /// diffs against the snapshot even if the revision did not change (cheap
    /// safety net for edits made outside history, e.g. on load).
    pub fn prepare(&mut self, t: &Terrain, revision: u64, stroke_rect: Option<VertexRect>, force: bool) {
        let Ok(mut s) = self.shared.lock() else { return };
        let up = &mut s.uploads;
        let Some(mesh) = self.mesh.as_mut() else {
            let mesh = TerrainMesh::build(t);
            up.full = Some(FullUpload {
                vertices: bytemuck::cast_slice(&mesh.vertices).to_vec(),
                indices: mesh::grid_indices(mesh.verts_x, mesh.verts_y),
            });
            up.rows.clear();
            up.pathing = Some((t.width, t.height, t.pathing.clone()));
            self.mesh = Some(mesh);
            self.revision = Some(revision);
            return;
        };
        let mut rows = None;
        let mut pathing = false;
        let mut resized = false;
        if let Some(r) = stroke_rect {
            rows = mesh.update_rect(t, r);
            pathing = mesh.take_pathing(t);
        }
        if self.revision != Some(revision) || force {
            self.revision = Some(revision);
            let r = mesh.sync(t);
            resized = r.resized;
            pathing |= r.pathing_changed;
            rows = match (rows, r.rows) {
                (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
                (a, b) => a.or(b),
            };
        }
        if resized {
            up.full = Some(FullUpload {
                vertices: bytemuck::cast_slice(&mesh.vertices).to_vec(),
                indices: mesh::grid_indices(mesh.verts_x, mesh.verts_y),
            });
            up.rows.clear();
        } else if let Some((y0, y1)) = rows {
            let (off, bytes) = mesh.row_bytes(y0, y1);
            if let Some(full) = up.full.as_mut() {
                // Not uploaded yet: patch the pending full upload instead.
                full.vertices[off..off + bytes.len()].copy_from_slice(bytes);
            } else {
                up.rows.push((off, bytes.to_vec()));
            }
        }
        if pathing || resized {
            up.pathing = Some((t.width, t.height, t.pathing.clone()));
        }
    }

    pub fn error(&self) -> Option<String> {
        self.shared.lock().ok().and_then(|s| s.error.clone())
    }
}
