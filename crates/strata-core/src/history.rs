//! Undo/redo. Every change to a [`MapDocument`] goes through an [`Edit`] so
//! that UI tools, the command palette and MCP clients share one history.

use serde::{Deserialize, Serialize};

use crate::catalog::CatalogEntry;
use crate::document::{DirtyFlags, MapDocument, MapInfo};
use crate::objects::{ObjectId, PlacedObject};
use crate::regions::{Camera, Region};
use crate::strings::Strings;
use crate::terrain::TerrainPatch;
use crate::triggers::TriggerLibrary;

/// A reversible change. `apply(doc, true)` does it, `apply(doc, false)` undoes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Edit {
    Terrain { before: TerrainPatch, after: TerrainPatch },
    /// Insert (`before: None`), delete (`after: None`) or modify an object.
    Object { id: ObjectId, before: Option<PlacedObject>, after: Option<PlacedObject> },
    Regions { before: Vec<Region>, after: Vec<Region> },
    Cameras { before: Vec<Camera>, after: Vec<Camera> },
    Triggers { before: Box<TriggerLibrary>, after: Box<TriggerLibrary> },
    /// Insert / delete / modify a catalog entry at `index`.
    Data { index: usize, before: Option<CatalogEntry>, after: Option<CatalogEntry> },
    Strings { before: Box<Strings>, after: Box<Strings> },
    Info { before: Box<MapInfo>, after: Box<MapInfo> },
    Batch(Vec<Edit>),
}

impl Edit {
    pub fn apply(&self, doc: &mut MapDocument, forward: bool) {
        fn pick<'a, T>(forward: bool, b: &'a T, a: &'a T) -> &'a T {
            if forward { a } else { b }
        }
        match self {
            Edit::Terrain { before, after } => {
                doc.terrain.apply(pick(forward, before, after));
                doc.dirty.terrain = true;
            }
            Edit::Object { id, before, after } => {
                let target = pick(forward, before, after);
                match (doc.objects.objects.iter().position(|o| o.id == *id), target) {
                    (Some(i), Some(o)) => doc.objects.objects[i] = o.clone(),
                    (Some(i), None) => {
                        doc.objects.objects.remove(i);
                    }
                    (None, Some(o)) => doc.objects.objects.push(o.clone()),
                    (None, None) => {}
                }
                doc.dirty.objects = true;
            }
            Edit::Regions { before, after } => {
                doc.regions = pick(forward, before, after).clone();
                doc.dirty.regions = true;
            }
            Edit::Cameras { before, after } => {
                doc.cameras = pick(forward, before, after).clone();
                doc.dirty.cameras = true;
            }
            Edit::Triggers { before, after } => {
                doc.triggers = (**pick(forward, before, after)).clone();
                doc.dirty.triggers = true;
            }
            Edit::Data { index, before, after } => {
                let (from, to) = if forward { (before, after) } else { (after, before) };
                match (from, to) {
                    (None, Some(e)) => doc.data.entries.insert((*index).min(doc.data.entries.len()), e.clone()),
                    (Some(_), None) => {
                        if *index < doc.data.entries.len() {
                            doc.data.entries.remove(*index);
                        }
                    }
                    (Some(_), Some(e)) => {
                        if let Some(slot) = doc.data.entries.get_mut(*index) {
                            *slot = e.clone();
                        }
                    }
                    (None, None) => {}
                }
                doc.dirty.data = true;
            }
            Edit::Strings { before, after } => {
                doc.strings = (**pick(forward, before, after)).clone();
                doc.dirty.strings = true;
            }
            Edit::Info { before, after } => {
                doc.info = (**pick(forward, before, after)).clone();
                doc.dirty.info = true;
            }
            Edit::Batch(edits) => {
                if forward {
                    edits.iter().for_each(|e| e.apply(doc, true));
                } else {
                    edits.iter().rev().for_each(|e| e.apply(doc, false));
                }
            }
        }
    }

