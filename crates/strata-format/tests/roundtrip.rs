use strata_core::catalog::{CatalogEntry, XmlNode};
use strata_core::objects::{ObjectId, ObjectKind, PlacedObject};
use strata_core::regions::{Camera, Region, Shape};
use strata_core::triggers::TriggerLibrary;
use strata_core::{MapDocument, Race, Vec2, Vec3};
use strata_format::{load_files, mark_saved, save_files, FileMap};

fn changed(a: &FileMap, b: &FileMap) -> Vec<String> {
    let mut keys: Vec<String> = a.keys().chain(b.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.into_iter().filter(|k| a.get(k) != b.get(k)).collect()
}

fn sample_doc() -> MapDocument {
    let mut doc = MapDocument::new("Strata Test", 96, 64);
    doc.info.description = "A test map".into();
    doc.info.tileset = "Agria".into();
    doc.info.players[1].race = Race::Zerg;
    let mut u = PlacedObject::new(ObjectId(10), ObjectKind::Unit, "Marine", Vec3::new(20.5, 30.25, 0.0));
    u.rotation = 1.5;
    u.player = 1;
    let mut d = PlacedObject::new(ObjectId(11), ObjectKind::Doodad, "AgriaRock", Vec3::new(40.0, 10.0, 8.0));
    d.scale = Vec3::splat(1.5);
    d.extra.push(("Variation".into(), "2".into()));
    let mut p = PlacedObject::new(ObjectId(12), ObjectKind::Point, "StartLoc", Vec3::new(12.5, 12.5, 0.0));
    p.name = "Start Location 001".into();
    doc.objects.objects = vec![u, d, p];
    doc.regions.push(Region {
        id: ObjectId(13),
        name: "Base".into(),
        color: [255, 0, 0],
        shapes: vec![Shape::Rect { min: Vec2::new(1.0, 2.0), max: Vec2::new(10.0, 20.0) }],
    });
    doc.cameras.push(Camera::new(ObjectId(14), "Intro", Vec2::new(48.0, 32.0)));
    let mut n = XmlNode::new("CUnit");
    n.set_attr("id", "Marine");
    n.set_attr("parent", "Marine");
    let mut e = CatalogEntry { file: "UnitData".into(), node: n };
    e.set_field("LifeMax", "55");
    doc.data.entries.push(e);
    doc.strings.set("enUS", "Unit/Name/Marine", "Space Marine");
    doc.strings.set("deDE", "DocInfo/Name", "Strata Testkarte");
    let i = doc.terrain.vertex_index(5, 5);
    doc.terrain.heights[i] = 14.0;
    doc.terrain.cliff_levels[i] = 3;
    doc
}

#[test]
fn new_map_has_complete_file_set_and_reloads() {
    let doc = sample_doc();
    let files = save_files(&doc).unwrap();
    for f in [
        "DocumentHeader",
        "DocumentInfo",
        "MapInfo",
        "ComponentList.SC2Components",
        "t3Terrain.xml",
        "t3HeightMap",
        "t3SyncHeightMap",
        "t3SyncCliffLevel",
        "t3CellFlags",
        "t3TextureMasks",
        "t3SyncTextureInfo",
        "t3VertCol",
        "t3Water",
        "t3HardTile",
        "t3FluffDoodad",
        "Objects",
        "Regions",
        "MapScript.galaxy",
        "Strata/Triggers.json",
        "Minimap.tga",
        "enUS.SC2Data/LocalizedData/GameStrings.txt",
        "deDE.SC2Data/LocalizedData/GameStrings.txt",
        "Base.SC2Data/GameData/UnitData.xml",
    ] {
        assert!(files.contains_key(f), "missing {f}");
    }
    let comps = String::from_utf8(files["ComponentList.SC2Components"].clone()).unwrap();
    for ty in ["gada", "info", "mapi", "terr", "plob", "regi", "Locale=\"enUS\"", "Locale=\"deDE\""] {
        assert!(comps.contains(ty), "ComponentList lacks {ty}: {comps}");
    }
    let gs = &files["enUS.SC2Data/LocalizedData/GameStrings.txt"];
    assert!(gs.starts_with(b"\xEF\xBB\xBF"));
    let gs = String::from_utf8_lossy(gs);
    assert!(gs.contains("DocInfo/Name=Strata Test\r\n"));
    assert!(gs.contains("MapInfo/Player00/Name=Neutral\r\n"));
    assert!(gs.contains("MapInfo/Player01/Name=Player 1\r\n"));

    let back = load_files(files.clone()).unwrap();
    assert_eq!(back.info.name, "Strata Test");
    assert_eq!(back.info.description, "A test map");
    assert_eq!(back.info.tileset, "Agria");
    assert_eq!((back.info.width, back.info.height), (96, 64));
    assert_eq!(back.info.playable, doc.info.playable);
    assert_eq!(back.info.dependencies, doc.info.dependencies);
    // Neutral (0) and hostile (15) slots are added like the editor does.
    let ids: Vec<u8> = back.info.players.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec![0, 1, 2, 15]);
    assert_eq!(back.info.players[2].race, Race::Zerg);
    assert_eq!(back.info.players[1].color, doc.info.players[0].color);
    assert_eq!(back.objects.objects, doc.objects.objects);
    assert_eq!(back.regions, doc.regions);
    assert_eq!(back.cameras[0].target, doc.cameras[0].target);
    assert_eq!(back.data, doc.data);
    assert_eq!(back.triggers, doc.triggers);
    assert_eq!(back.strings.get("enUS", "Unit/Name/Marine"), Some("Space Marine"));
    assert_eq!(back.strings.get("deDE", "DocInfo/Name"), Some("Strata Testkarte"));
    assert_eq!(back.terrain.cliff_levels, doc.terrain.cliff_levels);
    for (a, b) in back.terrain.heights.iter().zip(&doc.terrain.heights) {
        assert!((a - b).abs() < 1e-3);
    }
    assert!(back.objects.next_id > 14);
    assert!(!back.dirty.any());

    // Lossless from here on.
    assert_eq!(save_files(&back).unwrap(), files);
    let mut all = back.clone();
    all.dirty = strata_core::document::DirtyFlags::all();
    assert!(changed(&files, &save_files(&all).unwrap()).is_empty());
}

