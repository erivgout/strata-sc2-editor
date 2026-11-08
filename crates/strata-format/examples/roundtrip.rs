//! Load an .SC2Map archive, print a summary, save it back, and reload it.
//! Usage: cargo run --release -p strata-format --example roundtrip -- <in.SC2Map> [out.SC2Map]

use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(input) = args.first() else {
        eprintln!("usage: roundtrip <in.SC2Map> [out.SC2Map]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(input).expect("read input");
    let t = Instant::now();
    let doc = strata_format::load_sc2map(bytes).expect("load");
    println!(
        "loaded {} in {:?}: {}x{} tileset={} players={} objects={} regions={} data={} passthrough={}",
        doc.info.name,
        t.elapsed(),
        doc.info.width,
        doc.info.height,
        doc.info.tileset,
        doc.info.players.len(),
        doc.objects.objects.len(),
        doc.regions.len(),
        doc.data.entries.len(),
        doc.passthrough.len()
    );
    let t = Instant::now();
    let out = strata_format::save_sc2map(&doc).expect("save");
    println!("saved {} bytes in {:?}", out.len(), t.elapsed());
    let again = strata_format::load_sc2map(out.clone()).expect("reload");
    assert_eq!(again.terrain.heights, doc.terrain.heights, "heights changed on round trip");
    assert_eq!(again.objects.objects.len(), doc.objects.objects.len());
    let a = strata_format::save_files(&doc).expect("files");
    let b = strata_format::save_files(&again).expect("files");
    assert_eq!(a, b, "file sets differ after archive round trip");
    println!("round trip ok: {} files identical", a.len());
    if let Some(path) = args.get(1) {
        std::fs::write(path, out).expect("write output");
        println!("wrote {path}");
    }
}
