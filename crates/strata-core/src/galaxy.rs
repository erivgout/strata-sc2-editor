//! Galaxy script generation from the trigger library, plus a lightweight
//! Galaxy linter.
//!
//! Output layout matches what the SC2 editor emits for MapScript.galaxy:
//! includes, `InitLibs()`, globals, custom script, custom functions, one
//! function pair per trigger, `InitTriggers()` and `InitMap()`. Output is
//! deterministic: the same document always produces the same bytes.

use std::fmt::Write;

use crate::document::MapDocument;
use crate::objects::ObjectKind;
use crate::trigger_lib;
use crate::triggers::{Call, CustomFunction, Expr, GalaxyType, Literal, Stmt, Trigger, TriggerId, TriggerLibrary, Variable};

const RULE: &str = "//--------------------------------------------------------------------------------------------------";
const BANNER: &str = "//==================================================================================================";

/// Galaxy identifier for an editor name: "Melee Initialization" -> "MeleeInitialization".
pub fn ident(name: &str) -> String {
    let mut s: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    if s.is_empty() || s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}

/// Variable identifier, lower camel case like the official editor:
/// "Enemy Unit Types" -> "enemyUnitTypes".
pub fn var_ident(name: &str) -> String {
    let s = ident(name);
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_lowercase().to_string() + c.as_str(),
        None => s,
    }
}

pub fn trigger_var(t: &Trigger) -> String {
    format!("gt_{}", ident(&t.name))
}

pub fn global_var(name: &str) -> String {
    format!("gv_{}", var_ident(name))
}

pub fn function_name(f: &CustomFunction) -> String {
    format!("gf_{}", ident(&f.name))
}

fn quote(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => {}
            '\t' => o.push_str("\\t"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Galaxy `fixed` literal: always has a decimal point, never an exponent,
/// at most 4 fractional digits (fixed is 20.12 bits, ~0.00024 precision).
pub fn fixed_literal(v: f64) -> String {
    if !v.is_finite() {
        return "0.0".into();
    }
    let mut s = format!("{:.4}", v);
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.push('0');
    }
    if s == "-0.0" {
        s = "0.0".into();
    }
    s
}

/// The value Galaxy code uses for "nothing" of a type.
pub fn zero_value(ty: &str) -> &'static str {
    match ty {
        "int" | "byte" => "0",
        "fixed" => "0.0",
        "bool" => "false",
        "string" => "\"\"",
        _ => "null",
    }
}

/// Initial value the official editor gives handle types that must not be null.
fn handle_init(ty: &GalaxyType) -> Option<&'static str> {
    match ty {
        GalaxyType::UnitGroup => Some("UnitGroupEmpty()"),
        GalaxyType::PlayerGroup => Some("PlayerGroupEmpty()"),
        GalaxyType::Timer => Some("TimerCreate()"),
        _ => None,
    }
}

/// A generated script plus where each trigger / function starts in it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Generated {
    pub script: String,
    /// `(first line, 1-based; item id; editor name)`, in script order.
    pub sections: Vec<(usize, TriggerId, String)>,
    /// Every `StringExternal` key the script references, sorted and unique.
    pub text_keys: Vec<String>,
}

impl Generated {
    /// The trigger or function whose code contains `line` (1-based).
    pub fn item_at_line(&self, line: usize) -> Option<TriggerId> {
        self.sections.iter().rev().find(|(l, _, _)| *l <= line).map(|(_, id, _)| *id)
    }

    /// First line of an item's code.
    pub fn line_of(&self, id: TriggerId) -> Option<usize> {
        self.sections.iter().find(|(_, i, _)| *i == id).map(|(l, _, _)| *l)
    }
}

/// Code generation scope: one trigger or custom function.
struct Gen<'a> {
    lib: &'a TriggerLibrary,
    doc: Option<&'a MapDocument>,
    params: &'a [Variable],
    locals: &'a [Variable],
    /// Automatic declarations (`int auto3_ae;`), emitted after locals.
    autos: Vec<String>,
    /// Loop variables declared automatically.
    auto_vars: Vec<(String, String)>,
    counter: u32,
    /// `Some(return type)` inside a custom function, `None` in a trigger.
    function: Option<Option<&'a GalaxyType>>,
    text_keys: Vec<String>,
}

impl<'a> Gen<'a> {
    fn new(lib: &'a TriggerLibrary, doc: Option<&'a MapDocument>) -> Self {
        Gen {
            lib,
            doc,
            params: &[],
            locals: &[],
            autos: Vec::new(),
            auto_vars: Vec::new(),
            counter: 0,
            function: None,
            text_keys: Vec::new(),
        }
    }

    fn scoped(&self, params: &'a [Variable], locals: &'a [Variable], function: Option<Option<&'a GalaxyType>>) -> Gen<'a> {
        Gen { params, locals, function, ..Gen::new(self.lib, self.doc) }
    }

    fn lookup(&self, name: &str) -> Option<(&'static str, &Variable)> {
        if let Some(v) = self.params.iter().find(|v| v.name == name) {
            return Some(("lp_", v));
        }
        if let Some(v) = self.locals.iter().find(|v| v.name == name) {
            return Some(("lv_", v));
        }
        self.lib.variables.iter().find(|v| v.name == name).map(|v| ("gv_", v))
    }

    fn var(&self, name: &str) -> String {
        match self.lookup(name) {
            Some((prefix, _)) => format!("{prefix}{}", var_ident(name)),
            None => format!("lv_{}", var_ident(name)),
        }
    }

    fn var_type(&self, name: &str) -> Option<String> {
        if let Some((_, v)) = self.lookup(name) {
            return Some(v.ty.galaxy_name().to_owned());
        }
        self.auto_vars.iter().find(|(n, _)| n == name).map(|(_, t)| t.clone())
    }

    /// Declare a loop variable unless the user already did.
    fn ensure_loop_var(&mut self, name: &str, ty: &str) {
        if self.lookup(name).is_none() && !self.auto_vars.iter().any(|(n, _)| n == name) {
            self.auto_vars.push((name.to_owned(), ty.to_owned()));
            self.autos.push(format!("{ty} lv_{};", var_ident(name)));
        }
    }

    fn auto(&mut self, ty: &str, suffix: &str) -> String {
        let name = format!("auto{}_{suffix}", self.counter);
        self.autos.push(format!("{ty} {name};"));
        name
    }

    fn next_auto(&mut self) {
        self.counter += 1;
    }

    /// Static type of an expression, when it can be worked out.
    fn infer(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Literal(Literal::Int(_)) => Some("int".into()),
            Expr::Literal(Literal::Fixed(_)) => Some("fixed".into()),
            Expr::Literal(Literal::Bool(_)) => Some("bool".into()),
            Expr::Literal(Literal::String(_)) => Some("string".into()),
            Expr::Literal(Literal::Text(_)) => Some("text".into()),
            Expr::Variable(v) | Expr::Element { variable: v, .. } => self.var_type(v),
            Expr::Call(c) => match trigger_lib::get(&c.function) {
                Some(f) => (!f.ret.is_empty()).then(|| f.ret.to_owned()),
                None => self.lib.function_named(&c.function).and_then(|f| f.returns.as_ref()).map(|t| t.galaxy_name().to_owned()),
            },
            Expr::Binary { op, lhs, rhs } => match op.as_str() {
                "==" | "!=" | "<" | "<=" | ">" | ">=" | "&&" | "||" => Some("bool".into()),
                _ => self.infer(lhs).or_else(|| self.infer(rhs)),
            },
            Expr::Not(_) => Some("bool".into()),
            Expr::Trigger(_) => Some("trigger".into()),
            Expr::Placed(n) => self.placed(n, None).map(|(t, _)| t.to_owned()),
            Expr::Preset(_) | Expr::Raw(_) => None,
        }
    }

