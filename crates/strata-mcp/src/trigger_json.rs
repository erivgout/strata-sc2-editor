//! The trigger JSON accepted by `create_trigger` / `update_trigger`.
//!
//! Either the full model (`strata_core::triggers::Trigger` as serde JSON) or a
//! simplified form written for people and AI clients:
//!
//! ```json
//! {"name": "Spawn wave",
//!  "events": [{"function": "TriggerAddEventTimePeriodic", "args": [30.0, "c_timeGame"]}],
//!  "conditions": ["gv_wave < 10"],
//!  "actions": [
//!    {"function": "UnitCreate", "args": [5, "\"Zergling\"", 0, 2, {"point": "Spawn"}, 270.0]},
//!    {"set": "wave", "value": "gv_wave + 1"},
//!    {"if": "gv_wave == 10", "then": [{"function": "UIDisplayMessage", "args": ["PlayerGroupAll()", "c_messageAreaSubtitle", {"text": "Msg/Final"}]}]}
//!  ]}
//! ```
//!
//! Strings are Galaxy source: integers, decimals, `true`/`false`, `c_*`
//! presets and `"quoted"` strings become structured values; anything else is
//! kept verbatim. Objects pick a specific form (see [`parse_expr`]).

use serde_json::{Map, Value};
use strata_core::triggers::{Call, Expr, GalaxyType, Literal, Stmt, Trigger, TriggerId};

const MODEL_EXPR_KEYS: [&str; 8] = [
    "Literal", "Variable", "Call", "Preset", "Placed", "Binary", "Not", "Raw",
];
const MODEL_STMT_KEYS: [&str; 9] = [
    "Call", "Set", "If", "While", "For", "Wait", "Return", "Comment", "Raw",
];

fn single_key<'a>(o: &'a Map<String, Value>, keys: &[&str]) -> Option<&'a str> {
    if o.len() == 1 {
        let k = o.keys().next()?;
        keys.contains(&k.as_str()).then_some(k.as_str())
    } else {
        None
    }
}

fn is_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !s.starts_with(|c: char| c.is_ascii_digit())
}

/// A Galaxy expression written as source text.
fn galaxy_token(s: &str) -> Expr {
    let t = s.trim();
    if t == "true" || t == "false" {
        return Expr::Literal(Literal::Bool(t == "true"));
    }
    if let Ok(i) = t.parse::<i64>() {
        return Expr::Literal(Literal::Int(i));
    }
    if t.contains('.')
        && t.chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '-')
    {
        if let Ok(f) = t.parse::<f64>() {
            return Expr::Literal(Literal::Fixed(f));
        }
    }
    if t.starts_with("c_") && is_ident(t) {
        return Expr::Preset(t.to_owned());
    }
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        let inner = &t[1..t.len() - 1];
        let mut out = String::new();
        let mut chars = inner.chars();
        let mut simple = true;
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some(c @ ('"' | '\\')) => out.push(c),
                    _ => simple = false,
                },
                '"' => simple = false,
                c => out.push(c),
            }
        }
        if simple {
            return Expr::Literal(Literal::String(out));
        }
    }
    Expr::Raw(t.to_owned())
}

fn args(o: &Map<String, Value>) -> Result<Vec<Expr>, String> {
    match o.get("args") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(parse_expr).collect(),
        Some(_) => Err("args must be an array".into()),
    }
}

fn str_field<'a>(o: &'a Map<String, Value>, k: &str) -> Result<&'a str, String> {
    o.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("\"{k}\" must be a string"))
}

