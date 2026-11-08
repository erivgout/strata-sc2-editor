//! The `(attributes)` special file: per-block CRC32 / FILETIME / MD5.

pub const ATTR_VERSION: u32 = 100;
pub const ATTR_CRC32: u32 = 0x01;
pub const ATTR_FILETIME: u32 = 0x02;

/// Extract the CRC32 array (indexed by block) if the file carries one.
pub fn parse_crcs(data: &[u8], block_count: usize) -> Option<Vec<u32>> {
    if data.len() < 8 {
        return None;
    }
    let version = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    let flags = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
    if version != ATTR_VERSION || flags & ATTR_CRC32 == 0 {
        return None;
    }
    // Some writers omit the entry for (attributes) itself; take what exists.
    let body = &data[8..];
    let count = block_count.min(body.len() / 4);
    Some(
        body[..count * 4]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

/// Build an `(attributes)` file with CRC32 and FILETIME arrays. `crcs` has
/// one entry per block; FILETIMEs are written as zero for deterministic output.
pub fn build(crcs: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + crcs.len() * 12);
    out.extend_from_slice(&ATTR_VERSION.to_le_bytes());
    out.extend_from_slice(&(ATTR_CRC32 | ATTR_FILETIME).to_le_bytes());
    for crc in crcs {
        out.extend_from_slice(&crc.to_le_bytes());
    }
    for _ in crcs {
        out.extend_from_slice(&0u64.to_le_bytes());
    }
    out
}
