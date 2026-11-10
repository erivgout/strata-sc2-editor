//! Live link to a local `strata-mcp` server: receives ops from AI clients and
//! applies them to the open document as undoable edits.
//!
//! The editor is the WebSocket client (`web_sys::WebSocket` on the web, a
//! `tungstenite` thread natively); the protocol lives in
//! [`strata_core::ops::link`]. [`tick`] runs once per frame: it connects or
//! reconnects with backoff, drains incoming requests, applies them through
//! `EditorState::run_op(op, Origin::Mcp)` (so they are undoable and labeled AI
//! in History) and sends the replies.

use std::collections::VecDeque;

use serde_json::{json, Value};
use strata_core::history::Origin;
use strata_core::ops::link::{self, ClientMsg, LinkKind, LinkRequest, LinkResponse, ServerMsg};

use crate::state::{EditorState, ToastKind};

const SETTINGS_ID: &str = "strata.mcp.settings.v1";
const MAX_LOG: usize = 200;
const MAX_LOG_JSON: usize = 6000;
const MAX_BACKOFF: f64 = 30.0;

/// Connection settings, persisted with egui memory (localStorage on web).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct McpSettings {
    pub url: String,
    pub token: String,
    pub auto_connect: bool,
    pub allow_edits: bool,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            url: link::DEFAULT_WS_URL.into(),
            token: String::new(),
            auto_connect: true,
            allow_edits: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LinkStatus {
    Disconnected,
    Connecting,
    Connected { server: String, version: String },
}

/// One handled request, shown in the panel's activity log.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub seq: u64,
    pub time: f64,
    /// MCP tool name, e.g. "place_object".
    pub tool: String,
    pub ok: bool,
    pub error: Option<String>,
    pub request: String,
    pub response: String,
}

/// Live-link state, stored in `state.ui.mcp`.
pub struct McpState {
    pub settings: McpSettings,
    pub status: LinkStatus,
    /// Why the last attempt failed or the link dropped.
    pub last_error: Option<String>,
    pub log: VecDeque<LogEntry>,
    /// `seq` of the log entry whose JSON is expanded.
    pub selected: Option<u64>,
    pub show_token: bool,
    /// Seconds (egui time) until the next automatic attempt, if one is scheduled.
    pub next_attempt: Option<f64>,
    conn: Option<transport::Connection>,
    loaded: bool,
    saved: Option<McpSettings>,
    backoff: f64,
    /// Set by a rejection or a manual disconnect: auto-connect waits for a
    /// settings change or a click on Connect.
    paused: bool,
    sent_status: Option<(bool, String)>,
    seq: u64,
}

impl Default for McpState {
    fn default() -> Self {
        Self {
            settings: McpSettings::default(),
            status: LinkStatus::Disconnected,
            last_error: None,
            log: VecDeque::new(),
            selected: None,
            show_token: false,
            next_attempt: Some(0.0),
            conn: None,
            loaded: false,
            saved: None,
            backoff: 1.0,
            paused: false,
            sent_status: None,
            seq: 0,
        }
    }
}

impl McpState {
    pub fn is_connected(&self) -> bool {
        matches!(self.status, LinkStatus::Connected { .. })
    }

    /// Seconds until the next automatic attempt, when one is scheduled.
    pub fn retry_in(&self, now: f64) -> Option<f64> {
        if self.conn.is_some() || self.paused || !self.settings.auto_connect {
            return None;
        }
        self.next_attempt
            .map(|t| (t - now).ceil())
            .filter(|s| *s >= 1.0)
    }

    /// The token to send: the field, or natively the file `strata-mcp` wrote.
    fn effective_token(&self) -> Option<String> {
        let t = self.settings.token.trim();
        if !t.is_empty() {
            return Some(t.to_owned());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            link::read_token_file()
        }
        #[cfg(target_arch = "wasm32")]
        {
            None
        }
    }

    fn push_log(&mut self, entry: LogEntry) {
        self.log.push_back(entry);
        while self.log.len() > MAX_LOG {
            self.log.pop_front();
        }
    }
}