/// Parse an expression. Accepted forms:
/// - number (integer -> int, otherwise fixed), `true` / `false`
/// - string: Galaxy source, e.g. `"c_playerAny"`, `"\"text\""`, `"gv_score + 1"`
/// - `{"string": "hi"}`, `{"text": "String/Key"}`, `{"int": 3}`, `{"fixed": 1.5}`, `{"bool": true}`
/// - `{"var": "Score"}` (global or local variable by editor name)
/// - `{"preset": "c_unitCreateIgnorePlacement"}`, `{"point": "Start"}` (placed point by name), `{"raw": "..."}`
/// - `{"function": "UnitGetOwner", "args": [...]}` (a call)
/// - `{"op": "==", "lhs": ..., "rhs": ...}`, `{"not": ...}`
/// - the model form, e.g. `{"Preset": "c_playerAny"}`
pub fn parse_expr(v: &Value) -> Result<Expr, String> {
    match v {
        Value::Bool(b) => Ok(Expr::Literal(Literal::Bool(*b))),
        Value::Number(n) => Ok(match n.as_i64() {
            Some(i) => Expr::Literal(Literal::Int(i)),
            None => Expr::Literal(Literal::Fixed(n.as_f64().unwrap_or(0.0))),
        }),
        Value::String(s) => Ok(galaxy_token(s)),
        Value::Object(o) => {
            if single_key(o, &MODEL_EXPR_KEYS).is_some() {
                return serde_json::from_value(v.clone())
                    .map_err(|e| format!("invalid model expression {v}: {e}"));
            }
            if o.contains_key("function") || o.contains_key("call") {
                return Ok(Expr::Call(parse_call(v)?));
            }
            if let Some(op) = o.get("op") {
                let op = op
                    .as_str()
                    .ok_or("\"op\" must be a string such as \"==\" or \"+\"")?;
                const OPS: [&str; 13] = [
                    "+", "-", "*", "/", "%", "==", "!=", "<", "<=", ">", ">=", "&&", "||",
                ];
                if !OPS.contains(&op) {
                    return Err(format!(
                        "unknown operator \"{op}\"; use one of {}",
                        OPS.join(" ")
                    ));
                }
                let side = |k: &str| {
                    o.get(k)
                        .ok_or(format!("binary \"{op}\" needs \"{k}\""))
                        .and_then(parse_expr)
                };
                return Ok(Expr::Binary {
                    op: op.to_owned(),
                    lhs: Box::new(side("lhs")?),
                    rhs: Box::new(side("rhs")?),
                });
            }
            let (k, val) = match o.iter().next() {
                Some(kv) if o.len() == 1 => kv,
                _ => {
                    return Err(format!(
                    "cannot read expression {v}; see the create_trigger description for the forms"
                ))
                }
            };
            let s = || {
                val.as_str()
                    .map(str::to_owned)
                    .ok_or(format!("\"{k}\" must be a string"))
            };
            Ok(match k.as_str() {
                "string" => Expr::Literal(Literal::String(s()?)),
                "text" => Expr::Literal(Literal::Text(s()?)),
                "int" => Expr::Literal(Literal::Int(
                    val.as_i64().ok_or("\"int\" must be an integer")?,
                )),
                "fixed" => Expr::Literal(Literal::Fixed(
                    val.as_f64().ok_or("\"fixed\" must be a number")?,
                )),
                "bool" => Expr::Literal(Literal::Bool(
                    val.as_bool().ok_or("\"bool\" must be true or false")?,
                )),
                "var" | "variable" => Expr::Variable(s()?),
                "preset" => Expr::Preset(s()?),
                "point" | "placed" => Expr::Placed(s()?),
                "raw" | "galaxy" => Expr::Raw(s()?),
                "not" => Expr::Not(Box::new(parse_expr(val)?)),
                other => {
                    return Err(format!(
                        "unknown expression form \"{other}\"; see the create_trigger description"
                    ))
                }
            })
        }
        Value::Null => Err("expressions cannot be null".into()),
        Value::Array(_) => Err(
            "expressions cannot be arrays; wrap calls as {\"function\": ..., \"args\": [...]}"
                .into(),
        ),
    }
}

