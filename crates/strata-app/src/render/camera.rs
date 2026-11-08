//! RTS-style orbit camera with smooth damping.
//!
//! World axes match SC2: +X east, +Y north, +Z up. Yaw is the compass
//! direction the camera looks along (radians, counter-clockwise from +X;
//! `PI / 2` looks north). Pitch is the angle above the ground plane.

use glam::{Mat4, Vec2, Vec3, Vec4Swizzles};

pub const MIN_PITCH: f32 = 0.26; // ~15 degrees
pub const MAX_PITCH: f32 = 1.5533; // ~89 degrees
pub const MIN_DISTANCE: f32 = 4.0;
pub const MAX_DISTANCE: f32 = 900.0;
pub const DEFAULT_PITCH: f32 = 0.977; // 56 degrees, the SC2 game camera
/// Pitch used when framing the whole map: a little lower than the game
/// camera so sculpted relief reads at a glance.
pub const MAP_PITCH: f32 = 0.855; // 49 degrees
pub const DEFAULT_YAW: f32 = std::f32::consts::FRAC_PI_2;
pub const FOV_Y: f32 = 0.82; // ~47 degrees

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    /// Orbit pivot in world space.
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
}

impl Default for Pose {
    fn default() -> Self {
        Self { target: Vec3::new(64.0, 64.0, 12.0), yaw: DEFAULT_YAW, pitch: DEFAULT_PITCH, distance: 80.0 }
    }
}

impl Pose {
    /// Horizontal look direction.
    pub fn forward_xy(&self) -> Vec2 {
        Vec2::new(self.yaw.cos(), self.yaw.sin())
    }

    /// Screen-right direction on the ground plane.
    pub fn right_xy(&self) -> Vec2 {
        Vec2::new(self.yaw.sin(), -self.yaw.cos())
    }

    pub fn eye(&self) -> Vec3 {
        let f = self.forward_xy();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + self.distance * Vec3::new(-f.x * cp, -f.y * cp, sp)
    }

    fn clamp(mut self) -> Self {
        self.pitch = self.pitch.clamp(MIN_PITCH, MAX_PITCH);
        self.distance = self.distance.clamp(MIN_DISTANCE, MAX_DISTANCE);
        self
    }
}

/// The camera: a goal pose set by input and a current pose that eases toward it.
#[derive(Debug, Clone)]
pub struct OrbitCamera {
    pub goal: Pose,
    pub current: Pose,
    /// Top-down orthographic view.
    pub ortho: bool,
    /// Pitch to return to when leaving top-down.
    saved_pitch: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        Self { goal: Pose::default(), current: Pose::default(), ortho: false, saved_pitch: DEFAULT_PITCH }
    }
}

/// Exponential smoothing factor for `dt` seconds at `rate` per second.
pub fn damp_factor(rate: f32, dt: f32) -> f32 {
    1.0 - (-rate * dt.max(0.0)).exp()
}

fn wrap_angle(a: f32) -> f32 {
    let tau = std::f32::consts::TAU;
    (a + std::f32::consts::PI).rem_euclid(tau) - std::f32::consts::PI
}

impl OrbitCamera {
    /// Advance damping. Returns true while still moving.
    pub fn update(&mut self, dt: f32) -> bool {
        self.goal = self.goal.clamp();
        let k = damp_factor(16.0, dt);
        let c = &mut self.current;
        let g = &self.goal;
        c.target += (g.target - c.target) * k;
        c.yaw += wrap_angle(g.yaw - c.yaw) * k;
        c.pitch += (g.pitch - c.pitch) * k;
        // Ease distance in log space so zooming feels uniform.
        c.distance = (c.distance.ln() + (g.distance.ln() - c.distance.ln()) * k).exp();
        let moving = (c.target - g.target).length() > 0.002 * g.distance.max(1.0)
            || wrap_angle(g.yaw - c.yaw).abs() > 1e-4
            || (c.pitch - g.pitch).abs() > 1e-4
            || (c.distance / g.distance - 1.0).abs() > 1e-4;
        if !moving {
            *c = *g;
        }
        moving
    }

    /// Jump straight to the goal (no easing).
    pub fn snap(&mut self) {
        self.goal = self.goal.clamp();
        self.current = self.goal;
    }

