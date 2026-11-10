//! Ground connectivity: which cells a ground unit can stand on, and whether
//! two points are joined by walkable ground.
//!
//! A cell is walkable when it is inside the playable area, its pathing
//! flags allow walking, and it is not a cliff. A cliff is a cell whose
//! corners sit on different cliff levels *and* whose height changes by more
//! than [`MAX_STEP`] across it; a ramp (cliff levels changing over a gentle
//! slope) stays walkable. Units and doodads placed on the map are not
//! counted as obstacles.
//!
//! Movement is 8-directional, including diagonal steps between two blocked
//! cells: the flags the official editor saves mark ramp edges as a
//! staircase, and real ladder ramps only open diagonally through it (62 of
//! 66 real melee maps connect their start locations this way, none without).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use crate::document::MapDocument;
use crate::terrain::{pathing, Terrain};

/// Largest height change across one cell that still counts as a slope.
pub const MAX_STEP: f32 = 1.0;

/// Why a cell can't be walked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Blocked {
    OutsidePlayable,
    Unpathable,
    Cliff,
}

impl Blocked {
    pub fn describe(self) -> &'static str {
        match self {
            Blocked::OutsidePlayable => "outside the playable area",
            Blocked::Unpathable => "on unpathable ground",
            Blocked::Cliff => "on a cliff edge",
        }
    }
}

pub struct WalkGrid {
    pub width: u32,
    pub height: u32,
    cells: Vec<Option<Blocked>>,
}

/// A walkable route between two points.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// Length in world units along the cells walked.
    pub length: f32,
    /// Cell centers along the route, simplified to where it turns.
    pub waypoints: Vec<[f32; 2]>,
}

/// Why no route exists.
#[derive(Debug, Clone, PartialEq)]
pub enum NoRoute {
    /// The start or end point itself is blocked or off the map.
    Endpoint { which: &'static str, why: String },
    /// Both ends are walkable but separated. `reachable` counts the cells
    /// reachable from the start.
    Separated { reachable: usize },
}

impl WalkGrid {
    pub fn new(doc: &MapDocument) -> Self {
        let t = &doc.terrain;
        let [l, b, r, top] = doc.info.playable;
        let (l, b) = (l.min(t.width), b.min(t.height));
        let (r, top) = (r.clamp(l, t.width), top.clamp(b, t.height));
        // A map without sensible bounds is treated as fully playable.
        let (l, b, r, top) = if r <= l || top <= b { (0, 0, t.width, t.height) } else { (l, b, r, top) };
        let mut cells = Vec::with_capacity((t.width * t.height) as usize);
        for cy in 0..t.height {
            for cx in 0..t.width {
                let blocked = if cx < l || cx >= r || cy < b || cy >= top {
                    Some(Blocked::OutsidePlayable)
                } else if t.pathing[t.cell_index(cx, cy)] & pathing::UNWALKABLE != 0 {
                    Some(Blocked::Unpathable)
                } else if is_cliff(t, cx, cy) {
                    Some(Blocked::Cliff)
                } else {
                    None
                };
                cells.push(blocked);
            }
        }
        Self { width: t.width, height: t.height, cells }
    }

    fn index(&self, c: (u32, u32)) -> usize {
        (c.1 * self.width + c.0) as usize
    }

    /// Why the cell at `c` is blocked, or `None` when it is walkable.
    pub fn blocked(&self, c: (u32, u32)) -> Option<Blocked> {
        if c.0 >= self.width || c.1 >= self.height {
            return Some(Blocked::OutsidePlayable);
        }
        self.cells[self.index(c)]
    }

    pub fn walkable(&self, c: (u32, u32)) -> bool {
        self.blocked(c).is_none()
    }

    /// The cell containing a world point, if it is on the map.
    pub fn cell_at(&self, x: f32, y: f32) -> Option<(u32, u32)> {
        if !(x.is_finite() && y.is_finite()) || x < 0.0 || y < 0.0 {
            return None;
        }
        let (cx, cy) = (x.floor() as u32, y.floor() as u32);
        // Points on the far edge belong to the last cell.
        let cx = if x == self.width as f32 { cx.saturating_sub(1) } else { cx };
        let cy = if y == self.height as f32 { cy.saturating_sub(1) } else { cy };
        (cx < self.width && cy < self.height).then_some((cx, cy))
    }

