//! On-demand access to the user's own StarCraft II game assets (terrain
//! textures, `.m3` models and their textures) read from the install's CASC
//! storage at runtime. Strata never ships Blizzard files.
//!
//! Contract (shared by the terrain and model renderers):
//! - Every getter is non-blocking. The first call for a path queues a
//!   background load and returns [`Asset::Loading`]; later calls return
//!   [`Asset::Ready`] or [`Asset::Missing`]. Results are cached.
//! - When something finishes loading, the service requests an egui repaint,
//!   so callers simply ask again next frame.
//! - Paths are CASC paths with `/` separators, matched case-insensitively,
//!   e.g. `mods/core.sc2mod/base.sc2data/assets/textures/foo.dds`
//!   (see `resolve` for mod-relative lookups like `Assets\Textures\foo.dds`).
//!
//! Backends:
//! - Native: [`AssetSource::Dir`] names the install folder. A thread opens
//!   the CASC storage (filtered to the melee mods' `.dds`/`.m3` assets and
//!   GameData), then two worker threads serve a FIFO request queue.
//! - Web: [`AssetSource::Web`] hands over a storage the page already opened
//!   through the user's folder (see `game_data.rs`); requests run as
//!   `spawn_local` tasks, a few at a time, reading through the same File
//!   System Access / folder-input source.
//!
//! Memory is bounded: decoded textures and raw files live in LRU caches with
//! byte budgets. Entries still loading are never evicted, and an evicted
//! entry simply loads again on the next request.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard};

use strata_casc::{normalize_path, Source, Storage};
use strata_core::base_data::MELEE_MODS;

/// State of one requested asset.
#[derive(Debug, Clone)]
pub enum Asset<T> {
    Loading,
    Ready(T),
    Missing,
}

impl<T> Asset<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Asset::Ready(t) => Some(t),
            _ => None,
        }
    }
}

/// Where the game files come from; handed to [`Assets::attach`] by the
/// game-data loader.
pub enum AssetSource {
    /// The install folder (contains `.build.info` and `SC2Data`).
    #[cfg(not(target_arch = "wasm32"))]
    Dir(std::path::PathBuf),
    /// A storage the page opened from the folder the user picked, plus a
    /// name for it (the folder's name).
    #[cfg(target_arch = "wasm32")]
    Web(Box<web::Store>, String),
}

/// Diffuse and normal map of a terrain texture (`CTerrainTex`), as written
/// in game data (`Assets\Textures\Zerus_1.dds`); pass them through
/// [`Assets::resolve`]. Either may be absent.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TerrainTex {
    pub diffuse: Option<String>,
    pub normal: Option<String>,
}

/// Paths a storage opened for assets must keep (normalized paths): the
/// melee mods' textures and models. GameData (for `CTerrainTex`) comes from
/// [`strata_core::base_data::casc_filter`], which callers OR in.
pub fn casc_filter(path: &str) -> bool {
    // Campaign and Co-op roots: maps depending on Co-op (and the campaigns it
    // builds on) place units whose models and actor data live there.
    const EXTRA_ROOTS: [&str; 5] = [
        "campaigns/liberty.sc2campaign/",
        "campaigns/swarm.sc2campaign/",
        "campaigns/void.sc2campaign/",
        "mods/starcoop/starcoop.sc2mod/",
        "mods/novastoryassets.sc2mod/",
    ];
    if let Some(rest) = EXTRA_ROOTS.iter().find_map(|r| path.strip_prefix(r)) {
        let asset = rest.starts_with("base.sc2assets/") && (rest.ends_with(".dds") || rest.ends_with(".m3"));
        let data = rest.starts_with("base.sc2data/gamedata/")
            && (rest.ends_with("actordata.xml") || rest.ends_with("modeldata.xml"));
        return asset || data;
    }
    let Some(rest) = path.strip_prefix("mods/") else { return false };
    let Some((m, rest)) = rest.split_once(".sc2mod/") else { return false };
    rest.starts_with("base.sc2assets/")
        && (rest.ends_with(".dds") || rest.ends_with(".m3"))
        && MELEE_MODS.iter().any(|x| x.eq_ignore_ascii_case(m))
}

