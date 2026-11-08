//! `DocumentInfo` (XML, dependencies) and `ComponentList.SC2Components`.
//!
//! Both are edited by splicing so unrelated content (icons, screenshots,
//! component entries Strata does not know) is preserved exactly.

use crate::xml::{self, Doc, Edits, INDENT};

pub fn parse_dependencies(doc: &Doc) -> Vec<String> {
    doc.root
        .child("Dependencies")
        .map(|d| d.children_named("Value").map(|v| v.text.clone()).collect())
        .unwrap_or_default()
}

fn deps_block(deps: &[String], nl: &str) -> String {
    let mut s = String::from("<Dependencies>");
    for d in deps {
        s.push_str(nl);
        s.push_str(&INDENT.repeat(2));
        s.push_str("<Value>");
        s.push_str(&xml::esc_text(d));
        s.push_str("</Value>");
    }
    s.push_str(nl);
    s.push_str(INDENT);
    s.push_str("</Dependencies>");
    s
}

/// Replace the dependency list in an existing DocumentInfo.
pub fn patch_dependencies(doc: &Doc, deps: &[String]) -> Vec<u8> {
    let nl = doc.newline();
    let mut ed = Edits::default();
    match doc.root.child("Dependencies") {
        Some(d) => ed.replace(d.span.clone(), deps_block(deps, nl)),
        None => {
            let at = doc.root.children.last().map(|c| c.span.end).unwrap_or(doc.root.inner.start);
            ed.insert(at, format!("{nl}{INDENT}{}", deps_block(deps, nl)));
        }
    }
    doc.finish(ed.apply(&doc.src))
}

/// DocumentInfo as the editor writes it for a new map.
pub fn new_document_info(deps: &[String]) -> Vec<u8> {
    let nl = "\r\n";
    let mut s = String::from(xml::DECL);
    s.push_str(nl);
    s.push_str("<DocInfo>");
    s.push_str(nl);
    s.push_str(&format!("{INDENT}<ModType>{nl}{INDENT}{INDENT}<Value>Interface</Value>{nl}{INDENT}</ModType>{nl}"));
    s.push_str(INDENT);
    s.push_str(&deps_block(deps, nl));
    s.push_str(nl);
    s.push_str("</DocInfo>");
    s.push_str(nl);
    s.into_bytes()
}

/// (type code, locale, logical path)
pub type Component = (String, Option<String>, String);

pub fn parse_components(doc: &Doc) -> Vec<Component> {
    doc.root
        .children_named("DataComponent")
        .map(|c| {
            (c.attr("Type").unwrap_or("").to_owned(), c.attr("Locale").map(str::to_owned), c.text.clone())
        })
        .collect()
}

fn component_line(c: &Component) -> String {
    match &c.1 {
        Some(l) => format!(
            "<DataComponent Type=\"{}\" Locale=\"{}\">{}</DataComponent>",
            xml::esc_attr(&c.0),
            xml::esc_attr(l),
            xml::esc_text(&c.2)
        ),
        None => format!("<DataComponent Type=\"{}\">{}</DataComponent>", xml::esc_attr(&c.0), xml::esc_text(&c.2)),
    }
}

/// Add any missing components (matched by type + locale). Returns `None` when
/// nothing needs to change.
pub fn ensure_components(doc: &Doc, wanted: &[Component]) -> Option<Vec<u8>> {
    let have = parse_components(doc);
    let missing: Vec<&Component> =
        wanted.iter().filter(|w| !have.iter().any(|h| h.0 == w.0 && h.1 == w.1)).collect();
    if missing.is_empty() {
        return None;
    }
    let nl = doc.newline();
    let mut add = String::new();
    for c in missing {
        add.push_str(nl);
        add.push_str(INDENT);
        add.push_str(&component_line(c));
    }
    let mut ed = Edits::default();
    match doc.root.children.last() {
        Some(last) => ed.insert(last.span.end, add),
        None if doc.root.self_closing => {
            ed.replace(doc.root.span.clone(), format!("<{}>{add}{nl}</{}>", doc.root.name, doc.root.name))
        }
        None => ed.replace(doc.root.inner.clone(), format!("{add}{nl}")),
    }
    Some(doc.finish(ed.apply(&doc.src)))
}

/// ComponentList as the editor writes it: CRLF, no trailing newline.
pub fn new_components(list: &[Component]) -> Vec<u8> {
    let nl = "\r\n";
    let mut s = String::from(xml::DECL);
    s.push_str(nl);
    s.push_str("<Components>");
    for c in list {
        s.push_str(nl);
        s.push_str(INDENT);
        s.push_str(&component_line(c));
    }
    s.push_str(nl);
    s.push_str("</Components>");
    s.into_bytes()
}

pub fn comp(ty: &str, locale: Option<&str>, path: &str) -> Component {
    (ty.to_owned(), locale.map(str::to_owned), path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deps_roundtrip_and_patch() {
        let deps = vec!["bnet:Liberty.SC2Mod/0.0/999,file:Mods/Liberty.SC2Mod".to_owned()];
        let b = new_document_info(&deps);
        let d = xml::parse_bytes(&b).unwrap();
        assert_eq!(parse_dependencies(&d), deps);
        let deps2 = vec!["a".to_owned(), "b & c".to_owned()];
        let p = patch_dependencies(&d, &deps2);
        let d2 = xml::parse_bytes(&p).unwrap();
        assert_eq!(parse_dependencies(&d2), deps2);
        assert!(String::from_utf8(p).unwrap().contains("<ModType>"));
    }

    #[test]
    fn components_add_missing() {
        let b = new_components(&[comp("gada", None, "GameData"), comp("text", Some("enUS"), "GameText")]);
        let s = String::from_utf8(b.clone()).unwrap();
        assert!(s.ends_with("</Components>"));
        let d = xml::parse_bytes(&b).unwrap();
        assert!(ensure_components(&d, &[comp("gada", None, "GameData")]).is_none());
        let out = ensure_components(&d, &[comp("regi", None, "Regions")]).unwrap();
        let d2 = xml::parse_bytes(&out).unwrap();
        assert_eq!(parse_components(&d2).len(), 3);
        assert_eq!(parse_components(&d2)[2], comp("regi", None, "Regions"));
    }
}
