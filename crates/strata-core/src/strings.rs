//! Localized strings (GameStrings.txt and friends), keyed by locale.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const DEFAULT_LOCALE: &str = "enUS";
pub const LOCALES: [&str; 12] = [
    "enUS", "deDE", "esES", "esMX", "frFR", "itIT", "koKR", "plPL", "ptBR", "ruRU", "zhCN", "zhTW",
];

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
        self.locales
            .entry(locale.to_owned())
            .or_default()
            .insert(key.to_owned(), value.to_owned());
    }

    pub fn remove(&mut self, key: &str) {
        for m in self.locales.values_mut() {
            m.remove(key);
        }
    }

    /// Every key present in any locale, sorted.
    pub fn all_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self
            .locales
            .values()
            .flat_map(|m| m.keys().cloned())
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// Whether any locale has `key`.
    pub fn has_key(&self, key: &str) -> bool {
        self.locales.values().any(|m| m.contains_key(key))
    }

    /// Move `old` to `new` in every locale. Returns false (and changes
    /// nothing) if `old` is missing or `new` already exists.
    pub fn rename_key(&mut self, old: &str, new: &str) -> bool {
        if old == new || !self.has_key(old) || self.has_key(new) {
            return false;
        }
        for m in self.locales.values_mut() {
            if let Some(v) = m.remove(old) {
                m.insert(new.to_owned(), v);
            }
        }
        true
    }

    /// Keys that exist in some locale but have no value in `locale`.
    pub fn missing_in(&self, locale: &str) -> Vec<String> {
        let here = self.locales.get(locale);
        self.all_keys()
            .into_iter()
            .filter(|k| here.is_none_or(|m| !m.contains_key(k)))
            .collect()
    }

    /// Case-insensitive match of `query` against `key` or its value in any of `locales`.
    pub fn matches(&self, key: &str, query: &str, locales: &[String]) -> bool {
        let q = query.trim().to_lowercase();
        if q.is_empty() || key.to_lowercase().contains(&q) {
            return true;
        }
        locales.iter().any(|l| {
            self.get(l, key)
                .is_some_and(|v| v.to_lowercase().contains(&q))
        })
    }

    /// Plain-words description of what a well-known key is used for.
    pub fn reference_hint(key: &str) -> Option<String> {
        let fixed = match key {
            "DocInfo/Name" => Some("Map name"),
            "DocInfo/DescShort" => Some("Short map description"),
            "DocInfo/DescLong" => Some("Map description"),
            "DocInfo/Website" => Some("Map website"),
            "DocInfo/Author" => Some("Map author"),
            _ => None,
        };
        if let Some(f) = fixed {
            return Some(f.to_owned());
        }
        let prefixes = [
            ("Unit/Name/", "Name of unit"),
            ("Unit/Description/", "Description of unit"),
            ("Button/Name/", "Name of button"),
            ("Button/Tooltip/", "Tooltip of button"),
            ("Button/Hotkey/", "Hotkey of button"),
            ("Abil/Name/", "Name of ability"),
            ("Weapon/Name/", "Name of weapon"),
            ("Upgrade/Name/", "Name of upgrade"),
            ("Behavior/Name/", "Name of behavior"),
            ("Behavior/Tooltip/", "Tooltip of behavior"),
            ("Param/Value/", "Trigger text"),
            ("Trigger/Name/", "Name of trigger"),
        ];
        prefixes
            .iter()
            .find_map(|(p, what)| key.strip_prefix(p).map(|id| format!("{what} {id}")))
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

    #[test]
    fn missing_rename_and_search() {
        let mut s = Strings::default();
        s.set("enUS", "A", "Hello");
        s.set("enUS", "B", "World");
        s.set("deDE", "A", "Hallo");
        assert_eq!(s.missing_in("deDE"), ["B"]);
        assert_eq!(s.missing_in("frFR"), ["A", "B"]);
        assert!(s.rename_key("A", "C"));
        assert!(!s.rename_key("C", "B"));
        assert_eq!(s.get("deDE", "C"), Some("Hallo"));
        assert!(!s.has_key("A"));
        let locs = vec!["deDE".to_owned()];
        assert!(s.matches("C", "hal", &locs));
        assert!(!s.matches("B", "hal", &locs));
        assert_eq!(
            Strings::reference_hint("Unit/Name/Marine").as_deref(),
            Some("Name of unit Marine")
        );
        assert_eq!(
            Strings::reference_hint("DocInfo/Name").as_deref(),
            Some("Map name")
        );
        assert_eq!(Strings::reference_hint("Custom/Thing"), None);
    }
}
