//! A curated schema of frequently edited GameData fields.
//!
//! SC2 classes have hundreds of fields; this lists the ones people reach for
//! most, with their type, default and a one-line description, so the Data
//! panel can offer typed editors, link pickers and autocomplete. Fields not
//! listed here still work: they are edited as plain text.

use crate::catalog::CatalogKind;

/// How a field's value is edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Int,
    Real,
    /// `0` or `1`.
    Bool,
    /// Indexed boolean array, e.g. `Attributes[Armored]`; the choices are indices.
    Flags(&'static [&'static str]),
    /// Indexed numeric array, e.g. `CostResource[Minerals]`; the choices are indices.
    IndexedReal(&'static [&'static str]),
    /// Id of an entry in another catalog.
    Link(CatalogKind),
    /// Repeated elements whose `Link` attribute names an entry (`WeaponArray`).
    LinkArray(CatalogKind),
    /// Free text.
    Text,
    /// Key into the map's text (strings), such as `Unit/Name/Marine`.
    StringKey,
    Enum(&'static [&'static str]),
    /// A structure with nested elements; edited as a tree.
    Struct,
}

impl FieldType {
    pub fn label(self) -> &'static str {
        match self {
            FieldType::Int => "integer",
            FieldType::Real => "number",
            FieldType::Bool => "yes / no",
            FieldType::Flags(_) => "flags",
            FieldType::IndexedReal(_) => "number per index",
            FieldType::Link(_) => "link",
            FieldType::LinkArray(_) => "list of links",
            FieldType::Text => "text",
            FieldType::StringKey => "text key",
            FieldType::Enum(_) => "choice",
            FieldType::Struct => "structure",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FieldDef {
    /// Element name, e.g. "LifeMax" or "CostResource".
    pub name: &'static str,
    pub ty: FieldType,
    pub default: &'static str,
    pub desc: &'static str,
}

#[derive(Debug, Clone, Copy)]
pub struct ClassSchema {
    pub class: &'static str,
    pub kind: CatalogKind,
    pub desc: &'static str,
    pub fields: &'static [FieldDef],
}

const fn f(
    name: &'static str,
    ty: FieldType,
    default: &'static str,
    desc: &'static str,
) -> FieldDef {
    FieldDef {
        name,
        ty,
        default,
        desc,
    }
}

use CatalogKind as K;
use FieldType as T;

pub const RACES: &[&str] = &["Terr", "Prot", "Zerg", "Neut"];
pub const RESOURCES: &[&str] = &["Minerals", "Vespene", "Terrazine", "Custom"];
pub const UNIT_ATTRIBUTES: &[&str] = &[
    "Light",
    "Armored",
    "Biological",
    "Mechanical",
    "Robotic",
    "Psionic",
    "Massive",
    "Structure",
    "Hover",
    "Heroic",
    "Summoned",
];
pub const PLANES: &[&str] = &["Ground", "Air"];
pub const DAMAGE_KINDS: &[&str] = &["Melee", "Ranged", "Splash", "Spell", "None"];
pub const DEATH_TYPES: &[&str] = &[
    "Normal",
    "Blast",
    "Disintegrate",
    "Eat",
    "Electrocute",
    "Eviscerate",
    "Fire",
    "Freeze",
    "Impact",
    "Silentkill",
    "Squish",
    "Unknown",
];
pub const VITALS: &[&str] = &["Life", "Shields", "Energy"];

const UNIT_FIELDS: &[FieldDef] = &[
    f(
        "Name",
        T::StringKey,
        "Unit/Name/<id>",
        "Text key of the name shown in game.",
    ),
    f("Race", T::Enum(RACES), "Neut", "Race the unit belongs to."),
    f("LifeStart", T::Real, "0", "Life the unit spawns with."),
    f("LifeMax", T::Real, "0", "Maximum life (hit points)."),
    f(
        "LifeRegenRate",
        T::Real,
        "0",
        "Life regenerated per second.",
    ),
    f(
        "LifeArmor",
        T::Real,
        "0",
        "Armor subtracted from each incoming hit to life.",
    ),
    f(
        "ShieldsStart",
        T::Real,
        "0",
        "Shields the unit spawns with.",
    ),
    f("ShieldsMax", T::Real, "0", "Maximum shields."),
    f(
        "ShieldRegenDelay",
        T::Real,
        "0",
        "Seconds without damage before shields regenerate.",
    ),
    f(
        "ShieldRegenRate",
        T::Real,
        "0",
        "Shields regenerated per second.",
    ),
    f(
        "ShieldArmor",
        T::Real,
        "0",
        "Armor applied to damage taken by shields.",
    ),
    f("EnergyStart", T::Real, "0", "Energy the unit spawns with."),
    f("EnergyMax", T::Real, "0", "Maximum energy."),
    f(
        "EnergyRegenRate",
        T::Real,
        "0",
        "Energy regenerated per second.",
    ),
    f("Speed", T::Real, "0", "Movement speed in cells per second."),
    f(
        "Acceleration",
        T::Real,
        "0",
        "How quickly the unit reaches full speed.",
    ),
    f(
        "TurningRate",
        T::Real,
        "0",
        "Turning speed in degrees per second.",
    ),
    f("Sight", T::Real, "0", "Vision radius in cells."),
    f("Radius", T::Real, "0.5", "Collision radius in cells."),
    f(
        "SeparationRadius",
        T::Real,
        "0",
        "Radius used to keep units apart while moving.",
    ),
    f(
        "Food",
        T::Real,
        "0",
        "Supply provided (positive) or used (negative).",
    ),
    f(
        "CostResource",
        T::IndexedReal(RESOURCES),
        "0",
        "Cost per resource; the index is the resource.",
    ),
    f(
        "AttackTargetPriority",
        T::Int,
        "20",
        "How strongly enemies prefer to attack this unit.",
    ),
    f(
        "Attributes",
        T::Flags(UNIT_ATTRIBUTES),
        "0",
        "Attributes such as Armored or Biological; weapons get bonuses against them.",
    ),
    f(
        "PlaneArray",
        T::Flags(PLANES),
        "0",
        "Whether the unit is on the ground or in the air.",
    ),
    f(
        "Collide",
        T::Flags(&["Ground", "Flying", "Small", "Structure", "Unit", "Locust"]),
        "0",
        "What the unit collides with.",
    ),
    f(
        "Mover",
        T::Enum(&[
            "Ground",
            "Fly",
            "Burrowed",
            "Cliffjumper",
            "Colossus",
            "Reaper",
        ]),
        "Ground",
        "Pathing used when moving.",
    ),
    f(
        "WeaponArray",
        T::LinkArray(K::Weapon),
        "",
        "Weapons the unit attacks with.",
    ),
    f(
        "AbilArray",
        T::LinkArray(K::Ability),
        "",
        "Abilities the unit has (move, stop, attack...).",
    ),
    f(
        "BehaviorArray",
        T::LinkArray(K::Behavior),
        "",
        "Behaviors (buffs and passives) the unit starts with.",
    ),
    f("CardLayouts", T::Struct, "", "Command card buttons."),
    f(
        "DefaultAcquireLevel",
        T::Enum(&["Offensive", "Defensive", "Passive"]),
        "Offensive",
        "When the unit picks targets on its own.",
    ),
    f(
        "ScoreMake",
        T::Int,
        "0",
        "Score awarded for building the unit.",
    ),
    f(
        "ScoreKill",
        T::Int,
        "0",
        "Score awarded for killing the unit.",
    ),
    f(
        "SubgroupPriority",
        T::Int,
        "0",
        "Order in the selection panel; higher comes first.",
    ),
    f("KillXP", T::Int, "0", "Experience granted to the killer."),
    f(
        "Footprint",
        T::Text,
        "",
        "Footprint used for placement and pathing.",
    ),
    f(
        "Description",
        T::StringKey,
        "",
        "Text key of the tooltip description.",
    ),
    f(
        "Flags",
        T::Flags(&[
            "Untargetable",
            "Invulnerable",
            "Unselectable",
            "NoDraw",
            "Uncommandable",
            "Unclickable",
            "Uncursorable",
            "NoPortraitTalk",
        ]),
        "0",
        "Special unit flags.",
    ),
];

const WEAPON_FIELDS: &[FieldDef] = &[
    f("Range", T::Real, "0", "Attack range in cells."),
    f(
        "MinimumRange",
        T::Real,
        "0",
        "Targets closer than this cannot be attacked.",
    ),
    f(
        "Period",
        T::Real,
        "0",
        "Seconds between attacks (cooldown).",
    ),
    f(
        "DamagePoint",
        T::Real,
        "0",
        "Seconds into the attack animation when damage happens.",
    ),
    f(
        "Backswing",
        T::Real,
        "0",
        "Seconds after the damage point before the unit can act.",
    ),
    f(
        "Effect",
        T::Link(K::Effect),
        "",
        "Effect run on each attack (usually a damage or missile effect).",
    ),
    f(
        "DisplayEffect",
        T::Link(K::Effect),
        "",
        "Effect whose damage is shown in the tooltip.",
    ),
    f(
        "TargetFilters",
        T::Text,
        "",
        "Which targets are allowed; required;excluded (e.g. \"Ground,Visible;Missile,Dead\").",
    ),
    f(
        "Arc",
        T::Real,
        "0",
        "Facing arc in degrees the target must be in.",
    ),
    f(
        "ArcSlop",
        T::Real,
        "0",
        "Extra arc tolerance before turning.",
    ),
    f(
        "RangeSlop",
        T::Real,
        "0",
        "Extra range before an attack is interrupted.",
    ),
    f(
        "RandomDelayMin",
        T::Real,
        "0",
        "Minimum random delay added to each attack.",
    ),
    f(
        "RandomDelayMax",
        T::Real,
        "0",
        "Maximum random delay added to each attack.",
    ),
    f(
        "AllowedMovement",
        T::Enum(&["None", "Moving", "Slowing"]),
        "None",
        "Whether the unit can attack while moving.",
    ),
    f("Icon", T::Text, "", "Icon image path."),
    f(
        "Name",
        T::StringKey,
        "Weapon/Name/<id>",
        "Text key of the weapon name.",
    ),
    f(
        "Options",
        T::Flags(&[
            "Hidden",
            "Melee",
            "OnlyFireAtAttackTarget",
            "Disabled",
            "NoDeceleration",
            "ContinuousScan",
        ]),
        "0",
        "Weapon options.",
    ),
];

const DAMAGE_FIELDS: &[FieldDef] = &[
    f("Amount", T::Real, "0", "Base damage dealt."),
    f(
        "ArmorReduction",
        T::Real,
        "1",
        "Multiplier applied to the target's armor.",
    ),
    f(
        "AttributeBonus",
        T::IndexedReal(UNIT_ATTRIBUTES),
        "0",
        "Extra damage against an attribute, e.g. AttributeBonus[Armored].",
    ),
    f(
        "Kind",
        T::Enum(DAMAGE_KINDS),
        "Ranged",
        "Damage kind; affects responses and some upgrades.",
    ),
    f(
        "Death",
        T::Enum(DEATH_TYPES),
        "Normal",
        "Death animation when this kills.",
    ),
    f(
        "ShieldBonus",
        T::Real,
        "0",
        "Extra damage dealt to shields.",
    ),
    f(
        "LeechFraction",
        T::IndexedReal(VITALS),
        "0",
        "Fraction of damage returned to the caster.",
    ),
    f("SearchFilters", T::Text, "", "Targets hit by area damage."),
    f(
        "AreaArray",
        T::Struct,
        "",
        "Splash areas: radius and fraction of damage.",
    ),
    f(
        "ValidatorArray",
        T::LinkArray(K::Other),
        "",
        "Validators that must pass.",
    ),
    f(
        "Visibility",
        T::Enum(&["Visible", "Hidden", "Snapshot"]),
        "Visible",
        "Whether the damage is visible to the target's owner.",
    ),
];

const LAUNCH_MISSILE_FIELDS: &[FieldDef] = &[
    f(
        "AmmoUnit",
        T::Link(K::Unit),
        "",
        "Missile unit that travels to the target.",
    ),
    f(
        "ImpactEffect",
        T::Link(K::Effect),
        "",
        "Effect run when the missile arrives.",
    ),
    f(
        "LaunchEffect",
        T::Link(K::Effect),
        "",
        "Effect run when the missile is launched.",
    ),
    f(
        "Movers",
        T::Struct,
        "",
        "Mover used for the missile's flight path.",
    ),
    f("ImpactLocation", T::Struct, "", "Where the missile hits."),
    f("LaunchLocation", T::Struct, "", "Where the missile starts."),
    f(
        "DeathType",
        T::Enum(&["Impact", "Normal", "Remove", "Timeout"]),
        "Impact",
        "How the missile dies on arrival.",
    ),
    f(
        "ValidatorArray",
        T::LinkArray(K::Other),
        "",
        "Validators that must pass.",
    ),
];

const SET_FIELDS: &[FieldDef] = &[
    f(
        "EffectArray",
        T::LinkArray(K::Effect),
        "",
        "Effects run together.",
    ),
    f(
        "MinCount",
        T::Int,
        "0",
        "Minimum effects to pick at random.",
    ),
    f(
        "MaxCount",
        T::Int,
        "0",
        "Maximum effects to pick at random (0 = all).",
    ),
    f(
        "ValidatorArray",
        T::LinkArray(K::Other),
        "",
        "Validators that must pass.",
    ),
];

const APPLY_BEHAVIOR_FIELDS: &[FieldDef] = &[
    f(
        "Behavior",
        T::Link(K::Behavior),
        "",
        "Behavior applied to the target.",
    ),
    f("Count", T::Int, "1", "Stacks applied."),
    f(
        "ValidatorArray",
        T::LinkArray(K::Other),
        "",
        "Validators that must pass.",
    ),
];

const CREATE_UNIT_FIELDS: &[FieldDef] = &[
    f("SpawnUnit", T::Link(K::Unit), "", "Unit created."),
    f("SpawnCount", T::Int, "1", "How many units are created."),
    f(
        "SpawnEffect",
        T::Link(K::Effect),
        "",
        "Effect run on each created unit.",
    ),
    f("SpawnRange", T::Real, "0", "Search radius for a free spot."),
];

const ABIL_EFFECT_FIELDS: &[FieldDef] = &[
    f(
        "Effect",
        T::Link(K::Effect),
        "",
        "Effect run when the ability is cast.",
    ),
    f("Range", T::Real, "0", "Cast range in cells."),
    f(
        "Cost",
        T::Struct,
        "",
        "Energy, resource and cooldown costs.",
    ),
    f(
        "CmdButtonArray",
        T::Struct,
        "",
        "Command card button for the ability.",
    ),
    f(
        "TargetFilters",
        T::Text,
        "",
        "Which targets are allowed; required;excluded.",
    ),
    f("Arc", T::Real, "0", "Facing arc in degrees before casting."),
    f(
        "CastIntroTime",
        T::Real,
        "0",
        "Seconds before the effect happens.",
    ),
    f("PrepTime", T::Real, "0", "Preparation time in seconds."),
    f(
        "FinishTime",
        T::Real,
        "0",
        "Seconds after the effect before the unit can act.",
    ),
    f(
        "AutoCastFilters",
        T::Text,
        "",
        "Targets picked by autocast.",
    ),
    f("AutoCastRange", T::Real, "0", "Range used by autocast."),
    f(
        "Flags",
        T::Flags(&[
            "Transient",
            "AllowMovement",
            "AutoCast",
            "AutoCastOn",
            "SmartCast",
            "NoDeceleration",
        ]),
        "0",
        "Ability flags.",
    ),
];

const BUFF_FIELDS: &[FieldDef] = &[
    f(
        "Duration",
        T::Real,
        "0",
        "Seconds the behavior lasts (0 = forever).",
    ),
    f("Period", T::Real, "0", "Seconds between periodic effects."),
    f(
        "InitialEffect",
        T::Link(K::Effect),
        "",
        "Effect run when the behavior is applied.",
    ),
    f(
        "PeriodicEffect",
        T::Link(K::Effect),
        "",
        "Effect run every period.",
    ),
    f(
        "FinalEffect",
        T::Link(K::Effect),
        "",
        "Effect run when the behavior ends for any reason.",
    ),
    f(
        "ExpireEffect",
        T::Link(K::Effect),
        "",
        "Effect run when the duration runs out.",
    ),
    f(
        "Modification",
        T::Struct,
        "",
        "Stat changes: speed, damage, armor, regeneration...",
    ),
    f("MaxStackCount", T::Int, "1", "How many copies can stack."),
    f(
        "Alignment",
        T::Enum(&["Positive", "Negative", "Neutral"]),
        "Neutral",
        "Whether this counts as a buff or a debuff.",
    ),
    f(
        "InfoIcon",
        T::Text,
        "",
        "Icon shown on the unit's status bar.",
    ),
    f(
        "BehaviorFlags",
        T::Flags(&[
            "Hidden",
            "Refresh",
            "Channeled",
            "RemoveOnDeath",
            "Permanent",
        ]),
        "0",
        "Behavior flags.",
    ),
    f(
        "ValidatorArray",
        T::LinkArray(K::Other),
        "",
        "Validators that must pass to apply.",
    ),
    f(
        "RemoveValidatorArray",
        T::LinkArray(K::Other),
        "",
        "Validators that remove the behavior when they fail.",
    ),
];

const UPGRADE_FIELDS: &[FieldDef] = &[
    f(
        "Race",
        T::Enum(RACES),
        "Neut",
        "Race the upgrade belongs to.",
    ),
    f(
        "MaxLevel",
        T::Int,
        "1",
        "How many times the upgrade can be researched.",
    ),
    f(
        "EffectArray",
        T::Struct,
        "",
        "Changes made per level, e.g. Reference=\"Effect,GaussRifle,Amount\" Value=\"1\".",
    ),
    f(
        "AffectedUnitArray",
        T::LinkArray(K::Unit),
        "",
        "Units affected (shown in tooltips).",
    ),
    f("Icon", T::Text, "", "Icon image path."),
    f("ScoreAmount", T::Int, "0", "Score awarded per level."),
    f(
        "Name",
        T::StringKey,
        "Upgrade/Name/<id>",
        "Text key of the upgrade name.",
    ),
];

const ACTOR_UNIT_FIELDS: &[FieldDef] = &[
    f("Model", T::Link(K::Model), "", "Model shown for the unit."),
    f(
        "DeathArray",
        T::Struct,
        "",
        "Death models and sounds per death type.",
    ),
    f("UnitIcon", T::Text, "", "Icon image path."),
    f(
        "Wireframe",
        T::Struct,
        "",
        "Wireframe image in the selection panel.",
    ),
    f("GroupIcon", T::Struct, "", "Icon in group selection."),
    f("MinimapIcon", T::Text, "", "Icon on the minimap."),
    f(
        "MinimapIconScale",
        T::Real,
        "1",
        "Scale of the minimap icon.",
    ),
    f(
        "SelectionRadius",
        T::Real,
        "0",
        "Radius of the selection circle.",
    ),
    f(
        "PortraitModel",
        T::Link(K::Model),
        "",
        "Model shown in the portrait.",
    ),
];

const MODEL_FIELDS: &[FieldDef] = &[
    f(
        "Model",
        T::Text,
        "",
        "Path of the .m3 file, e.g. Assets\\Units\\Terran\\Marine\\Marine.m3.",
    ),
    f(
        "Radius",
        T::Real,
        "0",
        "Radius used for culling and selection.",
    ),
    f("ScaleMin", T::Real, "1", "Smallest random scale."),
    f("ScaleMax", T::Real, "1", "Largest random scale."),
];

const BUTTON_FIELDS: &[FieldDef] = &[
    f(
        "Name",
        T::StringKey,
        "Button/Name/<id>",
        "Text key of the button name.",
    ),
    f(
        "Tooltip",
        T::StringKey,
        "Button/Tooltip/<id>",
        "Text key of the tooltip.",
    ),
    f("Icon", T::Text, "", "Icon image path."),
    f(
        "Hotkey",
        T::StringKey,
        "Button/Hotkey/<id>",
        "Text key of the hotkey.",
    ),
    f("AlertIcon", T::Text, "", "Icon used in alerts."),
];

const REQUIREMENT_FIELDS: &[FieldDef] = &[f(
    "NodeArray",
    T::Struct,
    "",
    "Requirement nodes for use and show conditions.",
)];

pub const CLASSES: &[ClassSchema] = &[
    ClassSchema {
        class: "CUnit",
        kind: K::Unit,
        desc: "A unit, structure or missile.",
        fields: UNIT_FIELDS,
    },
    ClassSchema {
        class: "CUnitHero",
        kind: K::Unit,
        desc: "A hero unit.",
        fields: UNIT_FIELDS,
    },
    ClassSchema {
        class: "CWeaponLegacy",
        kind: K::Weapon,
        desc: "A standard attack.",
        fields: WEAPON_FIELDS,
    },
    ClassSchema {
        class: "CWeaponStrafe",
        kind: K::Weapon,
        desc: "An attack that strafes the ground.",
        fields: WEAPON_FIELDS,
    },
    ClassSchema {
        class: "CEffectDamage",
        kind: K::Effect,
        desc: "Deals damage.",
        fields: DAMAGE_FIELDS,
    },
    ClassSchema {
        class: "CEffectLaunchMissile",
        kind: K::Effect,
        desc: "Launches a missile.",
        fields: LAUNCH_MISSILE_FIELDS,
    },
    ClassSchema {
        class: "CEffectSet",
        kind: K::Effect,
        desc: "Runs several effects.",
        fields: SET_FIELDS,
    },
    ClassSchema {
        class: "CEffectApplyBehavior",
        kind: K::Effect,
        desc: "Applies a behavior.",
        fields: APPLY_BEHAVIOR_FIELDS,
    },
    ClassSchema {
        class: "CEffectCreateUnit",
        kind: K::Effect,
        desc: "Creates units.",
        fields: CREATE_UNIT_FIELDS,
    },
    ClassSchema {
        class: "CAbilEffectTarget",
        kind: K::Ability,
        desc: "Targeted ability that runs an effect.",
        fields: ABIL_EFFECT_FIELDS,
    },
    ClassSchema {
        class: "CAbilEffectInstant",
        kind: K::Ability,
        desc: "Instant ability that runs an effect.",
        fields: ABIL_EFFECT_FIELDS,
    },
    ClassSchema {
        class: "CBehaviorBuff",
        kind: K::Behavior,
        desc: "A buff or debuff.",
        fields: BUFF_FIELDS,
    },
    ClassSchema {
        class: "CUpgrade",
        kind: K::Upgrade,
        desc: "A researchable upgrade.",
        fields: UPGRADE_FIELDS,
    },
    ClassSchema {
        class: "CActorUnit",
        kind: K::Actor,
        desc: "Visuals and sounds for a unit.",
        fields: ACTOR_UNIT_FIELDS,
    },
    ClassSchema {
        class: "CModel",
        kind: K::Model,
        desc: "A model file and its settings.",
        fields: MODEL_FIELDS,
    },
    ClassSchema {
        class: "CButton",
        kind: K::Button,
        desc: "A command card button.",
        fields: BUTTON_FIELDS,
    },
    ClassSchema {
        class: "CRequirement",
        kind: K::Requirement,
        desc: "Conditions for using or showing something.",
        fields: REQUIREMENT_FIELDS,
    },
];

pub fn class_schema(class: &str) -> Option<&'static ClassSchema> {
    CLASSES.iter().find(|c| c.class == class)
}

/// Schema for one field of a class (matched by element name).
pub fn field_def(class: &str, name: &str) -> Option<&'static FieldDef> {
    class_schema(class)?.fields.iter().find(|d| d.name == name)
}