    fn neighbors(&self, c: (u32, u32)) -> impl Iterator<Item = ((u32, u32), u32)> + '_ {
        const STEPS: [(i32, i32, u32); 8] =
            [(1, 0, 10), (-1, 0, 10), (0, 1, 10), (0, -1, 10), (1, 1, 14), (1, -1, 14), (-1, 1, 14), (-1, -1, 14)];
        STEPS.iter().filter_map(move |&(dx, dy, cost)| {
            let (nx, ny) = (c.0 as i32 + dx, c.1 as i32 + dy);
            if nx < 0 || ny < 0 || nx >= self.width as i32 || ny >= self.height as i32 {
                return None;
            }
            let n = (nx as u32, ny as u32);
            self.walkable(n).then_some((n, cost))
        })
    }

    /// Label every walkable cell with its connected region (`u32::MAX` for blocked cells).
    pub fn components(&self) -> Vec<u32> {
        let mut label = vec![u32::MAX; self.cells.len()];
        let mut next = 0;
        let mut stack = Vec::new();
        for start in 0..self.cells.len() {
            if label[start] != u32::MAX || self.cells[start].is_some() {
                continue;
            }
            label[start] = next;
            stack.push(start);
            while let Some(i) = stack.pop() {
                let c = (i as u32 % self.width, i as u32 / self.width);
                for (n, _) in self.neighbors(c) {
                    let ni = self.index(n);
                    if label[ni] == u32::MAX {
                        label[ni] = next;
                        stack.push(ni);
                    }
                }
            }
            next += 1;
        }
        label
    }

    /// Shortest ground route between two world points (A*).
    pub fn route(&self, from: [f32; 2], to: [f32; 2]) -> Result<Route, NoRoute> {
        let end = |which: &'static str, p: [f32; 2]| -> Result<(u32, u32), NoRoute> {
            let c = self.cell_at(p[0], p[1]).ok_or_else(|| NoRoute::Endpoint {
                which,
                why: format!("({}, {}) is off the {}x{} map", p[0], p[1], self.width, self.height),
            })?;
            match self.blocked(c) {
                None => Ok(c),
                Some(b) => Err(NoRoute::Endpoint { which, why: format!("({}, {}) is {}", p[0], p[1], b.describe()) }),
            }
        };
        let (a, b) = (end("from", from)?, end("to", to)?);
        let h = |c: (u32, u32)| {
            let (dx, dy) = ((c.0 as i64 - b.0 as i64).unsigned_abs(), (c.1 as i64 - b.1 as i64).unsigned_abs());
            (10 * dx.max(dy) + 4 * dx.min(dy)) as u32
        };
        let n = self.cells.len();
        let mut cost = vec![u32::MAX; n];
        let mut came = vec![u32::MAX; n];
        let mut open = BinaryHeap::new();
        cost[self.index(a)] = 0;
        open.push(Reverse((h(a), 0u32, a)));
        let mut reached = 0usize;
        while let Some(Reverse((_, g, c))) = open.pop() {
            let ci = self.index(c);
            if g > cost[ci] {
                continue;
            }
            reached += 1;
            if c == b {
                let mut cells = vec![c];
                let mut i = ci;
                while came[i] != u32::MAX {
                    i = came[i] as usize;
                    cells.push((i as u32 % self.width, i as u32 / self.width));
                }
                cells.reverse();
                return Ok(Route { length: g as f32 / 10.0, waypoints: simplify(&cells) });
            }
            for (nb, step) in self.neighbors(c) {
                let ni = self.index(nb);
                let ng = g + step;
                if ng < cost[ni] {
                    cost[ni] = ng;
                    came[ni] = ci as u32;
                    open.push(Reverse((ng + h(nb), ng, nb)));
                }
            }
        }
        Err(NoRoute::Separated { reachable: reached })
    }
}