    /// Resolve a placed object by editor name to `(type, Galaxy accessor)`.
    fn placed(&self, name: &str, expected: Option<&str>) -> Option<(&'static str, String)> {
        let doc = self.doc?;
        let want = |t: &str| expected.is_none_or(|e| e == t || (e == "unitref" && t == "unit"));
        if want("point") {
            if let Some(o) = doc.objects.objects.iter().find(|o| o.kind == ObjectKind::Point && o.name == name) {
                return Some(("point", format!("PointFromId({})", o.id.0)));
            }
        }
        if want("region") {
            if let Some(r) = doc.regions.iter().find(|r| r.name == name) {
                return Some(("region", format!("RegionFromId({})", r.id.0)));
            }
        }
        if want("unit") {
            if let Some(o) = doc.objects.objects.iter().find(|o| o.kind == ObjectKind::Unit && o.name == name) {
                return Some(("unit", format!("UnitFromId({})", o.id.0)));
            }
        }
        if want("camerainfo") {
            if let Some(c) = doc.cameras.iter().find(|c| c.name == name) {
                return Some(("camerainfo", format!("CameraInfoFromId({})", c.id.0)));
            }
        }
        None
    }

    /// Parameter types of a callee, if known.
    fn param_types(&self, function: &str) -> Vec<String> {
        if let Some(f) = trigger_lib::get(function) {
            return f.params.iter().map(|p| p.ty.to_owned()).collect();
        }
        if let Some(f) = self.lib.function_named(function) {
            return f.params.iter().map(|p| p.ty.galaxy_name().to_owned()).collect();
        }
        Vec::new()
    }

    fn callee(&self, function: &str) -> String {
        match self.lib.function_named(function) {
            Some(f) if trigger_lib::get(function).is_none() => function_name(f),
            _ => function.to_owned(),
        }
    }

    fn call(&mut self, c: &Call, first: Option<&str>) -> String {
        let types = self.param_types(&c.function);
        let mut args: Vec<String> = first.map(|f| vec![f.to_owned()]).unwrap_or_default();
        for (i, a) in c.args.iter().enumerate() {
            let expected = types.get(i).map(String::as_str);
            args.push(self.expr(a, expected));
        }
        format!("{}({})", self.callee(&c.function), args.join(", "))
    }

    /// Galaxy for an expression. `expected` is the parameter type it is passed
    /// as, which drives literal coercion, unitref wrapping and placed objects.
    fn expr(&mut self, e: &Expr, expected: Option<&str>) -> String {
        if expected == Some("unitref") {
            return match e {
                Expr::Preset(p) if p == "null" => "null".into(),
                _ if e.is_unset() => "null".into(),
                Expr::Variable(v) if self.lookup(v).is_some() => format!("UnitRefFromVariable({})", quote(&self.var(v))),
                _ => format!("UnitRefFromUnit({})", self.expr(e, Some("unit"))),
            };
        }
        // Galaxy has no implicit conversions; insert the ones the editor would.
        if let (Some(want), false) = (expected, matches!(e, Expr::Literal(_) | Expr::Raw(_) | Expr::Preset(_))) {
            if let Some(have) = self.infer(e) {
                let conv = match (have.as_str(), want) {
                    ("int", "fixed") => Some(("IntToFixed", "")),
                    ("fixed", "int") => Some(("FixedToInt", "")),
                    ("int", "text") => Some(("IntToText", "")),
                    ("fixed", "text") => Some(("FixedToText", ", c_fixedPrecisionAny")),
                    ("string", "text") => Some(("StringToText", "")),
                    ("int", "string") => Some(("IntToString", "")),
                    ("fixed", "string") => Some(("FixedToString", ", c_fixedPrecisionAny")),
                    _ => None,
                };
                if let Some((f, extra)) = conv {
                    let inner = self.expr_plain(e, Some(&have));
                    return format!("{f}({inner}{extra})");
                }
            }
        }
        self.expr_plain(e, expected)
    }

    fn expr_plain(&mut self, e: &Expr, expected: Option<&str>) -> String {
        match e {
            Expr::Literal(Literal::Int(v)) => match expected {
                Some("fixed") => fixed_literal(*v as f64),
                _ => v.to_string(),
            },
            Expr::Literal(Literal::Fixed(v)) => fixed_literal(*v),
            Expr::Literal(Literal::Bool(b)) => b.to_string(),
            Expr::Literal(Literal::String(s)) => match expected {
                Some("text") => format!("StringToText({})", quote(s)),
                _ => quote(s),
            },
            Expr::Literal(Literal::Text(k)) => {
                self.text_keys.push(k.clone());
                format!("StringExternal({})", quote(k))
            }
            Expr::Variable(v) => self.var(v),
            Expr::Element { variable, index } => {
                let i = self.expr(index, Some("int"));
                format!("{}[{i}]", self.var(variable))
            }
            Expr::Call(c) => self.call(c, None),
            Expr::Preset(p) => p.clone(),
            Expr::Placed(n) => match self.placed(n, expected) {
                Some((_, code)) => code,
                None => match expected {
                    Some("region") => format!("RegionFromName({})", quote(n)),
                    Some("unit") => "null".into(),
                    _ => format!("PointFromName({})", quote(n)),
                },
            },
            Expr::Binary { op, lhs, rhs } => {
                let (le, re) = match op.as_str() {
                    "&&" | "||" => (Some("bool".to_owned()), Some("bool".to_owned())),
                    "==" | "!=" | "<" | "<=" | ">" | ">=" => {
                        let l = self.infer(lhs);
                        let r = self.infer(rhs);
                        // Compare as fixed when either side is fixed.
                        let both =
                            if l.as_deref() == Some("fixed") || r.as_deref() == Some("fixed") { Some("fixed".to_owned()) } else { None };
                        (both.clone().or(r), both.or(l))
                    }
                    _ => (expected.map(str::to_owned), expected.map(str::to_owned)),
                };
                let l = self.expr(lhs, le.as_deref());
                let r = self.expr(rhs, re.as_deref());
                format!("({l} {op} {r})")
            }
            Expr::Not(e) => format!("!({})", self.expr(e, Some("bool"))),
            Expr::Raw(r) if r.trim().is_empty() => zero_value(expected.unwrap_or("")).to_owned(),
            Expr::Raw(r) => r.clone(),
            Expr::Trigger(id) => match self.lib.trigger(*id) {
                Some(t) => trigger_var(t),
                None => "null".into(),
            },
        }
    }