/// Classes offered for new entries of `kind`, most common first.
pub fn classes_for_kind(kind: CatalogKind) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = CLASSES
        .iter()
        .filter(|c| c.kind == kind)
        .map(|c| c.class)
        .collect();
    if v.is_empty() {
        v.push(match kind {
            K::Other => "CValidatorUnitCompareVital",
            _ => "CUnit",
        });
    }
    v
}

/// Field completions for `class` matching `prefix` (case-insensitive),
/// prefix matches first. Indexed fields complete as `Name[Index]`.
pub fn suggest_fields(class: &str, prefix: &str) -> Vec<String> {
    let Some(schema) = class_schema(class) else {
        return Vec::new();
    };
    let p = prefix.trim().to_lowercase();
    let mut out: Vec<(u8, String)> = Vec::new();
    for d in schema.fields {
        let mut names = vec![d.name.to_owned()];
        if let FieldType::Flags(choices) | FieldType::IndexedReal(choices) = d.ty {
            names = choices.iter().map(|c| format!("{}[{c}]", d.name)).collect();
        }
        for n in names {
            let l = n.to_lowercase();
            if p.is_empty() || l.starts_with(&p) {
                out.push((0, n));
            } else if l.contains(&p) {
                out.push((1, n));
            }
        }
    }
    out.sort_by_key(|a| a.0);
    out.into_iter().map(|(_, n)| n).collect()
}

