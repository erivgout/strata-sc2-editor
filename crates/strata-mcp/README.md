# strata-mcp

An [MCP](https://modelcontextprotocol.io) server that lets AI assistants read
and edit StarCraft II maps with Strata. It works two ways:

- **Live**: the Strata editor you have open (web or native) connects to
  strata-mcp, and the assistant's tool calls edit that map in front of you.
  Every change is one undo step, labeled AI in History.
- **Headless**: with no editor connected, tool calls edit a map held by the
  server, which it opens from and saves to disk.

`editor_status` tells the assistant which one is active.

## Set up

Build and install from the repository root:

```sh
cargo install --path crates/strata-mcp
```

Then register it with your client.

**Claude Code**

```sh
claude mcp add strata -- strata-mcp
```

**Claude Desktop**: add this to `claude_desktop_config.json` and restart:

```json
{
  "mcpServers": {
    "strata": { "command": "strata-mcp" }
  }
}
```

**Other clients (Streamable HTTP)**: run `strata-mcp --http 127.0.0.1:7341`
and point the client at `http://127.0.0.1:7341/mcp`.

## Edit live in the editor

1. Your AI client starts strata-mcp. On start it prints a session token to
   stderr (your client's MCP log) and saves it to the token file:
   `%LOCALAPPDATA%\strata-mcp\token` on Windows, `~/Library/Caches/strata-mcp/token`
   on macOS, `$XDG_RUNTIME_DIR/strata-mcp/token` (or `~/.cache/...`) on Linux.
   The token is reused across restarts; `--new-token` makes a new one.
2. In Strata, open the **AI link** panel (View menu).
   - Native editor: leave Token empty and click **Connect**. It reads the token file.
   - Web editor: paste the token and click **Connect**, or open Strata with
     `?mcp=TOKEN` in the address.
3. The status pill reads "Connected to strata-mcp vX". With **Connect
   automatically** on, the editor reconnects with backoff when the server
   restarts.
4. **Allow AI edits** controls whether the assistant may change the map.
   When it is off, reads still work and every edit is refused with an error
   the assistant sees.

The activity log in the panel lists every request (tool, result) and shows
its JSON when clicked.

Browsers pause background tabs, so keep the Strata tab in front while the
assistant works. A request that arrives while the tab is hidden is refused
straight away with an explanation instead of hanging. Requests time out after
20 seconds.

## Command line

```
strata-mcp [--http ADDR] [--ws ADDR | --no-live] [--map PATH]
           [--token TOKEN] [--new-token] [--allow-origin ORIGIN]... [--quiet]
```

| Option | Meaning |
|---|---|
| (none) | MCP over stdio; live link on `ws://127.0.0.1:7342` |
| `--http ADDR` | Serve Streamable HTTP at `http://ADDR/mcp` instead of stdio (loopback only) |
| `--ws ADDR` | Live-link address (loopback only) |
| `--no-live` | Do not listen for editors; headless only |
| `--map PATH` | Open a `.SC2Map`, map folder or `.strata.json` at start |
| `--token TOKEN` | Fixed live-link token (16+ letters, digits, `-`, `_`) |
| `--new-token` | Replace the saved token with a fresh random one |
| `--allow-origin ORIGIN` | Also accept browser connections from this origin, e.g. where you host the web app (repeatable) |
| `-q`, `--quiet` | No logging |

stdout carries the stdio transport; all logging goes to stderr. If the
live-link port is taken (for example by a second strata-mcp), the server
keeps running headless and `editor_status` says why.

## Tools

Coordinates are world units, one per terrain cell. (0, 0) is the south-west
corner; x grows east and y grows north. New maps are flat at height 12
(cliff level 2; each cliff level is 2 units). Facing is in degrees,
counter-clockwise from east.

| Tool | What it does |
|---|---|
| `editor_status` | Where calls go (editor or headless), allow-edits state, map and save state |
| `map_summary` | Name, size, players, object/trigger/data counts, terrain overview, problems |
| `map_new` | Replace the map with a new flat one |
| `map_open` | Open a `.SC2Map`, map folder or `.strata.json` from disk |
| `map_save` | Save to `.SC2Map`, a map folder or `.strata.json` |
| `terrain_brush` | Raise, lower, smooth, flatten, noise, paint, erase, cliff, pathing along a path |
| `terrain_stats` | Height, cliff, texture and pathing statistics for the map or a rect |
| `sample_height` | Height, cliff level and pathing at points |
| `add_texture_layer` | Add a texture layer (max 8) to paint with |
| `set_water` | Set or remove the water plane |
| `place_object` | Place a unit, doodad or point |
| `move_object` | Move an object |
| `set_object_property` | Change owner, facing, scale, name or type |
| `delete_objects` | Delete objects by id |
| `list_objects` | Find objects by kind, type, owner, area, id or name |
| `create_region` / `delete_region` | Rect, circle or polygon regions |
| `list_regions` | Regions and cameras |
| `create_camera` | Camera objects |
| `create_trigger` / `update_trigger` / `delete_trigger` | Triggers, full model or simplified JSON |
| `list_triggers` / `get_trigger` | Trigger outline, variables; one trigger with its Galaxy |
| `get_galaxy_script` | Generated MapScript.galaxy (or one trigger's part) |
| `create_variable` / `delete_variable` | Global trigger variables |
| `set_custom_script` | Custom Galaxy code |
| `set_string` / `get_strings` | Localized text |
| `set_data_field` / `list_data` | GameData catalog fields (e.g. CUnit Marine LifeMax) |
| `set_map_info` / `set_player` | Map properties and player slots |
| `validate_map` | Problems list |
| `undo` / `redo` / `history` | History on the active target |

Tool failures (bad arguments, out-of-range values, missing names) come back
as tool results with `isError: true` and a message that says what was wrong
and what is valid. Unknown argument names are rejected, which catches typos.

### Simplified trigger JSON

```json
{"name": "Spawn wave",
 "events": [{"function": "TriggerAddEventTimePeriodic", "args": [30.0, "c_timeGame"]}],
 "conditions": ["gv_wave < 10"],
 "actions": [
   {"function": "UnitCreate", "args": [5, "\"Zergling\"", 0, 2, {"point": "Spawn"}, 270.0]},
   {"set": "wave", "value": "gv_wave + 1"},
   {"if": "gv_wave == 10", "then": [{"comment": "final wave"}]}
 ]}
```

Strings are Galaxy source: numbers, `true`/`false`, `c_*` presets and
`"quoted"` strings become structured values; anything else is kept verbatim.
Object forms: `{"string"}`, `{"text"}` (localized key), `{"int"}`,
`{"fixed"}`, `{"var"}`, `{"preset"}`, `{"point"}`, `{"raw"}`,
`{"function", "args"}`, `{"op", "lhs", "rhs"}`, `{"not"}`. Actions:
calls, `set`, `if`/`then`/`else`, `while`/`do`, `for`/`from`/`to`/`do`,
`wait`, `comment`, `raw`, `"return"`. The full model JSON returned by
`get_trigger` is accepted too.

## Security model

- Both listeners bind loopback addresses only and refuse non-loopback peers.
- The live link requires the session token in the editor's first message;
  a wrong token gets a rejection and the socket closes. Tokens are compared
  in constant time. The token file is written with user-only permissions
  on Unix.
- Browser `Origin` headers are checked on the WebSocket handshake and on
  HTTP: allowed are no origin (native clients), `localhost`, `127.0.0.1` and
  `[::1]` on any port, and origins given with `--allow-origin`. Others get
  403. HTTP also checks `Host` against DNS rebinding.
- "Allow AI edits" in the editor is enforced by the editor, so it holds even
  if the server misbehaves.
- `map_open` and `map_save` touch the local disk on behalf of the MCP client
  you configured, like any local tool; `map_save` only writes `.SC2Map`,
  `.strata.json` or into an existing map folder. Replacing a map with unsaved
  changes needs `discard_changes: true`.

## How the live link works

The editor is the WebSocket client; messages are JSON text frames defined in
`strata_core::ops::link`:

1. editor → server `hello {token, protocol, client, version, allow_edits, map_name}`
2. server → editor `welcome {server, version, protocol}` or `rejected {reason}`
3. server → editor `request {id, tool, kind}` where `kind` is an `op`
   (`strata_core::ops::Op`), a `query` (`strata_core::ops::query::Query`),
   `undo`, `redo`, `status`, `load` or `saved`
4. editor → server `response {id, ok, result | error}`
5. editor → server `status {allow_edits, map_name}` when either changes

Ops and queries run through the same code headless and live
(`Op::run`, `Query::run`), so answers match. A newer editor connection
replaces an older one.

## Development

```sh
cargo test -p strata-mcp      # protocol, tools, HTTP, and live-link end-to-end tests
cargo test -p strata-core ops # op validation and undo, queries, link messages
```

Saving and loading `.SC2Map` goes through `strata-format`; `.strata.json`
(serde JSON of `MapDocument`) always works and is lossless.
