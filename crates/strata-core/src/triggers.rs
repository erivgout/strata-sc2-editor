//! Trigger library: triggers, variables, custom functions and custom script,
//! organized in folders.
//!
//! The model mirrors how map makers think (event / condition / action) and is
//! compiled to Galaxy by [`crate::galaxy`]. Function names in [`Call`] are
//! Galaxy names (`UnitCreate`, `libNtve_gf_KillingUnit`) or the editor name of
//! a [`CustomFunction`]; [`crate::trigger_lib`] describes the natives.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TriggerId(pub u32);

/// Galaxy value types exposed in the trigger editor.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GalaxyType {
    Int,
    Fixed,
    Bool,
    String,
    Text,
    Point,
    Region,
    Unit,
    UnitGroup,
    PlayerGroup,
    Timer,
    Trigger,
    Sound,
    Color,
    /// Any other Galaxy type name, e.g. "actor" or "dialog".
    Other(String),
}

impl GalaxyType {
    /// Types offered when creating a variable, in menu order.
    pub fn common() -> Vec<GalaxyType> {
        use GalaxyType::*;
        let mut v = vec![Int, Fixed, Bool, String, Text, Point, Region, Unit, UnitGroup, PlayerGroup, Timer, Trigger, Sound, Color];
        for o in ["abilcmd", "actor", "bank", "camerainfo", "order", "revealer", "soundlink", "wave"] {
            v.push(Other(o.into()));
        }
        v
    }

    pub fn galaxy_name(&self) -> &str {
        match self {
            GalaxyType::Int => "int",
            GalaxyType::Fixed => "fixed",
            GalaxyType::Bool => "bool",
            GalaxyType::String => "string",
            GalaxyType::Text => "text",
            GalaxyType::Point => "point",
            GalaxyType::Region => "region",
            GalaxyType::Unit => "unit",
            GalaxyType::UnitGroup => "unitgroup",
            GalaxyType::PlayerGroup => "playergroup",
            GalaxyType::Timer => "timer",
            GalaxyType::Trigger => "trigger",
            GalaxyType::Sound => "sound",
            GalaxyType::Color => "color",
            GalaxyType::Other(s) => s,
        }
    }

    /// Parse a Galaxy type name. Unknown names become [`GalaxyType::Other`].
    pub fn from_name(name: &str) -> GalaxyType {
        match name {
            "int" => GalaxyType::Int,
            "fixed" => GalaxyType::Fixed,
            "bool" => GalaxyType::Bool,
            "string" => GalaxyType::String,
            "text" => GalaxyType::Text,
            "point" => GalaxyType::Point,
            "region" => GalaxyType::Region,
            "unit" => GalaxyType::Unit,
            "unitgroup" => GalaxyType::UnitGroup,
            "playergroup" => GalaxyType::PlayerGroup,
            "timer" => GalaxyType::Timer,
            "trigger" => GalaxyType::Trigger,
            "sound" => GalaxyType::Sound,
            "color" => GalaxyType::Color,
            other => GalaxyType::Other(other.into()),
        }
    }

