//! Load and save SC2 maps to and from [`strata_core::MapDocument`].
//!
//! A map is a set of files (an MPQ archive `.SC2Map`, or the same files in a
//! folder). Principle: lossless by default.
//!
//! - On load, **every** file of the map is kept in
//!   [`MapDocument::passthrough`] (modeled ones too: they are the baseline
//!   that saving diffs against).
//! - On save, the output starts as a copy of `passthrough`. A modeled
//!   component is regenerated only when its [`DirtyFlags`] bit is set, and
//!   even then only the parts whose model differs from the decoded original
//!   are rewritten (single changed catalog entries, objects, regions, terrain
//!   vertices, string lines...). Loading and saving without edits returns
//!   exactly the input files.
//! - After writing a save to disk, call [`mark_saved`] to make it the new
//!   baseline (or keep the dirty flags set; saving again is idempotent).
//!
//! What is modeled (see each module for the byte layouts and what was
//! verified against real editor output):
//!
//! | component | files | module |
//! |---|---|---|
//! | map info | `MapInfo` (v39 fully, older versions size/tileset/bounds), `DocumentHeader` (v8), `DocumentInfo` | [`info`], [`docinfo`] |
//! | terrain | `t3Terrain.xml`, `t3HeightMap`, `t3SyncHeightMap`, `t3SyncCliffLevel`, `t3CellFlags`, `t3TextureMasks` (+ generated `t3SyncTextureInfo`, `t3VertCol`, `t3Water`, `t3HardTile`, `t3FluffDoodad` for new maps) | [`terrain`] |
//! | objects, cameras | `Objects` | [`objects`] |
//! | regions | `Regions` | [`regions`] |
//! | game data | `Base.SC2Data/GameData/*.xml` | [`catalog`] |
//! | strings | `<locale>.SC2Data/LocalizedData/GameStrings.txt` | [`strings`] |
//! | triggers | `Strata/Triggers.json`, `MapScript.galaxy` | [`triggers`] |
//!
//! Everything else (`Triggers`, `Attributes`, `CustomAI`, `ObjectStrings.txt`,
//! `TriggerStrings.txt`, `Preload*`, `Minimap.tga`, assets, `*.version`...)
//! is passthrough.
//!
//! A document with no `MapInfo`/`t3Terrain.xml` in its passthrough (e.g.
//! [`MapDocument::new`]) is saved as a complete new map: `DocumentHeader`,
//! `DocumentInfo`, `MapInfo`, `ComponentList.SC2Components`, all terrain
//! files, `Objects`, `Regions`, `MapScript.galaxy`, `GameStrings.txt`,
//! `Minimap.tga` and any catalogs.
//!
//! API contract used by the app and `strata-mcp` (keep these signatures stable):
//! [`load_files`], [`save_files`], [`load_sc2map`], [`save_sc2map`], and on
//! native targets [`load_path`] / [`save_path`] / [`save_folder`].

use std::collections::BTreeMap;

use strata_core::document::DirtyFlags;
use strata_core::strings::DEFAULT_LOCALE;
use strata_core::{MapDocument, MapInfo, Player};

mod bin;
pub mod catalog;
pub mod docinfo;
pub mod info;
pub mod objects;
pub mod regions;
pub mod strings;
pub mod terrain;
pub mod triggers;
mod xml;

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
    /// A written map did not read back as what was written. During an
    /// archive save the original file is left untouched.
    #[error("the written map did not read back correctly: {0}")]
    Verify(String),
}

pub const MAP_INFO: &str = "MapInfo";
pub const DOCUMENT_HEADER: &str = "DocumentHeader";
pub const DOCUMENT_INFO: &str = "DocumentInfo";
pub const COMPONENT_LIST: &str = "ComponentList.SC2Components";
pub const OBJECTS: &str = "Objects";
pub const REGIONS: &str = "Regions";
pub const MINIMAP: &str = "Minimap.tga";