/// A call: `{"function": "Name", "args": [...]}` or a bare function name.
pub fn parse_call(v: &Value) -> Result<Call, String> {
    match v {
        Value::String(s) if is_ident(s.trim()) => Ok(Call { function: s.trim().to_owned(), args: Vec::new() }),
        Value::String(s) => Err(format!("\"{s}\" is not a function name; write calls as {{\"function\": \"Name\", \"args\": [...]}}")),
        Value::Object(o) => {
            let f = o.get("function").or_else(|| o.get("call")).and_then(Value::as_str).ok_or("a call needs \"function\": \"Name\"")?;
            if !is_ident(f) {
                return Err(format!("\"{f}\" is not a valid Galaxy function name"));
            }
            Ok(Call { function: f.to_owned(), args: args(o)? })
        }
        _ => Err(format!("cannot read call {v}")),
    }
}

fn block(v: Option<&Value>, what: &str) -> Result<Vec<Stmt>, String> {
    match v {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a.iter().map(parse_stmt).collect(),
        Some(one @ (Value::Object(_) | Value::String(_))) => Ok(vec![parse_stmt(one)?]),
        Some(_) => Err(format!("\"{what}\" must be an array of actions")),
    }
}

/// Parse an action. Accepted forms:
/// - `{"function": "Name", "args": [...]}`
/// - `{"set": "Variable", "value": expr}`
/// - `{"if": cond, "then": [...], "else": [...]}`
/// - `{"while": cond, "do": [...]}`
/// - `{"for": "i", "from": 1, "to": 10, "do": [...]}`
/// - `{"wait": seconds}`, `{"comment": "..."}`, `{"raw": "Galaxy;"}`, `"return"`
/// - a string of Galaxy statements (kept verbatim; `;` added if missing)
/// - the model form, e.g. `{"Comment": "hi"}`
pub fn parse_stmt(v: &Value) -> Result<Stmt, String> {
    match v {
        Value::String(s) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("return") || t == "return;" {
                return Ok(Stmt::Return);
            }
            if is_ident(t) {
                return Ok(Stmt::Call(Call {
                    function: t.to_owned(),
                    args: Vec::new(),
                }));
            }
            let mut raw = t.to_owned();
            if !raw.ends_with(';') && !raw.ends_with('}') {
                raw.push(';');
            }
            Ok(Stmt::Raw(raw))
        }
        Value::Object(o) => {
            if single_key(o, &MODEL_STMT_KEYS).is_some() {
                return serde_json::from_value(v.clone())
                    .map_err(|e| format!("invalid model action {v}: {e}"));
            }
            if o.contains_key("function") || o.contains_key("call") {
                return Ok(Stmt::Call(parse_call(v)?));
            }
            if let Some(var) = o.get("set") {
                let variable = var
                    .as_str()
                    .ok_or("\"set\" must be a variable name")?
                    .to_owned();
                let value = parse_expr(o.get("value").ok_or("\"set\" needs \"value\"")?)?;
                let index = o.get("index").map(parse_expr).transpose()?;
                return Ok(Stmt::Set { variable, value, index });
            }
            if let Some(cond) = o.get("if") {
                return Ok(Stmt::If {
                    cond: parse_expr(cond)?,
                    then: block(o.get("then"), "then")?,
                    otherwise: block(o.get("else"), "else")?,
                });
            }
            if let Some(cond) = o.get("while") {
                return Ok(Stmt::While {
                    cond: parse_expr(cond)?,
                    body: block(o.get("do"), "do")?,
                });
            }
            if let Some(var) = o.get("for") {
                let variable = var
                    .as_str()
                    .ok_or("\"for\" must be a variable name")?
                    .to_owned();
                let from = parse_expr(o.get("from").ok_or("\"for\" needs \"from\"")?)?;
                let to = parse_expr(o.get("to").ok_or("\"for\" needs \"to\"")?)?;
                let step = o.get("step").map(parse_expr).transpose()?;
                return Ok(Stmt::For {
                    variable,
                    from,
                    to,
                    step,
                    body: block(o.get("do"), "do")?,
                });
            }
            if let Some(w) = o.get("wait") {
                return Ok(Stmt::Wait(parse_expr(w)?));
            }
            if o.contains_key("comment") {
                return Ok(Stmt::Comment(str_field(o, "comment")?.to_owned()));
            }
            if o.contains_key("raw") {
                return Ok(Stmt::Raw(str_field(o, "raw")?.to_owned()));
            }
            if o.contains_key("return") {
                return Ok(Stmt::Return);
            }
            Err(format!(
                "cannot read action {v}; see the create_trigger description for the forms"
            ))
        }
        _ => Err(format!("cannot read action {v}")),
    }
}

