//! List an archive's files with sizes, flags and read status.
//!
//! `cargo run -p strata-mpq --example mpq_info -- <archive> [name-to-extract out-path]`
//! `cargo run -p strata-mpq --example mpq_info -- <archive> --rebuild <out-archive>`

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: mpq_info <archive> [name out-path]");
        std::process::exit(2);
    };
    let bytes = std::fs::read(path).expect("read archive");
    let archive = match strata_mpq::Archive::from_bytes(bytes) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("open failed: {e}");
            std::process::exit(1);
        }
    };
    match &args[..] {
        [_, flag, out] if flag == "--rebuild" => {
            let mut builder = strata_mpq::ArchiveBuilder::new();
            for name in archive.file_names() {
                builder.add(name, archive.read(name).expect("read file"));
            }
            std::fs::write(out, builder.build().expect("build")).expect("write output");
            return;
        }
        [_, name, out] => {
            let data = archive.read(name).expect("read file");
            std::fs::write(out, data).expect("write output");
            return;
        }
        _ => {}
    }
    println!(
        "format v{}, sector size {}, header at {:#x}, user data {} bytes, {} blocks, het/bet {}",
        archive.format_version() + 1,
        archive.sector_size(),
        archive.header_offset(),
        archive.user_data().map_or(0, |u| u.len()),
        archive.block_count(),
        archive.uses_het_bet(),
    );
    for info in archive.entries() {
        let status = match archive.read(&info.name) {
            Ok(_) => "ok".to_owned(),
            Err(e) => format!("ERR {e}"),
        };
        println!(
            "{:>10} {:>10} {:08x} {:>8} {} [{}]",
            info.size,
            info.compressed_size,
            info.flags,
            info.crc32.map_or("-".to_owned(), |c| format!("{c:08x}")),
            info.name,
            status
        );
    }
}
