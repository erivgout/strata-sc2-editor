//! The live link: a localhost WebSocket server the running editor connects
//! to. At most one editor is active; a newer connection replaces an older one.
//!
//! Each connection runs on its own thread, alternating between flushing
//! queued requests and a short-timeout read, so one socket serves both
//! directions without splitting it.

// tungstenite's error and handshake-response types are large, and
// `Bridge::request` hands the (large) request back when no editor is connected.
#![allow(clippy::result_large_err)]

use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use strata_core::ops::link::{self, ClientMsg, LinkKind, LinkRequest, LinkResponse, ServerMsg};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::protocol::frame::coding::CloseCode;
use tungstenite::protocol::CloseFrame;
use tungstenite::{Message, WebSocket};

use crate::log;

const POLL: Duration = Duration::from_millis(15);
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a routed tool call waits for the editor.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

/// What the server knows about the connected editor.
#[derive(Debug, Clone, Serialize)]
pub struct EditorInfo {
    pub client: String,
    pub version: String,
    pub allow_edits: bool,
    pub map_name: String,
    pub connected_seconds: u64,
    #[serde(skip)]
    since: Instant,
}

type Pending = Arc<Mutex<HashMap<u64, Sender<LinkResponse>>>>;

struct EditorConn {
    conn_id: u64,
    tx: Sender<String>,
    pending: Pending,
    info: EditorInfo,
}