fn list<T>(
    o: &Map<String, Value>,
    k: &str,
    f: impl Fn(&Value) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    match o.get(k) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .enumerate()
            .map(|(i, v)| f(v).map_err(|e| format!("{k}[{i}]: {e}")))
            .collect(),
        Some(one) => Ok(vec![f(one).map_err(|e| format!("{k}: {e}"))?]),
    }
}

/// Parse a trigger in model or simplified form. `default_name` fills in a
/// missing name (update_trigger keeps the old name by default).
pub fn parse_trigger(v: &Value, default_name: Option<&str>) -> Result<Trigger, String> {
    let o = v.as_object().ok_or("trigger must be a JSON object")?;
    let looks_like_model =
        o.contains_key("initially_on") && o.contains_key("id") && o.contains_key("comment");
    if looks_like_model {
        if let Ok(t) = serde_json::from_value::<Trigger>(v.clone()) {
            return Ok(t);
        }
    }
    const KEYS: [&str; 8] = [
        "name",
        "comment",
        "enabled",
        "initially_on",
        "events",
        "conditions",
        "actions",
        "id",
    ];
    if let Some(k) = o.keys().find(|k| !KEYS.contains(&k.as_str())) {
        return Err(format!(
            "unknown trigger field \"{k}\"; use {}",
            KEYS[..7].join(", ")
        ));
    }
    let name = match o.get("name") {
        Some(Value::String(s)) if !s.trim().is_empty() => s.clone(),
        None | Some(Value::Null) if default_name.is_some() => {
            default_name.unwrap_or_default().to_owned()
        }
        _ => return Err("trigger needs a \"name\"".into()),
    };
    let flag = |k: &str| match o.get(k) {
        None | Some(Value::Null) => Ok(true),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("\"{k}\" must be true or false")),
    };
    let mut t = Trigger::new(TriggerId(0), name);
    t.comment = o
        .get("comment")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    t.enabled = flag("enabled")?;
    t.initially_on = flag("initially_on")?;
    t.events = list(o, "events", parse_call)?;
    t.conditions = list(o, "conditions", parse_expr)?;
    t.actions = list(o, "actions", parse_stmt)?;
    Ok(t)
}

