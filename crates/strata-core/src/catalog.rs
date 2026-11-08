//! GameData catalogs (UnitData.xml, AbilData.xml, WeaponData.xml, ...).
//!
//! Entries are kept as a generic XML element tree so that every field SC2
//! supports round-trips, while the Data panel layers friendly editing on top.
//!
//! Besides the raw model this module provides:
//! - [`XmlNode::parse`] / [`XmlNode::to_xml`]: a small, dependency-free XML
//!   reader and writer for single catalog entries (the Data panel's raw view).
//! - [`FieldKey`]: how a field is addressed (`LifeMax`, `CostResource[Minerals]`).
//! - Inheritance through `parent` within the map ([`Catalog::resolved_fields`]).
//! - Reference search and rename ([`Catalog::references_to`]).
//! - Global search ([`Catalog::search`]).

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// A generic XML element: `<Name attr="v">children</Name>`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct XmlNode {
    pub name: String,
    pub attrs: Vec<(String, String)>,
    pub children: Vec<XmlNode>,
    /// Text content, if any (rare in SC2 catalogs).
    pub text: String,
}

impl XmlNode {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn set_attr(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.attrs.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.attrs.push((key.to_owned(), value)),
        }
    }

    /// Remove an attribute, returning its old value.
    pub fn remove_attr(&mut self, key: &str) -> Option<String> {
        let i = self.attrs.iter().position(|(k, _)| k == key)?;
        Some(self.attrs.remove(i).1)
    }

    pub fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn child_mut(&mut self, name: &str) -> Option<&mut XmlNode> {
        self.children.iter_mut().find(|c| c.name == name)
    }

    /// True for `<Field value="x"/>` style nodes (optionally with `index`).
    pub fn is_scalar(&self) -> bool {
        self.children.is_empty()
            && self.text.trim().is_empty()
            && self.attr("value").is_some()
            && self.attrs.iter().all(|(k, _)| k == "value" || k == "index")
    }

    /// Children paired with the key that addresses them. Children with an
    /// `index` attribute are keyed by it; others by their position among
    /// unindexed siblings of the same name.
    pub fn keyed_children(&self) -> Vec<(FieldKey, &XmlNode)> {
        let mut counts: Vec<(&str, usize)> = Vec::new();
        self.children
            .iter()
            .map(|c| {
                let key = match c.attr("index") {
                    Some(i) => FieldKey {
                        name: c.name.clone(),
                        index: Some(i.to_owned()),
                        ordinal: 0,
                    },
                    None => {
                        let ordinal = match counts.iter_mut().find(|(n, _)| *n == c.name) {
                            Some((_, n)) => {
                                *n += 1;
                                *n - 1
                            }
                            None => {
                                counts.push((&c.name, 1));
                                0
                            }
                        };
                        FieldKey {
                            name: c.name.clone(),
                            index: None,
                            ordinal,
                        }
                    }
                };
                (key, c)
            })
            .collect()
    }

    /// Position in `children` of the child addressed by `key`.
    pub fn find_keyed(&self, key: &FieldKey) -> Option<usize> {
        match &key.index {
            Some(i) => self
                .children
                .iter()
                .position(|c| c.name == key.name && c.attr("index") == Some(i.as_str())),
            None => self
                .children
                .iter()
                .enumerate()
                .filter(|(_, c)| c.name == key.name && c.attr("index").is_none())
                .nth(key.ordinal)
                .map(|(i, _)| i),
        }
    }

    /// Visit every attribute in this subtree as (`path`, name, value), where
    /// `path` names the element chain below this node ("" for the node itself).
    pub fn walk_attrs<'a>(&'a self, f: &mut impl FnMut(&str, &'a str, &'a str)) {
        fn go<'a>(n: &'a XmlNode, path: &str, f: &mut impl FnMut(&str, &'a str, &'a str)) {
            for (k, v) in &n.attrs {
                f(path, k, v);
            }
            for c in &n.children {
                let p = if path.is_empty() {
                    c.name.clone()
                } else {
                    format!("{path}.{}", c.name)
                };
                go(c, &p, f);
            }
        }
        go(self, "", f);
    }

    /// Serialize as indented XML (4 spaces), ending with a newline.
    pub fn to_xml(&self) -> String {
        let mut out = String::new();
        self.write_xml(&mut out, 0);
        out
    }

    fn write_xml(&self, out: &mut String, depth: usize) {
        let pad = "    ".repeat(depth);
        out.push_str(&pad);
        out.push('<');
        out.push_str(&self.name);
        for (k, v) in &self.attrs {
            out.push(' ');
            out.push_str(k);
            out.push_str("=\"");
            out.push_str(&escape_xml(v, true));
            out.push('"');
        }
        let text = self.text.trim();
        if self.children.is_empty() && text.is_empty() {
            out.push_str("/>\n");
            return;
        }
        out.push('>');
        if self.children.is_empty() {
            out.push_str(&escape_xml(text, false));
        } else {
            out.push('\n');
            if !text.is_empty() {
                out.push_str(&pad);
                out.push_str("    ");
                out.push_str(&escape_xml(text, false));
                out.push('\n');
            }
            for c in &self.children {
                c.write_xml(out, depth + 1);
            }
            out.push_str(&pad);
        }
        out.push_str("</");
        out.push_str(&self.name);
        out.push_str(">\n");
    }

    /// Parse exactly one element (an optional `<?xml?>` prolog and comments
    /// are allowed around it). Comments inside are dropped.
    pub fn parse(src: &str) -> Result<XmlNode, XmlError> {
        let mut nodes = Self::parse_fragment(src)?;
        match nodes.len() {
            1 => Ok(nodes.remove(0)),
            0 => Err(XmlError {
                line: 1,
                col: 1,
                message: "Expected an element such as <CUnit id=\"MyUnit\">.".into(),
            }),
            n => Err(XmlError {
                line: 1,
                col: 1,
                message: format!("Expected one element but found {n}. Edit one entry at a time."),
            }),
        }
    }

    /// Parse zero or more top-level elements.
    pub fn parse_fragment(src: &str) -> Result<Vec<XmlNode>, XmlError> {
        let mut p = Parser { src, pos: 0 };
        let mut out = Vec::new();
        loop {
            p.skip_misc()?;
            if p.eof() {
                return Ok(out);
            }
            if p.peek() == Some(b'<') {
                out.push(p.element()?);
            } else {
                return Err(p.err("Text outside of an element. Wrap values in an element such as <LifeMax value=\"45\"/>."));
            }
        }
    }
}