    fn stmts(&mut self, out: &mut String, body: &[Stmt], depth: usize) {
        for s in body {
            self.stmt(out, s, depth);
        }
    }

    fn block(&mut self, out: &mut String, header: &str, body: &[Stmt], depth: usize) {
        let pad = "    ".repeat(depth);
        let _ = writeln!(out, "{pad}{header} {{");
        self.stmts(out, body, depth + 1);
        let _ = writeln!(out, "{pad}}}");
    }

    fn if_chain(&mut self, out: &mut String, cond: &Expr, then: &[Stmt], otherwise: &[Stmt], depth: usize, prefix: &str) {
        let pad = "    ".repeat(depth);
        let c = self.expr(cond, Some("bool"));
        let _ = writeln!(out, "{pad}{prefix}if ({c}) {{");
        self.stmts(out, then, depth + 1);
        let _ = writeln!(out, "{pad}}}");
        match otherwise {
            [] => {}
            [Stmt::If { cond, then, otherwise }] => self.if_chain(out, cond, then, otherwise, depth, "else "),
            _ => self.block(out, "else", otherwise, depth),
        }
    }

    fn stmt(&mut self, out: &mut String, s: &Stmt, depth: usize) {
        let pad = "    ".repeat(depth);
        match s {
            Stmt::Call(c) => {
                let code = self.call(c, None);
                let _ = writeln!(out, "{pad}{code};");
            }
            Stmt::Set { variable, value, index } => {
                let ty = self.var_type(variable);
                let v = self.expr(value, ty.as_deref());
                let target = match index {
                    Some(i) => format!("{}[{}]", self.var(variable), self.expr(i, Some("int"))),
                    None => self.var(variable),
                };
                let _ = writeln!(out, "{pad}{target} = {v};");
            }
            Stmt::If { cond, then, otherwise } => self.if_chain(out, cond, then, otherwise, depth, ""),
            Stmt::While { cond, body } => {
                let c = self.expr(cond, Some("bool"));
                self.block(out, &format!("while ({c})"), body, depth);
            }
            Stmt::For { variable, from, to, body, step } => {
                self.ensure_loop_var(variable, "int");
                self.next_auto();
                let v = self.var(variable);
                let ty = self.var_type(variable).unwrap_or_else(|| "int".into());
                let ae = self.auto(&ty, "ae");
                let f = self.expr(from, Some(&ty));
                let t = self.expr(to, Some(&ty));
                let _ = writeln!(out, "{pad}{ae} = {t};");
                let _ = writeln!(out, "{pad}{v} = {f};");
                let literal_step = match step {
                    None => Some(1.0),
                    Some(Expr::Literal(Literal::Int(n))) => Some(*n as f64),
                    Some(Expr::Literal(Literal::Fixed(n))) => Some(*n),
                    _ => None,
                };
                let header = match (literal_step, step) {
                    (Some(n), _) => {
                        let inc = match step {
                            Some(e) => self.expr(e, Some(&ty)),
                            None if ty == "fixed" => "1.0".into(),
                            None => "1".into(),
                        };
                        let cmp = if n < 0.0 { ">=" } else { "<=" };
                        format!("for ( ; {v} {cmp} {ae} ; {v} += {inc} )")
                    }
                    (None, Some(e)) => {
                        let ai = self.auto(&ty, "ai");
                        let code = self.expr(e, Some(&ty));
                        let z = zero_value(&ty);
                        let _ = writeln!(out, "{pad}{ai} = {code};");
                        format!("for ( ; ( ({ai} >= {z} && {v} <= {ae}) || ({ai} < {z} && {v} >= {ae}) ) ; {v} += {ai} )")
                    }
                    (None, None) => unreachable!(),
                };
                self.block(out, &header, body, depth);
            }
            Stmt::ForEachUnit { variable, group, body } => {
                self.ensure_loop_var(variable, "unit");
                self.next_auto();
                let g = self.auto("unitgroup", "g");
                let u = self.auto("int", "u");
                let code = self.expr(group, Some("unitgroup"));
                let v = self.var(variable);
                let _ = writeln!(out, "{pad}{g} = {code};");
                let _ = writeln!(out, "{pad}{u} = UnitGroupCount({g}, c_unitCountAll);");
                let _ = writeln!(out, "{pad}for ( ; {u} > 0 ; {u} -= 1 ) {{");
                let _ = writeln!(out, "{pad}    {v} = UnitGroupUnit({g}, {u});");
                self.stmts(out, body, depth + 1);
                let _ = writeln!(out, "{pad}}}");
            }
            Stmt::ForEachPlayer { variable, group, body } => {
                self.ensure_loop_var(variable, "int");
                self.next_auto();
                let g = self.auto("playergroup", "g");
                let code = self.expr(group, Some("playergroup"));
                let v = self.var(variable);
                let _ = writeln!(out, "{pad}{g} = {code};");
                let _ = writeln!(out, "{pad}{v} = -1;");
                let _ = writeln!(out, "{pad}while (true) {{");
                let _ = writeln!(out, "{pad}    {v} = PlayerGroupNextPlayer({g}, {v});");
                let _ = writeln!(out, "{pad}    if ({v} < 0) {{ break; }}");
                self.stmts(out, body, depth + 1);
                let _ = writeln!(out, "{pad}}}");
            }
            Stmt::Switch { value, cases, default } => {
                self.next_auto();
                let ty = self.infer(value);
                let subject = match &ty {
                    Some(t) => {
                        let val = self.auto(t, "val");
                        let code = self.expr(value, Some(t));
                        let _ = writeln!(out, "{pad}{val} = {code};");
                        val
                    }
                    // Unknown type: compare the expression itself in each case.
                    None => self.expr(value, None),
                };
                if cases.is_empty() {
                    self.stmts(out, default, depth);
                    return;
                }
                for (n, c) in cases.iter().enumerate() {
                    let cv = self.expr(&c.value, ty.as_deref());
                    let kw = if n == 0 { "if" } else { "else if" };
                    self.block(out, &format!("{kw} ({subject} == {cv})"), &c.body, depth);
                }
                if !default.is_empty() {
                    self.block(out, "else", default, depth);
                }
            }
            Stmt::Break => {
                let _ = writeln!(out, "{pad}break;");
            }
            Stmt::Continue => {
                let _ = writeln!(out, "{pad}continue;");
            }
            Stmt::Wait(e) => {
                let code = self.expr(e, Some("fixed"));
                let _ = writeln!(out, "{pad}Wait({code}, c_timeGame);");
            }
            Stmt::Return => {
                let r = if self.function.is_some() { "return;" } else { "return true;" };
                let _ = writeln!(out, "{pad}{r}");
            }
            Stmt::ReturnValue(e) => {
                let ty = self.function.flatten().map(|t| t.galaxy_name().to_owned());
                let code = self.expr(e, ty.as_deref());
                let _ = writeln!(out, "{pad}return {code};");
            }
            Stmt::Comment(c) => {
                for line in c.lines() {
                    let _ = writeln!(out, "{pad}// {line}");
                }
            }
            Stmt::Raw(r) => {
                for line in r.lines() {
                    let _ = writeln!(out, "{pad}{line}");
                }
            }
            Stmt::Disabled(_) => {}
        }
    }

