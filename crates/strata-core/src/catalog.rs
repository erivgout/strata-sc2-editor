//! GameData catalogs (UnitData.xml, AbilData.xml, WeaponData.xml, ...).
//!
//! Entries are kept as a generic XML element tree so that every field SC2
//! supports round-trips, while the Data panel layers friendly editing on top.

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
        Self { name: name.into(), ..Default::default() }
    }

    pub fn attr(&self, key: &str) -> Option<&str> {
        self.attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn set_attr(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        match self.attrs.iter_mut().find(|(k, _)| k == key) {
            Some((_, v)) => *v = value,
            None => self.attrs.push((key.to_owned(), value)),
        }
    }

    pub fn child(&self, name: &str) -> Option<&XmlNode> {
        self.children.iter().find(|c| c.name == name)
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
    pub fn class(&self) -> &str {
        &self.node.name
    }
    pub fn id(&self) -> &str {
        self.node.attr("id").unwrap_or("")
    }
    pub fn parent(&self) -> Option<&str> {
        self.node.attr("parent")
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
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Catalog {
    pub entries: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn find(&self, class: &str, id: &str) -> Option<&CatalogEntry> {
        self.entries.iter().find(|e| e.class() == class && e.id() == id)
    }

    pub fn find_mut(&mut self, class: &str, id: &str) -> Option<&mut CatalogEntry> {
        self.entries.iter_mut().find(|e| e.class() == class && e.id() == id)
    }

    pub fn of_kind(&self, kind: CatalogKind) -> impl Iterator<Item = (usize, &CatalogEntry)> {
        self.entries.iter().enumerate().filter(move |(_, e)| CatalogKind::from_file(&e.file) == kind)
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
}
