use strata_mpq::crypt::{self, hash_string};
use strata_mpq::{
    Archive, ArchiveBuilder, MpqError, FLAG_COMPRESS, FLAG_ENCRYPTED, FLAG_EXISTS, FLAG_FIX_KEY,
    FLAG_SINGLE_UNIT,
};

/// Deterministic pseudo-random bytes (xorshift).
fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed | 1;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

fn text(len: usize, seed: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len + 64);
    let mut i = seed;
    while v.len() < len {
        v.extend_from_slice(format!("<Unit id=\"Marine{i}\" value=\"{}\"/>\n", i * 7).as_bytes());
        i += 1;
    }
    v.truncate(len);
    v
}

fn sample_files() -> Vec<(String, Vec<u8>)> {
    let mut files = vec![
        ("Empty.txt".to_owned(), Vec::new()),
        (
            "Base.SC2Data/GameData/UnitData.xml".to_owned(),
            text(300_000, 0),
        ),
        (
            "enUS.SC2Data\\LocalizedData\\GameStrings.txt".to_owned(),
            text(40_000, 9),
        ),
        ("MapScript.galaxy".to_owned(), text(16_384, 3)), // exactly one sector
        ("Minimap.tga".to_owned(), noise(16_385, 5)),     // one byte over a sector
        ("Noise.bin".to_owned(), noise(1_000_000, 7)),    // incompressible, many sectors
        ("t3HeightMap".to_owned(), vec![0u8; 2_000_000]), // highly compressible
        ("one".to_owned(), vec![42]),
    ];
    for i in 0..150 {
        let len = (i * 997) % 50_000;
        let data = if i % 3 == 0 {
            noise(len, i as u32)
        } else {
            text(len, i)
        };
        files.push((format!("Dir{}/Sub/File{i:03}.dat", i % 5), data));
    }
    files
}

#[test]
fn hash_vectors() {
    assert_eq!(hash_string("(hash table)", 0x300), 0xC3AF_3770);
    assert_eq!(hash_string("(block table)", 0x300), 0xEC83_B3A3);
}

#[test]
fn roundtrip_many_files() {
    let files = sample_files();
    let mut b = ArchiveBuilder::new();
    for (n, d) in &files {
        b.add(n, d.clone());
    }
    let bytes = b.build().unwrap();
    let a = Archive::from_bytes(bytes).unwrap();
    assert_eq!(a.format_version(), 2);
    assert!(!a.uses_het_bet());

    let listed = a.files();
    assert_eq!(listed.len(), files.len());
    assert!(listed.iter().all(|n| !n.contains('\\')));
    let mut sorted = listed.clone();
    sorted.sort_by_key(|n| n.to_ascii_uppercase());
    assert_eq!(listed, sorted, "builder writes files in sorted order");

    for (name, data) in &files {
        let fwd = name.replace('\\', "/");
        let back = name.replace('/', "\\");
        assert!(listed.contains(&fwd), "{fwd} listed");
        assert_eq!(&a.read(&fwd).unwrap(), data, "{fwd}");
        assert_eq!(&a.read(&back).unwrap(), data, "{back}");
        assert_eq!(&a.read(&fwd.to_ascii_lowercase()).unwrap(), data);
        assert!(a.contains(&back.to_ascii_uppercase()));
        let info = a.file_info(name).unwrap();
        assert_eq!(info.size, data.len() as u64);
        assert_eq!(info.crc32, Some(crc32fast::hash(data)));
    }
    // Incompressible data is stored; compressible data is compressed.
    assert_eq!(a.file_info("Noise.bin").unwrap().flags, FLAG_EXISTS);
    let hm = a.file_info("t3HeightMap").unwrap();
    assert_eq!(hm.flags, FLAG_EXISTS | FLAG_COMPRESS);
    assert!(hm.compressed_size < 50_000);
    assert!(a.contains("(listfile)"));
    assert!(a.contains("(attributes)"));
    assert!(matches!(a.read("missing.txt"), Err(MpqError::NotFound(_))));
}

#[test]
fn deterministic_and_order_independent() {
    let files = sample_files();
    let build = |rev: bool| {
        let mut b = ArchiveBuilder::new();
        let iter: Box<dyn Iterator<Item = &(String, Vec<u8>)>> = if rev {
            Box::new(files.iter().rev())
        } else {
            Box::new(files.iter())
        };
        for (n, d) in iter {
            b.add(n, d.clone());
        }
        b.build().unwrap()
    };
    assert_eq!(build(false), build(true));
}