    /// Declarations and initialization for locals (`prefix` is `lv_`).
    fn local_decls(&mut self, out: &mut String, vars: &[Variable]) -> String {
        let mut init = String::new();
        for v in vars {
            let name = format!("lv_{}", var_ident(&v.name));
            match v.array_len {
                Some(n) => {
                    let _ = writeln!(out, "    {}[{n}] {name};", v.ty.galaxy_name());
                }
                None => {
                    let _ = writeln!(out, "    {} {name};", v.ty.galaxy_name());
                    let value = match &v.initial {
                        Some(e) => Some(self.expr(e, Some(v.ty.galaxy_name()))),
                        None => handle_init(&v.ty).map(str::to_owned),
                    };
                    if let Some(value) = value {
                        let _ = writeln!(init, "    {name} = {value};");
                    }
                }
            }
        }
        init
    }
}

/// Galaxy for one expression, outside any trigger (globals only).
pub fn expr(e: &Expr, lib: &TriggerLibrary) -> String {
    Gen::new(lib, None).expr(e, None)
}

fn comment_lines(o: &mut String, text: &str, pad: &str) {
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            let _ = writeln!(o, "{pad}//");
        } else {
            let _ = writeln!(o, "{pad}// {line}");
        }
    }
}

fn header(o: &mut String, title: &str) {
    let _ = writeln!(o, "{RULE}\n// {title}\n{RULE}");
}

fn line_count(s: &str) -> usize {
    s.bytes().filter(|b| *b == b'\n').count()
}

fn signature(f: &CustomFunction) -> String {
    let ret = f.returns.as_ref().map(|t| t.galaxy_name().to_owned()).unwrap_or_else(|| "void".into());
    let params: Vec<String> = f.params.iter().map(|p| format!("{} lp_{}", p.ty.galaxy_name(), var_ident(&p.name))).collect();
    format!("{ret} {} ({})", function_name(f), params.join(", "))
}

fn emit_function(o: &mut String, g: &Gen, f: &CustomFunction, sections: &mut Vec<(usize, TriggerId, String)>, keys: &mut Vec<String>) {
    sections.push((line_count(o) + 1, f.id, f.name.clone()));
    header(o, &format!("Function: {}", f.name));
    comment_lines(o, &f.comment, "");
    let mut fg = g.scoped(&f.params, &f.locals, Some(f.returns.as_ref()));
    let mut body = String::new();
    fg.stmts(&mut body, &f.actions, 1);
    if let Some(t) = &f.returns {
        if !matches!(f.actions.last(), Some(Stmt::ReturnValue(_))) {
            let _ = writeln!(body, "    return {};", zero_value(t.galaxy_name()));
        }
    }
    let mut decls = String::new();
    let init = fg.local_decls(&mut decls, &f.locals);
    let _ = writeln!(o, "{} {{", signature(f));
    if !decls.is_empty() {
        let _ = writeln!(o, "    // Variable Declarations\n{decls}");
    }
    if !fg.autos.is_empty() {
        let _ = writeln!(o, "    // Automatic Variable Declarations");
        for a in &fg.autos {
            let _ = writeln!(o, "    {a}");
        }
        o.push('\n');
    }
    if !init.is_empty() {
        let _ = writeln!(o, "    // Variable Initialization\n{init}");
    }
    let _ = writeln!(o, "    // Implementation\n{body}}}\n");
    keys.append(&mut fg.text_keys);
}

fn emit_trigger(o: &mut String, g: &Gen, t: &Trigger, sections: &mut Vec<(usize, TriggerId, String)>, keys: &mut Vec<String>) {
    sections.push((line_count(o) + 1, t.id, t.name.clone()));
    header(o, &format!("Trigger: {}", t.name));
    comment_lines(o, &t.comment, "");
    let name = ident(&t.name);
    let tv = trigger_var(t);
    let mut tg = g.scoped(&[], &t.locals, None);

    let mut conds = String::new();
    for c in &t.conditions {
        let code = tg.expr(c, Some("bool"));
        let _ = writeln!(conds, "        if (!({code})) {{\n            return false;\n        }}");
    }
    let mut body = String::new();
    tg.stmts(&mut body, &t.actions, 1);
    let mut decls = String::new();
    let init = tg.local_decls(&mut decls, &t.locals);

    let _ = writeln!(o, "bool gt_{name}_Func (bool testConds, bool runActions) {{");
    if !decls.is_empty() {
        let _ = writeln!(o, "    // Variable Declarations\n{decls}");
    }
    if !tg.autos.is_empty() {
        let _ = writeln!(o, "    // Automatic Variable Declarations");
        for a in &tg.autos {
            let _ = writeln!(o, "    {a}");
        }
        o.push('\n');
    }
    if !init.is_empty() {
        let _ = writeln!(o, "    // Variable Initialization\n{init}");
    }
    if !conds.is_empty() {
        let _ = writeln!(o, "    // Conditions\n    if (testConds) {{\n{conds}    }}\n");
    }
    let _ = writeln!(o, "    // Actions\n    if (!runActions) {{\n        return true;\n    }}\n");
    o.push_str(&body);
    let _ = writeln!(o, "    return true;\n}}\n");

    let _ = writeln!(o, "{RULE}");
    let _ = writeln!(o, "void gt_{name}_Init () {{");
    let _ = writeln!(o, "    {tv} = TriggerCreate(\"gt_{name}_Func\");");
    if !t.initially_on {
        let _ = writeln!(o, "    TriggerEnable({tv}, false);");
    }
    for e in &t.events {
        let code = tg.call(e, Some(&tv));
        let _ = writeln!(o, "    {code};");
    }
    let _ = writeln!(o, "}}\n");
    keys.append(&mut tg.text_keys);
}

/// Generate MapScript.galaxy for a document.
pub fn generate(doc: &MapDocument) -> String {
    generate_full(doc).script
}

/// Generate MapScript.galaxy plus a map of where each trigger starts and
/// which text keys it references.
pub fn generate_full(doc: &MapDocument) -> Generated {
    generate_lib(&doc.triggers, Some(doc), &doc.info.name)
}

