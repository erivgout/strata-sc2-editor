//! The `(attributes)` special file: per-block CRC32 / FILETIME / MD5 arrays.
//!
//! The SC2 editor writes version 100 with flags `CRC32 | MD5` (no FILETIME);
//! [`build`] does the same, which also keeps output deterministic.

use md5::{Digest, Md5};

pub const ATTR_VERSION: u32 = 100;
pub const ATTR_CRC32: u32 = 0x01;
pub const ATTR_FILETIME: u32 = 0x02;
pub const ATTR_MD5: u32 = 0x04;

#[derive(Default)]
pub struct Attributes {
    pub crcs: Option<Vec<u32>>,
    pub md5s: Option<Vec<[u8; 16]>>,
}

pub fn md5(data: &[u8]) -> [u8; 16] {
    Md5::digest(data).into()
}

/// Parse the arrays (indexed by block) that the file carries.
pub fn parse(data: &[u8], block_count: usize) -> Attributes {
    let mut out = Attributes::default();
    if data.len() < 8 {
        return out;
    }
    let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let flags = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if version != ATTR_VERSION {
        return out;
    }
    let body = &data[8..];
    let per_entry = if flags & ATTR_CRC32 != 0 { 4 } else { 0 }
        + if flags & ATTR_FILETIME != 0 { 8 } else { 0 }
        + if flags & ATTR_MD5 != 0 { 16 } else { 0 };
    if per_entry == 0 {
        return out;
    }
    // Some writers omit the entry for (attributes) itself.
    let n = block_count.min(body.len() / per_entry);
    let mut pos = 0;
    if flags & ATTR_CRC32 != 0 {
        out.crcs = Some(
            body[pos..pos + n * 4]
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        );
        pos += n * 4;
    }
    if flags & ATTR_FILETIME != 0 {
        pos += n * 8;
    }
    if flags & ATTR_MD5 != 0 {
        out.md5s = Some(
            body[pos..pos + n * 16]
                .chunks_exact(16)
                .map(|c| c.try_into().unwrap())
                .collect(),
        );
    }
    out
}

/// Build an `(attributes)` file with CRC32 and MD5 arrays, one entry per
/// block. The entry for `(attributes)` itself should be zeroed.
pub fn build(crcs: &[u32], md5s: &[[u8; 16]]) -> Vec<u8> {
    debug_assert_eq!(crcs.len(), md5s.len());
    let mut out = Vec::with_capacity(8 + crcs.len() * 20);
    out.extend_from_slice(&ATTR_VERSION.to_le_bytes());
    out.extend_from_slice(&(ATTR_CRC32 | ATTR_MD5).to_le_bytes());
    for crc in crcs {
        out.extend_from_slice(&crc.to_le_bytes());
    }
    for md5 in md5s {
        out.extend_from_slice(md5);
    }
    out
}
