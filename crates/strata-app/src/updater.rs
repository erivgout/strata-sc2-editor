//! Desktop self-update from GitHub Releases.
//!
//! On startup (and on Help → Check for updates) a background thread asks the
//! GitHub API for the latest release of `erivgout/strata-sc2-editor`. If its version is newer
//! than this build, the app asks the user. Installing downloads the Windows
//! executable asset, verifies it against the release's `.sha256` asset
//! (required: no checksum, no install) and stages it beside the app. The
//! running executable is never touched: on restart or quit the staged copy
//! finishes the update itself with one atomic replace (see [`finish_swap`]).
//!
//! Release asset naming (see `scripts/release.ps1`):
//! `strata-v{version}-windows-x64.exe` and `strata-v{version}-windows-x64.exe.sha256`.
//!
//! The web app updates through its service worker instead; this module is
//! native-only.

use std::path::{Path, PathBuf};
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
    /// The verified update was staged at the path; apply it with [`Updater::apply`].
    Installed(Result<(Release, PathBuf), String>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Idle,
    Checking,
    Downloading { version: String, progress: f32 },
    /// Downloaded, verified and staged; applied when Strata restarts or quits.
    Ready { version: String, staged: PathBuf },
}

pub struct Updater {
    pub status: Status,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    /// The finishing process was started (once per session).
    applied: bool,
    /// Held while an update is staged so nothing (e.g. another instance's
    /// clean-up) can delete or change it; see [`lock_staged`].
    lock: Option<std::fs::File>,
}

impl Default for Updater {
    fn default() -> Self {
        let (tx, rx) = channel();
        Self { status: Status::Idle, tx, rx, applied: false, lock: None }
    }
}

/// Open the staged update so that, while this session lives, it can be read
/// and executed but not deleted, renamed or written (Windows share mode
/// FILE_SHARE_READ only). Clean-up in other sessions therefore can't remove
/// it, and the finishing process can only move it once this session exits.
fn lock_staged(path: &Path) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x1;
        o.share_mode(FILE_SHARE_READ);
    }
    o.open(path)
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
        if self.busy() || matches!(self.status, Status::Ready { .. }) {
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

    /// Download, verify and stage `release` in the background.
    pub fn install(&mut self, ctx: &egui::Context, release: Release) {
        if self.busy() || matches!(self.status, Status::Ready { .. }) {
            return;
        }
        self.status = Status::Downloading { version: release.version_text(), progress: 0.0 };
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = download_and_stage(&release, &tx, &ctx).map(|staged| (release, staged));
            let _ = tx.send(Event::Installed(result));
            ctx.request_repaint();
        });
    }

    /// Drain one event, updating [`Updater::status`].
    pub fn poll(&mut self) -> Option<Event> {
        let mut ev = self.rx.try_recv().ok()?;
        if let Event::Installed(Ok((_, staged))) = &ev {
            match lock_staged(staged) {
                Ok(f) => self.lock = Some(f),
                Err(e) => {
                    let _ = std::fs::remove_file(staged);
                    ev = Event::Installed(Err(format!("the downloaded update couldn't be kept ({e})")));
                }
            }
        }
        match &ev {
            Event::Checked { .. } => self.status = Status::Idle,
            Event::Progress(p) => {
                if let Status::Downloading { progress, .. } = &mut self.status {
                    *progress = *p;
                }
            }
            Event::Installed(Ok((r, staged))) => {
                self.status = Status::Ready { version: r.version_text(), staged: staged.clone() }
            }
            Event::Installed(Err(_)) => self.status = Status::Idle,
        }
        Some(ev)
    }

    /// Start the staged update in "finish" mode: it waits for this process
    /// to exit, then atomically replaces Strata's executable with itself and,
    /// if `relaunch`, starts it. Does nothing unless an update is staged, and
    /// only once per session. Call right before closing.
    pub fn apply(&mut self, relaunch: bool) -> Result<(), String> {
        let Status::Ready { staged, .. } = &self.status else { return Ok(()) };
        if self.applied {
            return Ok(());
        }
        let exe = std::env::current_exe().map_err(|e| format!("can't find Strata's own file ({e})"))?;
        let mut cmd = std::process::Command::new(staged);
        cmd.arg(FINISH_FLAG).arg(&exe);
        if relaunch {
            cmd.arg(RELAUNCH_FLAG);
        }
        cmd.spawn().map_err(|e| format!("couldn't start the update ({e})"))?;
        self.applied = true;
        Ok(())
    }
}

