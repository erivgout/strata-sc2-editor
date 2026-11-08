//! Archive reading: header discovery, table loading and file extraction.

use std::collections::HashSet;
use std::ops::Range;

use crate::compress::decompress_sector;
use crate::crypt::{
    decrypt, file_key, hash_string, HASH_NAME_A, HASH_NAME_B, HASH_TABLE_OFFSET, KEY_BLOCK_TABLE,
    KEY_HASH_TABLE,
};
use crate::hetbet::{parse_bet, parse_het, BetTable, HetTable};
use crate::{
    MpqError, FLAG_COMPRESS, FLAG_DELETE_MARKER, FLAG_ENCRYPTED, FLAG_EXISTS, FLAG_FIX_KEY,
    FLAG_IMPLODE, FLAG_PATCH_FILE, FLAG_SINGLE_UNIT,
};

const MAGIC_HEADER: &[u8; 4] = b"MPQ\x1A";
const MAGIC_USER_DATA: &[u8; 4] = b"MPQ\x1B";
const MAGIC_HET: &[u8; 4] = b"HET\x1A";
const MAGIC_BET: &[u8; 4] = b"BET\x1A";
const HASH_ENTRY_FREE: u32 = 0xFFFF_FFFF;
const HASH_ENTRY_DELETED: u32 = 0xFFFF_FFFE;

/// One block-table / BET entry. Positions are relative to the MPQ header.
#[derive(Clone, Copy, Debug)]
pub(crate) struct BlockEntry {
    pub pos: u64,
    pub csize: u64,
    pub fsize: u64,
    pub flags: u32,
}

#[derive(Clone, Copy)]
struct HashEntry {
    name1: u32,
    name2: u32,
    locale: u16,
    block: u32,
}

enum Index {
    Classic(Vec<HashEntry>),
    Het(HetTable, BetTable),
}

/// Options for [`Archive::from_bytes_with_options`].
#[derive(Clone, Debug, Default)]
pub struct OpenOptions {
    /// Use HET/BET tables even when classic hash/block tables are present
    /// (the default prefers the classic tables, like StormLib).
    pub prefer_het_bet: bool,
}

/// Metadata of one file in an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    /// Name with `/` separators.
    pub name: String,
    /// Uncompressed size in bytes.
    pub size: u64,
    /// Stored (compressed) size in bytes.
    pub compressed_size: u64,
    /// Raw `MPQ_FILE_*` flags (see the `FLAG_*` constants).
    pub flags: u32,
    /// CRC32 from `(attributes)`, when present.
    pub crc32: Option<u32>,
    /// MD5 of the uncompressed data from `(attributes)`, when present.
    pub md5: Option<[u8; 16]>,
}

/// A read-only MPQ archive held in memory.
pub struct Archive {
    data: Vec<u8>,
    /// Absolute offset of the MPQ header in `data`.
    base: usize,
    format_version: u16,
    sector_size: usize,
    user_data: Option<Range<usize>>,
    index: Index,
    blocks: Vec<BlockEntry>,
    names: Vec<String>,
    attributes: crate::attributes::Attributes,
}

