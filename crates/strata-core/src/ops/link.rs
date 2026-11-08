//! Wire protocol of the live link between `strata-mcp` (WebSocket server on
//! `ws://127.0.0.1:7342`) and a running Strata editor (the client).
//!
//! Every message is one JSON text frame tagged by `"type"`:
//!
//! 1. editor -> server: [`ClientMsg::Hello`] with the session token the server
//!    printed on stderr and wrote to [`token_file_path`].
//! 2. server -> editor: [`ServerMsg::Welcome`], or [`ServerMsg::Rejected`]
//!    followed by a close.
//! 3. server -> editor: [`ServerMsg::Request`] for each routed MCP tool call;
//!    the editor answers with [`ClientMsg::Response`] carrying the same `id`.
//! 4. editor -> server: [`ClientMsg::Status`] whenever "Allow AI edits" or the
//!    open map changes.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::document::MapDocument;
use crate::history::{History, Origin};
use crate::ops::query::Query;
use crate::ops::Op;

/// Bumped on incompatible changes to the messages below.
pub const PROTOCOL: u32 = 1;
pub const DEFAULT_WS_URL: &str = "ws://127.0.0.1:7342";
pub const DEFAULT_WS_ADDR: &str = "127.0.0.1:7342";
pub const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:7341";
/// Largest live-link message either side accepts. Whole documents travel
/// over the link (map_open, map_save), so this is far above the usual 64 MiB.
pub const MAX_MESSAGE_BYTES: usize = 512 << 20;

/// Error an editor returns for edits while "Allow AI edits" is off.
pub const EDITS_DISABLED: &str =
    "The editor refused this change because \"Allow AI edits\" is off. \
Ask the user to turn it on in the AI link panel; reading the map still works.";

/// What the server asks the editor to do.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    /// Apply an op as one undoable edit labeled as coming from the AI.
    Op(Op),
    /// Answer a read-only query.
    Query(Query),
    Undo,
    Redo,
    /// Report editor state (map, file, modified, selection, allow_edits).
    Status,
    /// Replace the open map (map_new / map_open). Refused when the editor
    /// has unsaved changes unless `discard_changes` is set.
    Load {
        doc: Box<MapDocument>,
        #[serde(default)]
        file_name: Option<String>,
        #[serde(default)]
        discard_changes: bool,
    },
    /// The server just saved the editor's map to `path`.
    Saved {
        path: String,
    },
}

impl LinkKind {
    /// Short name for logs, e.g. "place_object", "query:summary", "undo".
    pub fn name(&self) -> String {
        match self {
            LinkKind::Op(op) => op.name(),
            LinkKind::Query(q) => format!("query:{}", q.name()),
            LinkKind::Undo => "undo".into(),
            LinkKind::Redo => "redo".into(),
            LinkKind::Status => "status".into(),
            LinkKind::Load { .. } => "load".into(),
            LinkKind::Saved { .. } => "saved".into(),
        }
    }

    /// Whether this changes the document (and so needs "Allow AI edits").
    pub fn is_edit(&self) -> bool {
        matches!(
            self,
            LinkKind::Op(_) | LinkKind::Undo | LinkKind::Redo | LinkKind::Load { .. }
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkRequest {
    pub id: u64,
    /// MCP tool that caused this request, for the editor's activity log.
    #[serde(default)]
    pub tool: Option<String>,
    pub kind: LinkKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LinkResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(default)]
    pub result: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl LinkResponse {
    pub fn ok(id: u64, result: Value) -> Self {
        Self {
            id,
            ok: true,
            result,
            error: None,
        }
    }
    pub fn err(id: u64, error: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            result: Value::Null,
            error: Some(error.into()),
        }
    }
    pub fn from_result(id: u64, r: Result<Value, String>) -> Self {
        match r {
            Ok(v) => Self::ok(id, v),
            Err(e) => Self::err(id, e),
        }
    }
    pub fn into_result(self) -> Result<Value, String> {
        if self.ok {
            Ok(self.result)
        } else {
            Err(self
                .error
                .unwrap_or_else(|| "the editor reported an unknown error".into()))
        }
    }
}

/// Messages sent by the editor.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        token: String,
        protocol: u32,
        /// e.g. "strata-web" or "strata-native".
        client: String,
        version: String,
        allow_edits: bool,
        #[serde(default)]
        map_name: String,
    },
    Status {
        allow_edits: bool,
        #[serde(default)]
        map_name: String,
    },
    Response(LinkResponse),
}