#[test]
fn edits_touch_only_their_component() {
    let files = save_files(&sample_doc()).unwrap();
    let doc = load_files(files.clone()).unwrap();

    let mut d = doc.clone();
    d.objects.objects[0].position.x = 50.0;
    d.dirty.objects = true;
    assert_eq!(changed(&files, &save_files(&d).unwrap()), vec!["Objects"]);

    let mut d = doc.clone();
    d.regions[0].name = "Renamed".into();
    d.dirty.regions = true;
    assert_eq!(changed(&files, &save_files(&d).unwrap()), vec!["Regions"]);

    let mut d = doc.clone();
    d.data.entries[0].set_field("LifeMax", "60");
    d.dirty.data = true;
    assert_eq!(changed(&files, &save_files(&d).unwrap()), vec!["Base.SC2Data/GameData/UnitData.xml"]);

    let mut d = doc.clone();
    d.strings.set("deDE", "Unit/Name/Marine", "Marinesoldat");
    d.dirty.strings = true;
    assert_eq!(changed(&files, &save_files(&d).unwrap()), vec!["deDE.SC2Data/LocalizedData/GameStrings.txt"]);

    let mut d = doc.clone();
    d.info.name = "Renamed Map".into();
    d.dirty.info = true;
    let out = save_files(&d).unwrap();
    assert_eq!(changed(&files, &out), vec!["DocumentHeader", "enUS.SC2Data/LocalizedData/GameStrings.txt"]);
    assert_eq!(load_files(out).unwrap().info.name, "Renamed Map");

    let mut d = doc.clone();
    d.info.playable = [4, 4, 90, 60];
    d.info.players[2].control = strata_core::PlayerControl::Computer;
    d.dirty.info = true;
    let out = save_files(&d).unwrap();
    assert_eq!(changed(&files, &out), vec!["MapInfo"]);
    let re = load_files(out).unwrap();
    assert_eq!(re.info.playable, [4, 4, 90, 60]);
    assert_eq!(re.info.players[2].control, strata_core::PlayerControl::Computer);

    let mut d = doc.clone();
    d.triggers = TriggerLibrary::default();
    d.dirty.triggers = true;
    let out = save_files(&d).unwrap();
    assert_eq!(changed(&files, &out), vec!["MapScript.galaxy", "Strata/Triggers.json"]);
    assert_eq!(load_files(out).unwrap().triggers, TriggerLibrary::default());
}

#[test]
fn mark_saved_makes_a_new_baseline() {
    let mut doc = load_files(save_files(&sample_doc()).unwrap()).unwrap();
    doc.objects.objects[0].player = 2;
    doc.dirty.objects = true;
    let saved = save_files(&doc).unwrap();
    mark_saved(&mut doc, saved.clone());
    assert!(!doc.dirty.any());
    assert_eq!(save_files(&doc).unwrap(), saved);
    assert_eq!(load_files(saved).unwrap().objects.objects[0].player, 2);
}

