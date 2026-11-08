//! Viewport interaction tools, one per editing mode.
//!
//! The viewport gathers pointer/keyboard input into a [`ToolInput`] and calls
//! [`ToolState::handle`]. Every change a tool makes to the document ends up
//! in history as one undoable [`strata_core::history::Edit`].

pub mod brush;
pub mod place;
pub mod select;

use glam::{Vec2, Vec3};
use strata_core::terrain::VertexRect;

use crate::render::camera::View;
use crate::state::{EditorState, Mode};

/// Sculpt sub-tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SculptTool {
    #[default]
    Raise,
    Lower,
    Smooth,
    Flatten,
    Noise,
}

impl SculptTool {
    pub const ALL: [SculptTool; 5] =
        [SculptTool::Raise, SculptTool::Lower, SculptTool::Smooth, SculptTool::Flatten, SculptTool::Noise];

    pub fn label(self) -> &'static str {
        match self {
            SculptTool::Raise => "Raise",
            SculptTool::Lower => "Lower",
            SculptTool::Smooth => "Smooth",
            SculptTool::Flatten => "Flatten",
            SculptTool::Noise => "Noise",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SculptTool::Raise => "Raise terrain. Hold Ctrl or Alt to lower.",
            SculptTool::Lower => "Lower terrain. Hold Ctrl or Alt to raise.",
            SculptTool::Smooth => "Average heights to soften bumps.",
            SculptTool::Flatten => "Pull heights toward the height where the stroke started.",
            SculptTool::Noise => "Add natural-looking roughness.",
        }
    }
}

/// How the cliff brush picks its level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CliffOp {
    /// Paint the level chosen in the picker.
    #[default]
    Set,
    /// One level above wherever the stroke started.
    Raise,
    /// One level below wherever the stroke started.
    Lower,
}

impl CliffOp {
    pub const ALL: [CliffOp; 3] = [CliffOp::Set, CliffOp::Raise, CliffOp::Lower];

    pub fn label(self) -> &'static str {
        match self {
            CliffOp::Set => "Set level",
            CliffOp::Raise => "Raise one",
            CliffOp::Lower => "Lower one",
        }
    }
}

/// Options for every tool; edited from the brush HUD and the Inspector.
#[derive(Debug, Clone)]
pub struct ToolSettings {
    pub sculpt: SculptTool,
    pub paint_layer: usize,
    pub paint_erase: bool,
    pub cliff_op: CliffOp,
    pub cliff_level: u8,
    pub pathing_flags: u8,
    pub pathing_clear: bool,
    /// Snap moves and placements to `snap_step`.
    pub snap: bool,
    pub snap_step: f32,
    /// Facing for the next placed unit/doodad, radians.
    pub ghost_rotation: f32,
    pub point_type: String,
    pub noise_seed: u32,
    /// Texture id typed into "Add layer".
    pub new_layer_texture: String,
}

impl Default for ToolSettings {
    fn default() -> Self {
        Self {
            sculpt: SculptTool::Raise,
            paint_layer: 0,
            paint_erase: false,
            cliff_op: CliffOp::Set,
            cliff_level: 3,
            pathing_flags: strata_core::terrain::pathing::UNWALKABLE,
            pathing_clear: false,
            snap: true,
            snap_step: 0.5,
            ghost_rotation: 0.0,
            point_type: strata_core::known::POINT_NORMAL.into(),
            noise_seed: 1,
            new_layer_texture: String::new(),
        }
    }
}

/// Input for one frame, already resolved against the camera.
pub struct ToolInput {
    pub view: View,
    /// Viewport rect in screen points.
    pub rect: egui::Rect,
    /// Pointer in viewport-local points, if over the viewport.
    pub pointer: Option<Vec2>,
    /// Terrain point under the pointer.
    pub hover: Option<Vec3>,
    /// Primary button went down over the viewport (not an orbit gesture).
    pub pressed: bool,
    pub down: bool,
    pub released: bool,
    pub mods: egui::Modifiers,
    pub dt: f32,
    /// Viewport owns the keyboard this frame.
    pub keys: bool,
}

