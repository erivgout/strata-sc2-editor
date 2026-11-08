//! HET (hash extended) and BET (block extended) tables, MPQ format v3+.

use crate::crypt::hash_jenkins;
use crate::read::BlockEntry;
use crate::MpqError;

/// Little-endian bit array reader (Storm `TBitArray`).
struct BitArray<'a>(&'a [u8]);

impl BitArray<'_> {
    fn get(&self, bit: u64, count: u32) -> Option<u64> {
        if count == 0 {
            return Some(0);
        }
        if count > 64 {
            return None;
        }
        let end = bit.checked_add(count as u64)?;
        if end.div_ceil(8) > self.0.len() as u64 {
            return None;
        }
        let mut value = 0u64;
        let mut done = 0u32;
        let mut pos = bit;
        while done < count {
            let byte = self.0[(pos / 8) as usize] as u64;
            let shift = (pos % 8) as u32;
            let take = (8 - shift).min(count - done);
            let bits = (byte >> shift) & ((1u64 << take) - 1);
            value |= bits << done;
            done += take;
            pos += take as u64;
        }
        Some(value)
    }
}

fn u32_at(d: &[u8], off: usize) -> Result<u32, MpqError> {
    d.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or_else(|| MpqError::Corrupt("extended table truncated".into()))
}

pub struct HetTable {
    name_hash_bits: u32,
    name_hashes: Vec<u8>,
    index_size_total: u32,
    index_size: u32,
    index_bits: Vec<u8>,
}

/// `data` is the decrypted, decompressed table body after the 12-byte
/// extended header.
pub fn parse_het(data: &[u8]) -> Result<HetTable, MpqError> {
    let total_count = u32_at(data, 8)? as usize;
    let name_hash_bits = u32_at(data, 12)?;
    let index_size_total = u32_at(data, 16)?;
    let index_size = u32_at(data, 24)?;
    let index_table_size = u32_at(data, 28)? as usize;
    if !(8..=64).contains(&name_hash_bits) || index_size > 32 || total_count == 0 {
        return Err(MpqError::Corrupt("bad HET header".into()));
    }
    let hashes_start = 32;
    let name_hashes = data
        .get(hashes_start..hashes_start + total_count)
        .ok_or_else(|| MpqError::Corrupt("HET hashes truncated".into()))?
        .to_vec();
    let idx_start = hashes_start + total_count;
    let index_bits = data
        .get(idx_start..(idx_start + index_table_size).min(data.len()))
        .unwrap_or(&[])
        .to_vec();
    Ok(HetTable {
        name_hash_bits,
        name_hashes,
        index_size_total,
        index_size,
        index_bits,
    })
}

pub struct BetTable {
    pub blocks: Vec<BlockEntry>,
    name_hash2: Vec<u64>,
}

pub fn parse_bet(data: &[u8]) -> Result<BetTable, MpqError> {
    let entry_count = u32_at(data, 4)? as usize;
    let entry_size = u32_at(data, 12)? as u64;
    let idx = |i: usize| u32_at(data, 16 + i * 4);
    let (bi_pos, bi_fsize, bi_csize, bi_flag) = (idx(0)?, idx(1)?, idx(2)?, idx(3)?);
    let (bc_pos, bc_fsize, bc_csize, bc_flag) = (idx(5)?, idx(6)?, idx(7)?, idx(8)?);
    let hash2_total = u32_at(data, 56)? as u64;
    let hash2_count = u32_at(data, 64)?;
    let hash_array_size = u32_at(data, 68)? as usize;
    let flag_count = u32_at(data, 72)? as usize;
    let flags_start = 76usize;
    let mut flags = Vec::with_capacity(flag_count.min(1 << 16));
    for i in 0..flag_count {
        flags.push(u32_at(data, flags_start + i * 4)?);
    }
    let table_start = flags_start + flag_count * 4;
    let truncated = || MpqError::Corrupt("BET table truncated".into());
    if entry_size == 0 && entry_count > 0 {
        return Err(MpqError::Corrupt("BET entry size is zero".into()));
    }
    let table_len =
        usize::try_from((entry_size * entry_count as u64).div_ceil(8)).map_err(|_| truncated())?;
    let hash_start = table_start.checked_add(table_len).ok_or_else(truncated)?;
    let table = data.get(table_start..hash_start).ok_or_else(truncated)?;
    let hashes = data
        .get(hash_start..hash_start.saturating_add(hash_array_size).min(data.len()))
        .unwrap_or(&[]);
    let table = BitArray(table);
    let hashes = BitArray(hashes);
    let bad = || MpqError::Corrupt("BET entry out of range".into());

    let mut blocks = Vec::with_capacity(entry_count);
    let mut name_hash2 = Vec::with_capacity(entry_count);
    for i in 0..entry_count {
        let base = i as u64 * entry_size;
        let pos = table.get(base + bi_pos as u64, bc_pos).ok_or_else(bad)?;
        let fsize = table
            .get(base + bi_fsize as u64, bc_fsize)
            .ok_or_else(bad)?;
        let csize = table
            .get(base + bi_csize as u64, bc_csize)
            .ok_or_else(bad)?;
        let flag_index = table.get(base + bi_flag as u64, bc_flag).ok_or_else(bad)? as usize;
        let flags = flags.get(flag_index).copied().unwrap_or(crate::FLAG_EXISTS);
        blocks.push(BlockEntry {
            pos,
            csize,
            fsize,
            flags,
        });
        name_hash2.push(hashes.get(i as u64 * hash2_total, hash2_count).unwrap_or(0));
    }
    Ok(BetTable { blocks, name_hash2 })
}

impl HetTable {
    /// Find the BET index of `name`.
    pub fn lookup(&self, name: &str, bet: &BetTable) -> Option<usize> {
        let bits = self.name_hash_bits;
        let and_mask = if bits == 64 {
            u64::MAX
        } else {
            (1u64 << bits) - 1
        };
        let or_mask = 1u64 << (bits - 1);
        let hash = (hash_jenkins(name) & and_mask) | or_mask;
        let hash1 = (hash >> (bits - 8)) as u8;
        let hash2 = hash & (and_mask >> 8);
        let total = self.name_hashes.len();
        let start = (hash % total as u64) as usize;
        let index_bits = BitArray(&self.index_bits);
        for step in 0..total {
            let slot = (start + step) % total;
            let h = self.name_hashes[slot];
            if h == 0 {
                return None;
            }
            if h == hash1 {
                let bit = slot as u64 * self.index_size_total as u64;
                if let Some(index) = index_bits.get(bit, self.index_size) {
                    let index = index as usize;
                    if bet.name_hash2.get(index) == Some(&hash2) {
                        return Some(index);
                    }
                }
            }
        }
        None
    }
}
