//! `strata-mcp` command line. See `--help` and the crate README.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;

use strata_core::ops::link;
use strata_core::MapDocument;
use strata_mcp::bridge::Bridge;
use strata_mcp::http::HttpServer;
use strata_mcp::server::{read_map, Headless, LiveConfig, Strata};
use strata_mcp::{log, rpc, token};

const HELP: &str = "\
strata-mcp: MCP server for the Strata StarCraft II map editor

USAGE:
    strata-mcp [OPTIONS]

By default speaks MCP over stdio (for Claude Code, Claude Desktop and other
MCP clients) and listens for a running Strata editor on ws://127.0.0.1:7342.
When an editor is connected, tool calls edit the map open in it; otherwise
they edit a headless map kept in this process.

OPTIONS:
    --http ADDR           Serve Streamable HTTP at http://ADDR/mcp instead of
                          stdio, e.g. --http 127.0.0.1:7341 (loopback only)
    --ws ADDR             Live-link address (default 127.0.0.1:7342)
    --no-live             Do not listen for editors; headless only
    --map PATH            Open this map at start (.SC2Map, map folder or
                          .strata.json)
    --token TOKEN         Use a fixed live-link token instead of the saved one
    --new-token           Generate a fresh live-link token
    --allow-origin ORIGIN Also accept browser connections from ORIGIN, e.g.
                          https://strata.example.com (repeatable). localhost
                          and 127.0.0.1 origins are always accepted.
    -q, --quiet           Log nothing to stderr
    -h, --help            Show this help
    -V, --version         Show the version

SETUP:
    Claude Code:     claude mcp add strata -- strata-mcp
    Claude Desktop:  add {\"mcpServers\": {\"strata\": {\"command\": \"strata-mcp\"}}}
                     to claude_desktop_config.json
    Other clients:   strata-mcp --http 127.0.0.1:7341, then use
                     http://127.0.0.1:7341/mcp
    Then open the AI link panel in Strata, paste the token printed below and
    click Connect. The native editor reads the token file by itself.
";

struct Cli {
    http: Option<String>,
    ws: Option<String>,
    map: Option<PathBuf>,
    token: Option<String>,
    new_token: bool,
    origins: Vec<String>,
    quiet: bool,
}

fn parse_args() -> Result<Cli, String> {
    let mut cli = Cli {
        http: None,
        ws: Some(link::DEFAULT_WS_ADDR.into()),
        map: None,
        token: None,
        new_token: false,
        origins: Vec::new(),
        quiet: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(v.to_owned())),
            _ => (a.clone(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or(format!("{name} needs a value"))
        };
        match flag.as_str() {
            "-h" | "--help" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            "-V" | "--version" => {
                println!("{} {}", strata_mcp::NAME, strata_mcp::VERSION);
                std::process::exit(0);
            }
            "--http" => cli.http = Some(value("--http")?),
            "--ws" => cli.ws = Some(value("--ws")?),
            "--no-live" => cli.ws = None,
            "--map" => cli.map = Some(PathBuf::from(value("--map")?)),
            "--token" => cli.token = Some(value("--token")?),
            "--new-token" => cli.new_token = true,
            "--allow-origin" => cli.origins.push(value("--allow-origin")?),
            "-q" | "--quiet" => cli.quiet = true,
            other => return Err(format!("unknown option {other}; see --help")),
        }
    }
    Ok(cli)
}

fn main() {
    let cli = match parse_args() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("strata-mcp: {e}");
            std::process::exit(2);
        }
    };
    strata_mcp::set_quiet(cli.quiet);

    let (doc, path) = match &cli.map {
        Some(p) => match read_map(p) {
            Ok(d) => {
                log!("opened {}", p.display());
                (d, Some(p.clone()))
            }
            Err(e) => {
                eprintln!("strata-mcp: {e}");
                std::process::exit(1);
            }
        },
        None => (MapDocument::new("Untitled map", 128, 128), None),
    };

    let token_file = token::default_path();
    let mut live = LiveConfig {
        token_file: token_file.clone(),
        ..Default::default()
    };
    let bridge = match &cli.ws {
        None => {
            live.unavailable = Some("disabled with --no-live".into());
            None
        }
        Some(addr) => {
            let tok = match token::resolve(cli.token.clone(), cli.new_token, token_file.as_deref())
            {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("strata-mcp: {e}");
                    std::process::exit(2);
                }
            };
            match Bridge::start(addr, tok.clone(), cli.origins.clone()) {
                Ok(b) => {
                    log!("live link listening on {}", b.url());
                    log!("live link token: {tok}");
                    if let Some(p) = &token_file {
                        log!(
                            "token saved to {} (the native editor reads it automatically)",
                            p.display()
                        );
                    }
                    Some(b)
                }
                Err(e) => {
                    let why = if e.kind() == std::io::ErrorKind::AddrInUse {
                        format!("{addr} is already in use, probably by another strata-mcp; this one runs headless only")
                    } else {
                        format!("could not listen on {addr}: {e}")
                    };
                    log!("live link unavailable: {why}");
                    live.unavailable = Some(why);
                    None
                }
            }
        }
    };

    let http = match &cli.http {
        Some(addr) => match HttpServer::bind(addr) {
            Ok(h) => {
                live.http_url = Some(h.url());
                Some(h)
            }
            Err(e) => {
                eprintln!("strata-mcp: could not serve HTTP on {addr}: {e}");
                std::process::exit(1);
            }
        },
        None => None,
    };

    let strata = Arc::new(Strata::new(Headless::new(doc, path), bridge, live));

    if let Some(h) = http {
        log!("MCP over HTTP at {}", h.url());
        h.run(strata, Arc::new(cli.origins));
        return;
    }

    log!("MCP over stdio ready");
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Some(reply) = rpc::handle_text(&strata, &line) {
            if writeln!(stdout, "{reply}")
                .and_then(|_| stdout.flush())
                .is_err()
            {
                break;
            }
        }
    }
    log!("stdin closed; exiting");
}
