//! Real-map tests. Gated: set `STRATA_TEST_MAP` to one or more unpacked
//! `.SC2Map` folders (separated by `;`). Without it the test does nothing.
//!
//! ```text
//! STRATA_TEST_MAP="C:/Users/me/Documents/StarCraft II/Maps/test_map.SC2Map" cargo test -p strata-format --test real_maps -- --nocapture
//! ```

#![cfg(not(target_arch = "wasm32"))]

use std::path::Path;

use strata_core::objects::ObjectKind;
use strata_format::{load_files, read_folder, save_files, FileMap};

fn maps() -> Vec<String> {
    std::env::var("STRATA_TEST_MAP")
        .map(|v| v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

fn changed(a: &FileMap, b: &FileMap) -> Vec<String> {
    let mut keys: Vec<String> = a.keys().chain(b.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.into_iter().filter(|k| a.get(k) != b.get(k)).collect()
}

#[test]
fn real_maps_roundtrip_and_edit() {
    let maps = maps();
    if maps.is_empty() {
        eprintln!("STRATA_TEST_MAP not set; skipping real-map test");
        return;
    }
    for m in maps {
        let files = read_folder(Path::new(&m)).expect("read map folder");
        let doc = load_files(files.clone()).expect("load");
        eprintln!(
            "{m}: {}x{} tileset {:?}, {} players, {} objects, {} cameras, {} regions, {} catalog entries, {} locales, {} layers",
            doc.info.width,
            doc.info.height,
            doc.info.tileset,
            doc.info.players.len(),
            doc.objects.objects.len(),
            doc.cameras.len(),
            doc.regions.len(),
            doc.data.entries.len(),
            doc.strings.locales.len(),
            doc.terrain.layers.len(),
        );
        assert!(doc.info.width > 0 && doc.info.height > 0);
        assert_eq!(doc.terrain.heights.len(), ((doc.info.width + 1) * (doc.info.height + 1)) as usize);

        // 0. Our MapInfo integrity hash reproduces the editor's.
        assert_hash_ok(&files);

        // 1. Untouched: byte-identical.
        let out = save_files(&doc).expect("save");
        assert!(changed(&files, &out).is_empty(), "lossless save changed {:?}", changed(&files, &out));

        // 2. Everything dirty but nothing edited: still byte-identical.
        let mut all = doc.clone();
        all.dirty = strata_core::document::DirtyFlags::all();
        let out = save_files(&all).expect("save all-dirty");
        let diff = changed(&files, &out);
        assert!(diff.is_empty(), "all-dirty but unedited save regenerated {diff:?}");
        let re = load_files(out.clone()).expect("reload all-dirty");
        assert_eq!(re.objects.objects, doc.objects.objects);
        assert_eq!(re.regions, doc.regions);
        assert_eq!(re.data, doc.data);
        assert_eq!(re.terrain, doc.terrain);

        // 3. Targeted edits touch only their files.
        let mut ed = doc.clone();
        if let Some(u) = ed.objects.objects.iter_mut().find(|o| o.kind == ObjectKind::Unit) {
            u.position.x += 1.0;
            ed.dirty.objects = true;
        }
        let vi = ed.terrain.vertex_index(ed.terrain.width / 2, ed.terrain.height / 2);
        ed.terrain.heights[vi] += 0.75;
        ed.dirty.terrain = true;
        if let Some(e) = ed.data.entries.first_mut() {
            e.node.set_attr("strataTest", "1");
            ed.dirty.data = true;
        }
        let out = save_files(&ed).expect("save edited");
        let diff = changed(&files, &out);
        eprintln!("  edited save changes: {diff:?}");
        assert!(diff.iter().any(|k| k == "t3HeightMap"));
        for k in &diff {
            assert!(
                ["Objects", "t3HeightMap", "t3SyncHeightMap", "t3Terrain.xml"].contains(&k.as_str())
                    || k.starts_with("Base.SC2Data/GameData/"),
                "edit touched unexpected file {k}"
            );
        }
        let re = load_files(out).expect("reload edited");
        assert!((re.terrain.heights[vi] - ed.terrain.heights[vi]).abs() < 0.01);
        assert_eq!(re.objects.objects, ed.objects.objects);
        assert_eq!(re.data, ed.data);

        // 4. Changing the camera bounds keeps the integrity hash valid.
        let mut ed = doc.clone();
        ed.info.playable = [0, 0, ed.info.width, ed.info.height];
        ed.dirty.info = true;
        let out = save_files(&ed).expect("save bounds");
        assert_hash_ok(&out);
        assert_eq!(load_files(out).unwrap().info.playable, ed.info.playable);
    }
}

fn assert_hash_ok(files: &FileMap) {
    let Some(b) = files.get("MapInfo") else { return };
    let f = strata_format::info::parse_mapinfo(b).expect("MapInfo parses");
    if let (Some(hash), Some(flags)) = (f.hash, f.flags) {
        let want = strata_format::info::integrity_hash(&f.tileset, f.width, f.height, f.bounds, flags);
        assert_eq!(hash, want, "MapInfo integrity hash");
    }
}
