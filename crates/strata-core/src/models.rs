//! Which `.m3` file draws a placed unit or doodad.
//!
//! The game links them through data:
//!
//! - unit `Marine` -> the unit actor whose `unitName` token is `Marine`
//!   (`<CActorUnit id="Marine" parent="GenericUnitBase" unitName="Marine">`)
//! - actor -> its `Model` field (class defaults use `##id##` or
//!   `##unitName##`) -> a `CModel` id
//! - `CModel` -> its `Model` field, usually a template inherited from a
//!   default such as `Assets\##Prefix####Race##\##id##\##id##.m3`, filled
//!   from tokens (`<?token id="Race" value="Terran"?>` declared on parents,
//!   or set as attributes on the entry itself).
//!
//! Doodads are placed by actor or model id: a `CActorDoodad` with the id
//! leads to its model; otherwise the id is taken as a `CModel` id.
//!
//! [`ModelLookup`] layers catalogs: the map's own data first, then any
//! extra catalogs (e.g. a dependency's data), then the base game. An entry
//! found in a higher layer wins; a map entry without a `parent` that shares
//! a base id modifies that base entry and falls through to it.
//!
//! Models with `VariationCount` keep one file per variation
//! ([`ModelRef::variation_path`]).
//!
//! Not handled (the lookup reports a [`Miss`] or the base model): models
//! picked by actor events or terms (`ModelSwap`, `Create` on other units,
//! skins), and data from mods outside the given layers.

use std::collections::HashMap;

use crate::base_data::BaseCatalog;
use crate::catalog::{Catalog, CatalogKind, XmlNode};

/// A resolved model for one placed type.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRef {
    /// Path as written in data, e.g. `Assets\Units\Terran\Marine\Marine.m3`.
    pub path: String,
    /// The `CModel` id, when the path came from one.
    pub model: String,
    /// The actor the model came from, if any.
    pub actor: Option<String>,
    /// The model's own scale (`CModel` `ScaleMax`), multiplied into the
    /// placed object's scale.
    pub scale: [f32; 3],
    /// `VariationCount`: when non-zero the files are `Name_00.m3`,
    /// `Name_01.m3`... next to `path` (see [`ModelRef::variation_path`]).
    pub variations: u32,
}

impl ModelRef {
    /// The file of variation `v` (a placed doodad's `Variation`), or `path`
    /// for models without variations.
    pub fn variation_path(&self, v: u32) -> String {
        if self.variations == 0 {
            return self.path.clone();
        }
        let v = v.min(self.variations - 1);
        let stem = self.path.len() - 3; // ".m3", checked when resolved
        format!("{}_{v:02}{}", &self.path[..stem], &self.path[stem..])
    }
}

/// Why a type has no model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Miss {
    /// No actor and no model with this id in any layer.
    NotInData,
    /// The actor names a model that no layer defines.
    NoModel(String),
    /// The model entry has no `Model` path (or it resolved to nothing).
    NoPath(String),
}

impl std::fmt::Display for Miss {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Miss::NotInData => write!(f, "no actor or model in the loaded data"),
            Miss::NoModel(m) => write!(f, "model {m} is not defined"),
            Miss::NoPath(m) => write!(f, "model {m} has no file"),
        }
    }
}

/// One catalog layer.
#[derive(Clone, Copy)]
enum Layer<'a> {
    Map(&'a Catalog),
    Base(&'a BaseCatalog),
}

/// An entry in some layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Ent {
    layer: usize,
    index: usize,
}

/// Layered model resolution. Building one indexes the map layer and every
/// unit actor; reuse it while the catalogs stay the same.
pub struct ModelLookup<'a> {
    layers: Vec<Layer<'a>>,
    /// Map-layer index: (kind, id) -> entry indices.
    map_index: HashMap<(CatalogKind, &'a str), Vec<usize>>,
    /// unit id -> actor, best first.
    unit_actors: HashMap<String, Ent>,
}

/// Actor classes that stand for a placed unit.
fn is_unit_actor(class: &str) -> bool {
    class.starts_with("CActorUnit") || class == "CActorMissile"
}

fn is_doodad_actor(class: &str) -> bool {
    class.starts_with("CActorDoodad") || class == "CActorModel" || class.starts_with("CActorUnit")
}

