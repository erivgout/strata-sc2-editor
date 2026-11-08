//! JSON-RPC 2.0 / MCP message handling, independent of the transport.

use serde_json::{json, Value};

use crate::server::Strata;
use crate::tools::{self, CallError};

/// Newest MCP revision first; the server answers with the client's version
/// when it is listed here and with [`LATEST`] otherwise.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
pub const LATEST: &str = SUPPORTED_VERSIONS[0];

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;

const INSTRUCTIONS: &str = "Strata edits StarCraft II maps. Call editor_status first: tool calls go to the Strata editor \
the user has open (live, undoable, labeled AI in its History) when it is connected, otherwise to a headless map in this \
server that you open with map_open / map_new and write with map_save. Coordinates are world units equal to terrain cells, \
(0,0) is the south-west corner, x grows east and y north; new maps have flat ground at height 12 (cliff level 2). \
Inspect before editing (map_summary, list_objects, terrain_stats, list_triggers, get_trigger) and check your work with \
validate_map and get_galaxy_script. Every edit is one undo step; use undo if a result is not what you wanted.";

fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// Handle one transport message (a request, notification, response or
/// batch). Returns the reply to send, if any.
pub fn handle(strata: &Strata, msg: Value) -> Option<Value> {
    match msg {
        Value::Array(batch) => {
            if batch.is_empty() {
                return Some(error(Value::Null, INVALID_REQUEST, "empty batch"));
            }
            let replies: Vec<Value> = batch
                .into_iter()
                .filter_map(|m| handle_one(strata, m))
                .collect();
            (!replies.is_empty()).then_some(Value::Array(replies))
        }
        m => handle_one(strata, m),
    }
}

/// Handle raw text (one line of the stdio transport, or an HTTP body).
pub fn handle_text(strata: &Strata, text: &str) -> Option<String> {
    let reply = match serde_json::from_str::<Value>(text) {
        Ok(v) => handle(strata, v),
        Err(e) => Some(error(
            Value::Null,
            PARSE_ERROR,
            format!("invalid JSON: {e}"),
        )),
    };
    reply.map(|r| r.to_string())
}

