//! Opening, saving, downloading, drag-and-drop, autosave and the few browser
//! hooks the app needs (PWA install, update notice, page title).
//!
//! File dialogs and loading run off the UI thread (a thread on native, a
//! future on the web). Results come back through a channel that the app
//! polls once per frame with [`Io::poll`].

use std::sync::mpsc::{channel, Receiver, Sender};

use strata_core::MapDocument;

/// Extension of a Strata project: the whole document as JSON, lossless.
pub const PROJECT_EXT: &str = ".strata.json";
const AUTOSAVE_INTERVAL: f64 = 30.0;

/// What a save writes (native; the web downloads archives and projects).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveKind {
    /// A `.SC2Map` archive file.
    Archive,
    /// An unpacked map folder named `*.SC2Map` (the official editor's
    /// component format).
    Folder,
    /// A Strata project (`.strata.json`).
    Project,
}

/// Files picked or dropped for importing into the map.
pub struct ImportFiles {
    /// (name, bytes): a file name, or a path relative to an imported folder.
    pub files: Vec<(String, Vec<u8>)>,
    /// Names of files that aren't models, textures, sounds or fonts.
    pub skipped: Vec<String>,
}

/// Results delivered back to the UI thread.
pub enum IoEvent {
    Opened {
        doc: Box<MapDocument>,
        name: String,
        path: Option<String>,
    },
    OpenFailed {
        message: String,
    },
    /// Native: the user picked where to save.
    #[cfg(not(target_arch = "wasm32"))]
    SaveTo {
        path: std::path::PathBuf,
        kind: SaveKind,
    },
    /// Files to import into the open map.
    Import(ImportFiles),
    /// A map image was saved (toast text), or why not.
    ImageSaved(Result<String, String>),
    Cancelled,
}

/// What the app shows after a failed save.
pub struct SaveError {
    pub message: String,
    /// The `.SC2Map` writer failed; a project export would still work.
    pub offer_export: bool,
}

pub struct Io {
    tx: Sender<IoEvent>,
    rx: Receiver<IoEvent>,
    /// Shown in the status bar while a dialog or load is in flight.
    pub busy: Option<String>,
    /// Native: where Save writes without asking.
    #[cfg(not(target_arch = "wasm32"))]
    pub save_path: Option<std::path::PathBuf>,
    last_autosave: f64,
    autosaved_revision: Option<u64>,
    autosave_cleared: bool,
    autosave_warned: bool,
    /// Native: an autosave being written on a background thread (maps with
    /// large packed files take seconds to encode).
    #[cfg(not(target_arch = "wasm32"))]
    autosave_job: Option<Receiver<Result<(), String>>>,
}

impl Default for Io {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            busy: None,
            #[cfg(not(target_arch = "wasm32"))]
            save_path: None,
            last_autosave: 0.0,
            autosaved_revision: None,
            autosave_cleared: false,
            autosave_warned: false,
            #[cfg(not(target_arch = "wasm32"))]
            autosave_job: None,
        }
    }
}

