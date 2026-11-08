//! Trigger library: triggers, variables and custom script, organized in folders.
//!
//! The model mirrors how map makers think (event / condition / action) and is
//! compiled to Galaxy by [`crate::galaxy`].

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct TriggerId(pub u32);

/// Galaxy value types exposed in the trigger editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Global or local variable by name.
    Variable(String),
    Call(Call),
    /// Named preset constant, e.g. `c_unitCreateIgnorePlacement`.
    Preset(String),
    /// Reference to a placed point/unit/region by editor name.
    Placed(String),
    /// Binary operator: `lhs op rhs` with op one of + - * / == != < <= > >= && ||.
    Binary { op: String, lhs: Box<Expr>, rhs: Box<Expr> },
    Not(Box<Expr>),
    /// Raw Galaxy expression (escape hatch).
    Raw(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Stmt {
    Call(Call),
    Set { variable: String, value: Expr },
    If { cond: Expr, then: Vec<Stmt>, otherwise: Vec<Stmt> },
    While { cond: Expr, body: Vec<Stmt> },
    /// `for var = from to to step 1`
    For { variable: String, from: Expr, to: Expr, body: Vec<Stmt> },
    /// Wait N game seconds.
    Wait(Expr),
    Return,
    Comment(String),
    /// Raw Galaxy statement(s).
    Raw(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub ty: GalaxyType,
    pub array_len: Option<u32>,
    pub initial: Option<Expr>,
    pub comment: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Trigger {
    pub id: TriggerId,
    pub name: String,
    pub comment: String,
    pub enabled: bool,
    pub initially_on: bool,
    /// Event registrations. Each call is a `TriggerAddEvent*` native; the
    /// trigger handle is inserted as the first argument by codegen.
    pub events: Vec<Call>,
    /// All must be true (implicit AND).
    pub conditions: Vec<Expr>,
    pub actions: Vec<Stmt>,
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
        }
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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TriggerLibrary {
    pub items: Vec<TriggerItem>,
    pub variables: Vec<Variable>,
    /// Free-form Galaxy appended after generated globals (custom script).
    pub custom_script: String,
    pub next_id: u32,
}

impl TriggerLibrary {
    /// The standard melee initialization trigger SC2 creates for new maps.
    pub fn with_melee_init() -> Self {
        let mut lib = TriggerLibrary::default();
        let id = lib.alloc_id();
        let mut t = Trigger::new(id, "Melee Initialization");
        t.events.push(Call { function: "TriggerAddEventMapInit".into(), args: vec![] });
        for f in [
            "MeleeInitResources",
            "MeleeInitUnits",
            "MeleeInitAI",
            "MeleeInitOptions",
        ] {
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

    pub fn trigger_mut(&mut self, id: TriggerId) -> Option<&mut Trigger> {
        match self.find_mut(id)? {
            TriggerItem::Trigger(t) => Some(t),
            _ => None,
        }
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
}