fn handle_one(strata: &Strata, msg: Value) -> Option<Value> {
    let Value::Object(m) = msg else {
        return Some(error(
            Value::Null,
            INVALID_REQUEST,
            "a message must be a JSON object",
        ));
    };
    let id = m.get("id").cloned();
    let Some(method) = m.get("method").and_then(Value::as_str) else {
        // A response to something we sent (we send nothing), or junk.
        if m.contains_key("result") || m.contains_key("error") {
            return None;
        }
        return Some(error(
            id.unwrap_or(Value::Null),
            INVALID_REQUEST,
            "missing \"method\"",
        ));
    };
    let params = m.get("params").cloned().unwrap_or(Value::Null);
    let Some(id) = id else {
        // Notification: never answered.
        match method {
            "notifications/initialized"
            | "notifications/cancelled"
            | "notifications/roots/list_changed" => {}
            other => crate::log!("ignored notification {other}"),
        }
        return None;
    };
    if !(id.is_string() || id.is_number()) {
        return Some(error(
            Value::Null,
            INVALID_REQUEST,
            "id must be a string or number",
        ));
    }
    Some(match method {
        "initialize" => {
            let asked = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(LATEST);
            let version = if SUPPORTED_VERSIONS.contains(&asked) {
                asked
            } else {
                LATEST
            };
            if let Some(c) = params.get("clientInfo") {
                crate::log!(
                    "client {} {} connected (protocol {version})",
                    c.get("name").and_then(Value::as_str).unwrap_or("?"),
                    c.get("version").and_then(Value::as_str).unwrap_or("")
                );
            }
            result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}, "logging": {}},
                    "serverInfo": {"name": crate::NAME, "title": "Strata map editor", "version": crate::VERSION},
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => result(id, json!({})),
        "logging/setLevel" => result(id, json!({})),
        "tools/list" => result(id, json!({"tools": tools::list_json()})),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(error(id, INVALID_PARAMS, "tools/call needs params.name"));
            };
            let args = params.get("arguments").cloned().unwrap_or(Value::Null);
            match tools::call(strata, name, &args) {
                Ok(text) => result(
                    id,
                    json!({"content": [{"type": "text", "text": text}], "isError": false}),
                ),
                Err(CallError::Failed(e)) => {
                    crate::log!("{name} failed: {e}");
                    result(
                        id,
                        json!({"content": [{"type": "text", "text": e}], "isError": true}),
                    )
                }
                Err(CallError::UnknownTool(n)) => error(
                    id,
                    INVALID_PARAMS,
                    format!("unknown tool \"{n}\"; see tools/list"),
                ),
            }
        }
        "resources/list" => result(id, json!({"resources": []})),
        "resources/templates/list" => result(id, json!({"resourceTemplates": []})),
        "prompts/list" => result(id, json!({"prompts": []})),
        other => error(
            id,
            METHOD_NOT_FOUND,
            format!("method \"{other}\" is not supported"),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::MapDocument;

    fn strata() -> Strata {
        crate::set_quiet(true);
        Strata::headless_only(MapDocument::new("t", 64, 64))
    }

    fn call(s: &Strata, id: i64, method: &str, params: Value) -> Value {
        handle(
            s,
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        )
        .expect("a reply")
    }

    #[test]
    fn initialize_and_list() {
        let s = strata();
        let r = call(
            &s,
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}),
        );
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(r["result"]["serverInfo"]["name"], "strata-mcp");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        let r = call(
            &s,
            2,
            "initialize",
            json!({"protocolVersion": "2025-03-26"}),
        );
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        let r = call(
            &s,
            3,
            "initialize",
            json!({"protocolVersion": "1999-01-01"}),
        );
        assert_eq!(r["result"]["protocolVersion"], LATEST);

        assert!(handle(
            &s,
            json!({"jsonrpc": "2.0", "method": "notifications/initialized"})
        )
        .is_none());
        assert_eq!(call(&s, 4, "ping", json!({}))["result"], json!({}));

        let tools = call(&s, 5, "tools/list", json!({}));
        let list = tools["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = list.iter().map(|t| t["name"].as_str().unwrap()).collect();
        for n in [
            "place_object",
            "list_objects",
            "terrain_brush",
            "create_trigger",
            "editor_status",
            "undo",
            "map_save",
        ] {
            assert!(names.contains(&n), "{n} missing");
        }
        assert!(list.iter().all(|t| t["inputSchema"]["type"] == "object"
            && t["description"].as_str().is_some_and(|d| d.len() > 20)));
    }

    #[test]
    fn place_then_list() {
        let s = strata();
        let r = call(
            &s,
            1,
            "tools/call",
            json!({"name": "place_object", "arguments": {"type_id": "Marine", "x": 12, "y": 14}}),
        );
        assert_eq!(r["result"]["isError"], false);
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        let v: Value = serde_json::from_str(text).unwrap();
        assert_eq!(v["label"], "Place Marine");
        let r = call(
            &s,
            2,
            "tools/call",
            json!({"name": "list_objects", "arguments": {"type_id": "Marine"}}),
        );
        let v: Value =
            serde_json::from_str(r["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(v["total"], 1);
        assert_eq!(v["objects"][0]["x"], 12.0);
    }

    #[test]
    fn error_paths() {
        let s = strata();
        // Tool failure: reported in the result so the model can read it.
        let r = call(
            &s,
            1,
            "tools/call",
            json!({"name": "place_object", "arguments": {"type_id": "Marine", "x": 500, "y": 1}}),
        );
        assert_eq!(r["result"]["isError"], true);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("outside the 64x64 map"));
        let r = call(
            &s,
            2,
            "tools/call",
            json!({"name": "move_object", "arguments": {"id": 99, "x": 1, "y": 1}}),
        );
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("object 99"));
        // Protocol errors.
        assert_eq!(
            call(&s, 3, "tools/call", json!({"name": "nope"}))["error"]["code"],
            INVALID_PARAMS
        );
        assert_eq!(
            call(&s, 4, "frobnicate", json!({}))["error"]["code"],
            METHOD_NOT_FOUND
        );
        let r: Value = serde_json::from_str(&handle_text(&s, "{not json").unwrap()).unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
        assert_eq!(
            handle(&s, json!({"id": 5})).unwrap()["error"]["code"],
            INVALID_REQUEST
        );
        assert!(handle(&s, json!({"jsonrpc": "2.0", "id": 9, "result": {}})).is_none());
        let batch = handle(
            &s,
            json!([
                {"jsonrpc": "2.0", "id": 1, "method": "ping"},
                {"jsonrpc": "2.0", "method": "notifications/initialized"}
            ]),
        )
        .unwrap();
        assert_eq!(batch.as_array().unwrap().len(), 1);
    }
}