/// Columns shown in the Data table for `kind`, as field keys.
pub fn key_columns(kind: CatalogKind) -> &'static [&'static str] {
    match kind {
        K::Unit => &["LifeMax", "ShieldsMax", "Speed", "CostResource[Minerals]"],
        K::Weapon => &["Range", "Period", "Effect"],
        K::Effect => &["Amount", "Kind"],
        K::Ability => &["Effect", "Range"],
        K::Behavior => &["Duration", "Period"],
        K::Upgrade => &["MaxLevel", "Race"],
        K::Actor => &["Model"],
        K::Model => &["Model"],
        K::Button => &["Icon"],
        K::Requirement | K::Other => &[],
    }
}

/// Well-known base-game ids, offered as parents and used to sanity-check them.
pub fn base_ids(kind: CatalogKind) -> &'static [&'static str] {
    match kind {
        K::Unit => &[
            "Marine",
            "Marauder",
            "Reaper",
            "Ghost",
            "Hellion",
            "SiegeTank",
            "Cyclone",
            "Thor",
            "WidowMine",
            "VikingFighter",
            "Medivac",
            "Liberator",
            "Raven",
            "Banshee",
            "Battlecruiser",
            "SCV",
            "MULE",
            "CommandCenter",
            "SupplyDepot",
            "Barracks",
            "Factory",
            "Starport",
            "Bunker",
            "MissileTurret",
            "Probe",
            "Zealot",
            "Stalker",
            "Sentry",
            "Adept",
            "HighTemplar",
            "DarkTemplar",
            "Archon",
            "Immortal",
            "Colossus",
            "Disruptor",
            "Observer",
            "WarpPrism",
            "Phoenix",
            "VoidRay",
            "Oracle",
            "Tempest",
            "Carrier",
            "Mothership",
            "Nexus",
            "Pylon",
            "Gateway",
            "PhotonCannon",
            "Drone",
            "Zergling",
            "Baneling",
            "Roach",
            "Ravager",
            "Hydralisk",
            "Infestor",
            "Ultralisk",
            "Queen",
            "Overlord",
            "Overseer",
            "Mutalisk",
            "Corruptor",
            "BroodLord",
            "Viper",
            "Larva",
            "Hatchery",
            "SpawningPool",
            "SpineCrawler",
            "Broodling",
            "Interceptor",
            "MineralField",
            "VespeneGeyser",
            "XelNagaTower",
        ],
        K::Weapon => &[
            "GaussRifle",
            "PunisherGrenades",
            "C10CanisterRifle",
            "InfernalFlameThrower",
            "90mmCannons",
            "CrucioShockCannon",
            "ThorsHammer",
            "JavelinMissileLaunchers",
            "LanzerTorpedoes",
            "TwinGatlingCannon",
            "ATALaserBattery",
            "ATSLaserBattery",
            "PsiBlades",
            "ParticleDisruptors",
            "AcidSaliva",
            "NeedleSpines",
            "GlaiveWurm",
            "KaiserBlades",
        ],
        K::Effect => &[
            "GaussRifle",
            "PsiBlades",
            "DU_WEAP",
            "DU_WEAP_MISSILE",
            "DU_WEAP_SPLASH",
        ],
        K::Ability => &["move", "stop", "attack", "Stimpack"],
        K::Behavior => &["Stimpack", "GuardianShield"],
        K::Upgrade => &[
            "TerranInfantryWeaponsLevel1",
            "TerranInfantryArmorsLevel1",
            "Stimpack",
            "ShieldWall",
            "ProtossGroundWeaponsLevel1",
            "Charge",
            "BlinkTech",
        ],
        K::Actor => &["Marine", "Zealot", "Zergling", "GenericUnitStandard"],
        _ => &[],
    }
}

