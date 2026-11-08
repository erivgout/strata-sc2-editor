//! Strata CASC: a pure-Rust reader for the local CASC storage of an installed
//! StarCraft II, so the editor can see the base game's mods (Core, Liberty,
//! Swarm, Void...) without bundling any Blizzard data.
//!
//! Reading a file goes `.build.info` -> build config -> encoding file +
//! MNDX root -> content key -> encoded key -> local `.idx` -> `data.###`
//! -> [`blte`] decode.
//!
//! The crate does no I/O itself: everything is read through a [`Source`], a
//! "read this byte range of this file under the install folder" interface
//! whose methods return futures. Native code uses [`DirSource`] (std::fs,
//! always-ready futures, so [`block_on`] drives it); the browser implements
//! it over File System Access API handles and awaits real promises.
//!
//! Module map:
//! - [`config`]: `.build.info` and the build config.
//! - [`index`]: local `.idx` files (encoded key -> archive location).
//! - [`encoding`]: the encoding file (content key -> encoded key).
//! - [`mndx`]: the MNDX root (path -> content key).
//! - [`blte`]: the BLTE frame decoder.

pub mod blte;
pub mod config;
pub mod encoding;
pub mod index;
pub mod mndx;

use std::collections::HashMap;
use std::fmt;
use std::future::Future;

pub use config::{BuildConfig, BuildInfo};
pub use mndx::RootEntry;

#[derive(Debug, thiserror::Error)]
pub enum CascError {
    #[error("{0}")]
    Io(String),
    #[error("not a StarCraft II install folder: {0}")]
    NotAnInstall(String),
    #[error("corrupt or unsupported CASC data: {0}")]
    Format(String),
    #[error("file is encrypted")]
    Encrypted,
    #[error("file not found in CASC storage: {0}")]
    NotFound(String),
}

/// A 16-byte CASC key (content key or encoded key).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub [u8; 16]);

impl Key {
    pub fn from_hex(s: &str) -> Result<Key, CascError> {
        let s = s.trim();
        if s.len() != 32 {
            return Err(CascError::Format(format!("bad key {s:?}")));
        }
        let mut k = [0u8; 16];
        for (i, b) in k.iter_mut().enumerate() {
            *b = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16)
                .map_err(|_| CascError::Format(format!("bad key {s:?}")))?;
        }
        Ok(Key(k))
    }

    pub fn to_hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Key({})", self.to_hex())
    }
}

/// Byte access to the files under a StarCraft II install folder. Paths are
/// relative, `/`-separated (e.g. `SC2Data/data/data.003`).
pub trait Source {
    /// Reads `len` bytes at `offset` (`None` = to the end of the file).
    fn read(&self, path: &str, offset: u64, len: Option<u64>) -> impl Future<Output = Result<Vec<u8>, CascError>>;
    /// Lists the file names (not paths) in a directory.
    fn list(&self, dir: &str) -> impl Future<Output = Result<Vec<String>, CascError>>;
}

/// Drives a future whose source never actually waits (such as [`DirSource`]).
///
/// # Panics
/// If the future is not ready on its first poll, i.e. the source really is
/// asynchronous; await it instead.
pub fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
    match fut.as_mut().poll(&mut cx) {
        std::task::Poll::Ready(v) => v,
        std::task::Poll::Pending => panic!("strata_casc::block_on used with an asynchronous source"),
    }
}

/// A [`Source`] over a folder on the local file system.
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone)]
pub struct DirSource {
    pub root: std::path::PathBuf,
}

#[cfg(not(target_arch = "wasm32"))]
impl DirSource {
    pub fn new(root: impl Into<std::path::PathBuf>) -> DirSource {
        DirSource { root: root.into() }
    }

    fn read_sync(&self, path: &str, offset: u64, len: Option<u64>) -> Result<Vec<u8>, CascError> {
        use std::io::{Read, Seek, SeekFrom};
        let full = self.root.join(path);
        let io = |e: std::io::Error| CascError::Io(format!("{}: {e}", full.display()));
        let mut f = std::fs::File::open(&full).map_err(io)?;
        f.seek(SeekFrom::Start(offset)).map_err(io)?;
        let mut buf = Vec::new();
        match len {
            Some(n) => {
                buf.resize(n as usize, 0);
                f.read_exact(&mut buf).map_err(io)?;
            }
            None => {
                f.read_to_end(&mut buf).map_err(io)?;
            }
        }
        Ok(buf)
    }