/// Case-insensitive file lookup (MPQ names are case-insensitive).
pub(crate) fn lookup<'a>(files: &'a FileMap, name: &str) -> Option<&'a [u8]> {
    files
        .get(name)
        .or_else(|| files.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v))
        .map(Vec::as_slice)
}

/// Insert or replace a file, reusing the existing key's spelling.
pub(crate) fn put(out: &mut FileMap, name: &str, data: Vec<u8>) {
    let key = if out.contains_key(name) {
        name.to_owned()
    } else {
        out.keys().find(|k| k.eq_ignore_ascii_case(name)).cloned().unwrap_or_else(|| name.to_owned())
    };
    out.insert(key, data);
}

fn put_if_changed(out: &mut FileMap, name: &str, data: Vec<u8>) {
    if lookup(out, name) != Some(data.as_slice()) {
        put(out, name, data);
    }
}

fn parse_xml(files: &FileMap, name: &str) -> Option<xml::Doc> {
    lookup(files, name).and_then(|b| xml::parse_bytes(b).ok())
}

// ---- load ----------------------------------------------------------------------

pub fn load_files(files: FileMap) -> Result<MapDocument, FormatError> {
    let markers = [MAP_INFO, terrain::XML, DOCUMENT_HEADER, DOCUMENT_INFO, COMPONENT_LIST];
    if !markers.iter().any(|m| lookup(&files, m).is_some()) {
        return Err(FormatError::NotAMap(MAP_INFO.into()));
    }

    let mapinfo = lookup(&files, MAP_INFO).and_then(|b| info::parse_mapinfo(b).ok());
    let header = lookup(&files, DOCUMENT_HEADER).and_then(|b| info::parse_header(b).ok());

    let mut strings = strata_core::strings::Strings::default();
    for (path, bytes) in &files {
        if let Some(loc) = strings::locale_of(path) {
            strings.locales.insert(loc.to_owned(), strings::parse(bytes));
        }
    }

    let size = mapinfo.as_ref().map(|m| (m.width, m.height));
    let (terrain, desc_tileset) =
        terrain::load(&files, size).unwrap_or_else(|| (strata_core::terrain::Terrain::new(64, 64), String::new()));

    let (objects, cameras) = parse_xml(&files, OBJECTS).map(|d| objects::parse(&d)).unwrap_or_default();
    let regions = parse_xml(&files, REGIONS).map(|d| regions::parse(&d)).unwrap_or_default();

    let mut entries = Vec::new();
    for (path, bytes) in &files {
        if let Some(file) = catalog::file_of(path) {
            if let Some(es) = catalog::parse(file, bytes) {
                entries.extend(es);
            }
        }
    }

    let text = |key: &str| -> Option<String> {
        strings.get(DEFAULT_LOCALE, key).map(str::to_owned).or_else(|| {
            header.as_ref()?.entries.iter().find(|(k, l, _)| k == key && l == DEFAULT_LOCALE).map(|(_, _, v)| v.clone())
        })
    };

    let players: Vec<Player> = mapinfo
        .as_ref()
        .and_then(|m| m.players.as_ref())
        .map(|recs| {
            recs.iter()
                .enumerate()
                .map(|(i, r)| {
                    let name = text(&info::player_name_key(i)).unwrap_or_else(|| info::fallback_player_name(r.id));
                    let start = (r.start_point != 0)
                        .then(|| objects.iter().find(|o| o.id.0 == r.start_point).map(|o| o.position.truncate()))
                        .flatten();
                    info::player_from_rec(r, name, start)
                })
                .collect()
        })
        .unwrap_or_default();

    let dependencies = parse_xml(&files, DOCUMENT_INFO)
        .map(|d| docinfo::parse_dependencies(&d))
        .or_else(|| header.as_ref().map(|h| h.deps.clone()))
        .unwrap_or_default();

    let (w, h) = (terrain.width, terrain.height);
    let info = MapInfo {
        name: text("DocInfo/Name").unwrap_or_default(),
        description: text("DocInfo/DescLong").unwrap_or_default(),
        author: text("DocInfo/Author").unwrap_or_default(),
        tileset: mapinfo.as_ref().map(|m| m.tileset.clone()).filter(|t| !t.is_empty()).unwrap_or(desc_tileset),
        width: w,
        height: h,
        playable: mapinfo.as_ref().map(|m| m.bounds).unwrap_or([0, 0, w, h]),
        players,
        dependencies,
    };

    let max_id = objects
        .iter()
        .map(|o| o.id.0)
        .chain(regions.iter().map(|r| r.id.0))
        .chain(cameras.iter().map(|c| c.id.0))
        .max()
        .unwrap_or(0);
    let triggers = triggers::load(&files);

    Ok(MapDocument {
        info,
        terrain,
        objects: strata_core::objects::ObjectLayer { objects, next_id: max_id.saturating_add(1) },
        regions,
        cameras,
        triggers,
        data: strata_core::catalog::Catalog { entries },
        strings,
        passthrough: files,
        dirty: DirtyFlags::default(),
        removed_files: Default::default(),
    })
}

