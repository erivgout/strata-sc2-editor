//! On-demand access to the user's own StarCraft II game assets (terrain
//! textures, `.m3` models and their textures) read from the install's CASC
//! storage at runtime. Strata never ships Blizzard files.
//!
//! Contract (shared by the terrain and model renderers):
//! - Every getter is non-blocking. The first call for a path queues a
//!   background load and returns [`Asset::Loading`]; later calls return
//!   [`Asset::Ready`] or [`Asset::Missing`]. Results are cached.
//! - When something finishes loading, the service requests an egui repaint,
//!   so callers simply ask again next frame.
//! - Paths are CASC paths with `/` separators, matched case-insensitively,
//!   e.g. `mods/core.sc2mod/base.sc2data/assets/textures/foo.dds`
//!   (see `resolve` for mod-relative lookups like `Assets\Textures\foo.dds`).
//!
//! The backend (native thread + web File System Access source, keeping the
//! CASC storage open with a filter that includes `.dds` and `.m3`) is
//! implemented by the terrain-texture work; until then everything is Missing.

use std::sync::Arc;

/// State of one requested asset.
#[derive(Debug, Clone)]
pub enum Asset<T> {
    Loading,
    Ready(T),
    Missing,
}

impl<T> Asset<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Asset::Ready(t) => Some(t),
            _ => None,
        }
    }
}

#[derive(Default)]
pub struct Assets {}

impl Assets {
    /// Whether a game install is attached and assets can load at all.
    pub fn available(&self) -> bool {
        false
    }

    /// Raw bytes of a game file.
    pub fn file(&self, _path: &str) -> Asset<Arc<[u8]>> {
        Asset::Missing
    }

    /// A decoded DDS texture, using the largest mip whose longer side is at
    /// most `max_side` pixels.
    pub fn texture(&self, _path: &str, _max_side: u32) -> Asset<Arc<strata_dds::Image>> {
        Asset::Missing
    }

    /// Resolve a path as written in game data (e.g. `Assets\Units\Terran\Marine\Marine.m3`)
    /// to a CASC path by searching the dependency mods (Core, Liberty, Swarm,
    /// Void and their Multi variants, last mod wins). `None` if not found.
    pub fn resolve(&self, _data_path: &str) -> Option<String> {
        None
    }
}