/// Generate a script for a trigger library. With `doc`, placed-object
/// references resolve to their ids; without, they resolve by name at runtime.
pub fn generate_lib(lib: &TriggerLibrary, doc: Option<&MapDocument>, map_name: &str) -> Generated {
    let mut o = String::new();
    let mut sections = Vec::new();
    let mut keys = Vec::new();
    let g = Gen::new(lib, doc);

    let _ = writeln!(o, "{BANNER}\n//\n// Generated Map Script\n//\n// Name:   {map_name}\n// Generated by Strata\n//\n{BANNER}");
    let _ = writeln!(o, "include \"TriggerLibs/NativeLib\"\n");

    header(&mut o, "Library Initialization");
    let _ = writeln!(o, "void InitLibs () {{\n    libNtve_InitLib();\n}}\n");

    header(&mut o, "Global Variables");
    for v in &lib.variables {
        comment_lines(&mut o, &v.comment, "");
        match v.array_len {
            Some(n) => {
                let _ = writeln!(o, "{}[{n}] {};", v.ty.galaxy_name(), global_var(&v.name));
            }
            None => {
                let _ = writeln!(o, "{} {};", v.ty.galaxy_name(), global_var(&v.name));
            }
        }
    }
    if !lib.variables.is_empty() {
        o.push('\n');
    }
    let mut gg = Gen::new(lib, doc);
    let mut init = String::new();
    let mut needs_i = false;
    for v in &lib.variables {
        let value = match &v.initial {
            Some(e) => Some(gg.expr(e, Some(v.ty.galaxy_name()))),
            None => handle_init(&v.ty).map(str::to_owned),
        };
        let Some(value) = value else { continue };
        match v.array_len {
            Some(n) => {
                needs_i = true;
                let _ = writeln!(
                    init,
                    "    for (init_i = 0; init_i < {n}; init_i += 1) {{\n        {}[init_i] = {value};\n    }}",
                    global_var(&v.name)
                );
            }
            None => {
                let _ = writeln!(init, "    {} = {value};", global_var(&v.name));
            }
        }
    }
    keys.append(&mut gg.text_keys);
    let _ = writeln!(o, "void InitGlobals () {{");
    if needs_i {
        let _ = writeln!(o, "    int init_i;\n");
    }
    let _ = writeln!(o, "{init}}}\n");

    if !lib.custom_script.trim().is_empty() {
        header(&mut o, "Custom Script");
        let _ = writeln!(o, "{}\n", lib.custom_script.trim_end());
    }

    if !lib.functions.is_empty() {
        header(&mut o, "Function Declarations");
        for f in &lib.functions {
            let _ = writeln!(o, "{};", signature(f));
        }
        o.push('\n');
    }

    let triggers = lib.triggers();
    header(&mut o, "Trigger Variables");
    for t in &triggers {
        let _ = writeln!(o, "trigger {};", trigger_var(t));
    }
    o.push('\n');

    for f in &lib.functions {
        emit_function(&mut o, &g, f, &mut sections, &mut keys);
    }
    for t in &triggers {
        if t.enabled {
            emit_trigger(&mut o, &g, t, &mut sections, &mut keys);
        } else {
            sections.push((line_count(&o) + 1, t.id, t.name.clone()));
            let _ = writeln!(o, "{RULE}\n// Trigger: {} (disabled, not generated)\n{RULE}\n", t.name);
        }
    }

    header(&mut o, "Trigger Initialization");
    let _ = writeln!(o, "void InitTriggers () {{");
    for t in triggers.iter().filter(|t| t.enabled) {
        let _ = writeln!(o, "    gt_{}_Init();", ident(&t.name));
    }
    let _ = writeln!(o, "}}\n");
    header(&mut o, "Map Initialization");
    let _ = writeln!(o, "void InitMap () {{\n    InitLibs();\n    InitGlobals();\n    InitTriggers();\n}}");

    keys.sort();
    keys.dedup();
    Generated { script: o, sections, text_keys: keys }
}

// ---------------------------------------------------------------------------
// Linter
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LintSeverity {
    Error,
    Warning,
}

/// A lint finding. `line` and `col` are 1-based; `col` counts characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub line: usize,
    pub col: usize,
    pub message: String,
    pub severity: LintSeverity,
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Num,
    Str,
    Punct(&'static str),
}

#[derive(Debug, Clone)]
struct Token {
    tok: Tok,
    line: usize,
    col: usize,
}

const PUNCTS: &[&str] = &[
    "<<=", ">>=", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^=", "<<", ">>", "->", "(", ")", "{", "}",
    "[", "]", ";", ",", "+", "-", "*", "/", "%", "<", ">", "=", "!", "&", "|", "^", "~", ".", "?", ":", "#",
];

const KEYWORDS: &[&str] = &[
    "if",
    "else",
    "while",
    "for",
    "return",
    "break",
    "continue",
    "const",
    "static",
    "native",
    "include",
    "struct",
    "typedef",
    "true",
    "false",
    "null",
    "do",
    "breakpoint",
];

const TYPES: &[&str] = &[
    "void",
    "int",
    "fixed",
    "bool",
    "byte",
    "char",
    "string",
    "text",
    "point",
    "region",
    "unit",
    "unitgroup",
    "unitref",
    "unitfilter",
    "playergroup",
    "timer",
    "trigger",
    "sound",
    "soundlink",
    "color",
    "abilcmd",
    "actor",
    "actorscope",
    "aifilter",
    "bank",
    "bitmask",
    "camerainfo",
    "datetime",
    "doodad",
    "effecthistory",
    "generichandle",
    "marker",
    "order",
    "revealer",
    "transmissionsource",
    "wave",
    "waveinfo",
    "wavetarget",
    "handle",
    "arrayref",
    "structref",
    "funcref",
];

fn tokenize(src: &str, diags: &mut Vec<Diagnostic>) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut toks = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1usize, 1usize);
    let err = |diags: &mut Vec<Diagnostic>, line, col, m: &str| {
        diags.push(Diagnostic { line, col, message: m.into(), severity: LintSeverity::Error })
    };
    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            i += 1;
            line += 1;
            col = 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'*') {
            let (sl, sc) = (line, col);
            i += 2;
            col += 2;
            loop {
                if i >= chars.len() {
                    err(diags, sl, sc, "Comment is never closed. Add */.");
                    break;
                }
                if chars[i] == '*' && chars.get(i + 1) == Some(&'/') {
                    i += 2;
                    col += 2;
                    break;
                }
                if chars[i] == '\n' {
                    line += 1;
                    col = 1;
                } else {
                    col += 1;
                }
                i += 1;
            }
            continue;
        }
        let (sl, sc) = (line, col);
        if c == '"' || c == '\'' {
            i += 1;
            col += 1;
            let mut closed = false;
            while i < chars.len() && chars[i] != '\n' {
                if chars[i] == '\\' {
                    i += 2;
                    col += 2;
                    continue;
                }
                if chars[i] == c {
                    i += 1;
                    col += 1;
                    closed = true;
                    break;
                }
                i += 1;
                col += 1;
            }
            if !closed {
                err(diags, sl, sc, "String is not closed before the end of the line. Add a closing quote.");
            }
            toks.push(Token { tok: Tok::Str, line: sl, col: sc });
            continue;
        }
        if c.is_ascii_digit() {
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                i += 1;
                col += 1;
            }
            toks.push(Token { tok: Tok::Num, line: sl, col: sc });
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
                col += 1;
            }
            toks.push(Token { tok: Tok::Ident(chars[start..i].iter().collect()), line: sl, col: sc });
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 3)].iter().collect();
        match PUNCTS.iter().find(|p| rest.starts_with(**p)) {
            Some(p) => {
                i += p.len();
                col += p.len();
                toks.push(Token { tok: Tok::Punct(p), line: sl, col: sc });
            }
            None => {
                diags.push(Diagnostic {
                    line: sl,
                    col: sc,
                    message: format!("Unexpected character '{c}'."),
                    severity: LintSeverity::Error,
                });
                i += 1;
                col += 1;
            }
        }
    }
    toks
}

