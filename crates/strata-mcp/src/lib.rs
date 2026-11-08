//! `strata-mcp`: a Model Context Protocol server for the Strata map editor.
//!
//! - [`rpc`]: JSON-RPC 2.0 / MCP message handling (transport independent).
//! - [`tools`]: the tool catalog (schemas, descriptions) and dispatch.
//! - [`server`]: the editing target, either the headless document or a
//!   connected editor reached through [`bridge`].
//! - [`bridge`]: the localhost WebSocket the running editor connects to.
//! - [`http`]: Streamable HTTP transport (`POST /mcp`).
//! - [`trigger_json`]: the simplified trigger JSON accepted by trigger tools.
//! - [`token`]: the live-link session token.
//!
//! stdout belongs to the stdio transport; everything else logs to stderr.

pub mod bridge;
pub mod http;
pub mod rpc;
pub mod server;
pub mod token;
pub mod tools;
pub mod trigger_json;

pub const NAME: &str = "strata-mcp";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Silence [`log!`] output (tests, `--quiet`).
pub fn set_quiet(q: bool) {
    QUIET.store(q, std::sync::atomic::Ordering::Relaxed);
}

#[doc(hidden)]
pub fn log_enabled() -> bool {
    !QUIET.load(std::sync::atomic::Ordering::Relaxed)
}

/// Log one line to stderr (never stdout, which carries the stdio transport).
#[macro_export]
macro_rules! log {
    ($($t:tt)*) => {
        if $crate::log_enabled() {
            eprintln!("[strata-mcp] {}", format!($($t)*));
        }
    };
}
