//! Real-data test, gated on `STRATA_TEST_MAP`.
//!
//! `STRATA_TEST_MAP` is a `;`-separated list of `.SC2Map`/`.SC2Mod` archive
//! files or directories (searched recursively for archive files; component
//! folders named `*.SC2Map` are skipped). For every archive it:
//! - opens it and lists `(listfile)`,
//! - reads every file, checking CRC32s from `(attributes)` when present,
//! - re-reads every file through HET/BET (when present) and compares,
//! - rebuilds it with `ArchiveBuilder` and compares the round-tripped contents.
//!
//! Run: `STRATA_TEST_MAP="C:/path/to/Map.SC2Map" cargo test -p strata-mpq --release --test real_maps -- --nocapture`

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use strata_mpq::{Archive, ArchiveBuilder, MpqError, OpenOptions};

/// Histogram of block flags across all archives, plus user-data count.
static FLAGS: Mutex<BTreeMap<u32, usize>> = Mutex::new(BTreeMap::new());
static USER_DATA: Mutex<usize> = Mutex::new(0);

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_file() {
        out.push(path.to_owned());
    } else if path.is_dir() {
        let Ok(rd) = std::fs::read_dir(path) else {
            return;
        };
        let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            let ext = p
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            let archive_ext = matches!(
                ext.as_str(),
                "sc2map" | "sc2mod" | "sc2replay" | "mpq" | "s2ma" | "s2mod"
            );
            if p.is_file() && archive_ext {
                out.push(p);
            } else if p.is_dir() && !archive_ext {
                collect(&p, out);
            }
        }
    }
}

#[test]
fn real_maps() {
    let Ok(spec) = std::env::var("STRATA_TEST_MAP") else {
        eprintln!("STRATA_TEST_MAP not set; skipping real-map test");
        return;
    };
    let mut paths = Vec::new();
    for part in spec.split(';').filter(|s| !s.trim().is_empty()) {
        collect(Path::new(part.trim()), &mut paths);
    }
    assert!(!paths.is_empty(), "no archives found in STRATA_TEST_MAP");

    let mut failures = Vec::new();
    let mut skipped = 0;
    for path in &paths {
        match check_one(path) {
            Ok(true) => {}
            Ok(false) => skipped += 1,
            Err(e) => {
                eprintln!("FAIL {}: {e}", path.display());
                failures.push(path.clone());
            }
        }
    }
    eprintln!(
        "{} archives checked, {} skipped (not MPQ), {} failed",
        paths.len(),
        skipped,
        failures.len()
    );
    eprintln!(
        "archives with MPQ\x1B user data: {}",
        USER_DATA.lock().unwrap()
    );
    for (flags, count) in FLAGS.lock().unwrap().iter() {
        eprintln!("  flags {flags:08x}: {count} files");
    }
    assert!(failures.is_empty(), "failed: {failures:?}");
}

/// Ok(false) = skipped: tiny non-MPQ files (the Battle.net cache holds
/// small text stubs with archive extensions).
fn check_one(path: &Path) -> Result<bool, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let size = bytes.len();
    let t0 = Instant::now();
    let archive = match Archive::from_bytes(bytes.clone()) {
        Ok(a) => a,
        Err(MpqError::NotMpq) if size < 1024 => return Ok(false),
        Err(e) => return Err(format!("open: {e}")),
    };
    if archive.user_data().is_some() {
        *USER_DATA.lock().unwrap() += 1;
    }
    let names = archive.files();
    if names.is_empty() {
        return Err("empty listfile".into());
    }
    let mut total = 0usize;
    let mut crc_checked = 0usize;
    let mut contents = Vec::with_capacity(names.len());
    for name in &names {
        let data = archive
            .read(name)
            .map_err(|e| format!("read {name}: {e}"))?;
        let info = archive.file_info(name).unwrap();
        *FLAGS.lock().unwrap().entry(info.flags).or_default() += 1;
        if data.len() as u64 != info.size {
            return Err(format!("{name}: size {} != {}", data.len(), info.size));
        }
        if let Some(crc) = info.crc32.filter(|&c| c != 0) {
            let actual = crc32fast::hash(&data);
            if actual != crc {
                return Err(format!("{name}: crc {actual:08x} != {crc:08x}"));
            }
            crc_checked += 1;
        }
        total += data.len();
        contents.push((name.clone(), data));
    }
    let read_time = t0.elapsed();

    let het = Archive::from_bytes_with_options(
        bytes,
        &OpenOptions {
            prefer_het_bet: true,
        },
    )
    .map_err(|e| format!("open (HET/BET): {e}"))?;
    if het.uses_het_bet() {
        if het.files() != names {
            return Err("HET/BET listing differs from classic".into());
        }
        for (name, data) in &contents {
            let d = het
                .read(name)
                .map_err(|e| format!("HET read {name}: {e}"))?;
            if &d != data {
                return Err(format!("HET/BET content differs for {name}"));
            }
        }
    }

    let t1 = Instant::now();
    let mut builder = ArchiveBuilder::new();
    for (name, data) in &contents {
        builder.add(name, data.clone());
    }
    let rebuilt = builder.build().map_err(|e| format!("build: {e}"))?;
    let build_time = t1.elapsed();
    let rebuilt_len = rebuilt.len();
    let again = Archive::from_bytes(rebuilt).map_err(|e| format!("reopen: {e}"))?;
    let mut again_names = again.files();
    let mut want: Vec<String> = names.clone();
    again_names.sort_by_key(|n| n.to_ascii_uppercase());
    want.sort_by_key(|n| n.to_ascii_uppercase());
    if again_names != want {
        return Err("rebuilt listing differs".into());
    }
    for (name, data) in &contents {
        let d = again
            .read(name)
            .map_err(|e| format!("rebuilt read {name}: {e}"))?;
        if &d != data {
            return Err(format!("rebuilt content differs for {name}"));
        }
        let info = again.file_info(name).unwrap();
        if info.crc32 != Some(crc32fast::hash(data)) {
            return Err(format!("rebuilt attributes crc wrong for {name}"));
        }
    }

    eprintln!(
        "ok  {} | v{} sector {} | {} files, {} bytes unpacked, {} CRCs verified | het/bet: {} | read {:?} | rebuilt {} -> {} bytes in {:?}",
        path.display(),
        archive.format_version() + 1,
        archive.sector_size(),
        names.len(),
        total,
        crc_checked,
        if het.uses_het_bet() { "verified" } else { "absent" },
        read_time,
        size,
        rebuilt_len,
        build_time,
    );
    Ok(true)
}
