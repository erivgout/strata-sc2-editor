//! The encoding file: content key (MD5 of the plain file) -> encoded key
//! (what the local index is keyed by).
//!
//! Header (big-endian): `"EN"`, `u8 version`, `u8 ckey_len`, `u8 ekey_len`,
//! `u16 ce_page_kb`, `u16 espec_page_kb`, `u32 ce_page_count`,
//! `u32 espec_page_count`, `u8 0`, `u32 espec_block_size`. Then the ESpec
//! string block, the CE page table (`first_ckey, page_md5` per page) and the
//! CE pages. A page holds entries `u8 ekey_count, u40 content_size, ckey,
//! ekey * ekey_count`; an `ekey_count` of 0 pads out the page.
//!
//! Only the CE pages are kept; a lookup binary-searches the page table and
//! scans one page.

use crate::{CascError, Key};

pub struct Encoding {
    data: Vec<u8>,
    page_size: usize,
    /// Offset of the first page in `data`.
    pages_at: usize,
    /// First content key of each page.
    first_keys: Vec<[u8; 16]>,
}

impl Encoding {
    pub fn parse(data: Vec<u8>) -> Result<Encoding, CascError> {
        let bad = |m: &str| CascError::Format(format!("encoding file: {m}"));
        if data.len() < 22 || &data[0..2] != b"EN" {
            return Err(bad("bad signature"));
        }
        let (ckey_len, ekey_len) = (data[3], data[4]);
        if ckey_len != 16 || ekey_len != 16 {
            return Err(bad("unsupported key length"));
        }
        let page_size = u16::from_be_bytes([data[5], data[6]]) as usize * 1024;
        let page_count = u32::from_be_bytes(data[9..13].try_into().unwrap()) as usize;
        let espec_size = u32::from_be_bytes(data[18..22].try_into().unwrap()) as usize;
        let table_at = 22 + espec_size;
        let pages_at = table_at + page_count * 32;
        if page_size == 0 || pages_at + page_count * page_size > data.len() {
            return Err(bad("truncated"));
        }
        let first_keys = (0..page_count)
            .map(|i| {
                let mut k = [0u8; 16];
                k.copy_from_slice(&data[table_at + i * 32..table_at + i * 32 + 16]);
                k
            })
            .collect();
        Ok(Encoding { data, page_size, pages_at, first_keys })
    }

    pub(crate) fn empty() -> Encoding {
        Encoding { data: Vec::new(), page_size: 1, pages_at: 0, first_keys: Vec::new() }
    }

    pub fn page_count(&self) -> usize {
        self.first_keys.len()
    }

    /// The first encoded key and the content size of a content key.
    pub fn find(&self, ckey: &Key) -> Option<(Key, u64)> {
        let page = match self.first_keys.binary_search(&ckey.0) {
            Ok(i) => i,
            Err(0) => return None,
            Err(i) => i - 1,
        };
        let start = self.pages_at + page * self.page_size;
        let p = &self.data[start..start + self.page_size];
        let mut pos = 0;
        while pos + 22 <= p.len() {
            let count = p[pos] as usize;
            if count == 0 {
                break;
            }
            let size = u64::from_be_bytes([0, 0, 0, p[pos + 1], p[pos + 2], p[pos + 3], p[pos + 4], p[pos + 5]]);
            let entry_len = 22 + count * 16;
            if pos + entry_len > p.len() {
                break;
            }
            if p[pos + 6..pos + 22] == ckey.0 {
                let mut e = [0u8; 16];
                e.copy_from_slice(&p[pos + 22..pos + 38]);
                return Some((Key(e), size));
            }
            pos += entry_len;
        }
        None
    }
}

/// Builds an encoding file with one CE page for tests.
#[cfg(test)]
pub(crate) fn build_encoding(entries: &mut [([u8; 16], [u8; 16], u64)]) -> Vec<u8> {
    entries.sort_by_key(|e| e.0);
    let page_kb = 1u16;
    let mut out = b"EN".to_vec();
    out.extend_from_slice(&[1, 16, 16]);
    out.extend_from_slice(&page_kb.to_be_bytes());
    out.extend_from_slice(&page_kb.to_be_bytes());
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes());
    out.push(0);
    let espec = b"z\0";
    out.extend_from_slice(&(espec.len() as u32).to_be_bytes());
    out.extend_from_slice(espec);
    out.extend_from_slice(&entries[0].0);
    out.extend_from_slice(&[0u8; 16]);
    let mut page = Vec::new();
    for (ckey, ekey, size) in entries.iter() {
        page.push(1);
        page.extend_from_slice(&size.to_be_bytes()[3..]);
        page.extend_from_slice(ckey);
        page.extend_from_slice(ekey);
    }
    page.resize(page_kb as usize * 1024, 0);
    out.extend_from_slice(&page);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_keys_in_page() {
        let mut entries = [([3u8; 16], [30u8; 16], 300), ([1u8; 16], [10u8; 16], 100), ([2u8; 16], [20u8; 16], 200)];
        let enc = Encoding::parse(build_encoding(&mut entries)).unwrap();
        assert_eq!(enc.page_count(), 1);
        assert_eq!(enc.find(&Key([2u8; 16])), Some((Key([20u8; 16]), 200)));
        assert_eq!(enc.find(&Key([3u8; 16])), Some((Key([30u8; 16]), 300)));
        assert_eq!(enc.find(&Key([0u8; 16])), None);
        assert_eq!(enc.find(&Key([9u8; 16])), None);
    }
}
