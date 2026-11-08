//! Builds the base catalog from a real StarCraft II install and checks the
//! curated ids in `strata_core::known` against it. Skipped unless
//! `STRATA_SC2_DIR` points at an install, e.g.
//! `STRATA_SC2_DIR="C:/Program Files (x86)/StarCraft II" cargo test -p strata-core --release --test real_base_data -- --nocapture`.

use std::time::Instant;

use strata_core::base_data::{self, BaseCatalog};
use strata_core::catalog::{CatalogKind, FieldKey};
use strata_core::known::{self, UnitGroup};

fn load() -> Option<BaseCatalog> {
    let dir = std::env::var("STRATA_SC2_DIR").ok().filter(|s| !s.is_empty())?;
    let t = Instant::now();
    let cat = base_data::load_install(std::path::Path::new(&dir)).expect("build base catalog");
    let built = t.elapsed();
    let t = Instant::now();
    let bytes = cat.to_bytes();
    let encoded = t.elapsed();
    let t = Instant::now();
    let back = BaseCatalog::from_bytes(&bytes).expect("decode cache");
    eprintln!(
        "base data {}: {} entries, {} placeable units, {} doodads, {} strings; built in {built:?}, \
         cache {} KiB (encode {encoded:?}, decode {:?})",
        cat.version,
        cat.len(),
        cat.units().len(),
        cat.doodads().len(),
        cat.strings.len(),
        bytes.len() / 1024,
        t.elapsed()
    );
    assert_eq!(back.units().len(), cat.units().len());
    Some(back)
}

#[test]
fn real_base_data() {
    let Some(cat) = load() else {
        eprintln!("STRATA_SC2_DIR not set; skipping");
        return;
    };

    // Real values with inheritance.
    assert_eq!(cat.field(CatalogKind::Unit, "Marine", "LifeMax"), Some("45"));
    assert_eq!(cat.field(CatalogKind::Unit, "Marine", "Race"), Some("Terr"));
    assert_eq!(cat.display_name(CatalogKind::Unit, "HellionTank"), Some("Hellbat"));
    let marine = cat.find(CatalogKind::Unit, "Marine").unwrap();
    assert_eq!(cat.resolve_value(marine, &FieldKey::indexed("CostResource", "Minerals")), Some("50"));
    let cc = cat.unit("CommandCenter").expect("Command Center is placeable");
    assert!(cc.structure && cc.race == UnitGroup::Terran);
    assert_eq!(cc.footprint.as_deref(), Some("Footprint5x5Contour"));
    assert!(cat.unit("Mutalisk").is_some_and(|u| u.air));
    for g in UnitGroup::ALL {
        let n = cat.units().iter().filter(|u| u.race == g && u.is_melee()).count();
        eprintln!("  {}: {n} melee units", g.label());
        assert!(n > 10, "{} has only {n} melee units", g.label());
    }

    // Every curated id must exist in the real data.
    let mut missing = Vec::new();
    for (_, u) in known::all_units() {
        if !cat.has_unit(u.id) {
            missing.push(format!("unit {}", u.id));
        }
    }
    for d in known::DOODADS {
        if cat.find(CatalogKind::Actor, d.id).is_none() {
            missing.push(format!("doodad {}", d.id));
        }
    }
    assert!(missing.is_empty(), "ids not in the base game: {missing:?}");
}