/// The filter for a storage that serves both the base catalog and assets.
pub fn full_filter(path: &str) -> bool {
    strata_core::base_data::casc_filter(path) || casc_filter(path)
}

/// Byte budgets for the caches.
#[cfg(not(target_arch = "wasm32"))]
const TEXTURE_BUDGET: usize = 384 << 20;
#[cfg(target_arch = "wasm32")]
const TEXTURE_BUDGET: usize = 160 << 20;
#[cfg(not(target_arch = "wasm32"))]
const FILE_BUDGET: usize = 128 << 20;
#[cfg(target_arch = "wasm32")]
const FILE_BUDGET: usize = 48 << 20;

/// One load request.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Job {
    File(String),
    Texture(String, u32),
}

enum Outcome {
    File(Asset<Arc<[u8]>>),
    Texture(Asset<Arc<strata_dds::Image>>),
}

/// A byte-bounded LRU cache of asset states.
struct Lru<K, V> {
    map: HashMap<K, (Asset<V>, usize, u64)>,
    bytes: usize,
    budget: usize,
    tick: u64,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    fn new(budget: usize) -> Self {
        Self { map: HashMap::new(), bytes: 0, budget, tick: 0 }
    }

    fn get(&mut self, k: &K) -> Option<Asset<V>> {
        self.tick += 1;
        let tick = self.tick;
        self.map.get_mut(k).map(|e| {
            e.2 = tick;
            e.0.clone()
        })
    }

    fn put(&mut self, k: K, v: Asset<V>, size: usize) {
        self.tick += 1;
        if let Some(old) = self.map.insert(k, (v, size, self.tick)) {
            self.bytes -= old.1;
        }
        self.bytes += size;
        while self.bytes > self.budget {
            let Some(victim) = self
                .map
                .iter()
                .filter(|(_, e)| matches!(e.0, Asset::Ready(_)) && e.1 > 0)
                .min_by_key(|(_, e)| e.2)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(e) = self.map.remove(&victim) {
                self.bytes -= e.1;
            }
        }
    }

    fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }
}

#[derive(Debug, Clone, PartialEq)]
// The web backend receives an already opened storage, so it never opens or fails.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
enum Phase {
    Detached,
    Opening,
    Open,
    Failed(String),
}

/// `CTerrainTex` entries of the melee mods, ids lowercased.
#[derive(Default)]
struct TerrainCatalog {
    textures: HashMap<String, TerrainTex>,
}

struct Inner {
    phase: Phase,
    /// Identifies the attached install, so re-attaching it is a no-op.
    source_key: Option<String>,
    /// Bumped on every attach; results of older loads are dropped.
    epoch: u64,
    ctx: Option<egui::Context>,
    files: Lru<String, Arc<[u8]>>,
    textures: Lru<(String, u32), Arc<strata_dds::Image>>,
    resolved: HashMap<String, Option<String>>,
    terrain: Option<Arc<TerrainCatalog>>,
    /// Requests made while the storage was still opening.
    waiting: Vec<Job>,
    /// Finished loads so far (lets callers skip work when nothing changed).
    revision: u64,
    #[cfg(not(target_arch = "wasm32"))]
    native: Option<native::Running>,
    #[cfg(target_arch = "wasm32")]
    in_flight: usize,
}

impl Default for Inner {
    fn default() -> Self {
        Self {
            phase: Phase::Detached,
            source_key: None,
            epoch: 0,
            ctx: None,
            files: Lru::new(FILE_BUDGET),
            textures: Lru::new(TEXTURE_BUDGET),
            resolved: HashMap::new(),
            terrain: None,
            waiting: Vec::new(),
            revision: 0,
            #[cfg(not(target_arch = "wasm32"))]
            native: None,
            #[cfg(target_arch = "wasm32")]
            in_flight: 0,
        }
    }
}