/// Start connecting now (the Connect button).
pub fn connect(ctx: &egui::Context, m: &mut McpState) {
    m.paused = false;
    m.backoff = 1.0;
    open(ctx, m);
}

/// Drop the connection and stop reconnecting until Connect is clicked.
pub fn disconnect(m: &mut McpState) {
    m.conn = None;
    m.status = LinkStatus::Disconnected;
    m.paused = true;
    m.next_attempt = None;
    m.last_error = None;
}

fn open(ctx: &egui::Context, m: &mut McpState) {
    m.conn = None;
    let url = m.settings.url.trim().to_owned();
    if !url.starts_with("ws://") && !url.starts_with("wss://") {
        m.last_error = Some(format!(
            "The server address must start with ws://, e.g. {}",
            link::DEFAULT_WS_URL
        ));
        m.paused = true;
        return;
    }
    if m.effective_token().is_none() {
        m.last_error = Some("Paste the token strata-mcp printed when it started.".into());
        m.paused = true;
        return;
    }
    match transport::Connection::open(&url, ctx) {
        Ok(c) => {
            m.conn = Some(c);
            m.status = LinkStatus::Connecting;
            m.next_attempt = None;
        }
        Err(e) => {
            m.last_error = Some(e);
            m.paused = true;
        }
    }
}

fn load_settings(ctx: &egui::Context, m: &mut McpState) {
    m.loaded = true;
    if let Some(s) = ctx.data_mut(|d| d.get_persisted::<McpSettings>(egui::Id::new(SETTINGS_ID))) {
        m.settings = s;
    }
    if let Some(token) = transport::token_from_page_url() {
        m.settings.token = token;
        m.settings.auto_connect = true;
    }
    m.saved = Some(m.settings.clone());
}

/// Called once per frame by the app shell, before panels are drawn.
pub fn tick(ctx: &egui::Context, state: &mut EditorState) {
    let now = ctx.input(|i| i.time);
    let m = &mut state.ui.mcp;
    if !m.loaded {
        load_settings(ctx, m);
    }
    if m.saved.as_ref() != Some(&m.settings) {
        let changed_target = m
            .saved
            .as_ref()
            .is_none_or(|s| s.url != m.settings.url || s.token != m.settings.token);
        if changed_target
            || m.saved
                .as_ref()
                .is_some_and(|s| !s.auto_connect && m.settings.auto_connect)
        {
            // Wait a moment so typing a token does not fire an attempt per keystroke.
            m.paused = false;
            m.backoff = 1.0;
            m.next_attempt = Some(now + 1.0);
        }
        ctx.data_mut(|d| d.insert_persisted(egui::Id::new(SETTINGS_ID), m.settings.clone()));
        m.saved = Some(m.settings.clone());
    }

    if m.conn.is_none() && m.settings.auto_connect && !m.paused {
        match m.next_attempt {
            Some(t) if now >= t => {
                if m.effective_token().is_some() {
                    open(ctx, m);
                    // Keep auto-connect alive across failures of the attempt itself.
                    if m.conn.is_none() {
                        m.paused = false;
                        schedule_retry(m, now);
                    }
                } else {
                    m.next_attempt = Some(now + 2.0);
                }
            }
            Some(t) => {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64((t - now).max(0.05)))
            }
            None => m.next_attempt = Some(now),
        }
    }

    let Some(conn) = state.ui.mcp.conn.take() else {
        return;
    };
    let mut open_conn = true;
    for event in conn.poll() {
        match event {
            transport::Event::Open => {
                let m = &state.ui.mcp;
                let hello = ClientMsg::Hello {
                    token: m.effective_token().unwrap_or_default(),
                    protocol: link::PROTOCOL,
                    client: transport::CLIENT.into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    allow_edits: m.settings.allow_edits,
                    map_name: state.doc.info.name.clone(),
                };
                send(&conn, &hello);
            }
            transport::Event::Text(text) => match serde_json::from_str::<ServerMsg>(&text) {
                Ok(ServerMsg::Welcome {
                    server, version, ..
                }) => {
                    let m = &mut state.ui.mcp;
                    m.status = LinkStatus::Connected { server, version };
                    m.last_error = None;
                    m.backoff = 1.0;
                    m.sent_status = Some((m.settings.allow_edits, state.doc.info.name.clone()));
                }
                Ok(ServerMsg::Rejected { reason }) => {
                    let m = &mut state.ui.mcp;
                    m.last_error = Some(format!("strata-mcp refused the connection: {reason}"));
                    m.paused = true;
                }
                Ok(ServerMsg::Request(req)) => {
                    let reply = handle_request(state, req, now);
                    send(&conn, &ClientMsg::Response(reply));
                }
                Err(e) => log::warn!("AI link: ignored a message the editor cannot read: {e}"),
            },
            transport::Event::Refused { tool } => {
                let m = &mut state.ui.mcp;
                m.seq += 1;
                let entry = LogEntry {
                    seq: m.seq,
                    time: now,
                    tool,
                    ok: false,
                    error: Some("Refused while this tab was in the background.".into()),
                    request: String::new(),
                    response: String::new(),
                };
                m.push_log(entry);
            }
            transport::Event::Closed(reason) => {
                open_conn = false;
                let m = &mut state.ui.mcp;
                let was_connected = m.is_connected();
                m.status = LinkStatus::Disconnected;
                if m.last_error.is_none() || was_connected {
                    m.last_error = Some(match (was_connected, reason) {
                        (true, Some(r)) => format!("The connection closed: {r}"),
                        (true, None) => "The connection to strata-mcp closed.".into(),
                        (false, _) => format!(
                            "Could not reach strata-mcp at {}. It starts when your AI client starts it.",
                            m.settings.url.trim()
                        ),
                    });
                }
                if !m.paused {
                    schedule_retry(m, now);
                }
            }
        }
    }
    if !open_conn {
        return;
    }
    let m = &mut state.ui.mcp;
    if m.is_connected() {
        let status = (m.settings.allow_edits, state.doc.info.name.clone());
        if m.sent_status.as_ref() != Some(&status) {
            send(
                &conn,
                &ClientMsg::Status {
                    allow_edits: status.0,
                    map_name: status.1.clone(),
                },
            );
            m.sent_status = Some(status);
        }
    }
    m.conn = Some(conn);
    // Messages wake the UI themselves; this is a backstop.
    ctx.request_repaint_after(std::time::Duration::from_millis(500));
}

