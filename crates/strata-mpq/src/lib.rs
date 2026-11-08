//! Pure-Rust MPQ archive reading and writing for StarCraft II maps.
//! Must compile for `wasm32-unknown-unknown` (no C dependencies, no threads, no fs).
//!
//! API contract used by `strata-format` (keep these signatures stable):
//! - [`Archive::from_bytes`], [`Archive::files`], [`Archive::read`], [`Archive::contains`]
//! - [`ArchiveBuilder::new`], [`ArchiveBuilder::add`], [`ArchiveBuilder::build`]
//!
//! Reading supports format v1-v4 headers (optionally behind an `MPQ\x1B`
//! user-data block), classic hash/block tables (encrypted, optionally
//! compressed in v4), the hi-block table, HET/BET tables, single-unit,
//! sectored, encrypted (incl. `FIX_KEY`) files, and zlib / bzip2 / LZMA /
//! sparse / PKWARE implode compression. Huffman and ADPCM (WAV-only codecs
//! from older Blizzard games) are reported as [`MpqError::Unsupported`].

// `as_chunks` (the lint's suggestion) needs Rust 1.88; the workspace MSRV is 1.85.
#![allow(unknown_lints, clippy::chunks_exact_to_as_chunks)]

mod attributes;
mod compress;
pub mod crypt;
mod explode;
mod hetbet;
mod read;
mod write;

pub use read::{Archive, FileInfo, OpenOptions};
pub use write::ArchiveBuilder;

/// File is PKWARE-imploded.
pub const FLAG_IMPLODE: u32 = 0x0000_0100;
/// File is compressed (per-sector compression mask byte).
pub const FLAG_COMPRESS: u32 = 0x0000_0200;
/// File is encrypted.
pub const FLAG_ENCRYPTED: u32 = 0x0001_0000;
/// Encryption key is adjusted by file position and size.
pub const FLAG_FIX_KEY: u32 = 0x0002_0000;
/// File is an incremental patch.
pub const FLAG_PATCH_FILE: u32 = 0x0010_0000;
/// File is stored as one unit rather than in sectors.
pub const FLAG_SINGLE_UNIT: u32 = 0x0100_0000;
/// File is a deletion marker.
pub const FLAG_DELETE_MARKER: u32 = 0x0200_0000;
/// File has per-sector CRCs.
pub const FLAG_SECTOR_CRC: u32 = 0x0400_0000;
/// Block entry is in use.
pub const FLAG_EXISTS: u32 = 0x8000_0000;

#[derive(Debug, thiserror::Error)]
pub enum MpqError {
    #[error("not an MPQ archive")]
    NotMpq,
    #[error("archive is corrupt: {0}")]
    Corrupt(String),
    #[error("file not found in archive: {0}")]
    NotFound(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}
