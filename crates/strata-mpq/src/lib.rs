//! Pure-Rust MPQ archive reading and writing for StarCraft II maps.
//! Must compile for `wasm32-unknown-unknown` (no C dependencies, no threads, no fs).
//!
//! API contract used by `strata-format` (keep these signatures stable):
//! - [`Archive::from_bytes`], [`Archive::files`], [`Archive::read`], [`Archive::contains`]
//! - [`ArchiveBuilder::new`], [`ArchiveBuilder::add`], [`ArchiveBuilder::build`]

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

/// A read-only MPQ archive held in memory.
pub struct Archive {
    _data: Vec<u8>,
}

impl Archive {
    pub fn from_bytes(data: Vec<u8>) -> Result<Archive, MpqError> {
        let _ = data;
        Err(MpqError::Unsupported("MPQ reading not implemented yet".into()))
    }

    /// File names from `(listfile)`, with `\` separators converted to `/`.
    pub fn files(&self) -> Vec<String> {
        Vec::new()
    }

    pub fn contains(&self, name: &str) -> bool {
        let _ = name;
        false
    }

    /// Read and decompress a file. Accepts `/` or `\` separators.
    pub fn read(&self, name: &str) -> Result<Vec<u8>, MpqError> {
        Err(MpqError::NotFound(name.to_owned()))
    }
}

/// Builds a new MPQ archive (format v1 or v3 as SC2 expects) in memory.
#[derive(Default)]
pub struct ArchiveBuilder {
    files: Vec<(String, Vec<u8>)>,
}

impl ArchiveBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a file; `/` separators are stored as `\`.
    pub fn add(&mut self, name: &str, data: Vec<u8>) {
        self.files.push((name.to_owned(), data));
    }

    pub fn build(self) -> Result<Vec<u8>, MpqError> {
        Err(MpqError::Unsupported("MPQ writing not implemented yet".into()))
    }
}