fn rd_u16(d: &[u8], off: usize) -> Option<u16> {
    d.get(off..off + 2)
        .map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn rd_u32(d: &[u8], off: usize) -> Option<u32> {
    d.get(off..off + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn rd_u64(d: &[u8], off: usize) -> Option<u64> {
    d.get(off..off + 8)
        .map(|b| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
}

fn corrupt(msg: impl Into<String>) -> MpqError {
    MpqError::Corrupt(msg.into())
}

struct Header {
    format_version: u16,
    sector_shift: u16,
    hash_pos: u64,
    block_pos: u64,
    hash_count: u32,
    block_count: u32,
    hi_block_pos: u64,
    het_pos: u64,
    bet_pos: u64,
    archive_size: u64,
    // v4 compressed table sizes (0 when unknown)
    hash_size64: u64,
    block_size64: u64,
    hi_block_size64: u64,
    het_size64: u64,
    bet_size64: u64,
}

fn parse_header(d: &[u8], at: usize) -> Option<Header> {
    let h = d.get(at..)?;
    if h.len() < 32 || &h[0..4] != MAGIC_HEADER {
        return None;
    }
    let header_size = rd_u32(h, 4)? as usize;
    let mut version = rd_u16(h, 12)?;
    // Downgrade the version to what the header actually contains
    // (protected maps sometimes lie about either field).
    let available = header_size.min(h.len());
    if version > 3 {
        version = 0;
    }
    let need = [32usize, 44, 68, 208];
    while version > 0 && available < need[version as usize] {
        version -= 1;
    }
    let sector_shift = rd_u16(h, 14)?;
    let mut hdr = Header {
        format_version: version,
        sector_shift,
        hash_pos: rd_u32(h, 16)? as u64,
        block_pos: rd_u32(h, 20)? as u64,
        hash_count: rd_u32(h, 24)?,
        block_count: rd_u32(h, 28)?,
        hi_block_pos: 0,
        het_pos: 0,
        bet_pos: 0,
        archive_size: rd_u32(h, 8)? as u64,
        hash_size64: 0,
        block_size64: 0,
        hi_block_size64: 0,
        het_size64: 0,
        bet_size64: 0,
    };
    if version >= 1 {
        hdr.hi_block_pos = rd_u64(h, 32)?;
        hdr.hash_pos |= (rd_u16(h, 40)? as u64) << 32;
        hdr.block_pos |= (rd_u16(h, 42)? as u64) << 32;
    }
    if version >= 2 {
        hdr.archive_size = rd_u64(h, 44)?;
        hdr.bet_pos = rd_u64(h, 52)?;
        hdr.het_pos = rd_u64(h, 60)?;
    }
    if version >= 3 {
        hdr.hash_size64 = rd_u64(h, 68)?;
        hdr.block_size64 = rd_u64(h, 76)?;
        hdr.hi_block_size64 = rd_u64(h, 84)?;
        hdr.het_size64 = rd_u64(h, 92)?;
        hdr.bet_size64 = rd_u64(h, 100)?;
    }
    Some(hdr)
}

/// Locate the MPQ header: at a 512-byte aligned offset, either directly or
/// via an `MPQ\x1B` user-data block. Returns (header offset, user data range).
fn find_header(d: &[u8]) -> Option<(usize, Header, Option<Range<usize>>)> {
    let mut off = 0usize;
    while off + 32 <= d.len() {
        let magic = &d[off..off + 4];
        if magic == MAGIC_USER_DATA {
            if let (Some(header_offs), Some(ud_len)) = (rd_u32(d, off + 8), rd_u32(d, off + 12)) {
                let at = off.saturating_add(header_offs as usize);
                if header_offs as usize >= 16 {
                    if let Some(h) = parse_header(d, at) {
                        let ud_start = off + 16;
                        let ud_end = ud_start.saturating_add(ud_len as usize).min(at);
                        return Some((at, h, Some(ud_start..ud_end.max(ud_start))));
                    }
                }
            }
        } else if magic == MAGIC_HEADER {
            if let Some(h) = parse_header(d, off) {
                return Some((off, h, None));
            }
        }
        off += 512;
    }
    None
}

/// Read, decrypt and (if needed) decompress a classic table or HET/BET body.
fn load_table(
    d: &[u8],
    start: usize,
    full: usize,
    stored: usize,
    key: u32,
    pad: u8,
) -> Result<Vec<u8>, MpqError> {
    let available = d.len().saturating_sub(start);
    let take = stored.min(available);
    let mut buf = d[start.min(d.len())..start.min(d.len()) + take].to_vec();
    if key != 0 {
        decrypt(&mut buf, key);
    }
    if stored < full && take == stored {
        let mut out = vec![0u8; full];
        decompress_sector(&buf, &mut out)?;
        Ok(out)
    } else {
        // Truncated tables: pad (0xFF = free hash entries, 0 = absent blocks).
        buf.resize(full, pad);
        Ok(buf)
    }
}

/// Load an extended (HET/BET) table: 12-byte header, then an encrypted and
/// possibly compressed body.
fn load_ext_table(
    d: &[u8],
    start: usize,
    size: usize,
    magic: &[u8; 4],
    key: u32,
) -> Result<Vec<u8>, MpqError> {
    let raw = d
        .get(
            start
                ..start
                    .checked_add(size)
                    .ok_or_else(|| corrupt("ext table"))?,
        )
        .ok_or_else(|| corrupt("extended table out of bounds"))?;
    if raw.len() < 12 || &raw[0..4] != magic {
        return Err(corrupt("bad extended table signature"));
    }
    let data_size = rd_u32(raw, 8).unwrap_or(0) as usize;
    let mut body = raw[12..].to_vec();
    decrypt(&mut body, key);
    if data_size > body.len() {
        let mut out = vec![0u8; data_size];
        decompress_sector(&body, &mut out)?;
        Ok(out)
    } else {
        Ok(body)
    }
}

fn alloc_zeroed(len: u64) -> Result<Vec<u8>, MpqError> {
    let len = usize::try_from(len).map_err(|_| MpqError::Unsupported("file too large".into()))?;
    let mut v = Vec::new();
    v.try_reserve_exact(len)
        .map_err(|_| MpqError::Unsupported(format!("cannot allocate {len} bytes")))?;
    v.resize(len, 0);
    Ok(v)
}

/// Normalise to the archive's internal form (`\` separators).
fn internal_name(name: &str) -> String {
    name.replace('/', "\\")
}

impl Archive {
    /// Open an archive from its bytes (the whole file).
    pub fn from_bytes(data: Vec<u8>) -> Result<Archive, MpqError> {
        Self::from_bytes_with_options(data, &OpenOptions::default())
    }

    pub fn from_bytes_with_options(
        data: Vec<u8>,
        options: &OpenOptions,
    ) -> Result<Archive, MpqError> {
        let (base, hdr, user_data) = find_header(&data).ok_or(MpqError::NotMpq)?;
        if hdr.sector_shift > 23 {
            return Err(corrupt(format!("sector size shift {}", hdr.sector_shift)));
        }
        let sector_size = 512usize << hdr.sector_shift;
        let at = |pos: u64| -> Result<usize, MpqError> {
            usize::try_from(pos)
                .ok()
                .and_then(|p| p.checked_add(base))
                .ok_or_else(|| corrupt("table offset overflow"))
        };

        let has_classic = hdr.hash_count > 0 && hdr.block_count > 0;
        let has_ext = hdr.het_pos != 0 && hdr.bet_pos != 0;
        let use_ext = has_ext && (options.prefer_het_bet || !has_classic);

        let (index, blocks) = if use_ext {
            let (het_size, bet_size) = ext_sizes(&hdr, data.len() - base);
            let het = load_ext_table(&data, at(hdr.het_pos)?, het_size, MAGIC_HET, KEY_HASH_TABLE)?;
            let bet = load_ext_table(
                &data,
                at(hdr.bet_pos)?,
                bet_size,
                MAGIC_BET,
                KEY_BLOCK_TABLE,
            )?;
            let het = parse_het(&het)?;
            let mut bet = parse_bet(&bet)?;
            let blocks = std::mem::take(&mut bet.blocks);
            (Index::Het(het, bet), blocks)
        } else if has_classic {
            load_classic(&data, base, &hdr, &at)?
        } else {
            (Index::Classic(Vec::new()), Vec::new())
        };

        let mut archive = Archive {
            data,
            base,
            format_version: hdr.format_version,
            sector_size,
            user_data,
            index,
            blocks,
            names: Vec::new(),
            attributes: Default::default(),
        };
        archive.load_listfile();
        archive.load_attributes();
        Ok(archive)
    }

    fn load_listfile(&mut self) {
        let Ok(list) = self.read("(listfile)") else {
            return;
        };
        let mut seen = HashSet::new();
        let mut names = Vec::new();
        for raw in list.split(|&b| matches!(b, b';' | b'\r' | b'\n' | 0)) {
            let raw = raw.trim_ascii();
            if raw.is_empty() {
                continue;
            }
            let name = String::from_utf8_lossy(raw).replace('\\', "/");
            if self.find(&name).is_some() && seen.insert(name.to_ascii_lowercase()) {
                names.push(name);
            }
        }
        self.names = names;
    }

    fn load_attributes(&mut self) {
        let Ok(attr) = self.read("(attributes)") else {
            return;
        };
        self.attributes = crate::attributes::parse(&attr, self.blocks.len());
    }

    /// File names from `(listfile)`, with `\` separators converted to `/`.
    /// Only names that actually exist in the archive are returned.
    pub fn files(&self) -> Vec<String> {
        self.names.clone()
    }

    /// Borrowing variant of [`Archive::files`].
    pub fn file_names(&self) -> &[String] {
        &self.names
    }

    pub fn contains(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// Read and decompress a file. Accepts `/` or `\` separators.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, MpqError> {
        let index = self
            .find(name)
            .ok_or_else(|| MpqError::NotFound(name.to_owned()))?;
        self.read_block(index, name)
    }

    /// Metadata for a file, or `None` if it does not exist.
    pub fn file_info(&self, name: &str) -> Option<FileInfo> {
        let index = self.find(name)?;
        let b = &self.blocks[index];
        Some(FileInfo {
            name: name.replace('\\', "/"),
            size: b.fsize,
            compressed_size: b.csize,
            flags: b.flags,
            crc32: (self.attributes.crcs.as_ref()).and_then(|c| c.get(index).copied()),
            md5: (self.attributes.md5s.as_ref()).and_then(|m| m.get(index).copied()),
        })
    }

    /// Metadata for every listed file (see [`Archive::files`]).
    pub fn entries(&self) -> Vec<FileInfo> {
        self.names
            .iter()
            .filter_map(|n| self.file_info(n))
            .collect()
    }

    /// MPQ format version: 0 = v1, 1 = v2, 2 = v3, 3 = v4.
    pub fn format_version(&self) -> u16 {
        self.format_version
    }

    pub fn sector_size(&self) -> usize {
        self.sector_size
    }

    /// Offset of the MPQ header within the archive bytes.
    pub fn header_offset(&self) -> usize {
        self.base
    }

    /// Contents of the `MPQ\x1B` user-data block, if any.
    pub fn user_data(&self) -> Option<&[u8]> {
        self.user_data.clone().map(|r| &self.data[r])
    }

    /// Number of block-table entries (including unnamed and deleted ones).
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Whether files are located through the HET/BET tables.
    pub fn uses_het_bet(&self) -> bool {
        matches!(self.index, Index::Het(..))
    }

    /// The whole archive as given to [`Archive::from_bytes`].
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Number of live files the hash table points to that no known name
    /// (from `(listfile)` or the MPQ bookkeeping files) reaches. Such files
    /// can be read by nobody and would be dropped by a rebuild. Only the
    /// classic hash table is inspected; HET-only archives report 0.
    pub fn unlisted_count(&self) -> usize {
        let Index::Classic(table) = &self.index else { return 0 };
        let usable = |i: usize| {
            self.blocks.get(i).is_some_and(|b| b.flags & FLAG_EXISTS != 0 && b.flags & FLAG_DELETE_MARKER == 0)
        };
        let live: std::collections::BTreeSet<usize> =
            table.iter().map(|h| h.block as usize).filter(|&i| usable(i)).collect();
        let known: std::collections::BTreeSet<usize> = self
            .names
            .iter()
            .map(String::as_str)
            .chain(["(listfile)", "(attributes)", "(signature)", "(user data)"])
            .filter_map(|n| self.find(n))
            .collect();
        live.difference(&known).count()
    }

    fn find(&self, name: &str) -> Option<usize> {
        let usable = |i: usize| {
            self.blocks
                .get(i)
                .is_some_and(|b| b.flags & FLAG_EXISTS != 0 && b.flags & FLAG_DELETE_MARKER == 0)
        };
        match &self.index {
            Index::Classic(table) => {
                if table.is_empty() {
                    return None;
                }
                let n = table.len();
                let h0 = hash_string(name, HASH_TABLE_OFFSET) as usize;
                let a = hash_string(name, HASH_NAME_A);
                let b = hash_string(name, HASH_NAME_B);
                let start = if n.is_power_of_two() {
                    h0 & (n - 1)
                } else {
                    h0 % n
                };
                let mut fallback = None;
                for step in 0..n {
                    let e = &table[(start + step) % n];
                    if e.block == HASH_ENTRY_FREE {
                        break;
                    }
                    if e.block == HASH_ENTRY_DELETED || e.name1 != a || e.name2 != b {
                        continue;
                    }
                    let block = e.block as usize;
                    if usable(block) {
                        if e.locale == 0 {
                            return Some(block);
                        }
                        fallback.get_or_insert(block);
                    }
                }
                fallback
            }
            Index::Het(het, bet) => het.lookup(&internal_name(name), bet).filter(|&i| usable(i)),
        }
    }

    fn read_block(&self, index: usize, name: &str) -> Result<Vec<u8>, MpqError> {
        let b = self.blocks[index];
        if b.flags & FLAG_PATCH_FILE != 0 {
            return Err(MpqError::Unsupported(format!("patch file: {name}")));
        }
        if b.fsize == 0 {
            return Ok(Vec::new());
        }
        let start = usize::try_from(b.pos)
            .ok()
            .and_then(|p| p.checked_add(self.base))
            .ok_or_else(|| corrupt(format!("{name}: bad offset")))?;
        let encrypted = b.flags & FLAG_ENCRYPTED != 0;
        let key = if encrypted {
            file_key(name, b.flags & FLAG_FIX_KEY != 0, b.pos, b.fsize as u32)
        } else {
            0
        };
        let packed = b.flags & (FLAG_COMPRESS | FLAG_IMPLODE) != 0;
        let imploded = b.flags & FLAG_IMPLODE != 0;
        let fsize = usize::try_from(b.fsize).map_err(|_| corrupt("file too large"))?;

        let raw_len = if packed { b.csize } else { b.fsize };
        let raw = usize::try_from(raw_len)
            .ok()
            .and_then(|l| start.checked_add(l))
            .and_then(|end| self.data.get(start..end))
            .ok_or_else(|| corrupt(format!("{name}: data out of bounds")))?;

        if b.flags & FLAG_SINGLE_UNIT != 0 {
            let mut buf;
            let input: &[u8] = if encrypted {
                buf = raw.to_vec();
                decrypt(&mut buf, key);
                &buf
            } else {
                raw
            };
            if packed && input.len() < fsize {
                // Guard against garbage sizes before allocating the output.
                if b.fsize > b.csize.saturating_mul(4096).max(1 << 24) {
                    return Err(corrupt(format!("{name}: implausible size {}", b.fsize)));
                }
                let mut out = alloc_zeroed(b.fsize)?;
                unpack(input, &mut out, imploded).map_err(|e| ctx(e, name))?;
                return Ok(out);
            }
            buf = input
                .get(..fsize)
                .ok_or_else(|| corrupt(format!("{name}: short")))?
                .to_vec();
            return Ok(buf);
        }

        let ss = self.sector_size;
        let sectors = fsize.div_ceil(ss);

        if !packed {
            let mut out = raw[..fsize].to_vec();
            if encrypted {
                for (i, chunk) in out.chunks_mut(ss).enumerate() {
                    decrypt(chunk, key.wrapping_add(i as u32));
                }
            }
            return Ok(out);
        }

        let table_len = (sectors + 1) * 4;
        let mut table = raw
            .get(..table_len)
            .ok_or_else(|| corrupt(format!("{name}: sector table out of bounds")))?
            .to_vec();
        if encrypted {
            decrypt(&mut table, key.wrapping_sub(1));
        }
        let offsets: Vec<usize> = table
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as usize)
            .collect();
        // Grow the output as sectors decode, so a corrupt size cannot force
        // a huge allocation up front.
        let mut out = Vec::new();
        out.try_reserve_exact(fsize.min(raw.len().saturating_mul(16).max(ss)))
            .map_err(|_| MpqError::Unsupported(format!("cannot allocate {fsize} bytes")))?;
        let mut scratch = Vec::new();
        for i in 0..sectors {
            let (lo, hi) = (offsets[i], offsets[i + 1]);
            let sector = raw
                .get(lo..hi)
                .ok_or_else(|| corrupt(format!("{name}: sector {i} out of bounds")))?;
            let sector: &[u8] = if encrypted {
                scratch.clear();
                scratch.extend_from_slice(sector);
                decrypt(&mut scratch, key.wrapping_add(i as u32));
                &scratch
            } else {
                sector
            };
            let start = out.len();
            let len = ss.min(fsize - start);
            if sector.len() < len {
                out.resize(start + len, 0);
                unpack(sector, &mut out[start..], imploded).map_err(|e| ctx(e, name))?;
            } else {
                out.extend_from_slice(&sector[..len]);
            }
        }
        Ok(out)
    }
}

fn ctx(e: MpqError, name: &str) -> MpqError {
    match e {
        MpqError::Corrupt(m) => MpqError::Corrupt(format!("{name}: {m}")),
        MpqError::Unsupported(m) => MpqError::Unsupported(format!("{name}: {m}")),
        other => other,
    }
}

fn unpack(input: &[u8], out: &mut [u8], imploded: bool) -> Result<(), MpqError> {
    if imploded {
        match crate::explode::explode(input, out) {
            Some(n) if n == out.len() => Ok(()),
            _ => Err(corrupt("bad PKWARE imploded data")),
        }
    } else {
        decompress_sector(input, out)
    }
}

/// HET/BET sizes: from the v4 header, or estimated from table layout (v3).
fn ext_sizes(hdr: &Header, avail: usize) -> (usize, usize) {
    if hdr.het_size64 != 0 && hdr.bet_size64 != 0 {
        return (hdr.het_size64 as usize, hdr.bet_size64 as usize);
    }
    let mut bounds = vec![
        hdr.hash_pos,
        hdr.block_pos,
        hdr.hi_block_pos,
        hdr.het_pos,
        hdr.bet_pos,
    ];
    bounds.push(if hdr.archive_size != 0 {
        hdr.archive_size
    } else {
        avail as u64
    });
    bounds.retain(|&p| p != 0);
    let next = |pos: u64| {
        bounds
            .iter()
            .copied()
            .filter(|&p| p > pos)
            .min()
            .unwrap_or(avail as u64)
            .min(avail as u64)
    };
    (
        next(hdr.het_pos).saturating_sub(hdr.het_pos) as usize,
        next(hdr.bet_pos).saturating_sub(hdr.bet_pos) as usize,
    )
}

type Tables = (Index, Vec<BlockEntry>);

fn load_classic(
    data: &[u8],
    base: usize,
    hdr: &Header,
    at: &dyn Fn(u64) -> Result<usize, MpqError>,
) -> Result<Tables, MpqError> {
    let avail = data.len().saturating_sub(base);
    // Clamp absurd counts to what could physically fit in the file.
    let hash_count = (hdr.hash_count as usize).min(avail / 16 + 1);
    let block_count = (hdr.block_count as usize).min(avail / 16 + 1);

    let hash_full = hash_count * 16;
    let hash_stored = match hdr.hash_size64 {
        0 => hash_full,
        s => (s as usize).min(hash_full),
    };
    let hash_raw = load_table(
        data,
        at(hdr.hash_pos)?,
        hash_full,
        hash_stored,
        KEY_HASH_TABLE,
        0xFF,
    )?;
    let hash = hash_raw
        .chunks_exact(16)
        .map(|e| HashEntry {
            name1: rd_u32(e, 0).unwrap_or(0),
            name2: rd_u32(e, 4).unwrap_or(0),
            locale: rd_u16(e, 8).unwrap_or(0),
            block: rd_u32(e, 12).unwrap_or(HASH_ENTRY_FREE),
        })
        .collect();

    let block_full = block_count * 16;
    let block_stored = match hdr.block_size64 {
        0 => block_full,
        s => (s as usize).min(block_full),
    };
    let block_raw = load_table(
        data,
        at(hdr.block_pos)?,
        block_full,
        block_stored,
        KEY_BLOCK_TABLE,
        0,
    )?;

    let hi = if hdr.hi_block_pos != 0 {
        let full = block_count * 2;
        let stored = match hdr.hi_block_size64 {
            0 => full,
            s => (s as usize).min(full),
        };
        Some(load_table(data, at(hdr.hi_block_pos)?, full, stored, 0, 0)?)
    } else {
        None
    };

    let blocks = block_raw
        .chunks_exact(16)
        .enumerate()
        .map(|(i, e)| {
            let hi_bits = hi
                .as_ref()
                .and_then(|h| rd_u16(h, i * 2))
                .map_or(0, |v| v as u64);
            BlockEntry {
                pos: rd_u32(e, 0).unwrap_or(0) as u64 | (hi_bits << 32),
                csize: rd_u32(e, 4).unwrap_or(0) as u64,
                fsize: rd_u32(e, 8).unwrap_or(0) as u64,
                flags: rd_u32(e, 12).unwrap_or(0),
            }
        })
        .collect();
    Ok((Index::Classic(hash), blocks))
}
