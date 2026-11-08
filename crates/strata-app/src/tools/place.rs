//! Placement modes: Units, Doodads, Points, Regions and Cameras.

use glam::{Vec2, Vec3};
use strata_core::history::Edit;
use strata_core::known;
use strata_core::objects::{ObjectKind, PlacedObject};
use strata_core::regions::{Camera, Region, Shape};

use super::select::Drag;
use super::{next_numbered, ToolInput, ToolState};
use crate::render::overlay;
use crate::state::{EditorState, Mode};
use crate::theme;

/// Default color for new regions (sRGB). Matches `ops::CreateRegion`.
const REGION_COLOR: [u8; 3] = [98, 205, 235];

impl ToolState {
    pub(crate) fn place_object_tool(&mut self, state: &mut EditorState, input: &ToolInput) {
        if !input.pressed {
            return;
        }
        let Some(h) = input.hover else { return };
        let kind = if state.mode == Mode::Units { ObjectKind::Unit } else { ObjectKind::Doodad };
        let type_id = state.place_type.trim().to_owned();
        if type_id.is_empty() {
            state.toast(crate::state::ToastKind::Info, "Pick a type in the Inspector first");
            return;
        }
        let xy = self.snap(h.truncate()).clamp(Vec2::ZERO, state.doc.world_size());
        let id = state.doc.objects.alloc_id();
        let mut o =
            PlacedObject::new(id, kind, type_id, Vec3::new(xy.x, xy.y, state.doc.terrain.sample_height(xy.x, xy.y)));
        o.rotation = self.settings.ghost_rotation;
        if kind == ObjectKind::Unit {
            // Resources, towers and rocks belong to the neutral player.
            o.player = if known::is_neutral_unit(&o.type_id) { 0 } else { state.place_player };
        }
        let label =
            format!("Place {}", known::unit_name(&o.type_id).or(known::doodad_name(&o.type_id)).unwrap_or(&o.type_id));
        state.commit(label, Edit::Object { id, before: None, after: Some(o) });
        state.selection.objects.clear();
        state.selection.objects.insert(id);
        state.selection.region = None;
        state.selection.camera = None;
        if !input.mods.shift {
            state.mode = Mode::Select;
        }
    }

    pub(crate) fn place_point_tool(&mut self, state: &mut EditorState, input: &ToolInput) {
        if !input.pressed {
            return;
        }
        let Some(h) = input.hover else { return };
        let xy = self.snap(h.truncate()).clamp(Vec2::ZERO, state.doc.world_size());
        let start = self.settings.point_type == known::POINT_START_LOCATION;
        let prefix = if start { "Start Location" } else { "Point" };
        let name = next_numbered(prefix, state.doc.objects.objects.iter().map(|o| o.name.as_str()));
        let id = state.doc.objects.alloc_id();
        let mut o = PlacedObject::new(
            id,
            ObjectKind::Point,
            self.settings.point_type.clone(),
            Vec3::new(xy.x, xy.y, state.doc.terrain.sample_height(xy.x, xy.y)),
        );
        o.name = name.clone();
        state.commit(format!("Place {name}"), Edit::Object { id, before: None, after: Some(o) });
        state.selection.objects.clear();
        state.selection.objects.insert(id);
    }

    pub(crate) fn region_tool(&mut self, state: &mut EditorState, input: &ToolInput) {
        if input.pressed && self.drag.is_none() {
            if let Some(h) = input.hover {
                self.drag = Some(Drag::Region { start: self.snap(h.truncate()), circle: input.mods.alt });
            }
        }
        let Some(Drag::Region { start, circle }) = self.drag.as_ref() else { return };
        if input.down {
            return;
        }
        let (start, circle) = (*start, *circle);
        self.drag = None;
        let Some(h) = input.hover else { return };
        let Some(shape) = region_shape(start, self.snap(h.truncate()), circle) else { return };
        let before = state.doc.regions.clone();
        let id = state.doc.objects.alloc_id();
        let name = next_numbered("Region", state.doc.regions.iter().map(|r| r.name.as_str()));
        let mut after = before.clone();
        after.push(Region { id, name: name.clone(), color: REGION_COLOR, shapes: vec![shape] });
        state.commit(format!("Create region {name}"), Edit::Regions { before, after });
        state.selection.objects.clear();
        state.selection.camera = None;
        state.selection.region = Some(id);
    }