/// Parse a map from bytes, by file name: a Strata project or an `.SC2Map`.
pub fn load_bytes(name: &str, bytes: Vec<u8>) -> Result<MapDocument, String> {
    if name.to_ascii_lowercase().ends_with(".json") {
        serde_json::from_slice(&bytes)
            .map_err(|e| format!("{name} is not a valid Strata project ({e}). Projects are made with File › Export project."))
    } else {
        strata_format::load_sc2map(bytes).map_err(|e| format!("Strata could not read {name}: {e}."))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn load_native(path: &std::path::Path) -> Result<MapDocument, String> {
    let name = file_name_of(path);
    if path.is_file() && name.to_ascii_lowercase().ends_with(".json") {
        let bytes = std::fs::read(path).map_err(|e| format!("Could not read {name}: {e}."))?;
        load_bytes(&name, bytes)
    } else {
        strata_format::load_path(path).map_err(|e| format!("Strata could not read {name}: {e}."))
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn file_name_of(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// File types for the Open dialog. Native: a map folder also opens by
/// picking any file in it, such as its ComponentList.SC2Components.
#[cfg(not(target_arch = "wasm32"))]
const OPEN_FILTERS: &[(&str, &[&str])] = &[
    ("StarCraft II map, map folder or Strata project", &["SC2Map", "SC2Components", "json"]),
    ("StarCraft II map", &["SC2Map"]),
    ("Strata project", &["json"]),
];
#[cfg(target_arch = "wasm32")]
const OPEN_FILTERS: &[(&str, &[&str])] = &[
    ("StarCraft II map or Strata project", &["SC2Map", "json"]),
    ("StarCraft II map", &["SC2Map"]),
    ("Strata project", &["json"]),
];

fn open_dialog() -> rfd::AsyncFileDialog {
    OPEN_FILTERS.iter().fold(rfd::AsyncFileDialog::new().set_title("Open map"), |d, (name, exts)| d.add_filter(*name, exts))
}

/// Native: ask for a map to open. On Windows, opening a map folder in the
/// dialog picks it rather than browsing into it.
#[cfg(not(target_arch = "wasm32"))]
fn pick_map_to_open() -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    match crate::win_dialog::open("Open map", OPEN_FILTERS) {
        Ok(picked) => return picked.map(|p| map_root(&p)),
        Err(e) => log::warn!("Windows open dialog failed ({e}); using the portable one"),
    }
    block_on(open_dialog().pick_file()).map(|h| map_root(h.path()))
}

/// The map to open for a picked path: a file inside a map folder (named
/// `*.SC2Map`) opens that folder; anything else opens as itself.
#[cfg(not(target_arch = "wasm32"))]
fn map_root(path: &std::path::Path) -> std::path::PathBuf {
    let is_map = |p: &std::path::Path| p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().ends_with(".sc2map"));
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if path.is_dir() || is_map(path) || lower.ends_with(".json") {
        return path.to_path_buf();
    }
    path.ancestors().skip(1).find(|a| a.is_dir() && is_map(a)).map(|a| a.to_path_buf()).unwrap_or_else(|| path.to_path_buf())
}

/// Run a future to completion on the current (non-UI) thread.
#[cfg(not(target_arch = "wasm32"))]
fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    use std::sync::Arc;
    use std::task::{Context, Poll, Wake, Waker};
    struct ThreadWaker(std::thread::Thread);
    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// Every model, texture, sound and font under `root`, named by their path
/// relative to it (`Voices/hello.mp3`).
#[cfg(not(target_arch = "wasm32"))]
fn collect_folder(root: &std::path::Path, out: &mut ImportFiles) {
    fn walk(dir: &std::path::Path, prefix: &str, out: &mut ImportFiles, budget: &mut usize) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let p = e.path();
            if p.is_dir() {
                walk(&p, &rel, out, budget);
            } else if strata_core::map_files::FileKind::of(&rel).is_none() || *budget == 0 {
                out.skipped.push(rel);
            } else if let Ok(bytes) = std::fs::read(&p) {
                *budget = budget.saturating_sub(1);
                out.files.push((rel, bytes));
            }
        }
    }
    let mut budget = 5000;
    walk(root, "", out, &mut budget);
}

/// Sort one picked or dropped file into `out`.
fn add_import(out: &mut ImportFiles, name: String, bytes: Vec<u8>) {
    if strata_core::map_files::FileKind::of(&name).is_some() {
        out.files.push((name, bytes));
    } else {
        out.skipped.push(name);
    }
}

/// Should these dropped files be imported into the open map (models,
/// textures, sounds, or a plain folder of them) rather than opened as a map?
pub fn is_import_drop(files: &[egui::DroppedFileHandle]) -> bool {
    files.iter().any(|f| {
        let path = f.path();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".sc2map") || lower.ends_with(".json") {
            return false;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if path.is_dir() {
            return true;
        }
        strata_core::map_files::FileKind::of(&name).is_some()
    })
}

/// A file name in `dir` for `name` that doesn't clash with an existing
/// entry of the other kind (a folder when saving a file, a file when
/// saving a folder): `Map.SC2Map`, else `Map 2.SC2Map`, `Map 3.SC2Map`...
#[cfg(not(target_arch = "wasm32"))]
fn free_name(dir: Option<&std::path::Path>, name: String, folder: bool) -> String {
    let Some(dir) = dir else { return name };
    let clash = |n: &str| {
        let p = dir.join(n);
        if folder { p.is_file() } else { p.is_dir() }
    };
    if !clash(&name) {
        return name;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s.to_owned(), format!(".{e}")),
        None => (name.clone(), String::new()),
    };
    (2..100).map(|n| format!("{stem} {n}{ext}")).find(|n| !clash(n)).unwrap_or(name)
}

impl Io {
    #[cfg(not(target_arch = "wasm32"))]
    fn send_from_worker(
        &self,
        ctx: &egui::Context,
        work: impl FnOnce() -> IoEvent + Send + 'static,
    ) {
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
    }