// ---- save ----------------------------------------------------------------------

fn is_new(doc: &MapDocument) -> bool {
    lookup(&doc.passthrough, MAP_INFO).is_none() && lookup(&doc.passthrough, terrain::XML).is_none()
}

/// The player list as written to MapInfo (new maps gain neutral and hostile slots).
fn file_players(doc: &MapDocument, new: bool) -> Vec<Player> {
    if new { info::new_recs(&doc.info.players) } else { doc.info.players.clone() }
}

/// `doc.strings` with the info-derived keys (name, description, author,
/// player names) applied to the default locale.
///
/// A key missing from GameStrings is only added when its value differs from
/// what loading produced without it (the DocumentHeader copy, or the
/// synthesized "Player N"), so unedited maps stay byte-identical.
fn effective_strings(doc: &MapDocument, new: bool) -> strata_core::strings::Strings {
    let header = lookup(&doc.passthrough, DOCUMENT_HEADER).and_then(|b| info::parse_header(b).ok());
    let from_header = |key: &str| -> Option<String> {
        header.as_ref()?.entries.iter().find(|(k, l, _)| k == key && l == DEFAULT_LOCALE).map(|(_, _, v)| v.clone())
    };
    let mut s = doc.strings.clone();
    let mut set = |key: &str, value: &str, baseline: String| {
        let exists = s.get(DEFAULT_LOCALE, key).is_some();
        let write = exists || if new { !value.is_empty() } else { value != baseline };
        if write {
            s.set(DEFAULT_LOCALE, key, value);
        }
    };
    for (key, value) in
        [("DocInfo/Name", &doc.info.name), ("DocInfo/DescLong", &doc.info.description), ("DocInfo/Author", &doc.info.author)]
    {
        set(key, value, from_header(key).unwrap_or_default());
    }
    for (i, p) in file_players(doc, new).iter().enumerate() {
        let key = info::player_name_key(i);
        let baseline = from_header(&key).unwrap_or_else(|| info::fallback_player_name(p.id));
        set(&key, &p.name, baseline);
    }
    s
}

