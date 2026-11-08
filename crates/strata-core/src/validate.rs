//! Document checks surfaced in the Problems panel and via MCP.

use serde::{Deserialize, Serialize};

use crate::document::MapDocument;
use crate::objects::{ObjectId, ObjectKind};
use crate::strings::DEFAULT_LOCALE;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// What a problem points at, so the UI can jump to it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Target {
    Map,
    Object(ObjectId),
    Region(ObjectId),
    Trigger(String),
    Data { class: String, id: String },
    String(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Problem {
    pub severity: Severity,
    pub message: String,
    pub target: Target,
}

pub fn validate(doc: &MapDocument) -> Vec<Problem> {
    let mut out = Vec::new();
    let mut push = |severity, message: String, target| out.push(Problem { severity, message, target });

    let [l, b, r, t] = doc.info.playable;
    if l >= r || b >= t || r > doc.info.width || t > doc.info.height {
        push(Severity::Error, "Playable area is empty or extends past the map edge.".into(), Target::Map);
    }
    if doc.info.name.trim().is_empty() {
        push(Severity::Warning, "Map has no name. Set one in Map properties.".into(), Target::Map);
    }
    let user_slots = doc.info.players.iter().filter(|p| p.control == crate::PlayerControl::User).count();
    if user_slots == 0 {
        push(Severity::Error, "No player slot is controlled by a user, so nobody can join.".into(), Target::Map);
    }

    let (w, h) = (doc.terrain.width as f32, doc.terrain.height as f32);
    for o in &doc.objects.objects {
        let p = o.position;
        if p.x < 0.0 || p.y < 0.0 || p.x > w || p.y > h {
            push(Severity::Error, format!("{} is outside the map.", o.display_name()), Target::Object(o.id));
        }
        if o.kind == ObjectKind::Unit {
            if o.type_id.is_empty() {
                push(Severity::Error, "A unit has no unit type.".into(), Target::Object(o.id));
            }
            if o.player as usize > 15 {
                push(Severity::Error, format!("{} belongs to player {}, which does not exist.", o.display_name(), o.player), Target::Object(o.id));
            }
        }
    }

    let mut seen = std::collections::HashSet::new();
    for t in doc.triggers.triggers() {
        let id = crate::galaxy::ident(&t.name);
        if !seen.insert(id.clone()) {
            push(Severity::Error, format!("Two triggers compile to the same name \"{id}\". Rename one."), Target::Trigger(t.name.clone()));
        }
        if t.events.is_empty() && t.enabled {
            push(Severity::Info, format!("Trigger \"{}\" has no events, so it only runs when another trigger calls it.", t.name), Target::Trigger(t.name.clone()));
        }
    }

    for r in &doc.regions {
        if r.shapes.is_empty() {
            push(Severity::Warning, format!("Region \"{}\" has no shapes.", r.name), Target::Region(r.id));
        }
    }

    if let Some(base) = doc.strings.locales.get(DEFAULT_LOCALE) {
        for (loc, map) in &doc.strings.locales {
            if loc == DEFAULT_LOCALE {
                continue;
            }
            let missing = base.keys().filter(|k| !map.contains_key(*k)).count();
            if missing > 0 {
                push(Severity::Info, format!("{loc} is missing {missing} translated strings."), Target::Map);
            }
        }
    }

    out.sort_by_key(|p| p.severity);
    out
}
