//! A small span-tracking XML layer over quick-xml.
//!
//! Every element remembers the byte ranges of itself, its start tag and its
//! content, so components can be edited by splicing only the changed parts
//! into the original text ([`Edits`]). Untouched bytes, including comments,
//! whitespace and attribute quoting, survive exactly.
//!
//! When whole elements must be written, [`write_node`] produces the layout
//! Blizzard's editor emits: 4-space indent, `<a b="c"/>` for empty elements,
//! and the document's own newline style (CRLF for editor output).

use std::ops::Range;

use quick_xml::events::{BytesStart, Event};
use quick_xml::reader::Reader;
use strata_core::catalog::XmlNode;

pub const DECL: &str = r#"<?xml version="1.0" encoding="utf-8"?>"#;
pub const INDENT: &str = "    ";

#[derive(Debug, Clone)]
pub struct Elem {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<Elem>,
    /// Text content. Whitespace-only text is dropped for elements with children.
    pub text: String,
    pub span: Range<usize>,
    pub start_tag: Range<usize>,
    /// Content between the tags (empty, positioned at the tag end, when self-closing).
    pub inner: Range<usize>,
    pub self_closing: bool,
}

impl Elem {
    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn child(&self, name: &str) -> Option<&Elem> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn children_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Elem> + 'a {
        self.children.iter().filter(move |c| c.name == name)
    }

    /// `value` attribute of a named child: `<name value="..."/>`.
    pub fn child_value(&self, name: &str) -> Option<&str> {
        self.child(name).and_then(|c| c.attr("value"))
    }

    pub fn to_node(&self) -> XmlNode {
        XmlNode {
            name: self.name.clone(),
            attrs: self.attrs.clone(),
            children: self.children.iter().map(Elem::to_node).collect(),
            text: self.text.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Doc {
    /// Source text with any UTF-8 BOM removed; spans index into this.
    pub src: String,
    pub bom: bool,
    pub root: Elem,
}

impl Doc {
    pub fn newline(&self) -> &'static str {
        newline_of(&self.src)
    }

    /// Re-attach the BOM and encode.
    pub fn finish(&self, text: String) -> Vec<u8> {
        finish(self.bom, text)
    }

    pub fn raw(&self, r: &Range<usize>) -> &str {
        &self.src[r.clone()]
    }
}

pub fn finish(bom: bool, text: String) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 3);
    if bom {
        out.extend_from_slice(b"\xEF\xBB\xBF");
    }
    out.extend_from_slice(text.as_bytes());
    out
}

pub fn newline_of(src: &str) -> &'static str {
    if src.contains("\r\n") || !src.contains('\n') { "\r\n" } else { "\n" }
}

fn read_start(e: &BytesStart, start: usize, end: usize, self_closing: bool) -> Result<Elem, String> {
    let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
    let mut attrs = Vec::new();
    for a in e.attributes() {
        let a = a.map_err(|err| format!("bad attribute in <{name}>: {err}"))?;
        let key = String::from_utf8_lossy(a.key.as_ref()).into_owned();
        let value = match a.unescape_value() {
            Ok(v) => v.into_owned(),
            Err(_) => String::from_utf8_lossy(&a.value).into_owned(),
        };
        attrs.push((key, value));
    }
    Ok(Elem {
        name,
        attrs,
        children: Vec::new(),
        text: String::new(),
        span: start..end,
        start_tag: start..end,
        inner: end..end,
        self_closing,
    })
}

/// Parse a document. Accepts bytes with or without a UTF-8 BOM.
pub fn parse_bytes(bytes: &[u8]) -> Result<Doc, String> {
    let (bom, body) = match bytes.strip_prefix(b"\xEF\xBB\xBF") {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body).map_err(|e| format!("not UTF-8: {e}"))?;
    let mut doc = parse(text)?;
    doc.bom = bom;
    Ok(doc)
}