    /// Show the open dialog and load the chosen map.
    pub fn open(&mut self, ctx: &egui::Context) {
        // Browsers do not always report a cancelled file picker, so the web
        // build shows no busy state while the picker is open.
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.busy = Some("Choosing a map…".into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.send_from_worker(ctx, || match pick_map_to_open() {
            Some(path) => {
                let name = file_name_of(&path);
                match load_native(&path) {
                    Ok(doc) => IoEvent::Opened {
                        doc: Box::new(doc),
                        name,
                        path: Some(path.display().to_string()),
                    },
                    Err(message) => IoEvent::OpenFailed { message },
                }
            }
            None => IoEvent::Cancelled,
        });
        #[cfg(target_arch = "wasm32")]
        {
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let ev = match open_dialog().pick_file().await {
                    Some(h) => {
                        let name = h.file_name();
                        let bytes = h.read().await;
                        match load_bytes(&name, bytes) {
                            Ok(doc) => IoEvent::Opened {
                                doc: Box::new(doc),
                                name,
                                path: None,
                            },
                            Err(message) => IoEvent::OpenFailed { message },
                        }
                    }
                    None => IoEvent::Cancelled,
                };
                let _ = tx.send(ev);
                ctx.request_repaint();
            });
        }
    }

    /// Native: open an unpacked map folder.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_folder(&mut self, ctx: &egui::Context) {
        self.busy = Some("Choosing a map folder…".into());
        self.send_from_worker(ctx, || {
            match block_on(
                rfd::AsyncFileDialog::new()
                    .set_title("Open map folder")
                    .pick_folder(),
            ) {
                Some(h) => {
                    let path = h.path().to_path_buf();
                    let name = file_name_of(&path);
                    match load_native(&path) {
                        Ok(doc) => IoEvent::Opened {
                            doc: Box::new(doc),
                            name,
                            path: Some(path.display().to_string()),
                        },
                        Err(message) => IoEvent::OpenFailed { message },
                    }
                }
                None => IoEvent::Cancelled,
            }
        });
    }

    /// Native: reopen a recent file by path.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_path(&mut self, ctx: &egui::Context, path: String) {
        let p = map_root(std::path::Path::new(&path));
        let path = p.display().to_string();
        let name = file_name_of(&p);
        self.busy = Some(format!("Opening {name}…"));
        self.send_from_worker(ctx, move || match load_native(&p) {
            Ok(doc) => IoEvent::Opened {
                doc: Box::new(doc),
                name,
                path: Some(path),
            },
            Err(message) => IoEvent::OpenFailed { message },
        });
    }

    /// Load files dropped onto the window (the first one wins).
    pub fn open_dropped(&mut self, ctx: &egui::Context, file: egui::DroppedFileHandle) {
        let name = file
            .path()
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "dropped file".into());
        self.busy = Some(format!("Opening {name}…"));
        #[cfg(not(target_arch = "wasm32"))]
        self.send_from_worker(ctx, move || {
            let path = file.path().to_path_buf();
            let result = if path.is_dir() {
                load_native(&path)
            } else if path.as_os_str().is_empty() || !path.exists() {
                file.bytes()
                    .map_err(|e| format!("Could not read {name}: {e}."))
                    .and_then(|b| load_bytes(&name, b))
            } else {
                load_native(&path)
            };
            let shown = (!path.as_os_str().is_empty()).then(|| path.display().to_string());
            match result {
                Ok(doc) => IoEvent::Opened {
                    doc: Box::new(doc),
                    name,
                    path: shown,
                },
                Err(message) => IoEvent::OpenFailed { message },
            }
        });
        #[cfg(target_arch = "wasm32")]
        {
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let ev = match file.bytes_async().await {
                    Ok(bytes) => match load_bytes(&name, bytes) {
                        Ok(doc) => IoEvent::Opened {
                            doc: Box::new(doc),
                            name,
                            path: None,
                        },
                        Err(message) => IoEvent::OpenFailed { message },
                    },
                    Err(e) => IoEvent::OpenFailed {
                        message: format!("The browser could not read {name}: {e}."),
                    },
                };
                let _ = tx.send(ev);
                ctx.request_repaint();
            });
        }
    }

    /// Pick models, textures and sounds to import (or, native, a folder of them).
    pub fn pick_import(&mut self, ctx: &egui::Context, folder: bool) {
        const EXTS: [&str; 13] =
            ["m3", "m3a", "dds", "tga", "png", "jpg", "mp3", "ogg", "wav", "ttf", "otf", "ogv", "SC2Layout"];
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.busy = Some("Choosing files to import…".into());
            self.send_from_worker(ctx, move || {
                let mut out = ImportFiles { files: Vec::new(), skipped: Vec::new() };
                if folder {
                    match block_on(rfd::AsyncFileDialog::new().set_title("Import a folder of assets").pick_folder()) {
                        Some(h) => collect_folder(h.path(), &mut out),
                        None => return IoEvent::Cancelled,
                    }
                } else {
                    let dialog = rfd::AsyncFileDialog::new()
                        .set_title("Import models, textures and sounds")
                        .add_filter("Models, textures and sounds", &EXTS)
                        .add_filter("All files", &["*"]);
                    let Some(handles) = block_on(dialog.pick_files()) else { return IoEvent::Cancelled };
                    for h in handles {
                        match std::fs::read(h.path()) {
                            Ok(b) => add_import(&mut out, file_name_of(h.path()), b),
                            Err(_) => out.skipped.push(file_name_of(h.path())),
                        }
                    }
                }
                IoEvent::Import(out)
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = folder;
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let dialog = rfd::AsyncFileDialog::new()
                    .set_title("Import models, textures and sounds")
                    .add_filter("Models, textures and sounds", &EXTS);
                let ev = match dialog.pick_files().await {
                    Some(handles) => {
                        let mut out = ImportFiles { files: Vec::new(), skipped: Vec::new() };
                        for h in handles {
                            let name = h.file_name();
                            let bytes = h.read().await;
                            add_import(&mut out, name, bytes);
                        }
                        IoEvent::Import(out)
                    }
                    None => IoEvent::Cancelled,
                };
                let _ = tx.send(ev);
                ctx.request_repaint();
            });
        }
    }

    /// Import files (and, native, folders) dropped onto the window.
    pub fn import_dropped(&mut self, ctx: &egui::Context, dropped: Vec<egui::DroppedFileHandle>) {
        self.busy = Some("Reading dropped files…".into());
        #[cfg(not(target_arch = "wasm32"))]
        self.send_from_worker(ctx, move || {
            let mut out = ImportFiles { files: Vec::new(), skipped: Vec::new() };
            for f in dropped {
                let p = f.path().to_path_buf();
                if p.is_dir() {
                    collect_folder(&p, &mut out);
                    continue;
                }
                let name = file_name_of(&p);
                match f.bytes() {
                    Ok(b) => add_import(&mut out, name, b),
                    Err(_) => out.skipped.push(name),
                }
            }
            IoEvent::Import(out)
        });
        #[cfg(target_arch = "wasm32")]
        {
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let mut out = ImportFiles { files: Vec::new(), skipped: Vec::new() };
                for f in dropped {
                    let name = f.path().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    match f.bytes_async().await {
                        Ok(b) => add_import(&mut out, name, b),
                        Err(_) => out.skipped.push(name),
                    }
                }
                let _ = tx.send(IoEvent::Import(out));
                ctx.request_repaint();
            });
        }
    }

    /// Next finished background result, if any.
    pub fn poll(&mut self) -> Option<IoEvent> {
        let ev = self.rx.try_recv().ok()?;
        self.busy = None;
        Some(ev)
    }

    /// Save as `.SC2Map`. Native writes to the remembered path, or asks;
    /// the web downloads the archive. Returns `Ok(Some(name))` when saved,
    /// `Ok(None)` when a dialog is now open.
    pub fn save(
        &mut self,
        ctx: &egui::Context,
        doc: &MapDocument,
        file_name: Option<&str>,
        save_as: bool,
    ) -> Result<Option<String>, SaveError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            match (&self.save_path, save_as) {
                (Some(path), false) => {
                    let path = path.clone();
                    let kind = if path.is_dir() {
                        SaveKind::Folder
                    } else if path.to_string_lossy().to_ascii_lowercase().ends_with(".json") {
                        SaveKind::Project
                    } else {
                        SaveKind::Archive
                    };
                    self.write_native(doc, &path, kind).map(Some)
                }
                _ => {
                    self.ask_save_path(ctx, doc, file_name, SaveKind::Archive);
                    Ok(None)
                }
            }
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (ctx, save_as);
            let name = sc2map_name(doc, file_name);
            let bytes = strata_format::save_sc2map(doc).map_err(|e| SaveError {
                message: format!("Strata could not write {name}: {e}."),
                offer_export: true,
            })?;
            download(&name, &bytes, "application/octet-stream").map_err(|e| SaveError {
                message: e,
                offer_export: true,
            })?;
            Ok(Some(name))
        }
    }

    /// Export the document as a Strata project (always lossless).
    pub fn export_project(
        &mut self,
        ctx: &egui::Context,
        doc: &MapDocument,
        file_name: Option<&str>,
    ) -> Result<Option<String>, SaveError> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ask_save_path(ctx, doc, file_name, SaveKind::Project);
            Ok(None)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = ctx;
            let name = project_name(doc, file_name);
            let bytes = serde_json::to_vec(doc).map_err(|e| SaveError {
                message: format!("Could not export the project: {e}."),
                offer_export: false,
            })?;
            download(&name, &bytes, "application/json").map_err(|e| SaveError {
                message: e,
                offer_export: false,
            })?;
            Ok(Some(name))
        }
    }

    /// Save a PNG image: native asks where (then writes it on a worker);
    /// the web downloads it. `summary` describes the image for the toast,
    /// e.g. "2048 × 1536 pixels".
    pub fn save_png(&mut self, ctx: &egui::Context, name: String, png: Vec<u8>, summary: String) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.busy = Some("Choosing where to save the image…".into());
            let dir = self.save_path.as_ref().and_then(|p| p.parent()).map(|d| d.to_path_buf());
            self.send_from_worker(ctx, move || {
                let path = {
                    #[cfg(windows)]
                    {
                        match crate::win_dialog::save("Export map image", &[("PNG image", &["png"])], &name, dir.as_deref()) {
                            Ok(p) => Some(p),
                            Err(e) => {
                                log::warn!("Windows save dialog failed ({e}); using the portable one");
                                None
                            }
                        }
                    }
                    #[cfg(not(windows))]
                    {
                        None::<Option<std::path::PathBuf>>
                    }
                };
                let path = match path {
                    Some(p) => p,
                    None => {
                        let d = rfd::AsyncFileDialog::new()
                            .set_title("Export map image")
                            .add_filter("PNG image", &["png"])
                            .set_file_name(&name);
                        let d = match &dir {
                            Some(dir) => d.set_directory(dir),
                            None => d,
                        };
                        block_on(d.save_file()).map(|h| h.path().to_path_buf())
                    }
                };
                let Some(path) = path else { return IoEvent::Cancelled };
                IoEvent::ImageSaved(match std::fs::write(&path, &png) {
                    Ok(()) => Ok(format!("Saved {} ({summary})", path.display())),
                    Err(e) => Err(format!(
                        "Strata couldn't write {} ({e}). Check that the folder exists and is writable.",
                        path.display()
                    )),
                })
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = ctx;
            let r = download(&name, &png, "image/png").map(|()| format!("Downloaded {name} ({summary})"));
            let _ = self.tx.send(IoEvent::ImageSaved(r));
        }
    }

    /// Native: ask where to save the map as an unpacked map folder.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_as_folder(&mut self, ctx: &egui::Context, doc: &MapDocument, file_name: Option<&str>) {
        self.ask_save_path(ctx, doc, file_name, SaveKind::Folder);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn ask_save_path(&mut self, ctx: &egui::Context, doc: &MapDocument, file_name: Option<&str>, kind: SaveKind) {
        self.busy = Some("Choosing where to save…".into());
        let dir = self.save_path.as_ref().and_then(|p| p.parent()).map(|d| d.to_path_buf());
        let file = match kind {
            SaveKind::Project => project_name(doc, file_name),
            SaveKind::Archive => free_name(dir.as_deref(), sc2map_name(doc, file_name), false),
            SaveKind::Folder => free_name(dir.as_deref(), sc2map_name(doc, file_name), true),
        };
        self.send_from_worker(ctx, move || {
            // Windows: picking an existing map folder chooses it instead
            // of browsing into it.
            #[cfg(windows)]
            if !matches!(kind, SaveKind::Project) {
                let (title, filter): (&str, &[(&str, &[&str])]) = match kind {
                    SaveKind::Folder => ("Save map as a map folder", &[("StarCraft II map folder", &["SC2Map"])]),
                    _ => ("Save map as a .SC2Map file", &[("StarCraft II map", &["SC2Map"])]),
                };
                match crate::win_dialog::save(title, filter, &file, dir.as_deref()) {
                    Ok(Some(path)) => return IoEvent::SaveTo { path, kind },
                    Ok(None) => return IoEvent::Cancelled,
                    Err(e) => log::warn!("Windows save dialog failed ({e}); using the portable one"),
                }
            }
            let dialog = match kind {
                SaveKind::Project => {
                    rfd::AsyncFileDialog::new().set_title("Export project").add_filter("Strata project", &["json"])
                }
                SaveKind::Archive => rfd::AsyncFileDialog::new()
                    .set_title("Save map as a .SC2Map file")
                    .add_filter("StarCraft II map", &["SC2Map"]),
                SaveKind::Folder => rfd::AsyncFileDialog::new()
                    .set_title("Save map as a map folder")
                    .add_filter("StarCraft II map folder", &["SC2Map"]),
            };
            let dialog = dialog.set_file_name(file);
            let dialog = match dir {
                Some(dir) => dialog.set_directory(dir),
                None => dialog,
            };
            match block_on(dialog.save_file()) {
                Some(h) => IoEvent::SaveTo { path: h.path().to_path_buf(), kind },
                None => IoEvent::Cancelled,
            }
        });
    }

    /// Native: write `doc` to `path` now. Remembers map paths (files and
    /// folders) for Save.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_native(
        &mut self,
        doc: &MapDocument,
        path: &std::path::Path,
        kind: SaveKind,
    ) -> Result<String, SaveError> {
        let name = file_name_of(path);
        let fail = |message: String, offer_export: bool| SaveError { message, offer_export };
        match kind {
            SaveKind::Project => {
                let bytes =
                    serde_json::to_vec(doc).map_err(|e| fail(format!("Could not export the project: {e}."), false))?;
                std::fs::write(path, bytes).map_err(|e| {
                    fail(format!("Could not write {name}: {e}. Check that the folder exists and is writable."), false)
                })?;
                return Ok(name);
            }
            SaveKind::Archive => {
                if path.is_dir() {
                    return Err(fail(
                        format!(
                            "There is already a map folder named {name} there. Choose another name, or use Save to update that folder."
                        ),
                        false,
                    ));
                }
                strata_format::save_path(doc, path)
                    .map_err(|e| fail(format!("Strata could not write {name}: {e}."), true))?;
            }
            SaveKind::Folder => {
                let current = self.save_path.as_deref() == Some(path);
                if path.is_file() {
                    // The save dialog already asked to replace the file:
                    // write the folder beside it first, then swap.
                    let tmp = path.with_extension("SC2Map.strata-tmp");
                    let _ = std::fs::remove_dir_all(&tmp);
                    strata_format::save_folder(doc, &tmp).map_err(|e| {
                        let _ = std::fs::remove_dir_all(&tmp);
                        fail(format!("Strata could not write {name}: {e}."), true)
                    })?;
                    std::fs::remove_file(path).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| {
                        fail(
                            format!("Strata wrote the map folder to {} but couldn't replace {name} ({e}).", tmp.display()),
                            false,
                        )
                    })?;
                } else {
                    let occupied =
                        path.is_dir() && std::fs::read_dir(path).map(|mut d| d.next().is_some()).unwrap_or(false);
                    if occupied && !current {
                        return Err(fail(
                            format!(
                                "{name} already exists and isn't the map you have open. Choose another name, or delete that folder first."
                            ),
                            false,
                        ));
                    }
                    strata_format::save_folder(doc, path)
                        .map_err(|e| fail(format!("Strata could not write {name}: {e}."), true))?;
                }
            }
        }
        self.save_path = Some(path.to_path_buf());
        Ok(name)
    }

    /// Forget the current path (after New).
    pub fn reset_path(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.save_path = None;
        }
    }

    /// Remember the path a map was opened from, when Save can write there.
    pub fn set_opened_path(&mut self, path: Option<&str>) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.save_path = path
                .map(std::path::PathBuf::from)
                .filter(|p| !p.to_string_lossy().to_ascii_lowercase().ends_with(".json"));
        }
        #[cfg(target_arch = "wasm32")]
        let _ = path;
    }

    /// Autosave every 30 s while the map has unsaved changes; clear the
    /// autosave once everything is saved. Returns a warning to show, once.
    pub fn tick_autosave(
        &mut self,
        ctx: &egui::Context,
        doc: &MapDocument,
        modified: bool,
        revision: u64,
        file_name: Option<&str>,
    ) -> Option<String> {
        let now = ctx.input(|i| i.time);
        #[cfg_attr(target_arch = "wasm32", allow(unused_mut))]
        let mut warning = None;
        // A write still running finishes before the autosave is cleared or
        // written again, so a late write can never resurrect a cleared one.
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(job) = &self.autosave_job {
            match job.try_recv() {
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(500));
                    return None;
                }
                Ok(Err(e)) if !self.autosave_warned => {
                    self.autosave_warned = true;
                    warning = Some(e);
                }
                _ => {}
            }
            self.autosave_job = None;
        }
        if !modified {
            if !self.autosave_cleared {
                clear_autosave();
                self.autosave_cleared = true;
                self.autosaved_revision = Some(revision);
            }
            self.last_autosave = now;
            return warning;
        }
        self.autosave_cleared = false;
        if now - self.last_autosave < AUTOSAVE_INTERVAL || self.autosaved_revision == Some(revision)
        {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(AUTOSAVE_INTERVAL));
            return warning;
        }
        self.last_autosave = now;
        self.autosaved_revision = Some(revision);
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (doc, name) = (Box::new(doc.clone()), file_name.map(str::to_owned));
            let (tx, rx) = channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(write_autosave(&doc, name.as_deref()));
                ctx.request_repaint();
            });
            self.autosave_job = Some(rx);
            warning
        }
        #[cfg(target_arch = "wasm32")]
        match write_autosave(doc, file_name) {
            Ok(()) => warning,
            Err(e) if !self.autosave_warned => {
                self.autosave_warned = true;
                Some(e)
            }
            Err(_) => warning,
        }
    }

    /// The document on screen is the autosave itself; don't clear it until
    /// the user saves or discards.
    pub fn note_restored(&mut self, revision: u64) {
        self.autosaved_revision = Some(revision);
        self.autosave_cleared = false;
    }
}

