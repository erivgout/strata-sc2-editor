//! Unit icons for pickers: the button icon a unit shows in the game
//! (`btn-unit-terran-marine.dds`), read from the map's own files or the
//! user's install through [`crate::assets`], decoded small and kept as egui
//! textures. Everything is lazy: callers ask only for rows on screen, and
//! get `None` (draw a placeholder) until the icon has loaded.

use std::collections::HashMap;

use strata_core::base_data::BaseCatalog;
use strata_core::catalog::{Catalog, CatalogKind, FieldKey};

use crate::state::EditorState;

/// Longest side, in pixels, of the decoded icon (rows draw them at ~20 pt).
const ICON_SIDE: u32 = 64;
/// How many parents of a map unit are followed to find an icon.
const MAX_PARENTS: usize = 12;

enum Slot {
    Ready(egui::TextureHandle),
    Missing,
}

#[derive(Default)]
pub struct UnitIcons {
    /// Decoded icons by normalized data path.
    textures: HashMap<String, Slot>,
    /// Icon data path of each unit id, valid for `paths_key`.
    paths: HashMap<String, Option<String>>,
    paths_key: (u64, usize),
}

impl UnitIcons {
    /// The icon of unit type `id`, if it has loaded.
    pub fn icon(&mut self, ctx: &egui::Context, state: &EditorState, id: &str) -> Option<egui::TextureHandle> {
        let key = (state.history.revision(), state.base.as_ref().map(|b| std::sync::Arc::as_ptr(b) as usize).unwrap_or(0));
        if self.paths_key != key {
            self.paths.clear();
            self.paths_key = key;
        }
        let path = self
            .paths
            .entry(id.to_owned())
            .or_insert_with(|| icon_path(&state.doc.data, state.base.as_deref(), id))
            .clone()?;
        self.texture(ctx, state, &path)
    }

    fn texture(&mut self, ctx: &egui::Context, state: &EditorState, path: &str) -> Option<egui::TextureHandle> {
        let norm = strata_core::map_files::norm(path);
        match self.textures.get(&norm) {
            Some(Slot::Ready(t)) => return Some(t.clone()),
            Some(Slot::Missing) => return None,
            None => {}
        }
        // A custom icon packed into the map.
        if let Some(bytes) = state.doc.passthrough.iter().find(|(k, _)| strata_core::map_files::norm(k) == norm).map(|(_, v)| v) {
            let slot = match strata_dds::decode_max_side(bytes, ICON_SIDE) {
                Ok(img) => Slot::Ready(upload(ctx, &norm, &img)),
                Err(_) => Slot::Missing,
            };
            return self.store(norm, slot);
        }
        let assets = &state.assets;
        if !assets.ready() {
            // No install (yet): draw placeholders, try again once it opens.
            return None;
        }
        let Some(casc) = assets.resolve(path) else {
            return self.store(norm, Slot::Missing);
        };
        match assets.texture(&casc, ICON_SIDE) {
            crate::assets::Asset::Loading => None,
            crate::assets::Asset::Missing => self.store(norm, Slot::Missing),
            crate::assets::Asset::Ready(img) => {
                let tex = upload(ctx, &norm, &img);
                self.store(norm, Slot::Ready(tex))
            }
        }
    }

    fn store(&mut self, key: String, slot: Slot) -> Option<egui::TextureHandle> {
        let out = match &slot {
            Slot::Ready(t) => Some(t.clone()),
            Slot::Missing => None,
        };
        self.textures.insert(key, slot);
        out
    }
}

fn upload(ctx: &egui::Context, name: &str, img: &strata_dds::Image) -> egui::TextureHandle {
    let color = egui::ColorImage::from_rgba_unmultiplied([img.width as usize, img.height as usize], &img.rgba);
    ctx.load_texture(format!("unit-icon:{name}"), color, egui::TextureOptions::LINEAR)
}

/// The icon a unit type shows in the game, as written in game data
/// (`Assets\Textures\btn-unit-terran-marine.dds`). The unit's button
/// (`CButton` with the unit's id) is tried first, then its actor's
/// `UnitIcon`. A map unit without either uses its parent's, so a custom
/// Marine copy shows the Marine. The map's data wins over the game's.
pub fn icon_path(map: &Catalog, base: Option<&BaseCatalog>, id: &str) -> Option<String> {
    let mut ids = vec![id.to_owned()];
    while ids.len() <= MAX_PARENTS {
        let last = ids.last().map(String::as_str).unwrap_or_default();
        let parent = map
            .entries
            .iter()
            .find(|e| e.kind() == CatalogKind::Unit && e.id() == last)
            .and_then(|e| e.parent())
            .filter(|p| !p.is_empty() && !ids.iter().any(|i| i == p))
            .map(str::to_owned);
        match parent {
            Some(p) => ids.push(p),
            None => break,
        }
    }
    let map_field = |kind: CatalogKind, id: &str, field: &str| -> Option<String> {
        let i = map.entries.iter().position(|e| e.kind() == kind && e.id() == id)?;
        map.resolve_value(i, &FieldKey::new(field)).map(|(v, _)| v.to_owned())
    };
    let base_field = |kind: CatalogKind, id: &str, field: &str| -> Option<String> {
        base?.field(kind, id, field).map(str::to_owned)
    };
    ids.iter()
        .find_map(|id| {
            map_field(CatalogKind::Button, id, "Icon")
                .or_else(|| map_field(CatalogKind::Actor, id, "UnitIcon"))
                .or_else(|| base_field(CatalogKind::Button, id, "Icon"))
                .or_else(|| base_field(CatalogKind::Actor, id, "UnitIcon"))
        })
        .filter(|p| !p.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::catalog::CatalogEntry;

    fn base() -> BaseCatalog {
        let mut b = BaseCatalog::builder("test");
        b.begin_mod("Liberty");
        b.add_catalog("UnitData.xml", br#"<Catalog><CUnit id="Marine"/><CUnit id="Rock"/></Catalog>"#).unwrap();
        b.add_catalog(
            "ButtonData.xml",
            br#"<Catalog><CButton id="Marine"><Icon value="Assets\Textures\btn-unit-terran-marine.dds"/></CButton></Catalog>"#,
        )
        .unwrap();
        b.add_catalog(
            "ActorData.xml",
            br#"<Catalog><CActorUnit id="Rock"><UnitIcon value="Assets\Textures\btn-rock.dds"/></CActorUnit></Catalog>"#,
        )
        .unwrap();
        b.finish()
    }

    #[test]
    fn button_then_actor_then_parent() {
        let b = base();
        let mut map = Catalog::default();
        assert_eq!(icon_path(&map, Some(&b), "Marine").as_deref(), Some(r"Assets\Textures\btn-unit-terran-marine.dds"));
        assert_eq!(icon_path(&map, Some(&b), "Rock").as_deref(), Some(r"Assets\Textures\btn-rock.dds"));
        assert_eq!(icon_path(&map, Some(&b), "Nothing"), None);
        map.entries.push(CatalogEntry::new("UnitData", "CUnit", "MyMarine", Some("Marine")));
        assert_eq!(icon_path(&map, Some(&b), "MyMarine").as_deref(), Some(r"Assets\Textures\btn-unit-terran-marine.dds"));
        let mut own = CatalogEntry::new("ButtonData", "CButton", "MyMarine", None);
        own.set_field("Icon", r"Assets\Textures\my-marine.dds");
        map.entries.push(own);
        assert_eq!(icon_path(&map, Some(&b), "MyMarine").as_deref(), Some(r"Assets\Textures\my-marine.dds"));
        assert_eq!(icon_path(&map, None, "Marine"), None);
    }
}
