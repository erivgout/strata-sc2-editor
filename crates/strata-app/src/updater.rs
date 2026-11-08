//! Desktop self-update from GitHub Releases.
//!
//! On startup (and on Help → Check for updates) a background thread asks the
//! GitHub API for the latest release of `erivgout/strata-sc2-editor`. If its version is newer
//! than this build, the app asks the user. Installing downloads the Windows
//! executable asset, verifies it against the release's `.sha256` asset
//! (required: no checksum, no install), swaps it in for the running
//! executable (with rollback, see [`swap_in`]), and offers to restart.
//!
//! Release asset naming (see `scripts/release.ps1`):
//! `strata-v{version}-windows-x64.exe` and `strata-v{version}-windows-x64.exe.sha256`.
//!
//! The web app updates through its service worker instead; this module is
//! native-only.

use std::sync::mpsc::{channel, Receiver, Sender};

const API_LATEST: &str = "https://api.github.com/repos/erivgout/strata-sc2-editor/releases/latest";
/// Refuse absurd downloads (the editor is ~15 MB).
const MAX_DOWNLOAD: u64 = 256 << 20;

/// `major.minor.patch`, compared numerically.
pub type Version = (u64, u64, u64);

/// Parse "v1.2.3" / "1.2.3" (pre-release or build suffixes are ignored).
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    if it.next().is_some() {
        return None;
    }
    Some(v)
}

/// This build's version. `STRATA_PRETEND_VERSION` overrides it so the update
/// flow can be exercised against a real release.
pub fn current_version() -> Version {
    std::env::var("STRATA_PRETEND_VERSION")
        .ok()
        .and_then(|v| parse_version(&v))
        .or_else(|| parse_version(env!("CARGO_PKG_VERSION")))
        .unwrap_or((0, 0, 0))
}

/// A newer release the user can install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub exe_url: String,
    pub sha_url: String,
    pub page_url: String,
}

impl Release {
    pub fn version_text(&self) -> String {
        let (a, b, c) = self.version;
        format!("{a}.{b}.{c}")
    }
}

fn asset_name(v: Version) -> String {
    format!("strata-v{}.{}.{}-windows-x64.exe", v.0, v.1, v.2)
}

/// Read the GitHub "latest release" JSON. `Ok(None)` when it isn't newer
/// than `current`, has no Windows build, or is a draft/pre-release.
pub fn pick_release(json: &serde_json::Value, current: Version) -> Result<Option<Release>, String> {
    let tag = json["tag_name"].as_str().ok_or("the release has no tag")?;
    if json["draft"].as_bool() == Some(true) || json["prerelease"].as_bool() == Some(true) {
        return Ok(None);
    }
    let version = parse_version(tag).ok_or_else(|| format!("can't read the version in tag {tag:?}"))?;
    if version <= current {
        return Ok(None);
    }
    let want = asset_name(version);
    let assets = json["assets"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"].as_str().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_owned)
    };
    let (Some(exe_url), Some(sha_url)) = (url_of(&want), url_of(&format!("{want}.sha256"))) else {
        return Ok(None);
    };
    Ok(Some(Release {
        tag: tag.to_owned(),
        version,
        exe_url,
        sha_url,
        page_url: json["html_url"].as_str().unwrap_or_default().to_owned(),
    }))
}

/// The hex digest from a `.sha256` file (`"<hex>"` or `"<hex>  name"`).
pub fn parse_sha256(text: &str) -> Option<[u8; 32]> {
    let hex = text.split_whitespace().next()?;
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    sha2::Sha256::digest(bytes).into()
}