/// Escape `&`, `<`, `>` (and `"` in attributes).
pub fn escape_xml(s: &str, attr: bool) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' if attr => out.push_str("&quot;"),
            '\n' if attr => out.push_str("&#10;"),
            c => out.push(c),
        }
    }
    out
}

/// A parse error with a 1-based position.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("Line {line}, column {col}: {message}")]
pub struct XmlError {
    pub line: usize,
    pub col: usize,
    pub message: String,
}

struct Parser<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn eof(&self) -> bool {
        self.pos >= self.src.len()
    }
    fn peek(&self) -> Option<u8> {
        self.src.as_bytes().get(self.pos).copied()
    }
    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }
    fn err_at(&self, pos: usize, message: impl Into<String>) -> XmlError {
        let before = &self.src[..pos.min(self.src.len())];
        let line = before.matches('\n').count() + 1;
        let col = before
            .rsplit('\n')
            .next()
            .map(|l| l.chars().count())
            .unwrap_or(0)
            + 1;
        XmlError {
            line,
            col,
            message: message.into(),
        }
    }
    fn err(&self, message: impl Into<String>) -> XmlError {
        self.err_at(self.pos, message)
    }
    fn skip_ws(&mut self) {
        while self.peek().is_some_and(|b| b.is_ascii_whitespace()) {
            self.pos += 1;
        }
    }
    fn skip_until(&mut self, end: &str, what: &str) -> Result<&'a str, XmlError> {
        let start = self.pos;
        match self.rest().find(end) {
            Some(i) => {
                let s = &self.src[self.pos..self.pos + i];
                self.pos += i + end.len();
                Ok(s)
            }
            None => Err(self.err_at(
                start,
                format!("{what} is never closed (missing \"{end}\")."),
            )),
        }
    }
    /// Skip whitespace, prolog, comments and doctype.
    fn skip_misc(&mut self) -> Result<(), XmlError> {
        loop {
            self.skip_ws();
            let r = self.rest();
            if r.starts_with("<?") {
                self.pos += 2;
                self.skip_until("?>", "The <? declaration")?;
            } else if r.starts_with("<!--") {
                self.pos += 4;
                self.skip_until("-->", "A comment")?;
            } else if r.starts_with("<!DOCTYPE") || r.starts_with("<!doctype") {
                self.skip_until(">", "The doctype")?;
            } else {
                return Ok(());
            }
        }
    }
    fn name(&mut self) -> Result<&'a str, XmlError> {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_whitespace() || matches!(b, b'/' | b'>' | b'=' | b'<' | b'"' | b'\'') {
                break;
            }
            self.pos += 1;
        }
        if self.pos == start {
            return Err(self.err("Expected a name here."));
        }
        Ok(&self.src[start..self.pos])
    }
    fn element(&mut self) -> Result<XmlNode, XmlError> {
        let open = self.pos;
        self.pos += 1; // '<'
        let name = self.name()?;
        let mut node = XmlNode::new(name);
        loop {
            self.skip_ws();
            match self.peek() {
                None => return Err(self.err_at(open, format!("<{name}> is never closed."))),
                Some(b'/') => {
                    if self.rest().starts_with("/>") {
                        self.pos += 2;
                        return Ok(node);
                    }
                    return Err(self.err("Expected \"/>\"."));
                }
                Some(b'>') => {
                    self.pos += 1;
                    break;
                }
                Some(_) => {
                    let at = self.pos;
                    let key = self.name()?;
                    self.skip_ws();
                    if self.peek() != Some(b'=') {
                        return Err(self.err(format!(
                            "Attribute \"{key}\" needs a value, like {key}=\"1\"."
                        )));
                    }
                    self.pos += 1;
                    self.skip_ws();
                    let quote = match self.peek() {
                        Some(q @ (b'"' | b'\'')) => q,
                        _ => {
                            return Err(
                                self.err(format!("The value of \"{key}\" must be in quotes."))
                            )
                        }
                    };
                    self.pos += 1;
                    let vstart = self.pos;
                    let end = self.rest().find(quote as char).ok_or_else(|| {
                        self.err_at(
                            vstart,
                            format!("The value of \"{key}\" is missing its closing quote."),
                        )
                    })?;
                    let raw = &self.src[vstart..vstart + end];
                    self.pos = vstart + end + 1;
                    if node.attr(key).is_some() {
                        return Err(self.err_at(
                            at,
                            format!("Attribute \"{key}\" appears twice on <{name}>."),
                        ));
                    }
                    let value = unescape(raw).map_err(|m| self.err_at(vstart, m))?;
                    node.attrs.push((key.to_owned(), value));
                }
            }
        }
        // Content.
        loop {
            let r = self.rest();
            if r.is_empty() {
                return Err(self.err_at(open, format!("<{name}> is never closed. Add </{name}>.")));
            }
            if r.starts_with("</") {
                let at = self.pos;
                self.pos += 2;
                let close = self.name()?;
                self.skip_ws();
                if self.peek() != Some(b'>') {
                    return Err(self.err("Expected \">\"."));
                }
                self.pos += 1;
                if close != name {
                    return Err(
                        self.err_at(at, format!("Expected </{name}> but found </{close}>."))
                    );
                }
                node.text = node.text.trim().to_owned();
                return Ok(node);
            } else if r.starts_with("<!--") {
                self.pos += 4;
                self.skip_until("-->", "A comment")?;
            } else if r.starts_with("<?") {
                // Processing instructions such as Blizzard's template
                // `<?token id="abil" type="CAbilLink"?>`; dropped like comments.
                self.pos += 2;
                self.skip_until("?>", "The <? instruction")?;
            } else if r.starts_with("<![CDATA[") {
                self.pos += 9;
                let s = self.skip_until("]]>", "A CDATA section")?;
                node.text.push_str(s);
            } else if r.starts_with('<') {
                node.children.push(self.element()?);
            } else {
                let start = self.pos;
                let end = r.find('<').unwrap_or(r.len());
                let raw = &self.src[start..start + end];
                self.pos += end;
                let t = unescape(raw).map_err(|m| self.err_at(start, m))?;
                node.text.push_str(&t);
            }
        }
    }
}