    pub(crate) fn camera_tool(&mut self, state: &mut EditorState, input: &ToolInput) {
        if !input.pressed {
            return;
        }
        let Some(h) = input.hover else { return };
        let before = state.doc.cameras.clone();
        let id = state.doc.objects.alloc_id();
        let name = next_numbered("Camera", state.doc.cameras.iter().map(|c| c.name.as_str()));
        let mut cam = Camera::new(id, name.clone(), h.truncate());
        // Start from the angle the viewport is looking from.
        let pose = state.ui.viewport.camera.current;
        cam.yaw = pose.yaw.to_degrees().rem_euclid(360.0);
        cam.pitch = pose.pitch.to_degrees().clamp(10.0, 89.0);
        let mut after = before.clone();
        after.push(cam);
        state.commit(format!("Add {name}"), Edit::Cameras { before, after });
        state.selection.objects.clear();
        state.selection.region = None;
        state.selection.camera = Some(id);
    }

    pub(crate) fn paint_ghost(&self, painter: &egui::Painter, state: &EditorState, input: &ToolInput) {
        let Some(h) = input.hover else { return };
        let xy = self.snap(h.truncate());
        let p = Vec3::new(xy.x, xy.y, state.doc.terrain.sample_height(xy.x, xy.y));
        let tk = theme::tokens(painter.ctx());
        let unit = state.mode == Mode::Units;
        let color = if unit { overlay::player_color(&state.doc, state.place_player, &tk) } else { tk.text_dim };
        let mut ghost = PlacedObject::new(
            strata_core::objects::ObjectId(0),
            if unit { ObjectKind::Unit } else { ObjectKind::Doodad },
            state.place_type.clone(),
            p,
        );
        ghost.rotation = self.settings.ghost_rotation;
        overlay::draw_object(painter, &input.view, input.rect.min, &ghost, color.gamma_multiply(0.6), None, true, &tk);
    }

    pub(crate) fn paint_region_preview(&self, painter: &egui::Painter, state: &EditorState, input: &ToolInput) {
        let tk = theme::tokens(painter.ctx());
        if let (Some(Drag::Region { start, circle }), Some(h)) = (self.drag.as_ref(), input.hover) {
            if let Some(shape) = region_shape(*start, self.snap(h.truncate()), *circle) {
                overlay::draw_shape(painter, &input.view, input.rect.min, &state.doc.terrain, &shape, tk.accent, true);
            }
        } else {
            self.paint_crosshair(painter, input);
        }
    }

    pub(crate) fn paint_crosshair(&self, painter: &egui::Painter, input: &ToolInput) {
        let Some(h) = input.hover else { return };
        let tk = theme::tokens(painter.ctx());
        if let Some(p) = input.project(h) {
            let s = egui::Stroke::new(1.0, tk.text);
            painter.line_segment([p - egui::vec2(6.0, 0.0), p + egui::vec2(6.0, 0.0)], s);
            painter.line_segment([p - egui::vec2(0.0, 6.0), p + egui::vec2(0.0, 6.0)], s);
        }
    }
}

/// Region shape from a drag. `None` if the drag is too small.
pub fn region_shape(a: Vec2, b: Vec2, circle: bool) -> Option<Shape> {
    if circle {
        let r = a.distance(b);
        (r >= 0.5).then_some(Shape::Circle { center: a, radius: r })
    } else {
        let (min, max) = (a.min(b), a.max(b));
        let d = max - min;
        (d.x >= 0.5 && d.y >= 0.5).then_some(Shape::Rect { min, max })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_shapes_from_drags() {
        assert_eq!(
            region_shape(Vec2::new(4.0, 6.0), Vec2::new(1.0, 2.0), false),
            Some(Shape::Rect { min: Vec2::new(1.0, 2.0), max: Vec2::new(4.0, 6.0) })
        );
        assert_eq!(
            region_shape(Vec2::ZERO, Vec2::new(3.0, 4.0), true),
            Some(Shape::Circle { center: Vec2::ZERO, radius: 5.0 })
        );
        assert_eq!(region_shape(Vec2::ZERO, Vec2::new(0.1, 5.0), false), None);
    }
}