/// What the background thread reports.
pub enum Event {
    /// `manual`: the user asked, so "up to date" and errors are worth showing.
    Checked { result: Result<Option<Release>, String>, manual: bool },
    Progress(f32),
    Installed(Result<Release, String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    Downloading { version: String, progress: f32 },
    /// Installed; restart to run it.
    Ready(String),
}

pub struct Updater {
    pub status: Status,
    tx: Sender<Event>,
    rx: Receiver<Event>,
}

impl Default for Updater {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { status: Status::Idle, tx, rx }
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .user_agent(concat!("Strata/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

fn get(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    agent
        .get(url)
        .call()
        .map_err(|e| format!("couldn't reach GitHub ({e})"))?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| format!("download failed ({e})"))
}

impl Updater {
    pub fn busy(&self) -> bool {
        matches!(self.status, Status::Checking | Status::Downloading { .. })
    }

    /// Ask GitHub for the latest release in the background.
    pub fn check(&mut self, ctx: &egui::Context, manual: bool) {
        if self.busy() || matches!(self.status, Status::Ready(_)) {
            return;
        }
        self.status = Status::Checking;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = get(&agent(), API_LATEST, 4 << 20)
                .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).map_err(|e| e.to_string()))
                .and_then(|json| pick_release(&json, current_version()));
            let _ = tx.send(Event::Checked { result, manual });
            ctx.request_repaint();
        });
    }

    /// Download, verify and install `release` in the background.
    pub fn install(&mut self, ctx: &egui::Context, release: Release) {
        if self.busy() {
            return;
        }
        self.status = Status::Downloading { version: release.version_text(), progress: 0.0 };
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = download_and_replace(&release, &tx, &ctx).map(|()| release);
            let _ = tx.send(Event::Installed(result));
            ctx.request_repaint();
        });
    }

    /// Drain one event, updating [`Updater::status`].
    pub fn poll(&mut self) -> Option<Event> {
        let ev = self.rx.try_recv().ok()?;
        match &ev {
            Event::Checked { .. } => self.status = Status::Idle,
            Event::Progress(p) => {
                if let Status::Downloading { progress, .. } = &mut self.status {
                    *progress = *p;
                }
            }
            Event::Installed(Ok(r)) => self.status = Status::Ready(r.version_text()),
            Event::Installed(Err(_)) => self.status = Status::Idle,
        }
        Some(ev)
    }
}

fn download_and_replace(release: &Release, tx: &Sender<Event>, ctx: &egui::Context) -> Result<(), String> {
    let agent = agent();
    let sha_text = get(&agent, &release.sha_url, 4096)?;
    let expected = parse_sha256(&String::from_utf8_lossy(&sha_text))
        .ok_or("the release's checksum file is malformed, so Strata won't install it")?;
    let _ = tx.send(Event::Progress(0.05));
    ctx.request_repaint();

    let mut resp = agent.get(&release.exe_url).call().map_err(|e| format!("couldn't download the update ({e})"))?;
    let total = resp.body().content_length().unwrap_or(0);
    if total > MAX_DOWNLOAD {
        return Err("the update is unexpectedly large, so Strata won't install it".into());
    }
    let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();
    let mut bytes = Vec::with_capacity(total as usize);
    let mut buf = vec![0u8; 256 << 10];
    loop {
        let n = std::io::Read::read(&mut reader, &mut buf).map_err(|e| format!("download failed ({e})"))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if total > 0 {
            let _ = tx.send(Event::Progress(0.05 + 0.9 * bytes.len() as f32 / total as f32));
            ctx.request_repaint();
        }
    }
    if sha256(&bytes) != expected {
        return Err("the download doesn't match its published checksum, so Strata didn't install it".into());
    }

    let exe = std::env::current_exe().map_err(|e| format!("can't find Strata's own file ({e})"))?;
    let tag = format!("{}-{}", release.version_text(), std::process::id());
    let staged = exe.with_file_name(format!(".strata-update-{tag}.exe"));
    let backup = exe.with_file_name(format!(".strata-old-{tag}.exe"));
    // `create_new`: never reuse a file another instance (or anyone) made.
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&staged)
        .and_then(|mut f| {
            std::io::Write::write_all(&mut f, &bytes)?;
            f.sync_all()
        });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&staged);
        return Err(format!("can't write the update next to Strata ({e})"));
    }
    let swapped = swap_in(&exe, &staged, &backup);
    let _ = std::fs::remove_file(&staged);
    swapped
}