fn unescape(s: &str) -> Result<String, String> {
    if !s.contains('&') {
        return Ok(s.to_owned());
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest
            .find(';')
            .ok_or_else(|| "An \"&\" must start an entity such as &amp;.".to_owned())?;
        let ent = &rest[1..end];
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            _ => {
                let code =
                    if let Some(h) = ent.strip_prefix("#x").or_else(|| ent.strip_prefix("#X")) {
                        u32::from_str_radix(h, 16).ok()
                    } else if let Some(d) = ent.strip_prefix('#') {
                        d.parse().ok()
                    } else {
                        None
                    };
                match code.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => {
                        return Err(format!(
                            "Unknown entity \"&{ent};\". Use &amp; for a literal \"&\"."
                        ))
                    }
                }
            }
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Address of a field within an entry: `LifeMax`, `CostResource[Minerals]`,
/// or the n-th unindexed `WeaponArray` (`ordinal`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FieldKey {
    pub name: String,
    pub index: Option<String>,
    /// Position among unindexed siblings with the same name.
    pub ordinal: usize,
}

impl FieldKey {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            index: None,
            ordinal: 0,
        }
    }

    pub fn indexed(name: impl Into<String>, index: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            index: Some(index.into()),
            ordinal: 0,
        }
    }

    /// Parse `Name`, `Name[index]` or `Name #3` (third unindexed element).
    pub fn parse(s: &str) -> Self {
        let s = s.trim();
        if let Some((name, rest)) = s.split_once('[') {
            let index = rest.trim_end_matches(']').trim();
            return Self::indexed(name.trim(), index);
        }
        if let Some((name, n)) = s.rsplit_once(" #") {
            if let Ok(n) = n.trim().parse::<usize>() {
                return Self {
                    name: name.trim().to_owned(),
                    index: None,
                    ordinal: n.saturating_sub(1),
                };
            }
        }
        Self::new(s)
    }
}

impl fmt::Display for FieldKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.index {
            Some(i) => write!(f, "{}[{i}]", self.name),
            None if self.ordinal > 0 => write!(f, "{} #{}", self.name, self.ordinal + 1),
            None => f.write_str(&self.name),
        }
    }
}

/// One catalog entry, e.g. `<CUnit id="Marine" parent="...">`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CatalogEntry {
    /// Which GameData file this lives in, e.g. "UnitData".
    pub file: String,
    /// The element itself; `node.name` is the class ("CUnit", "CWeaponLegacy").
    pub node: XmlNode,
}

impl CatalogEntry {
    /// A new entry `<class id="id" parent="parent"/>` in `file`.
    pub fn new(file: impl Into<String>, class: &str, id: &str, parent: Option<&str>) -> Self {
        let mut node = XmlNode::new(class);
        node.set_attr("id", id);
        if let Some(p) = parent.filter(|p| !p.is_empty()) {
            node.set_attr("parent", p);
        }
        Self {
            file: file.into(),
            node,
        }
    }

    pub fn class(&self) -> &str {
        &self.node.name
    }
    pub fn id(&self) -> &str {
        self.node.attr("id").unwrap_or("")
    }
    pub fn parent(&self) -> Option<&str> {
        self.node.attr("parent")
    }
    pub fn kind(&self) -> CatalogKind {
        CatalogKind::from_file(&self.file)
    }

