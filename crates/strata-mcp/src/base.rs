//! The base game's data (real units, abilities, names) for the
//! `list_base_units`, `search_base_data` and `get_base_data` tools.
//!
//! On start the server looks for a StarCraft II install (`STRATA_SC2_DIR`
//! or the default folder) in the background, reuses a cached index next to
//! the token file when the game version matches, and otherwise builds one
//! from the install's CASC storage. Nothing is bundled with Strata.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use strata_core::base_data::{self, BaseCatalog, MELEE_MODS};
use strata_core::catalog::CatalogKind;
use strata_core::known::UnitGroup;

enum State {
    NotStarted,
    Loading(PathBuf),
    Ready(Arc<BaseCatalog>),
    Missing(String),
}

static STATE: Mutex<State> = Mutex::new(State::NotStarted);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

fn cache_file(version: &str) -> Option<PathBuf> {
    let safe: String = version
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' })
        .collect();
    Some(
        crate::token::default_path()?
            .parent()?
            .join(format!("gamedata-{safe}.bin")),
    )
}

fn load(root: &std::path::Path) -> Result<BaseCatalog, String> {
    let version = base_data::install_version(root);
    if let Some(cat) = version
        .as_deref()
        .and_then(cache_file)
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| BaseCatalog::from_bytes(&b).ok())
        .filter(|c| {
            Some(&c.version) == version.as_ref() && c.mods.iter().map(String::as_str).eq(MELEE_MODS.iter().copied())
        })
    {
        return Ok(cat);
    }
    let cat = base_data::load_install(root)?;
    if let Some(p) = cache_file(&cat.version) {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, cat.to_bytes());
    }
    Ok(cat)
}

/// Starts finding and reading the install in the background.
pub fn start_loading() {
    let Some(root) = base_data::default_install_dirs().into_iter().next() else {
        *state() = State::Missing(
            "StarCraft II was not found. Set STRATA_SC2_DIR to the install folder (the one with .build.info) and restart strata-mcp."
                .into(),
        );
        return;
    };
    *state() = State::Loading(root.clone());
    std::thread::spawn(move || {
        let t = std::time::Instant::now();
        let next = match load(&root) {
            Ok(cat) => {
                crate::log!(
                    "base game data {}: {} units, {} entries ({:?})",
                    cat.version,
                    cat.units().len(),
                    cat.len(),
                    t.elapsed()
                );
                State::Ready(Arc::new(cat))
            }
            Err(e) => {
                crate::log!("base game data unavailable: {e}");
                State::Missing(format!("Could not read StarCraft II data from {}: {e}", root.display()))
            }
        };
        *state() = next;
    });
}

/// Uses `cat` as the base data (tests).
pub fn set(cat: BaseCatalog) {
    *state() = State::Ready(Arc::new(cat));
}

pub fn get() -> Result<Arc<BaseCatalog>, String> {
    match &*state() {
        State::Ready(c) => Ok(c.clone()),
        State::Loading(p) => Err(format!(
            "Base game data is still loading from {}; try again in a few seconds.",
            p.display()
        )),
        State::Missing(why) => Err(why.clone()),
        State::NotStarted => Err("Base game data is not loaded in this server.".into()),
    }
}

/// For `editor_status`.
pub fn status_json() -> Value {
    match &*state() {
        State::Ready(c) => json!({
            "loaded": true,
            "version": c.version,
            "mods": c.mods,
            "units": c.units().len(),
            "entries": c.len(),
        }),
        State::Loading(p) => json!({"loaded": false, "loading_from": p.display().to_string()}),
        State::Missing(why) => json!({"loaded": false, "reason": why}),
        State::NotStarted => json!({"loaded": false}),
    }
}

fn race_name(g: UnitGroup) -> &'static str {
    match g {
        UnitGroup::Terran => "terran",
        UnitGroup::Protoss => "protoss",
        UnitGroup::Zerg => "zerg",
        UnitGroup::Neutral => "neutral",
    }
}

pub fn list_units(search: Option<&str>, race: Option<&str>, melee_only: bool, limit: usize) -> Result<Value, String> {
    let cat = get()?;
    let q = search.unwrap_or("").trim().to_lowercase();
    let matches: Vec<_> = cat
        .units()
        .iter()
        .filter(|u| !melee_only || u.is_melee())
        .filter(|u| race.is_none_or(|r| race_name(u.race) == r))
        .filter(|u| q.is_empty() || u.id.to_lowercase().contains(&q) || u.name.to_lowercase().contains(&q))
        .collect();
    let total = matches.len();
    let units: Vec<Value> = matches
        .into_iter()
        .take(limit)
        .map(|u| {
            json!({
                "id": u.id,
                "name": u.name,
                "race": race_name(u.race),
                "type": u.object_type,
                "family": u.family,
                "structure": u.structure,
                "air": u.air,
                "radius": u.radius,
                "footprint": u.footprint,
                "mod": u.source_mod,
                "named": u.named,
            })
        })
        .collect();
    Ok(json!({"version": cat.version, "total": total, "units": units}))
}

pub fn search(query: &str, kind: Option<CatalogKind>, limit: usize) -> Result<Value, String> {
    let cat = get()?;
    let hits: Vec<Value> = cat
        .search(query, kind, limit)
        .into_iter()
        .map(|i| {
            let e = &cat.entries[i];
            json!({
                "kind": format!("{:?}", e.kind()).to_lowercase(),
                "class": e.class(),
                "id": e.id(),
                "name": cat.display_name(e.kind(), e.id()),
                "parent": e.parent(),
                "mod": cat.origin_of(i),
            })
        })
        .collect();
    Ok(json!({"version": cat.version, "results": hits}))
}

pub fn get_entry(kind: CatalogKind, id: &str, full: bool) -> Result<Value, String> {
    let cat = get()?;
    let i = cat.find(kind, id).ok_or_else(|| {
        format!(
            "no base-game {} with id \"{id}\"; use search_base_data to find ids",
            kind.noun()
        )
    })?;
    let e = &cat.entries[i];
    let chain: Vec<String> = cat
        .chain(i)
        .into_iter()
        .skip(1)
        .map(|j| {
            let p = &cat.entries[j];
            if p.id().is_empty() {
                format!("{} (class default)", p.class())
            } else {
                p.id().to_owned()
            }
        })
        .collect();
    let mut fields = serde_json::Map::new();
    let mut structured = Vec::new();
    for f in cat.fields(i) {
        let key = f.key.to_string();
        if f.node.is_scalar() {
            fields.insert(key, Value::String(f.node.attr("value").unwrap_or("").to_owned()));
        } else if f.node.children.is_empty() && f.node.attrs.len() <= 3 {
            let attrs: serde_json::Map<String, Value> = f
                .node
                .attrs
                .iter()
                .filter(|(k, _)| k != "index")
                .map(|(k, v)| (k.clone(), Value::String(v.clone())))
                .collect();
            fields.insert(key, Value::Object(attrs));
        } else {
            structured.push(key);
        }
    }
    let mut out = json!({
        "version": cat.version,
        "kind": format!("{kind:?}").to_lowercase(),
        "class": e.class(),
        "id": e.id(),
        "name": cat.display_name(kind, id),
        "mod": cat.origin_of(i),
        "inherits_from": chain,
        "fields": fields,
        "structured_fields": structured,
        "note": "Fields include values inherited through parents and the class default. Map entries with this id or parent inherit them.",
    });
    if full {
        out["xml"] = Value::String(e.to_xml());
    }
    Ok(out)
}
