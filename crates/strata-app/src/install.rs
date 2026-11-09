//! Keeps the Windows installer's record of Strata current.
//!
//! The installer (`installer/strata.iss`) registers Strata in Settings >
//! Apps with its version. The in-app updater later replaces `strata.exe`
//! in place, so on start an installed Strata writes its own version into
//! that record. Portable copies (not in the installed folder) leave it alone.

/// The installer's uninstall key (Inno Setup: `{AppId}_is1`, per user).
const UNINSTALL_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{B8FEE9B6-88C3-4D61-902C-7B6031E45F70}_is1";

pub fn refresh_record() {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
    let Ok(key) = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(UNINSTALL_KEY, KEY_READ | KEY_SET_VALUE)
    else {
        return;
    };
    let Ok(location) = key.get_value::<String, _>("InstallLocation") else { return };
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) else { return };
    let same = |a: &std::path::Path, b: &std::path::Path| {
        let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        canon(a) == canon(b)
    };
    if !same(&dir, std::path::Path::new(location.trim_end_matches(['\\', '/']))) {
        return;
    }
    let version = env!("CARGO_PKG_VERSION");
    if key.get_value::<String, _>("DisplayVersion").ok().as_deref() != Some(version) {
        let _ = key.set_value("DisplayVersion", &version);
        let _ = key.set_value("DisplayName", &"Strata");
    }
}