const FINISH_FLAG: &str = "--strata-finish-update";
const RELAUNCH_FLAG: &str = "--relaunch";

/// Entry point for the finishing process (see [`Updater::apply`]). Returns
/// true when the arguments asked for it, in which case `main` must exit
/// without starting the UI.
pub fn finish_update_from_args() -> bool {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if args.get(1).is_none_or(|a| a != FINISH_FLAG) {
        return false;
    }
    let (Some(target), Ok(me)) = (args.get(2), std::env::current_exe()) else { return true };
    let target = PathBuf::from(target);
    // The old Strata is closing; its file can't be replaced until it has.
    // If it never frees up (another window stays open), the update stays
    // staged for next time, and a requested restart still happens.
    let _ = finish_swap(&me, &target, 240, std::time::Duration::from_millis(500));
    if args.get(3).is_some_and(|a| a == RELAUNCH_FLAG) {
        let _ = std::process::Command::new(&target).spawn();
    }
    true
}

/// Move `staged` onto `target`, retrying while `target` is still in use.
///
/// A same-directory rename that replaces an existing file is atomic on
/// Windows (MoveFileEx with MOVEFILE_REPLACE_EXISTING), so at every instant
/// `target` is either the complete old executable or the complete new one;
/// an interruption can never leave it missing. A running executable can't
/// be replaced, so this only succeeds once the old Strata has exited.
fn finish_swap(staged: &Path, target: &Path, tries: u32, wait: std::time::Duration) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..tries.max(1) {
        match std::fs::rename(staged, target) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        std::thread::sleep(wait);
    }
    Err(last)
}

fn download_and_stage(release: &Release, tx: &Sender<Event>, ctx: &egui::Context) -> Result<PathBuf, String> {
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
    let staged = exe.with_file_name(format!(".strata-update-{}-{}.exe", release.version_text(), std::process::id()));
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
    Ok(staged)
}

/// Delete staged updates that were never applied (older than a day). A
/// session that still has one staged holds it open via [`lock_staged`], so
/// deleting it fails and it is left alone.
pub fn clean_up_old() {
    let Ok(me) = std::env::current_exe() else { return };
    let Some(dir) = me.parent() else { return };
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let day = std::time::Duration::from_secs(24 * 3600);
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let stale = e
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > day);
        if name.starts_with(".strata-update-") && name.ends_with(".exe") && e.path() != me && stale {
            let _ = std::fs::remove_file(e.path());
        }
    }
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
    fn finish_swap_replaces_atomically_or_leaves_target_intact() {
        let dir = std::env::temp_dir().join(format!("strata-swap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (target, staged) = (dir.join("strata.exe"), dir.join(".strata-update-x.exe"));
        let quick = std::time::Duration::from_millis(1);

        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&staged, b"new").unwrap();
        finish_swap(&staged, &target, 3, quick).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!staged.exists());

        // A failed swap (nothing staged) leaves the executable untouched.
        assert!(finish_swap(&staged, &target, 2, quick).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn staged_update_is_protected_while_locked() {
        let path = std::env::temp_dir().join(format!(".strata-update-locktest-{}.exe", std::process::id()));
        std::fs::write(&path, b"staged").unwrap();
        let lock = lock_staged(&path).unwrap();
        assert!(std::fs::remove_file(&path).is_err(), "a live session's staged update must survive clean-up");
        assert!(std::fs::rename(&path, path.with_extension("moved")).is_err());
        drop(lock);
        std::fs::remove_file(&path).unwrap();
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
