//! Terrain brush modes: Sculpt, Paint, Cliff and Pathing.

use glam::{Vec2, Vec3};
use strata_core::brush::{Brush, BrushShape, BrushTool, Falloff, Stroke};
use strata_core::terrain::Terrain;

use super::{CliffOp, SculptTool, ToolInput, ToolSettings, ToolState};
use crate::state::{EditorState, Mode};
use crate::theme;

/// Height tools apply this fraction of the brush strength per dab, so a
/// stroke at spacing radius/4 builds up at a controllable rate.
const FLOW: f32 = 0.25;
/// Dabs per second while the pointer is held still (airbrush).
const STILL_RATE: f32 = 30.0;

pub(crate) struct ActiveStroke {
    stroke: Stroke,
    last: Vec2,
    carry: f32,
    still: f32,
    label: &'static str,
    continuous: bool,
}

/// Points along `from -> to` spaced `spacing` apart, continuing a stroke whose
/// last dab was `carry` units before `from`. Updates `carry`.
pub fn dab_positions(from: Vec2, to: Vec2, spacing: f32, carry: &mut f32) -> Vec<Vec2> {
    let spacing = spacing.max(0.05);
    let d = to - from;
    let len = d.length();
    if len <= 0.0 {
        return Vec::new();
    }
    let dir = d / len;
    let mut out = Vec::new();
    let mut t = spacing - *carry;
    while t <= len {
        out.push(from + dir * t);
        t += spacing;
    }
    *carry = len - (t - spacing);
    out
}

/// Dab spacing for a brush radius.
pub fn spacing_for(radius: f32) -> f32 {
    (radius * 0.25).max(0.25)
}

/// Normalized distance at which `falloff` drops to half weight (inner ring).
pub fn half_weight_radius(falloff: Falloff) -> Option<f32> {
    if falloff == Falloff::Constant {
        return None;
    }
    let (mut a, mut b) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let m = 0.5 * (a + b);
        if falloff.weight(m) > 0.5 {
            a = m;
        } else {
            b = m;
        }
    }
    Some(0.5 * (a + b))
}

/// The concrete brush for a stroke starting at `at` in `mode`.
pub fn effective_tool(mode: Mode, s: &ToolSettings, terrain: &Terrain, at: Vec2, invert: bool) -> Option<BrushTool> {
    Some(match mode {
        Mode::Sculpt => match (s.sculpt, invert) {
            (SculptTool::Raise, false) | (SculptTool::Lower, true) => BrushTool::Raise,
            (SculptTool::Raise, true) | (SculptTool::Lower, false) => BrushTool::Lower,
            (SculptTool::Smooth, _) => BrushTool::Smooth,
            (SculptTool::Flatten, _) => BrushTool::Flatten { target: terrain.sample_height(at.x, at.y) },
            (SculptTool::Noise, _) => BrushTool::Noise { seed: s.noise_seed },
        },
        Mode::Paint => {
            let layer = s.paint_layer.min(terrain.layers.len().saturating_sub(1));
            if s.paint_erase != invert {
                BrushTool::Erase { layer }
            } else {
                BrushTool::Paint { layer }
            }
        }
        Mode::Cliff => {
            let vx = (at.x.round().max(0.0) as u32).min(terrain.width);
            let vy = (at.y.round().max(0.0) as u32).min(terrain.height);
            let start = terrain.cliff_levels[terrain.vertex_index(vx, vy)];
            let op = match (s.cliff_op, invert) {
                (CliffOp::Raise, true) => CliffOp::Lower,
                (CliffOp::Lower, true) => CliffOp::Raise,
                (op, _) => op,
            };
            let level = match op {
                CliffOp::Set => s.cliff_level,
                CliffOp::Raise => (start + 1).min(15),
                CliffOp::Lower => start.saturating_sub(1),
            };
            BrushTool::Cliff { level }
        }
        Mode::Pathing => BrushTool::Pathing { flags: s.pathing_flags, on: s.pathing_clear == invert },
        _ => return None,
    })
}