/// Default parent offered when creating a new entry of `class`.
pub fn default_parent(class: &str) -> &'static str {
    match class {
        "CUnit" => "Marine",
        "CWeaponLegacy" => "GaussRifle",
        "CEffectDamage" => "DU_WEAP",
        "CEffectLaunchMissile" => "DU_WEAP_MISSILE",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_lookups() {
        assert_eq!(field_def("CUnit", "LifeMax").unwrap().ty, FieldType::Real);
        assert!(matches!(
            field_def("CWeaponLegacy", "Effect").unwrap().ty,
            FieldType::Link(CatalogKind::Effect)
        ));
        let s = suggest_fields("CUnit", "life");
        assert!(s.starts_with(&["LifeStart".to_owned(), "LifeMax".to_owned()]));
        assert!(suggest_fields("CUnit", "cost").contains(&"CostResource[Minerals]".to_owned()));
        assert!(suggest_fields("CUnit", "regen").contains(&"LifeRegenRate".to_owned()));
        assert_eq!(classes_for_kind(CatalogKind::Effect)[0], "CEffectDamage");
        for c in CLASSES {
            let mut names: Vec<&str> = c.fields.iter().map(|d| d.name).collect();
            names.sort();
            let n = names.len();
            names.dedup();
            assert_eq!(n, names.len(), "duplicate field in {}", c.class);
        }
    }
}
