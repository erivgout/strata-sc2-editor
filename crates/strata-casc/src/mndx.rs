//! The MNDX root file StarCraft II (and Heroes of the Storm) use to map file
//! names to content keys.
//!
//! This is a port of the algorithm in CascLib's `CascRootFile_MNDX.cpp`
//! (Ladislav Zezula, MIT), which reverse-engineered it from the game client.
//! The rank/select queries the original answers with lookup tables are
//! computed here from per-word prefix popcounts, which gives the same answers
//! with far less code.
//!
//! # Layout
//!
//! The root file holds three "MAR" name databases (package names, names with
//! the package stripped, full names) and an array of 24-byte CKey entries
//! `{ u32 flags, [u8; 16] ckey, u32 size }`. Entries are grouped by stripped
//! name index; bit 31 of `flags` ends a group and the low 24 bits are the
//! package index. A file's full path is `<package>/<stripped name>`.
//!
//! A MAR database is a LOUDS-encoded trie: node `n`'s children are the run
//! of set bits after the `n`-th zero of the "collision table" bit array, and
//! the child at bit position `p` has node index `p - n - 1`. A node's edge
//! label is either one character (`lo_bits[n]`) or, when `hi_bits_idx` has
//! bit `n` set, a multi-character fragment whose offset is
//! `hi_bits[rank(n)] << 8 | lo_bits[n]`. Fragments live either in a plain
//! string table or, recursively, in a child database whose trie stores them
//! as paths read from a node up towards its root. A node that ends a name has
//! its bit set in `file_names`, and the name's index is that bit's rank.

use crate::CascError;

/// A bit array with rank/select support.
#[derive(Debug, Default)]
struct BitArray {
    words: Vec<u32>,
    total: u32,
    valid: u32,
    /// `ranks[i]` = number of set bits in `words[..i]`.
    ranks: Vec<u32>,
}

impl BitArray {
    fn new(words: Vec<u32>, total: u32, valid: u32) -> BitArray {
        let mut ranks = Vec::with_capacity(words.len() + 1);
        let mut acc = 0u32;
        for w in &words {
            ranks.push(acc);
            acc += w.count_ones();
        }
        ranks.push(acc);
        BitArray {
            words,
            total,
            valid,
            ranks,
        }
    }

    fn is_empty(&self) -> bool {
        self.total == 0
    }

    fn get(&self, i: u32) -> bool {
        self.words
            .get((i >> 5) as usize)
            .is_some_and(|w| w >> (i & 31) & 1 != 0)
    }

    /// Number of set bits before position `i`.
    fn rank1(&self, i: u32) -> u32 {
        let w = (i >> 5) as usize;
        match self.words.get(w) {
            Some(word) => self.ranks[w] + (word & ((1u32 << (i & 31)) - 1)).count_ones(),
            None => *self.ranks.last().unwrap_or(&0),
        }
    }

    /// Position of the `k`-th (0-based) set bit.
    fn select1(&self, k: u32) -> Option<u32> {
        // Last word whose prefix count is <= k.
        let w = self.ranks[..self.words.len()]
            .partition_point(|&r| r <= k)
            .checked_sub(1)?;
        let within = k - self.ranks[w];
        let word = self.words[w];
        if within >= word.count_ones() {
            return None;
        }
        Some((w as u32) * 32 + nth_set_bit(word, within))
    }