impl<'a> ModelLookup<'a> {
    /// Layers in priority order: the map's data, extra catalogs, the base game.
    pub fn new(map: Option<&'a Catalog>, extra: &[&'a BaseCatalog], base: Option<&'a BaseCatalog>) -> Self {
        let mut layers = Vec::new();
        if let Some(m) = map {
            layers.push(Layer::Map(m));
        }
        layers.extend(extra.iter().map(|c| Layer::Base(c)));
        if let Some(b) = base {
            layers.push(Layer::Base(b));
        }
        let mut map_index: HashMap<(CatalogKind, &'a str), Vec<usize>> = HashMap::new();
        if let Some(m) = map {
            for (i, e) in m.entries.iter().enumerate() {
                map_index.entry((e.kind(), e.id())).or_default().push(i);
            }
        }
        let mut me = ModelLookup { layers, map_index, unit_actors: HashMap::new() };
        me.index_unit_actors();
        me
    }

    fn index_unit_actors(&mut self) {
        let mut found: HashMap<String, (Ent, bool)> = HashMap::new();
        for layer in 0..self.layers.len() {
            let count = match self.layers[layer] {
                Layer::Map(c) => c.entries.len(),
                Layer::Base(c) => c.entries.len(),
            };
            for index in 0..count {
                let e = Ent { layer, index };
                let node = self.node(e);
                if !is_unit_actor(&node.name) || node.attr("default") == Some("1") || self.kind(e) != CatalogKind::Actor
                {
                    continue;
                }
                let id = node.attr("id").unwrap_or("");
                if id.is_empty() {
                    continue;
                }
                let chain = self.chain(e);
                let unit = self.token(&chain, "unitName").unwrap_or_else(|| id.to_owned());
                if unit.is_empty() || unit == "Bogus" {
                    continue;
                }
                // Higher layers win; within a layer the actor named like the unit wins.
                let exact = id == unit;
                match found.get(&unit) {
                    Some((prev, prev_exact))
                        if prev.layer < layer || (prev.layer == layer && (*prev_exact || !exact)) => {}
                    _ => {
                        found.insert(unit, (e, exact));
                    }
                }
            }
        }
        self.unit_actors = found.into_iter().map(|(k, (e, _))| (k, e)).collect();
    }

    // -- entries ------------------------------------------------------------