fn schedule_retry(m: &mut McpState, now: f64) {
    if m.settings.auto_connect {
        m.next_attempt = Some(now + m.backoff);
        m.backoff = (m.backoff * 2.0).min(MAX_BACKOFF);
    } else {
        m.next_attempt = None;
    }
}

fn send(conn: &transport::Connection, msg: &ClientMsg) {
    match serde_json::to_string(msg) {
        Ok(t) => conn.send(t),
        Err(e) => log::warn!("AI link: could not encode a reply: {e}"),
    }
}

fn clip(mut s: String) -> String {
    if s.len() > MAX_LOG_JSON {
        let mut cut = MAX_LOG_JSON;
        while !s.is_char_boundary(cut) {
            cut -= 1;
        }
        s.truncate(cut);
        s.push_str("\n… (truncated)");
    }
    s
}

fn describe_request(kind: &LinkKind) -> String {
    match kind {
        LinkKind::Load { doc, file_name, discard_changes } => serde_json::to_string_pretty(&json!({
            "load": {"name": doc.info.name, "size": [doc.info.width, doc.info.height], "file_name": file_name, "discard_changes": discard_changes}
        }))
        .unwrap_or_default(),
        other => clip(serde_json::to_string_pretty(other).unwrap_or_default()),
    }
}

/// Whether replacing the open map would lose unsaved work.
fn has_unsaved_changes(state: &EditorState) -> bool {
    state.history.is_modified()
}

fn status_json(state: &EditorState) -> Value {
    let sel = &state.selection;
    json!({
        "map_name": state.doc.info.name,
        "file_name": state.file_name,
        "unsaved_changes": has_unsaved_changes(state),
        "allow_edits": state.ui.mcp.settings.allow_edits,
        "mode": state.mode.label(),
        "selection": {
            "objects": sel.objects.iter().map(|id| id.0).collect::<Vec<_>>(),
            "region": sel.region.and_then(|id| state.doc.regions.iter().find(|r| r.id == id)).map(|r| r.name.clone()),
            "trigger": sel.trigger.and_then(|id| state.doc.triggers.triggers().into_iter().find(|t| t.id == id)).map(|t| t.name.clone()),
        },
        "history": {"undo": state.history.undo_entries().len(), "redo": state.history.redo_entries().len()},
        "client": transport::CLIENT,
    })
}