    /// Position of the `k`-th (0-based) clear bit.
    fn select0(&self, k: u32) -> Option<u32> {
        let zeros = |w: usize| w as u32 * 32 - self.ranks[w];
        let n = self.words.len();
        // Binary search for the last word whose prefix zero count is <= k.
        let (mut lo, mut hi) = (0usize, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if zeros(mid) <= k {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let w = lo.checked_sub(1)?;
        let within = k - zeros(w);
        let word = !self.words[w];
        if within >= word.count_ones() {
            return None;
        }
        Some((w as u32) * 32 + nth_set_bit(word, within))
    }
}

fn nth_set_bit(mut word: u32, mut n: u32) -> u32 {
    let mut pos = 0;
    loop {
        let tz = word.trailing_zeros();
        pos += tz;
        word >>= tz;
        if n == 0 {
            return pos;
        }
        n -= 1;
        word >>= 1;
        pos += 1;
    }
}

/// A packed array of fixed-width entries.
#[derive(Debug, Default)]
struct BitEntryArray {
    words: Vec<u32>,
    bits: u32,
    mask: u32,
}

impl BitEntryArray {
    fn get(&self, i: u32) -> u32 {
        let bit = i as u64 * self.bits as u64;
        let w = (bit >> 5) as usize;
        let lo = *self.words.get(w).unwrap_or(&0) as u64;
        let hi = *self.words.get(w + 1).unwrap_or(&0) as u64;
        (((hi << 32 | lo) >> (bit & 31)) as u32) & self.mask
    }
}

#[derive(Debug, Clone, Copy)]
struct HashEntry {
    node: u32,
    next: u32,
    fragment: u32,
}

impl HashEntry {
    fn single_char(&self) -> Option<u8> {
        ((self.fragment & 0xFFFF_FF00) == 0xFFFF_FF00).then_some(self.fragment as u8)
    }
}

struct Stream<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Stream<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], CascError> {
        let s = self
            .data
            .get(self.pos..self.pos.checked_add(n).ok_or_else(truncated)?)
            .ok_or_else(truncated)?;
        self.pos += n;
        Ok(s)
    }

    fn u32(&mut self) -> Result<u32, CascError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, CascError> {
        Ok(u64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    /// A length-prefixed array (`u64` byte count, data, padding to 8 bytes).
    fn array(&mut self, item_size: usize) -> Result<&'a [u8], CascError> {
        let len = self.u64()?;
        if len > u32::MAX as u64 || len as usize % item_size != 0 {
            return Err(CascError::Format("MNDX: bad array length".into()));
        }
        let s = self.bytes(len as usize)?;
        self.bytes((len as usize).wrapping_neg() & 7)?;
        Ok(s)
    }

    fn u32_array(&mut self) -> Result<Vec<u32>, CascError> {
        Ok(self
            .array(4)?
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect())
    }

    fn bit_array(&mut self) -> Result<BitArray, CascError> {
        let words = self.u32_array()?;
        let total = self.u32()?;
        let valid = self.u32()?;
        if valid > total || (total as u64) > words.len() as u64 * 32 {
            return Err(CascError::Format("MNDX: bad sparse array".into()));
        }
        // Rank checkpoints (12-byte BASEVALS) and select shortcuts; the
        // same answers are recomputed by `BitArray::new`.
        self.array(12)?;
        self.array(4)?;
        self.array(4)?;
        Ok(BitArray::new(words, total, valid))
    }
}

fn truncated() -> CascError {
    CascError::Format("MNDX: truncated data".into())
}

/// One MAR name database (a node of the database chain).
#[derive(Debug, Default)]
struct NameDb {
    collision: BitArray,
    file_names: BitArray,
    hi_bits_idx: BitArray,
    lo_bits: Vec<u8>,
    hi_bits: BitEntryArray,
    fragments: Vec<u8>,
    path_marks: BitArray,
    child: Option<Box<NameDb>>,
    hash: Vec<HashEntry>,
    hash_mask: u32,
    root_limit: u32,
}

impl NameDb {
    fn parse_mar(data: &[u8]) -> Result<NameDb, CascError> {
        let mut s = Stream { data, pos: 0 };
        if s.bytes(4)? != b"MAR\0" {
            return Err(CascError::Format("MNDX: bad MAR signature".into()));
        }
        NameDb::load(&mut s, 0)
    }