pub struct Bridge {
    token: String,
    allowed_origins: Vec<String>,
    addr: SocketAddr,
    editor: Mutex<Option<EditorConn>>,
    next_request: AtomicU64,
    next_conn: AtomicU64,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Bridge {
    /// Bind `addr` (must be a loopback address) and start accepting editors.
    pub fn start(
        addr: &str,
        token: String,
        allowed_origins: Vec<String>,
    ) -> std::io::Result<Arc<Bridge>> {
        let listener = TcpListener::bind(addr)?;
        let local = listener.local_addr()?;
        if !local.ip().is_loopback() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!("refusing to listen on {local}: the live link only binds loopback addresses such as 127.0.0.1"),
            ));
        }
        let bridge = Arc::new(Bridge {
            token,
            allowed_origins,
            addr: local,
            editor: Mutex::new(None),
            next_request: AtomicU64::new(1),
            next_conn: AtomicU64::new(1),
        });
        let b = bridge.clone();
        thread::Builder::new()
            .name("live-link-accept".into())
            .spawn(move || {
                for stream in listener.incoming().flatten() {
                    let b = b.clone();
                    let _ = thread::Builder::new()
                        .name("live-link-conn".into())
                        .spawn(move || b.serve(stream));
                }
            })?;
        Ok(bridge)
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// The connected editor, if any.
    pub fn editor(&self) -> Option<EditorInfo> {
        lock(&self.editor).as_ref().map(|c| {
            let mut i = c.info.clone();
            i.connected_seconds = i.since.elapsed().as_secs();
            i
        })
    }

    /// Route a request to the editor. Returns `Err(kind)` (handing the
    /// request back) when no editor is connected.
    pub fn request(&self, tool: &str, kind: LinkKind) -> Result<Result<Value, String>, LinkKind> {
        let id = self.next_request.fetch_add(1, Ordering::Relaxed);
        let (resp_tx, resp_rx) = mpsc::channel();
        let pending = {
            let guard = lock(&self.editor);
            let Some(conn) = guard.as_ref() else {
                return Err(kind);
            };
            let msg = ServerMsg::Request(LinkRequest {
                id,
                tool: Some(tool.to_owned()),
                kind,
            });
            let text = match serde_json::to_string(&msg) {
                Ok(t) => t,
                Err(e) => return Ok(Err(format!("could not encode the request: {e}"))),
            };
            lock(&conn.pending).insert(id, resp_tx);
            if conn.tx.send(text).is_err() {
                lock(&conn.pending).remove(&id);
                return Ok(Err(
                    "the editor disconnected before the request was sent".into()
                ));
            }
            conn.pending.clone()
        };
        Ok(match resp_rx.recv_timeout(REQUEST_TIMEOUT) {
            Ok(r) => r.into_result(),
            Err(RecvTimeoutError::Timeout) => {
                lock(&pending).remove(&id);
                Err(format!(
                    "The editor did not answer within {} s. Browsers pause background tabs and some systems pause \
                     minimized windows, so ask the user to bring the Strata window to the front, then retry.",
                    REQUEST_TIMEOUT.as_secs()
                ))
            }
            Err(RecvTimeoutError::Disconnected) => {
                Err("the editor disconnected before answering".into())
            }
        })
    }

    fn serve(self: Arc<Self>, stream: TcpStream) {
        let peer = stream.peer_addr().ok();
        if !peer.is_some_and(|p| p.ip().is_loopback()) {
            log!("live link: refused non-loopback peer {peer:?}");
            return;
        }
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let origins = self.allowed_origins.clone();
        let check = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
            let origin = req.headers().get("origin").and_then(|v| v.to_str().ok());
            if link::origin_allowed(origin, &origins) {
                Ok(resp)
            } else {
                log!("live link: refused origin {origin:?} (allow it with --allow-origin)");
                let mut r = ErrorResponse::new(Some(format!(
                    "origin {} may not connect; start strata-mcp with --allow-origin to allow it",
                    origin.unwrap_or("")
                )));
                *r.status_mut() = tungstenite::http::StatusCode::FORBIDDEN;
                Err(r)
            }
        };
        let mut ws = match tungstenite::accept_hdr(stream, check) {
            Ok(ws) => ws,
            Err(e) => {
                log!("live link: handshake failed: {e}");
                return;
            }
        };
        let _ = ws.get_ref().set_read_timeout(Some(POLL));

        let Some((client, version, allow_edits, map_name)) = self.await_hello(&mut ws) else {
            return;
        };
        let welcome = ServerMsg::Welcome {
            server: crate::NAME.into(),
            version: crate::VERSION.into(),
            protocol: link::PROTOCOL,
        };
        if send(&mut ws, &welcome).is_err() {
            return;
        }

        let conn_id = self.next_conn.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel::<String>();
        let pending: Pending = Arc::default();
        let info = EditorInfo {
            client,
            version,
            allow_edits,
            map_name,
            connected_seconds: 0,
            since: Instant::now(),
        };
        log!(
            "live link: editor connected ({} {}), routing tool calls to it",
            info.client,
            info.version
        );
        if lock(&self.editor)
            .replace(EditorConn {
                conn_id,
                tx,
                pending: pending.clone(),
                info,
            })
            .is_some()
        {
            log!("live link: replaced the previously connected editor");
        }

        self.pump(&mut ws, conn_id, &rx, &pending);

        {
            let mut guard = lock(&self.editor);
            if guard.as_ref().is_some_and(|c| c.conn_id == conn_id) {
                *guard = None;
                log!("live link: editor disconnected; tool calls now edit the headless map");
            }
        }
        for (id, s) in lock(&pending).drain() {
            let _ = s.send(LinkResponse::err(
                id,
                "the editor disconnected before answering",
            ));
        }
    }

    fn await_hello(&self, ws: &mut WebSocket<TcpStream>) -> Option<(String, String, bool, String)> {
        let deadline = Instant::now() + HELLO_TIMEOUT;
        loop {
            match ws.read() {
                Ok(Message::Text(t)) => {
                    let reason = match serde_json::from_str::<ClientMsg>(t.as_str()) {
                        Ok(ClientMsg::Hello {
                            token,
                            protocol,
                            client,
                            version,
                            allow_edits,
                            map_name,
                        }) => {
                            if protocol != link::PROTOCOL {
                                format!("this strata-mcp speaks live-link protocol {}, the editor speaks {protocol}; update the older one", link::PROTOCOL)
                            } else if !link::tokens_match(&token, &self.token) {
                                "wrong token: copy the token strata-mcp printed when it started"
                                    .to_owned()
                            } else {
                                return Some((client, version, allow_edits, map_name));
                            }
                        }
                        _ => "the first message must be a hello".to_owned(),
                    };
                    log!("live link: rejected an editor: {reason}");
                    let _ = send(
                        ws,
                        &ServerMsg::Rejected {
                            reason: reason.clone(),
                        },
                    );
                    let _ = ws.close(Some(CloseFrame {
                        code: CloseCode::Policy,
                        reason: "rejected".into(),
                    }));
                    let end = Instant::now() + Duration::from_millis(500);
                    while Instant::now() < end && ws.read().is_ok() {}
                    let _ = ws.flush();
                    return None;
                }
                Ok(Message::Close(_)) => return None,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if is_timeout(&e) && Instant::now() < deadline => {}
                Err(_) => return None,
            }
        }
    }

    fn pump(
        &self,
        ws: &mut WebSocket<TcpStream>,
        conn_id: u64,
        rx: &Receiver<String>,
        pending: &Pending,
    ) {
        loop {
            loop {
                match rx.try_recv() {
                    Ok(text) => {
                        if ws.send(Message::text(text)).is_err() {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let _ = ws.close(Some(CloseFrame {
                            code: CloseCode::Normal,
                            reason: "another editor connected".into(),
                        }));
                        let _ = ws.flush();
                        return;
                    }
                }
            }
            match ws.read() {
                Ok(Message::Text(t)) => match serde_json::from_str::<ClientMsg>(t.as_str()) {
                    Ok(ClientMsg::Response(r)) => {
                        if let Some(s) = lock(pending).remove(&r.id) {
                            let _ = s.send(r);
                        }
                    }
                    Ok(ClientMsg::Status {
                        allow_edits,
                        map_name,
                    }) => {
                        if let Some(c) =
                            lock(&self.editor).as_mut().filter(|c| c.conn_id == conn_id)
                        {
                            c.info.allow_edits = allow_edits;
                            c.info.map_name = map_name;
                        }
                    }
                    Ok(ClientMsg::Hello { .. }) => {}
                    Err(e) => log!("live link: ignored a malformed message from the editor: {e}"),
                },
                Ok(Message::Close(_)) => return,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if is_timeout(&e) => {}
                Err(_) => return,
            }
        }
    }
}

fn is_timeout(e: &std::io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

fn send(ws: &mut WebSocket<TcpStream>, msg: &ServerMsg) -> tungstenite::Result<()> {
    let text = serde_json::to_string(msg).unwrap_or_default();
    ws.send(Message::text(text))
}