fn base_name(doc: &MapDocument, file_name: Option<&str>) -> String {
    let raw = file_name.unwrap_or(&doc.info.name);
    let lower = raw.to_ascii_lowercase();
    let stem = if lower.ends_with(PROJECT_EXT) {
        &raw[..raw.len() - PROJECT_EXT.len()]
    } else if lower.ends_with(".sc2map") || lower.ends_with(".json") {
        &raw[..raw.rfind('.').unwrap_or(raw.len())]
    } else {
        raw
    };
    let clean: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_.()".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "Untitled map".into()
    } else {
        clean.into()
    }
}

pub fn sc2map_name(doc: &MapDocument, file_name: Option<&str>) -> String {
    format!("{}.SC2Map", base_name(doc, file_name))
}

/// Suggested file name for an exported map image.
pub fn image_name(doc: &MapDocument, file_name: Option<&str>) -> String {
    format!("{}.png", base_name(doc, file_name))
}

pub fn project_name(doc: &MapDocument, file_name: Option<&str>) -> String {
    format!("{}{PROJECT_EXT}", base_name(doc, file_name))
}

// ---------------------------------------------------------------------------
// Autosave

/// What autosave stores: the document plus enough to label it.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct AutosaveRecord {
    /// Unix time in milliseconds.
    pub saved_at_ms: f64,
    pub file_name: Option<String>,
    pub doc: MapDocument,
}

