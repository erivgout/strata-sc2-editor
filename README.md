<img src="assets/icon.svg" width="72" height="72" alt="">

# Strata

Strata is a StarCraft II map editor written in Rust. The same code runs in the
browser (WebAssembly), installs as an app (PWA), and runs natively on Windows,
macOS and Linux. The goal is an editor that is faster and easier to use than
the official Galaxy Editor, and that AI assistants can drive through MCP.

> Strata is an early work in progress. The list below separates what works
> today from what is planned. Back up your maps and keep the official editor
> around for anything Strata cannot do yet.

## Strata and the official editor

| | Galaxy Editor | Strata |
|---|---|---|
| Platforms | Windows and macOS, with a Battle.net install | Any modern browser, installable app, or native build. No game install needed to open maps. |
| Finding things | Nested menus and modules | One command palette (Ctrl+K) with every command and its shortcut |
| Layout | Separate module windows | Dockable panels around the map; triggers never hide the map |
| Undo | Per editor module | One history for every change, including edits made by AI clients |
| AI assistants | None | Built-in MCP server, plus a live link that applies AI edits in the open editor (undoable, labeled AI) |
| File format | SC2Map archives and folders | Same files, written back losslessly; convert either way with Save as; plus a JSON project format |
| Custom assets | Import module, then wire up models, actors and sounds by hand | Drop files on the window; textures land where their models expect them; a custom unit (model, portrait, icon, voice) in one dialog |

What works today:

- Real maps: opens `.SC2Map` archives and folder maps (the official
  editor's component format), and writes them back byte-for-byte unless you
  changed something. **File › Save map as .SC2Map file** and **Save map as
  folder** convert between the two. Verified in StarCraft II (build 98310):
  a map made from scratch in Strata, a real map saved unchanged, and edited
  real maps all load and play.
- Custom models, textures and sounds (**Assets** panel): drop files or a
  folder on the window, or use **Map › Import models, textures and sounds**.
  Textures go to the path their model looks for, and files the map's data
  is missing go where the data expects them. The panel shows what uses each
  file, plays sounds, and lists what's missing (checked against your game
  install). **New custom unit** makes a unit from any unit's stats with its
  own model, portrait, icon and voice lines in one step, ready to place;
  **New sound** turns audio files into a sound for triggers or voices.
  Custom models draw in the 3D view from the map's own files.
- Real game data: on desktop Strata reads the base game's units, abilities,
  weapons and names straight from your StarCraft II install (in a browser,
  pick the install folder once). The Data editor shows inherited values,
  such as Marine's 45 life.
- Map editing: 3D terrain with sculpt, paint, cliff level and pathing
  brushes; units, doodads, points, start locations, regions and cameras.
- Triggers: editor with a searchable library of 348 functions taken from the
  game's own trigger library, live Galaxy script with a linter.
- Data, Text, Map properties, Problems and History panels; workspaces
  (Ctrl+1..4), command palette, dark and light themes, autosave,
  installable web app.
- MCP: `strata-mcp` for Claude Code, Claude Desktop and other MCP clients.

## Known issues

- Maps whose triggers were made in the official editor keep those triggers
  untouched, but Strata can't add its own triggers to them yet: saving such a
  map with Strata triggers is refused rather than breaking the existing ones.
- Cliff edits change cliff levels but don't generate cliff or ramp meshes.
- Water and the unbuildable / no-fly pathing flags can't be edited yet.
- Editing `DocInfo/*` or player-name strings in the Text panel may be
  overridden by Map properties on save; teams and start locations edited in
  Map properties aren't saved yet.
- `strata-mcp --http` (opt-in) is loopback-only and Origin-checked but does
  not require the pairing token yet; the default stdio transport is
  unaffected.
- Archives whose `(listfile)` is missing entries are refused, because their
  unnamed files couldn't be preserved on save.
- Model animations aren't played: units and doodads show their first pose.
- On Windows 11, "Open in Strata" for map folders is under **Show more
  options** in Explorer's context menu. Dropping a map folder on Strata's
  window or shortcut also opens it.

## Screenshots

Screenshots are coming.

## Download