/// Messages sent by the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        server: String,
        version: String,
        protocol: u32,
    },
    Rejected {
        reason: String,
    },
    Request(LinkRequest),
}

/// Output shape for an applied op: `{"label": "...", "result": {...}}`.
pub fn op_output(label: &str, result: Value) -> Value {
    json!({"label": label, "result": result})
}

/// Run an op, query, undo or redo against a document and its history; the
/// headless server uses this directly and the editor mirrors it. `Status`,
/// `Load` and `Saved` depend on the host and are rejected here.
pub fn execute(
    kind: LinkKind,
    doc: &mut MapDocument,
    history: &mut History,
) -> Result<Value, String> {
    match kind {
        LinkKind::Op(op) => {
            let a = op.run(doc).map_err(|e| e.to_string())?;
            let out = op_output(&a.label, a.result);
            history.push(a.label, Origin::Mcp, a.edit);
            Ok(out)
        }
        LinkKind::Query(q) => q.run(doc, history).map_err(|e| e.to_string()),
        LinkKind::Undo => match history.undo(doc) {
            Some(l) => Ok(json!({"undid": l})),
            None => Err("nothing to undo".into()),
        },
        LinkKind::Redo => match history.redo(doc) {
            Some(l) => Ok(json!({"redid": l})),
            None => Err("nothing to redo".into()),
        },
        other => Err(format!("{} must be handled by the host", other.name())),
    }
}

/// Compare tokens without leaking where they differ through timing.
pub fn tokens_match(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim().as_bytes(), b.trim().as_bytes());
    if a.len() != b.len() || a.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Whether a browser `Origin` header may open the live link or call the
/// HTTP endpoint: absent (native clients), any `localhost` / `127.0.0.1` /
/// `[::1]` origin on any port, or an exact entry of `allowed`.
pub fn origin_allowed(origin: Option<&str>, allowed: &[String]) -> bool {
    let Some(origin) = origin.map(|o| o.trim().trim_end_matches('/')) else {
        return true;
    };
    if allowed
        .iter()
        .any(|a| a.trim().trim_end_matches('/').eq_ignore_ascii_case(origin))
    {
        return true;
    }
    let Some(rest) = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
    else {
        return false;
    };
    let host = if let Some(v6) = rest.strip_prefix('[') {
        match v6.split_once(']') {
            Some((h, tail)) if tail.is_empty() || tail.starts_with(':') => return h == "::1",
            _ => return false,
        }
    } else {
        rest.split(':').next().unwrap_or("")
    };
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1"
}

/// Where `strata-mcp` writes its session token so a native editor on the same
/// machine can connect without copying it. Per-user cache directory.
#[cfg(not(target_arch = "wasm32"))]
pub fn token_file_path() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    let env = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let base = if cfg!(windows) {
        env("LOCALAPPDATA").or_else(|| env("APPDATA"))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Caches"))
    } else {
        env("XDG_RUNTIME_DIR")
            .or_else(|| env("XDG_CACHE_HOME"))
            .or_else(|| env("HOME").map(|h| h.join(".cache")))
    };
    Some(
        base.unwrap_or_else(std::env::temp_dir)
            .join("strata-mcp")
            .join("token"),
    )
}