pub fn parse(text: &str) -> Result<Doc, String> {
    let mut reader = Reader::from_str(text);
    let mut stack: Vec<Elem> = Vec::new();
    let mut root: Option<Elem> = None;
    loop {
        let before = reader.buffer_position() as usize;
        let ev = reader.read_event().map_err(|e| format!("XML error at byte {before}: {e}"))?;
        let after = reader.buffer_position() as usize;
        match ev {
            Event::Start(e) => {
                stack.push(read_start(&e, before, after, false)?);
            }
            Event::Empty(e) => {
                let el = read_start(&e, before, after, true)?;
                match stack.last_mut() {
                    Some(p) => p.children.push(el),
                    None => {
                        if root.is_none() {
                            root = Some(el);
                        }
                    }
                }
            }
            Event::End(_) => {
                let mut el = stack.pop().ok_or("unbalanced end tag")?;
                el.inner = el.start_tag.end..before;
                el.span = el.start_tag.start..after;
                if !el.children.is_empty() && el.text.trim().is_empty() {
                    el.text.clear();
                }
                match stack.last_mut() {
                    Some(p) => p.children.push(el),
                    None => {
                        if root.is_none() {
                            root = Some(el);
                        }
                    }
                }
            }
            Event::Text(t) => {
                if let Some(p) = stack.last_mut() {
                    let raw = t.decode().map_err(|e| e.to_string())?;
                    match quick_xml::escape::unescape(&raw) {
                        Ok(s) => p.text.push_str(&s),
                        Err(_) => p.text.push_str(&raw),
                    }
                }
            }
            Event::CData(c) => {
                if let Some(p) = stack.last_mut() {
                    p.text.push_str(&String::from_utf8_lossy(&c));
                }
            }
            Event::GeneralRef(r) => {
                if let Some(p) = stack.last_mut() {
                    if let Ok(Some(ch)) = r.resolve_char_ref() {
                        p.text.push(ch);
                    } else {
                        let name = r.decode().map_err(|e| e.to_string())?;
                        match quick_xml::escape::resolve_predefined_entity(&name) {
                            Some(s) => p.text.push_str(s),
                            None => {
                                p.text.push('&');
                                p.text.push_str(&name);
                                p.text.push(';');
                            }
                        }
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !stack.is_empty() {
        return Err("unclosed element at end of document".into());
    }
    let root = root.ok_or("document has no root element")?;
    Ok(Doc { src: text.to_owned(), bom: false, root })
}

pub fn esc_attr(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\r' => o.push_str("&#13;"),
            '\n' => o.push_str("&#10;"),
            '\t' => o.push_str("&#9;"),
            c => o.push(c),
        }
    }
    o
}

pub fn esc_text(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            c => o.push(c),
        }
    }
    o
}

/// `<name a="b">` (or `<name a="b"/>` when `self_closing`).
pub fn start_tag(name: &str, attrs: &[(String, String)], self_closing: bool) -> String {
    let mut s = format!("<{name}");
    for (k, v) in attrs {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        s.push_str(&esc_attr(v));
        s.push('"');
    }
    s.push_str(if self_closing { "/>" } else { ">" });
    s
}

/// Serialize `n` at `depth` (the caller writes the indentation before the
/// first line; nested lines are indented by this function).
pub fn write_node(out: &mut String, n: &XmlNode, depth: usize, nl: &str) {
    if n.children.is_empty() && n.text.is_empty() {
        out.push_str(&start_tag(&n.name, &n.attrs, true));
        return;
    }
    out.push_str(&start_tag(&n.name, &n.attrs, false));
    if n.children.is_empty() {
        out.push_str(&esc_text(&n.text));
    } else {
        if !n.text.trim().is_empty() {
            out.push_str(&esc_text(&n.text));
        }
        for c in &n.children {
            out.push_str(nl);
            out.push_str(&INDENT.repeat(depth + 1));
            write_node(out, c, depth + 1, nl);
        }
        out.push_str(nl);
        out.push_str(&INDENT.repeat(depth));
    }
    out.push_str("</");
    out.push_str(&n.name);
    out.push('>');
}

pub fn node_string(n: &XmlNode, depth: usize, nl: &str) -> String {
    let mut s = String::new();
    write_node(&mut s, n, depth, nl);
    s
}

/// A whole document: declaration, root, trailing newline.
pub fn document(root: &XmlNode, nl: &str, trailing_newline: bool) -> String {
    let mut s = String::from(DECL);
    s.push_str(nl);
    write_node(&mut s, root, 0, nl);
    if trailing_newline {
        s.push_str(nl);
    }
    s
}

/// What to do with an existing child of the root when rebuilding a document.
pub enum Keep {
    Raw,
    Drop,
    Replace(String),
}

/// Rebuild the root's children: each existing child is kept verbatim,
/// replaced or dropped (`decide`), then `appended` elements (already
/// serialized at depth 1) are added at the end. Text outside the root and
/// the whitespace/comments between kept children are preserved.
pub fn rebuild(doc: &Doc, mut decide: impl FnMut(&Elem) -> Keep, appended: &[String]) -> Vec<u8> {
    let nl = doc.newline();
    let root = &doc.root;
    let mut body = String::new();
    let mut cursor = root.inner.start;
    for c in &root.children {
        let trivia = &doc.src[cursor..c.span.start];
        cursor = c.span.end;
        match decide(c) {
            Keep::Raw => {
                body.push_str(trivia);
                body.push_str(doc.raw(&c.span));
            }
            Keep::Replace(s) => {
                body.push_str(trivia);
                body.push_str(&s);
            }
            Keep::Drop => {
                if trivia.contains("<!--") {
                    body.push_str(trivia.trim_end());
                }
            }
        }
    }
    let trailing = &doc.src[cursor..root.inner.end];
    for s in appended {
        body.push_str(nl);
        body.push_str(INDENT);
        body.push_str(s);
    }
    let mut out = String::with_capacity(doc.src.len() + body.len());
    if root.self_closing {
        out.push_str(&doc.src[..root.span.start]);
        out.push_str(&start_tag(&root.name, &root.attrs, false));
        out.push_str(&body);
        out.push_str(nl);
        out.push_str(&format!("</{}>", root.name));
        out.push_str(&doc.src[root.span.end..]);
    } else {
        out.push_str(&doc.src[..root.inner.start]);
        out.push_str(&body);
        out.push_str(if trailing.is_empty() && !body.is_empty() { nl } else { trailing });
        out.push_str(&doc.src[root.inner.end..]);
    }
    doc.finish(out)
}

/// Byte-range replacements applied to a source string in one pass.
#[derive(Default)]
pub struct Edits {
    list: Vec<(Range<usize>, String)>,
}

impl Edits {
    pub fn replace(&mut self, r: Range<usize>, s: impl Into<String>) {
        self.list.push((r, s.into()));
    }

    pub fn insert(&mut self, at: usize, s: impl Into<String>) {
        self.list.push((at..at, s.into()));
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn apply(mut self, src: &str) -> String {
        // Stable sort keeps insertion order for inserts at the same offset.
        self.list.sort_by_key(|(r, _)| r.start);
        let mut out = String::with_capacity(src.len());
        let mut cur = 0;
        for (r, s) in self.list {
            if r.start < cur {
                continue; // overlapping edit; keep the earlier one
            }
            out.push_str(&src[cur..r.start]);
            out.push_str(&s);
            cur = r.end;
        }
        out.push_str(&src[cur..]);
        out
    }
}

/// Replace one attribute value inside an element's start tag, keeping the
/// rest of the tag verbatim. Adds the attribute if missing.
pub fn set_attr_edit(doc: &Doc, el: &Elem, key: &str, value: &str, edits: &mut Edits) {
    if el.attr(key) == Some(value) {
        return;
    }
    let mut attrs = el.attrs.clone();
    match attrs.iter_mut().find(|(k, _)| k == key) {
        Some((_, v)) => *v = value.to_owned(),
        None => attrs.push((key.to_owned(), value.to_owned())),
    }
    // Rebuild the tag only when we cannot patch it in place.
    let tag = doc.raw(&el.start_tag);
    if let Some((vs, ve)) = find_attr_value(tag, key) {
        let base = el.start_tag.start;
        edits.replace(base + vs..base + ve, esc_attr(value));
    } else {
        edits.replace(el.start_tag.clone(), start_tag(&el.name, &attrs, el.self_closing));
    }
}

/// Byte range (within `tag`) of the value of attribute `key`, excluding quotes.
fn find_attr_value(tag: &str, key: &str) -> Option<(usize, usize)> {
    let b = tag.as_bytes();
    let mut i = 0;
    while let Some(off) = tag[i..].find(key) {
        let s = i + off;
        i = s + key.len();
        let before_ok = s > 0 && b[s - 1].is_ascii_whitespace();
        let mut j = s + key.len();
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if !before_ok || j >= b.len() || b[j] != b'=' {
            continue;
        }
        j += 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= b.len() || (b[j] != b'"' && b[j] != b'\'') {
            continue;
        }
        let q = b[j];
        let vs = j + 1;
        let ve = vs + tag[vs..].find(q as char)?;
        return Some((vs, ve));
    }
    None
}

/// Whitespace that precedes `pos` on its line (the element's indentation).
pub fn indent_before(src: &str, pos: usize) -> &str {
    let line_start = src[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let ws = &src[line_start..pos];
    if ws.chars().all(|c| c == ' ' || c == '\t') { ws } else { "" }
}

/// Format a float the way the SC2 editor writes object fields: at most 4
/// decimals, trailing zeros trimmed ("105.5515", "1", "0").
pub fn fmt_num(v: f32) -> String {
    if !v.is_finite() {
        return "0".into();
    }
    let mut s = format!("{:.4}", v);
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        s = "0".into();
    }
    s
}

pub fn fmt_list(vals: &[f32]) -> String {
    vals.iter().map(|v| fmt_num(*v)).collect::<Vec<_>>().join(",")
}

pub fn parse_list(s: &str) -> Vec<f32> {
    s.split(',').map(|p| p.trim().parse::<f32>().unwrap_or(0.0)).collect()
}

/// C `printf("%e")` style: `1.674004e-04`.
pub fn fmt_sci(v: f64) -> String {
    let s = format!("{:.6e}", v);
    match s.split_once('e') {
        Some((m, e)) => {
            let (sign, digits) = match e.strip_prefix('-') {
                Some(d) => ('-', d),
                None => ('+', e),
            };
            format!("{m}e{sign}{digits:0>2}")
        }
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_and_text() {
        let src = "<?xml version=\"1.0\"?>\r\n<A x=\"1 &amp; 2\">\r\n    <!-- c -->\r\n    <B v=\"q\"/>\r\n    <C>t &lt; u</C>\r\n</A>";
        let d = parse(src).unwrap();
        assert_eq!(d.root.name, "A");
        assert_eq!(d.root.attr("x"), Some("1 & 2"));
        assert_eq!(d.root.children.len(), 2);
        let b = &d.root.children[0];
        assert_eq!(d.raw(&b.span), "<B v=\"q\"/>");
        let c = &d.root.children[1];
        assert_eq!(c.text, "t < u");
        assert_eq!(d.raw(&c.inner), "t &lt; u");
        assert_eq!(d.root.text, "");
    }

    #[test]
    fn set_attr_in_place() {
        let src = "<r><e  a='1'   b=\"2\"/></r>";
        let d = parse(src).unwrap();
        let mut ed = Edits::default();
        set_attr_edit(&d, &d.root.children[0], "b", "x&y", &mut ed);
        set_attr_edit(&d, &d.root.children[0], "a", "1", &mut ed);
        assert_eq!(ed.apply(&d.src), "<r><e  a='1'   b=\"x&amp;y\"/></r>");
    }

    #[test]
    fn writer_layout() {
        let mut n = XmlNode::new("CUnit");
        n.set_attr("id", "A\"B");
        let mut f = XmlNode::new("LifeMax");
        f.set_attr("value", "45");
        n.children.push(f);
        let s = node_string(&n, 1, "\r\n");
        assert_eq!(s, "<CUnit id=\"A&quot;B\">\r\n        <LifeMax value=\"45\"/>\r\n    </CUnit>");
        let back = parse(&s).unwrap().root.to_node();
        assert_eq!(back, n);
    }

    #[test]
    fn numbers() {
        assert_eq!(fmt_num(105.5515), "105.5515");
        assert_eq!(fmt_num(1.0), "1");
        assert_eq!(fmt_num(-0.00001), "0");
        assert_eq!(fmt_sci(1.674004e-4), "1.674004e-04");
        assert_eq!(fmt_sci(8.0), "8.000000e+00");
    }
}