**Windows:** download `strata-v<version>-windows-x64-setup.exe` from the
[latest release](https://github.com/erivgout/strata-sc2-editor/releases/latest)
and run it. It installs Strata for your account (no admin rights) into
`%LOCALAPPDATA%\Programs\Strata`, adds it to the Start menu and to
Settings › Apps (where you can uninstall it), lists Strata under **Open
with** for `.SC2Map` files, and adds **Open in Strata** to map folders. Windows
keeps the app you already chose for `.SC2Map` files (usually the StarCraft
II editor); to open maps in Strata on double-click, leave **Choose Strata for
.SC2Map files** checked at the end of setup and pick Strata in the settings
page it opens. Windows may warn that the installer is from an unknown
publisher (it isn't code-signed yet): choose **More info → Run anyway**.

Prefer no installer? `strata-v<version>-windows-x64.exe` is the same app as a
single file you can run from anywhere.

Strata checks for new versions when it starts (and from **Help → Check for
updates…**). When one is available it asks first; **Update now** downloads
it, verifies it against the release's SHA-256 checksum, and finishes the
update when you restart or quit.

`strata-mcp-v<version>-windows-x64.exe` is the MCP server for AI assistants
(see [AI assistants](#ai-assistants-mcp)).

To publish a release: bump `version` in `Cargo.toml`, push to `main`, then
run `./scripts/release.ps1 -Version <version> -Notes "…"`. It needs
[Inno Setup 6](https://jrsoftware.org/isinfo.php) for the installer
(`winget install JRSoftware.InnoSetup --scope user`); the installer script is
`installer/strata.iss`.

## Build and run

You need a recent stable Rust toolchain (1.85 or newer).

### Native

```sh
cargo run --release -p strata-app
```

The binary is called `strata`. On Linux you may need the usual windowing
development packages (for example `libxkbcommon-dev`, `libgtk-3-dev` for file
dialogs).

### Web

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked      # trunk 0.22 or newer
cd crates/strata-app
trunk serve                       # development server on http://127.0.0.1:8080
trunk build --release             # production build in crates/strata-app/dist
```

Strata needs a browser with WebGL 2 (current Chrome, Edge, Firefox and Safari).
The release build is about 7 MB of WebAssembly (about 2.7 MB gzipped) and is
cached by the service worker after the first visit.

### Install as an app

Open the web version in Chrome or Edge and choose **Help › Install Strata as an
app** (or the install icon in the address bar). On Safari, use **Share › Add
to Dock** or **Add to Home Screen**. The installed app works offline. When a
new version is deployed, Strata offers to reload.

## Keyboard shortcuts

On macOS, Ctrl is Cmd. Press F1 in the app for the full list.

| Action | Shortcut |
|---|---|
| Search commands | Ctrl+K |
| New map | Ctrl+N |
| Open map | Ctrl+O |
| Save map | Ctrl+S |
| Save map as | Ctrl+Shift+S |
| Undo | Ctrl+Z |
| Redo | Ctrl+Shift+Z or Ctrl+Y |
| Keyboard shortcuts | F1 |
| Modes: Select, Sculpt, Paint, Cliffs, Pathing | 1, 2, 3, 4, 5 |
| Modes: Units, Doodads, Points, Regions, Cameras | 6, 7, 8, 9, 0 |

Undo, redo and the number keys pause while a text field has focus. Browsers
reserve some shortcuts (such as Ctrl+N); use the menu or the command palette
there.

## Base-game data

Strata reads the real units, abilities, weapons and their names from your own
StarCraft II install, so the unit palette lists every unit and the Data panel
shows real inherited values (Marine LifeMax 45, from Marine in the base game).
The native app finds the install on start (`STRATA_SC2_DIR` or the default
Program Files folder) and caches a compact index; in the browser, run **Map >
Load game data from StarCraft II folder…** (Chrome or Edge read the folder
in place; nothing is uploaded). No Blizzard data ships with Strata.

## AI assistants (MCP)

Strata ships an MCP server so assistants such as Claude can inspect and edit
maps, headless or live inside the running editor, with every change undoable
and labeled in History. See [`crates/strata-mcp`](crates/strata-mcp).

## Project layout

```
crates/
  strata-mpq/      MPQ archive reading and writing (.SC2Map containers)
  strata-casc/     Reads the installed game's CASC storage (base-game mods)
  strata-core/     Document model, undoable edits, ops API, validation, Galaxy codegen
  strata-format/   Load and save .SC2Map archives and folders to the document model
  strata-app/      The editor (egui + egui_dock); native and web
    assets/        Fonts, icons, PWA manifest, service worker, web loader
    index.html     Web entry point and loading screen (built with trunk)
  strata-mcp/      MCP server
installer/         Windows installer (Inno Setup script)
scripts/           Release script
docs/DESIGN.md     Design system: tokens, type, layout, principles
```

## Contributing

Read [docs/DESIGN.md](docs/DESIGN.md) before changing the UI. Run
`cargo test --workspace` and
`cargo check -p strata-app --target wasm32-unknown-unknown` before sending a
change.

## License

Strata is dual-licensed under the MIT license or the Apache License 2.0, at
your option.

Bundled fonts: [Barlow](https://github.com/jpt/barlow) and
[JetBrains Mono](https://github.com/JetBrains/JetBrainsMono), both under the
SIL Open Font License 1.1 (see `crates/strata-app/assets/fonts`).

Strata is not affiliated with, endorsed by, or sponsored by Blizzard
Entertainment. StarCraft is a trademark or registered trademark of Blizzard
Entertainment, Inc. in the U.S. and other countries. Strata does not include
any Blizzard game assets.
