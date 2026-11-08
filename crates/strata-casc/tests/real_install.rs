//! Reads from a real StarCraft II install. Skipped unless `STRATA_SC2_DIR`
//! points at one, e.g.
//! `STRATA_SC2_DIR="C:/Program Files (x86)/StarCraft II" cargo test -p strata-casc --release -- --nocapture`.

use std::time::Instant;

use strata_casc::{block_on, DirSource, Storage};

fn install() -> Option<String> {
    std::env::var("STRATA_SC2_DIR").ok().filter(|s| !s.is_empty())
}

#[test]
fn reads_liberty_unit_data() {
    let Some(dir) = install() else {
        eprintln!("STRATA_SC2_DIR not set; skipping");
        return;
    };
    let t = Instant::now();
    let storage = block_on(Storage::open(DirSource::new(&dir))).expect("open storage");
    let opened = t.elapsed();
    let files = storage.files().len();
    assert!(files > 1000, "only {files} files in the root");

    let path = "Mods/Liberty.SC2Mod/Base.SC2Data/GameData/UnitData.xml";
    let entry = storage.find(path).expect("Liberty UnitData.xml is listed");
    let t = Instant::now();
    let bytes = block_on(storage.read(path)).expect("read UnitData.xml");
    assert_eq!(bytes.len(), entry.size as usize);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.starts_with("<?xml"), "{}", &text[..text.len().min(80)]);
    assert!(text.contains("<CUnit id=\"Marine\">"));
    eprintln!(
        "build {} ({}): {files} files, {} local keys, opened in {opened:?}; UnitData.xml {} bytes in {:?}",
        storage.build.version,
        storage.build_name,
        storage.local_key_count(),
        bytes.len(),
        t.elapsed()
    );
}