impl ToolState {
    pub(crate) fn brush_tool(&mut self, state: &mut EditorState, input: &ToolInput) {
        // Keep `state.brush.tool` describing what a click would do right now.
        let invert = input.mods.command || (state.mode == Mode::Sculpt && input.mods.alt);
        if let Some(p) = input.hover {
            if let Some(t) = effective_tool(state.mode, &self.settings, &state.doc.terrain, p.truncate(), invert) {
                if self.stroke.is_none() {
                    state.brush.tool = t;
                }
            }
        }

        if input.pressed && self.stroke.is_none() {
            if let Some(p) = input.hover {
                self.begin_stroke(state, p.truncate(), invert);
            }
        }

        if let Some(s) = self.stroke.as_mut() {
            if input.down {
                if let Some(p) = input.hover {
                    let to = p.truncate();
                    let spacing = spacing_for(s.stroke.brush().radius);
                    let pts = dab_positions(s.last, to, spacing, &mut s.carry);
                    let mut dirty = self.frame_dirty.unwrap_or(empty_rect());
                    if pts.is_empty() {
                        s.still += input.dt;
                        if s.continuous && s.still >= 1.0 / STILL_RATE {
                            s.still = 0.0;
                            dirty = dirty.union(&s.stroke.dab(&mut state.doc.terrain, to));
                        }
                    } else {
                        s.still = 0.0;
                        for q in pts {
                            dirty = dirty.union(&s.stroke.dab(&mut state.doc.terrain, q));
                        }
                    }
                    s.last = to;
                    if !dirty.is_empty() {
                        self.frame_dirty = Some(dirty);
                        state.doc.dirty.terrain = true;
                    }
                }
            }
            if input.released || !input.down {
                self.finish_stroke(state);
            }
        }
    }

    fn begin_stroke(&mut self, state: &mut EditorState, at: Vec2, invert: bool) {
        let Some(tool) = effective_tool(state.mode, &self.settings, &state.doc.terrain, at, invert) else { return };
        if matches!(tool, BrushTool::Paint { .. } | BrushTool::Erase { .. }) && state.doc.terrain.layers.is_empty() {
            state.toast(crate::state::ToastKind::Info, "Add a texture layer in the Inspector before painting");
            return;
        }
        let continuous = matches!(
            tool,
            BrushTool::Raise
                | BrushTool::Lower
                | BrushTool::Smooth
                | BrushTool::Flatten { .. }
                | BrushTool::Noise { .. }
                | BrushTool::Paint { .. }
                | BrushTool::Erase { .. }
        );
        let mut brush = Brush { tool, ..state.brush };
        if continuous {
            brush.strength *= FLOW;
        }
        if matches!(tool, BrushTool::Noise { .. }) {
            self.settings.noise_seed = self.settings.noise_seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        }
        state.brush.tool = tool;
        let mut stroke = Stroke::begin(&state.doc.terrain, brush);
        let r = stroke.dab(&mut state.doc.terrain, at);
        if !r.is_empty() {
            self.frame_dirty = Some(self.frame_dirty.unwrap_or(empty_rect()).union(&r));
            state.doc.dirty.terrain = true;
        }
        self.stroke = Some(ActiveStroke { stroke, last: at, carry: 0.0, still: 0.0, label: tool.label(), continuous });
    }

    pub(crate) fn finish_stroke(&mut self, state: &mut EditorState) {
        if let Some(s) = self.stroke.take() {
            if let Some(edit) = s.stroke.finish(&state.doc.terrain) {
                state.push_edit(s.label, edit);
            }
        }
    }

    pub(crate) fn cancel_stroke(&mut self, state: &mut EditorState) {
        if let Some(s) = self.stroke.take() {
            let rect = s.stroke.rect();
            if let Some(edit) = s.stroke.finish(&state.doc.terrain) {
                edit.apply(&mut state.doc, false);
                self.frame_dirty = Some(self.frame_dirty.unwrap_or(empty_rect()).union(&rect));
            }
        }
    }

    pub(crate) fn paint_brush(&self, painter: &egui::Painter, state: &EditorState, input: &ToolInput) {
        let Some(c) = input.hover else { return };
        let tk = theme::tokens(painter.ctx());
        let b = &state.brush;
        let color = if self.stroke.is_some() { tk.accent } else { tk.text };
        let shadow = egui::Stroke::new(3.0, tk.bg_deep.gamma_multiply(0.7));
        let t = &state.doc.terrain;
        let center = c.truncate();
        let ring = |r: f32| -> Vec<egui::Pos2> { ring_points(t, center, r, b.shape, input) };
        let outer = ring(b.radius);
        if outer.len() > 2 {
            painter.add(egui::Shape::closed_line(outer.clone(), shadow));
            painter.add(egui::Shape::closed_line(outer, egui::Stroke::new(1.5, color)));
        }
        if let Some(h) = half_weight_radius(b.falloff) {
            let inner = ring(b.radius * h);
            if inner.len() > 2 {
                painter.add(egui::Shape::closed_line(inner, egui::Stroke::new(1.0, color.gamma_multiply(0.55))));
            }
        }
        if let Some(p) = input.project(c) {
            painter.circle_filled(p, 2.0, color);
        }
    }
}