    fn load(s: &mut Stream, depth: u32) -> Result<NameDb, CascError> {
        if depth > 8 {
            return Err(CascError::Format("MNDX: name database nests too deeply".into()));
        }
        let collision = s.bit_array()?;
        let file_names = s.bit_array()?;
        let hi_bits_idx = s.bit_array()?;
        let lo_bits = s.array(1)?.to_vec();
        let words = s.u32_array()?;
        let bits = s.u32()?;
        let mask = s.u32()?;
        let _entries = s.u64()?;
        if bits > 32 {
            return Err(CascError::Format("MNDX: bad bit entry width".into()));
        }
        let hi_bits = BitEntryArray { words, bits, mask };
        let fragments = s.array(1)?.to_vec();
        let path_marks = s.bit_array()?;
        let child = if hi_bits_idx.valid != 0 && fragments.is_empty() {
            Some(Box::new(NameDb::load(s, depth + 1)?))
        } else {
            None
        };
        let hash: Vec<HashEntry> = s
            .array(12)?
            .chunks_exact(12)
            .map(|c| HashEntry {
                node: u32::from_le_bytes(c[0..4].try_into().unwrap()),
                next: u32::from_le_bytes(c[4..8].try_into().unwrap()),
                fragment: u32::from_le_bytes(c[8..12].try_into().unwrap()),
            })
            .collect();
        if hash.is_empty() || !hash.len().is_power_of_two() {
            return Err(CascError::Format("MNDX: bad hash table".into()));
        }
        let hash_mask = hash.len() as u32 - 1;
        let root_limit = s.u32()?;
        let _flags = s.u32()?;
        Ok(NameDb {
            collision,
            file_names,
            hi_bits_idx,
            lo_bits,
            hi_bits,
            fragments,
            path_marks,
            child,
            hash,
            hash_mask,
            root_limit,
        })
    }

    fn lo(&self, node: u32) -> u8 {
        *self.lo_bits.get(node as usize).unwrap_or(&0)
    }

    /// Appends the edge label of `node`.
    fn copy_label(&self, node: u32, out: &mut Vec<u8>) -> Result<(), CascError> {
        if self.hi_bits_idx.get(node) {
            let offset = (self.hi_bits.get(self.hi_bits_idx.rank1(node)) << 8) | self.lo(node) as u32;
            self.copy_fragment(offset, out)
        } else {
            out.push(self.lo(node));
            Ok(())
        }
    }

    /// Appends the fragment at `offset`: a child-database path or a string
    /// in this database's fragment table.
    fn copy_fragment(&self, offset: u32, out: &mut Vec<u8>) -> Result<(), CascError> {
        if let Some(child) = &self.child {
            return child.copy_by_index(offset, out);
        }
        let mut i = offset as usize;
        if self.path_marks.is_empty() {
            while let Some(&c) = self.fragments.get(i) {
                if c == 0 {
                    return Ok(());
                }
                out.push(c);
                i += 1;
            }
        } else {
            while let Some(&c) = self.fragments.get(i) {
                out.push(c);
                if self.path_marks.get(i as u32) {
                    return Ok(());
                }
                i += 1;
            }
        }
        Err(CascError::Format("MNDX: unterminated path fragment".into()))
    }

    /// Appends the string stored as the path from `node` up to the root
    /// (CascLib's `CopyPathFragmentByIndex`).
    fn copy_by_index(&self, mut node: u32, out: &mut Vec<u8>) -> Result<(), CascError> {
        for _ in 0..0x10000 {
            let he = self.hash[(node & self.hash_mask) as usize];
            if node == he.next {
                match he.single_char() {
                    Some(c) => out.push(c),
                    None => self.copy_fragment(he.fragment, out)?,
                }
                node = he.node;
                if node == 0 {
                    return Ok(());
                }
            } else {
                self.copy_label(node, out)?;
                if node <= self.root_limit {
                    return Ok(());
                }
                // Parent in the LOUDS trie.
                let pos = self
                    .collision
                    .select1(node)
                    .ok_or_else(|| CascError::Format("MNDX: bad parent link".into()))?;
                node = pos.wrapping_sub(node).wrapping_sub(1);
            }
        }
        Err(CascError::Format("MNDX: fragment path does not terminate".into()))
    }