    fn node(&self, e: Ent) -> &'a XmlNode {
        match self.layers[e.layer] {
            Layer::Map(c) => &c.entries[e.index].node,
            Layer::Base(c) => &c.entries[e.index].node,
        }
    }

    fn kind(&self, e: Ent) -> CatalogKind {
        match self.layers[e.layer] {
            Layer::Map(c) => c.entries[e.index].kind(),
            Layer::Base(c) => c.entries[e.index].kind(),
        }
    }

    /// The entry `kind`/`id` in the first layer at or after `from` that has it.
    fn find_from(&self, kind: CatalogKind, id: &str, from: usize) -> Option<Ent> {
        for layer in from..self.layers.len() {
            let hit = match self.layers[layer] {
                Layer::Map(_) => self.map_index.get(&(kind, id)).and_then(|v| v.first().copied()),
                Layer::Base(c) => c.find(kind, id),
            };
            if let Some(index) = hit {
                return Some(Ent { layer, index });
            }
        }
        None
    }

    fn find(&self, kind: CatalogKind, id: &str) -> Option<Ent> {
        self.find_from(kind, id, 0)
    }

    /// What entry `e` inherits from: its `parent` (searched from the top
    /// layer), else the same id in a lower layer (an entry that modifies
    /// it), else its class default.
    fn parent(&self, e: Ent) -> Option<Ent> {
        let node = self.node(e);
        let kind = self.kind(e);
        let id = node.attr("id").unwrap_or("");
        if let Some(p) = node.attr("parent").filter(|p| !p.is_empty()) {
            let from = if p == id { e.layer + 1 } else { 0 };
            return self.find_from(kind, p, from).filter(|&x| x != e);
        }
        if node.attr("default") == Some("1") && id.is_empty() {
            return None;
        }
        if !id.is_empty() {
            if let Some(x) = self.find_from(kind, id, e.layer + 1) {
                return Some(x);
            }
        }
        (e.layer..self.layers.len()).find_map(|layer| match self.layers[layer] {
            Layer::Base(c) => c.class_default(&node.name).map(|index| Ent { layer, index }).filter(|&x| x != e),
            Layer::Map(_) => None,
        })
    }

    /// `e` and its ancestors, nearest first (cycle-safe).
    fn chain(&self, e: Ent) -> Vec<Ent> {
        let mut out = vec![e];
        let mut cur = e;
        while let Some(p) = self.parent(cur) {
            if out.contains(&p) || out.len() > 48 {
                break;
            }
            out.push(p);
            cur = p;
        }
        out
    }

    /// Scalar field `name` through the chain.
    fn field(&self, chain: &[Ent], name: &str) -> Option<&'a str> {
        chain.iter().find_map(|&e| self.node(e).child(name).and_then(|c| c.attr("value")))
    }

    /// Value of token `name` for the entry at the head of `chain`: an
    /// attribute set on the nearest entry, else the nearest declaration.
    fn token(&self, chain: &[Ent], name: &str) -> Option<String> {
        if name == "id" {
            return chain.first().and_then(|&e| self.node(e).attr("id")).map(str::to_owned);
        }
        for &e in chain {
            if let Some(v) = self.node(e).attr(name) {
                return Some(v.to_owned());
            }
            if let Layer::Base(c) = self.layers[e.layer] {
                if let Some((_, v)) = c.declared_tokens(e.index).iter().find(|(t, _)| t == name) {
                    return Some(v.clone());
                }
            }
        }
        None
    }

    /// Fills `##token##` placeholders from the chain; unknown tokens become "".
    fn expand(&self, chain: &[Ent], value: &str) -> String {
        let mut s = value.to_owned();
        for _ in 0..4 {
            if !s.contains("##") {
                break;
            }
            let mut out = String::with_capacity(s.len());
            let mut rest = s.as_str();
            while let Some(a) = rest.find("##") {
                out.push_str(&rest[..a]);
                let after = &rest[a + 2..];
                match after.find("##") {
                    Some(b) => {
                        let name = &after[..b];
                        out.push_str(&self.token(chain, name).unwrap_or_default());
                        rest = &after[b + 2..];
                    }
                    None => {
                        out.push_str(&rest[a..]);
                        rest = "";
                    }
                }
            }
            out.push_str(rest);
            s = out;
        }
        s
    }

    // -- resolution -----------------------------------------------------------

    /// The model of a placed unit type.
    pub fn unit_model(&self, unit: &str) -> Result<ModelRef, Miss> {
        if let Some(&actor) = self.unit_actors.get(unit) {
            return self.actor_model(actor);
        }
        // No actor for it (or data the layers do not cover): a model of the same id.
        match self.find(CatalogKind::Model, unit) {
            Some(m) => self.model_path(m, None),
            None => Err(Miss::NotInData),
        }
    }

    /// The model of a placed doodad type (an actor or model id).
    pub fn doodad_model(&self, doodad: &str) -> Result<ModelRef, Miss> {
        if let Some(actor) = self.find(CatalogKind::Actor, doodad).filter(|&e| is_doodad_actor(&self.node(e).name)) {
            if let Ok(m) = self.actor_model(actor) {
                return Ok(m);
            }
        }
        match self.find(CatalogKind::Model, doodad) {
            Some(m) => self.model_path(m, None),
            None => Err(Miss::NotInData),
        }
    }

    fn actor_model(&self, actor: Ent) -> Result<ModelRef, Miss> {
        let chain = self.chain(actor);
        let actor_id = self.node(actor).attr("id").map(str::to_owned);
        let raw = self.field(&chain, "Model").unwrap_or("##id##");
        let model_id = self.expand(&chain, raw);
        if model_id.is_empty() {
            return Err(Miss::NoModel(raw.to_owned()));
        }
        let m = self.find(CatalogKind::Model, &model_id).ok_or(Miss::NoModel(model_id))?;
        self.model_path(m, actor_id)
    }

    fn model_path(&self, model: Ent, actor: Option<String>) -> Result<ModelRef, Miss> {
        let chain = self.chain(model);
        let id = self.node(model).attr("id").unwrap_or("").to_owned();
        let path = self.field(&chain, "Model").map(|v| self.expand(&chain, v)).unwrap_or_default();
        let path = path.trim().to_owned();
        if path.is_empty() || !path.to_ascii_lowercase().ends_with(".m3") {
            return Err(Miss::NoPath(id));
        }
        let scale = self.field(&chain, "ScaleMax").and_then(parse_vec3).unwrap_or([1.0; 3]);
        let variations =
            self.field(&chain, "VariationCount").and_then(|v| v.trim().parse().ok()).unwrap_or(0u32).min(100);
        Ok(ModelRef { path, model: id, actor, scale, variations })
    }
}