fn is_ident(t: &Token) -> Option<&str> {
    match &t.tok {
        Tok::Ident(s) => Some(s),
        _ => None,
    }
}

fn is_punct(t: Option<&Token>, p: &str) -> bool {
    matches!(t, Some(Token { tok: Tok::Punct(q), .. }) if *q == p)
}

/// Lint Galaxy source: unbalanced brackets, unterminated strings and
/// comments, missing semicolons (heuristic), declarations after statements,
/// and calls to functions that are neither natives nor defined in `src`
/// (warnings).
pub fn lint(src: &str) -> Vec<Diagnostic> {
    let mut diags = Vec::new();
    let toks = tokenize(src, &mut diags);

    // Brackets.
    let mut stack: Vec<&Token> = Vec::new();
    for t in &toks {
        let Tok::Punct(p) = &t.tok else { continue };
        match *p {
            "(" | "[" | "{" => stack.push(t),
            ")" | "]" | "}" => {
                let open = match *p {
                    ")" => "(",
                    "]" => "[",
                    _ => "{",
                };
                match stack.last() {
                    Some(o) if matches!(o.tok, Tok::Punct(q) if q == open) => {
                        stack.pop();
                    }
                    Some(o) => {
                        let Tok::Punct(q) = o.tok else { unreachable!() };
                        diags.push(Diagnostic {
                            line: t.line,
                            col: t.col,
                            message: format!("'{p}' does not match '{q}' opened on line {}.", o.line),
                            severity: LintSeverity::Error,
                        });
                        // Recover: drop the mismatched opener if it is a paren.
                        if q != "{" {
                            stack.pop();
                            if matches!(stack.last(), Some(o) if matches!(o.tok, Tok::Punct(q) if q == open)) {
                                stack.pop();
                            }
                        }
                    }
                    None => diags.push(Diagnostic {
                        line: t.line,
                        col: t.col,
                        message: format!("'{p}' has no matching '{open}'."),
                        severity: LintSeverity::Error,
                    }),
                }
            }
            _ => {}
        }
    }
    for o in stack {
        let Tok::Punct(q) = o.tok else { continue };
        diags.push(Diagnostic { line: o.line, col: o.col, message: format!("'{q}' is never closed."), severity: LintSeverity::Error });
    }

    // Function definitions and prototypes: `type name (` at brace depth 0.
    let mut defined = std::collections::HashSet::new();
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate() {
        match &t.tok {
            Tok::Punct("{") => depth += 1,
            Tok::Punct("}") => depth -= 1,
            Tok::Ident(name) if depth == 0 && is_punct(toks.get(i + 1), "(") && i > 0 && is_ident(&toks[i - 1]).is_some() => {
                defined.insert(name.clone());
            }
            _ => {}
        }
    }

    // Calls to unknown functions.
    for (i, t) in toks.iter().enumerate() {
        let Some(name) = is_ident(t) else { continue };
        if !is_punct(toks.get(i + 1), "(") || KEYWORDS.contains(&name) || TYPES.contains(&name) {
            continue;
        }
        if i > 0 && is_ident(&toks[i - 1]).is_some_and(|p| !KEYWORDS.contains(&p) || p == "native") {
            continue; // definition or prototype
        }
        if !defined.contains(name) && !trigger_lib::is_known_function(name) {
            diags.push(Diagnostic {
                line: t.line,
                col: t.col,
                message: format!("Unknown function \"{name}\". It is not a native and is not defined in this script."),
                severity: LintSeverity::Warning,
            });
        }
    }

    // Missing semicolons: a line ending in a value with no operator joining it
    // to the next line.
    let mut paren = 0i32;
    for (i, t) in toks.iter().enumerate() {
        match t.tok {
            Tok::Punct("(") | Tok::Punct("[") => paren += 1,
            Tok::Punct(")") | Tok::Punct("]") => paren -= 1,
            _ => {}
        }
        let next = toks.get(i + 1);
        if next.is_some_and(|n| n.line == t.line) || paren > 0 {
            continue;
        }
        let ends_value = matches!(t.tok, Tok::Ident(_) | Tok::Num | Tok::Str | Tok::Punct(")") | Tok::Punct("]"));
        if !ends_value {
            continue;
        }
        if let Some(n) = next {
            if let Tok::Punct(p) = n.tok {
                if p != "}" {
                    continue; // `{`, operators, `)`... continue the statement
                }
            }
        }
        // First token of this line decides whether it is a control header.
        let first = toks[..=i].iter().rev().take_while(|x| x.line == t.line).last().unwrap_or(t);
        if matches!(is_ident(first), Some("if" | "while" | "for" | "else" | "include")) || matches!(first.tok, Tok::Punct("#")) {
            continue;
        }
        if matches!(is_ident(t), Some("else")) {
            continue;
        }
        diags.push(Diagnostic {
            line: t.line,
            col: t.col + 1,
            message: "Missing semicolon at the end of this statement.".into(),
            severity: LintSeverity::Error,
        });
    }

    // Local declarations must come before statements in a function body.
    let mut depth = 0i32;
    let mut seen_stmt = false;
    let mut at_start = false;
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        match t.tok {
            Tok::Punct("{") => {
                if depth == 0 {
                    seen_stmt = false;
                }
                depth += 1;
                at_start = true;
                i += 1;
                continue;
            }
            Tok::Punct("}") => {
                depth -= 1;
                at_start = true;
                i += 1;
                continue;
            }
            Tok::Punct(";") => {
                at_start = true;
                i += 1;
                continue;
            }
            _ => {}
        }
        if depth > 0 && at_start {
            at_start = false;
            let mut j = i;
            if is_ident(&toks[j]) == Some("const") {
                j += 1;
            }
            let is_decl = toks.get(j).and_then(is_ident).is_some_and(|ty| TYPES.contains(&ty)) && {
                let mut k = j + 1;
                while is_punct(toks.get(k), "[") {
                    while k < toks.len() && !is_punct(toks.get(k), "]") {
                        k += 1;
                    }
                    k += 1;
                }
                toks.get(k).and_then(is_ident).is_some()
                    && (is_punct(toks.get(k + 1), "=") || is_punct(toks.get(k + 1), ";") || is_punct(toks.get(k + 1), "["))
            };
            if is_decl && seen_stmt {
                diags.push(Diagnostic {
                    line: t.line,
                    col: t.col,
                    message: "Galaxy needs local variables declared at the top of the function, before other statements.".into(),
                    severity: LintSeverity::Error,
                });
            } else if !is_decl {
                seen_stmt = true;
            }
        }
        i += 1;
    }

    diags.sort_by_key(|d| (d.line, d.col));
    diags.dedup();
    diags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::{ObjectId, PlacedObject};
    use crate::regions::Region;
    use crate::triggers::{SwitchCase, TriggerItem};

    fn lib_with(t: Trigger) -> TriggerLibrary {
        TriggerLibrary { next_id: 10, items: vec![TriggerItem::Trigger(t)], ..Default::default() }
    }

    #[test]
    fn melee_init_generates() {
        let doc = MapDocument::new("Test", 64, 64);
        let s = generate(&doc);
        assert!(s.contains("bool gt_MeleeInitialization_Func"));
        assert!(s.contains("TriggerAddEventMapInit(gt_MeleeInitialization);"));
        assert!(s.contains("MeleeInitResources();"));
        assert!(s.contains("void InitMap ()"));
        assert!(s.contains("void InitLibs () {\n    libNtve_InitLib();\n}"));
        assert!(lint(&s).is_empty(), "{:?}", lint(&s));
        assert_eq!(s, generate(&doc), "output is deterministic");
    }

    #[test]
    fn golden_small_trigger() {
        let mut t = Trigger::new(TriggerId(1), "Spawn Wave");
        t.comment = "Spawns marines.".into();
        t.events.push(Call { function: "TriggerAddEventTimeElapsed".into(), args: vec![Expr::int(5), Expr::Preset("c_timeGame".into())] });
        t.conditions.push(Expr::Binary { op: ">".into(), lhs: Box::new(Expr::Variable("Count".into())), rhs: Box::new(Expr::int(0)) });
        t.locals.push(Variable::new("i", GalaxyType::Int));
        t.actions.push(Stmt::For {
            variable: "i".into(),
            from: Expr::int(1),
            to: Expr::Variable("Count".into()),
            step: None,
            body: vec![Stmt::Call(Call {
                function: "UnitCreate".into(),
                args: vec![
                    Expr::int(1),
                    Expr::Literal(Literal::String("Marine".into())),
                    Expr::Preset("c_unitCreateIgnorePlacement".into()),
                    Expr::int(1),
                    Expr::call("Point", vec![Expr::int(10), Expr::fixed(12.5)]),
                    Expr::int(270),
                ],
            })],
        });
        t.actions.push(Stmt::Wait(Expr::int(2)));
        t.actions.push(Stmt::Call(Call {
            function: "UIDisplayMessage".into(),
            args: vec![
                Expr::call("PlayerGroupAll", vec![]),
                Expr::Preset("c_messageAreaSubtitle".into()),
                Expr::Literal(Literal::Text("Param/Value/0001".into())),
            ],
        }));
        let mut lib = lib_with(t);
        let mut count = Variable::new("Count", GalaxyType::Int);
        count.initial = Some(Expr::int(3));
        lib.variables.push(count);
        let out = generate_lib(&lib, None, "Golden");
        let expected = r#"//==================================================================================================
//
// Generated Map Script
//
// Name:   Golden
// Generated by Strata
//
//==================================================================================================
include "TriggerLibs/NativeLib"

//--------------------------------------------------------------------------------------------------
// Library Initialization
//--------------------------------------------------------------------------------------------------
void InitLibs () {
    libNtve_InitLib();
}

//--------------------------------------------------------------------------------------------------
// Global Variables
//--------------------------------------------------------------------------------------------------
int gv_count;

void InitGlobals () {
    gv_count = 3;
}

//--------------------------------------------------------------------------------------------------
// Trigger Variables
//--------------------------------------------------------------------------------------------------
trigger gt_SpawnWave;

//--------------------------------------------------------------------------------------------------
// Trigger: Spawn Wave
//--------------------------------------------------------------------------------------------------
// Spawns marines.
bool gt_SpawnWave_Func (bool testConds, bool runActions) {
    // Variable Declarations
    int lv_i;

    // Automatic Variable Declarations
    int auto1_ae;

    // Conditions
    if (testConds) {
        if (!((gv_count > 0))) {
            return false;
        }
    }

    // Actions
    if (!runActions) {
        return true;
    }

    auto1_ae = gv_count;
    lv_i = 1;
    for ( ; lv_i <= auto1_ae ; lv_i += 1 ) {
        UnitCreate(1, "Marine", c_unitCreateIgnorePlacement, 1, Point(10.0, 12.5), 270.0);
    }
    Wait(2.0, c_timeGame);
    UIDisplayMessage(PlayerGroupAll(), c_messageAreaSubtitle, StringExternal("Param/Value/0001"));
    return true;
}

//--------------------------------------------------------------------------------------------------
void gt_SpawnWave_Init () {
    gt_SpawnWave = TriggerCreate("gt_SpawnWave_Func");
    TriggerAddEventTimeElapsed(gt_SpawnWave, 5.0, c_timeGame);
}

//--------------------------------------------------------------------------------------------------
// Trigger Initialization
//--------------------------------------------------------------------------------------------------
void InitTriggers () {
    gt_SpawnWave_Init();
}

//--------------------------------------------------------------------------------------------------
// Map Initialization
//--------------------------------------------------------------------------------------------------
void InitMap () {
    InitLibs();
    InitGlobals();
    InitTriggers();
}
"#;
        assert_eq!(out.script, expected);
        assert_eq!(out.text_keys, vec!["Param/Value/0001".to_string()]);
        assert_eq!(out.sections.len(), 1);
        let line = out.sections[0].0;
        assert!(out.script.lines().nth(line - 1).unwrap().starts_with("//---"));
        assert_eq!(out.item_at_line(line + 5), Some(TriggerId(1)));
        assert!(lint(&out.script).is_empty(), "{:?}", lint(&out.script));
    }

    #[test]
    fn loops_switch_and_functions() {
        let mut t = Trigger::new(TriggerId(1), "Loops");
        t.initially_on = false;
        t.events.push(Call { function: "TriggerAddEventUnitDied".into(), args: vec![Expr::Preset("null".into())] });
        t.actions.push(Stmt::ForEachUnit {
            variable: "u".into(),
            group: Expr::call("UnitGroupEmpty", vec![]),
            body: vec![Stmt::If {
                cond: Expr::call("UnitIsAlive", vec![Expr::Variable("u".into())]),
                then: vec![Stmt::Continue],
                otherwise: vec![Stmt::Break],
            }],
        });
        t.actions.push(Stmt::ForEachPlayer { variable: "p".into(), group: Expr::call("PlayerGroupAll", vec![]), body: vec![] });
        t.actions.push(Stmt::Switch {
            value: Expr::call("EventPlayer", vec![]),
            cases: vec![
                SwitchCase { value: Expr::int(1), body: vec![Stmt::Comment("one".into())] },
                SwitchCase { value: Expr::int(2), body: vec![] },
            ],
            default: vec![Stmt::Return],
        });
        t.actions.push(Stmt::For { variable: "i".into(), from: Expr::int(10), to: Expr::int(0), step: Some(Expr::int(-2)), body: vec![] });
        t.actions.push(Stmt::For {
            variable: "j".into(),
            from: Expr::int(0),
            to: Expr::int(9),
            step: Some(Expr::Variable("i".into())),
            body: vec![],
        });
        t.actions
            .push(Stmt::Disabled(Box::new(Stmt::Call(Call { function: "UnitKill".into(), args: vec![Expr::call("EventUnit", vec![])] }))));
        t.actions.push(Stmt::Call(Call { function: "Double It".into(), args: vec![Expr::int(4)] }));
        let mut lib = lib_with(t);
        let mut f = CustomFunction::new(TriggerId(2), "Double It");
        f.params.push(Variable::new("Value", GalaxyType::Int));
        f.returns = Some(GalaxyType::Int);
        f.actions.push(Stmt::ReturnValue(Expr::Binary {
            op: "*".into(),
            lhs: Box::new(Expr::Variable("Value".into())),
            rhs: Box::new(Expr::int(2)),
        }));
        lib.functions.push(f);
        let mut off = Trigger::new(TriggerId(3), "Off");
        off.enabled = false;
        lib.items.push(TriggerItem::Trigger(off));

        let s = generate_lib(&lib, None, "x").script;
        assert!(s.contains("int gf_DoubleIt (int lp_value);"));
        assert!(s.contains("int gf_DoubleIt (int lp_value) {"));
        assert!(s.contains("return (lp_value * 2);"));
        assert!(s.contains("gf_DoubleIt(4);"));
        assert!(s.contains("TriggerEnable(gt_Loops, false);"));
        assert!(s.contains("TriggerAddEventUnitDied(gt_Loops, null);"));
        assert!(s.contains("    unit lv_u;\n"));
        assert!(s.contains("lv_u = UnitGroupUnit(auto1_g, auto1_u);"));
        assert!(s.contains("lv_p = PlayerGroupNextPlayer(auto2_g, lv_p);"));
        assert!(s.contains("int auto3_val;"));
        assert!(s.contains("else if (auto3_val == 2) {"));
        assert!(s.contains("for ( ; lv_i >= auto4_ae ; lv_i += -2 )"));
        assert!(s.contains("auto5_ai = lv_i;"));
        assert!(!s.contains("UnitKill"));
        assert!(s.contains("trigger gt_Off;"));
        assert!(!s.contains("gt_Off_Init();"));
        assert!(!s.contains("gt_Off_Func"));
        // Declarations stay ahead of statements.
        let func = &s[s.find("bool gt_Loops_Func").unwrap()..];
        assert!(func.find("int auto5_ai;").unwrap() < func.find("// Actions").unwrap());
        assert!(lint(&s).is_empty(), "{:?}\n{s}", lint(&s));
    }

    #[test]
    fn placed_objects_and_coercions() {
        let mut doc = MapDocument::new("P", 64, 64);
        let mut p = PlacedObject::new(ObjectId(7), ObjectKind::Point, "", glam::Vec3::ZERO);
        p.name = "Spawn".into();
        doc.objects.objects.push(p);
        let mut u = PlacedObject::new(ObjectId(8), ObjectKind::Unit, "Marine", glam::Vec3::ZERO);
        u.name = "Hero".into();
        doc.objects.objects.push(u);
        doc.regions.push(Region { id: ObjectId(9), name: "Base".into(), color: [0, 0, 0], shapes: vec![] });
        let mut t = Trigger::new(TriggerId(1), "T");
        t.events.push(Call {
            function: "TriggerAddEventUnitRegion".into(),
            args: vec![Expr::Placed("Hero".into()), Expr::Placed("Base".into()), Expr::Literal(Literal::Bool(true))],
        });
        t.actions.push(Stmt::Call(Call {
            function: "UnitSetPosition".into(),
            args: vec![Expr::Placed("Hero".into()), Expr::Placed("Spawn".into()), Expr::Literal(Literal::Bool(false))],
        }));
        t.actions.push(Stmt::Call(Call {
            function: "UIDisplayMessage".into(),
            args: vec![Expr::unset(), Expr::unset(), Expr::Literal(Literal::String("hi".into()))],
        }));
        t.locals.push(Variable::new("n", GalaxyType::Int));
        t.locals.push(Variable::new("label", GalaxyType::Text));
        t.actions.push(Stmt::Wait(Expr::Variable("n".into())));
        t.actions.push(Stmt::Set { variable: "label".into(), value: Expr::Variable("n".into()), index: None });
        doc.triggers = lib_with(t);
        let s = generate(&doc);
        assert!(s.contains("Wait(IntToFixed(lv_n), c_timeGame);"), "{s}");
        assert!(s.contains("lv_label = IntToText(lv_n);"), "{s}");
        assert!(s.contains("TriggerAddEventUnitRegion(gt_T, UnitRefFromUnit(UnitFromId(8)), RegionFromId(9), true);"), "{s}");
        assert!(s.contains("UnitSetPosition(UnitFromId(8), PointFromId(7), false);"));
        assert!(s.contains("UIDisplayMessage(null, 0, StringToText(\"hi\"));"));
        // Without a document, names resolve at runtime.
        let s = generate_lib(&doc.triggers, None, "x").script;
        assert!(s.contains("RegionFromName(\"Base\")"));
    }

    #[test]
    fn fixed_literals() {
        assert_eq!(fixed_literal(1.0), "1.0");
        assert_eq!(fixed_literal(0.5), "0.5");
        assert_eq!(fixed_literal(-2.25), "-2.25");
        assert_eq!(fixed_literal(1e-9), "0.0");
        assert_eq!(fixed_literal(1.0 / 3.0), "0.3333");
        assert_eq!(fixed_literal(123456.0), "123456.0");
        assert_eq!(fixed_literal(f64::NAN), "0.0");
    }

    #[test]
    fn var_idents() {
        assert_eq!(var_ident("Enemy Unit Types"), "enemyUnitTypes");
        assert_eq!(global_var("Count"), "gv_count");
        assert_eq!(ident("2 fast"), "_2fast");
    }

    #[test]
    fn lint_finds_problems() {
        let src = "void f () {\n    int a = 1;\n    a = 2\n    UnitKil(null);\n    int b;\n    g(\"oops);\n}\n";
        let d = lint(src);
        let msgs: Vec<_> = d.iter().map(|d| (d.line, d.message.as_str())).collect();
        assert!(msgs.iter().any(|(l, m)| *l == 3 && m.contains("semicolon")), "{msgs:?}");
        assert!(msgs.iter().any(|(l, m)| *l == 4 && m.contains("UnitKil")), "{msgs:?}");
        assert!(msgs.iter().any(|(l, m)| *l == 5 && m.contains("top of the function")), "{msgs:?}");
        assert!(msgs.iter().any(|(l, m)| *l == 6 && m.contains("String is not closed")), "{msgs:?}");
        assert!(d.iter().any(|d| d.message.contains("never closed") || d.message.contains("does not match")), "{msgs:?}");
    }

    #[test]
    fn lint_brackets_and_defined_functions() {
        let d = lint("void helper (int x) {\n}\nvoid g () {\n    helper((1);\n}\n");
        assert!(d.iter().any(|d| d.severity == LintSeverity::Error), "{d:?}");
        let ok = lint("int helper (int x);\nint helper (int x) {\n    return x;\n}\nvoid g () {\n    helper(1);\n    UnitKill(null);\n}\n");
        assert!(ok.is_empty(), "{ok:?}");
        let d = lint("void g () {\n}\n}\n");
        assert_eq!(d.len(), 1);
        assert!(d[0].message.contains("no matching"));
    }
}
