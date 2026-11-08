//! End to end: start the live-link bridge on an ephemeral port, connect a
//! fake editor over a real WebSocket, and check MCP tool calls are routed to
//! it (and fall back to the headless map when it leaves).

#![allow(clippy::result_large_err)]

use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use strata_core::history::History;
use strata_core::ops::link::{self, ClientMsg, LinkKind, LinkResponse, ServerMsg};
use strata_core::MapDocument;
use strata_mcp::bridge::Bridge;
use strata_mcp::server::{Headless, LiveConfig, Strata};
use strata_mcp::tools;
use tungstenite::client::IntoClientRequest;
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

const TOKEN: &str = "test-token-0123456789abcdef";

type Ws = WebSocket<MaybeTlsStream<TcpStream>>;

fn connect(url: &str, origin: Option<&str>) -> tungstenite::Result<Ws> {
    let mut req = url.into_client_request()?;
    if let Some(o) = origin {
        req.headers_mut().insert("Origin", o.parse().unwrap());
    }
    tungstenite::connect(req).map(|(ws, _)| ws)
}

fn hello(ws: &mut Ws, token: &str) -> ServerMsg {
    let h = ClientMsg::Hello {
        token: token.into(),
        protocol: link::PROTOCOL,
        client: "fake-editor".into(),
        version: "0.0.1".into(),
        allow_edits: true,
        map_name: "Live".into(),
    };
    ws.send(Message::text(serde_json::to_string(&h).unwrap()))
        .unwrap();
    loop {
        if let Message::Text(t) = ws.read().unwrap() {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

struct FakeEditor {
    doc: Arc<Mutex<MapDocument>>,
    allow_edits: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FakeEditor {
    /// Answers requests the way strata-app does: ops/queries/undo through
    /// `link::execute`, edits refused while allow_edits is off.
    fn start(mut ws: Ws) -> FakeEditor {
        let doc = Arc::new(Mutex::new(MapDocument::new("Live", 48, 48)));
        let allow_edits = Arc::new(AtomicBool::new(true));
        let stop = Arc::new(AtomicBool::new(false));
        if let MaybeTlsStream::Plain(s) = ws.get_ref() {
            s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        }
        let (d, allow, st) = (doc.clone(), allow_edits.clone(), stop.clone());
        let thread = thread::spawn(move || {
            let mut history = History::default();
            while !st.load(Ordering::Relaxed) {
                let text = match ws.read() {
                    Ok(Message::Text(t)) => t.to_string(),
                    Ok(_) => continue,
                    Err(tungstenite::Error::Io(e))
                        if matches!(
                            e.kind(),
                            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                        ) =>
                    {
                        continue
                    }
                    Err(_) => return,
                };
                let Ok(ServerMsg::Request(req)) = serde_json::from_str::<ServerMsg>(&text) else {
                    continue;
                };
                let result = if req.kind.is_edit() && !allow.load(Ordering::Relaxed) {
                    Err(link::EDITS_DISABLED.to_owned())
                } else {
                    let mut doc = d.lock().unwrap();
                    match req.kind {
                        LinkKind::Status => {
                            Ok(json!({"map_name": doc.info.name, "allow_edits": true}))
                        }
                        LinkKind::Load { doc: new, .. } => {
                            *doc = *new;
                            history = History::default();
                            Ok(json!({}))
                        }
                        LinkKind::Saved { .. } => Ok(json!({})),
                        other => link::execute(other, &mut doc, &mut history),
                    }
                };
                let resp = ClientMsg::Response(LinkResponse::from_result(req.id, result));
                ws.send(Message::text(serde_json::to_string(&resp).unwrap()))
                    .unwrap();
            }
            let _ = ws.close(None);
            let _ = ws.flush();
        });
        FakeEditor {
            doc,
            allow_edits,
            stop,
            thread: Some(thread),
        }
    }

    fn shutdown(mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.thread.take().unwrap().join().unwrap();
    }
}

fn wait_until(what: &str, f: impl Fn() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(10));
    }
}

fn call(s: &Strata, name: &str, args: Value) -> Result<Value, String> {
    match tools::call(s, name, &args) {
        Ok(text) => Ok(serde_json::from_str(&text).unwrap_or(Value::String(text))),
        Err(tools::CallError::Failed(e)) => Err(e),
        Err(e) => panic!("{e:?}"),
    }
}

#[test]
fn routed_tool_calls_reach_the_editor() {
    strata_mcp::set_quiet(true);
    let bridge = Bridge::start(
        "127.0.0.1:0",
        TOKEN.into(),
        vec!["https://strata.example".into()],
    )
    .unwrap();
    let url = bridge.url();
    let strata = Strata::new(
        Headless::new(MapDocument::new("Headless", 32, 32), None),
        Some(bridge.clone()),
        LiveConfig::default(),
    );

    // No editor yet: headless.
    let out = call(
        &strata,
        "place_object",
        json!({"type_id": "SCV", "x": 4, "y": 4}),
    )
    .unwrap();
    assert_eq!(out["target"], "headless");
    assert_eq!(
        call(&strata, "editor_status", json!({})).unwrap()["target"],
        "headless"
    );

    // Foreign browser origins cannot even open the socket.
    assert!(connect(&url, Some("https://evil.example")).is_err());
    // A wrong token is rejected with a reason.
    let mut ws = connect(&url, Some("http://localhost:8080")).unwrap();
    match hello(&mut ws, "wrong-token-wrong-token") {
        ServerMsg::Rejected { reason } => assert!(reason.contains("wrong token"), "{reason}"),
        other => panic!("expected a rejection, got {other:?}"),
    }
    assert!(bridge.editor().is_none());

    // A good editor (allow-listed deployed origin) is welcomed and gets the calls.
    let mut ws = connect(&url, Some("https://strata.example")).unwrap();
    assert!(matches!(
        hello(&mut ws, TOKEN),
        ServerMsg::Welcome {
            protocol: link::PROTOCOL,
            ..
        }
    ));
    let editor = FakeEditor::start(ws);
    wait_until("the editor to register", || bridge.editor().is_some());

    let status = call(&strata, "editor_status", json!({})).unwrap();
    assert_eq!(status["target"], "editor");
    assert_eq!(status["live_link"]["editor"]["client"], "fake-editor");
    assert_eq!(status["live_link"]["editor_state"]["map_name"], "Live");

    let out = call(
        &strata,
        "place_object",
        json!({"type_id": "Marine", "x": 10, "y": 11, "player": 2}),
    )
    .unwrap();
    assert_eq!(out["target"], "editor");
    assert_eq!(out["label"], "Place Marine");
    assert_eq!(
        editor.doc.lock().unwrap().objects.objects[0].type_id,
        "Marine"
    );

    let list = call(&strata, "list_objects", json!({})).unwrap();
    assert_eq!(
        (list["target"].as_str(), list["total"].as_u64()),
        (Some("editor"), Some(1))
    );
    assert_eq!(list["objects"][0]["player"], 2);

    let script = tools::call(&strata, "get_galaxy_script", &json!({})).unwrap();
    assert!(
        script.starts_with("// target: editor") && script.contains("// Name:   Live")
    );

    // Errors from the editor come back as tool errors.
    let e = call(&strata, "move_object", json!({"id": 999, "x": 1, "y": 1})).unwrap_err();
    assert!(e.contains("object 999"), "{e}");
    editor.allow_edits.store(false, Ordering::Relaxed);
    let e = call(
        &strata,
        "place_object",
        json!({"type_id": "Marine", "x": 1, "y": 1}),
    )
    .unwrap_err();
    assert!(e.contains("Allow AI edits"), "{e}");
    assert_eq!(
        call(&strata, "map_summary", json!({})).unwrap()["counts"]["units"],
        1
    );
    editor.allow_edits.store(true, Ordering::Relaxed);

    assert_eq!(
        call(&strata, "undo", json!({})).unwrap()["undid"],
        json!(["Place Marine"])
    );
    assert!(editor.doc.lock().unwrap().objects.objects.is_empty());
    call(&strata, "redo", json!({})).unwrap();

    // map_new replaces the editor's map; map_save pulls it back to disk.
    call(
        &strata,
        "map_new",
        json!({"name": "From AI", "width": 32, "height": 32, "discard_changes": true}),
    )
    .unwrap();
    assert_eq!(editor.doc.lock().unwrap().info.name, "From AI");
    let dir = std::env::temp_dir().join(format!("strata-live-{}", strata_mcp::token::generate()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("live.strata.json");
    let saved = call(&strata, "map_save", json!({"path": path.to_string_lossy()})).unwrap();
    assert_eq!(saved["target"], "editor");
    let back: MapDocument = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(back.info.name, "From AI");
    let _ = std::fs::remove_dir_all(dir);

    // When the editor leaves, calls go back to the headless map.
    editor.shutdown();
    wait_until("the editor to disconnect", || bridge.editor().is_none());
    let list = call(&strata, "list_objects", json!({})).unwrap();
    assert_eq!(list["target"], "headless");
    assert_eq!(list["objects"][0]["type_id"], "SCV");
}

#[test]
fn newer_editor_replaces_older() {
    strata_mcp::set_quiet(true);
    let bridge = Bridge::start("127.0.0.1:0", TOKEN.into(), vec![]).unwrap();
    let mut a = connect(&bridge.url(), None).unwrap();
    assert!(matches!(hello(&mut a, TOKEN), ServerMsg::Welcome { .. }));
    wait_until("first editor", || bridge.editor().is_some());
    let mut b = connect(&bridge.url(), None).unwrap();
    assert!(matches!(hello(&mut b, TOKEN), ServerMsg::Welcome { .. }));
    // The first connection is closed by the server.
    let closed = loop {
        match a.read() {
            Ok(Message::Close(_)) | Err(_) => break true,
            Ok(_) => {}
        }
    };
    assert!(closed);
    assert!(bridge.editor().is_some());
}