#[test]
fn duplicates_last_wins_and_reserved_names_ignored() {
    let mut b = ArchiveBuilder::new();
    b.add("A/b.txt", b"first".to_vec());
    b.add("a\\B.TXT", b"second".to_vec());
    b.add("(listfile)", b"bogus".to_vec());
    let a = Archive::from_bytes(b.build().unwrap()).unwrap();
    assert_eq!(a.files(), vec!["a/B.TXT".to_owned()]);
    assert_eq!(a.read("a/b.txt").unwrap(), b"second");
    assert_eq!(a.read("(listfile)").unwrap(), b"a\\B.TXT\r\n");
}

#[test]
fn empty_archive() {
    let a = Archive::from_bytes(ArchiveBuilder::new().build().unwrap()).unwrap();
    assert!(a.files().is_empty());
}

#[test]
fn options_small_sectors_no_attributes_stored() {
    let data = text(100_000, 1);
    let mut b = ArchiveBuilder::new()
        .with_sector_size_shift(0)
        .with_attributes(false);
    b.add("x.txt", data.clone());
    let a = Archive::from_bytes(b.build().unwrap()).unwrap();
    assert_eq!(a.sector_size(), 512);
    assert_eq!(a.read("x.txt").unwrap(), data);
    assert!(!a.contains("(attributes)"));
    assert_eq!(a.file_info("x.txt").unwrap().crc32, None);

    let mut b = ArchiveBuilder::new().with_compression_level(0);
    b.add("x.txt", data.clone());
    let a = Archive::from_bytes(b.build().unwrap()).unwrap();
    assert_eq!(a.file_info("x.txt").unwrap().flags, FLAG_EXISTS);
    assert_eq!(a.read("x.txt").unwrap(), data);
}

#[test]
fn user_data_header() {
    let mut b = ArchiveBuilder::new().with_user_data(b"StarCraft II replay\x1b11".to_vec());
    b.add("replay.details", text(5000, 2));
    let bytes = b.build().unwrap();
    assert_eq!(&bytes[..4], b"MPQ\x1B");
    let a = Archive::from_bytes(bytes).unwrap();
    assert_eq!(a.header_offset(), 512);
    assert_eq!(a.user_data().unwrap(), b"StarCraft II replay\x1b11");
    assert_eq!(a.read("replay.details").unwrap(), text(5000, 2));
}

#[test]
fn header_found_at_aligned_offset_without_user_data() {
    let mut b = ArchiveBuilder::new();
    b.add("f", b"hello".to_vec());
    let mut bytes = vec![0xAAu8; 1024];
    bytes.extend_from_slice(&b.build().unwrap());
    let a = Archive::from_bytes(bytes).unwrap();
    assert_eq!(a.header_offset(), 1024);
    assert_eq!(a.read("f").unwrap(), b"hello");
}

#[test]
fn not_mpq() {
    assert!(matches!(
        Archive::from_bytes(vec![0; 4096]),
        Err(MpqError::NotMpq)
    ));
    assert!(matches!(
        Archive::from_bytes(Vec::new()),
        Err(MpqError::NotMpq)
    ));
}

// ---- Hand-crafted archives exercising reader paths the builder never emits.

struct Craft {
    name: &'static str,
    data: Vec<u8>,
    flags: u32,
}