fn save_info(doc: &MapDocument, new: bool, strings: &strata_core::strings::Strings, out: &mut FileMap) {
    let orig = &doc.passthrough;
    let players = file_players(doc, new);

    // MapInfo
    match lookup(orig, MAP_INFO) {
        Some(b) => {
            if let Ok(f) = info::parse_mapinfo(b) {
                let recs = f.players.as_ref().map(|p| info::recs_from_players(&players, p));
                put_if_changed(out, MAP_INFO, info::patch_mapinfo(b, &f, &doc.info, recs.as_deref()));
            }
        }
        None => {
            let recs = info::recs_from_players(&players, &[]);
            put(out, MAP_INFO, info::new_mapinfo(&doc.info, &recs));
        }
    }

    // DocumentInfo
    match parse_xml(orig, DOCUMENT_INFO) {
        Some(d) => {
            if docinfo::parse_dependencies(&d) != doc.info.dependencies {
                put(out, DOCUMENT_INFO, docinfo::patch_dependencies(&d, &doc.info.dependencies));
            }
        }
        None if new => put(out, DOCUMENT_INFO, docinfo::new_document_info(&doc.info.dependencies)),
        None => {}
    }

    // DocumentHeader
    let mut wanted: Vec<String> = ["DocInfo/Name", "DocInfo/DescLong", "DocInfo/DescShort"].map(String::from).to_vec();
    wanted.extend((0..players.len()).map(info::player_name_key));
    let header = match lookup(orig, DOCUMENT_HEADER) {
        Some(b) => info::parse_header(b).ok(), // unknown version: keep as is
        None if new => {
            Some(info::Header { prefix: info::new_header_prefix(), deps: Vec::new(), entries: Vec::new(), tail: Vec::new() })
        }
        None => None,
    };
    if let Some(mut h) = header {
        // Only carry over values that changed since load: editor headers are
        // often stale copies, and an unedited save must not touch them.
        let baseline_deps = parse_xml(orig, DOCUMENT_INFO).map(|d| docinfo::parse_dependencies(&d)).unwrap_or(h.deps.clone());
        if new || doc.info.dependencies != baseline_deps {
            h.deps = doc.info.dependencies.clone();
        }
        let mut locales: Vec<String> = h.entries.iter().map(|(_, l, _)| l.clone()).collect();
        locales.push(DEFAULT_LOCALE.to_owned());
        locales.sort();
        locales.dedup();
        let orig_strings: BTreeMap<String, BTreeMap<String, String>> = locales
            .into_iter()
            .map(|l| {
                let m = lookup(orig, &strings::path(&l)).map(strings::parse).unwrap_or_default();
                (l, m)
            })
            .collect();
        let changed = |l: &str, k: &str| -> Option<String> {
            let cur = strings.get(l, k)?;
            let was = orig_strings.get(l).and_then(|m| m.get(k)).map(String::as_str);
            (new || was != Some(cur)).then(|| cur.to_owned())
        };
        for (k, l, v) in h.entries.iter_mut() {
            if let Some(cur) = changed(l, k) {
                *v = cur;
            }
        }
        for k in &wanted {
            if let Some(v) = changed(DEFAULT_LOCALE, k) {
                if !h.entries.iter().any(|(ek, el, _)| ek == k && el == DEFAULT_LOCALE) {
                    h.entries.push((k.clone(), DEFAULT_LOCALE.to_owned(), v));
                }
            }
        }
        put_if_changed(out, DOCUMENT_HEADER, info::write_header(&h));
    }
}

fn save_data(doc: &MapDocument, out: &mut FileMap) {
    let orig = &doc.passthrough;
    let mut by_file: BTreeMap<String, Vec<&strata_core::catalog::CatalogEntry>> = BTreeMap::new();
    for e in &doc.data.entries {
        by_file.entry(e.file.trim_end_matches(".xml").to_owned()).or_default().push(e);
    }
    // Files that were catalogs originally but now have no entries.
    for (path, bytes) in orig {
        if let Some(file) = catalog::file_of(path) {
            if catalog::parse(file, bytes).is_some() && !by_file.keys().any(|k| k.eq_ignore_ascii_case(file)) {
                by_file.insert(file.to_owned(), Vec::new());
            }
        }
    }
    for (file, entries) in by_file {
        let path = catalog::path_of(&file);
        let orig_bytes = lookup(orig, &path);
        if let Some(b) = orig_bytes {
            if let Some(orig_entries) = catalog::parse(&file, b) {
                let same = orig_entries.len() == entries.len()
                    && orig_entries.iter().zip(&entries).all(|(a, b)| a.node == b.node);
                if same {
                    continue;
                }
            }
        }
        put(out, &path, catalog::write(orig_bytes, &entries));
    }
}

