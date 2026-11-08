//! Streamable HTTP transport: `POST /mcp` with JSON responses (no SSE
//! streams; `GET /mcp` answers 405 as the spec allows). Loopback only, with
//! `Origin` and `Host` checks against DNS rebinding.

use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use strata_core::ops::link;

use crate::log;
use crate::rpc;
use crate::server::Strata;

const MAX_HEADERS: usize = 64 * 1024;
const MAX_BODY: usize = 32 * 1024 * 1024;

pub struct HttpServer {
    pub addr: SocketAddr,
    listener: TcpListener,
}

impl HttpServer {
    pub fn bind(addr: &str) -> std::io::Result<HttpServer> {
        let listener = TcpListener::bind(addr)?;
        let addr = listener.local_addr()?;
        if !addr.ip().is_loopback() {
            return Err(std::io::Error::new(
                ErrorKind::InvalidInput,
                format!(
                    "refusing to serve HTTP on {addr}: use a loopback address such as 127.0.0.1"
                ),
            ));
        }
        Ok(HttpServer { addr, listener })
    }

    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// Serve forever (blocks).
    pub fn run(self, strata: Arc<Strata>, allowed_origins: Arc<Vec<String>>) {
        for stream in self.listener.incoming().flatten() {
            let (s, o) = (strata.clone(), allowed_origins.clone());
            let _ = thread::Builder::new()
                .name("http-conn".into())
                .spawn(move || {
                    if let Err(e) = serve_conn(stream, &s, &o) {
                        if !matches!(
                            e.kind(),
                            ErrorKind::UnexpectedEof
                                | ErrorKind::ConnectionReset
                                | ErrorKind::TimedOut
                                | ErrorKind::WouldBlock
                        ) {
                            log!("http: {e}");
                        }
                    }
                });
        }
    }