    fn list_sync(&self, dir: &str) -> Result<Vec<String>, CascError> {
        let full = self.root.join(dir);
        let rd = std::fs::read_dir(&full).map_err(|e| CascError::Io(format!("{}: {e}", full.display())))?;
        Ok(rd
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Source for DirSource {
    fn read(&self, path: &str, offset: u64, len: Option<u64>) -> impl Future<Output = Result<Vec<u8>, CascError>> {
        std::future::ready(self.read_sync(path, offset, len))
    }

    fn list(&self, dir: &str) -> impl Future<Output = Result<Vec<String>, CascError>> {
        std::future::ready(self.list_sync(dir))
    }
}

/// Normalizes a storage path for lookup: lowercase, `/` separators.
pub fn normalize_path(path: &str) -> String {
    path.trim_start_matches(['/', '\\'])
        .replace('\\', "/")
        .to_ascii_lowercase()
}

/// An opened CASC storage: the file list plus what is needed to read files.
pub struct Storage<S> {
    source: S,
    data_dir: String,
    pub build: BuildInfo,
    pub build_name: String,
    index: index::LocalIndex,
    encoding: encoding::Encoding,
    files: Vec<RootEntry>,
    by_path: HashMap<String, usize>,
}

impl<S: Source> Storage<S> {
    /// Opens the storage, keeping every file of the root in the file list.
    pub async fn open(source: S) -> Result<Storage<S>, CascError> {
        Storage::open_filtered(source, |_| true).await
    }

    /// Opens the storage, keeping only files whose normalized path passes
    /// `keep` (the full SC2 file list is a few hundred thousand names).
    pub async fn open_filtered(source: S, keep: impl Fn(&str) -> bool) -> Result<Storage<S>, CascError> {
        let info_bytes = source
            .read(".build.info", 0, None)
            .await
            .map_err(|e| CascError::NotAnInstall(format!("no .build.info ({e})")))?;
        let build = BuildInfo::parse(&String::from_utf8_lossy(&info_bytes))?;

        let mut data_dir = None;
        let mut idx_names = Vec::new();
        for dir in ["SC2Data", "Data"] {
            if let Ok(names) = source.list(&format!("{dir}/data")).await {
                if names.iter().any(|n| n.to_ascii_lowercase().ends_with(".idx")) {
                    data_dir = Some(dir.to_string());
                    idx_names = names;
                    break;
                }
            }
        }
        let data_dir =
            data_dir.ok_or_else(|| CascError::NotAnInstall("no SC2Data/data folder with .idx files".into()))?;

        let cfg_bytes = source
            .read(&config::config_path(&data_dir, &build.build_key), 0, None)
            .await?;
        let cfg = BuildConfig::parse(&String::from_utf8_lossy(&cfg_bytes))?;

        let mut index = index::LocalIndex::default();
        for name in index::LocalIndex::select_files(&idx_names) {
            let bytes = source.read(&format!("{data_dir}/data/{name}"), 0, None).await?;
            index.add_file(&bytes)?;
        }

        let mut storage = Storage {
            source,
            data_dir,
            build,
            build_name: cfg.build_name.clone(),
            index,
            encoding: encoding::Encoding::empty(),
            files: Vec::new(),
            by_path: HashMap::new(),
        };
        let enc_bytes = storage.read_ekey(&cfg.encoding_ekey).await?;
        storage.encoding = encoding::Encoding::parse(enc_bytes)?;

        let (root_ekey, _) = storage
            .encoding
            .find(&cfg.root_ckey)
            .ok_or_else(|| CascError::Format("root file is not in the encoding table".into()))?;
        let root = storage.read_ekey(&root_ekey).await?;
        let mut files = mndx::parse(&root)?;
        files.retain(|f| keep(&normalize_path(&f.path)));
        let mut by_path = HashMap::with_capacity(files.len());
        for (i, f) in files.iter().enumerate() {
            by_path.entry(normalize_path(&f.path)).or_insert(i);
        }
        storage.files = files;
        storage.by_path = by_path;
        Ok(storage)
    }

    pub fn source(&self) -> &S {
        &self.source
    }

    /// Every file kept from the root.
    pub fn files(&self) -> &[RootEntry] {
        &self.files
    }

    /// Looks a file up by path (case-insensitive, either slash).
    pub fn find(&self, path: &str) -> Option<&RootEntry> {
        self.by_path.get(&normalize_path(path)).map(|&i| &self.files[i])
    }

    /// Reads and decodes a file by path.
    pub async fn read(&self, path: &str) -> Result<Vec<u8>, CascError> {
        let entry = self.find(path).ok_or_else(|| CascError::NotFound(path.to_string()))?;
        self.read_ckey(&Key(entry.ckey)).await
    }

    /// Reads and decodes a file by content key.
    pub async fn read_ckey(&self, ckey: &Key) -> Result<Vec<u8>, CascError> {
        let (ekey, _) = self
            .encoding
            .find(ckey)
            .ok_or_else(|| CascError::NotFound(format!("content key {}", ckey.to_hex())))?;
        self.read_ekey(&ekey).await
    }

    /// Reads and decodes a file by encoded key.
    pub async fn read_ekey(&self, ekey: &Key) -> Result<Vec<u8>, CascError> {
        let loc = self
            .index
            .find(&ekey.0)
            .ok_or_else(|| CascError::NotFound(format!("encoded key {} (not installed locally)", ekey.to_hex())))?;
        if loc.size < 30 {
            return Err(CascError::Format("index entry too small".into()));
        }
        let path = format!("{}/data/data.{:03}", self.data_dir, loc.archive);
        let raw = self
            .source
            .read(&path, loc.offset as u64, Some(loc.size as u64))
            .await?;
        blte::decode(&raw[30..])
    }

    /// Number of encoded keys present in the local indexes.
    pub fn local_key_count(&self) -> usize {
        self.index.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_hex_roundtrip() {
        let k = Key::from_hex("1d231af579faf03e61a364722a3b87b4").unwrap();
        assert_eq!(k.to_hex(), "1d231af579faf03e61a364722a3b87b4");
        assert!(Key::from_hex("xyz").is_err());
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(
            normalize_path("\\Mods\\Liberty.SC2Mod/Base.SC2Data/GameData/UnitData.xml"),
            "mods/liberty.sc2mod/base.sc2data/gamedata/unitdata.xml"
        );
    }
}