/// Read the token written by a running `strata-mcp`, if any.
#[cfg(not(target_arch = "wasm32"))]
pub fn read_token_file() -> Option<String> {
    let t = std::fs::read_to_string(token_file_path()?).ok()?;
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::ObjectKind;

    #[test]
    fn messages_roundtrip() {
        let req = ServerMsg::Request(LinkRequest {
            id: 7,
            tool: Some("place_object".into()),
            kind: LinkKind::Op(Op::PlaceObject {
                kind: ObjectKind::Unit,
                type_id: "Marine".into(),
                x: 1.0,
                y: 2.0,
                player: None,
                rotation: 0.0,
                name: String::new(),
            }),
        });
        let s = serde_json::to_string(&req).unwrap();
        let v: Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["type"], "request");
        assert_eq!(v["kind"]["op"]["op"], "place_object");
        let back: ServerMsg = serde_json::from_str(&s).unwrap();
        let ServerMsg::Request(r) = back else {
            panic!()
        };
        assert_eq!(
            (r.id, r.kind.name().as_str(), r.kind.is_edit()),
            (7, "place_object", true)
        );

        let q: ServerMsg = serde_json::from_value(
            json!({"type": "request", "id": 1, "kind": {"query": {"query": "summary"}}}),
        )
        .unwrap();
        let ServerMsg::Request(r) = q else { panic!() };
        assert_eq!(r.kind.name(), "query:summary");
        assert!(!r.kind.is_edit());
        let u: LinkKind = serde_json::from_value(json!("undo")).unwrap();
        assert!(matches!(u, LinkKind::Undo));

        let resp = ClientMsg::Response(LinkResponse::err(7, "nope"));
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v,
            json!({"type": "response", "id": 7, "ok": false, "result": null, "error": "nope"})
        );
        let ClientMsg::Response(r) = serde_json::from_value(v).unwrap() else {
            panic!()
        };
        assert_eq!(r.into_result(), Err("nope".to_owned()));

        let hello: ClientMsg = serde_json::from_value(json!({
            "type": "hello", "token": "abc", "protocol": 1, "client": "strata-web", "version": "0.1.0", "allow_edits": true
        }))
        .unwrap();
        assert!(matches!(
            hello,
            ClientMsg::Hello {
                allow_edits: true,
                ..
            }
        ));
    }

    #[test]
    fn execute_op_undo_redo() {
        let mut doc = MapDocument::new("t", 32, 32);
        let mut h = History::default();
        let op = Op::PlaceObject {
            kind: ObjectKind::Unit,
            type_id: "Marine".into(),
            x: 1.0,
            y: 2.0,
            player: None,
            rotation: 0.0,
            name: String::new(),
        };
        let out = execute(LinkKind::Op(op), &mut doc, &mut h).unwrap();
        assert_eq!(out["label"], "Place Marine");
        assert_eq!(h.undo_entries()[0].origin, Origin::Mcp);
        assert_eq!(
            execute(LinkKind::Undo, &mut doc, &mut h).unwrap()["undid"],
            "Place Marine"
        );
        assert!(doc.objects.objects.is_empty());
        assert!(execute(LinkKind::Undo, &mut doc, &mut h).is_err());
        execute(LinkKind::Redo, &mut doc, &mut h).unwrap();
        assert_eq!(doc.objects.objects.len(), 1);
        assert!(execute(LinkKind::Status, &mut doc, &mut h).is_err());
    }

    #[test]
    fn origins_and_tokens() {
        let allowed = vec!["https://strata.example".to_owned()];
        for ok in [
            None,
            Some("http://localhost:8080"),
            Some("http://127.0.0.1"),
            Some("https://[::1]:3000"),
            Some("https://strata.example/"),
        ] {
            assert!(origin_allowed(ok, &allowed), "{ok:?}");
        }
        for bad in [
            Some("https://evil.example"),
            Some("http://localhost.evil.example"),
            Some("null"),
            Some("http://127.0.0.1.nip.io"),
            Some("http://[::2]"),
        ] {
            assert!(!origin_allowed(bad, &allowed), "{bad:?}");
        }
        assert!(tokens_match("abc", " abc\n"));
        assert!(!tokens_match("abc", "abd"));
        assert!(!tokens_match("", ""));
    }
}
