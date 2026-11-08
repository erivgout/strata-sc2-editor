//! Load and save SC2 maps to and from [`strata_core::MapDocument`].
//!
//! A map is a set of files (an MPQ archive `.SC2Map`, or the same files in a
//! folder). Principle: lossless by default. Files Strata does not model, and
//! modeled components that are not dirty, are written back byte-for-byte via
//! `MapDocument::passthrough`.
//!
//! API contract used by the app and `strata-mcp` (keep these signatures stable):
//! [`load_files`], [`save_files`], [`load_sc2map`], [`save_sc2map`], and on
//! native targets [`load_path`] / [`save_path`].

use std::collections::BTreeMap;

use strata_core::MapDocument;

/// Archive path (with `/` separators) -> file contents.
pub type FileMap = BTreeMap<String, Vec<u8>>;

#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("archive error: {0}")]
    Mpq(#[from] strata_mpq::MpqError),
    #[error("{file} is malformed: {reason}")]
    Malformed { file: String, reason: String },
    #[error("this does not look like a StarCraft II map (missing {0})")]
    NotAMap(String),
    #[error("io error: {0}")]
    Io(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub fn load_files(files: FileMap) -> Result<MapDocument, FormatError> {
    let _ = files;
    Err(FormatError::Unsupported("loading not implemented yet".into()))
}

pub fn save_files(doc: &MapDocument) -> Result<FileMap, FormatError> {
    let _ = doc;
    Err(FormatError::Unsupported("saving not implemented yet".into()))
}

pub fn load_sc2map(bytes: Vec<u8>) -> Result<MapDocument, FormatError> {
    let archive = strata_mpq::Archive::from_bytes(bytes)?;
    let mut files = FileMap::new();
    for name in archive.files() {
        let data = archive.read(&name)?;
        files.insert(name, data);
    }
    load_files(files)
}

pub fn save_sc2map(doc: &MapDocument) -> Result<Vec<u8>, FormatError> {
    let mut b = strata_mpq::ArchiveBuilder::new();
    for (name, data) in save_files(doc)? {
        b.add(&name, data);
    }
    Ok(b.build()?)
}

/// Load a `.SC2Map` archive or an unpacked map folder.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_path(path: &std::path::Path) -> Result<MapDocument, FormatError> {
    let _ = path;
    Err(FormatError::Unsupported("loading from disk not implemented yet".into()))
}

/// Save as an archive, or as a folder if `path` is an existing directory.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_path(doc: &MapDocument, path: &std::path::Path) -> Result<(), FormatError> {
    let _ = (doc, path);
    Err(FormatError::Unsupported("saving to disk not implemented yet".into()))
}
