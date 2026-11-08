//! MPQ hashing and encryption primitives (the classic Storm crypt table)
//! plus the Jenkins `hashlittle2` hash used by HET tables.

/// Hash types for [`hash_string`].
pub const HASH_TABLE_OFFSET: u32 = 0x000;
pub const HASH_NAME_A: u32 = 0x100;
pub const HASH_NAME_B: u32 = 0x200;
pub const HASH_FILE_KEY: u32 = 0x300;

/// `hash_string("(hash table)", HASH_FILE_KEY)`.
pub const KEY_HASH_TABLE: u32 = 0xC3AF_3770;
/// `hash_string("(block table)", HASH_FILE_KEY)`.
pub const KEY_BLOCK_TABLE: u32 = 0xEC83_B3A3;

const fn build_crypt_table() -> [u32; 0x500] {
    let mut table = [0u32; 0x500];
    let mut seed: u32 = 0x0010_0001;
    let mut i = 0;
    while i < 0x100 {
        let mut index = i;
        let mut j = 0;
        while j < 5 {
            seed = (seed * 125 + 3) % 0x2A_AAAB;
            let hi = (seed & 0xFFFF) << 16;
            seed = (seed * 125 + 3) % 0x2A_AAAB;
            let lo = seed & 0xFFFF;
            table[index] = hi | lo;
            index += 0x100;
            j += 1;
        }
        i += 1;
    }
    table
}

static CRYPT_TABLE: [u32; 0x500] = build_crypt_table();

/// Storm's normalisation for classic hashes: ASCII uppercase, `/` becomes `\`.
#[inline]
fn upper_slash(b: u8) -> u8 {
    if b == b'/' {
        b'\\'
    } else {
        b.to_ascii_uppercase()
    }
}

/// Storm `HashString`: case-insensitive, `/` and `\` equivalent.
pub fn hash_string(name: &str, hash_type: u32) -> u32 {
    let mut seed1: u32 = 0x7FED_7FED;
    let mut seed2: u32 = 0xEEEE_EEEE;
    for &b in name.as_bytes() {
        let ch = upper_slash(b) as u32;
        seed1 = CRYPT_TABLE[(hash_type + ch) as usize] ^ seed1.wrapping_add(seed2);
        seed2 = ch
            .wrapping_add(seed1)
            .wrapping_add(seed2)
            .wrapping_add(seed2 << 5)
            .wrapping_add(3);
    }
    seed1
}

/// Decrypt `data` in place. Only whole little-endian dwords are encrypted;
/// trailing bytes (len % 4) are left untouched, matching Storm.
pub fn decrypt(data: &mut [u8], key: u32) {
    let mut seed1 = key;
    let mut seed2: u32 = 0xEEEE_EEEE;
    for chunk in data.chunks_exact_mut(4) {
        seed2 = seed2.wrapping_add(CRYPT_TABLE[0x400 + (seed1 & 0xFF) as usize]);
        let value = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let plain = value ^ seed1.wrapping_add(seed2);
        seed1 = ((!seed1) << 0x15).wrapping_add(0x1111_1111) | (seed1 >> 0x0B);
        seed2 = plain
            .wrapping_add(seed2)
            .wrapping_add(seed2 << 5)
            .wrapping_add(3);
        chunk.copy_from_slice(&plain.to_le_bytes());
    }
}

/// Encrypt `data` in place (inverse of [`decrypt`]).
pub fn encrypt(data: &mut [u8], key: u32) {
    let mut seed1 = key;
    let mut seed2: u32 = 0xEEEE_EEEE;
    for chunk in data.chunks_exact_mut(4) {
        seed2 = seed2.wrapping_add(CRYPT_TABLE[0x400 + (seed1 & 0xFF) as usize]);
        let plain = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let value = plain ^ seed1.wrapping_add(seed2);
        seed1 = ((!seed1) << 0x15).wrapping_add(0x1111_1111) | (seed1 >> 0x0B);
        seed2 = plain
            .wrapping_add(seed2)
            .wrapping_add(seed2 << 5)
            .wrapping_add(3);
        chunk.copy_from_slice(&value.to_le_bytes());
    }
}

/// The encryption key of a file: hash of its bare name (after the last
/// path separator), optionally adjusted for `MPQ_FILE_FIX_KEY`.
pub fn file_key(name: &str, fix_key: bool, file_pos: u64, file_size: u32) -> u32 {
    let bare = name.rsplit(['\\', '/']).next().unwrap_or(name);
    let key = hash_string(bare, HASH_FILE_KEY);
    if fix_key {
        key.wrapping_add(file_pos as u32) ^ file_size
    } else {
        key
    }
}

#[inline]
fn mix(a: &mut u32, b: &mut u32, c: &mut u32) {
    *a = a.wrapping_sub(*c);
    *a ^= c.rotate_left(4);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a);
    *b ^= a.rotate_left(6);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b);
    *c ^= b.rotate_left(8);
    *b = b.wrapping_add(*a);
    *a = a.wrapping_sub(*c);
    *a ^= c.rotate_left(16);
    *c = c.wrapping_add(*b);
    *b = b.wrapping_sub(*a);
    *b ^= a.rotate_left(19);
    *a = a.wrapping_add(*c);
    *c = c.wrapping_sub(*b);
    *c ^= b.rotate_left(4);
    *b = b.wrapping_add(*a);
}