    pub fn set_id(&mut self, id: &str) {
        self.node.set_attr("id", id);
    }

    /// Set or clear (`None` / empty) the parent.
    pub fn set_parent(&mut self, parent: Option<&str>) {
        match parent.filter(|p| !p.is_empty()) {
            Some(p) => self.node.set_attr("parent", p),
            None => {
                self.node.remove_attr("parent");
            }
        }
    }

    /// Value of a simple scalar field such as `<LifeMax value="45"/>`.
    pub fn field(&self, name: &str) -> Option<&str> {
        self.node.child(name).and_then(|c| c.attr("value"))
    }

    pub fn set_field(&mut self, name: &str, value: impl Into<String>) {
        let value = value.into();
        match self.node.children.iter_mut().find(|c| c.name == name) {
            Some(c) => c.set_attr("value", value),
            None => {
                let mut c = XmlNode::new(name);
                c.set_attr("value", value);
                self.node.children.push(c);
            }
        }
    }

    /// The child element addressed by `key`.
    pub fn get(&self, key: &FieldKey) -> Option<&XmlNode> {
        self.node.find_keyed(key).map(|i| &self.node.children[i])
    }

    /// The `value` attribute of the child addressed by `key`.
    pub fn value(&self, key: &FieldKey) -> Option<&str> {
        self.get(key).and_then(|n| n.attr("value"))
    }

    /// Set the `value` attribute of `key`, creating the element if needed.
    pub fn set_value(&mut self, key: &FieldKey, value: impl Into<String>) {
        let value = value.into();
        match self.node.find_keyed(key) {
            Some(i) => self.node.children[i].set_attr("value", value),
            None => {
                let mut c = XmlNode::new(&key.name);
                if let Some(i) = &key.index {
                    c.set_attr("index", i.as_str());
                }
                c.set_attr("value", value);
                self.insert_child(c);
            }
        }
    }

    /// Insert or replace the element addressed by `key`.
    pub fn put(&mut self, key: &FieldKey, node: XmlNode) {
        match self.node.find_keyed(key) {
            Some(i) => self.node.children[i] = node,
            None => self.insert_child(node),
        }
    }

    /// Remove the element addressed by `key`.
    pub fn remove(&mut self, key: &FieldKey) -> Option<XmlNode> {
        let i = self.node.find_keyed(key)?;
        Some(self.node.children.remove(i))
    }

    /// Insert after the last sibling with the same name, so arrays stay together.
    fn insert_child(&mut self, c: XmlNode) {
        match self.node.children.iter().rposition(|x| x.name == c.name) {
            Some(i) => self.node.children.insert(i + 1, c),
            None => self.node.children.push(c),
        }
    }

    /// The entry as XML text.
    pub fn to_xml(&self) -> String {
        self.node.to_xml()
    }

    /// Parse an entry from XML text, keeping `file`.
    pub fn from_xml(file: impl Into<String>, src: &str) -> Result<CatalogEntry, XmlError> {
        let node = XmlNode::parse(src)?;
        if node.attr("id").is_none_or(|s| s.trim().is_empty()) {
            return Err(XmlError {
                line: 1,
                col: 1,
                message: format!("<{}> needs an id attribute, like id=\"MyUnit\".", node.name),
            });
        }
        Ok(CatalogEntry {
            file: file.into(),
            node,
        })
    }
}

/// The catalog family an entry belongs to, derived from its file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum CatalogKind {
    Unit,
    Ability,
    Weapon,
    Effect,
    Behavior,
    Upgrade,
    Actor,
    Model,
    Button,
    Requirement,
    Other,
}

impl CatalogKind {
    pub const ALL: [CatalogKind; 11] = [
        CatalogKind::Unit,
        CatalogKind::Ability,
        CatalogKind::Weapon,
        CatalogKind::Effect,
        CatalogKind::Behavior,
        CatalogKind::Upgrade,
        CatalogKind::Actor,
        CatalogKind::Model,
        CatalogKind::Button,
        CatalogKind::Requirement,
        CatalogKind::Other,
    ];

    pub fn from_file(file: &str) -> CatalogKind {
        match file.trim_end_matches(".xml") {
            "UnitData" => CatalogKind::Unit,
            "AbilData" => CatalogKind::Ability,
            "WeaponData" => CatalogKind::Weapon,
            "EffectData" => CatalogKind::Effect,
            "BehaviorData" => CatalogKind::Behavior,
            "UpgradeData" => CatalogKind::Upgrade,
            "ActorData" => CatalogKind::Actor,
            "ModelData" => CatalogKind::Model,
            "ButtonData" => CatalogKind::Button,
            "RequirementData" => CatalogKind::Requirement,
            _ => CatalogKind::Other,
        }
    }