    /// Calls `f(name, name_index)` for every name, depth-first.
    fn for_each_name(&self, mut f: impl FnMut(&[u8], u32)) -> Result<(), CascError> {
        struct Frame {
            pos: u32,
            child: u32,
            base: usize,
        }
        let children = |node: u32| -> Result<Frame, CascError> {
            let zero = self
                .collision
                .select0(node)
                .ok_or_else(|| CascError::Format("MNDX: bad child link".into()))?;
            let pos = zero + 1;
            Ok(Frame {
                pos,
                child: pos - node - 1,
                base: 0,
            })
        };
        let mut path = Vec::with_capacity(256);
        if self.file_names.get(0) {
            f(&path, self.file_names.rank1(0));
        }
        let mut stack = vec![children(0)?];
        while let Some(top) = stack.last_mut() {
            if !self.collision.get(top.pos) {
                stack.pop();
                continue;
            }
            let node = top.child;
            top.pos += 1;
            top.child += 1;
            path.truncate(top.base);
            self.copy_label(node, &mut path)?;
            if self.file_names.get(node) {
                f(&path, self.file_names.rank1(node));
            }
            if stack.len() > 4096 {
                return Err(CascError::Format("MNDX: name trie is too deep".into()));
            }
            let mut frame = children(node)?;
            frame.base = path.len();
            stack.push(frame);
        }
        Ok(())
    }