fn apply(state: &mut EditorState, kind: LinkKind) -> Result<Value, String> {
    if kind.is_edit() && !state.ui.mcp.settings.allow_edits {
        return Err(link::EDITS_DISABLED.into());
    }
    let out = match kind {
        LinkKind::Op(op) => {
            let result = state.run_op(op, Origin::Mcp).map_err(|e| e.to_string())?;
            let label = state
                .history
                .undo_entries()
                .last()
                .map(|e| e.label.clone())
                .unwrap_or_default();
            link::op_output(&label, result)
        }
        LinkKind::Query(q) => q
            .run_with(&state.doc, &state.history, state.base.as_deref())
            .map_err(|e| e.to_string())?,
        LinkKind::Undo => {
            let label = state
                .history
                .undo(&mut state.doc)
                .map(str::to_owned)
                .ok_or("nothing to undo")?;
            json!({"undid": label})
        }
        LinkKind::Redo => {
            let label = state
                .history
                .redo(&mut state.doc)
                .map(str::to_owned)
                .ok_or("nothing to redo")?;
            json!({"redid": label})
        }
        LinkKind::Status => status_json(state),
        LinkKind::Load {
            doc,
            file_name,
            discard_changes,
        } => {
            if has_unsaved_changes(state) && !discard_changes {
                return Err("The editor has unsaved changes. Ask the user to save first, or pass discard_changes: true to replace the map anyway.".into());
            }
            let name = doc.info.name.clone();
            state.load(*doc, file_name);
            state.toast(ToastKind::Info, format!("The AI opened {name}"));
            json!({"loaded": name})
        }
        LinkKind::Saved { path } => {
            state.history.mark_saved();
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned());
            if name.is_some() {
                state.file_name = name;
            }
            state.toast(
                ToastKind::Success,
                format!("The AI saved the map to {path}"),
            );
            json!({})
        }
    };
    state.prune_selection();
    Ok(out)
}

fn handle_request(state: &mut EditorState, req: LinkRequest, now: f64) -> LinkResponse {
    let tool = req.tool.clone().unwrap_or_else(|| req.kind.name());
    let request = describe_request(&req.kind);
    let result = apply(state, req.kind);
    let response = match &result {
        Ok(v) => clip(serde_json::to_string_pretty(v).unwrap_or_default()),
        Err(e) => e.clone(),
    };
    let m = &mut state.ui.mcp;
    m.seq += 1;
    let entry = LogEntry {
        seq: m.seq,
        time: now,
        tool,
        ok: result.is_ok(),
        error: result.as_ref().err().cloned(),
        request,
        response,
    };
    m.push_log(entry);
    LinkResponse::from_result(req.id, result)
}

#[cfg(not(target_arch = "wasm32"))]
mod transport {
    use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
    use std::time::Duration;

    use tungstenite::stream::MaybeTlsStream;
    use tungstenite::Message;

    pub const CLIENT: &str = "strata-native";

    pub enum Event {
        Open,
        Text(String),
        #[allow(dead_code)] // Only the web transport refuses requests on its own.
        Refused {
            tool: String,
        },
        Closed(Option<String>),
    }

    /// A WebSocket on a background thread; dropping it closes the socket.
    pub struct Connection {
        out: Sender<String>,
        events: Receiver<Event>,
    }

    impl Connection {
        pub fn open(url: &str, ctx: &egui::Context) -> Result<Connection, String> {
            if url.starts_with("wss://") {
                return Err(
                    "The native editor connects with ws:// only; strata-mcp runs on this computer."
                        .into(),
                );
            }
            let (out, out_rx) = mpsc::channel();
            let (ev_tx, events) = mpsc::channel();
            let (url, ctx) = (url.to_owned(), ctx.clone());
            std::thread::Builder::new()
                .name("strata-ai-link".into())
                .spawn(move || run(&url, &out_rx, &ev_tx, &ctx))
                .map_err(|e| format!("could not start the AI link thread: {e}"))?;
            Ok(Connection { out, events })
        }