    /// Human label for menus ("Unit group").
    pub fn label(&self) -> String {
        match self {
            GalaxyType::Fixed => "Real".into(),
            GalaxyType::Int => "Integer".into(),
            GalaxyType::Bool => "Boolean".into(),
            GalaxyType::UnitGroup => "Unit group".into(),
            GalaxyType::PlayerGroup => "Player group".into(),
            other => {
                let n = other.galaxy_name();
                let mut c = n.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Literal {
    Int(i64),
    Fixed(f64),
    Bool(bool),
    String(String),
    /// Reference to a localized string key; compiles to `StringExternal("key")`.
    Text(String),
}

/// A call to a native or library function, e.g. `UnitCreate(...)`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Call {
    pub function: String,
    pub args: Vec<Expr>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Expr {
    Literal(Literal),
    /// Global, local or parameter variable by editor name.
    Variable(String),
    Call(Call),
    /// Named preset constant, e.g. `c_unitCreateIgnorePlacement`.
    Preset(String),
    /// Reference to a placed point/unit/region/camera by editor name.
    Placed(String),
    /// Binary operator: `lhs op rhs` with op one of + - * / % == != < <= > >= && ||.
    Binary {
        op: String,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Not(Box<Expr>),
    /// Raw Galaxy expression (escape hatch). An empty string means "not set yet".
    Raw(String),
    /// A trigger in this library; compiles to its `gt_` variable.
    Trigger(TriggerId),
    /// Element of an array variable: `variable[index]`.
    Element {
        variable: String,
        index: Box<Expr>,
    },
}

impl Expr {
    /// The "not chosen yet" placeholder.
    pub fn unset() -> Expr {
        Expr::Raw(String::new())
    }

    pub fn is_unset(&self) -> bool {
        matches!(self, Expr::Raw(r) if r.trim().is_empty())
    }

    pub fn int(v: i64) -> Expr {
        Expr::Literal(Literal::Int(v))
    }

    pub fn fixed(v: f64) -> Expr {
        Expr::Literal(Literal::Fixed(v))
    }

    pub fn call(function: impl Into<String>, args: Vec<Expr>) -> Expr {
        Expr::Call(Call { function: function.into(), args })
    }

    /// Visit this expression and every nested one, depth first.
    pub fn walk(&self, f: &mut dyn FnMut(&Expr)) {
        f(self);
        match self {
            Expr::Call(c) => c.args.iter().for_each(|a| a.walk(f)),
            Expr::Binary { lhs, rhs, .. } => {
                lhs.walk(f);
                rhs.walk(f);
            }
            Expr::Not(e) => e.walk(f),
            Expr::Element { index, .. } => index.walk(f),
            _ => {}
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SwitchCase {
    pub value: Expr,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Call(Call),
    /// `variable = value`, or `variable[index] = value` for arrays.
    Set {
        variable: String,
        value: Expr,
        #[serde(default)]
        index: Option<Expr>,
    },
    If {
        cond: Expr,
        then: Vec<Stmt>,
        otherwise: Vec<Stmt>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    /// `for variable = from to to step step` (inclusive; step defaults to 1).
    For {
        variable: String,
        from: Expr,
        to: Expr,
        body: Vec<Stmt>,
        #[serde(default)]
        step: Option<Expr>,
    },
    /// Run `body` once per unit in `group`, with `variable` set to the unit.
    ForEachUnit {
        variable: String,
        group: Expr,
        body: Vec<Stmt>,
    },
    /// Run `body` once per player in `group`, with `variable` set to the player.
    ForEachPlayer {
        variable: String,
        group: Expr,
        body: Vec<Stmt>,
    },
    /// Compare `value` against each case in order; first match runs.
    Switch {
        value: Expr,
        cases: Vec<SwitchCase>,
        default: Vec<Stmt>,
    },
    Break,
    Continue,
    /// Wait N game seconds.
    Wait(Expr),
    /// Leave the trigger (or a void custom function).
    Return,
    /// Return a value from a custom function.
    ReturnValue(Expr),
    Comment(String),
    /// Raw Galaxy statement(s).
    Raw(String),
    /// A statement kept in the editor but left out of the generated script.
    Disabled(Box<Stmt>),
}

impl Stmt {
    /// Nested statement lists (for blocks), in display order.
    pub fn children(&self) -> Vec<&Vec<Stmt>> {
        match self {
            Stmt::If { then, otherwise, .. } => vec![then, otherwise],
            Stmt::While { body, .. } | Stmt::For { body, .. } | Stmt::ForEachUnit { body, .. } | Stmt::ForEachPlayer { body, .. } => {
                vec![body]
            }
            Stmt::Switch { cases, default, .. } => {
                let mut v: Vec<&Vec<Stmt>> = cases.iter().map(|c| &c.body).collect();
                v.push(default);
                v
            }
            Stmt::Disabled(s) => s.children(),
            _ => vec![],
        }
    }

    /// Mutable version of [`Stmt::children`].
    pub fn children_mut(&mut self) -> Vec<&mut Vec<Stmt>> {
        match self {
            Stmt::If { then, otherwise, .. } => vec![then, otherwise],
            Stmt::While { body, .. } | Stmt::For { body, .. } | Stmt::ForEachUnit { body, .. } | Stmt::ForEachPlayer { body, .. } => {
                vec![body]
            }
            Stmt::Switch { cases, default, .. } => {
                let mut v: Vec<&mut Vec<Stmt>> = cases.iter_mut().map(|c| &mut c.body).collect();
                v.push(default);
                v
            }
            Stmt::Disabled(s) => s.children_mut(),
            _ => vec![],
        }
    }

    pub fn is_loop(&self) -> bool {
        matches!(self, Stmt::While { .. } | Stmt::For { .. } | Stmt::ForEachUnit { .. } | Stmt::ForEachPlayer { .. })
    }

    /// Every expression directly held by this statement (not nested blocks).
    pub fn exprs(&self) -> Vec<&Expr> {
        match self {
            Stmt::Call(c) => c.args.iter().collect(),
            Stmt::Set { value, index, .. } => std::iter::once(value).chain(index.iter()).collect(),
            Stmt::If { cond, .. } | Stmt::While { cond, .. } => vec![cond],
            Stmt::For { from, to, step, .. } => [from, to].into_iter().chain(step.iter()).collect(),
            Stmt::ForEachUnit { group, .. } | Stmt::ForEachPlayer { group, .. } => vec![group],
            Stmt::Switch { value, cases, .. } => std::iter::once(value).chain(cases.iter().map(|c| &c.value)).collect(),
            Stmt::Wait(e) | Stmt::ReturnValue(e) => vec![e],
            _ => vec![],
        }
    }
}

/// Visit every statement in `body` (depth first, including nested blocks).
pub fn walk_stmts<'a>(body: &'a [Stmt], f: &mut dyn FnMut(&'a Stmt, usize)) {
    fn go<'a>(body: &'a [Stmt], depth: usize, f: &mut dyn FnMut(&'a Stmt, usize)) {
        for s in body {
            f(s, depth);
            for c in s.children() {
                go(c, depth + 1, f);
            }
        }
    }
    go(body, 0, f)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub ty: GalaxyType,
    pub array_len: Option<u32>,
    pub initial: Option<Expr>,
    pub comment: String,
}

impl Variable {
    pub fn new(name: impl Into<String>, ty: GalaxyType) -> Self {
        Self { name: name.into(), ty, array_len: None, initial: None, comment: String::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    pub id: TriggerId,
    pub name: String,
    pub comment: String,
    /// Disabled triggers are kept in the library but left out of the script.
    pub enabled: bool,
    pub initially_on: bool,
    /// Event registrations. Each call is a `TriggerAddEvent*` native; the
    /// trigger handle is inserted as the first argument by codegen.
    pub events: Vec<Call>,
    /// All must be true (implicit AND).
    pub conditions: Vec<Expr>,
    pub actions: Vec<Stmt>,
    /// Local variables, declared at the top of the trigger function.
    #[serde(default)]
    pub locals: Vec<Variable>,
}

impl Trigger {
    pub fn new(id: TriggerId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            comment: String::new(),
            enabled: true,
            initially_on: true,
            events: Vec::new(),
            conditions: Vec::new(),
            actions: Vec::new(),
            locals: Vec::new(),
        }
    }
}

/// A user-defined function or action (`gf_Name` in Galaxy).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomFunction {
    pub id: TriggerId,
    pub name: String,
    #[serde(default)]
    pub comment: String,
    #[serde(default)]
    pub params: Vec<Variable>,
    /// `None` for an action (void).
    #[serde(default)]
    pub returns: Option<GalaxyType>,
    #[serde(default)]
    pub locals: Vec<Variable>,
    #[serde(default)]
    pub actions: Vec<Stmt>,
}

impl CustomFunction {
    pub fn new(id: TriggerId, name: impl Into<String>) -> Self {
        Self { id, name: name.into(), comment: String::new(), params: Vec::new(), returns: None, locals: Vec::new(), actions: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TriggerItem {
    Folder { id: TriggerId, name: String, children: Vec<TriggerItem> },
    Trigger(Trigger),
}

impl TriggerItem {
    pub fn id(&self) -> TriggerId {
        match self {
            TriggerItem::Folder { id, .. } => *id,
            TriggerItem::Trigger(t) => t.id,
        }
    }
    pub fn name(&self) -> &str {
        match self {
            TriggerItem::Folder { name, .. } => name,
            TriggerItem::Trigger(t) => &t.name,
        }
    }
    pub fn set_name(&mut self, n: String) {
        match self {
            TriggerItem::Folder { name, .. } => *name = n,
            TriggerItem::Trigger(t) => t.name = n,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TriggerLibrary {
    pub items: Vec<TriggerItem>,
    pub variables: Vec<Variable>,
    /// Free-form Galaxy appended after generated globals (custom script).
    pub custom_script: String,
    pub next_id: u32,
    /// User-defined functions and actions.
    #[serde(default)]
    pub functions: Vec<CustomFunction>,
}

impl TriggerLibrary {
    /// The standard melee initialization trigger SC2 creates for new maps.
    pub fn with_melee_init() -> Self {
        let mut lib = TriggerLibrary::default();
        let id = lib.alloc_id();
        let mut t = Trigger::new(id, "Melee Initialization");
        t.events.push(Call { function: "TriggerAddEventMapInit".into(), args: vec![] });
        for f in ["MeleeInitResources", "MeleeInitUnits", "MeleeInitAI", "MeleeInitOptions"] {
            t.actions.push(Stmt::Call(Call { function: f.into(), args: vec![] }));
        }
        lib.items.push(TriggerItem::Trigger(t));
        lib
    }

    pub fn alloc_id(&mut self) -> TriggerId {
        self.next_id = self.next_id.max(1);
        let id = TriggerId(self.next_id);
        self.next_id += 1;
        id
    }

    /// Depth-first iterator over every trigger.
    pub fn triggers(&self) -> Vec<&Trigger> {
        fn walk<'a>(items: &'a [TriggerItem], out: &mut Vec<&'a Trigger>) {
            for i in items {
                match i {
                    TriggerItem::Folder { children, .. } => walk(children, out),
                    TriggerItem::Trigger(t) => out.push(t),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out
    }

    pub fn find(&self, id: TriggerId) -> Option<&TriggerItem> {
        fn walk(items: &[TriggerItem], id: TriggerId) -> Option<&TriggerItem> {
            for i in items {
                if i.id() == id {
                    return Some(i);
                }
                if let TriggerItem::Folder { children, .. } = i {
                    if let Some(f) = walk(children, id) {
                        return Some(f);
                    }
                }
            }
            None
        }
        walk(&self.items, id)
    }

    pub fn find_mut(&mut self, id: TriggerId) -> Option<&mut TriggerItem> {
        fn walk(items: &mut [TriggerItem], id: TriggerId) -> Option<&mut TriggerItem> {
            for i in items {
                if i.id() == id {
                    return Some(i);
                }
                if let TriggerItem::Folder { children, .. } = i {
                    if let Some(f) = walk(children, id) {
                        return Some(f);
                    }
                }
            }
            None
        }
        walk(&mut self.items, id)
    }

    pub fn trigger(&self, id: TriggerId) -> Option<&Trigger> {
        match self.find(id)? {
            TriggerItem::Trigger(t) => Some(t),
            _ => None,
        }
    }

    pub fn trigger_mut(&mut self, id: TriggerId) -> Option<&mut Trigger> {
        match self.find_mut(id)? {
            TriggerItem::Trigger(t) => Some(t),
            _ => None,
        }
    }

    pub fn function(&self, id: TriggerId) -> Option<&CustomFunction> {
        self.functions.iter().find(|f| f.id == id)
    }

    pub fn function_mut(&mut self, id: TriggerId) -> Option<&mut CustomFunction> {
        self.functions.iter_mut().find(|f| f.id == id)
    }

    pub fn function_named(&self, name: &str) -> Option<&CustomFunction> {
        self.functions.iter().find(|f| f.name == name)
    }

    /// Remove an item anywhere in the tree.
    pub fn remove(&mut self, id: TriggerId) -> Option<TriggerItem> {
        fn walk(items: &mut Vec<TriggerItem>, id: TriggerId) -> Option<TriggerItem> {
            if let Some(i) = items.iter().position(|i| i.id() == id) {
                return Some(items.remove(i));
            }
            for i in items.iter_mut() {
                if let TriggerItem::Folder { children, .. } = i {
                    if let Some(f) = walk(children, id) {
                        return Some(f);
                    }
                }
            }
            None
        }
        walk(&mut self.items, id)
    }

    /// The list containing `id` and its index there. `None` folder = top level.
    pub fn locate(&self, id: TriggerId) -> Option<(Option<TriggerId>, usize)> {
        fn walk(items: &[TriggerItem], parent: Option<TriggerId>, id: TriggerId) -> Option<(Option<TriggerId>, usize)> {
            for (n, i) in items.iter().enumerate() {
                if i.id() == id {
                    return Some((parent, n));
                }
                if let TriggerItem::Folder { id: fid, children, .. } = i {
                    if let Some(r) = walk(children, Some(*fid), id) {
                        return Some(r);
                    }
                }
            }
            None
        }
        walk(&self.items, None, id)
    }

    /// The child list of a folder (or the top level for `None`).
    pub fn list_mut(&mut self, folder: Option<TriggerId>) -> Option<&mut Vec<TriggerItem>> {
        match folder {
            None => Some(&mut self.items),
            Some(f) => match self.find_mut(f)? {
                TriggerItem::Folder { children, .. } => Some(children),
                _ => None,
            },
        }
    }

    /// Insert `item` into `folder` at `index` (clamped).
    pub fn insert(&mut self, folder: Option<TriggerId>, index: usize, item: TriggerItem) {
        let list = match self.list_mut(folder) {
            Some(l) => l,
            None => &mut self.items,
        };
        let index = index.min(list.len());
        list.insert(index, item);
    }

    /// True if `id` is `ancestor` or nested inside it.
    pub fn is_within(&self, id: TriggerId, ancestor: TriggerId) -> bool {
        if id == ancestor {
            return true;
        }
        match self.find(ancestor) {
            Some(TriggerItem::Folder { children, .. }) => {
                fn walk(items: &[TriggerItem], id: TriggerId) -> bool {
                    items.iter().any(|i| i.id() == id || matches!(i, TriggerItem::Folder { children, .. } if walk(children, id)))
                }
                walk(children, id)
            }
            _ => false,
        }
    }

    /// Move an item to `folder` at `index`. Refuses to move a folder into itself.
    pub fn move_to(&mut self, id: TriggerId, folder: Option<TriggerId>, index: usize) -> bool {
        if let Some(f) = folder {
            if self.is_within(f, id) || !matches!(self.find(f), Some(TriggerItem::Folder { .. })) {
                return false;
            }
        }
        let Some((from_folder, from_index)) = self.locate(id) else { return false };
        let Some(item) = self.remove(id) else { return false };
        let index = if from_folder == folder && from_index < index { index - 1 } else { index };
        self.insert(folder, index, item);
        true
    }

    /// Move an item one step up or down within its list.
    pub fn move_by(&mut self, id: TriggerId, delta: isize) -> bool {
        let Some((folder, i)) = self.locate(id) else { return false };
        let Some(list) = self.list_mut(folder) else { return false };
        let j = i as isize + delta;
        if j < 0 || j as usize >= list.len() {
            return false;
        }
        list.swap(i, j as usize);
        true
    }

    /// Every name used by a trigger, folder or custom function.
    fn taken_names(&self) -> Vec<String> {
        fn walk(items: &[TriggerItem], out: &mut Vec<String>) {
            for i in items {
                out.push(i.name().to_owned());
                if let TriggerItem::Folder { children, .. } = i {
                    walk(children, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut out);
        out.extend(self.functions.iter().map(|f| f.name.clone()));
        out
    }

    /// `base`, or `base 2`, `base 3`... whichever is free.
    pub fn unique_name(&self, base: &str) -> String {
        let taken = self.taken_names();
        if !taken.iter().any(|n| n == base) {
            return base.to_owned();
        }
        (2..).map(|n| format!("{base} {n}")).find(|c| !taken.contains(c)).unwrap_or_else(|| base.to_owned())
    }

    /// Unique global variable name.
    pub fn unique_variable_name(&self, base: &str) -> String {
        let taken = |n: &str| self.variables.iter().any(|v| v.name == n);
        if !taken(base) {
            return base.to_owned();
        }
        (2..).map(|n| format!("{base} {n}")).find(|c| !taken(c)).unwrap_or_else(|| base.to_owned())
    }

    /// Deep copy of an item with fresh ids and a unique name, inserted after it.
    pub fn duplicate(&mut self, id: TriggerId) -> Option<TriggerId> {
        let (folder, index) = self.locate(id)?;
        let mut copy = self.find(id)?.clone();
        fn reid(lib: &mut TriggerLibrary, item: &mut TriggerItem) {
            match item {
                TriggerItem::Folder { id, children, .. } => {
                    *id = lib.alloc_id();
                    for c in children {
                        reid(lib, c);
                    }
                }
                TriggerItem::Trigger(t) => t.id = lib.alloc_id(),
            }
        }
        reid(self, &mut copy);
        let name = self.unique_name(&format!("{} copy", copy.name()));
        copy.set_name(name);
        let new_id = copy.id();
        self.insert(folder, index + 1, copy);
        Some(new_id)
    }
}

const RESERVED: &[&str] = &[
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
    "void",
    "int",
    "fixed",
    "bool",
    "string",
    "text",
    "point",
    "region",
    "unit",
    "unitgroup",
    "playergroup",
    "timer",
    "trigger",
    "sound",
    "color",
    "do",
    "switch",
    "case",
    "default",
    "goto",
];

/// Trigger-model problems for the Problems panel: `(trigger or function name, message)`.
///
/// Checks calls against [`crate::trigger_lib`] (unknown functions, wrong
/// argument counts), unset parameters, undeclared variables, `Break` /
/// `Continue` outside loops, misplaced events, duplicate variable names and
/// references to missing triggers.
pub fn check(lib: &TriggerLibrary) -> Vec<(String, String)> {
    use crate::galaxy::ident;
    use crate::trigger_lib;

    let mut out: Vec<(String, String)> = Vec::new();

    let mut seen = std::collections::HashMap::new();
    for v in &lib.variables {
        let id = crate::galaxy::var_ident(&v.name);
        if let Some(prev) = seen.insert(id.clone(), v.name.clone()) {
            out.push(("Variables".into(), format!("Variables \"{prev}\" and \"{}\" compile to the same name. Rename one.", v.name)));
        }
        if RESERVED.contains(&v.name.as_str()) {
            out.push(("Variables".into(), format!("\"{}\" is a Galaxy keyword. Rename the variable.", v.name)));
        }
    }
    let mut fseen = std::collections::HashSet::new();
    for f in &lib.functions {
        if !fseen.insert(ident(&f.name)) {
            out.push((f.name.clone(), format!("Two custom functions compile to \"gf_{}\". Rename one.", ident(&f.name))));
        }
    }

    struct Scope<'a> {
        owner: String,
        locals: &'a [Variable],
        params: &'a [Variable],
        returns: Option<&'a GalaxyType>,
        is_function: bool,
    }

    fn check_call(lib: &TriggerLibrary, sc: &Scope, c: &Call, out: &mut Vec<(String, String)>) {
        if c.function.trim().is_empty() {
            out.push((sc.owner.clone(), "A call has no function chosen.".into()));
            return;
        }
        if let Some(spec) = trigger_lib::get(&c.function) {
            if spec.kind == trigger_lib::FnKind::Event {
                out.push((sc.owner.clone(), format!("\"{}\" is an event; move it to the Events section.", spec.label)));
            }
            if c.args.len() != spec.params.len() {
                out.push((
                    sc.owner.clone(),
                    format!("\"{}\" takes {} parameters but has {}.", spec.label, spec.params.len(), c.args.len()),
                ));
            }
            for (a, p) in c.args.iter().zip(spec.params) {
                if a.is_unset() {
                    out.push((sc.owner.clone(), format!("\"{}\" needs a value for {}.", spec.label, p.label.to_lowercase())));
                }
            }
        } else if let Some(f) = lib.function_named(&c.function) {
            if c.args.len() != f.params.len() {
                out.push((sc.owner.clone(), format!("\"{}\" takes {} parameters but has {}.", f.name, f.params.len(), c.args.len())));
            }
        } else if !trigger_lib::is_known_function(&c.function) {
            out.push((sc.owner.clone(), format!("Unknown function \"{}\". Check the spelling or define it in custom script.", c.function)));
        }
    }

    fn check_expr(lib: &TriggerLibrary, sc: &Scope, e: &Expr, out: &mut Vec<(String, String)>) {
        e.walk(&mut |e| match e {
            Expr::Call(c) => check_call(lib, sc, c, out),
            Expr::Variable(v) | Expr::Element { variable: v, .. } => check_var(lib, sc, v, out),
            Expr::Trigger(id) if lib.trigger(*id).is_none() => {
                out.push((sc.owner.clone(), "Refers to a trigger that no longer exists.".into()));
            }
            _ => {}
        });
    }

    fn check_var(lib: &TriggerLibrary, sc: &Scope, v: &str, out: &mut Vec<(String, String)>) {
        let declared = sc.locals.iter().chain(sc.params).chain(&lib.variables).any(|x| x.name == v);
        if !declared {
            out.push((sc.owner.clone(), format!("Variable \"{v}\" is not declared. Add it as a local or global variable.")));
        }
    }

    fn check_body(lib: &TriggerLibrary, sc: &Scope, body: &[Stmt], loops: usize, out: &mut Vec<(String, String)>) {
        for s in body {
            let s = match s {
                Stmt::Disabled(_) => continue,
                s => s,
            };
            match s {
                Stmt::Call(c) => check_call(lib, sc, c, out),
                Stmt::Set { variable, .. } => check_var(lib, sc, variable, out),
                Stmt::For { variable, .. } | Stmt::ForEachUnit { variable, .. } | Stmt::ForEachPlayer { variable, .. } => {
                    // Undeclared loop variables are declared automatically.
                    let _ = variable;
                }
                Stmt::Break | Stmt::Continue if loops == 0 => {
                    out.push((sc.owner.clone(), "Break and continue only work inside a loop.".into()));
                }
                Stmt::Return if sc.is_function && sc.returns.is_some() => {
                    out.push((sc.owner.clone(), "This function returns a value; use \"Return value\" instead.".into()));
                }
                Stmt::ReturnValue(_) if !sc.is_function || sc.returns.is_none() => {
                    out.push((sc.owner.clone(), "\"Return value\" only works in a custom function that returns a value.".into()));
                }
                _ => {}
            }
            for e in s.exprs() {
                if e.is_unset() {
                    out.push((sc.owner.clone(), "An action has an empty value. Click it to choose one.".into()));
                }
                check_expr(lib, sc, e, out);
            }
            let inner = loops + usize::from(s.is_loop());
            for c in s.children() {
                check_body(lib, sc, c, inner, out);
            }
        }
    }

    fn check_locals(owner: &str, locals: &[Variable], out: &mut Vec<(String, String)>) {
        let mut names = std::collections::HashSet::new();
        for v in locals {
            if !names.insert(crate::galaxy::var_ident(&v.name)) {
                out.push((owner.to_owned(), format!("Two local variables compile to the same name as \"{}\".", v.name)));
            }
            if RESERVED.contains(&v.name.as_str()) {
                out.push((owner.to_owned(), format!("\"{}\" is a Galaxy keyword. Rename the variable.", v.name)));
            }
        }
    }

    for t in lib.triggers() {
        if !t.enabled {
            continue;
        }
        let sc = Scope { owner: t.name.clone(), locals: &t.locals, params: &[], returns: None, is_function: false };
        check_locals(&t.name, &t.locals, &mut out);
        for e in &t.events {
            match trigger_lib::get(&e.function) {
                Some(spec) if spec.kind != trigger_lib::FnKind::Event => {
                    out.push((t.name.clone(), format!("\"{}\" is not an event.", spec.label)));
                }
                Some(spec) if e.args.len() != spec.params.len() => {
                    out.push((
                        t.name.clone(),
                        format!("Event \"{}\" takes {} parameters but has {}.", spec.label, spec.params.len(), e.args.len()),
                    ));
                }
                None if !e.function.starts_with("TriggerAddEvent") => {
                    out.push((t.name.clone(), format!("\"{}\" is not an event.", e.function)));
                }
                _ => {}
            }
            for a in &e.args {
                if a.is_unset() {
                    out.push((t.name.clone(), "An event has an empty value. Click it to choose one.".into()));
                }
                check_expr(lib, &sc, a, &mut out);
            }
        }
        for c in &t.conditions {
            if c.is_unset() {
                out.push((t.name.clone(), "A condition is empty.".into()));
            }
            check_expr(lib, &sc, c, &mut out);
        }
        check_body(lib, &sc, &t.actions, 0, &mut out);
    }
    for f in &lib.functions {
        let sc = Scope { owner: f.name.clone(), locals: &f.locals, params: &f.params, returns: f.returns.as_ref(), is_function: true };
        check_locals(&f.name, &f.params, &mut out);
        check_locals(&f.name, &f.locals, &mut out);
        check_body(lib, &sc, &f.actions, 0, &mut out);
    }
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_json_still_loads() {
        // A trigger saved before `locals` / `functions` / `index` / `step` existed.
        let json = r#"{"items":[{"Trigger":{"id":1,"name":"T","comment":"","enabled":true,"initially_on":true,
            "events":[],"conditions":[],"actions":[{"Set":{"variable":"x","value":{"Literal":{"Int":1}}}},
            {"For":{"variable":"i","from":{"Literal":{"Int":1}},"to":{"Literal":{"Int":3}},"body":[]}}]}}],
            "variables":[],"custom_script":"","next_id":2}"#;
        let lib: TriggerLibrary = serde_json::from_str(json).unwrap();
        assert!(lib.functions.is_empty());
        assert!(lib.triggers()[0].locals.is_empty());
    }

    #[test]
    fn move_and_duplicate() {
        let mut lib = TriggerLibrary::with_melee_init();
        let folder = lib.alloc_id();
        lib.items.push(TriggerItem::Folder { id: folder, name: "Folder".into(), children: vec![] });
        let first = lib.items[0].id();
        assert!(lib.move_to(first, Some(folder), 0));
        assert_eq!(lib.locate(first), Some((Some(folder), 0)));
        assert!(!lib.move_to(folder, Some(folder), 0));
        let copy = lib.duplicate(first).unwrap();
        assert_eq!(lib.find(copy).unwrap().name(), "Melee Initialization copy");
        assert!(lib.move_by(copy, -1));
        assert_eq!(lib.locate(copy), Some((Some(folder), 0)));
    }

    #[test]
    fn check_reports_model_problems() {
        let mut lib = TriggerLibrary::with_melee_init();
        assert!(check(&lib).is_empty(), "{:?}", check(&lib));
        let id = lib.items[0].id();
        let t = lib.trigger_mut(id).unwrap();
        t.actions.push(Stmt::Break);
        t.actions.push(Stmt::Call(Call { function: "NotARealNative".into(), args: vec![] }));
        t.actions.push(Stmt::Set { variable: "missing".into(), value: Expr::int(1), index: None });
        t.actions.push(Stmt::Call(Call { function: "UnitKill".into(), args: vec![Expr::unset()] }));
        let msgs: Vec<String> = check(&lib).into_iter().map(|(_, m)| m).collect();
        assert!(msgs.iter().any(|m| m.contains("Break")));
        assert!(msgs.iter().any(|m| m.contains("NotARealNative")));
        assert!(msgs.iter().any(|m| m.contains("\"missing\"")));
        assert!(msgs.iter().any(|m| m.contains("needs a value")));
    }
}