    pub fn file(self) -> &'static str {
        match self {
            CatalogKind::Unit => "UnitData",
            CatalogKind::Ability => "AbilData",
            CatalogKind::Weapon => "WeaponData",
            CatalogKind::Effect => "EffectData",
            CatalogKind::Behavior => "BehaviorData",
            CatalogKind::Upgrade => "UpgradeData",
            CatalogKind::Actor => "ActorData",
            CatalogKind::Model => "ModelData",
            CatalogKind::Button => "ButtonData",
            CatalogKind::Requirement => "RequirementData",
            CatalogKind::Other => "OtherData",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CatalogKind::Unit => "Units",
            CatalogKind::Ability => "Abilities",
            CatalogKind::Weapon => "Weapons",
            CatalogKind::Effect => "Effects",
            CatalogKind::Behavior => "Behaviors",
            CatalogKind::Upgrade => "Upgrades",
            CatalogKind::Actor => "Actors",
            CatalogKind::Model => "Models",
            CatalogKind::Button => "Buttons",
            CatalogKind::Requirement => "Requirements",
            CatalogKind::Other => "Other",
        }
    }

    /// Lowercase singular noun for sentences ("unit", "ability").
    pub fn noun(self) -> &'static str {
        match self {
            CatalogKind::Unit => "unit",
            CatalogKind::Ability => "ability",
            CatalogKind::Weapon => "weapon",
            CatalogKind::Effect => "effect",
            CatalogKind::Behavior => "behavior",
            CatalogKind::Upgrade => "upgrade",
            CatalogKind::Actor => "actor",
            CatalogKind::Model => "model",
            CatalogKind::Button => "button",
            CatalogKind::Requirement => "requirement",
            CatalogKind::Other => "entry",
        }
    }
}

/// A field value as seen from an entry, own or inherited.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedField {
    pub key: FieldKey,
    pub node: XmlNode,
    /// Index of the entry that sets this value (the entry itself if own).
    pub source: usize,
    pub inherited: bool,
}

/// Something in the catalog that mentions an id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    /// Entry that holds the reference.
    pub index: usize,
    /// Where: "parent", "WeaponArray.Link", "Effect.value"...
    pub path: String,
}