pub fn now_ms() -> f64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64() * 1000.0)
            .unwrap_or(0.0)
    }
}

/// "just now", "4 minutes ago", "2 days ago".
pub fn relative_time(then_ms: f64) -> String {
    let secs = ((now_ms() - then_ms) / 1000.0).max(0.0) as u64;
    let (n, unit) = match secs {
        0..=59 => return "just now".into(),
        60..=3599 => (secs / 60, "minute"),
        3600..=86_399 => (secs / 3600, "hour"),
        _ => (secs / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

fn encode_record(doc: &MapDocument, file_name: Option<&str>) -> Result<Vec<u8>, String> {
    #[derive(serde::Serialize)]
    struct Ref<'a> {
        saved_at_ms: f64,
        file_name: Option<&'a str>,
        doc: &'a MapDocument,
    }
    let json = serde_json::to_vec(&Ref {
        saved_at_ms: now_ms(),
        file_name,
        doc,
    })
    .map_err(|e| format!("Autosave failed: {e}."))?;
    Ok(miniz_oxide::deflate::compress_to_vec(&json, 3))
}

fn decode_record(bytes: &[u8]) -> Option<AutosaveRecord> {
    let json = miniz_oxide::inflate::decompress_to_vec(bytes).ok()?;
    serde_json::from_slice(&json).ok()
}

#[cfg(not(target_arch = "wasm32"))]
fn autosave_file() -> Option<std::path::PathBuf> {
    eframe::storage_dir("Strata").map(|d| d.join("autosave.strata.deflate"))
}

#[cfg(target_arch = "wasm32")]
const AUTOSAVE_KEY: &str = "strata.autosave.v1";

#[cfg(target_arch = "wasm32")]
fn local_storage() -> Option<web_sys::Storage> {
    web_sys::window()?.local_storage().ok().flatten()
}

fn write_autosave(doc: &MapDocument, file_name: Option<&str>) -> Result<(), String> {
    let bytes = encode_record(doc, file_name)?;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let path =
            autosave_file().ok_or("Autosave is off: Strata could not find an app data folder.")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("Autosave failed: {e}."))?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, bytes).map_err(|e| format!("Autosave failed: {e}."))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("Autosave failed: {e}."))?;
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let storage =
            local_storage().ok_or("Autosave is off: this browser blocks local storage.")?;
        let text = base64::encode(&bytes);
        storage.set_item(AUTOSAVE_KEY, &text).map_err(|_| {
            let _ = storage.remove_item(AUTOSAVE_KEY);
            "Autosave is off for this map: it is too large for browser storage. Save the map or export the project to keep your changes."
                .to_owned()
        })
    }
}

