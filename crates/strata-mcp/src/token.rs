//! Live-link session token: random, reused across restarts via the token
//! file so a browser tab only needs it once.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};

/// 128 random bits as 32 hex characters. `RandomState` is seeded from the
/// OS random source, which is plenty for a localhost pairing token.
pub fn generate() -> String {
    let mut out = String::with_capacity(32);
    for salt in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(salt);
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        h.write_u32(std::process::id());
        out.push_str(&format!("{:016x}", h.finish()));
    }
    out
}

fn valid(t: &str) -> bool {
    t.len() >= 16
        && t.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Decide the token: `explicit` wins, then the existing token file (unless
/// `fresh`), else a new random one. Writes it to `path` for native editors.
pub fn resolve(
    explicit: Option<String>,
    fresh: bool,
    path: Option<&Path>,
) -> Result<String, String> {
    let token = match explicit {
        Some(t) if !valid(t.trim()) => {
            return Err("--token must be at least 16 letters, digits, '-' or '_'".into())
        }
        Some(t) => t.trim().to_owned(),
        None => {
            let existing = if fresh {
                None
            } else {
                path.and_then(|p| std::fs::read_to_string(p).ok())
            };
            match existing.map(|t| t.trim().to_owned()).filter(|t| valid(t)) {
                Some(t) => t,
                None => generate(),
            }
        }
    };
    if let Some(p) = path {
        if let Err(e) = write(p, &token) {
            crate::log!("could not write the token file {}: {e}", p.display());
        }
    }
    Ok(token)
}

fn write(path: &Path, token: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, token)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

pub fn default_path() -> Option<PathBuf> {
    strata_core::ops::link::token_file_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_is_random_hex() {
        let (a, b) = (generate(), generate());
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn resolve_reuses_file() {
        let dir = std::env::temp_dir().join(format!("strata-mcp-test-{}", generate()));
        let p = dir.join("token");
        let t1 = resolve(None, false, Some(&p)).unwrap();
        assert_eq!(resolve(None, false, Some(&p)).unwrap(), t1);
        assert_ne!(resolve(None, true, Some(&p)).unwrap(), t1);
        assert!(resolve(Some("short".into()), false, None).is_err());
        assert_eq!(
            resolve(Some("my-stable-token-123".into()), false, Some(&p)).unwrap(),
            "my-stable-token-123"
        );
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "my-stable-token-123");
        let _ = std::fs::remove_dir_all(dir);
    }
}