/// Hand-written editor-style files with quirks (LF endings, comments, unknown
/// elements and files, no trailing newline) survive load -> save untouched,
/// and survive a save with every dirty flag set.
#[test]
fn quirky_fixture_is_lossless() {
    let base = save_files(&sample_doc()).unwrap();
    let mut files: FileMap = base
        .into_iter()
        .filter(|(k, _)| !k.ends_with(".xml") && !k.contains("GameStrings") && k != "Objects" && k != "Regions")
        .collect();
    files.insert("t3Terrain.xml".into(), {
        let mut d = FileMap::new();
        strata_format::terrain::fresh(&sample_doc(), &mut d);
        d["t3Terrain.xml"].clone()
    });
    files.insert(
        "Objects".into(),
        b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<PlacedObjects Version=\"27\">\n  <!-- hand edited -->\n  <ObjectUnit Id=\"5\" Position=\"1.50,2,0\" Scale=\"1,1,1\" UnitType=\"SCV\" Player=\"1\" Custom='x'/>\n  <ObjectThing Id=\"6\" Odd=\"&amp;\"/>\n</PlacedObjects>"
            .to_vec(),
    );
    files.insert(
        "Regions".into(),
        b"<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<Regions>\n    <region id=\"1\">\n        <name value=\"A\"/>\n        <shape type=\"weird\"><foo/></shape>\n    </region>\n</Regions>\n"
            .to_vec(),
    );
    files.insert(
        "Base.SC2Data/GameData/UnitData.xml".into(),
        b"<?xml version=\"1.0\" encoding=\"us-ascii\"?>\n<!-- top -->\n<Catalog>\n\t<CUnit id=\"A\" default=\"1\"><Text>hi &lt;b&gt;</Text></CUnit>\n\t<const id=\"$x\" value=\"1\"/>\n</Catalog>"
            .to_vec(),
    );
    // Name matches the DocumentHeader copy; player names live only in the header.
    files.insert("enUS.SC2Data/LocalizedData/GameStrings.txt".into(), b"DocInfo/Name=Strata Test\nNoEquals line\nA=b=c".to_vec());
    files.insert("Assets/Textures/thing.dds".into(), vec![1, 2, 3, 0, 255]);
    files.insert("Triggers".into(), b"<?xml version=\"1.0\"?>\n<TriggerData/>".to_vec());
    files.remove("Strata/Triggers.json");

    let doc = load_files(files.clone()).unwrap();
    assert_eq!(doc.info.name, "Strata Test");
    assert_eq!(doc.info.description, "A test map"); // from DocumentHeader
    assert_eq!(doc.info.players[0].name, "Neutral");
    assert_eq!(doc.objects.objects.len(), 1);
    assert_eq!(doc.objects.objects[0].extra, vec![("Custom".to_owned(), "x".to_owned())]);
    assert_eq!(doc.data.entries.len(), 2);
    assert_eq!(doc.data.entries[0].node.child("Text").unwrap().text, "hi <b>");
    assert_eq!(doc.strings.get("enUS", "A"), Some("b=c"));
    assert!(doc.regions[0].shapes.is_empty());
    assert!(doc.triggers.items.is_empty());
    assert_eq!(save_files(&doc).unwrap(), files);

    let mut all = doc.clone();
    all.dirty = strata_core::document::DirtyFlags::all();
    assert!(changed(&files, &save_files(&all).unwrap()).is_empty());

    // An edit keeps the quirks around it.
    let mut d = doc.clone();
    d.objects.objects[0].position.y = 3.0;
    d.dirty.objects = true;
    let out = save_files(&d).unwrap();
    let objects = String::from_utf8(out["Objects"].clone()).unwrap();
    assert_eq!(
        objects,
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<PlacedObjects Version=\"27\">\n  <!-- hand edited -->\n  <ObjectUnit Id=\"5\" Position=\"1.5,3,0\" Scale=\"1,1,1\" UnitType=\"SCV\" Player=\"1\" Custom=\"x\"/>\n  <ObjectThing Id=\"6\" Odd=\"&amp;\"/>\n</PlacedObjects>"
    );
}

#[test]
fn not_a_map() {
    let mut files = FileMap::new();
    files.insert("readme.txt".into(), b"hi".to_vec());
    assert!(matches!(load_files(files), Err(strata_format::FormatError::NotAMap(_))));
}

#[cfg(not(target_arch = "wasm32"))]
#[test]
fn folder_save_and_load() {
    let dir = std::env::temp_dir().join(format!("strata-format-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let doc = sample_doc();
    strata_format::save_path(&doc, &dir).unwrap();
    assert!(dir.join("Base.SC2Data").join("GameData").join("UnitData.xml").is_file());
    let back = strata_format::load_path(&dir).unwrap();
    assert_eq!(back.objects.objects, doc.objects.objects);
    assert_eq!(save_files(&back).unwrap(), strata_format::read_folder(&dir).unwrap());
    std::fs::remove_dir_all(&dir).unwrap();
}