/// The autosaved document from a previous session, if any.
pub fn restore_autosave() -> Option<AutosaveRecord> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        decode_record(&std::fs::read(autosave_file()?).ok()?)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let text = local_storage()?.get_item(AUTOSAVE_KEY).ok()??;
        decode_record(&base64::decode(&text)?)
    }
}

pub fn clear_autosave() {
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(p) = autosave_file() {
        let _ = std::fs::remove_file(p);
    }
    #[cfg(target_arch = "wasm32")]
    if let Some(s) = local_storage() {
        let _ = s.remove_item(AUTOSAVE_KEY);
    }
}

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
pub(crate) mod base64 {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    pub fn encode(data: &[u8]) -> String {
        let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    pub fn decode(text: &str) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(text.len() / 4 * 3);
        let mut buf = 0u32;
        let mut bits = 0;
        for c in text.bytes().filter(|&c| c != b'=') {
            let v = ALPHABET.iter().position(|&a| a == c)? as u32;
            buf = buf << 6 | v;
            bits += 6;
            if bits >= 8 {
                bits -= 8;
                out.push((buf >> bits) as u8);
                buf &= (1 << bits) - 1;
            }
        }
        Some(out)
    }
}

// ---------------------------------------------------------------------------
// Browser hooks (no-ops on native)