/// One global search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub index: usize,
    /// What matched: "id", "parent", or a field path such as "LifeMax".
    pub field: String,
    pub value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn find(&self, class: &str, id: &str) -> Option<&CatalogEntry> {
        self.entries
            .iter()
            .find(|e| e.class() == class && e.id() == id)
    }

    pub fn find_mut(&mut self, class: &str, id: &str) -> Option<&mut CatalogEntry> {
        self.entries
            .iter_mut()
            .find(|e| e.class() == class && e.id() == id)
    }

    /// Index of the entry with this class and id.
    pub fn index_of(&self, class: &str, id: &str) -> Option<usize> {
        self.entries
            .iter()
            .position(|e| e.class() == class && e.id() == id)
    }

    pub fn of_kind(&self, kind: CatalogKind) -> impl Iterator<Item = (usize, &CatalogEntry)> {
        self.entries
            .iter()
            .enumerate()
            .filter(move |(_, e)| CatalogKind::from_file(&e.file) == kind)
    }

    /// Number of entries per kind, in [`CatalogKind::ALL`] order.
    pub fn counts(&self) -> [usize; 11] {
        let mut out = [0; 11];
        for e in &self.entries {
            let k = e.kind();
            if let Some(i) = CatalogKind::ALL.iter().position(|x| *x == k) {
                out[i] += 1;
            }
        }
        out
    }

    /// Resolve a scalar field through the `parent` chain (map-local only).
    pub fn resolve_field(&self, class: &str, id: &str, field: &str) -> Option<String> {
        let mut cur = self.find(class, id);
        let mut guard = 0;
        while let Some(e) = cur {
            if let Some(v) = e.field(field) {
                return Some(v.to_owned());
            }
            guard += 1;
            if guard > 32 {
                break;
            }
            cur = e.parent().and_then(|p| self.find(class, p));
        }
        None
    }

    /// The map-local parent of entry `index`: an entry of the same kind whose
    /// id is this entry's `parent`, preferring the same class.
    pub fn parent_of(&self, index: usize) -> Option<usize> {
        let e = self.entries.get(index)?;
        let p = e.parent()?;
        let kind = e.kind();
        let mut fallback = None;
        for (i, c) in self.entries.iter().enumerate() {
            if i == index || c.id() != p || c.kind() != kind {
                continue;
            }
            if c.class() == e.class() {
                return Some(i);
            }
            fallback.get_or_insert(i);
        }
        fallback
    }

    /// Map-local ancestors of `index`, nearest first. Stops at cycles.
    pub fn parent_chain(&self, index: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = index;
        while let Some(p) = self.parent_of(cur) {
            if p == index || out.contains(&p) || out.len() > 64 {
                break;
            }
            out.push(p);
            cur = p;
        }
        out
    }

    /// True if following `parent` from `index` comes back to it.
    pub fn has_parent_cycle(&self, index: usize) -> bool {
        let mut seen = vec![index];
        let mut cur = index;
        while let Some(p) = self.parent_of(cur) {
            if seen.contains(&p) {
                return true;
            }
            seen.push(p);
            cur = p;
        }
        false
    }

    /// The `parent` id at the top of the map-local chain, if it points outside
    /// the map (usually a base-game entry such as "Marine").
    pub fn external_root(&self, index: usize) -> Option<&str> {
        let top = self.parent_chain(index).last().copied().unwrap_or(index);
        let e = self.entries.get(top)?;
        if self.parent_of(top).is_some() {
            return None; // cycle
        }
        e.parent()
    }

    /// The element `key` as seen from `index`, with the entry that sets it.
    pub fn resolve(&self, index: usize, key: &FieldKey) -> Option<(&XmlNode, usize)> {
        let e = self.entries.get(index)?;
        if let Some(n) = e.get(key) {
            return Some((n, index));
        }
        self.parent_chain(index)
            .into_iter()
            .find_map(|p| self.entries[p].get(key).map(|n| (n, p)))
    }

    /// The scalar `value` of `key` as seen from `index`, with its source.
    pub fn resolve_value(&self, index: usize, key: &FieldKey) -> Option<(&str, usize)> {
        self.resolve(index, key)
            .and_then(|(n, src)| n.attr("value").map(|v| (v, src)))
    }

    /// Every field visible on `index`: its own (in document order) followed by
    /// inherited ones from the nearest ancestor that sets each.
    pub fn resolved_fields(&self, index: usize) -> Vec<ResolvedField> {
        let Some(e) = self.entries.get(index) else {
            return Vec::new();
        };
        let mut out: Vec<ResolvedField> = e
            .node
            .keyed_children()
            .into_iter()
            .map(|(key, node)| ResolvedField {
                key,
                node: node.clone(),
                source: index,
                inherited: false,
            })
            .collect();
        for p in self.parent_chain(index) {
            for (key, node) in self.entries[p].node.keyed_children() {
                if !out.iter().any(|f| f.key == key) {
                    out.push(ResolvedField {
                        key,
                        node: node.clone(),
                        source: p,
                        inherited: true,
                    });
                }
            }
        }
        out
    }

    /// Every place in the catalog that mentions `id`, either as a whole
    /// attribute value or as one item of a `,`/`;`/space separated list.
    /// The `id` attribute of the entries themselves is not a reference.
    pub fn references_to(&self, id: &str) -> Vec<Reference> {
        let mut out = Vec::new();
        if id.is_empty() {
            return out;
        }
        for (i, e) in self.entries.iter().enumerate() {
            e.node.walk_attrs(&mut |path, k, v| {
                if path.is_empty() && k == "id" {
                    return;
                }
                if mentions(v, id) {
                    let p = if path.is_empty() {
                        k.to_owned()
                    } else {
                        format!("{path}.{k}")
                    };
                    out.push(Reference { index: i, path: p });
                }
            });
        }
        out
    }

    /// Entries that would change if `old` were renamed to `new` everywhere it
    /// is referenced, as (index, updated entry). Does not modify the catalog.
    pub fn renamed_references(&self, old: &str, new: &str) -> Vec<(usize, CatalogEntry)> {
        fn fix(n: &mut XmlNode, old: &str, new: &str, root: bool) -> bool {
            let mut changed = false;
            for (k, v) in n.attrs.iter_mut() {
                if root && k == "id" {
                    continue;
                }
                if mentions(v, old) {
                    *v = replace_token(v, old, new);
                    changed = true;
                }
            }
            for c in &mut n.children {
                changed |= fix(c, old, new, false);
            }
            changed
        }
        let mut out = Vec::new();
        if old.is_empty() || old == new {
            return out;
        }
        for (i, e) in self.entries.iter().enumerate() {
            let mut e2 = e.clone();
            if fix(&mut e2.node, old, new, true) {
                out.push((i, e2));
            }
        }
        out
    }

    /// Case-insensitive search across ids, parents, classes and every
    /// attribute value. At most one hit per entry (the best match).
    pub fn search(&self, query: &str) -> Vec<SearchHit> {
        let q = query.trim().to_lowercase();
        if q.is_empty() {
            return Vec::new();
        }
        let mut hits: Vec<(u8, SearchHit)> = Vec::new();
        for (i, e) in self.entries.iter().enumerate() {
            let id = e.id();
            if id.to_lowercase().contains(&q) {
                let rank = if id.eq_ignore_ascii_case(&q) { 0 } else { 1 };
                hits.push((
                    rank,
                    SearchHit {
                        index: i,
                        field: "id".into(),
                        value: id.into(),
                    },
                ));
                continue;
            }
            if let Some(p) = e.parent().filter(|p| p.to_lowercase().contains(&q)) {
                hits.push((
                    2,
                    SearchHit {
                        index: i,
                        field: "parent".into(),
                        value: p.into(),
                    },
                ));
                continue;
            }
            if e.class().to_lowercase().contains(&q) {
                hits.push((
                    3,
                    SearchHit {
                        index: i,
                        field: "class".into(),
                        value: e.class().into(),
                    },
                ));
                continue;
            }
            let mut found: Option<SearchHit> = None;
            e.node.walk_attrs(&mut |path, k, v| {
                if found.is_some() || (path.is_empty() && (k == "id" || k == "parent")) {
                    return;
                }
                let field = if path.is_empty() {
                    k.to_owned()
                } else {
                    path.to_owned()
                };
                if v.to_lowercase().contains(&q) || field.to_lowercase().contains(&q) {
                    found = Some(SearchHit {
                        index: i,
                        field,
                        value: v.to_owned(),
                    });
                }
            });
            if let Some(h) = found {
                hits.push((4, h));
            }
        }
        hits.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.index.cmp(&b.1.index)));
        hits.into_iter().map(|(_, h)| h).collect()
    }

    /// `base`, or `base2`, `base3`... whichever is not yet used by an entry of `class`.
    pub fn unique_id(&self, class: &str, base: &str) -> String {
        let base = if base.is_empty() { "New" } else { base };
        if self.find(class, base).is_none() {
            return base.to_owned();
        }
        (2..)
            .map(|n| format!("{base}{n}"))
            .find(|id| self.find(class, id).is_none())
            .unwrap_or_default()
    }
}