    pub fn set_ortho(&mut self, on: bool) {
        if on == self.ortho {
            return;
        }
        self.ortho = on;
        if on {
            self.saved_pitch = self.goal.pitch;
            self.goal.pitch = MAX_PITCH;
        } else {
            self.goal.pitch = self.saved_pitch.min(MAX_PITCH - 0.05);
        }
    }

    /// Orbit by a pointer delta in points.
    pub fn orbit(&mut self, delta: Vec2) {
        self.goal.yaw = wrap_angle(self.goal.yaw - delta.x * 0.008);
        if !self.ortho {
            self.goal.pitch = (self.goal.pitch + delta.y * 0.006).clamp(MIN_PITCH, MAX_PITCH);
        }
        self.current.yaw = self.goal.yaw;
        self.current.pitch = self.goal.pitch;
    }

    /// Move the goal target along the ground in camera-relative directions.
    /// `dir.x` is right, `dir.y` is forward, in world units.
    pub fn pan_relative(&mut self, dir: Vec2) {
        let f = self.goal.forward_xy();
        let r = self.goal.right_xy();
        let d = r * dir.x + f * dir.y;
        self.goal.target += d.extend(0.0);
    }

    /// Zoom by `factor` (< 1 zooms in) keeping world point `anchor` fixed on screen.
    pub fn zoom_at(&mut self, factor: f32, anchor: Option<Vec3>) {
        let old = self.goal.distance;
        let new = (old * factor).clamp(MIN_DISTANCE, MAX_DISTANCE);
        let k = new / old;
        if let Some(a) = anchor {
            // Scaling the eye about the anchor (eye' = a + k (eye - a)) keeps
            // the view direction, so the anchor stays under the cursor.
            self.goal.target = a + (self.goal.target - a) * k;
        }
        self.goal.distance = new;
    }

    /// Frame a world-space box: center at `center`, horizontal radius `radius`.
    pub fn frame(&mut self, center: Vec3, radius: f32) {
        self.goal.target = center;
        let half = FOV_Y * 0.5;
        self.goal.distance = (radius.max(4.0) / half.tan() * 1.15).clamp(MIN_DISTANCE, MAX_DISTANCE);
        if !self.ortho {
            self.goal.pitch = DEFAULT_PITCH;
        }
    }

    /// Frame `points` so they all fit in a `size` viewport inside `pad`
    /// (left, top, right, bottom, in points), looking from `yaw` / `pitch`.
    /// The target ends up on the plane `z` so orbiting pivots on the ground.
    pub fn fit(&mut self, points: &[Vec3], size: Vec2, pad: [f32; 4], yaw: f32, pitch: f32, z: f32) {
        if points.is_empty() {
            return;
        }
        let size = size.max(Vec2::splat(64.0));
        let lo = Vec2::new(pad[0], pad[1]);
        let hi = (size - Vec2::new(pad[2], pad[3])).max(lo + Vec2::splat(16.0));
        let avail = hi - lo;
        let center = points.iter().fold(Vec3::ZERO, |a, p| a + *p) / points.len() as f32;
        let mut pose = Pose { target: Vec3::new(center.x, center.y, z), yaw, pitch, distance: self.goal.distance };
        if self.ortho {
            pose.pitch = MAX_PITCH;
        }
        let ortho = self.ortho;
        // Screen bounds of the points, or None if any is behind the camera.
        let bounds = |pose: &Pose| -> Option<(Vec2, Vec2)> {
            let v = View::new(pose, ortho, size);
            let mut a = Vec2::splat(f32::MAX);
            let mut b = Vec2::splat(f32::MIN);
            for p in points {
                let s = v.project(*p)?;
                a = a.min(s);
                b = b.max(s);
            }
            Some((a, b))
        };
        for _ in 0..4 {
            // Smallest distance at which the projected box fits `avail`.
            let (mut near, mut far) = (MIN_DISTANCE, MAX_DISTANCE);
            for _ in 0..40 {
                let mid = (near * far).sqrt();
                pose.distance = mid;
                let fits = bounds(&pose).is_some_and(|(a, b)| {
                    let d = b - a;
                    d.x <= avail.x && d.y <= avail.y
                });
                if fits {
                    far = mid;
                } else {
                    near = mid;
                }
            }
            pose.distance = far;
            // Shift the target so the box sits centered in the padded area.
            let Some((a, b)) = bounds(&pose) else { break };
            let v = View::new(&pose, ortho, size);
            let (Some(from), Some(to)) = (v.ray_plane((a + b) * 0.5, z), v.ray_plane((lo + hi) * 0.5, z)) else {
                break;
            };
            pose.target += (from - to).truncate().extend(0.0);
        }
        self.goal.target = pose.target;
        self.goal.distance = pose.distance;
        self.goal.yaw = yaw;
        if !self.ortho {
            self.goal.pitch = pitch;
        }
    }