#[cfg(target_arch = "wasm32")]
fn download(name: &str, bytes: &[u8], mime: &str) -> Result<(), String> {
    use wasm_bindgen::JsCast;
    let fail = |_| {
        format!("The browser blocked the download of {name}. Allow downloads for this site and try again.")
    };
    let parts = js_sys::Array::new();
    parts.push(&js_sys::Uint8Array::from(bytes));
    let props = web_sys::BlobPropertyBag::new();
    props.set_type(mime);
    let blob =
        web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &props).map_err(fail)?;
    let url = web_sys::Url::create_object_url_with_blob(&blob).map_err(fail)?;
    let window = web_sys::window().ok_or("No browser window.")?;
    let document = window.document().ok_or("No browser document.")?;
    let a: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(fail)?
        .dyn_into()
        .map_err(|_| "Could not start the download.")?;
    a.set_href(&url);
    a.set_download(name);
    a.click();
    // Revoke later: revoking right away can cancel the download in some browsers.
    let revoke = wasm_bindgen::closure::Closure::once_into_js(move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    let _ = window
        .set_timeout_with_callback_and_timeout_and_arguments_0(revoke.unchecked_ref(), 30_000);
    Ok(())
}

/// Call `window[name]()` and return its result.
#[cfg(target_arch = "wasm32")]
fn call_window_fn(name: &str) -> Option<wasm_bindgen::JsValue> {
    use wasm_bindgen::JsCast;
    let window = web_sys::window()?;
    let f = js_sys::Reflect::get(&window, &name.into())
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    f.call0(&window).ok()
}

