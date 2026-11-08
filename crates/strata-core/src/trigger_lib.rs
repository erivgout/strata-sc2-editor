//! Curated library of Galaxy natives and NativeLib functions for the trigger
//! editor: signatures, parameter presets, natural-language phrases and fuzzy
//! search.
//!
//! The table in [`natives`] is extracted from the game's own
//! `TriggerLibs/NativeLib.triggerlib` (build 98310), so names, parameter order,
//! types, defaults and preset values match what the official editor emits.
//! Functions that are not natives (`native: false`) live in NativeLib and are
//! called with the `libNtve_gf_` prefix, which is already part of `name`.
//!
//! A handful of hidden natives the official codegen uses but the library does
//! not list (`UnitFromId`, `PointFromId`, `RegionFromId`, `CameraInfoFromId`,
//! `UnitRefFromUnit`, `UnitRefFromVariable`, `PlayerGroupNextPlayer`,
//! `UnitGroupUnitFromEnd`) are only in [`KNOWN_FUNCTIONS`]; of these,
//! `UnitGroupUnitFromEnd` is the one not confirmed against a real map script.

mod natives;

use std::collections::HashMap;
use std::sync::OnceLock;

pub use natives::{FUNCTIONS, KNOWN_FUNCTIONS};

use crate::triggers::{Call, Expr, GalaxyType, Literal};

/// What a library entry is used as in the trigger editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FnKind {
    /// `TriggerAddEvent*`: registered in the trigger's init function.
    Event,
    /// Returns `bool`; offered when adding a condition.
    Condition,
    /// A statement in the actions list (may also return a value).
    Action,
    /// A value function used inside parameters.
    Function,
}

impl FnKind {
    pub fn label(self) -> &'static str {
        match self {
            FnKind::Event => "Event",
            FnKind::Condition => "Condition",
            FnKind::Action => "Action",
            FnKind::Function => "Function",
        }
    }
}