impl Inner {
    fn finish(&mut self, job: Job, out: Outcome) {
        match (job, out) {
            (Job::File(p), Outcome::File(a)) => {
                let size = a.ready().map_or(0, |b| b.len());
                self.files.put(p, a, size);
            }
            (Job::Texture(p, m), Outcome::Texture(a)) => {
                let size = a.ready().map_or(0, |i| i.rgba.len());
                self.textures.put((p, m), a, size);
            }
            _ => {}
        }
        self.revision += 1;
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

#[derive(Default)]
pub struct Assets {
    inner: Arc<Mutex<Inner>>,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Assets {
    /// Connect a game install. Re-attaching the same install keeps the
    /// caches; a different one starts over.
    pub fn attach(&self, source: AssetSource, ctx: &egui::Context) {
        let mut g = lock(&self.inner);
        g.ctx = Some(ctx.clone());
        let key = source_key(&source);
        if g.source_key.as_deref() == Some(key.as_str()) && matches!(g.phase, Phase::Opening | Phase::Open) {
            return;
        }
        g.epoch += 1;
        g.source_key = Some(key);
        g.files.clear();
        g.textures.clear();
        g.resolved.clear();
        g.terrain = None;
        g.waiting.clear();
        // Loads of the old source drop their results without touching the
        // new source's slot count.
        #[cfg(target_arch = "wasm32")]
        {
            g.in_flight = 0;
        }
        g.revision += 1;
        match source {
            #[cfg(not(target_arch = "wasm32"))]
            AssetSource::Dir(root) => {
                g.phase = Phase::Opening;
                if let Some(old) = g.native.take() {
                    old.close();
                }
                native::open(self.inner.clone(), g.epoch, root);
            }
            #[cfg(target_arch = "wasm32")]
            AssetSource::Web(store, _) => {
                g.phase = Phase::Open;
                web::set_store(*store);
                web::load_terrain_catalog(self.inner.clone(), g.epoch);
            }
        }
    }

    /// Whether a game install is attached and assets can load at all.
    pub fn available(&self) -> bool {
        matches!(lock(&self.inner).phase, Phase::Opening | Phase::Open)
    }

    /// Why assets are unavailable, if they are (for status lines).
    pub fn problem(&self) -> Option<String> {
        match &lock(&self.inner).phase {
            Phase::Failed(e) => Some(e.clone()),
            _ => None,
        }
    }

    /// Counts finished loads; unchanged means no getter result changed.
    pub fn revision(&self) -> u64 {
        lock(&self.inner).revision
    }

    /// Raw bytes of a game file.
    pub fn file(&self, path: &str) -> Asset<Arc<[u8]>> {
        let path = normalize_path(path);
        let mut g = lock(&self.inner);
        if !matches!(g.phase, Phase::Opening | Phase::Open) {
            return Asset::Missing;
        }
        if let Some(a) = g.files.get(&path) {
            return a;
        }
        g.files.put(path.clone(), Asset::Loading, 0);
        self.submit(&mut g, Job::File(path));
        Asset::Loading
    }

    /// A decoded DDS texture, using the largest mip whose longer side is at
    /// most `max_side` pixels.
    pub fn texture(&self, path: &str, max_side: u32) -> Asset<Arc<strata_dds::Image>> {
        let key = (normalize_path(path), max_side.max(1));
        let mut g = lock(&self.inner);
        if !matches!(g.phase, Phase::Opening | Phase::Open) {
            return Asset::Missing;
        }
        if let Some(a) = g.textures.get(&key) {
            return a;
        }
        g.textures.put(key.clone(), Asset::Loading, 0);
        self.submit(&mut g, Job::Texture(key.0, key.1));
        Asset::Loading
    }

    /// Resolve a path as written in game data (e.g. `Assets\Units\Terran\Marine\Marine.m3`)
    /// to a CASC path by searching the dependency mods (Core, Liberty, Swarm,
    /// Void and their Multi variants, last mod wins). `None` if not found.
    pub fn resolve(&self, data_path: &str) -> Option<String> {
        let p = normalize_path(data_path);
        let mut g = lock(&self.inner);
        if g.phase != Phase::Open {
            return None;
        }
        if let Some(r) = g.resolved.get(&p) {
            return r.clone();
        }
        #[cfg(not(target_arch = "wasm32"))]
        let r = g.native.as_ref().and_then(|n| resolve_in(&n.store, &p));
        #[cfg(target_arch = "wasm32")]
        let r = web::with_store(|s| resolve_in(s, &p)).flatten();
        g.resolved.insert(p, r.clone());
        r
    }

    /// The diffuse and normal map of terrain texture `id` (e.g. "Zerus3",
    /// from `CTerrainTex` in the base game's TerrainTexData).
    pub fn terrain_texture(&self, id: &str) -> Asset<TerrainTex> {
        let g = lock(&self.inner);
        match (&g.phase, &g.terrain) {
            (Phase::Opening | Phase::Open, None) => Asset::Loading,
            (Phase::Open, Some(cat)) => match cat.textures.get(&id.to_ascii_lowercase()) {
                Some(t) => Asset::Ready(t.clone()),
                None => Asset::Missing,
            },
            _ => Asset::Missing,
        }
    }

    fn submit(&self, g: &mut Inner, job: Job) {
        match g.phase {
            Phase::Opening => g.waiting.push(job),
            #[cfg(not(target_arch = "wasm32"))]
            Phase::Open => match &g.native {
                Some(n) => n.push(job),
                None => g.waiting.push(job),
            },
            #[cfg(target_arch = "wasm32")]
            Phase::Open => web::spawn(&self.inner, g, job),
            _ => {}
        }
    }
}

fn source_key(s: &AssetSource) -> String {
    match s {
        #[cfg(not(target_arch = "wasm32"))]
        AssetSource::Dir(p) => p.display().to_string(),
        #[cfg(target_arch = "wasm32")]
        AssetSource::Web(_, name) => format!("web:{name}"),
    }
}

/// See [`Assets::resolve`]. `p` is normalized.
fn resolve_in<S: Source>(store: &Storage<S>, p: &str) -> Option<String> {
    if store.find(p).is_some() {
        return Some(p.to_owned());
    }
    MELEE_MODS.iter().rev().find_map(|m| {
        let m = m.to_ascii_lowercase();
        ["base.sc2assets", "base.sc2data"].iter().find_map(|c| {
            let cand = format!("mods/{m}.sc2mod/{c}/{p}");
            store.find(&cand).is_some().then_some(cand)
        })
    })
}

/// Loads one request from the storage.
async fn run<S: Source>(store: &Storage<S>, job: &Job) -> Outcome {
    match job {
        Job::File(p) => Outcome::File(match store.read(p).await {
            Ok(b) => Asset::Ready(Arc::from(b)),
            Err(e) => {
                log::debug!("asset {p}: {e}");
                Asset::Missing
            }
        }),
        Job::Texture(p, max) => Outcome::Texture(match store.read(p).await {
            Ok(b) => match strata_dds::decode_max_side(&b, *max) {
                Ok(img) => Asset::Ready(Arc::new(img)),
                Err(e) => {
                    log::warn!("texture {p}: {e}");
                    Asset::Missing
                }
            },
            Err(e) => {
                log::debug!("texture {p}: {e}");
                Asset::Missing
            }
        }),
    }
}

/// Reads the melee mods' `CTerrainTex` entries (later mods override).
async fn read_terrain_catalog<S: Source>(store: &Storage<S>) -> TerrainCatalog {
    use strata_core::catalog::XmlNode;
    #[derive(Default)]
    struct Raw {
        texture: Option<String>,
        normal: Option<String>,
        parent: Option<String>,
    }
    let mut raw: HashMap<String, Raw> = HashMap::new();
    for m in MELEE_MODS {
        let path = format!("mods/{}.sc2mod/base.sc2data/gamedata/terraintexdata.xml", m.to_ascii_lowercase());
        if store.find(&path).is_none() {
            continue;
        }
        let Ok(bytes) = store.read(&path).await else { continue };
        let text = String::from_utf8_lossy(&bytes);
        let Ok(root) = XmlNode::parse(text.trim_start_matches('\u{feff}')) else { continue };
        for e in root.children.iter().filter(|c| c.name == "CTerrainTex") {
            let Some(id) = e.attr("id") else { continue };
            let entry = raw.entry(id.to_ascii_lowercase()).or_default();
            for c in &e.children {
                let v = c.attr("value").map(str::to_owned);
                if c.name.eq_ignore_ascii_case("Texture") {
                    entry.texture = v;
                } else if c.name.eq_ignore_ascii_case("NormalMap") {
                    entry.normal = v;
                }
            }
            if let Some(p) = e.attr("parent") {
                entry.parent = Some(p.to_ascii_lowercase());
            }
        }
    }
    // A field missing from an entry comes from its parent.
    let lookup = |id: &str, normal: bool| -> Option<String> {
        let mut cur = id.to_owned();
        for _ in 0..8 {
            let e = raw.get(&cur)?;
            let v = if normal { &e.normal } else { &e.texture };
            if v.is_some() {
                return v.clone();
            }
            cur = e.parent.clone()?;
        }
        None
    };
    let textures =
        raw.keys().map(|id| (id.clone(), TerrainTex { diffuse: lookup(id, false), normal: lookup(id, true) })).collect();
    TerrainCatalog { textures }
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::{Arc, Condvar, Mutex};

    use strata_casc::{block_on, DirSource, Storage};

    use super::{lock, read_terrain_catalog, run, Inner, Job, Phase};

    pub type Store = Storage<DirSource>;

    /// Worker threads serving one opened storage.
    const WORKERS: usize = 2;

    #[derive(Default)]
    struct Queue {
        jobs: VecDeque<Job>,
        closed: bool,
    }

    pub struct Running {
        pub store: Arc<Store>,
        queue: Arc<(Mutex<Queue>, Condvar)>,
    }

    impl Running {
        pub fn push(&self, job: Job) {
            let (m, cv) = &*self.queue;
            m.lock().unwrap_or_else(|e| e.into_inner()).jobs.push_back(job);
            cv.notify_one();
        }

        pub fn close(&self) {
            let (m, cv) = &*self.queue;
            m.lock().unwrap_or_else(|e| e.into_inner()).closed = true;
            cv.notify_all();
        }
    }

    pub fn open(inner: Arc<Mutex<Inner>>, epoch: u64, root: PathBuf) {
        std::thread::Builder::new()
            .name("strata-assets-open".into())
            .spawn(move || {
                let t = std::time::Instant::now();
                let opened = block_on(Storage::open_filtered(DirSource::new(&root), super::full_filter));
                let opened = match opened {
                    Ok(store) => {
                        let store = Arc::new(store);
                        let cat = block_on(read_terrain_catalog(&store));
                        log::info!(
                            "game assets: {} files, {} terrain textures, opened in {:?}",
                            store.files().len(),
                            cat.textures.len(),
                            t.elapsed()
                        );
                        Ok((store, cat))
                    }
                    Err(e) => Err(e),
                };
                let mut g = lock(&inner);
                if g.epoch != epoch {
                    return;
                }
                match opened {
                    Ok((store, cat)) => {
                        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
                        for i in 0..WORKERS {
                            let (inner, store, queue) = (inner.clone(), store.clone(), queue.clone());
                            let _ = std::thread::Builder::new()
                                .name(format!("strata-assets-{i}"))
                                .spawn(move || work(inner, epoch, store, queue));
                        }
                        let running = Running { store, queue };
                        for j in std::mem::take(&mut g.waiting) {
                            running.push(j);
                        }
                        g.native = Some(running);
                        g.terrain = Some(Arc::new(cat));
                        g.phase = Phase::Open;
                        g.revision += 1;
                    }
                    Err(e) => {
                        log::warn!("game assets unavailable: {e}");
                        g.phase = Phase::Failed(format!("Could not read game assets: {e}"));
                        g.waiting.clear();
                        g.files.clear();
                        g.textures.clear();
                        g.revision += 1;
                    }
                }
                if let Some(ctx) = &g.ctx {
                    ctx.request_repaint();
                }
            })
            .expect("spawn asset thread");
    }

    fn work(inner: Arc<Mutex<Inner>>, epoch: u64, store: Arc<Store>, queue: Arc<(Mutex<Queue>, Condvar)>) {
        loop {
            let job = {
                let (m, cv) = &*queue;
                let mut q = m.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if q.closed {
                        return;
                    }
                    if let Some(j) = q.jobs.pop_front() {
                        break j;
                    }
                    q = cv.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            let t = std::time::Instant::now();
            let out = block_on(run(&store, &job));
            log::debug!("loaded {job:?} in {:?}", t.elapsed());
            let mut g = lock(&inner);
            if g.epoch != epoch {
                return;
            }
            g.finish(job, out);
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub mod web {
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::{Arc, Mutex};

    use strata_casc::Storage;

    use super::{lock, read_terrain_catalog, run, Inner, Job};

    pub type Store = Storage<crate::game_data::WebSource>;

    /// Loads running at once; more wait their turn.
    const MAX_IN_FLIGHT: usize = 4;

    thread_local! {
        static STORE: RefCell<Option<Rc<Store>>> = const { RefCell::new(None) };
    }

    pub(super) fn set_store(s: Store) {
        STORE.with(|c| *c.borrow_mut() = Some(Rc::new(s)));
    }

    fn store() -> Option<Rc<Store>> {
        STORE.with(|c| c.borrow().clone())
    }

    pub(super) fn with_store<R>(f: impl FnOnce(&Store) -> R) -> Option<R> {
        store().map(|s| f(&s))
    }

    pub(super) fn load_terrain_catalog(inner: Arc<Mutex<Inner>>, epoch: u64) {
        let Some(s) = store() else { return };
        wasm_bindgen_futures::spawn_local(async move {
            let cat = read_terrain_catalog(&s).await;
            log::info!("game assets: {} files, {} terrain textures", s.files().len(), cat.textures.len());
            let mut g = lock(&inner);
            if g.epoch == epoch {
                g.terrain = Some(Arc::new(cat));
                g.revision += 1;
                if let Some(ctx) = &g.ctx {
                    ctx.request_repaint();
                }
            }
        });
    }

    pub(super) fn spawn(inner: &Arc<Mutex<Inner>>, g: &mut Inner, job: Job) {
        if g.in_flight >= MAX_IN_FLIGHT {
            g.waiting.push(job);
            return;
        }
        let Some(s) = store() else { return };
        g.in_flight += 1;
        let epoch = g.epoch;
        let inner = inner.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let out = run(&s, &job).await;
            let next = {
                let mut g = lock(&inner);
                if g.epoch != epoch {
                    return;
                }
                g.in_flight -= 1;
                g.finish(job, out);
                (!g.waiting.is_empty()).then(|| g.waiting.remove(0))
            };
            if let Some(j) = next {
                let mut g = lock(&inner);
                spawn(&inner, &mut g, j);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_keeps_melee_assets_only() {
        assert!(casc_filter("mods/liberty.sc2mod/base.sc2assets/assets/textures/zerus_3.dds"));
        assert!(casc_filter("mods/voidmulti.sc2mod/base.sc2assets/assets/units/x/x.m3"));
        assert!(!casc_filter("mods/war3data.sc2mod/base.sc2assets/assets/textures/a.dds"));
        assert!(!casc_filter("mods/liberty.sc2mod/enus.sc2assets/assets/textures/a.dds"));
        assert!(!casc_filter("mods/liberty.sc2mod/base.sc2assets/assets/sounds/a.wav"));
        assert!(full_filter("mods/swarm.sc2mod/base.sc2data/gamedata/terraintexdata.xml"));
    }

    #[test]
    fn lru_evicts_oldest_ready_entries() {
        let mut c: Lru<u32, u32> = Lru::new(100);
        c.put(1, Asset::Ready(1), 60);
        c.put(2, Asset::Loading, 0);
        c.put(3, Asset::Ready(3), 30);
        assert!(c.get(&1).is_some(), "touch 1 so 3 is older");
        c.put(4, Asset::Ready(4), 30);
        assert!(c.get(&3).is_none(), "oldest ready entry evicted");
        assert!(matches!(c.get(&2), Some(Asset::Loading)), "loading entries stay");
        assert!(c.bytes <= 100);
    }

    #[test]
    fn detached_service_reports_missing() {
        let a = Assets::default();
        assert!(!a.available());
        assert!(matches!(a.texture("x.dds", 512), Asset::Missing));
        assert!(matches!(a.file("x.m3"), Asset::Missing));
        assert!(matches!(a.terrain_texture("Zerus3"), Asset::Missing));
        assert_eq!(a.resolve("Assets\\Textures\\x.dds"), None);
    }
}