    /// Serve on a background thread.
    pub fn spawn(self, strata: Arc<Strata>, allowed_origins: Vec<String>) -> SocketAddr {
        let addr = self.addr;
        let origins = Arc::new(allowed_origins);
        thread::spawn(move || self.run(strata, origins));
        addr
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Read one request; `None` when the client closed the connection cleanly.
fn read_request(stream: &mut TcpStream, buf: &mut Vec<u8>) -> std::io::Result<Option<Request>> {
    let bad = |m: &str| std::io::Error::new(ErrorKind::InvalidData, m.to_owned());
    let mut chunk = [0u8; 8192];
    loop {
        let mut headers = [httparse::EMPTY_HEADER; 48];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(buf).map_err(|e| bad(&e.to_string()))? {
            httparse::Status::Complete(head_len) => {
                let method = req.method.unwrap_or("").to_owned();
                let path = req.path.unwrap_or("").to_owned();
                let headers: Vec<(String, String)> = req
                    .headers
                    .iter()
                    .map(|h| {
                        (
                            h.name.to_owned(),
                            String::from_utf8_lossy(h.value).into_owned(),
                        )
                    })
                    .collect();
                let len = headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| {
                        v.trim()
                            .parse::<usize>()
                            .map_err(|_| bad("bad content-length"))
                    })
                    .transpose()?
                    .unwrap_or(0);
                if headers.iter().any(|(k, v)| {
                    k.eq_ignore_ascii_case("transfer-encoding")
                        && !v.eq_ignore_ascii_case("identity")
                }) {
                    return Err(bad(
                        "chunked request bodies are not supported; send Content-Length",
                    ));
                }
                if len > MAX_BODY {
                    return Err(bad("request body too large"));
                }
                while buf.len() < head_len + len {
                    let n = stream.read(&mut chunk)?;
                    if n == 0 {
                        return Err(ErrorKind::UnexpectedEof.into());
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let body = buf[head_len..head_len + len].to_vec();
                buf.drain(..head_len + len);
                return Ok(Some(Request {
                    method,
                    path,
                    headers,
                    body,
                }));
            }
            httparse::Status::Partial => {
                if buf.len() > MAX_HEADERS {
                    return Err(bad("request headers too large"));
                }
                let n = stream.read(&mut chunk)?;
                if n == 0 {
                    return if buf.is_empty() {
                        Ok(None)
                    } else {
                        Err(ErrorKind::UnexpectedEof.into())
                    };
                }
                buf.extend_from_slice(&chunk[..n]);
            }
        }
    }
}

struct Response {
    status: u16,
    reason: &'static str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

impl Response {
    fn new(status: u16, reason: &'static str) -> Self {
        Response {
            status,
            reason,
            headers: Vec::new(),
            body: Vec::new(),
        }
    }
    fn text(status: u16, reason: &'static str, body: &str) -> Self {
        let mut r = Self::new(status, reason);
        r.headers
            .push(("Content-Type", "text/plain; charset=utf-8".into()));
        r.body = body.as_bytes().to_vec();
        r
    }
    fn json(body: String) -> Self {
        let mut r = Self::new(200, "OK");
        r.headers.push(("Content-Type", "application/json".into()));
        r.body = body.into_bytes();
        r
    }
}

fn host_allowed(host: Option<&str>) -> bool {
    let Some(h) = host else { return true };
    let h = h.trim();
    let name = if h.starts_with('[') {
        h.split(']').next().map(|s| &s[1..]).unwrap_or("")
    } else {
        h.split(':').next().unwrap_or("")
    };
    name.eq_ignore_ascii_case("localhost") || name == "127.0.0.1" || name == "::1"
}

fn route(req: &Request, strata: &Strata, allowed: &[String]) -> Response {
    let path = req.path.split('?').next().unwrap_or("");
    let origin = req.header("origin");
    if !link::origin_allowed(origin, allowed) {
        log!("http: refused origin {origin:?} (allow it with --allow-origin)");
        return Response::text(403, "Forbidden", "origin not allowed");
    }
    if !host_allowed(req.header("host")) {
        return Response::text(403, "Forbidden", "host not allowed");
    }
    let mut resp = match (req.method.as_str(), path) {
        ("OPTIONS", _) => {
            let mut r = Response::new(204, "No Content");
            r.headers.push((
                "Access-Control-Allow-Methods",
                "POST, GET, DELETE, OPTIONS".into(),
            ));
            r.headers.push((
                "Access-Control-Allow-Headers",
                "content-type, accept, authorization, mcp-session-id, mcp-protocol-version, last-event-id".into(),
            ));
            r.headers.push(("Access-Control-Max-Age", "600".into()));
            r
        }
        ("POST", "/mcp") => {
            if let Some(v) = req.header("mcp-protocol-version") {
                if !rpc::SUPPORTED_VERSIONS.contains(&v.trim()) {
                    return Response::text(
                        400,
                        "Bad Request",
                        &format!("unsupported MCP-Protocol-Version {v}"),
                    );
                }
            }
            let text = String::from_utf8_lossy(&req.body);
            let is_init = text.contains("\"initialize\"");
            match rpc::handle_text(strata, &text) {
                Some(reply) => {
                    let mut r = Response::json(reply);
                    if is_init {
                        r.headers.push(("Mcp-Session-Id", crate::token::generate()));
                    }
                    r
                }
                None => Response::new(202, "Accepted"),
            }
        }
        ("GET", "/mcp") => {
            let mut r = Response::text(
                405,
                "Method Not Allowed",
                "this server does not offer an SSE stream; POST JSON-RPC to /mcp",
            );
            r.headers.push(("Allow", "POST, DELETE, OPTIONS".into()));
            r
        }
        ("DELETE", "/mcp") => Response::new(204, "No Content"),
        ("GET", "/" | "/health") => Response::text(
            200,
            "OK",
            &format!(
                "{} {} is running. MCP endpoint: POST /mcp\n",
                crate::NAME,
                crate::VERSION
            ),
        ),
        _ => Response::text(404, "Not Found", "not found; the MCP endpoint is /mcp"),
    };
    if let Some(o) = origin {
        resp.headers
            .push(("Access-Control-Allow-Origin", o.to_owned()));
        resp.headers
            .push(("Access-Control-Expose-Headers", "Mcp-Session-Id".into()));
        resp.headers.push(("Vary", "Origin".into()));
    }
    resp
}

fn serve_conn(mut stream: TcpStream, strata: &Strata, allowed: &[String]) -> std::io::Result<()> {
    if !stream.peer_addr()?.ip().is_loopback() {
        return Ok(());
    }
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    let mut buf = Vec::new();
    loop {
        let req = match read_request(&mut stream, &mut buf) {
            Ok(Some(r)) => r,
            Ok(None) => return Ok(()),
            Err(e) if e.kind() == ErrorKind::InvalidData => {
                write_response(
                    &mut stream,
                    &Response::text(400, "Bad Request", &e.to_string()),
                    true,
                )?;
                return Ok(());
            }
            Err(e) => return Err(e),
        };
        let close = req
            .header("connection")
            .is_some_and(|c| c.eq_ignore_ascii_case("close"));
        let resp = route(&req, strata, allowed);
        write_response(&mut stream, &resp, close)?;
        if close {
            return Ok(());
        }
    }
}

fn write_response(stream: &mut TcpStream, r: &Response, close: bool) -> std::io::Result<()> {
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n",
        r.status,
        r.reason,
        r.body.len()
    );
    for (k, v) in &r.headers {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str(if close {
        "Connection: close\r\n\r\n"
    } else {
        "Connection: keep-alive\r\n\r\n"
    });
    stream.write_all(head.as_bytes())?;
    stream.write_all(&r.body)?;
    stream.flush()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use strata_core::MapDocument;

    fn send(addr: SocketAddr, raw: &str) -> (u16, String) {
        let mut s = TcpStream::connect(addr).unwrap();
        s.write_all(raw.as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        let status = out
            .split(' ')
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let body = out.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
        (status, body)
    }

    fn post(addr: SocketAddr, body: &str, extra: &str) -> (u16, String) {
        send(
            addr,
            &format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ),
        )
    }

    #[test]
    fn http_transport() {
        crate::set_quiet(true);
        let strata = Arc::new(Strata::headless_only(MapDocument::new("h", 32, 32)));
        let addr = HttpServer::bind("127.0.0.1:0")
            .unwrap()
            .spawn(strata, vec![]);

        let init = json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18"}}).to_string();
        let (status, body) = post(addr, &init, "");
        assert_eq!(status, 200);
        let v: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");

        let note = json!({"jsonrpc": "2.0", "method": "notifications/initialized"}).to_string();
        assert_eq!(
            post(addr, &note, "MCP-Protocol-Version: 2025-06-18\r\n").0,
            202
        );

        let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "map_summary", "arguments": {}}}).to_string();
        let (status, body) = post(addr, &call, "Origin: http://localhost:5173\r\n");
        assert_eq!(status, 200);
        assert!(body.contains("\\\"name\\\": \\\"h\\\""), "{body}");

        assert_eq!(post(addr, &call, "Origin: https://evil.example\r\n").0, 403);
        assert_eq!(
            post(addr, &call, "MCP-Protocol-Version: 1999-01-01\r\n").0,
            400
        );
        assert_eq!(
            send(
                addr,
                "GET /mcp HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .0,
            405
        );
        assert_eq!(
            send(
                addr,
                "GET /mcp HTTP/1.1\r\nHost: evil.example\r\nConnection: close\r\n\r\n"
            )
            .0,
            403
        );
        assert_eq!(
            send(
                addr,
                "GET /nope HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            )
            .0,
            404
        );
        assert!(HttpServer::bind("0.0.0.0:0").is_err());
    }
}