        pub fn send(&self, text: String) {
            let _ = self.out.send(text);
        }

        pub fn poll(&self) -> Vec<Event> {
            let mut v: Vec<Event> = self.events.try_iter().collect();
            if v.is_empty() && matches!(self.events.try_recv(), Err(TryRecvError::Disconnected)) {
                v.push(Event::Closed(None));
            }
            v
        }
    }

    fn run(url: &str, out: &Receiver<String>, ev: &Sender<Event>, ctx: &egui::Context) {
        let notify = |e: Event| {
            let _ = ev.send(e);
            ctx.request_repaint();
        };
        let max = strata_core::ops::link::MAX_MESSAGE_BYTES;
        let config = tungstenite::protocol::WebSocketConfig::default()
            .max_message_size(Some(max))
            .max_frame_size(Some(max));
        let mut ws = match tungstenite::client::connect_with_config(url, Some(config), 3) {
            Ok((ws, _)) => ws,
            Err(e) => return notify(Event::Closed(Some(e.to_string()))),
        };
        if let MaybeTlsStream::Plain(s) = ws.get_ref() {
            let _ = s.set_read_timeout(Some(Duration::from_millis(20)));
        }
        notify(Event::Open);
        loop {
            loop {
                match out.try_recv() {
                    Ok(text) => {
                        if let Err(e) = ws.send(Message::text(text)) {
                            return notify(Event::Closed(Some(e.to_string())));
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let _ = ws.close(None);
                        let _ = ws.flush();
                        return;
                    }
                }
            }
            match ws.read() {
                Ok(Message::Text(t)) => notify(Event::Text(t.to_string())),
                Ok(Message::Close(frame)) => {
                    let reason = frame
                        .map(|f| f.reason.to_string())
                        .filter(|r| !r.is_empty());
                    return notify(Event::Closed(reason));
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if is_timeout(&e) => {}
                Err(e) => return notify(Event::Closed(Some(e.to_string()))),
            }
        }
    }

    fn is_timeout(e: &std::io::Error) -> bool {
        matches!(
            e.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        )
    }

    pub fn token_from_page_url() -> Option<String> {
        None
    }
}

#[cfg(target_arch = "wasm32")]
mod transport {
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::rc::Rc;

    use strata_core::ops::link::{ClientMsg, LinkResponse, ServerMsg};
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};

    pub const CLIENT: &str = "strata-web";

    pub enum Event {
        Open,
        Text(String),
        /// A request answered with an error straight from the socket
        /// callback because the tab was hidden (browsers pause hidden tabs).
        Refused {
            tool: String,
        },
        Closed(Option<String>),
    }

    type Handler = Closure<dyn FnMut(JsValue)>;

    pub struct Connection {
        ws: web_sys::WebSocket,
        queue: Rc<RefCell<VecDeque<Event>>>,
        _handlers: [Handler; 4],
    }

    fn page_hidden() -> bool {
        web_sys::window()
            .and_then(|w| w.document())
            .is_some_and(|d| d.hidden())
    }

    /// Answer a request immediately when the tab is hidden, since frames
    /// (and so `tick`) stop until the user comes back.
    fn refuse_if_hidden(ws: &web_sys::WebSocket, text: &str) -> Option<Event> {
        if !page_hidden() {
            return None;
        }
        let Ok(ServerMsg::Request(req)) = serde_json::from_str::<ServerMsg>(text) else {
            return None;
        };
        let reply = ClientMsg::Response(LinkResponse::err(
            req.id,
            "The Strata browser tab is in the background, and browsers pause background tabs. \
             Ask the user to switch to the Strata tab, then retry.",
        ));
        let _ = ws.send_with_str(&serde_json::to_string(&reply).unwrap_or_default());
        Some(Event::Refused {
            tool: req.tool.unwrap_or_else(|| req.kind.name()),
        })
    }

    impl Connection {
        pub fn open(url: &str, ctx: &egui::Context) -> Result<Connection, String> {
            let ws = web_sys::WebSocket::new(url).map_err(|e| {
                format!(
                    "The browser refused to open {url}: {}",
                    e.as_string().unwrap_or_else(|| format!("{e:?}"))
                )
            })?;
            let queue: Rc<RefCell<VecDeque<Event>>> = Rc::default();
            let push = {
                let (q, c) = (queue.clone(), ctx.clone());
                move |e: Event| {
                    q.borrow_mut().push_back(e);
                    c.request_repaint();
                }
            };
            let on_open: Handler = {
                let push = push.clone();
                Closure::wrap(Box::new(move |_: JsValue| push(Event::Open)))
            };
            let on_message: Handler = {
                let (push, ws2) = (push.clone(), ws.clone());
                Closure::wrap(Box::new(move |e: JsValue| {
                    let Some(text) = e
                        .dyn_ref::<web_sys::MessageEvent>()
                        .and_then(|m| m.data().as_string())
                    else {
                        return;
                    };
                    match refuse_if_hidden(&ws2, &text) {
                        Some(refused) => push(refused),
                        None => push(Event::Text(text)),
                    }
                }))
            };
            let on_close: Handler = {
                let push = push.clone();
                Closure::wrap(Box::new(move |e: JsValue| {
                    let reason = js_sys::Reflect::get(&e, &JsValue::from_str("reason"))
                        .ok()
                        .and_then(|r| r.as_string())
                        .filter(|r| !r.is_empty());
                    push(Event::Closed(reason));
                }))
            };
            // Errors are always followed by a close event, which reports them.
            let on_error: Handler = Closure::wrap(Box::new(move |_: JsValue| {}));
            ws.set_onopen(Some(on_open.as_ref().unchecked_ref()));
            ws.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
            ws.set_onclose(Some(on_close.as_ref().unchecked_ref()));
            ws.set_onerror(Some(on_error.as_ref().unchecked_ref()));
            Ok(Connection {
                ws,
                queue,
                _handlers: [on_open, on_message, on_close, on_error],
            })
        }

        pub fn send(&self, text: String) {
            let _ = self.ws.send_with_str(&text);
        }

        pub fn poll(&self) -> Vec<Event> {
            self.queue.borrow_mut().drain(..).collect()
        }
    }

    impl Drop for Connection {
        fn drop(&mut self) {
            // Detach first so the browser never calls a dropped closure.
            self.ws.set_onopen(None);
            self.ws.set_onmessage(None);
            self.ws.set_onclose(None);
            self.ws.set_onerror(None);
            let _ = self.ws.close();
        }
    }

    /// `?mcp=TOKEN` in the page URL fills in the token (and connects).
    pub fn token_from_page_url() -> Option<String> {
        let window = web_sys::window()?;
        let location = js_sys::Reflect::get(&window, &JsValue::from_str("location")).ok()?;
        let search = js_sys::Reflect::get(&location, &JsValue::from_str("search"))
            .ok()?
            .as_string()?;
        search
            .trim_start_matches('?')
            .split('&')
            .find_map(|kv| kv.strip_prefix("mcp="))
            .map(|t| {
                js_sys::decode_uri_component(t)
                    .ok()
                    .and_then(|s| s.as_string())
                    .unwrap_or_else(|| t.to_owned())
            })
            .filter(|t| !t.trim().is_empty())
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};
    use strata_core::objects::ObjectKind;
    use strata_core::ops::query::Query;
    use strata_core::ops::Op;
    use strata_core::MapDocument;
    use tungstenite::Message;

    fn place(x: f32) -> LinkKind {
        LinkKind::Op(Op::PlaceObject {
            kind: ObjectKind::Unit,
            type_id: "Marine".into(),
            x,
            y: 4.0,
            player: None,
            rotation: 0.0,
            name: String::new(),
        })
    }

    fn request(state: &mut EditorState, id: u64, kind: LinkKind) -> LinkResponse {
        handle_request(
            state,
            LinkRequest {
                id,
                tool: Some("test".into()),
                kind,
            },
            0.0,
        )
    }

    #[test]
    fn requests_apply_as_ai_edits() {
        let mut state = EditorState::new(MapDocument::new("Live", 32, 32));
        let r = request(&mut state, 1, place(4.0));
        assert!(r.ok, "{r:?}");
        assert_eq!(r.result["label"], "Place Marine");
        assert_eq!(state.history.undo_entries()[0].origin, Origin::Mcp);
        assert_eq!(state.ui.mcp.log.len(), 1);

        let r = request(&mut state, 2, LinkKind::Query(Query::Summary));
        assert_eq!(r.result["counts"]["units"], 1);
        let r = request(&mut state, 3, LinkKind::Status);
        assert_eq!(r.result["map_name"], "Live");

        // With AI edits off, edits are refused but reads still work.
        state.ui.mcp.settings.allow_edits = false;
        let r = request(&mut state, 4, place(6.0));
        assert_eq!(r.error.as_deref(), Some(link::EDITS_DISABLED));
        assert!(!request(&mut state, 5, LinkKind::Undo).ok);
        assert!(request(&mut state, 6, LinkKind::Query(Query::Validate)).ok);
        state.ui.mcp.settings.allow_edits = true;

        // Unsaved work blocks replacing the map unless the AI insists.
        let load = |discard| LinkKind::Load {
            doc: Box::new(MapDocument::new("New", 16, 16)),
            file_name: None,
            discard_changes: discard,
        };
        assert!(request(&mut state, 7, load(false))
            .error
            .unwrap()
            .contains("unsaved"));
        assert!(request(&mut state, 8, load(true)).ok);
        assert_eq!(state.doc.info.name, "New");
        assert_eq!(state.ui.mcp.log.len(), 8, "the log survives a map load");
        assert!(
            request(&mut state, 9, load(false)).ok,
            "a fresh map has nothing to lose"
        );

        assert!(
            request(
                &mut state,
                10,
                LinkKind::Saved {
                    path: "C:/maps/x.SC2Map".into()
                }
            )
            .ok
        );
        assert_eq!(state.file_name.as_deref(), Some("x.SC2Map"));
    }

    #[test]
    fn tick_connects_and_answers_over_websocket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut ws = tungstenite::accept(stream).unwrap();
            let read = |ws: &mut tungstenite::WebSocket<std::net::TcpStream>| loop {
                if let Message::Text(t) = ws.read().unwrap() {
                    return t.to_string();
                }
            };
            let hello: ClientMsg = serde_json::from_str(&read(&mut ws)).unwrap();
            assert!(
                matches!(hello, ClientMsg::Hello { ref token, .. } if token == "secret-token-123456")
            );
            let welcome = ServerMsg::Welcome {
                server: "strata-mcp".into(),
                version: "9.9.9".into(),
                protocol: link::PROTOCOL,
            };
            ws.send(Message::text(serde_json::to_string(&welcome).unwrap()))
                .unwrap();
            let req = ServerMsg::Request(LinkRequest {
                id: 42,
                tool: Some("place_object".into()),
                kind: place(8.0),
            });
            ws.send(Message::text(serde_json::to_string(&req).unwrap()))
                .unwrap();
            loop {
                if let ClientMsg::Response(r) = serde_json::from_str(&read(&mut ws)).unwrap() {
                    return r;
                }
            }
        });

        let ctx = egui::Context::default();
        let mut state = EditorState::new(MapDocument::new("Live", 32, 32));
        state.ui.mcp.settings.url = url;
        state.ui.mcp.settings.token = "secret-token-123456".into();
        let end = Instant::now() + Duration::from_secs(5);
        while !server.is_finished() {
            assert!(
                Instant::now() < end,
                "timed out; status {:?}, error {:?}",
                state.ui.mcp.status,
                state.ui.mcp.last_error
            );
            tick(&ctx, &mut state);
            std::thread::sleep(Duration::from_millis(5));
        }
        let resp = server.join().unwrap();
        assert_eq!((resp.id, resp.ok), (42, true));
        assert!(state.ui.mcp.is_connected());
        assert_eq!(state.doc.objects.objects.len(), 1);
        assert_eq!(
            state.ui.mcp.log.back().map(|e| e.tool.as_str()),
            Some("place_object")
        );
    }
}