/// Keep the first and last cell and every cell where the direction changes.
fn simplify(cells: &[(u32, u32)]) -> Vec<[f32; 2]> {
    let center = |c: (u32, u32)| [c.0 as f32 + 0.5, c.1 as f32 + 0.5];
    let mut out = Vec::new();
    for (i, &c) in cells.iter().enumerate() {
        let turn = i > 0 && i + 1 < cells.len() && {
            let (p, n) = (cells[i - 1], cells[i + 1]);
            (c.0 as i64 - p.0 as i64, c.1 as i64 - p.1 as i64) != (n.0 as i64 - c.0 as i64, n.1 as i64 - c.1 as i64)
        };
        if i == 0 || i + 1 == cells.len() || turn {
            out.push(center(c));
        }
    }
    out
}

/// Corners on different cliff levels with a steep height change between them.
fn is_cliff(t: &Terrain, cx: u32, cy: u32) -> bool {
    let corners = [(cx, cy), (cx + 1, cy), (cx, cy + 1), (cx + 1, cy + 1)].map(|(x, y)| t.vertex_index(x, y));
    let (mut lo_l, mut hi_l) = (u8::MAX, 0u8);
    let (mut lo_h, mut hi_h) = (f32::MAX, f32::MIN);
    for i in corners {
        let (l, h) = (t.cliff_levels[i], t.heights[i]);
        lo_l = lo_l.min(l);
        hi_l = hi_l.max(l);
        lo_h = lo_h.min(h);
        hi_h = hi_h.max(h);
    }
    lo_l != hi_l && hi_h - lo_h > MAX_STEP
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::CLIFF_LEVEL_HEIGHT;

    fn open_map() -> MapDocument {
        let mut d = MapDocument::new("w", 32, 32);
        d.info.playable = [0, 0, 32, 32];
        d
    }

    /// Raise every vertex with x >= `from_x` by one cliff level.
    fn cliff_wall(d: &mut MapDocument, from_x: u32) {
        let t = &mut d.terrain;
        for y in 0..=t.height {
            for x in from_x..=t.width {
                let i = t.vertex_index(x, y);
                t.cliff_levels[i] += 1;
                t.heights[i] += CLIFF_LEVEL_HEIGHT;
            }
        }
    }

    #[test]
    fn open_ground_routes_straight() {
        let d = open_map();
        let g = WalkGrid::new(&d);
        let r = g.route([2.5, 2.5], [20.5, 2.5]).unwrap();
        assert_eq!(r.length, 18.0);
        assert_eq!(r.waypoints, vec![[2.5, 2.5], [20.5, 2.5]]);
    }

    #[test]
    fn cliffs_separate_and_unpathable_cells_block() {
        let mut d = open_map();
        cliff_wall(&mut d, 16);
        let g = WalkGrid::new(&d);
        assert_eq!(g.blocked((15, 3)), Some(Blocked::Cliff));
        assert!(matches!(g.route([2.5, 2.5], [25.5, 2.5]), Err(NoRoute::Separated { .. })));
        let labels = g.components();
        assert_ne!(labels[g.index((2, 2))], labels[g.index((25, 2))]);

        let mut d = open_map();
        for y in 0..32 {
            let i = d.terrain.cell_index(10, y);
            d.terrain.pathing[i] |= pathing::UNWALKABLE;
        }
        let g = WalkGrid::new(&d);
        let e = g.route([10.5, 4.0], [20.0, 4.0]).unwrap_err();
        assert!(matches!(e, NoRoute::Endpoint { which: "from", .. }), "{e:?}");
        assert!(g.route([2.0, 4.0], [20.0, 4.0]).is_err());
    }

    #[test]
    fn a_gentle_slope_between_levels_is_walkable() {
        let mut d = open_map();
        cliff_wall(&mut d, 16);
        // Ramp: from x = 10 to 22, rows 4..8, heights rise linearly.
        let t = &mut d.terrain;
        let (lo, hi) = (t.heights[t.vertex_index(0, 0)], t.heights[t.vertex_index(31, 0)]);
        for y in 4..=8 {
            for x in 10..=22 {
                let i = t.vertex_index(x, y);
                t.heights[i] = lo + (hi - lo) * (x - 10) as f32 / 12.0;
            }
        }
        let g = WalkGrid::new(&d);
        let r = g.route([2.5, 6.5], [28.5, 6.5]).expect("ramp connects");
        assert!(r.length >= 26.0);
    }
}
