//! Loading the base game's data (units, abilities, names...) from the user's
//! StarCraft II install, and caching it.
//!
//! - Native: on startup a background thread finds the install (the
//!   `STRATA_SC2_DIR` variable or the usual Program Files folder), reuses the
//!   cached index when the game version matches, and otherwise rebuilds it
//!   from CASC. "Load game data…" picks another folder.
//! - Web: the cached index is restored from local storage on startup. "Load
//!   game data…" asks for the StarCraft II folder (File System Access API,
//!   or a folder upload input where that is missing) and reads CASC from it
//!   in the page; nothing leaves the browser.
//!
//! Either way the install is also handed to the game-asset service
//! ([`crate::assets`]) for textures and models: the folder path natively; on
//! the web the opened storage itself. The page remembers the picked folder
//! (IndexedDB) and reopens it on later visits where the browser still grants
//! access, so textures come back without asking again.
//!
//! The index is built from the user's own install at runtime; Strata never
//! ships Blizzard data.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use strata_core::base_data::BaseCatalog;

enum Msg {
    Progress(String),
    Done(Result<Box<BaseCatalog>, String>, Source),
    /// Nothing to load (no install found, picker cancelled).
    Idle(Option<String>),
    /// The install to read game assets (textures, models) from.
    Assets(crate::assets::AssetSource),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Cache,
    Install,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No data loaded and none being loaded; the text says why.
    Missing(String),
    Loading(String),
    Ready,
    Failed(String),
}

pub struct GameData {
    pub status: Status,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    assets: Option<crate::assets::AssetSource>,
}

/// What [`GameData::poll`] tells the app.
pub enum Event {
    Loaded {
        catalog: Arc<BaseCatalog>,
        from_cache: bool,
    },
    Failed(String),
}

impl Default for GameData {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self {
            status: Status::Missing("Game data not loaded".into()),
            tx,
            rx,
            assets: None,
        }
    }
}

impl GameData {
    pub fn is_loading(&self) -> bool {
        matches!(self.status, Status::Loading(_))
    }

    /// Start-up: restore the cache, or (native) find and read the install.
    pub fn start(&mut self, ctx: &egui::Context) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.status = Status::Loading("Looking for StarCraft II…".into());
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || native::autodetect(&tx, &ctx));
        }
        #[cfg(target_arch = "wasm32")]
        {
            let cached = web::restore_cache();
            let version = cached.as_ref().map(|c| c.version.clone());
            match cached {
                Some(cat) => {
                    let _ = self.tx.send(Msg::Done(Ok(Box::new(cat)), Source::Cache));
                }
                None => {
                    self.status =
                        Status::Missing("No game data yet: run \"Load game data from StarCraft II folder…\"".into());
                }
            }
            // Reopen the folder picked last time, where the browser still
            // allows it, for game assets (and a fresh index if needed).
            web::reopen(self.tx.clone(), ctx.clone(), version);
        }
    }

    /// Ask for the StarCraft II folder and (re)build the index from it.
    pub fn pick_folder(&mut self, ctx: &egui::Context) -> Result<(), String> {
        if self.is_loading() {
            return Err("Game data is already loading.".into());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.status = Status::Loading("Choosing the StarCraft II folder…".into());
            let tx = self.tx.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || native::pick(&tx, &ctx));
            Ok(())
        }
        #[cfg(target_arch = "wasm32")]
        {
            web::pick(self.tx.clone(), ctx.clone())?;
            self.status = Status::Loading("Choosing the StarCraft II folder…".into());
            Ok(())
        }
    }

    /// The install found or picked, for [`crate::assets::Assets::attach`].
    /// Call after [`GameData::poll`].
    pub fn take_assets(&mut self) -> Option<crate::assets::AssetSource> {
        self.assets.take()
    }

    /// Drain finished work. Call once per frame.
    pub fn poll(&mut self) -> Option<Event> {
        let mut out = None;
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Progress(p) => self.status = Status::Loading(p),
                Msg::Assets(a) => self.assets = Some(a),
                Msg::Idle(why) => self.status = Status::Missing(why.unwrap_or_else(|| "Game data not loaded".into())),
                Msg::Done(Ok(cat), source) => {
                    self.status = Status::Ready;
                    out = Some(Event::Loaded {
                        catalog: Arc::from(cat),
                        from_cache: source == Source::Cache,
                    });
                }
                Msg::Done(Err(e), _) => {
                    self.status = Status::Failed(e.clone());
                    out = Some(Event::Failed(e));
                }
            }
        }
        out
    }
}

