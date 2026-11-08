//! The editing target. Tool calls go to the connected editor when there is
//! one (see [`crate::bridge`]), otherwise to a headless document kept here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};
use strata_core::history::History;
use strata_core::ops::link::{self, LinkKind};
use strata_core::ops::query::Query;
use strata_core::MapDocument;

use crate::bridge::Bridge;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Editor,
    Headless,
}

impl Target {
    pub fn as_str(self) -> &'static str {
        match self {
            Target::Editor => "editor",
            Target::Headless => "headless",
        }
    }
}

/// A tool result and where it ran.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub target: Target,
    pub value: Value,
}

pub struct Headless {
    pub doc: MapDocument,
    pub history: History,
    pub path: Option<PathBuf>,
}

impl Headless {
    pub fn new(doc: MapDocument, path: Option<PathBuf>) -> Self {
        let mut history = History::default();
        history.mark_saved();
        Self { doc, history, path }
    }
}

/// Live-link details reported by `editor_status`.
#[derive(Debug, Clone, Default)]
pub struct LiveConfig {
    pub token_file: Option<PathBuf>,
    /// Why the live link is unavailable (disabled, port in use...).
    pub unavailable: Option<String>,
    pub http_url: Option<String>,
}

pub struct Strata {
    headless: Mutex<Headless>,
    bridge: Option<Arc<Bridge>>,
    live: LiveConfig,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Strata {
    pub fn new(headless: Headless, bridge: Option<Arc<Bridge>>, live: LiveConfig) -> Self {
        Self {
            headless: Mutex::new(headless),
            bridge,
            live,
        }
    }

    /// Headless only, with a fresh map (tests, `--no-live`).
    pub fn headless_only(doc: MapDocument) -> Self {
        Self::new(
            Headless::new(doc, None),
            None,
            LiveConfig {
                unavailable: Some("disabled with --no-live".into()),
                ..Default::default()
            },
        )
    }

    pub fn bridge(&self) -> Option<&Arc<Bridge>> {
        self.bridge.as_ref()
    }

    pub fn target(&self) -> Target {
        if self.bridge.as_ref().is_some_and(|b| b.editor().is_some()) {
            Target::Editor
        } else {
            Target::Headless
        }
    }

    /// Run a request on the active target.
    pub fn run(&self, tool: &str, kind: LinkKind) -> Result<Outcome, String> {
        let kind = match &self.bridge {
            Some(b) => match b.request(tool, kind) {
                Ok(r) => {
                    return r.map(|value| Outcome {
                        target: Target::Editor,
                        value,
                    })
                }
                Err(kind) => kind,
            },
            None => kind,
        };
        let mut h = lock(&self.headless);
        let value = match kind {
            LinkKind::Load {
                doc,
                file_name: _,
                discard_changes,
            } => {
                if h.history.is_modified() && !discard_changes {
                    return Err(
                        "the headless map has unsaved changes; save it with map_save or pass discard_changes: true".into()
                    );
                }
                *h = Headless::new(*doc, None);
                json!({"loaded": h.doc.info.name})
            }
            LinkKind::Saved { .. } => json!({}),
            LinkKind::Status => json!({
                "map_name": h.doc.info.name,
                "path": h.path.as_ref().map(|p| p.display().to_string()),
                "modified": h.history.is_modified(),
            }),
            other => {
                let Headless { doc, history, .. } = &mut *h;
                link::execute(other, doc, history)?
            }
        };
        Ok(Outcome {
            target: Target::Headless,
            value,
        })
    }

    pub fn query(&self, tool: &str, q: Query) -> Result<Outcome, String> {
        self.run(tool, LinkKind::Query(q))
    }

    /// Open a map file into the active target.
    pub fn open(&self, path: &Path, discard_changes: bool) -> Result<Outcome, String> {
        let doc = read_map(path)?;
        let file_name = path.file_name().map(|f| f.to_string_lossy().into_owned());
        let summary = json!({
            "path": absolute(path).display().to_string(),
            "name": doc.info.name,
            "size": [doc.info.width, doc.info.height],
        });
        let out = self.run(
            "map_open",
            LinkKind::Load {
                doc: Box::new(doc),
                file_name,
                discard_changes,
            },
        )?;
        if out.target == Target::Headless {
            lock(&self.headless).path = Some(path.to_owned());
        }
        Ok(Outcome {
            target: out.target,
            value: summary,
        })
    }

    /// Replace the active target's map with a new one.
    pub fn new_map(&self, doc: MapDocument, discard_changes: bool) -> Result<Outcome, String> {
        let summary = json!({"name": doc.info.name, "size": [doc.info.width, doc.info.height]});
        let out = self.run(
            "map_new",
            LinkKind::Load {
                doc: Box::new(doc),
                file_name: None,
                discard_changes,
            },
        )?;
        Ok(Outcome {
            target: out.target,
            value: summary,
        })
    }

    /// Save the active target's map to `path` (or where it came from).
    pub fn save(&self, path: Option<PathBuf>) -> Result<Outcome, String> {
        let target = self.target();
        let (doc, known) = match target {
            Target::Editor => {
                let out = self.run("map_save", LinkKind::Query(Query::Document))?;
                if out.target == Target::Headless {
                    // The editor disconnected in between; save the headless map instead.
                    return self.save(path);
                }
                let doc: MapDocument = serde_json::from_value(out.value)
                    .map_err(|e| format!("the editor sent a map this server cannot read: {e}"))?;
                // The user can open another map in the editor at any time, so
                // a remembered path may belong to a different map: require one.
                if path.is_none() {
                    return Err("pass path when saving the editor's map, e.g. \"C:/Maps/MyMap.SC2Map\" (Strata won't guess which file the open map belongs to)".into());
                }
                (doc, None)
            }
            Target::Headless => {
                let h = lock(&self.headless);
                (h.doc.clone(), h.path.clone())
            }
        };
        let path = path.or(known).ok_or(
            "this map has no file yet; pass path, e.g. \"C:/Maps/MyMap.SC2Map\" or \"MyMap.strata.json\"".to_owned(),
        )?;
        write_map(&doc, &path)?;
        let shown = absolute(&path).display().to_string();
        match target {
            Target::Editor => {
                let _ = self.run(
                    "map_save",
                    LinkKind::Saved {
                        path: shown.clone(),
                    },
                );
            }
            Target::Headless => {
                let mut h = lock(&self.headless);
                h.history.mark_saved();
                h.path = Some(path.clone());
            }
        }
        Ok(Outcome {
            target,
            value: json!({"saved": shown}),
        })
    }

    /// Everything `editor_status` reports.
    pub fn status(&self) -> Value {
        let editor = self.bridge.as_ref().and_then(|b| b.editor());
        let target = if editor.is_some() {
            Target::Editor
        } else {
            Target::Headless
        };
        let live_state = editor
            .as_ref()
            .and_then(|_| self.run("editor_status", LinkKind::Status).ok())
            .map(|o| o.value);
        let h = lock(&self.headless);
        let note = match (&editor, &self.bridge) {
            (Some(e), _) if !e.allow_edits => {
                "An editor is connected but \"Allow AI edits\" is off: reads work, edits are refused until the user turns it on."
            }
            (Some(_), _) => "Tool calls edit the map open in the connected editor; changes are undoable there and labeled AI.",
            (None, Some(_)) => {
                "No editor is connected, so tool calls edit the headless map. To edit live, the user opens the AI link panel in Strata, pastes the token and clicks Connect."
            }
            (None, None) => "The live link is off, so tool calls edit the headless map.",
        };
        json!({
            "target": target.as_str(),
            "note": note,
            "server": {"name": crate::NAME, "version": crate::VERSION},
            "live_link": {
                "listening": self.bridge.as_ref().map(|b| b.url()),
                "unavailable": self.live.unavailable,
                "token_file": self.live.token_file.as_ref().map(|p| p.display().to_string()),
                "editor": editor,
                "editor_state": live_state,
            },
            "http": self.live.http_url,
            "base_data": crate::base::status_json(),
            "headless": {
                "map_name": h.doc.info.name,
                "path": h.path.as_ref().map(|p| p.display().to_string()),
                "modified": h.history.is_modified(),
            },
        })
    }
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_owned();
    }
    std::env::current_dir()
        .map(|d| d.join(path))
        .unwrap_or_else(|_| path.to_owned())
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// Read a `.strata.json` document, `.SC2Map` archive or unpacked map folder.
pub fn read_map(path: &Path) -> Result<MapDocument, String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    if is_json(path) {
        let bytes =
            std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        return serde_json::from_slice(&bytes)
            .map_err(|e| format!("{} is not a Strata document: {e}", path.display()));
    }
    strata_format::load_path(path).map_err(|e| format!("could not open {}: {e}", path.display()))
}

/// Write a document. `.strata.json` is always available; `.SC2Map` files
/// and existing folders go through `strata-format`.
pub fn write_map(doc: &MapDocument, path: &Path) -> Result<(), String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.ends_with(".strata.json") {
        let bytes = serde_json::to_vec(doc).map_err(|e| e.to_string())?;
        return std::fs::write(path, bytes)
            .map_err(|e| format!("could not write {}: {e}", path.display()));
    }
    if name.ends_with(".sc2map") || path.is_dir() {
        return strata_format::save_path(doc, path).map_err(|e| {
            format!(
                "could not save {}: {e}. Saving as MyMap.strata.json always works.",
                path.display()
            )
        });
    }
    Err(format!(
        "{} must end in .SC2Map or .strata.json (or be an existing map folder)",
        path.display()
    ))
}
