//! Map plumbing for debugging against the real game/editor.
//!
//! ```text
//! mapkit unpack <in.SC2Map> <folder>      raw archive members -> folder
//! mapkit pack   <folder> <out.SC2Map>     folder -> archive (strata-mpq)
//! mapkit resave <in> <out>                load + save through strata-format (archive or folder by extension/dir)
//! ```

use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    match a.first().map(String::as_str) {
        Some("unpack") if a.len() == 3 => {
            let ar = strata_mpq::Archive::from_bytes(std::fs::read(&a[1]).expect("read")).expect("open archive");
            let mut files = strata_format::FileMap::new();
            for n in ar.files() {
                files.insert(n.clone(), ar.read(&n).expect("read member"));
            }
            std::fs::create_dir_all(&a[2]).expect("mkdir");
            strata_format::write_folder(Path::new(&a[2]), &files).expect("write folder");
            println!("unpacked {} files", files.len());
        }
        Some("pack") if a.len() == 3 => {
            let files = strata_format::read_folder(Path::new(&a[1])).expect("read folder");
            let mut b = strata_mpq::ArchiveBuilder::new();
            for (n, d) in &files {
                b.add(n, d.clone());
            }
            std::fs::write(&a[2], b.build().expect("build")).expect("write");
            println!("packed {} files", files.len());
        }
        Some("resave") if a.len() == 3 => {
            let doc = strata_format::load_path(Path::new(&a[1])).expect("load");
            // Saves as a folder when <out> is an existing directory.
            strata_format::save_path(&doc, Path::new(&a[2])).expect("save");
            println!("resaved");
        }
        Some("fresh") if a.len() == 3 => {
            // Regenerate every file from the model, as for a brand-new map.
            let mut doc = strata_format::load_path(Path::new(&a[1])).expect("load");
            doc.passthrough.clear();
            doc.dirty = strata_core::document::DirtyFlags::all();
            strata_format::save_path(&doc, Path::new(&a[2])).expect("save");
            println!("fresh copy written");
        }
        Some("edit") if a.len() >= 3 => {
            // mapkit edit <in|blank:WxH> <out> [deps=a|b] [tileset=X] [tex0=Y] [triggers=0]
            // -- regenerated as a new map.
            let mut doc = match a[1].strip_prefix("blank:") {
                Some(size) => {
                    let (w, h) = size.split_once('x').expect("WxH");
                    strata_core::MapDocument::new("Strata blank", w.parse().unwrap(), h.parse().unwrap())
                }
                None => strata_format::load_path(Path::new(&a[1])).expect("load"),
            };
            for kv in &a[3..] {
                let (k, v) = kv.split_once('=').expect("key=value");
                match k {
                    "deps" => doc.info.dependencies = v.split('|').filter(|s| !s.is_empty()).map(String::from).collect(),
                    "tileset" => doc.info.tileset = v.into(),
                    "tex0" => doc.terrain.layers[0].texture = v.into(),
                    "triggers" if v == "0" => doc.triggers = Default::default(),
                    "playable" => {
                        let p: Vec<u32> = v.split(',').map(|s| s.parse().unwrap()).collect();
                        doc.info.playable = [p[0], p[1], p[2], p[3]];
                    }
                    "users" => {
                        let n: usize = v.parse().unwrap();
                        doc.info.players.retain(|p| p.control != strata_core::PlayerControl::User || p.id as usize <= n);
                    }
                    "objects" if v == "0" => doc.objects.objects.clear(),
                    _ => panic!("unknown key {k}"),
                }
            }
            doc.passthrough.clear();
            doc.dirty = strata_core::document::DirtyFlags::all();
            strata_format::save_path(&doc, Path::new(&a[2])).expect("save");
            println!("edited");
        }
        Some("nudge") if a.len() == 3 => {
            // Small real edit on an existing map: move the first unit 2 cells
            // east and raise a 9x9-vertex patch at the map centre by 1.5.
            let mut doc = strata_format::load_path(Path::new(&a[1])).expect("load");
            if let Some(u) = doc.objects.objects.iter_mut().find(|o| o.kind == strata_core::objects::ObjectKind::Unit) {
                println!("moving {} #{} from {:?}", u.type_id, u.id.0, u.position);
                u.position.x += 2.0;
                doc.dirty.objects = true;
            }
            let t = &mut doc.terrain;
            let (cx, cy) = (t.width / 2, t.height / 2);
            for y in cy - 4..=cy + 4 {
                for x in cx - 4..=cx + 4 {
                    let i = t.vertex_index(x, y);
                    t.heights[i] += 1.5;
                }
            }
            doc.dirty.terrain = true;
            let changed: Vec<String> = {
                let before = &doc.passthrough;
                let after = strata_format::save_files(&doc).expect("save");
                after.iter().filter(|(k, v)| before.get(*k) != Some(*v)).map(|(k, _)| k.clone()).collect()
            };
            println!("changed files: {changed:?}");
            strata_format::save_path(&doc, Path::new(&a[2])).expect("save");
        }
        Some("bounds") if a.len() == 4 => {
            // Change only the camera bounds of an existing map (MapInfo patch path).
            let mut doc = strata_format::load_path(Path::new(&a[1])).expect("load");
            let p: Vec<u32> = a[3].split(',').map(|s| s.parse().expect("number")).collect();
            doc.info.playable = [p[0], p[1], p[2], p[3]];
            doc.dirty.info = true;
            strata_format::save_path(&doc, Path::new(&a[2])).expect("save");
            println!("bounds set to {:?}", doc.info.playable);
        }
        Some("deps") if a.len() >= 3 => {
            // mapkit deps <map> <out> [dep...]: replace the dependency list.
            let mut doc = strata_format::load_path(Path::new(&a[1])).expect("load");
            doc.info.dependencies = a[3..].to_vec();
            doc.dirty.info = true;
            // Saves as a folder when <out> is an existing directory.
            strata_format::save_path(&doc, Path::new(&a[2])).expect("save");
            println!("deps set to {:?}", doc.info.dependencies);
        }
        _ => {
            eprintln!("usage: mapkit unpack <in> <dir> | pack <dir> <out> | resave <in> <out>");
            std::process::exit(2);
        }
    }
}