fn empty_rect() -> strata_core::terrain::VertexRect {
    strata_core::terrain::VertexRect { x0: 0, y0: 0, x1: 0, y1: 0 }
}

/// Brush outline projected onto the terrain (heights sampled along the ring).
pub fn ring_points(t: &Terrain, center: Vec2, r: f32, shape: BrushShape, input: &ToolInput) -> Vec<egui::Pos2> {
    let n = ((r * 6.0) as usize).clamp(32, 160);
    let pts: Vec<Vec2> = match shape {
        BrushShape::Circle => (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                center + Vec2::new(a.cos(), a.sin()) * r
            })
            .collect(),
        BrushShape::Square => {
            let per = (n / 4).max(4);
            let corners = [Vec2::new(-r, -r), Vec2::new(r, -r), Vec2::new(r, r), Vec2::new(-r, r)];
            (0..4)
                .flat_map(|e| {
                    let (a, b) = (corners[e], corners[(e + 1) % 4]);
                    (0..per).map(move |i| center + a.lerp(b, i as f32 / per as f32))
                })
                .collect()
        }
    };
    pts.into_iter().filter_map(|p| input.project(Vec3::new(p.x, p.y, t.sample_height(p.x, p.y) + 0.06))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dabs_are_evenly_spaced_across_segments() {
        let mut carry = 0.0;
        let a = dab_positions(Vec2::ZERO, Vec2::new(2.5, 0.0), 1.0, &mut carry);
        assert_eq!(a, vec![Vec2::new(1.0, 0.0), Vec2::new(2.0, 0.0)]);
        assert!((carry - 0.5).abs() < 1e-5);
        let b = dab_positions(Vec2::new(2.5, 0.0), Vec2::new(4.2, 0.0), 1.0, &mut carry);
        assert_eq!(b.len(), 2);
        assert!((b[0].x - 3.0).abs() < 1e-5 && (b[1].x - 4.0).abs() < 1e-5);
        assert!((carry - 0.2).abs() < 1e-4);
    }

    #[test]
    fn short_moves_accumulate() {
        let mut carry = 0.0;
        let mut total = 0;
        let mut x = 0.0;
        for _ in 0..100 {
            total += dab_positions(Vec2::new(x, 0.0), Vec2::new(x + 0.1, 0.0), 1.33, &mut carry).len();
            x += 0.1;
        }
        // 10 units at spacing 1.33 -> 7 dabs.
        assert_eq!(total, 7);
    }

    #[test]
    fn zero_move_emits_nothing() {
        let mut carry = 0.3;
        assert!(dab_positions(Vec2::ONE, Vec2::ONE, 1.0, &mut carry).is_empty());
        assert_eq!(carry, 0.3);
    }

    #[test]
    fn spacing_is_quarter_radius() {
        assert_eq!(spacing_for(8.0), 2.0);
        assert_eq!(spacing_for(0.5), 0.25);
    }

    #[test]
    fn half_weight() {
        assert!((half_weight_radius(Falloff::Linear).unwrap() - 0.5).abs() < 1e-4);
        assert!(half_weight_radius(Falloff::Constant).is_none());
        let s = half_weight_radius(Falloff::Sharp).unwrap();
        assert!((Falloff::Sharp.weight(s) - 0.5).abs() < 1e-3);
    }

    #[test]
    fn invert_and_cliff_relative() {
        let mut t = Terrain::new(8, 8);
        let s = ToolSettings::default();
        let at = Vec2::new(4.0, 4.0);
        assert_eq!(effective_tool(Mode::Sculpt, &s, &t, at, false), Some(BrushTool::Raise));
        assert_eq!(effective_tool(Mode::Sculpt, &s, &t, at, true), Some(BrushTool::Lower));
        let i = t.vertex_index(4, 4);
        t.cliff_levels[i] = 5;
        let s = ToolSettings { cliff_op: CliffOp::Raise, ..ToolSettings::default() };
        assert_eq!(effective_tool(Mode::Cliff, &s, &t, at, false), Some(BrushTool::Cliff { level: 6 }));
        assert_eq!(effective_tool(Mode::Cliff, &s, &t, at, true), Some(BrushTool::Cliff { level: 4 }));
        let s = ToolSettings { pathing_clear: false, ..ToolSettings::default() };
        assert!(matches!(effective_tool(Mode::Pathing, &s, &t, at, true), Some(BrushTool::Pathing { on: false, .. })));
    }
}
