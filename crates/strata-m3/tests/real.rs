//! Reads real models from a local StarCraft II install. Skipped unless
//! `STRATA_SC2_DIR` points at the install folder, e.g.
//! `STRATA_SC2_DIR="C:/Program Files (x86)/StarCraft II" cargo test -p strata-m3 --release -- --nocapture`.

use strata_casc::{block_on, DirSource, Storage};

const MODELS: &[(&str, usize)] = &[
    // (path, minimum triangle count)
    ("mods/liberty.sc2mod/base.sc2assets/assets/units/terran/marine/marine.m3", 500),
    ("mods/liberty.sc2mod/base.sc2assets/assets/buildings/terran/commandcenter/commandcenter.m3", 2000),
    ("mods/liberty.sc2mod/base.sc2assets/assets/buildings/protoss/pylonex2/pylonex2.m3", 200),
    ("mods/liberty.sc2mod/base.sc2assets/assets/buildings/zerg/hive/hive.m3", 1000),
    ("mods/liberty.sc2mod/base.sc2assets/assets/doodads/korhal_guardtower/korhal_guardtower.m3", 200),
    ("mods/liberty.sc2mod/base.sc2assets/assets/doodads/lavasplash/lavasplash.m3", 0),
];

#[test]
fn reads_installed_models() {
    let Ok(dir) = std::env::var("STRATA_SC2_DIR") else {
        eprintln!("STRATA_SC2_DIR not set; skipping");
        return;
    };
    let storage = block_on(Storage::open_filtered(DirSource::new(dir), |p| p.ends_with(".m3"))).expect("open CASC");
    for &(path, min_tris) in MODELS {
        let bytes = block_on(storage.read(path)).unwrap_or_else(|e| panic!("{path}: {e}"));
        let t = std::time::Instant::now();
        let m = strata_m3::parse(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        let ms = t.elapsed().as_secs_f32() * 1000.0;
        let b = m.bounds;
        let size = [b.max[0] - b.min[0], b.max[1] - b.min[1], b.max[2] - b.min[2]];
        println!(
            "{path}: v{} {} verts, {} tris, {} submeshes, size {:.2} x {:.2} x {:.2}, min z {:.2}, parsed in {ms:.1} ms",
            m.version,
            m.positions.len(),
            m.triangle_count(),
            m.submeshes.len(),
            size[0],
            size[1],
            size[2],
            b.min[2]
        );
        for s in &m.submeshes {
            let mt = &s.material;
            println!(
                "    {:>6} tris  {:?} {:?} test {} flags {:#x} diffuse {:?} emissive {:?} alpha {:?}",
                s.index_count / 3,
                mt.kind,
                mt.blend,
                mt.alpha_test,
                mt.flags,
                mt.diffuse.as_ref().and_then(|l| l.path.as_deref()),
                mt.emissive.as_ref().and_then(|l| l.path.as_deref()),
                mt.alpha_mask.as_ref().and_then(|l| l.path.as_deref()),
            );
        }
        let bind = strata_m3::parse_with(&bytes, strata_m3::Pose::Bind).unwrap();
        println!("    (bind pose: {} submeshes, {} tris)", bind.submeshes.len(), bind.triangle_count());
        assert!(m.triangle_count() >= min_tris, "{path}: only {} triangles", m.triangle_count());
        assert_eq!(m.positions.len(), m.normals.len());
        assert_eq!(m.positions.len(), m.uvs.len());
        assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        if min_tris > 0 {
            // Units and buildings are a few world units across, standing on z = 0.
            assert!(size.iter().all(|s| s.is_finite() && *s < 40.0), "{path}: size {size:?}");
            assert!(size[0] > 0.2 && size[1] > 0.2 && size[2] > 0.2, "{path}: size {size:?}");
            if !path.contains("/doodads/") {
                assert!(b.min[2] > -3.0 && b.min[2] < 2.0, "{path}: min z {}", b.min[2]);
            }
            assert!(m.submeshes.iter().any(|s| s
                .material
                .diffuse
                .as_ref()
                .and_then(|l| l.path.as_deref())
                .is_some_and(|p| p.to_ascii_lowercase().ends_with(".dds"))));
        }
    }
}

/// Parses every `.m3` in the install (slow; run with `--ignored`).
#[test]
#[ignore]
fn parses_every_installed_model() {
    let Ok(dir) = std::env::var("STRATA_SC2_DIR") else { return };
    let storage = block_on(Storage::open_filtered(DirSource::new(dir), |p| p.ends_with(".m3"))).expect("open CASC");
    let (mut ok, mut failed, mut unreadable) = (0, 0, 0);
    let mut errors = std::collections::BTreeMap::<String, usize>::new();
    for f in storage.files() {
        let Ok(bytes) = block_on(storage.read(&f.path)) else {
            unreadable += 1;
            continue;
        };
        match strata_m3::parse(&bytes) {
            Ok(_) => ok += 1,
            Err(e) => {
                failed += 1;
                let key = e.to_string();
                let key = key.split(" at ").next().unwrap_or("").to_owned();
                if !errors.contains_key(&key) {
                    println!("{}: {e}", f.path);
                }
                *errors.entry(key).or_default() += 1;
            }
        }
    }
    println!("parsed {ok}, failed {failed}, unreadable {unreadable}");
    for (e, n) in &errors {
        println!("  {n:>6}  {e}");
    }
    assert!(failed * 50 < ok, "too many failures");
}