fn parse_vec3(s: &str) -> Option<[f32; 3]> {
    let v: Vec<f32> = s.split(',').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
    match v.as_slice() {
        [x, y, z] if [x, y, z].iter().all(|c| c.is_finite() && **c > 0.0) => Some([*x, *y, *z]),
        [x] if x.is_finite() && *x > 0.0 => Some([*x; 3]),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::CatalogEntry;

    const MODELS: &str = r###"<Catalog>
    <CModel default="1">
        <Radius value="0.5"/>
    </CModel>
    <CModel default="1" id="Unit">
        <?token id="Race" value="Neutral"?>
        <?token id="Prefix" value="Units\"?>
        <Model value="Assets\##Prefix####Race##\##id##\##id##.m3"/>
    </CModel>
    <CModel default="1" id="Doodad">
        <Model value="Assets\Doodads\##id##\##id##.m3"/>
    </CModel>
    <CModel default="1" id="MarineBase" parent="Unit">
        <?token id="Race" value="Terran"?>
    </CModel>
    <CModel id="Marine" parent="MarineBase"/>
    <CModel id="Pylon" parent="Unit">
        <Model value="Assets\Buildings\Protoss\PylonEx2\PylonEx2.m3"/>
        <ScaleMax value="0.85,0.85,0.85"/>
    </CModel>
    <CModel id="Zergling" parent="Unit" Race="Zerg"/>
    <CModel id="KorhalMart" parent="Doodad"/>
    <CModel id="Rock" parent="Doodad">
        <VariationCount value="3"/>
    </CModel>
</Catalog>"###;

    const ACTORS: &str = r###"<Catalog>
    <CActorUnit default="1">
        <Model value="##id##"/>
    </CActorUnit>
    <CActorDoodad default="1">
        <Model value="##id##"/>
    </CActorDoodad>
    <CActorUnit default="1" id="GenericUnitBase">
        <?token id="unitName" value="Bogus"?>
    </CActorUnit>
    <CActorUnit default="1" id="BuildingBase" parent="GenericUnitBase">
        <Model value="##unitName##"/>
    </CActorUnit>
    <CActorUnit id="Marine" parent="GenericUnitBase" unitName="Marine"/>
    <CActorUnit id="PylonActor" parent="BuildingBase" unitName="Pylon"/>
    <CActorUnit id="Zergling" parent="GenericUnitBase" unitName="Zergling"/>
    <CActorDoodad id="KorhalMart"/>
</Catalog>"###;

    fn base() -> BaseCatalog {
        let mut b = BaseCatalog::builder("1");
        b.begin_mod("Core");
        b.add_catalog("modeldata.xml", MODELS.as_bytes()).unwrap();
        b.add_catalog("actordata.xml", ACTORS.as_bytes()).unwrap();
        b.finish()
    }

    #[test]
    fn units_resolve_through_actor_and_model_templates() {
        let b = base();
        let l = ModelLookup::new(None, &[], Some(&b));
        let m = l.unit_model("Marine").unwrap();
        assert_eq!(m.path, r"Assets\Units\Terran\Marine\Marine.m3");
        assert_eq!((m.model.as_str(), m.actor.as_deref(), m.scale), ("Marine", Some("Marine"), [1.0; 3]));
        // The building actor names its model through ##unitName##.
        let p = l.unit_model("Pylon").unwrap();
        assert_eq!(p.path, r"Assets\Buildings\Protoss\PylonEx2\PylonEx2.m3");
        assert_eq!((p.actor.as_deref(), p.scale), (Some("PylonActor"), [0.85; 3]));
        // A token set as an attribute beats the declared default.
        assert_eq!(l.unit_model("Zergling").unwrap().path, r"Assets\Units\Zerg\Zergling\Zergling.m3");
        assert_eq!(l.unit_model("Nothing"), Err(Miss::NotInData));
    }

    #[test]
    fn doodads_resolve_by_actor_or_model() {
        let b = base();
        let l = ModelLookup::new(None, &[], Some(&b));
        assert_eq!(l.doodad_model("KorhalMart").unwrap().path, r"Assets\Doodads\KorhalMart\KorhalMart.m3");
        let rock = l.doodad_model("Rock").unwrap();
        assert_eq!((rock.path.as_str(), rock.variations), (r"Assets\Doodads\Rock\Rock.m3", 3));
        assert_eq!(rock.variation_path(1), r"Assets\Doodads\Rock\Rock_01.m3");
        assert_eq!(rock.variation_path(7), r"Assets\Doodads\Rock\Rock_02.m3", "clamped to the last variation");
        assert_eq!(l.doodad_model("KorhalMart").unwrap().variation_path(4), r"Assets\Doodads\KorhalMart\KorhalMart.m3");
        assert_eq!(l.doodad_model("Missing"), Err(Miss::NotInData));
    }

    #[test]
    fn map_data_overrides_base() {
        let b = base();
        let mut map = Catalog::default();
        // A custom unit actor using a base model.
        let mut a = CatalogEntry::new("ActorData", "CActorUnit", "HeroActor", Some("GenericUnitBase"));
        a.node.set_attr("unitName", "Hero");
        a.set_field("Model", "Pylon");
        map.entries.push(a);
        // The map edits the base Marine model in place (no parent).
        let mut m = CatalogEntry::new("ModelData", "CModel", "Marine", None);
        m.set_field("Model", r"Assets\Custom\BigMarine.m3");
        map.entries.push(m);
        // A model that is only in the map, inheriting a base template.
        map.entries.push(CatalogEntry::new("ModelData", "CModel", "MyRock", Some("Doodad")));
        let l = ModelLookup::new(Some(&map), &[], Some(&b));
        let h = l.unit_model("Hero").unwrap();
        assert_eq!((h.path.as_str(), h.scale), (r"Assets\Buildings\Protoss\PylonEx2\PylonEx2.m3", [0.85; 3]));
        assert_eq!(l.unit_model("Marine").unwrap().path, r"Assets\Custom\BigMarine.m3");
        assert_eq!(l.doodad_model("MyRock").unwrap().path, r"Assets\Doodads\MyRock\MyRock.m3");
    }

    #[test]
    fn extra_layers_sit_between_map_and_base() {
        let b = base();
        let mut x = BaseCatalog::builder("1");
        x.begin_mod("Campaign");
        x.add_catalog(
            "actordata.xml",
            br#"<Catalog><CActorUnit id="Tumor" parent="GenericUnitBase" unitName="TumorUsed"><Model value="Rock"/></CActorUnit></Catalog>"#,
        )
        .unwrap();
        let x = x.finish();
        let l = ModelLookup::new(None, &[&x], Some(&b));
        let t = l.unit_model("TumorUsed").unwrap();
        assert_eq!((t.path.as_str(), t.actor.as_deref()), (r"Assets\Doodads\Rock\Rock.m3", Some("Tumor")));
    }

    #[test]
    fn cycles_and_broken_templates_do_not_hang() {
        let mut map = Catalog::default();
        map.entries.push(CatalogEntry::new("ModelData", "CModel", "A", Some("B")));
        map.entries.push(CatalogEntry::new("ModelData", "CModel", "B", Some("A")));
        let mut c = CatalogEntry::new("ModelData", "CModel", "C", None);
        c.set_field("Model", "Assets\\##Unclosed\\C.m3");
        map.entries.push(c);
        let l = ModelLookup::new(Some(&map), &[], None);
        assert_eq!(l.doodad_model("A"), Err(Miss::NoPath("A".into())));
        assert_eq!(l.doodad_model("C").unwrap().path, "Assets\\##Unclosed\\C.m3");
    }
}