/// Whether the browser offered to install Strata as an app.
pub fn can_install() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        call_window_fn("strataCanInstall")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

pub fn install() {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = call_window_fn("strataInstall");
    }
}

/// A new version of the app finished downloading (service worker).
pub fn update_ready() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        call_window_fn("strataUpdateReady")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

pub fn reload_for_update() {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = call_window_fn("strataReload");
    }
}

/// Set the window or browser tab title.
pub fn set_title(ctx: &egui::Context, title: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.to_owned()));
    #[cfg(target_arch = "wasm32")]
    {
        let _ = ctx;
        if let Some(d) = web_sys::window().and_then(|w| w.document()) {
            d.set_title(title);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn files_inside_a_map_folder_open_the_folder() {
        let dir = std::env::temp_dir().join(format!("strata-map-root-{}", std::process::id()));
        let map = dir.join("Hero.SC2Map");
        std::fs::create_dir_all(map.join("Base.SC2Data")).unwrap();
        std::fs::write(map.join("ComponentList.SC2Components"), b"x").unwrap();
        std::fs::write(map.join("Base.SC2Data").join("x.xml"), b"x").unwrap();
        std::fs::write(dir.join("loose.txt"), b"x").unwrap();
        assert_eq!(map_root(&map.join("ComponentList.SC2Components")), map);
        assert_eq!(map_root(&map.join("Base.SC2Data").join("x.xml")), map);
        assert_eq!(map_root(&map), map);
        assert_eq!(map_root(&dir.join("loose.txt")), dir.join("loose.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn base64_round_trip() {
        for data in [
            &b""[..],
            b"f",
            b"fo",
            b"foo",
            b"foob",
            b"fooba",
            b"foobar",
            &[0, 255, 128, 7],
        ] {
            let e = base64::encode(data);
            assert_eq!(base64::decode(&e).unwrap(), data);
        }
        assert_eq!(base64::encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn autosave_record_round_trip() {
        let doc = MapDocument::new("Test", 16, 16);
        let bytes = encode_record(&doc, Some("Test.SC2Map")).unwrap();
        let rec = decode_record(&bytes).unwrap();
        assert_eq!(rec.doc.info.name, "Test");
        assert_eq!(rec.file_name.as_deref(), Some("Test.SC2Map"));
    }

    #[test]
    fn project_round_trip_is_lossless() {
        let doc = MapDocument::new("Round trip", 24, 16);
        let bytes = serde_json::to_vec(&doc).unwrap();
        let back = load_bytes("x.strata.json", bytes).unwrap();
        assert_eq!(
            serde_json::to_value(&doc).unwrap(),
            serde_json::to_value(&back).unwrap()
        );
    }

    #[test]
    fn file_names() {
        let doc = MapDocument::new("My map", 8, 8);
        assert_eq!(sc2map_name(&doc, None), "My map.SC2Map");
        assert_eq!(sc2map_name(&doc, Some("Arena.SC2Map")), "Arena.SC2Map");
        assert_eq!(
            project_name(&doc, Some("Arena.SC2Map")),
            "Arena.strata.json"
        );
        assert_eq!(
            project_name(&doc, Some("Arena.strata.json")),
            "Arena.strata.json"
        );
        assert_eq!(relative_time(now_ms()), "just now");
    }

}