/// A parameter default, as written in the trigger library.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Def {
    /// The user has to choose a value.
    None,
    Int(i64),
    Fixed(f64),
    Bool(bool),
    Str(&'static str),
    /// A preset constant such as `c_unitCreateIgnorePlacement`.
    Preset(&'static str),
    /// A Galaxy expression such as `UnitGetPosition(EventUnit())`.
    Raw(&'static str),
}

impl Def {
    pub fn to_expr(self) -> Option<Expr> {
        Some(match self {
            Def::None => return None,
            Def::Int(v) => Expr::Literal(Literal::Int(v)),
            Def::Fixed(v) => Expr::Literal(Literal::Fixed(v)),
            Def::Bool(v) => Expr::Literal(Literal::Bool(v)),
            Def::Str(s) => Expr::Literal(Literal::String(s.into())),
            Def::Preset(p) => Expr::Preset(p.into()),
            Def::Raw(r) => parse_expr(r).unwrap_or_else(|| Expr::Raw(r.into())),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParamSpec {
    /// Identifier used in the phrase template (`~id~`).
    pub id: &'static str,
    pub label: &'static str,
    /// Galaxy type name of the argument (`int`, `point`, `unitref`...).
    pub ty: &'static str,
    /// Trigger-library type (`gamelink`, `preset`, `unit`...). Picks the editor widget.
    pub editor: &'static str,
    /// Catalog for `gamelink` parameters (`Unit`, `Abil`...), or the preset name.
    pub link: &'static str,
    pub default: Def,
    /// (label, Galaxy value) choices.
    pub presets: &'static [(&'static str, &'static str)],
    /// Presets are bit flags that combine with `|`.
    pub bits: bool,
    /// For `bool` parameters: "On text/Off text", e.g. "Show/Don't Show".
    pub alts: &'static str,
}

impl ParamSpec {
    pub fn galaxy_type(&self) -> GalaxyType {
        GalaxyType::from_name(self.ty)
    }

    /// The value a new call starts with.
    pub fn default_expr(&self) -> Expr {
        self.default.to_expr().unwrap_or_else(|| Expr::Raw(String::new()))
    }

    /// Label for a preset value, if this parameter has it as a choice.
    pub fn preset_label(&self, value: &str) -> Option<&'static str> {
        self.presets.iter().find(|(_, v)| *v == value).map(|(l, _)| *l)
    }

    /// The on/off words for a bool parameter.
    pub fn bool_words(&self) -> Option<(&'static str, &'static str)> {
        self.alts.split_once('/')
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FnSpec {
    /// Galaxy function name, ready to call.
    pub name: &'static str,
    pub label: &'static str,
    pub kind: FnKind,
    pub category: &'static str,
    /// Galaxy return type name, empty for `void`.
    pub ret: &'static str,
    pub params: &'static [ParamSpec],
    /// Natural-language template, e.g. "Create ~count~ ~type~ for player ~player~".
    pub phrase: &'static str,
    pub desc: &'static str,
    pub native: bool,
}

/// A piece of a rendered phrase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhrasePart<'a> {
    Text(&'a str),
    /// Index into `FnSpec::params`.
    Param(usize),
}

impl FnSpec {
    pub fn returns(&self) -> Option<GalaxyType> {
        (!self.ret.is_empty()).then(|| GalaxyType::from_name(self.ret))
    }

    pub fn param_index(&self, id: &str) -> Option<usize> {
        self.params.iter().position(|p| p.id == id)
    }

    /// A call with every parameter set to its default.
    pub fn new_call(&self) -> Call {
        Call { function: self.name.into(), args: self.params.iter().map(ParamSpec::default_expr).collect() }
    }

    /// Split the phrase into text and parameter slots. Parameters the phrase
    /// does not mention are appended at the end so every argument is editable.
    pub fn phrase_parts(&self) -> Vec<PhrasePart<'static>> {
        let mut out = Vec::new();
        let mut used = vec![false; self.params.len()];
        let mut rest: &'static str = self.phrase;
        while let Some(start) = rest.find('~') {
            let Some(len) = rest[start + 1..].find('~') else { break };
            let id = &rest[start + 1..start + 1 + len];
            if start > 0 {
                out.push(PhrasePart::Text(&rest[..start]));
            }
            match self.param_index(id) {
                Some(i) => {
                    used[i] = true;
                    out.push(PhrasePart::Param(i));
                }
                None => out.push(PhrasePart::Text(&rest[start..start + len + 2])),
            }
            rest = &rest[start + len + 2..];
        }
        if !rest.is_empty() {
            out.push(PhrasePart::Text(rest));
        }
        if self.phrase.is_empty() {
            out.push(PhrasePart::Text(self.label));
        }
        for (i, u) in used.iter().enumerate() {
            if !u {
                out.push(PhrasePart::Text(if i == 0 && self.phrase.is_empty() { " (" } else { ", " }));
                out.push(PhrasePart::Param(i));
            }
        }
        out
    }
}

fn index() -> &'static HashMap<&'static str, &'static FnSpec> {
    static INDEX: OnceLock<HashMap<&'static str, &'static FnSpec>> = OnceLock::new();
    INDEX.get_or_init(|| FUNCTIONS.iter().map(|f| (f.name, f)).collect())
}

/// Every curated function.
pub fn all() -> &'static [FnSpec] {
    FUNCTIONS
}

/// Look up a curated function by its Galaxy name.
pub fn get(name: &str) -> Option<&'static FnSpec> {
    index().get(name).copied()
}

/// True if `name` is any function the native library defines (curated or not).
pub fn is_known_function(name: &str) -> bool {
    get(name).is_some() || KNOWN_FUNCTIONS.binary_search(&name).is_ok()
}

/// Categories in library order.
pub fn categories() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for f in FUNCTIONS {
        if !out.contains(&f.category) {
            out.push(f.category);
        }
    }
    out
}

fn is_word_start(t: &[char], i: usize) -> bool {
    if i == 0 {
        return true;
    }
    let (p, c) = (t[i - 1], t[i]);
    !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase()) || (p.is_alphabetic() && c.is_ascii_digit())
}

/// Fuzzy subsequence score of `query` against `text` (case-insensitive).
/// Rewards matches at word starts (spaces, `_`, camel humps) and runs of
/// consecutive characters. `None` if `query` is not a subsequence.
pub fn fuzzy_score(query: &str, text: &str) -> Option<i32> {
    let q: Vec<char> = query.chars().flat_map(char::to_lowercase).filter(|c| !c.is_whitespace()).collect();
    if q.is_empty() {
        return Some(0);
    }
    let t: Vec<char> = text.chars().collect();
    let mut qi = 0;
    let mut score = 0;
    let mut prev_match: Option<usize> = None;
    let mut first: Option<usize> = None;
    for (i, &c) in t.iter().enumerate() {
        if qi == q.len() {
            break;
        }
        let lc = c.to_lowercase().next().unwrap_or(c);
        if lc != q[qi] {
            continue;
        }
        let word_start = is_word_start(&t, i);
        score += 1;
        if word_start {
            score += 8;
        }
        if let Some(pm) = prev_match {
            if pm + 1 == i {
                score += 4;
            } else {
                score -= ((i - pm - 1) as i32).min(3);
            }
        }
        first.get_or_insert(i);
        prev_match = Some(i);
        qi += 1;
    }
    if qi < q.len() {
        return None;
    }
    // Bonus for the query appearing whole at the start of the text or of a word.
    let lower: Vec<char> = t.iter().map(|c| c.to_lowercase().next().unwrap_or(*c)).collect();
    if lower.starts_with(&q) {
        score += 30;
    } else if (1..t.len()).any(|i| is_word_start(&t, i) && lower[i..].starts_with(&q)) {
        score += 15;
    }
    score -= (first.unwrap_or(0) as i32).min(10);
    Some(score)
}

/// Score one library entry against a (possibly multi-word) query. Every word
/// has to match the label, Galaxy name, phrase or category.
pub fn score(f: &FnSpec, query: &str) -> Option<i32> {
    let mut total = 0;
    for word in query.split_whitespace() {
        let best = [
            fuzzy_score(word, f.label).map(|s| s * 3),
            fuzzy_score(word, f.name).map(|s| s * 3),
            fuzzy_score(word, f.phrase).map(|s| s * 2),
            fuzzy_score(word, f.category).map(|s| s * 2),
        ]
        .into_iter()
        .flatten()
        .max()?;
        total += best;
    }
    Some(total)
}

/// Search the library. An empty query returns every entry passing `filter`
/// in library order; otherwise best matches first.
pub fn search(query: &str, filter: impl Fn(&FnSpec) -> bool) -> Vec<&'static FnSpec> {
    let mut hits: Vec<(i32, usize, &'static FnSpec)> =
        FUNCTIONS.iter().enumerate().filter(|(_, f)| filter(f)).filter_map(|(i, f)| score(f, query).map(|s| (s, i, f))).collect();
    if !query.trim().is_empty() {
        hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    }
    hits.into_iter().map(|(_, _, f)| f).collect()
}

/// Parse a small Galaxy expression: literals, `null`, preset constants and
/// (nested) function calls. Anything else returns `None`.
pub fn parse_expr(src: &str) -> Option<Expr> {
    let mut p = Parser { s: src.as_bytes(), i: 0 };
    let e = p.expr()?;
    p.ws();
    (p.i == p.s.len()).then_some(e)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn expr(&mut self) -> Option<Expr> {
        self.ws();
        let c = self.peek()?;
        if c == b'"' {
            self.i += 1;
            let mut out = String::new();
            loop {
                let c = self.peek()?;
                self.i += 1;
                match c {
                    b'"' => break,
                    b'\\' => {
                        let e = self.peek()?;
                        self.i += 1;
                        out.push(match e {
                            b'n' => '\n',
                            b't' => '\t',
                            other => other as char,
                        });
                    }
                    _ => {
                        // Re-decode multi-byte UTF-8 runs.
                        let start = self.i - 1;
                        let mut end = self.i;
                        while end < self.s.len() && (self.s[end] & 0xC0) == 0x80 {
                            end += 1;
                        }
                        out.push_str(std::str::from_utf8(&self.s[start..end]).ok()?);
                        self.i = end;
                    }
                }
            }
            return Some(Expr::Literal(Literal::String(out)));
        }
        if c == b'-' || c.is_ascii_digit() {
            let start = self.i;
            self.i += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit() || c == b'.') {
                self.i += 1;
            }
            let t = std::str::from_utf8(&self.s[start..self.i]).ok()?;
            return if t.contains('.') {
                t.parse().ok().map(|v| Expr::Literal(Literal::Fixed(v)))
            } else {
                t.parse().ok().map(|v| Expr::Literal(Literal::Int(v)))
            };
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            let start = self.i;
            while self.peek().is_some_and(|c| c.is_ascii_alphanumeric() || c == b'_') {
                self.i += 1;
            }
            let id = std::str::from_utf8(&self.s[start..self.i]).ok()?.to_owned();
            self.ws();
            if self.peek() == Some(b'(') {
                self.i += 1;
                let mut args = Vec::new();
                self.ws();
                if self.peek() == Some(b')') {
                    self.i += 1;
                } else {
                    loop {
                        args.push(self.expr()?);
                        self.ws();
                        match self.peek()? {
                            b',' => self.i += 1,
                            b')' => {
                                self.i += 1;
                                break;
                            }
                            _ => return None,
                        }
                    }
                }
                return Some(Expr::Call(Call { function: id, args }));
            }
            return Some(match id.as_str() {
                "true" => Expr::Literal(Literal::Bool(true)),
                "false" => Expr::Literal(Literal::Bool(false)),
                _ if id == "null" || id.starts_with("c_") || id.contains("_ge_") => Expr::Preset(id),
                _ => Expr::Raw(id),
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_signatures() {
        let f = get("UnitCreate").unwrap();
        assert_eq!(f.kind, FnKind::Action);
        let ids: Vec<_> = f.params.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["count", "type", "flags", "player", "pos", "angle"]);
        assert!(f.params[2].presets.iter().any(|(_, v)| *v == "c_unitCreateIgnorePlacement"));
        assert_eq!(get("TriggerAddEventUnitDied").unwrap().params[0].ty, "unitref");
        assert!(get("libNtve_gf_KillingUnit").is_some());
        assert!(get("MeleeInitResources").is_some());
        assert!(is_known_function("UnitFromId"));
        assert!(!is_known_function("DefinitelyNotANative"));
    }

    #[test]
    fn names_are_unique_and_sorted_index() {
        let mut names: Vec<_> = FUNCTIONS.iter().map(|f| f.name).collect();
        let n = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), n);
        assert!(KNOWN_FUNCTIONS.windows(2).all(|w| w[0] < w[1]));
        assert!(n >= 150);
    }

    #[test]
    fn phrase_parts_cover_every_param() {
        for f in FUNCTIONS {
            let parts = f.phrase_parts();
            for i in 0..f.params.len() {
                assert!(parts.contains(&PhrasePart::Param(i)), "{} param {i}", f.name);
            }
        }
    }

    #[test]
    fn fuzzy_ranks_word_starts() {
        let hits = search("crt unit", |f| f.kind == FnKind::Action);
        assert!(hits.iter().take(5).any(|f| f.name == "UnitCreate"), "{:?}", hits.iter().take(5).map(|f| f.name).collect::<Vec<_>>());
        assert!(fuzzy_score("ued", "TriggerAddEventUnitDied").is_some());
        assert!(fuzzy_score("xyz", "UnitCreate").is_none());
        assert!(fuzzy_score("uc", "UnitCreate") > fuzzy_score("uc", "Ruckus"));
        let hits = search("text message", |_| true);
        assert_eq!(hits[0].name, "UIDisplayMessage");
    }

    #[test]
    fn parse_defaults() {
        assert_eq!(
            parse_expr("UnitGetPosition(EventUnit())"),
            Some(Expr::Call(Call {
                function: "UnitGetPosition".into(),
                args: vec![Expr::Call(Call { function: "EventUnit".into(), args: vec![] })]
            }))
        );
        assert_eq!(parse_expr("c_unitCountAll"), Some(Expr::Preset("c_unitCountAll".into())));
        assert_eq!(parse_expr("Point(1.5, -2)").map(|e| matches!(e, Expr::Call(_))), Some(true));
        assert_eq!(parse_expr("a + b"), None);
        for f in FUNCTIONS {
            for p in f.params {
                if let Def::Raw(r) = p.default {
                    assert!(parse_expr(r).is_some(), "{} {} {r}", f.name, p.id);
                }
            }
        }
    }
}
