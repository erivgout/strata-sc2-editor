//! `Base.SC2Data/GameData/*.xml` catalogs.
//!
//! Every top-level element of `<Catalog>` becomes a [`CatalogEntry`]
//! (including `<const>` and similar), in file order, with attribute order
//! preserved. When a file is regenerated, entries that did not change are
//! copied from the original text verbatim (with the comments and whitespace
//! in front of them); only changed or new entries are re-serialized.

use std::collections::HashMap;

use strata_core::catalog::{CatalogEntry, XmlNode};

use crate::xml::{self, INDENT};

pub const DIR: &str = "Base.SC2Data/GameData/";

/// "Base.SC2Data/GameData/UnitData.xml" -> "UnitData".
pub fn file_of(path: &str) -> Option<&str> {
    if path.len() <= DIR.len() || !path[..DIR.len()].eq_ignore_ascii_case(DIR) {
        return None;
    }
    let name = &path[DIR.len()..];
    if name.contains('/') || name.len() <= 4 || !name[name.len() - 4..].eq_ignore_ascii_case(".xml") {
        return None;
    }
    Some(&name[..name.len() - 4])
}

pub fn path_of(file: &str) -> String {
    format!("{DIR}{file}.xml")
}

/// Parse one catalog file. Returns `None` if it is not a `<Catalog>` document.
pub fn parse(file: &str, bytes: &[u8]) -> Option<Vec<CatalogEntry>> {
    let doc = xml::parse_bytes(bytes).ok()?;
    if doc.root.name != "Catalog" {
        return None;
    }
    Some(doc.root.children.iter().map(|c| CatalogEntry { file: file.to_owned(), node: c.to_node() }).collect())
}

fn key(n: &XmlNode) -> (String, String) {
    (n.name.clone(), n.attr("id").unwrap_or("").to_owned())
}

pub fn write(orig: Option<&[u8]>, entries: &[&CatalogEntry]) -> Vec<u8> {
    let doc = orig.and_then(|b| xml::parse_bytes(b).ok()).filter(|d| d.root.name == "Catalog");
    let Some(doc) = doc else {
        let mut root = XmlNode::new("Catalog");
        root.children = entries.iter().map(|e| e.node.clone()).collect();
        return xml::document(&root, "\r\n", true).into_bytes();
    };
    let nl = doc.newline();

    // Pair originals and current entries by (class, id, occurrence).
    let mut orig_by_key: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (i, c) in doc.root.children.iter().enumerate() {
        orig_by_key.entry((c.name.clone(), c.attr("id").unwrap_or("").to_owned())).or_default().push(i);
    }
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    let mut mapped: Vec<Option<usize>> = vec![None; doc.root.children.len()];
    let mut fresh: Vec<&XmlNode> = Vec::new();
    for (j, e) in entries.iter().enumerate() {
        let k = key(&e.node);
        let n = seen.entry(k.clone()).or_insert(0);
        match orig_by_key.get(&k).and_then(|v| v.get(*n)) {
            Some(&i) => mapped[i] = Some(j),
            None => fresh.push(&e.node),
        }
        *n += 1;
    }

    let mut body = String::new();
    let mut cursor = doc.root.inner.start;
    for (i, c) in doc.root.children.iter().enumerate() {
        let trivia = &doc.src[cursor..c.span.start];
        cursor = c.span.end;
        match mapped[i] {
            Some(j) => {
                body.push_str(trivia);
                let cur = &entries[j].node;
                if *cur == c.to_node() {
                    body.push_str(doc.raw(&c.span));
                } else {
                    xml::write_node(&mut body, cur, 1, nl);
                }
            }
            None => {
                // Deleted entry: keep any comments that preceded it.
                if trivia.contains("<!--") {
                    body.push_str(trivia.trim_end());
                }
            }
        }
    }
    let trailing = &doc.src[cursor..doc.root.inner.end];
    for n in fresh {
        body.push_str(nl);
        body.push_str(INDENT);
        xml::write_node(&mut body, n, 1, nl);
    }
    let mut out = String::with_capacity(doc.src.len() + body.len());
    if doc.root.self_closing {
        out.push_str(&doc.src[..doc.root.span.start]);
        out.push_str("<Catalog>");
        out.push_str(&body);
        out.push_str(nl);
        out.push_str("</Catalog>");
        out.push_str(&doc.src[doc.root.span.end..]);
    } else {
        out.push_str(&doc.src[..doc.root.inner.start]);
        out.push_str(&body);
        out.push_str(if trailing.is_empty() && !body.is_empty() { nl } else { trailing });
        out.push_str(&doc.src[doc.root.inner.end..]);
    }
    doc.finish(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<Catalog>\r\n    <!-- marine -->\r\n    <CUnit id=\"Marine\" parent=\"X\">\r\n        <LifeMax value=\"45\"/>\r\n    </CUnit>\r\n    <CUnit id=\"Zealot\"/>\r\n</Catalog>\r\n";

    #[test]
    fn parse_and_identity_write() {
        let es = parse("UnitData", SRC.as_bytes()).unwrap();
        assert_eq!(es.len(), 2);
        assert_eq!(es[0].id(), "Marine");
        assert_eq!(es[0].field("LifeMax"), Some("45"));
        assert_eq!(es[0].node.attrs[1], ("parent".to_owned(), "X".to_owned()));
        let refs: Vec<&CatalogEntry> = es.iter().collect();
        assert_eq!(write(Some(SRC.as_bytes()), &refs), SRC.as_bytes());
    }

    #[test]
    fn edit_delete_add() {
        let mut es = parse("UnitData", SRC.as_bytes()).unwrap();
        es[0].set_field("LifeMax", "55");
        es.remove(1);
        let mut n = XmlNode::new("CUnit");
        n.set_attr("id", "Hydra");
        es.push(CatalogEntry { file: "UnitData".into(), node: n });
        let refs: Vec<&CatalogEntry> = es.iter().collect();
        let out = String::from_utf8(write(Some(SRC.as_bytes()), &refs)).unwrap();
        assert_eq!(
            out,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\r\n<Catalog>\r\n    <!-- marine -->\r\n    <CUnit id=\"Marine\" parent=\"X\">\r\n        <LifeMax value=\"55\"/>\r\n    </CUnit>\r\n    <CUnit id=\"Hydra\"/>\r\n</Catalog>\r\n"
        );
        assert_eq!(parse("UnitData", out.as_bytes()).unwrap(), es);
    }

    #[test]
    fn paths() {
        assert_eq!(file_of("Base.SC2Data/GameData/UnitData.xml"), Some("UnitData"));
        assert_eq!(file_of("Base.SC2Data/GameData/Sub/UnitData.xml"), None);
        assert_eq!(path_of("AbilData"), "Base.SC2Data/GameData/AbilData.xml");
    }
}
