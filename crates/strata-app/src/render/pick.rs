//! Ray picking against the terrain heightfield.

use glam::Vec3;
use strata_core::terrain::Terrain;

/// First intersection of a ray with the terrain surface, or `None` if the ray
/// misses the map. Marches in steps of at most half a cell over the part of
/// the ray inside the map's bounding box, then refines by bisection.
pub fn ray_terrain(terrain: &Terrain, origin: Vec3, dir: Vec3, (min_h, max_h): (f32, f32)) -> Option<Vec3> {
    let dir = dir.normalize_or_zero();
    if dir == Vec3::ZERO {
        return None;
    }
    let lo = Vec3::new(0.0, 0.0, min_h - 0.01);
    let hi = Vec3::new(terrain.width as f32, terrain.height as f32, max_h + 0.01);
    let (t0, t1) = ray_aabb(origin, dir, lo, hi)?;
    let above = |t: f32| {
        let p = origin + dir * t;
        p.z - terrain.sample_height(p.x, p.y)
    };
    // Step so each move covers at most half a cell horizontally.
    let horiz = dir.truncate().length().max(1e-4);
    let step = (0.5 / horiz).min(0.5 / dir.z.abs().max(1e-4)).max(0.05);
    let mut t_prev = t0;
    if above(t0) <= 0.0 {
        return Some(origin + dir * t0);
    }
    let mut t = t0;
    while t < t1 {
        t = (t + step).min(t1);
        let f = above(t);
        if f <= 0.0 {
            let (mut a, mut b) = (t_prev, t);
            for _ in 0..24 {
                let m = 0.5 * (a + b);
                if above(m) > 0.0 {
                    a = m;
                } else {
                    b = m;
                }
            }
            let p = origin + dir * b;
            return Some(Vec3::new(p.x, p.y, terrain.sample_height(p.x, p.y)));
        }
        t_prev = t;
    }
    None
}

/// Slab test. Returns the entry/exit distances along the ray (entry >= 0).
pub fn ray_aabb(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> Option<(f32, f32)> {
    let mut t0 = 0.0f32;
    let mut t1 = f32::INFINITY;
    for i in 0..3 {
        if d[i].abs() < 1e-9 {
            if o[i] < lo[i] || o[i] > hi[i] {
                return None;
            }
            continue;
        }
        let inv = 1.0 / d[i];
        let (mut a, mut b) = ((lo[i] - o[i]) * inv, (hi[i] - o[i]) * inv);
        if a > b {
            std::mem::swap(&mut a, &mut b);
        }
        t0 = t0.max(a);
        t1 = t1.min(b);
        if t0 > t1 {
            return None;
        }
    }
    Some((t0, t1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(h: f32) -> Terrain {
        let mut t = Terrain::new(32, 32);
        t.heights.iter_mut().for_each(|x| *x = h);
        t
    }

    #[test]
    fn hits_flat_ground() {
        let t = flat(5.0);
        let hit = ray_terrain(&t, Vec3::new(10.0, 10.0, 50.0), Vec3::new(0.1, 0.2, -1.0), (5.0, 5.0)).unwrap();
        assert!((hit.z - 5.0).abs() < 1e-3);
        let expect = Vec3::new(10.0, 10.0, 50.0) + Vec3::new(0.1, 0.2, -1.0) * 45.0;
        assert!((hit.truncate() - expect.truncate()).length() < 1e-2, "{hit:?}");
    }

    #[test]
    fn misses_outside_map() {
        let t = flat(5.0);
        assert!(ray_terrain(&t, Vec3::new(-10.0, -10.0, 50.0), Vec3::new(0.0, 0.0, -1.0), (5.0, 5.0)).is_none());
        assert!(ray_terrain(&t, Vec3::new(10.0, 10.0, 50.0), Vec3::new(0.0, 0.0, 1.0), (5.0, 5.0)).is_none());
    }

    #[test]
    fn hits_the_front_of_a_hill_not_the_ground_behind() {
        let mut t = flat(0.0);
        // A wall of height 10 at x in [15, 17].
        for y in 0..=32 {
            for x in 15..=17 {
                let i = t.vertex_index(x, y);
                t.heights[i] = 10.0;
            }
        }
        let (lo, hi) = t.min_max_height();
        // Low grazing ray travelling +X at z = 5.
        let hit = ray_terrain(&t, Vec3::new(0.0, 16.0, 5.0), Vec3::new(1.0, 0.0, -0.0001), (lo, hi)).unwrap();
        assert!(hit.x > 14.0 && hit.x < 15.0, "{hit:?}");
    }

    #[test]
    fn aabb_entry_exit() {
        let (a, b) = ray_aabb(Vec3::new(-1.0, 0.5, 0.5), Vec3::X, Vec3::ZERO, Vec3::ONE).unwrap();
        assert!((a - 1.0).abs() < 1e-6 && (b - 2.0).abs() < 1e-6);
    }
}