/// Put `staged` at `exe`'s path, keeping the old file at `backup`.
///
/// Windows lets a running executable be renamed (not overwritten), so:
/// rename `exe` -> `backup`, then `staged` -> `exe`. If the second step
/// fails the first is undone, so `exe` always exists afterwards. The backup
/// of a successful swap is deleted on the next start ([`clean_up_old`]).
fn swap_in(exe: &std::path::Path, staged: &std::path::Path, backup: &std::path::Path) -> Result<(), String> {
    std::fs::rename(exe, backup).map_err(|e| format!("can't replace Strata's file ({e})"))?;
    if let Err(e) = std::fs::rename(staged, exe) {
        return match std::fs::rename(backup, exe) {
            Ok(()) => Err(format!("can't put the update in place ({e})")),
            Err(e2) => Err(format!(
                "can't put the update in place ({e}), and restoring the old file failed ({e2}); it is at {}",
                backup.display()
            )),
        };
    }
    Ok(())
}

/// Delete backups left by earlier updates (the running exe is the new one).
pub fn clean_up_old() {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".strata-old-") && name.ends_with(".exe") {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Start the (now updated) executable and let the caller close this one.
pub fn relaunch() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe).spawn().map(|_| ()).map_err(|e| format!("couldn't restart Strata ({e})"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn release_json(tag: &str, names: &[&str]) -> serde_json::Value {
        json!({
            "tag_name": tag,
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/erivgout/strata-sc2-editor/releases/tag/x",
            "assets": names.iter().map(|n| json!({"name": n, "browser_download_url": format!("https://dl/{n}")})).collect::<Vec<_>>(),
        })
    }

    #[test]
    fn versions_parse_and_order() {
        assert_eq!(parse_version("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse_version("0.10.0-beta.1"), Some((0, 10, 0)));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(parse_version("0.10.0") > parse_version("0.9.9"));
    }

    #[test]
    fn picks_only_newer_releases_with_exe_and_checksum() {
        let names = ["strata-v0.2.0-windows-x64.exe", "strata-v0.2.0-windows-x64.exe.sha256", "strata-mcp-v0.2.0-windows-x64.exe"];
        let r = pick_release(&release_json("v0.2.0", &names), (0, 1, 0)).unwrap().unwrap();
        assert_eq!(r.version, (0, 2, 0));
        assert_eq!(r.exe_url, "https://dl/strata-v0.2.0-windows-x64.exe");
        assert_eq!(r.sha_url, "https://dl/strata-v0.2.0-windows-x64.exe.sha256");
        // Same or older: nothing to do.
        assert_eq!(pick_release(&release_json("v0.2.0", &names), (0, 2, 0)).unwrap(), None);
        // No checksum asset: never offered.
        assert_eq!(pick_release(&release_json("v0.2.0", &names[..1]), (0, 1, 0)).unwrap(), None);
        // Pre-releases are skipped.
        let mut pre = release_json("v0.3.0", &names);
        pre["prerelease"] = json!(true);
        assert_eq!(pick_release(&pre, (0, 1, 0)).unwrap(), None);
    }

    #[test]
    fn swap_replaces_and_rolls_back() {
        let dir = std::env::temp_dir().join(format!("strata-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (exe, staged, backup) = (dir.join("strata.exe"), dir.join("new.exe"), dir.join("old.exe"));

        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(&staged, b"new").unwrap();
        swap_in(&exe, &staged, &backup).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");

        // Failure after the first rename (the staged file is gone): the
        // original executable must be back in place.
        std::fs::remove_file(&backup).unwrap();
        std::fs::write(&exe, b"old").unwrap();
        assert!(swap_in(&exe, &dir.join("missing.exe"), &backup).is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
        assert!(!backup.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn checksum_files_parse_and_match() {
        let digest = sha256(b"strata");
        let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
        assert_eq!(parse_sha256(&format!("{hex}  strata.exe\n")), Some(digest));
        assert_eq!(parse_sha256(&hex.to_uppercase()), Some(digest));
        assert_eq!(parse_sha256("abc"), None);
        assert_eq!(parse_sha256(""), None);
    }
}
