//! Test the open map in StarCraft II, the way the official editor's Test
//! Document does: write the map under the install's `Maps\Test`, write a
//! test config beside it, and start `SC2Switcher` with `-run`. The switcher
//! starts the current game build, which loads the map straight into a game.
//!
//! The map is written from memory, so unsaved edits are tested too.

use std::path::{Path, PathBuf};

use strata_core::MapDocument;

/// The staged map's name under `Maps\Test`. Only Strata's own two files
/// there are ever replaced; the editor's `EditorTest` files are left alone.
const MAP_NAME: &str = "StrataTest.SC2Map";
const CONFIG_NAME: &str = "StrataTest.SC2TestConfig";

/// The test config the editor writes (players 1-5 human-controlled slots,
/// player 16 neutral).
const TEST_CONFIG: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n\
<TestConfig>\r\n\
    <Attribute AttNamespace=\"0\" Id=\"1\" Player=\"1\" Value=\"0001\"/>\r\n\
    <Attribute AttNamespace=\"0\" Id=\"1\" Player=\"2\" Value=\"0001\"/>\r\n\
    <Attribute AttNamespace=\"0\" Id=\"1\" Player=\"4\" Value=\"0001\"/>\r\n\
    <Attribute AttNamespace=\"0\" Id=\"1\" Player=\"3\" Value=\"0001\"/>\r\n\
    <Attribute AttNamespace=\"0\" Id=\"1\" Player=\"5\" Value=\"0001\"/>\r\n\
    <Attribute AttNamespace=\"0\" Id=\"2\" Player=\"16\" Value=\"0001\"/>\r\n\
</TestConfig>\r\n";

/// The launcher in an install folder, if it has one.
fn switcher(root: &Path) -> Option<PathBuf> {
    [["Support64", "SC2Switcher_x64.exe"], ["Support", "SC2Switcher.exe"]]
        .iter()
        .map(|[dir, exe]| root.join(dir).join(exe))
        .find(|p| p.is_file())
}

/// The arguments the editor passes for Test Document.
fn arguments(config: &Path) -> Vec<std::ffi::OsString> {
    let mut args: Vec<std::ffi::OsString> = [
        "-run",
        &format!("Test\\{MAP_NAME}"),
        "-displaymode",
        "2",
        "-preload",
        "1",
        "-NoUserCheats",
        "-reloadcheck",
        "-meleeMod",
        "Void",
        "-difficulty",
        "2",
        "-speed",
        "2",
        "-testconfig",
    ]
    .iter()
    .map(Into::into)
    .collect();
    args.push(config.as_os_str().to_owned());
    args
}

/// Is a StarCraft II game client already running? A second one can't start
/// while it is.
#[cfg(windows)]
fn game_running() -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map(|o| {
            let out = String::from_utf8_lossy(&o.stdout).to_ascii_lowercase();
            out.lines().any(|l| l.starts_with("\"sc2_x64.exe\"") || l.starts_with("\"sc2.exe\""))
        })
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn game_running() -> bool {
    false
}

/// Write `doc` into the install at `root` and start StarCraft II on it.
/// Returns a short status for a toast, or a message explaining what to do.
pub fn launch(doc: &MapDocument, root: Option<&Path>) -> Result<String, String> {
    let Some(root) = root else {
        return Err("Strata couldn't find StarCraft II. Use \"Load game data from StarCraft II folder…\" to point it at your install, then test again.".into());
    };
    let Some(switcher) = switcher(root) else {
        return Err(format!(
            "{} has no SC2Switcher (Support64\\SC2Switcher_x64.exe). Repair StarCraft II in the Battle.net app, then test again.",
            root.display()
        ));
    };
    if game_running() {
        return Err("StarCraft II is already running. Close it, then test again.".into());
    }
    let bytes = strata_format::save_sc2map(doc).map_err(|e| format!("Strata couldn't build the map to test: {e}."))?;

    let test_dir = root.join("Maps").join("Test");
    let map = test_dir.join(MAP_NAME);
    let config = test_dir.join(CONFIG_NAME);
    let cant_write = |e: std::io::Error| {
        format!(
            "Strata couldn't write the test map to {} ({e}). Check that your Windows account can write to the StarCraft II folder.",
            test_dir.display()
        )
    };
    std::fs::create_dir_all(&test_dir).map_err(cant_write)?;
    if map.is_dir() {
        std::fs::remove_dir_all(&map).map_err(cant_write)?;
    }
    std::fs::write(&map, bytes).map_err(cant_write)?;
    std::fs::write(&config, TEST_CONFIG).map_err(cant_write)?;

    std::process::Command::new(&switcher)
        .args(arguments(&config))
        .current_dir(root)
        .spawn()
        .map_err(|e| format!("Strata couldn't start StarCraft II ({} : {e}).", switcher.display()))?;
    Ok("Starting StarCraft II with this map…".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_match_the_editor() {
        let args = arguments(Path::new("C:\\SC2\\Maps\\Test\\StrataTest.SC2TestConfig"));
        let args: Vec<_> = args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(args[..2], ["-run", "Test\\StrataTest.SC2Map"]);
        assert_eq!(args.last().unwrap(), "C:\\SC2\\Maps\\Test\\StrataTest.SC2TestConfig");
        assert!(args.windows(2).any(|w| w == ["-testconfig", "C:\\SC2\\Maps\\Test\\StrataTest.SC2TestConfig"]));
    }

    #[test]
    fn no_install_explains_how_to_fix_it() {
        let e = launch(&MapDocument::new("Test", 64, 64), None).unwrap_err();
        assert!(e.contains("Load game data"));
        let dir = std::env::temp_dir().join(format!("strata-playtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let e = launch(&MapDocument::new("Test", 64, 64), Some(&dir)).unwrap_err();
        assert!(e.contains("SC2Switcher"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
