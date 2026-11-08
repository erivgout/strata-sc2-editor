//! Write a brand-new Strata map as an unpacked folder, for opening in the
//! SC2 editor or checking with other tools:
//!
//! ```text
//! cargo run -p strata-format --example new_map -- "C:/.../Maps/StrataNew.SC2Map" [width] [height]
//! ```

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: new_map <folder.SC2Map> [width] [height]");
        std::process::exit(2);
    };
    let w = args.next().and_then(|a| a.parse().ok()).unwrap_or(64);
    let h = args.next().and_then(|a| a.parse().ok()).unwrap_or(64);
    let mut doc = strata_core::MapDocument::new("Strata New Map", w, h);
    let mut p = strata_core::objects::PlacedObject::new(
        doc.objects.alloc_id(),
        strata_core::objects::ObjectKind::Point,
        "StartLoc",
        strata_core::Vec3::new(w as f32 / 4.0, h as f32 / 4.0, 0.0),
    );
    p.name = "Start Location 001".into();
    doc.objects.objects.push(p);
    let dir = std::path::Path::new(&path);
    std::fs::create_dir_all(dir).expect("create folder");
    strata_format::save_path(&doc, dir).expect("save");
    println!("wrote {}", dir.display());
}