    /// Components this edit touches.
    pub fn dirty(&self) -> DirtyFlags {
        let mut d = DirtyFlags::default();
        match self {
            Edit::Terrain { .. } => d.terrain = true,
            Edit::Object { .. } => d.objects = true,
            Edit::Regions { .. } => d.regions = true,
            Edit::Cameras { .. } => d.cameras = true,
            Edit::Triggers { .. } => d.triggers = true,
            Edit::Data { .. } => d.data = true,
            Edit::Strings { .. } => d.strings = true,
            Edit::Info { .. } => d.info = true,
            Edit::Batch(es) => es.iter().for_each(|e| d.merge(e.dirty())),
        }
        d
    }
}

/// Where an edit came from; shown in the History panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    User,
    Mcp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub label: String,
    pub origin: Origin,
    pub edit: Edit,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct History {
    undo: Vec<HistoryEntry>,
    redo: Vec<HistoryEntry>,
    /// `undo.len()` at the last save; `None` if that state is unreachable.
    saved_at: Option<usize>,
    /// Bumped on every change; lets views cache derived data (meshes, scripts).
    revision: u64,
}

const MAX_UNDO: usize = 500;

impl History {
    /// Record an edit that has *already been applied* to the document.
    pub fn push(&mut self, label: impl Into<String>, origin: Origin, edit: Edit) {
        self.redo.clear();
        if let Some(s) = self.saved_at {
            if s > self.undo.len() {
                self.saved_at = None;
            }
        }
        self.undo.push(HistoryEntry { label: label.into(), origin, edit });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
            self.saved_at = self.saved_at.and_then(|s| s.checked_sub(1));
        }
        self.revision += 1;
    }

    /// Apply an edit to the document and record it.
    pub fn commit(&mut self, doc: &mut MapDocument, label: impl Into<String>, origin: Origin, edit: Edit) {
        edit.apply(doc, true);
        self.push(label, origin, edit);
    }

    pub fn undo(&mut self, doc: &mut MapDocument) -> Option<&str> {
        let e = self.undo.pop()?;
        e.edit.apply(doc, false);
        self.redo.push(e);
        self.revision += 1;
        self.redo.last().map(|e| e.label.as_str())
    }

    pub fn redo(&mut self, doc: &mut MapDocument) -> Option<&str> {
        let e = self.redo.pop()?;
        e.edit.apply(doc, true);
        self.undo.push(e);
        self.revision += 1;
        self.undo.last().map(|e| e.label.as_str())
    }

    /// Undo or redo until `undo.len() == target`.
    pub fn jump_to(&mut self, doc: &mut MapDocument, target: usize) {
        while self.undo.len() > target && self.undo(doc).is_some() {}
        while self.undo.len() < target && self.redo(doc).is_some() {}
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_entries(&self) -> &[HistoryEntry] {
        &self.undo
    }
    pub fn redo_entries(&self) -> &[HistoryEntry] {
        &self.redo
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn mark_saved(&mut self) {
        self.saved_at = Some(self.undo.len());
    }
    pub fn is_modified(&self) -> bool {
        self.saved_at != Some(self.undo.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::objects::ObjectKind;

    #[test]
    fn undo_redo_object() {
        let mut doc = MapDocument::new("t", 32, 32);
        let mut h = History::default();
        let id = doc.objects.alloc_id();
        let o = PlacedObject::new(id, ObjectKind::Unit, "Marine", glam::Vec3::new(4.0, 4.0, 0.0));
        h.commit(&mut doc, "Place Marine", Origin::User, Edit::Object { id, before: None, after: Some(o) });
        assert_eq!(doc.objects.objects.len(), 1);
        h.undo(&mut doc);
        assert!(doc.objects.objects.is_empty());
        h.redo(&mut doc);
        assert_eq!(doc.objects.objects.len(), 1);
    }

    #[test]
    fn saved_state_tracking() {
        let mut doc = MapDocument::new("t", 8, 8);
        let mut h = History::default();
        h.mark_saved();
        assert!(!h.is_modified());
        let before = Box::new(doc.info.clone());
        let mut info = doc.info.clone();
        info.name = "x".into();
        h.commit(&mut doc, "Rename", Origin::User, Edit::Info { before, after: Box::new(info) });
        assert!(h.is_modified());
        h.undo(&mut doc);
        assert!(!h.is_modified());
    }
}