    /// View/projection for the current pose and a viewport size in points.
    pub fn view(&self, size: Vec2) -> View {
        View::new(&self.current, self.ortho, size)
    }
}

/// A frozen camera for one frame: matrices plus helpers for projection and rays.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub view_proj: Mat4,
    pub inv_view_proj: Mat4,
    pub eye: Vec3,
    pub forward: Vec3,
    pub ortho: bool,
    /// Viewport size in points.
    pub size: Vec2,
    /// World height visible at the target (ortho) or `2 * tan(fov / 2)` (perspective).
    pub scale: f32,
}

impl View {
    pub fn new(pose: &Pose, ortho: bool, size: Vec2) -> Self {
        let size = size.max(Vec2::splat(1.0));
        let aspect = size.x / size.y;
        let eye = pose.eye();
        let forward = (pose.target - eye).normalize_or(Vec3::NEG_Z);
        let right = pose.right_xy().extend(0.0);
        let up = right.cross(forward).normalize_or(Vec3::Y);
        let view = Mat4::look_at_rh(eye, pose.target, up);
        let far = pose.distance * 3.0 + 1500.0;
        let (proj, scale) = if ortho {
            let h = pose.distance * (FOV_Y * 0.5).tan();
            (Mat4::orthographic_rh_gl(-h * aspect, h * aspect, -h, h, 0.1, far), 2.0 * h)
        } else {
            let near = (pose.distance * 0.02).clamp(0.1, 4.0);
            (Mat4::perspective_rh_gl(FOV_Y, aspect, near, far), 2.0 * (FOV_Y * 0.5).tan())
        };
        let view_proj = proj * view;
        Self { view_proj, inv_view_proj: view_proj.inverse(), eye, forward, ortho, size, scale }
    }