/// A lookup table from (kind, id) to entry indices, for views that resolve
/// inheritance for many entries at once. Rebuild it after the catalog changes.
#[derive(Debug, Clone, Default)]
pub struct CatalogIndex {
    by_id: HashMap<(CatalogKind, String), Vec<usize>>,
}

impl Catalog {
    pub fn build_index(&self) -> CatalogIndex {
        let mut by_id: HashMap<(CatalogKind, String), Vec<usize>> = HashMap::new();
        for (i, e) in self.entries.iter().enumerate() {
            by_id
                .entry((e.kind(), e.id().to_owned()))
                .or_default()
                .push(i);
        }
        CatalogIndex { by_id }
    }
}

impl CatalogIndex {
    /// Entries of `kind` with this id.
    pub fn lookup(&self, kind: CatalogKind, id: &str) -> &[usize] {
        // Borrowed lookup without allocating requires a matching key type; ids are short.
        self.by_id
            .get(&(kind, id.to_owned()))
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Same as [`Catalog::parent_of`], in constant time.
    pub fn parent_of(&self, cat: &Catalog, index: usize) -> Option<usize> {
        let e = cat.entries.get(index)?;
        let cands = self.lookup(e.kind(), e.parent()?);
        let mut fallback = None;
        for &i in cands {
            if i == index {
                continue;
            }
            if cat.entries[i].class() == e.class() {
                return Some(i);
            }
            fallback.get_or_insert(i);
        }
        fallback
    }

    /// Same as [`Catalog::resolve_value`], using the index.
    pub fn resolve_value<'c>(
        &self,
        cat: &'c Catalog,
        index: usize,
        key: &FieldKey,
    ) -> Option<(&'c str, usize)> {
        let mut cur = index;
        for _ in 0..64 {
            if let Some(v) = cat.entries.get(cur)?.value(key) {
                return Some((v, cur));
            }
            cur = self.parent_of(cat, cur)?;
            if cur == index {
                return None;
            }
        }
        None
    }
}

fn is_sep(c: char) -> bool {
    matches!(c, ',' | ';' | ' ' | '|')
}

/// Whether attribute value `v` names `id` (whole value or one list item).
fn mentions(v: &str, id: &str) -> bool {
    v == id || (v.contains(id) && v.split(is_sep).any(|t| t == id))
}

