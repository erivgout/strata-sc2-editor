//! Regions (trigger areas) and cameras.

use serde::{Deserialize, Serialize};

use crate::objects::ObjectId;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Shape {
    Rect { min: glam::Vec2, max: glam::Vec2 },
    Circle { center: glam::Vec2, radius: f32 },
    /// Closed polygon, counter-clockwise.
    Polygon { points: Vec<glam::Vec2> },
}

impl Shape {
    pub fn contains(&self, p: glam::Vec2) -> bool {
        match self {
            Shape::Rect { min, max } => p.x >= min.x && p.y >= min.y && p.x <= max.x && p.y <= max.y,
            Shape::Circle { center, radius } => p.distance(*center) <= *radius,
            Shape::Polygon { points } => {
                let mut inside = false;
                let n = points.len();
                for i in 0..n {
                    let a = points[i];
                    let b = points[(i + n - 1) % n];
                    if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
                        inside = !inside;
                    }
                }
                inside
            }
        }
    }

    pub fn bounds(&self) -> (glam::Vec2, glam::Vec2) {
        match self {
            Shape::Rect { min, max } => (*min, *max),
            Shape::Circle { center, radius } => (*center - glam::Vec2::splat(*radius), *center + glam::Vec2::splat(*radius)),
            Shape::Polygon { points } => points.iter().fold(
                (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN)),
                |(lo, hi), p| (lo.min(*p), hi.max(*p)),
            ),
        }
    }

    pub fn translate(&mut self, d: glam::Vec2) {
        match self {
            Shape::Rect { min, max } => {
                *min += d;
                *max += d;
            }
            Shape::Circle { center, .. } => *center += d,
            Shape::Polygon { points } => points.iter_mut().for_each(|p| *p += d),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub id: ObjectId,
    pub name: String,
    /// sRGB display color.
    pub color: [u8; 3],
    /// Union of shapes (SC2 regions can hold several).
    pub shapes: Vec<Shape>,
}

impl Region {
    pub fn contains(&self, p: glam::Vec2) -> bool {
        self.shapes.iter().any(|s| s.contains(p))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub id: ObjectId,
    pub name: String,
    pub target: glam::Vec2,
    pub distance: f32,
    /// Pitch in degrees (SC2 default 56).
    pub pitch: f32,
    /// Yaw in degrees (SC2 default 90).
    pub yaw: f32,
    pub fov: f32,
}

impl Camera {
    pub fn new(id: ObjectId, name: impl Into<String>, target: glam::Vec2) -> Self {
        Self { id, name: name.into(), target, distance: 34.0, pitch: 56.0, yaw: 90.0, fov: 27.0 }
    }
}