fn save_strings(doc: &MapDocument, strings: &strata_core::strings::Strings, out: &mut FileMap) {
    for (loc, map) in &strings.locales {
        let path = strings::path(loc);
        let orig = lookup(&doc.passthrough, &path);
        if orig.is_some_and(|b| strings::parse(b) == *map) {
            continue;
        }
        put(out, &path, strings::write(orig, map));
    }
}

/// New maps get a full component list; existing lists only gain entries for
/// component files this save created. A map without a list is left alone.
fn save_components(doc: &MapDocument, new: bool, out: &mut FileMap) {
    let orig = &doc.passthrough;
    let list = parse_xml(orig, COMPONENT_LIST);
    if list.is_none() && !new {
        return;
    }
    let created = |name: &str| lookup(out, name).is_some() && (new || lookup(orig, name).is_none());
    let mut wanted: Vec<docinfo::Component> = Vec::new();
    if out.keys().any(|k| catalog::file_of(k).is_some() && created(k)) {
        wanted.push(docinfo::comp("gada", None, "GameData"));
    }
    for k in out.keys() {
        if let Some(loc) = strings::locale_of(k) {
            if created(k) {
                wanted.push(docinfo::comp("text", Some(loc), "GameText"));
            }
        }
    }
    for (ty, name) in [
        ("info", DOCUMENT_INFO),
        ("mapi", MAP_INFO),
        ("trig", triggers::EDITOR_TRIGGERS),
        ("terr", terrain::XML),
        ("plob", OBJECTS),
        ("regi", REGIONS),
    ] {
        if created(name) {
            wanted.push(docinfo::comp(ty, None, name));
        }
    }
    let bytes = match &list {
        Some(d) => docinfo::ensure_components(d, &wanted),
        None => Some(docinfo::new_components(&wanted)),
    };
    if let Some(b) = bytes {
        put(out, COMPONENT_LIST, b);
    }
}

pub fn save_files(doc: &MapDocument) -> Result<FileMap, FormatError> {
    let new = is_new(doc);
    let d = if new { DirtyFlags::all() } else { doc.dirty };
    let mut out = doc.passthrough.clone();
    let strings = effective_strings(doc, new);

    if d.info {
        save_info(doc, new, &strings, &mut out);
    }
    if d.terrain || d.info {
        terrain::save(doc, &doc.passthrough, &mut out);
    }
    if d.objects || d.cameras {
        let orig = parse_xml(&doc.passthrough, OBJECTS);
        let absent = lookup(&doc.passthrough, OBJECTS).is_none();
        let empty = doc.objects.objects.is_empty() && doc.cameras.is_empty();
        if orig.is_some() || (absent && (new || !empty)) {
            put_if_changed(&mut out, OBJECTS, objects::write(orig.as_ref(), &doc.objects.objects, &doc.cameras));
        }
    }
    if d.regions {
        let orig = parse_xml(&doc.passthrough, REGIONS);
        if orig.is_some() || (lookup(&doc.passthrough, REGIONS).is_none() && !doc.regions.is_empty()) {
            put_if_changed(&mut out, REGIONS, regions::write(orig.as_ref(), &doc.regions));
        }
    }
    if d.data {
        save_data(doc, &mut out);
    }
    if d.strings || (d.info && strings != doc.strings) {
        save_strings(doc, &strings, &mut out);
    }
    if d.triggers {
        triggers::save(doc, &doc.passthrough, &mut out)?;
    }
    if new && lookup(&out, MINIMAP).is_none() {
        put(&mut out, MINIMAP, terrain::minimap(&doc.terrain));
    }
    save_components(doc, new, &mut out);
    Ok(out)
}