    /// World to viewport-local points (origin top-left). `None` if behind the camera.
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let c = self.view_proj * p.extend(1.0);
        if c.w <= 1e-5 {
            return None;
        }
        let ndc = c.xyz() / c.w;
        Some(Vec2::new((ndc.x * 0.5 + 0.5) * self.size.x, (0.5 - ndc.y * 0.5) * self.size.y))
    }

    /// Pixels (points) per world unit at the depth of `p`.
    pub fn points_per_unit(&self, p: Vec3) -> f32 {
        if self.ortho {
            self.size.y / self.scale
        } else {
            let depth = (p - self.eye).dot(self.forward).max(0.01);
            self.size.y / (self.scale * depth)
        }
    }

    /// Ray through a viewport-local point. Returns (origin, normalized direction).
    pub fn ray(&self, screen: Vec2) -> (Vec3, Vec3) {
        let ndc = Vec2::new(screen.x / self.size.x * 2.0 - 1.0, 1.0 - screen.y / self.size.y * 2.0);
        let near = self.inv_view_proj.project_point3(ndc.extend(-1.0));
        let far = self.inv_view_proj.project_point3(ndc.extend(1.0));
        (near, (far - near).normalize_or(Vec3::NEG_Z))
    }

    /// Intersect a viewport ray with the horizontal plane `z = h`.
    pub fn ray_plane(&self, screen: Vec2, h: f32) -> Option<Vec3> {
        let (o, d) = self.ray(screen);
        if d.z.abs() < 1e-6 {
            return None;
        }
        let t = (h - o.z) / d.z;
        (t > 0.0).then(|| o + d * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: Vec3, b: Vec3, eps: f32) -> bool {
        (a - b).length() < eps
    }

    #[test]
    fn eye_is_behind_and_above_target() {
        let p = Pose { target: Vec3::new(10.0, 10.0, 0.0), yaw: DEFAULT_YAW, pitch: DEFAULT_PITCH, distance: 10.0 };
        let e = p.eye();
        assert!(e.z > 0.0);
        assert!(e.y < 10.0, "looking north means the eye is south of the target");
        assert!((e - p.target).length() - 10.0 < 1e-4);
    }

    #[test]
    fn project_center_and_ray_roundtrip() {
        let p = Pose { target: Vec3::new(32.0, 40.0, 4.0), yaw: 0.3, pitch: 0.9, distance: 50.0 };
        for ortho in [false, true] {
            let v = View::new(&p, ortho, Vec2::new(800.0, 600.0));
            let s = v.project(p.target).unwrap();
            assert!((s - Vec2::new(400.0, 300.0)).length() < 0.01, "{s:?}");
            let hit = v.ray_plane(Vec2::new(400.0, 300.0), 4.0).unwrap();
            assert!(approx(hit, p.target, 1e-2), "{hit:?}");
            // An arbitrary point projects back onto itself.
            let q = Vec3::new(20.0, 50.0, 4.0);
            let sq = v.project(q).unwrap();
            let back = v.ray_plane(sq, 4.0).unwrap();
            assert!(approx(back, q, 1e-2), "{back:?}");
        }
    }

    #[test]
    fn north_is_up_on_screen() {
        let p = Pose { target: Vec3::ZERO, yaw: DEFAULT_YAW, pitch: 1.2, distance: 30.0 };
        let v = View::new(&p, false, Vec2::new(400.0, 400.0));
        let n = v.project(Vec3::new(0.0, 5.0, 0.0)).unwrap();
        let e = v.project(Vec3::new(5.0, 0.0, 0.0)).unwrap();
        assert!(n.y < 200.0);
        assert!(e.x > 200.0);
    }

    #[test]
    fn top_down_is_stable() {
        let p = Pose { target: Vec3::ZERO, yaw: 1.0, pitch: MAX_PITCH, distance: 30.0 };
        let v = View::new(&p, true, Vec2::new(400.0, 300.0));
        assert!(v.view_proj.is_finite());
    }

    #[test]
    fn zoom_keeps_anchor_on_screen() {
        let mut c = OrbitCamera::default();
        c.snap();
        let size = Vec2::new(800.0, 600.0);
        let v = c.view(size);
        let cursor = Vec2::new(600.0, 200.0);
        let anchor = v.ray_plane(cursor, c.goal.target.z).unwrap();
        c.zoom_at(0.5, Some(anchor));
        c.snap();
        let after = c.view(size).project(anchor).unwrap();
        assert!((after - cursor).length() < 0.5, "{after:?}");
    }

    #[test]
    fn damping_converges() {
        let mut c = OrbitCamera::default();
        c.goal.target = Vec3::new(100.0, 20.0, 3.0);
        c.goal.distance = 30.0;
        let mut frames = 0;
        while c.update(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 600, "camera never settled");
        }
        assert_eq!(c.current, c.goal);
    }

    #[test]
    fn fit_keeps_every_point_inside_the_padding() {
        let mut c = OrbitCamera::default();
        let size = Vec2::new(900.0, 460.0);
        let pad = [16.0, 52.0, 16.0, 16.0];
        let pts: Vec<Vec3> = [(0.0, 0.0), (128.0, 0.0), (0.0, 128.0), (128.0, 128.0)]
            .iter()
            .flat_map(|&(x, y)| [Vec3::new(x, y, 8.0), Vec3::new(x, y, 20.0)])
            .collect();
        c.fit(&pts, size, pad, DEFAULT_YAW, MAP_PITCH, 12.0);
        c.snap();
        let v = c.view(size);
        let mut lo = Vec2::splat(f32::MAX);
        let mut hi = Vec2::splat(f32::MIN);
        for p in &pts {
            let s = v.project(*p).unwrap();
            lo = lo.min(s);
            hi = hi.max(s);
        }
        assert!(lo.x >= pad[0] - 1.0 && lo.y >= pad[1] - 1.0, "{lo:?}");
        assert!(hi.x <= size.x - pad[2] + 1.0 && hi.y <= size.y - pad[3] + 1.0, "{hi:?}");
        // Tight: one axis nearly fills the padded area.
        let used = (hi - lo) / (size - Vec2::new(pad[0] + pad[2], pad[1] + pad[3]));
        assert!(used.x.max(used.y) > 0.97, "{used:?}");
    }

    #[test]
    fn damp_factor_bounds() {
        assert_eq!(damp_factor(10.0, 0.0), 0.0);
        assert!(damp_factor(10.0, 10.0) > 0.999);
    }
}
