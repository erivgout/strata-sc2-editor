//! Local index files (`data/XXVVVVVVVV.idx`, revision 7).
//!
//! Each of the 16 buckets has one or more versions; the highest version wins.
//! Layout (little-endian unless noted):
//! - `u32 header_block_size, u32 header_hash`
//! - `u16 revision (7), u8 bucket, u8 flags, u8 size_bytes (4),
//!   u8 offset_bytes (5), u8 key_bytes (9), u8 segment_bits (30), u64 max_size`
//! - padding to 0x20, then `u32 entries_size, u32 entries_hash`
//! - `entries_size / 18` entries: `[u8; 9] ekey prefix, u40 BE location,
//!   u32 encoded_size`. The location's top bits are the `data.###` number,
//!   the low `segment_bits` bits the offset inside it.

use std::collections::HashMap;

use crate::CascError;

/// Where an encoded file lives inside the `data.###` archives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub archive: u32,
    pub offset: u32,
    /// Encoded size, including the 30-byte local header in front of BLTE.
    pub size: u32,
}

/// The merged contents of every bucket's newest index file.
#[derive(Debug, Default)]
pub struct LocalIndex {
    entries: HashMap<[u8; 9], Location>,
}

impl LocalIndex {
    /// Picks the newest `.idx` per bucket from a directory listing.
    pub fn select_files(names: &[String]) -> Vec<String> {
        let mut best: HashMap<u8, (u32, &String)> = HashMap::new();
        for name in names {
            let lower = name.to_ascii_lowercase();
            let Some(stem) = lower.strip_suffix(".idx") else {
                continue;
            };
            if stem.len() != 10 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            let (Ok(bucket), Ok(version)) = (u8::from_str_radix(&stem[..2], 16), u32::from_str_radix(&stem[2..], 16))
            else {
                continue;
            };
            if bucket > 0x0F {
                continue;
            }
            let slot = best.entry(bucket).or_insert((version, name));
            if version >= slot.0 {
                *slot = (version, name);
            }
        }
        let mut out: Vec<String> = best.into_values().map(|(_, n)| n.clone()).collect();
        out.sort();
        out
    }

    /// Adds the entries of one index file.
    pub fn add_file(&mut self, data: &[u8]) -> Result<(), CascError> {
        let bad = |m: &str| CascError::Format(format!("index file: {m}"));
        if data.len() < 0x28 {
            return Err(bad("too short"));
        }
        let header_size = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let h = &data[8..];
        let revision = u16::from_le_bytes([h[0], h[1]]);
        let (size_bytes, offset_bytes, key_bytes, segment_bits) = (h[4], h[5], h[6], h[7]);
        if revision != 7
            || size_bytes != 4
            || offset_bytes != 5
            || key_bytes != 9
            || segment_bits == 0
            || segment_bits > 39
        {
            return Err(bad(&format!("unsupported layout (revision {revision})")));
        }
        let block = (8 + header_size + 0x0F) & !0x0F;
        let entries_size = data
            .get(block..block + 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize)
            .ok_or_else(|| bad("missing entry block"))?;
        let start = block + 8;
        let end = (start + entries_size).min(data.len());
        let mask = (1u64 << segment_bits) - 1;
        self.entries.reserve((end - start) / 18);
        for e in data[start..end].chunks_exact(18) {
            let mut key = [0u8; 9];
            key.copy_from_slice(&e[..9]);
            let loc = u64::from_be_bytes([0, 0, 0, e[9], e[10], e[11], e[12], e[13]]);
            let size = u32::from_le_bytes(e[14..18].try_into().unwrap());
            self.entries.entry(key).or_insert(Location {
                archive: (loc >> segment_bits) as u32,
                offset: (loc & mask) as u32,
                size,
            });
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn find(&self, ekey: &[u8; 16]) -> Option<Location> {
        let mut key = [0u8; 9];
        key.copy_from_slice(&ekey[..9]);
        self.entries.get(&key).copied()
    }
}

/// Builds a revision-7 index file for tests.
#[cfg(test)]
pub(crate) fn build_idx(bucket: u8, entries: &[([u8; 16], Location)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&7u16.to_le_bytes());
    out.extend_from_slice(&[bucket, 0, 4, 5, 9, 30]);
    out.extend_from_slice(&0x4000_0000u64.to_le_bytes());
    out.resize(0x20, 0);
    out.extend_from_slice(&((entries.len() * 18) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for (ekey, loc) in entries {
        out.extend_from_slice(&ekey[..9]);
        let packed = ((loc.archive as u64) << 30) | loc.offset as u64;
        out.extend_from_slice(&packed.to_be_bytes()[3..]);
        out.extend_from_slice(&loc.size.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_version_per_bucket() {
        let names: Vec<String> = [
            "0000000016.idx",
            "0000000017.idx",
            "0100000003.IDX",
            "readme.txt",
            "1000000001.idx",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            LocalIndex::select_files(&names),
            vec!["0000000017.idx", "0100000003.IDX"]
        );
    }

    #[test]
    fn parses_entries() {
        let key = [7u8; 16];
        let loc = Location {
            archive: 25,
            offset: 0x1234_5678 & 0x3FFF_FFFF,
            size: 999,
        };
        let mut idx = LocalIndex::default();
        idx.add_file(&build_idx(3, &[(key, loc)])).unwrap();
        assert_eq!(idx.len(), 1);
        assert_eq!(idx.find(&key), Some(loc));
        assert_eq!(idx.find(&[8u8; 16]), None);
    }
}
