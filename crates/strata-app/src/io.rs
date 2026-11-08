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
        project: bool,
    },
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

fn open_dialog() -> rfd::AsyncFileDialog {
    rfd::AsyncFileDialog::new()
        .set_title("Open map")
        .add_filter("StarCraft II map or Strata project", &["SC2Map", "json"])
        .add_filter("StarCraft II map", &["SC2Map"])
        .add_filter("Strata project", &["json"])
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
        self.send_from_worker(ctx, || match block_on(open_dialog().pick_file()) {
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
        let p = std::path::PathBuf::from(&path);
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
                    let project = path
                        .to_string_lossy()
                        .to_ascii_lowercase()
                        .ends_with(".json");
                    self.write_native(doc, &path, project).map(Some)
                }
                _ => {
                    self.ask_save_path(ctx, doc, file_name, false);
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
            self.ask_save_path(ctx, doc, file_name, true);
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

    #[cfg(not(target_arch = "wasm32"))]
    fn ask_save_path(
        &mut self,
        ctx: &egui::Context,
        doc: &MapDocument,
        file_name: Option<&str>,
        project: bool,
    ) {
        self.busy = Some("Choosing where to save…".into());
        let file = if project {
            project_name(doc, file_name)
        } else {
            sc2map_name(doc, file_name)
        };
        let dir = self
            .save_path
            .as_ref()
            .and_then(|p| p.parent())
            .map(|d| d.to_path_buf());
        self.send_from_worker(ctx, move || {
            let dialog = if project {
                rfd::AsyncFileDialog::new()
                    .set_title("Export project")
                    .add_filter("Strata project", &["json"])
            } else {
                rfd::AsyncFileDialog::new()
                    .set_title("Save map as")
                    .add_filter("StarCraft II map", &["SC2Map"])
            };
            let dialog = dialog.set_file_name(file);
            let dialog = match dir {
                Some(dir) => dialog.set_directory(dir),
                None => dialog,
            };
            match block_on(dialog.save_file()) {
                Some(h) => IoEvent::SaveTo {
                    path: h.path().to_path_buf(),
                    project,
                },
                None => IoEvent::Cancelled,
            }
        });
    }

    /// Native: write `doc` to `path` now. Remembers `.SC2Map` paths for Save.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn write_native(
        &mut self,
        doc: &MapDocument,
        path: &std::path::Path,
        project: bool,
    ) -> Result<String, SaveError> {
        let name = file_name_of(path);
        if project {
            let bytes = serde_json::to_vec(doc).map_err(|e| SaveError {
                message: format!("Could not export the project: {e}."),
                offer_export: false,
            })?;
            std::fs::write(path, bytes).map_err(|e| SaveError {
                message: format!(
                    "Could not write {name}: {e}. Check that the folder exists and is writable."
                ),
                offer_export: false,
            })?;
        } else {
            strata_format::save_path(doc, path).map_err(|e| SaveError {
                message: format!("Strata could not write {name}: {e}."),
                offer_export: true,
            })?;
            self.save_path = Some(path.to_path_buf());
        }
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
        if !modified {
            if !self.autosave_cleared {
                clear_autosave();
                self.autosave_cleared = true;
                self.autosaved_revision = Some(revision);
            }
            self.last_autosave = now;
            return None;
        }
        self.autosave_cleared = false;
        if now - self.last_autosave < AUTOSAVE_INTERVAL || self.autosaved_revision == Some(revision)
        {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(AUTOSAVE_INTERVAL));
            return None;
        }
        self.last_autosave = now;
        self.autosaved_revision = Some(revision);
        match write_autosave(doc, file_name) {
            Ok(()) => None,
            Err(e) if !self.autosave_warned => {
                self.autosave_warned = true;
                Some(e)
            }
            Err(_) => None,
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
mod base64 {
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
