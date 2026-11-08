//! Lists or extracts files from a local StarCraft II install.
//!
//! `cargo run -p strata-casc --example casc_ls -- "C:/Program Files (x86)/StarCraft II" [filter] [--cat path]`

use std::time::Instant;

use strata_casc::{block_on, DirSource, Storage};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = args.first().cloned().unwrap_or_else(|| "C:/Program Files (x86)/StarCraft II".into());
    let t = Instant::now();
    let storage = match block_on(Storage::open(DirSource::new(&root))) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("open failed: {e}");
            std::process::exit(1);
        }
    };
    eprintln!(
        "opened {} ({}) in {:?}: {} files, {} local keys",
        storage.build.version,
        storage.build_name,
        t.elapsed(),
        storage.files().len(),
        storage.local_key_count()
    );
    if let Some(i) = args.iter().position(|a| a == "--cat") {
        let path = &args[i + 1];
        match block_on(storage.read(path)) {
            Ok(bytes) => {
                use std::io::Write;
                std::io::stdout().write_all(&bytes).unwrap();
            }
            Err(e) => eprintln!("read failed: {e}"),
        }
        return;
    }
    let filter = args.get(1).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let out = args.iter().position(|a| a == "--out").map(|i| std::path::PathBuf::from(&args[i + 1]));
    for f in storage.files() {
        if f.path.to_ascii_lowercase().contains(&filter) {
            println!("{}\t{}", f.size, f.path);
            if let Some(dir) = &out {
                let dest = dir.join(&f.path);
                std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                match block_on(storage.read(&f.path)) {
                    Ok(bytes) => std::fs::write(dest, bytes).unwrap(),
                    Err(e) => eprintln!("{}: {e}", f.path),
                }
            }
        }
    }
}