#[inline]
fn final_mix(a: &mut u32, b: &mut u32, c: &mut u32) {
    *c ^= *b;
    *c = c.wrapping_sub(b.rotate_left(14));
    *a ^= *c;
    *a = a.wrapping_sub(c.rotate_left(11));
    *b ^= *a;
    *b = b.wrapping_sub(a.rotate_left(25));
    *c ^= *b;
    *c = c.wrapping_sub(b.rotate_left(16));
    *a ^= *c;
    *a = a.wrapping_sub(c.rotate_left(4));
    *b ^= *a;
    *b = b.wrapping_sub(a.rotate_left(14));
    *c ^= *b;
    *c = c.wrapping_sub(b.rotate_left(24));
}

#[inline]
fn le32(k: &[u8]) -> u32 {
    u32::from_le_bytes([k[0], k[1], k[2], k[3]])
}

/// Bob Jenkins' lookup3 `hashlittle2`. `pc` is the primary seed/result,
/// `pb` the secondary.
pub fn hashlittle2(key: &[u8], pc: &mut u32, pb: &mut u32) {
    let init = 0xDEAD_BEEFu32
        .wrapping_add(key.len() as u32)
        .wrapping_add(*pc);
    let (mut a, mut b, mut c) = (init, init, init.wrapping_add(*pb));
    let mut k = key;
    while k.len() > 12 {
        a = a.wrapping_add(le32(&k[0..]));
        b = b.wrapping_add(le32(&k[4..]));
        c = c.wrapping_add(le32(&k[8..]));
        mix(&mut a, &mut b, &mut c);
        k = &k[12..];
    }
    if k.is_empty() {
        *pc = c;
        *pb = b;
        return;
    }
    let mut tail = [0u8; 12];
    tail[..k.len()].copy_from_slice(k);
    a = a.wrapping_add(le32(&tail[0..]));
    b = b.wrapping_add(le32(&tail[4..]));
    c = c.wrapping_add(le32(&tail[8..]));
    final_mix(&mut a, &mut b, &mut c);
    *pc = c;
    *pb = b;
}

/// 64-bit Jenkins name hash used by HET tables (lowercase, `/` -> `\`).
pub fn hash_jenkins(name: &str) -> u64 {
    let normalized: Vec<u8> = name
        .bytes()
        .map(|b| {
            if b == b'/' {
                b'\\'
            } else {
                b.to_ascii_lowercase()
            }
        })
        .collect();
    let mut primary: u32 = 1;
    let mut secondary: u32 = 2;
    hashlittle2(&normalized, &mut secondary, &mut primary);
    ((primary as u64) << 32) | secondary as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crypt_table_known_values() {
        assert_eq!(CRYPT_TABLE[0], 0x55C6_36E2);
        assert_eq!(CRYPT_TABLE[1], 0x02BE_0170);
    }

    #[test]
    fn table_keys() {
        assert_eq!(hash_string("(hash table)", HASH_FILE_KEY), KEY_HASH_TABLE);
        assert_eq!(hash_string("(block table)", HASH_FILE_KEY), KEY_BLOCK_TABLE);
    }

    #[test]
    fn hash_is_case_and_slash_insensitive() {
        for ht in [HASH_TABLE_OFFSET, HASH_NAME_A, HASH_NAME_B, HASH_FILE_KEY] {
            assert_eq!(
                hash_string("Base.SC2Data\\GameData/UnitData.xml", ht),
                hash_string("base.sc2data/gamedata\\unitdata.XML", ht)
            );
        }
        assert_eq!(hash_jenkins("A/B.txt"), hash_jenkins("a\\b.TXT"));
    }

    #[test]
    fn listfile_hashes() {
        // Well-known values for "(listfile)".
        assert_eq!(hash_string("(listfile)", HASH_TABLE_OFFSET), 0x5F3D_E859);
        assert_eq!(hash_string("(listfile)", HASH_NAME_A), 0xFD65_7910);
        assert_eq!(hash_string("(listfile)", HASH_NAME_B), 0x4E9B_98A7);
    }

    #[test]
    fn encrypt_roundtrip() {
        let original: Vec<u8> = (0..103u8).collect();
        let mut buf = original.clone();
        encrypt(&mut buf, 0x1234_5678);
        assert_ne!(buf[..100], original[..100]);
        assert_eq!(buf[100..], original[100..]);
        decrypt(&mut buf, 0x1234_5678);
        assert_eq!(buf, original);
    }

    #[test]
    fn lookup3_vectors() {
        let run = |s: &[u8], c: u32, b: u32| {
            let (mut c, mut b) = (c, b);
            hashlittle2(s, &mut c, &mut b);
            (c, b)
        };
        assert_eq!(run(b"", 0, 0), (0xDEAD_BEEF, 0xDEAD_BEEF));
        assert_eq!(run(b"", 0, 0xDEAD_BEEF), (0xBD5B_7DDE, 0xDEAD_BEEF));
        assert_eq!(
            run(b"", 0xDEAD_BEEF, 0xDEAD_BEEF),
            (0x9C09_3CCD, 0xBD5B_7DDE)
        );
        let s = b"Four score and seven years ago";
        assert_eq!(run(s, 0, 0), (0x1777_0551, 0xCE72_26E6));
        assert_eq!(run(s, 0, 1), (0xE360_7CAE, 0xBD37_1DE4));
        assert_eq!(run(s, 1, 0), (0xCD62_8161, 0x6CBE_A4B3));
    }
}