/// "int", "unitgroup", "actor"... -> [`GalaxyType`].
pub fn parse_type(s: &str) -> GalaxyType {
    match s.trim().to_ascii_lowercase().as_str() {
        "int" | "integer" => GalaxyType::Int,
        "fixed" | "real" | "float" => GalaxyType::Fixed,
        "bool" | "boolean" => GalaxyType::Bool,
        "string" => GalaxyType::String,
        "text" => GalaxyType::Text,
        "point" => GalaxyType::Point,
        "region" => GalaxyType::Region,
        "unit" => GalaxyType::Unit,
        "unitgroup" | "unit_group" => GalaxyType::UnitGroup,
        "playergroup" | "player_group" => GalaxyType::PlayerGroup,
        "timer" => GalaxyType::Timer,
        "trigger" => GalaxyType::Trigger,
        "sound" => GalaxyType::Sound,
        "color" => GalaxyType::Color,
        _ => GalaxyType::Other(s.trim().to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tokens() {
        assert_eq!(galaxy_token("5"), Expr::Literal(Literal::Int(5)));
        assert_eq!(galaxy_token("-2.5"), Expr::Literal(Literal::Fixed(-2.5)));
        assert_eq!(
            galaxy_token("c_playerAny"),
            Expr::Preset("c_playerAny".into())
        );
        assert_eq!(
            galaxy_token("\"Zergling\""),
            Expr::Literal(Literal::String("Zergling".into()))
        );
        assert_eq!(
            galaxy_token("\"a\" + \"b\""),
            Expr::Raw("\"a\" + \"b\"".into())
        );
        assert_eq!(
            galaxy_token("PlayerGroupAll()"),
            Expr::Raw("PlayerGroupAll()".into())
        );
    }

    #[test]
    fn simplified_trigger() {
        let t = parse_trigger(
            &json!({
                "name": "Spawn",
                "events": [{"function": "TriggerAddEventTimePeriodic", "args": [30.0, "c_timeGame"]}, "TriggerAddEventMapInit"],
                "conditions": ["gv_wave < 10", {"op": "==", "lhs": {"var": "Ready"}, "rhs": true}],
                "actions": [
                    {"function": "UnitCreate", "args": [5, "\"Zergling\"", 0, 2, {"point": "Spawn"}, 270.0]},
                    {"set": "wave", "value": "gv_wave + 1"},
                    {"if": "gv_wave == 10", "then": [{"comment": "last"}], "else": "return"},
                    {"for": "i", "from": 1, "to": 3, "do": [{"wait": 1.0}]},
                    "UnitKill(gv_boss)",
                    {"Comment": "model form"}
                ]
            }),
            None,
        )
        .unwrap();
        assert_eq!(t.events.len(), 2);
        assert_eq!(t.events[0].args[1], Expr::Preset("c_timeGame".into()));
        assert_eq!(t.actions.len(), 6);
        assert_eq!(t.actions[4], Stmt::Raw("UnitKill(gv_boss);".into()));
        assert_eq!(t.actions[5], Stmt::Comment("model form".into()));
        let Stmt::Call(c) = &t.actions[0] else {
            panic!()
        };
        assert_eq!(c.args[4], Expr::Placed("Spawn".into()));
        assert_eq!(c.args[5], Expr::Literal(Literal::Fixed(270.0)));

        let mut doc = strata_core::MapDocument::new("t", 32, 32);
        doc.triggers
            .items
            .push(strata_core::triggers::TriggerItem::Trigger(t));
        let g = strata_core::galaxy::generate(&doc);
        assert!(
            g.contains("UnitCreate(5, \"Zergling\", 0, 2, PointFromName(\"Spawn\"), 270.0);"),
            "{g}"
        );
        assert!(
            g.contains("TriggerAddEventTimePeriodic(gt_Spawn, 30.0, c_timeGame);"),
            "{g}"
        );
    }

    #[test]
    fn model_trigger_and_errors() {
        let model = serde_json::to_value(Trigger::new(TriggerId(4), "M")).unwrap();
        assert_eq!(parse_trigger(&model, None).unwrap().name, "M");
        assert!(parse_trigger(&json!({"name": "x", "actoins": []}), None)
            .unwrap_err()
            .contains("actoins"));
        assert!(parse_trigger(&json!({"events": []}), None)
            .unwrap_err()
            .contains("name"));
        assert_eq!(
            parse_trigger(&json!({"actions": []}), Some("Old"))
                .unwrap()
                .name,
            "Old"
        );
        let e = parse_trigger(
            &json!({"name": "x", "actions": [{"if": "a", "then": [{"bogus": 1}]}]}),
            None,
        )
        .unwrap_err();
        assert!(e.starts_with("actions[0]"), "{e}");
        assert!(parse_expr(&json!({"op": "<>", "lhs": 1, "rhs": 2})).is_err());
        assert_eq!(parse_type("UnitGroup"), GalaxyType::UnitGroup);
        assert_eq!(parse_type("actor"), GalaxyType::Other("actor".into()));
    }
}