fn replace_token(v: &str, old: &str, new: &str) -> String {
    if v == old {
        return new.to_owned();
    }
    let mut out = String::with_capacity(v.len());
    let mut tok = String::new();
    for c in v.chars() {
        if is_sep(c) {
            out.push_str(if tok == old { new } else { &tok });
            tok.clear();
            out.push(c);
        } else {
            tok.push(c);
        }
    }
    out.push_str(if tok == old { new } else { &tok });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(id: &str, parent: Option<&str>) -> CatalogEntry {
        CatalogEntry::new("UnitData", "CUnit", id, parent)
    }

    #[test]
    fn xml_roundtrip() {
        let src = r#"<?xml version="1.0" encoding="utf-8"?>
<!-- a comment -->
<CUnit id="MyMarine" parent="Marine">
    <LifeMax value="55"/>
    <CostResource index="Minerals" value="40"/>
    <WeaponArray Link="GaussRifle" Turret="Marine"/>
    <Name value="Tom &amp; &quot;Jerry&quot; &#65;"/>
    <Cost>
        <Vital index="Energy" value="50"/>
    </Cost>
</CUnit>"#;
        let n = XmlNode::parse(src).unwrap();
        assert_eq!(n.name, "CUnit");
        assert_eq!(n.attr("parent"), Some("Marine"));
        assert_eq!(n.children.len(), 5);
        assert_eq!(n.children[3].attr("value"), Some("Tom & \"Jerry\" A"));
        assert_eq!(n.children[4].children[0].attr("index"), Some("Energy"));
        let again = XmlNode::parse(&n.to_xml()).unwrap();
        assert_eq!(again, n);
    }

    #[test]
    fn xml_errors_are_located() {
        let e = XmlNode::parse("<CUnit id=\"A\">\n  <LifeMax value=\"1\">\n</CUnit>").unwrap_err();
        assert_eq!(e.line, 3);
        assert!(e.message.contains("Expected </LifeMax>"), "{}", e.message);
        let e = XmlNode::parse("<CUnit id=A/>").unwrap_err();
        assert!(e.message.contains("quotes"));
        let e = XmlNode::parse("<CUnit id=\"A\" id=\"B\"/>").unwrap_err();
        assert!(e.message.contains("twice"));
        assert!(XmlNode::parse("<A/><B/>").is_err());
        assert!(XmlNode::parse("<CUnit id=\"A\">")
            .unwrap_err()
            .message
            .contains("never closed"));
        assert!(CatalogEntry::from_xml("UnitData", "<CUnit/>").is_err());
    }

    #[test]
    fn field_keys() {
        let k = FieldKey::parse("CostResource[Minerals]");
        assert_eq!(k, FieldKey::indexed("CostResource", "Minerals"));
        assert_eq!(k.to_string(), "CostResource[Minerals]");
        assert_eq!(FieldKey::parse("WeaponArray #2").ordinal, 1);
        let mut e = unit("A", None);
        e.set_value(&k, "75");
        e.set_value(&FieldKey::indexed("CostResource", "Vespene"), "25");
        e.set_value(&FieldKey::new("LifeMax"), "10");
        assert_eq!(e.value(&k), Some("75"));
        assert_eq!(e.node.children[1].attr("index"), Some("Vespene"));
        assert!(e.remove(&k).is_some());
        assert_eq!(e.value(&k), None);
        let mut w = XmlNode::new("WeaponArray");
        w.set_attr("Link", "A");
        e.node.children.push(w.clone());
        w.set_attr("Link", "B");
        e.node.children.push(w);
        let keys: Vec<String> = e
            .node
            .keyed_children()
            .iter()
            .map(|(k, _)| k.to_string())
            .collect();
        assert_eq!(
            keys,
            [
                "CostResource[Vespene]",
                "LifeMax",
                "WeaponArray",
                "WeaponArray #2"
            ]
        );
        assert_eq!(
            e.get(&FieldKey::parse("WeaponArray #2"))
                .unwrap()
                .attr("Link"),
            Some("B")
        );
    }

    #[test]
    fn inheritance_resolution() {
        let mut c = Catalog::default();
        let mut base = unit("Base", Some("Marine"));
        base.set_field("LifeMax", "100");
        base.set_field("Speed", "3");
        let mut mid = unit("Mid", Some("Base"));
        mid.set_field("Speed", "4");
        let mut leaf = unit("Leaf", Some("Mid"));
        leaf.set_field("Sight", "9");
        c.entries = vec![leaf, base, mid];
        assert_eq!(c.parent_chain(0), vec![2, 1]);
        assert_eq!(c.resolve_value(0, &FieldKey::new("Speed")), Some(("4", 2)));
        assert_eq!(
            c.resolve_value(0, &FieldKey::new("LifeMax")),
            Some(("100", 1))
        );
        assert_eq!(c.external_root(0), Some("Marine"));
        let idx = c.build_index();
        assert_eq!(idx.parent_of(&c, 0), c.parent_of(0));
        assert_eq!(
            idx.resolve_value(&c, 0, &FieldKey::new("LifeMax")),
            Some(("100", 1))
        );
        let fields = c.resolved_fields(0);
        let names: Vec<(&str, bool, usize)> = fields
            .iter()
            .map(|f| (f.key.name.as_str(), f.inherited, f.source))
            .collect();
        assert_eq!(
            names,
            [
                ("Sight", false, 0),
                ("Speed", true, 2),
                ("LifeMax", true, 1)
            ]
        );

        // Cycles terminate.
        c.entries[1].set_parent(Some("Leaf"));
        assert!(c.has_parent_cycle(0));
        assert_eq!(c.parent_chain(0), vec![2, 1]);
        assert_eq!(c.external_root(0), None);
    }

    #[test]
    fn parent_lookup_respects_kind() {
        let mut c = Catalog::default();
        c.entries.push(CatalogEntry::new(
            "WeaponData",
            "CWeaponLegacy",
            "Gun",
            None,
        ));
        c.entries.push(unit("Gun", None));
        c.entries.push(unit("Child", Some("Gun")));
        assert_eq!(c.parent_of(2), Some(1));
    }

    #[test]
    fn references_and_rename() {
        let mut c = Catalog::default();
        let mut w = CatalogEntry::new("WeaponData", "CWeaponLegacy", "Gun", None);
        w.set_field("Effect", "GunDamage");
        let mut u = unit("Tank", None);
        let mut arr = XmlNode::new("WeaponArray");
        arr.set_attr("Link", "Gun");
        u.node.children.push(arr);
        let mut up = CatalogEntry::new("UpgradeData", "CUpgrade", "GunUp", None);
        let mut ea = XmlNode::new("EffectArray");
        ea.set_attr("Reference", "Weapon,Gun,Level");
        up.node.children.push(ea);
        let child = CatalogEntry::new("WeaponData", "CWeaponLegacy", "Gun2", Some("Gun"));
        c.entries = vec![w, u, up, child];
        let refs = c.references_to("Gun");
        let got: Vec<(usize, &str)> = refs.iter().map(|r| (r.index, r.path.as_str())).collect();
        assert_eq!(
            got,
            [
                (1, "WeaponArray.Link"),
                (2, "EffectArray.Reference"),
                (3, "parent")
            ]
        );
        assert!(c.references_to("Gu").is_empty());

        let changed = c.renamed_references("Gun", "Cannon");
        assert_eq!(changed.len(), 3);
        assert_eq!(
            changed[1].1.node.children[0].attr("Reference"),
            Some("Weapon,Cannon,Level")
        );
        assert_eq!(changed[2].1.parent(), Some("Cannon"));
        // The renamed entry's own id is untouched by reference rewriting.
        assert!(changed.iter().all(|(i, _)| *i != 0));
    }

    #[test]
    fn search_ranks_ids_first() {
        let mut c = Catalog::default();
        let mut a = unit("Zealotish", None);
        a.set_field("Description", "Like a marine");
        c.entries = vec![a, unit("MyMarine", Some("Marine")), unit("Marine", None)];
        let hits = c.search("marine");
        assert_eq!(hits.iter().map(|h| h.index).collect::<Vec<_>>(), [2, 1, 0]);
        assert_eq!(hits[2].field, "Description");
        assert_eq!(c.unique_id("CUnit", "Marine"), "Marine2");
    }
}
