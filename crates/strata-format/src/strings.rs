//! `<locale>.SC2Data/LocalizedData/GameStrings.txt`: `Key=Value` lines,
//! UTF-8 with BOM and CRLF in editor output.
//!
//! Regeneration keeps the original line order, untouched lines byte-for-byte,
//! and non `Key=Value` lines; removed keys are dropped and new keys appended
//! in sorted order.

use std::collections::{BTreeMap, BTreeSet};

pub fn path(locale: &str) -> String {
    format!("{locale}.SC2Data/LocalizedData/GameStrings.txt")
}

/// Locale of a GameStrings path, if it is one.
pub fn locale_of(path: &str) -> Option<&str> {
    let (loc, rest) = path.split_once(".SC2Data/")?;
    if rest.eq_ignore_ascii_case("LocalizedData/GameStrings.txt") && !loc.contains('/') {
        Some(loc)
    } else {
        None
    }
}

fn decode(bytes: &[u8]) -> (bool, String) {
    match bytes.strip_prefix(b"\xEF\xBB\xBF") {
        Some(rest) => (true, String::from_utf8_lossy(rest).into_owned()),
        None => (false, String::from_utf8_lossy(bytes).into_owned()),
    }
}

fn split_line(line: &str) -> Option<(&str, &str)> {
    let l = line.strip_suffix('\r').unwrap_or(line);
    let (k, v) = l.split_once('=')?;
    if k.is_empty() { None } else { Some((k, v)) }
}

pub fn parse(bytes: &[u8]) -> BTreeMap<String, String> {
    let (_, text) = decode(bytes);
    text.split('\n').filter_map(split_line).map(|(k, v)| (k.to_owned(), v.to_owned())).collect()
}

pub fn write(orig: Option<&[u8]>, map: &BTreeMap<String, String>) -> Vec<u8> {
    let (bom, text) = match orig {
        Some(b) => decode(b),
        None => (true, String::new()),
    };
    let nl = if text.contains("\r\n") || !text.contains('\n') { "\r\n" } else { "\n" };
    let mut out = String::with_capacity(text.len() + 64);
    let mut done: BTreeSet<&str> = BTreeSet::new();
    let ends_with_nl = text.is_empty() || text.ends_with('\n');
    let mut lines: Vec<&str> = text.split('\n').collect();
    if ends_with_nl {
        lines.pop();
    }
    let mut first = true;
    let mut push = |out: &mut String, s: &str| {
        if !first {
            out.push('\n');
        }
        first = false;
        out.push_str(s);
    };
    for line in &lines {
        match split_line(line) {
            Some((k, v)) => {
                if done.contains(k) {
                    continue;
                }
                if let Some(cur) = map.get(k) {
                    done.insert(k);
                    if cur == v {
                        push(&mut out, line);
                    } else {
                        let cr = if line.ends_with('\r') { "\r" } else { "" };
                        push(&mut out, &format!("{k}={cur}{cr}"));
                    }
                }
            }
            None => push(&mut out, line),
        }
    }
    let cr = if nl == "\r\n" { "\r" } else { "" };
    for (k, v) in map {
        if !done.contains(k.as_str()) {
            push(&mut out, &format!("{k}={v}{cr}"));
        }
    }
    if ends_with_nl && !out.is_empty() {
        out.push('\n');
    }
    crate::xml::finish(bom, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_preserves_order_and_bom() {
        let src = "\u{feff}Z/Key=last\r\nA/Key=first = x\r\n// note\r\n".as_bytes();
        let m = parse(src);
        assert_eq!(m["A/Key"], "first = x");
        assert_eq!(write(Some(src), &m), src);
        let mut m2 = m.clone();
        m2.insert("A/Key".into(), "changed".into());
        m2.remove("Z/Key");
        m2.insert("B/New".into(), "n".into());
        let out = String::from_utf8(write(Some(src), &m2)).unwrap();
        assert_eq!(out, "\u{feff}A/Key=changed\r\n// note\r\nB/New=n\r\n");
    }

    #[test]
    fn new_file_has_bom_and_crlf() {
        let mut m = BTreeMap::new();
        m.insert("DocInfo/Name".to_owned(), "My Map".to_owned());
        assert_eq!(write(None, &m), "\u{feff}DocInfo/Name=My Map\r\n".as_bytes());
        assert_eq!(locale_of("enUS.SC2Data/LocalizedData/GameStrings.txt"), Some("enUS"));
        assert_eq!(locale_of("enUS.SC2Data/LocalizedData/ObjectStrings.txt"), None);
    }
}