impl ToolInput {
    pub fn to_screen(&self, p: Vec2) -> egui::Pos2 {
        self.rect.min + egui::vec2(p.x, p.y)
    }

    pub fn project(&self, p: Vec3) -> Option<egui::Pos2> {
        self.view.project(p).map(|s| self.to_screen(s))
    }
}

#[derive(Default)]
pub struct ToolState {
    pub settings: ToolSettings,
    pub(crate) stroke: Option<brush::ActiveStroke>,
    pub(crate) drag: Option<select::Drag>,
    /// Vertex rect touched by brush dabs this frame (cells for pathing).
    pub frame_dirty: Option<VertexRect>,
    pub(crate) last_mode: Option<Mode>,
}

impl ToolState {
    pub fn is_busy(&self) -> bool {
        self.stroke.is_some() || self.drag.is_some()
    }

    pub fn stroking(&self) -> bool {
        self.stroke.is_some()
    }

    pub fn handle(&mut self, ctx: &egui::Context, state: &mut EditorState, input: &ToolInput) {
        if self.last_mode != Some(state.mode) {
            // Finish anything the previous mode left open.
            self.finish_stroke(state);
            self.cancel_drag(state);
            self.last_mode = Some(state.mode);
        }
        if input.keys && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.stroke.is_some() {
                self.cancel_stroke(state);
            } else if self.drag.is_some() {
                self.cancel_drag(state);
            } else if matches!(state.mode, Mode::Units | Mode::Doodads | Mode::Points | Mode::Regions | Mode::Cameras) {
                state.mode = Mode::Select;
            } else {
                state.selection.objects.clear();
                state.selection.region = None;
                state.selection.camera = None;
            }
        }
        match state.mode {
            Mode::Sculpt | Mode::Paint | Mode::Cliff | Mode::Pathing => self.brush_tool(state, input),
            Mode::Select => self.select_tool(ctx, state, input),
            Mode::Units | Mode::Doodads => self.place_object_tool(state, input),
            Mode::Points => self.place_point_tool(state, input),
            Mode::Regions => self.region_tool(state, input),
            Mode::Cameras => self.camera_tool(state, input),
        }
        if input.keys && matches!(state.mode, Mode::Select | Mode::Units | Mode::Doodads) {
            self.rotate_keys(ctx, state);
        }
    }

    /// Draw tool previews (brush ring, ghost, drag rectangles).
    pub fn paint(&self, painter: &egui::Painter, state: &EditorState, input: &ToolInput) {
        match state.mode {
            Mode::Sculpt | Mode::Paint | Mode::Cliff | Mode::Pathing => self.paint_brush(painter, state, input),
            Mode::Select => self.paint_select(painter, input),
            Mode::Units | Mode::Doodads => self.paint_ghost(painter, state, input),
            Mode::Regions => self.paint_region_preview(painter, state, input),
            Mode::Points | Mode::Cameras => self.paint_crosshair(painter, input),
        }
    }

    pub fn snap(&self, p: Vec2) -> Vec2 {
        if self.settings.snap && self.settings.snap_step > 0.0 {
            (p / self.settings.snap_step).round() * self.settings.snap_step
        } else {
            p
        }
    }
}

/// Smallest unused "Prefix NNN" number among `names`.
pub fn next_numbered<'a>(prefix: &str, names: impl Iterator<Item = &'a str>) -> String {
    let max = names.filter_map(|n| n.strip_prefix(prefix)?.trim().parse::<u32>().ok()).max().unwrap_or(0);
    format!("{prefix} {:03}", max + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbering_continues_after_highest() {
        let names = ["Point 001", "Point 007", "Other 100", "Point x"];
        assert_eq!(next_numbered("Point", names.iter().copied()), "Point 008");
        assert_eq!(next_numbered("Region", std::iter::empty()), "Region 001");
    }

    #[test]
    fn snapping() {
        let t = ToolState::default();
        assert_eq!(t.snap(Vec2::new(1.26, 3.74)), Vec2::new(1.5, 3.5));
    }
}