/// Make a saved file set the document's new baseline and clear its dirty flags.
pub fn mark_saved(doc: &mut MapDocument, files: FileMap) {
    doc.passthrough = files;
    doc.dirty = DirtyFlags::default();
}

// ---- archives and folders ----------------------------------------------------

/// MPQ bookkeeping files that are not part of the map's content.
fn is_mpq_internal(name: &str) -> bool {
    matches!(name, "(listfile)" | "(attributes)" | "(signature)" | "(user data)")
}

pub fn load_sc2map(bytes: Vec<u8>) -> Result<MapDocument, FormatError> {
    let archive = strata_mpq::Archive::from_bytes(bytes)?;
    // Files missing from (listfile) can't be named, so a save would silently
    // drop them. Refuse instead of losing data.
    let unlisted = archive.unlisted_count();
    if unlisted > 0 {
        return Err(FormatError::Unsupported(format!(
            "this archive has {unlisted} file(s) that are not in its (listfile), so Strata can't keep them when saving. Open and re-save the map in the StarCraft II editor first."
        )));
    }
    let mut files = FileMap::new();
    for name in archive.files() {
        if is_mpq_internal(&name) {
            continue;
        }
        let data = archive.read(&name)?;
        files.insert(name, data);
    }
    load_files(files)
}

pub fn save_sc2map(doc: &MapDocument) -> Result<Vec<u8>, FormatError> {
    let (bytes, files) = build_sc2map(doc)?;
    verify_sc2map(bytes.clone(), &files)?;
    Ok(bytes)
}

fn build_sc2map(doc: &MapDocument) -> Result<(Vec<u8>, FileMap), FormatError> {
    let files = save_files(doc)?;
    let mut b = strata_mpq::ArchiveBuilder::new();
    for (name, data) in &files {
        b.add(name, data.clone());
    }
    Ok((b.build()?, files))
}

/// Check that an `.SC2Map` archive holds exactly `files`: it opens, every
/// file reads back byte-for-byte, and it contains nothing else.
pub fn verify_sc2map(archive: Vec<u8>, files: &FileMap) -> Result<(), FormatError> {
    let bad = |m: String| FormatError::Verify(m);
    let archive = strata_mpq::Archive::from_bytes(archive).map_err(|e| bad(format!("it doesn't open: {e}")))?;
    for (name, data) in files {
        match archive.read(name) {
            Ok(got) if got == *data => {}
            Ok(got) => return Err(bad(format!("{name} reads back as {} bytes instead of {}", got.len(), data.len()))),
            Err(e) => return Err(bad(format!("{name} can't be read back: {e}"))),
        }
    }
    let stored = archive.files().into_iter().filter(|n| !is_mpq_internal(n)).count();
    if stored != files.len() {
        return Err(bad(format!("it holds {stored} files instead of {}", files.len())));
    }
    Ok(())
}

/// Check that every file of `files` under the folder `root` holds exactly
/// what was written.
#[cfg(not(target_arch = "wasm32"))]
pub fn verify_folder(root: &std::path::Path, files: &FileMap) -> Result<(), FormatError> {
    for (name, data) in files {
        let p = folder_path(root, name)?;
        match std::fs::read(&p) {
            Ok(got) if got == *data => {}
            Ok(got) => {
                return Err(FormatError::Verify(format!(
                    "{name} reads back as {} bytes instead of {}",
                    got.len(),
                    data.len()
                )))
            }
            Err(e) => return Err(FormatError::Verify(format!("{name} can't be read back: {e}"))),
        }
    }
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn io(e: std::io::Error, path: &std::path::Path) -> FormatError {
    FormatError::Io(format!("{}: {e}", path.display()))
}

/// Read every file under an unpacked map folder, keyed by `/`-separated relative path.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_folder(root: &std::path::Path) -> Result<FileMap, FormatError> {
    fn walk(dir: &std::path::Path, prefix: &str, out: &mut FileMap) -> Result<(), FormatError> {
        let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| io(e, dir))?.collect::<Result<_, _>>().map_err(|e| io(e, dir))?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let p = e.path();
            if p.is_dir() {
                walk(&p, &rel, out)?;
            } else {
                out.insert(rel, std::fs::read(&p).map_err(|e| io(e, &p))?);
            }
        }
        Ok(())
    }
    let mut out = FileMap::new();
    walk(root, "", &mut out)?;
    Ok(out)
}

