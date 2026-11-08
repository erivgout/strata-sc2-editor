//! `.build.info` (the install's table of builds) and the build config it
//! points at.

use crate::{CascError, Key};

/// One row of `.build.info`: a pipe-separated table whose header names its
/// columns `Name!TYPE:size`.
#[derive(Debug, Clone)]
pub struct BuildInfo {
    pub build_key: Key,
    pub version: String,
    pub product: String,
}

impl BuildInfo {
    /// Parses `.build.info`, returning the active row (or the first row if no
    /// row is marked active).
    pub fn parse(text: &str) -> Result<BuildInfo, CascError> {
        let mut lines = text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#'));
        let header: Vec<String> = lines
            .next()
            .ok_or_else(|| CascError::Format(".build.info is empty".into()))?
            .split('|')
            .map(|c| c.split('!').next().unwrap_or("").trim().to_ascii_lowercase())
            .collect();
        let col = |name: &str| header.iter().position(|h| h == name);
        let key_col = col("build key").ok_or_else(|| CascError::Format(".build.info has no Build Key".into()))?;
        let rows: Vec<Vec<&str>> = lines.map(|l| l.split('|').collect()).collect();
        let active = col("active");
        let row = rows
            .iter()
            .find(|r| active.and_then(|a| r.get(a)).is_some_and(|v| v.trim() == "1"))
            .or_else(|| rows.first())
            .ok_or_else(|| CascError::Format(".build.info has no builds".into()))?;
        let get = |c: Option<usize>| c.and_then(|c| row.get(c)).map(|s| s.trim().to_string()).unwrap_or_default();
        Ok(BuildInfo {
            build_key: Key::from_hex(&get(Some(key_col)))?,
            version: get(col("version")),
            product: get(col("product")),
        })
    }
}

/// The parts of a build config (`config/xx/yy/<build key>`) the reader needs.
#[derive(Debug, Clone)]
pub struct BuildConfig {
    /// Content key of the root file (MNDX for StarCraft II).
    pub root_ckey: Key,
    /// Encoded key of the encoding file (the second value of `encoding =`).
    pub encoding_ekey: Key,
    pub build_name: String,
}

impl BuildConfig {
    pub fn parse(text: &str) -> Result<BuildConfig, CascError> {
        let mut root = None;
        let mut encoding = None;
        let mut build_name = String::new();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let vals: Vec<&str> = v.split_whitespace().collect();
            match k.trim() {
                "root" => root = vals.first().map(|s| Key::from_hex(s)).transpose()?,
                "encoding" => encoding = vals.get(1).map(|s| Key::from_hex(s)).transpose()?,
                "build-name" => build_name = v.trim().to_string(),
                _ => {}
            }
        }
        Ok(BuildConfig {
            root_ckey: root.ok_or_else(|| CascError::Format("build config has no root".into()))?,
            encoding_ekey: encoding.ok_or_else(|| CascError::Format("build config has no encoding key".into()))?,
            build_name,
        })
    }
}

/// `config/ab/cd/abcd...` for a config key.
pub fn config_path(data_dir: &str, key: &Key) -> String {
    let hex = key.to_hex();
    format!("{data_dir}/config/{}/{}/{hex}", &hex[0..2], &hex[2..4])
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD_INFO: &str = "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|CDN Key!HEX:16|Version!STRING:0|Product!STRING:0\n\
        eu|0|00000000000000000000000000000000|11111111111111111111111111111111|1.0||\n\
        us|1|1d231af579faf03e61a364722a3b87b4|e4f2e2da310c08495f2701d401d14659|5.0.17.98310||\n";

    #[test]
    fn picks_active_row() {
        let info = BuildInfo::parse(BUILD_INFO).unwrap();
        assert_eq!(info.build_key.to_hex(), "1d231af579faf03e61a364722a3b87b4");
        assert_eq!(info.version, "5.0.17.98310");
    }

    #[test]
    fn build_config_keys() {
        let cfg = BuildConfig::parse(
            "# Build Configuration\n\nroot = 9c1f3d3d30ca252e33fc67ca05cbe1ea\n\
             encoding = 9eb1125aa40d8764a0bfbcae5ea646d3 58a909228b3339316bf5ecfd9a842852\nbuild-name = B98310\n",
        )
        .unwrap();
        assert_eq!(cfg.root_ckey.to_hex(), "9c1f3d3d30ca252e33fc67ca05cbe1ea");
        assert_eq!(cfg.encoding_ekey.to_hex(), "58a909228b3339316bf5ecfd9a842852");
        assert_eq!(cfg.build_name, "B98310");
        assert_eq!(config_path("SC2Data", &cfg.root_ckey), "SC2Data/config/9c/1f/9c1f3d3d30ca252e33fc67ca05cbe1ea");
    }
}