    fn name_count(&self) -> u32 {
        self.file_names.valid
    }
}

/// One file of the root: its full path, content key and size.
#[derive(Debug, Clone)]
pub struct RootEntry {
    pub path: String,
    pub ckey: [u8; 16],
    pub size: u32,
}

/// Parses an MNDX root file into the list of every file it names.
pub fn parse(data: &[u8]) -> Result<Vec<RootEntry>, CascError> {
    let bad = |m: &str| CascError::Format(format!("MNDX: {m}"));
    let mut s = Stream { data, pos: 0 };
    if s.bytes(4)? != b"MNDX" {
        return Err(bad("not an MNDX root file"));
    }
    let header_version = s.u32()?;
    let format_version = s.u32()?;
    if !(1..=2).contains(&format_version) || header_version > 2 {
        return Err(bad("unsupported version"));
    }
    if header_version == 2 {
        s.u32()?;
        s.u32()?;
    }
    let mar_info_offset = s.u32()? as usize;
    let mar_info_count = s.u32()? as usize;
    let mar_info_size = s.u32()? as usize;
    let ckey_offset = s.u32()? as usize;
    let ckey_count = s.u32()? as usize;
    let file_name_count = s.u32()? as usize;
    let ckey_entry_size = s.u32()? as usize;
    if mar_info_count != 3 || mar_info_size != 20 || ckey_entry_size != 24 {
        return Err(bad("unexpected header layout"));
    }
    let mut mars = Vec::new();
    for i in 0..mar_info_count {
        let mut info = Stream {
            data,
            pos: mar_info_offset + i * mar_info_size,
        };
        let _index = info.u32()?;
        let size = info.u32()? as usize;
        let _size_hi = info.u32()?;
        let offset = info.u32()? as usize;
        let mar = data.get(offset..offset + size).ok_or_else(truncated)?;
        mars.push(NameDb::parse_mar(mar)?);
    }
    let (packages_db, stripped_db) = (&mars[0], &mars[1]);
    if stripped_db.name_count() as usize != file_name_count {
        return Err(bad("name count mismatch"));
    }
    let entries = data
        .get(ckey_offset..ckey_offset + ckey_count * 24)
        .ok_or_else(truncated)?;
    let entry = |i: usize| {
        let e = &entries[i * 24..i * 24 + 24];
        let flags = u32::from_le_bytes(e[0..4].try_into().unwrap());
        let mut ckey = [0u8; 16];
        ckey.copy_from_slice(&e[4..20]);
        (flags, ckey, u32::from_le_bytes(e[20..24].try_into().unwrap()))
    };
    // First entry of every name group.
    let mut group_start = Vec::with_capacity(file_name_count + 1);
    group_start.push(0usize);
    for i in 0..ckey_count {
        if group_start.len() > file_name_count {
            break;
        }
        if entry(i).0 & 0x8000_0000 != 0 {
            group_start.push(i + 1);
        }
    }
    if group_start.len() - 1 != file_name_count {
        return Err(bad("CKey groups do not match the name count"));
    }

    let mut packages: Vec<Option<String>> = vec![None; packages_db.name_count() as usize];
    packages_db.for_each_name(|name, index| {
        if let Some(slot) = packages.get_mut(index as usize) {
            *slot = Some(String::from_utf8_lossy(name).into_owned());
        }
    })?;

    let mut out = Vec::with_capacity(ckey_count);
    stripped_db.for_each_name(|name, index| {
        let Some(&start) = group_start.get(index as usize) else {
            return;
        };
        if index as usize >= file_name_count {
            return;
        }
        let name = String::from_utf8_lossy(name);
        for i in start..ckey_count {
            let (flags, ckey, size) = entry(i);
            if let Some(Some(pkg)) = packages.get((flags & 0x00FF_FFFF) as usize) {
                out.push(RootEntry {
                    path: format!("{pkg}/{name}"),
                    ckey,
                    size,
                });
            }
            if flags & 0x8000_0000 != 0 {
                break;
            }
        }
    })?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(pattern: &[u8]) -> BitArray {
        let mut words = vec![0u32; pattern.len().div_ceil(32)];
        for (i, &b) in pattern.iter().enumerate() {
            if b == 1 {
                words[i / 32] |= 1 << (i % 32);
            }
        }
        let valid = pattern.iter().filter(|&&b| b == 1).count() as u32;
        BitArray::new(words, pattern.len() as u32, valid)
    }

    #[test]
    fn rank_and_select_agree_with_naive() {
        let pattern: Vec<u8> = (0..200u32).map(|i| ((i * 7 + i / 3) % 5 < 2) as u8).collect();
        let b = bits(&pattern);
        let ones: Vec<u32> = (0..200).filter(|&i| pattern[i as usize] == 1).collect();
        let zeros: Vec<u32> = (0..200).filter(|&i| pattern[i as usize] == 0).collect();
        for i in 0..200u32 {
            assert_eq!(b.rank1(i), ones.iter().filter(|&&p| p < i).count() as u32);
        }
        for (k, &p) in ones.iter().enumerate() {
            assert_eq!(b.select1(k as u32), Some(p));
        }
        for (k, &p) in zeros.iter().enumerate() {
            assert_eq!(b.select0(k as u32), Some(p));
        }
        assert_eq!(b.select1(ones.len() as u32), None);
    }

    #[test]
    fn bit_entries_cross_word_boundaries() {
        // 12-bit entries 0..20 packed little-endian.
        let mut words = vec![0u32; 8];
        for i in 0..20u64 {
            let bit = i * 12;
            let v = i * 37 % 4096;
            let w = (bit / 32) as usize;
            let packed = (v << (bit % 32)) as u128;
            words[w] |= packed as u32;
            if w + 1 < words.len() {
                words[w + 1] |= (packed >> 32) as u32;
            }
        }
        let arr = BitEntryArray {
            words,
            bits: 12,
            mask: 0xFFF,
        };
        for i in 0..20u32 {
            assert_eq!(arr.get(i), i * 37 % 4096);
        }
    }

    /// A trie with single-character labels: root -> 'a' -> 'b' ('ab' is a
    /// name), root -> 'c' ('c' is a name).
    #[test]
    fn enumerates_a_louds_trie() {
        // Nodes in BFS order: 0 root, 1 'a', 2 'c', 3 'b'.
        // LOUDS with the usual "10" super-root prefix: root has 2 children
        // "110", a has 1 "10", c none "0", b none "0".
        let collision = bits(&[1, 0, 1, 1, 0, 1, 0, 0, 0]);
        let mut file_names = bits(&[0, 0, 1, 1]);
        file_names.total = 4;
        let db = NameDb {
            collision,
            file_names,
            hi_bits_idx: bits(&[0, 0, 0, 0]),
            lo_bits: vec![0, b'a', b'c', b'b'],
            hash: vec![HashEntry {
                node: u32::MAX,
                next: u32::MAX,
                fragment: 0,
            }],
            ..NameDb::default()
        };
        let mut names = Vec::new();
        db.for_each_name(|n, i| names.push((String::from_utf8(n.to_vec()).unwrap(), i)))
            .unwrap();
        assert_eq!(names, vec![("ab".to_string(), 1), ("c".to_string(), 0)]);
        // Parent links walk back up: b -> a.
        assert_eq!(db.collision.select1(3).unwrap() - 3 - 1, 1);
    }
}