/// `root` joined with a `/`-separated map path, refusing anything that could
/// escape the folder.
#[cfg(not(target_arch = "wasm32"))]
fn folder_path(root: &std::path::Path, name: &str) -> Result<std::path::PathBuf, FormatError> {
    let mut p = root.to_path_buf();
    for part in name.split(['/', '\\']) {
        if part.is_empty() || part == "." || part == ".." || part.contains(':') {
            return Err(FormatError::Io(format!("refusing unsafe path {name:?}")));
        }
        p.push(part);
    }
    Ok(p)
}

/// Write a file set into a folder (creating subfolders). Existing files that
/// are not in `files` are left alone.
#[cfg(not(target_arch = "wasm32"))]
pub fn write_folder(root: &std::path::Path, files: &FileMap) -> Result<(), FormatError> {
    for (name, data) in files {
        let p = folder_path(root, name)?;
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| io(e, parent))?;
        }
        if std::fs::read(&p).ok().as_deref() != Some(data.as_slice()) {
            std::fs::write(&p, data).map_err(|e| io(e, &p))?;
        }
    }
    Ok(())
}

/// Save into an unpacked map folder (what the official editor calls the
/// component format), creating it if needed. Packed files removed in the
/// editor ([`MapDocument::removed_files`]) are deleted from the folder too,
/// along with folders that leaves empty; any other file already in the folder
/// is left alone.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_folder(doc: &MapDocument, root: &std::path::Path) -> Result<(), FormatError> {
    let files = save_files(doc)?;
    std::fs::create_dir_all(root).map_err(|e| io(e, root))?;
    write_folder(root, &files)?;
    verify_folder(root, &files)?;
    for name in &doc.removed_files {
        if lookup(&files, name).is_some() {
            continue;
        }
        let p = folder_path(root, name)?;
        match std::fs::remove_file(&p) {
            Ok(()) => {
                let mut dir = p.parent();
                while let Some(d) = dir.filter(|d| *d != root && d.starts_with(root)) {
                    if std::fs::remove_dir(d).is_err() {
                        break; // not empty
                    }
                    dir = d.parent();
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io(e, &p)),
        }
    }
    Ok(())
}

/// Load a `.SC2Map` archive or an unpacked map folder.
#[cfg(not(target_arch = "wasm32"))]
pub fn load_path(path: &std::path::Path) -> Result<MapDocument, FormatError> {
    if path.is_dir() {
        load_files(read_folder(path)?)
    } else {
        load_sc2map(std::fs::read(path).map_err(|e| io(e, path))?)
    }
}

/// Save as an archive, or as a folder if `path` is an existing directory
/// (see [`save_folder`]).
///
/// An archive is written beside `path` first, read back from disk and
/// checked against what was meant to be written ([`verify_sc2map`]); only
/// then does it replace `path`, so a failed save never damages the map.
#[cfg(not(target_arch = "wasm32"))]
pub fn save_path(doc: &MapDocument, path: &std::path::Path) -> Result<(), FormatError> {
    if path.is_dir() {
        return save_folder(doc, path);
    }
    let (bytes, files) = build_sc2map(doc)?;
    let tmp = path.with_extension("SC2Map.strata-tmp");
    std::fs::write(&tmp, bytes).map_err(|e| io(e, &tmp))?;
    let written = std::fs::read(&tmp).map_err(|e| io(e, &tmp));
    if let Err(e) = written.and_then(|b| verify_sc2map(b, &files)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        io(e, path)
    })
}