/// Build a v2 archive (with an all-zero hi-block table) whose files use the
/// given flags, encoding them independently of `ArchiveBuilder`.
fn craft(files: &[Craft], sector_size: usize, shift: u16) -> Vec<u8> {
    let header_size = 44usize;
    let mut out = vec![0u8; header_size];
    let mut blocks = Vec::new();
    for f in files {
        let pos = out.len() as u32;
        let encrypted = f.flags & FLAG_ENCRYPTED != 0;
        let key = crypt::file_key(
            f.name,
            f.flags & FLAG_FIX_KEY != 0,
            pos as u64,
            f.data.len() as u32,
        );
        let mut body = Vec::new();
        if f.flags & FLAG_SINGLE_UNIT != 0 {
            let mut z = vec![0x02];
            z.extend(miniz_oxide::deflate::compress_to_vec_zlib(&f.data, 6));
            body = if f.flags & FLAG_COMPRESS != 0 {
                z
            } else {
                f.data.clone()
            };
            if encrypted {
                crypt::encrypt(&mut body, key);
            }
        } else if f.flags & FLAG_COMPRESS != 0 {
            let n = f.data.len().div_ceil(sector_size);
            let mut offsets = vec![((n + 1) * 4) as u32];
            let mut sectors = Vec::new();
            for (i, chunk) in f.data.chunks(sector_size).enumerate() {
                let mut s = vec![0x02];
                s.extend(miniz_oxide::deflate::compress_to_vec_zlib(chunk, 6));
                if s.len() >= chunk.len() {
                    s = chunk.to_vec();
                }
                if encrypted {
                    crypt::encrypt(&mut s, key.wrapping_add(i as u32));
                }
                sectors.extend_from_slice(&s);
                offsets.push(((n + 1) * 4 + sectors.len()) as u32);
            }
            let mut table: Vec<u8> = offsets.iter().flat_map(|o| o.to_le_bytes()).collect();
            if encrypted {
                crypt::encrypt(&mut table, key.wrapping_sub(1));
            }
            body.extend_from_slice(&table);
            body.extend_from_slice(&sectors);
        } else {
            body = f.data.clone();
            if encrypted {
                for (i, c) in body.chunks_mut(sector_size).enumerate() {
                    crypt::encrypt(c, key.wrapping_add(i as u32));
                }
            }
        }
        blocks.push([pos, body.len() as u32, f.data.len() as u32, f.flags]);
        out.extend_from_slice(&body);
    }
    let hash_count = 16usize;
    let mut hash = vec![0xFFu8; hash_count * 16];
    for (i, f) in files.iter().enumerate() {
        let mut slot = hash_string(f.name, 0) as usize & (hash_count - 1);
        while hash[slot * 16 + 12..slot * 16 + 16] != [0xFF; 4] {
            slot = (slot + 1) & (hash_count - 1);
        }
        let e = &mut hash[slot * 16..slot * 16 + 16];
        e[0..4].copy_from_slice(&hash_string(f.name, 0x100).to_le_bytes());
        e[4..8].copy_from_slice(&hash_string(f.name, 0x200).to_le_bytes());
        e[8..12].copy_from_slice(&[0; 4]);
        e[12..16].copy_from_slice(&(i as u32).to_le_bytes());
    }
    crypt::encrypt(&mut hash, crypt::KEY_HASH_TABLE);
    let hash_pos = out.len() as u32;
    out.extend_from_slice(&hash);
    let mut table: Vec<u8> = blocks
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    crypt::encrypt(&mut table, crypt::KEY_BLOCK_TABLE);
    let block_pos = out.len() as u32;
    out.extend_from_slice(&table);
    let hi_pos = out.len() as u64;
    out.extend(std::iter::repeat_n(0u8, files.len() * 2));
    let size = out.len() as u32;

    out[0..4].copy_from_slice(b"MPQ\x1A");
    out[4..8].copy_from_slice(&(header_size as u32).to_le_bytes());
    out[8..12].copy_from_slice(&size.to_le_bytes());
    out[12..14].copy_from_slice(&1u16.to_le_bytes());
    out[14..16].copy_from_slice(&shift.to_le_bytes());
    out[16..20].copy_from_slice(&hash_pos.to_le_bytes());
    out[20..24].copy_from_slice(&block_pos.to_le_bytes());
    out[24..28].copy_from_slice(&(hash_count as u32).to_le_bytes());
    out[28..32].copy_from_slice(&(files.len() as u32).to_le_bytes());
    out[32..40].copy_from_slice(&hi_pos.to_le_bytes());
    out
}

#[test]
fn encrypted_single_unit_and_fix_key_files() {
    let e = FLAG_EXISTS;
    let files = vec![
        Craft { name: "(listfile)", data: b"enc\\sectored.txt\r\nenc/fixkey.txt\nsingle.txt;single-enc.txt\nstored-enc.bin\r\nmissing.txt\r\n".to_vec(), flags: e | FLAG_COMPRESS },
        Craft { name: "enc\\sectored.txt", data: text(10_000, 4), flags: e | FLAG_COMPRESS | FLAG_ENCRYPTED },
        Craft { name: "enc\\fixkey.txt", data: text(9_000, 5), flags: e | FLAG_COMPRESS | FLAG_ENCRYPTED | FLAG_FIX_KEY },
        Craft { name: "single.txt", data: text(7_000, 6), flags: e | FLAG_COMPRESS | FLAG_SINGLE_UNIT },
        Craft { name: "single-enc.txt", data: text(7_001, 7), flags: e | FLAG_COMPRESS | FLAG_SINGLE_UNIT | FLAG_ENCRYPTED | FLAG_FIX_KEY },
        Craft { name: "stored-enc.bin", data: noise(5_003, 8), flags: e | FLAG_ENCRYPTED },
    ];
    // 4 KiB sectors so the sectored files span several sectors.
    let a = Archive::from_bytes(craft(&files, 4096, 3)).unwrap();
    assert_eq!(a.format_version(), 1);
    assert_eq!(
        a.files(),
        vec![
            "enc/sectored.txt",
            "enc/fixkey.txt",
            "single.txt",
            "single-enc.txt",
            "stored-enc.bin"
        ]
    );
    for f in &files[1..] {
        assert_eq!(a.read(f.name).unwrap(), f.data, "{}", f.name);
    }
}