/// Mods read for the index, with a friendly progress line.
fn progress_line(m: &str) -> String {
    format!("Reading {m}.SC2Mod…")
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::path::{Path, PathBuf};

    use strata_core::base_data::{self, BaseCatalog, MELEE_MODS};

    use super::{progress_line, Msg, Source};

    fn cache_file(version: &str) -> Option<PathBuf> {
        let safe: String = version
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '_' })
            .collect();
        eframe::storage_dir("Strata").map(|d| d.join(format!("gamedata-{safe}.bin")))
    }

    fn send(tx: &std::sync::mpsc::Sender<Msg>, ctx: &egui::Context, m: Msg) {
        let _ = tx.send(m);
        ctx.request_repaint();
    }

    /// Cached index for this install's version, or a fresh build.
    fn load(root: &Path, tx: &std::sync::mpsc::Sender<Msg>, ctx: &egui::Context) {
        send(tx, ctx, Msg::Assets(crate::assets::AssetSource::Dir(root.to_path_buf())));
        let version = base_data::install_version(root);
        if let Some(cat) = version
            .as_deref()
            .and_then(cache_file)
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|b| BaseCatalog::from_bytes(&b).ok())
            .filter(|c| {
                Some(&c.version) == version.as_ref() && c.mods.iter().map(String::as_str).eq(MELEE_MODS.iter().copied())
            })
        {
            send(tx, ctx, Msg::Done(Ok(Box::new(cat)), Source::Cache));
            return;
        }
        send(
            tx,
            ctx,
            Msg::Progress(format!("Reading StarCraft II data from {}…", root.display())),
        );
        let result = (|| {
            let storage = strata_casc::block_on(strata_casc::Storage::open_filtered(
                strata_casc::DirSource::new(root),
                base_data::casc_filter,
            ))
            .map_err(|e| e.to_string())?;
            strata_casc::block_on(base_data::load_from_casc(&storage, MELEE_MODS, |m| {
                send(tx, ctx, Msg::Progress(progress_line(m)))
            }))
        })();
        if let Ok(cat) = &result {
            if let Some(p) = cache_file(&cat.version) {
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                if let Err(e) = std::fs::write(&p, cat.to_bytes()) {
                    log::warn!("could not cache game data at {}: {e}", p.display());
                }
            }
        }
        send(
            tx,
            ctx,
            Msg::Done(
                result
                    .map(Box::new)
                    .map_err(|e| format!("Could not read StarCraft II data: {e}")),
                Source::Install,
            ),
        );
    }

    pub fn autodetect(tx: &std::sync::mpsc::Sender<Msg>, ctx: &egui::Context) {
        match base_data::default_install_dirs().into_iter().next() {
            Some(root) => load(&root, tx, ctx),
            None => send(
                tx,
                ctx,
                Msg::Idle(Some(
                    "StarCraft II not found: run \"Load game data from StarCraft II folder…\"".into(),
                )),
            ),
        }
    }

    pub fn pick(tx: &std::sync::mpsc::Sender<Msg>, ctx: &egui::Context) {
        let folder = rfd::FileDialog::new()
            .set_title("Choose your StarCraft II folder")
            .pick_folder();
        match folder {
            Some(root) if root.join(".build.info").is_file() => load(&root, tx, ctx),
            Some(root) => send(
                tx,
                ctx,
                Msg::Done(
                    Err(format!(
                        "{} is not a StarCraft II install folder (it has no .build.info). Pick the folder that contains \"StarCraft II.exe\" and SC2Data.",
                        root.display()
                    )),
                    Source::Install,
                ),
            ),
            None => send(tx, ctx, Msg::Idle(None)),
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub use web::WebSource;

#[cfg(target_arch = "wasm32")]
mod web {
    use std::future::Future;

    use js_sys::{Function, Promise, Reflect, Uint8Array};
    use strata_casc::CascError;
    use strata_core::base_data::{self, BaseCatalog, MELEE_MODS};
    use wasm_bindgen::{JsCast, JsValue};
    use wasm_bindgen_futures::JsFuture;

    use super::{progress_line, Msg, Source};

    const CACHE_KEY: &str = "strata.gamedata.v1";

    fn window_fn(name: &str) -> Option<(web_sys::Window, Function)> {
        let w = web_sys::window()?;
        let f = Reflect::get(&w, &name.into()).ok()?.dyn_into::<Function>().ok()?;
        Some((w, f))
    }

    fn js_err(v: JsValue) -> String {
        v.as_string().unwrap_or_else(|| format!("{v:?}"))
    }

    async fn call(name: &str, args: &[JsValue]) -> Result<JsValue, String> {
        let (w, f) = window_fn(name).ok_or_else(|| format!("{name} is missing from index.html"))?;
        let arr = js_sys::Array::new();
        for a in args {
            arr.push(a);
        }
        let p = f.apply(&w, &arr).map_err(js_err)?;
        JsFuture::from(Promise::resolve(&p)).await.map_err(js_err)
    }

    /// Reads the folder the user picked, through the page's JS glue.
    pub struct WebSource;

    impl strata_casc::Source for WebSource {
        fn read(&self, path: &str, offset: u64, len: Option<u64>) -> impl Future<Output = Result<Vec<u8>, CascError>> {
            let path = path.to_owned();
            async move {
                let len = len.map(|l| JsValue::from_f64(l as f64)).unwrap_or(JsValue::NULL);
                let v = call(
                    "strataSc2Read",
                    &[path.clone().into(), JsValue::from_f64(offset as f64), len],
                )
                .await
                .map_err(|e| CascError::Io(format!("{path}: {e}")))?;
                Ok(Uint8Array::new(&v).to_vec())
            }
        }

        fn list(&self, dir: &str) -> impl Future<Output = Result<Vec<String>, CascError>> {
            let dir = dir.to_owned();
            async move {
                let v = call("strataSc2List", &[dir.clone().into()])
                    .await
                    .map_err(|e| CascError::Io(format!("{dir}: {e}")))?;
                Ok(js_sys::Array::from(&v).iter().filter_map(|s| s.as_string()).collect())
            }
        }
    }

    fn local_storage() -> Option<web_sys::Storage> {
        web_sys::window()?.local_storage().ok().flatten()
    }

    pub fn restore_cache() -> Option<BaseCatalog> {
        let text = local_storage()?.get_item(CACHE_KEY).ok()??;
        BaseCatalog::from_bytes(&crate::io::base64::decode(&text)?).ok()
    }

    fn save_cache(cat: &BaseCatalog) {
        if let Some(s) = local_storage() {
            if s.set_item(CACHE_KEY, &crate::io::base64::encode(&cat.to_bytes()))
                .is_err()
            {
                let _ = s.remove_item(CACHE_KEY);
                log::warn!("game data is too large for local storage; it will be re-read next time");
            }
        }
    }

    /// Whether this browser can open a folder at all.
    fn support() -> Result<(), String> {
        let ok = window_fn("strataSc2Support")
            .and_then(|(w, f)| f.call0(&w).ok())
            .and_then(|v| v.as_string())
            .is_some_and(|s| !s.is_empty());
        if ok {
            Ok(())
        } else {
            Err("This browser can't open folders, so it can't read your StarCraft II install. Use Chrome or Edge, or the Strata desktop app.".into())
        }
    }

    pub fn pick(tx: std::sync::mpsc::Sender<Msg>, ctx: egui::Context) -> Result<(), String> {
        support()?;
        wasm_bindgen_futures::spawn_local(async move {
            let picked = match call("strataSc2Pick", &[]).await {
                Ok(v) => v.as_string(),
                Err(e) => {
                    let _ = tx.send(Msg::Done(Err(format!("Could not open the folder: {e}")), Source::Install));
                    ctx.request_repaint();
                    return;
                }
            };
            match picked {
                Some(name) => open_and_load(&tx, &ctx, name, None).await,
                None => {
                    let _ = tx.send(Msg::Idle(None));
                    ctx.request_repaint();
                }
            }
        });
        Ok(())
    }

    /// Start-up: reopen the folder picked in an earlier session if the
    /// browser still grants access without asking (the page keeps the
    /// folder handle in IndexedDB). Rebuilds the index only when the cached
    /// one (`cached_version`) is missing or from another game version.
    pub fn reopen(tx: std::sync::mpsc::Sender<Msg>, ctx: egui::Context, cached_version: Option<String>) {
        if window_fn("strataSc2Restore").is_none() {
            return;
        }
        wasm_bindgen_futures::spawn_local(async move {
            match call("strataSc2Restore", &[]).await {
                Ok(v) => {
                    if let Some(name) = v.as_string() {
                        open_and_load(&tx, &ctx, name, cached_version).await;
                    }
                }
                Err(e) => log::info!("previous StarCraft II folder not reopened: {e}"),
            }
        });
    }

    /// Opens the CASC storage of the folder the page holds, builds the index
    /// unless `cached_version` already matches, and hands the storage to the
    /// asset service.
    async fn open_and_load(
        tx: &std::sync::mpsc::Sender<Msg>,
        ctx: &egui::Context,
        name: String,
        cached_version: Option<String>,
    ) {
        let send = |m: Msg| {
            let _ = tx.send(m);
            ctx.request_repaint();
        };
        let fresh = cached_version.is_none();
        if fresh {
            send(Msg::Progress(format!("Reading StarCraft II data from {name}…")));
        }
        let storage = match strata_casc::Storage::open_filtered(WebSource, crate::assets::full_filter).await {
            Ok(s) => s,
            Err(e) => {
                let e = match e {
                    CascError::NotAnInstall(why) => format!(
                        "\"{name}\" is not a StarCraft II install folder ({why}). Pick the folder that contains \"StarCraft II.exe\" and SC2Data."
                    ),
                    e => e.to_string(),
                };
                if fresh {
                    send(Msg::Done(Err(format!("Could not read StarCraft II data: {e}")), Source::Install));
                } else {
                    log::warn!("previous StarCraft II folder: {e}");
                }
                return;
            }
        };
        if cached_version.as_deref() != Some(storage.build.version.as_str()) {
            if !fresh {
                send(Msg::Progress(format!("Reading StarCraft II data from {name}…")));
            }
            let result =
                base_data::load_from_casc(&storage, MELEE_MODS, |m| send(Msg::Progress(progress_line(m)))).await;
            if let Ok(cat) = &result {
                save_cache(cat);
            }
            send(Msg::Done(
                result.map(Box::new).map_err(|e| format!("Could not read StarCraft II data: {e}")),
                Source::Install,
            ));
        }
        send(Msg::Assets(crate::assets::AssetSource::Web(Box::new(storage), name)));
    }
}
