//! Localized strings (GameStrings.txt and friends), keyed by locale.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const DEFAULT_LOCALE: &str = "enUS";
pub const LOCALES: [&str; 12] =
    ["enUS", "deDE", "esES", "esMX", "frFR", "itIT", "koKR", "plPL", "ptBR", "ruRU", "zhCN", "zhTW"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Strings {
    /// locale -> key -> value
    pub locales: BTreeMap<String, BTreeMap<String, String>>,
}

impl Strings {
    pub fn get(&self, locale: &str, key: &str) -> Option<&str> {
        self.locales.get(locale)?.get(key).map(String::as_str)
    }

    pub fn set(&mut self, locale: &str, key: &str, value: &str) {
        self.locales.entry(locale.to_owned()).or_default().insert(key.to_owned(), value.to_owned());
    }

    pub fn remove(&mut self, key: &str) {
        for m in self.locales.values_mut() {
            m.remove(key);
        }
    }

    /// Every key present in any locale, sorted.
    pub fn all_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.locales.values().flat_map(|m| m.keys().cloned()).collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Parse `Key=Value` lines (GameStrings.txt). Lines without `=` are skipped.
    pub fn parse_txt(text: &str) -> BTreeMap<String, String> {
        text.trim_start_matches('\u{feff}')
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.to_owned(), v.trim_end_matches('\r').to_owned()))
            .collect()
    }

    pub fn to_txt(map: &BTreeMap<String, String>) -> String {
        let mut s = String::new();
        for (k, v) in map {
            s.push_str(k);
            s.push('=');
            s.push_str(v);
            s.push_str("\r\n");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txt_roundtrip() {
        let src = "\u{feff}A/B=hello = world\r\nC=\r\n";
        let m = Strings::parse_txt(src);
        assert_eq!(m["A/B"], "hello = world");
        assert_eq!(m["C"], "");
        assert_eq!(Strings::parse_txt(&Strings::to_txt(&m)), m);
    }
}
