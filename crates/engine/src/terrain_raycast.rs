//! Tool picking against canonical resident triangles, without allocating a second
//! render mesh for every editable cell in a distant terrain view.
use crate::{StreamedTerrainSurface, WorldOrigin};
use bevy::prelude::*;
use world::TerrainHeightfield;

pub fn raycast_resident_terrain<'a>(
    origin: &WorldOrigin,
    surfaces: impl IntoIterator<Item = (Entity, &'a StreamedTerrainSurface)>,
    ray: Ray3d,
) -> Option<(Entity, Vec3)> {
    let mut nearest = f32::INFINITY;
    let mut hit = None;
    for (entity, s) in surfaces {
        if Some(s.key.space) != origin.space() {
            continue;
        }
        let shift = Vec3::new(
            (i64::from(s.key.cell.x) - i64::from(origin.cell().x)) as f32 * s.cell_size,
            0.,
            (i64::from(s.key.cell.z) - i64::from(origin.cell().z)) as f32 * s.cell_size,
        );
        if let Some(t) = intersect(
            &s.heightfield,
            s.cell_size,
            ray.origin - shift,
            *ray.direction,
            nearest,
        ) {
            nearest = t;
            hit = Some((entity, ray.get_point(t)));
        }
    }
    hit
}
fn intersect(
    field: &TerrainHeightfield,
    size: f32,
    origin: Vec3,
    direction: Vec3,
    limit: f32,
) -> Option<f32> {
    if !origin.is_finite() || !direction.is_finite() || !size.is_finite() || size <= 0. {
        return None;
    }
    let mut lo = 0_f32;
    let mut hi = limit;
    for axis in [0, 2] {
        let o = origin[axis];
        let d = direction[axis];
        if d.abs() < 1e-8 {
            if o < 0. || o > size {
                return None;
            }
        } else {
            let a = -o / d;
            let b = (size - o) / d;
            lo = lo.max(a.min(b));
            hi = hi.min(a.max(b));
        }
    }
    if lo > hi {
        return None;
    }
    let end = usize::from(field.resolution) - 1;
    let step = size / end as f32;
    let quad = |t: f32| {
        let p = origin + direction * t;
        let x = (p.x / step).floor().clamp(0., (end - 1) as f32) as usize;
        let z = (p.z / step).floor().clamp(0., (end - 1) as f32) as usize;
        let vertex = |x, z| Vec3::new(x as f32 * step, field.height_at(x, z), z as f32 * step);
        let [a, b, c, d] = [
            vertex(x, z),
            vertex(x, z + 1),
            vertex(x + 1, z + 1),
            vertex(x + 1, z),
        ];
        [
            triangle(origin, direction, a, b, c),
            triangle(origin, direction, a, c, d),
        ]
        .into_iter()
        .flatten()
        .filter(|t| *t >= lo - 0.0001 && *t <= hi + 0.0001)
        .min_by(f32::total_cmp)
    };
    if !hi.is_finite() {
        return quad(lo);
    } // Vertical ray: just one grid square.
    let mut cuts = vec![lo, hi];
    for axis in [0, 2] {
        if direction[axis].abs() >= 1e-8 {
            for i in 1..end {
                let t = (i as f32 * step - origin[axis]) / direction[axis];
                if t > lo && t < hi {
                    cuts.push(t);
                }
            }
        }
    }
    cuts.sort_by(f32::total_cmp);
    cuts.dedup();
    for w in cuts.windows(2) {
        if let Some(t) = quad(w[0] + (w[1] - w[0]) * 0.5) {
            return Some(t.max(0.));
        }
    }
    None
}
fn triangle(o: Vec3, d: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e = b - a;
    let f = c - a;
    let p = d.cross(f);
    let det = e.dot(p);
    if det.abs() < 1e-8 {
        return None;
    }
    let inv = det.recip();
    let s = o - a;
    let u = s.dot(p) * inv;
    let q = s.cross(e);
    let v = d.dot(q) * inv;
    if u < -0.00001 || v < -0.00001 || u + v > 1.00001 {
        return None;
    }
    let t = f.dot(q) * inv;
    (t >= 0.).then_some(t)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn picks_actual_triangles_from_above_below_and_across_cells() {
        let field =
            TerrainHeightfield::from_heights(3, &[0., 0., 0., 0., 2., 0., 0., 0., 0.], 0., 2., 2.)
                .unwrap();
        for (x, z) in [(0.1, 0.7), (0.8, 0.2), (1., 1.), (1.8, 1.3)] {
            let height = field.sample([x, z], 2.).height;
            let t =
                intersect(&field, 2., Vec3::new(x, 10., z), Vec3::NEG_Y, f32::INFINITY).unwrap();
            assert!((10. - t - height).abs() < 0.0001);
            assert!(intersect(&field, 2., Vec3::new(x, -10., z), Vec3::Y, f32::INFINITY).is_some());
        }
        let t = intersect(&field, 2., Vec3::new(-1., 1., 1.), Vec3::X, 100.).unwrap();
        assert!((t - 1.5).abs() < 0.0001);
        assert!(intersect(&field, 2., Vec3::new(-1., 3., 1.), Vec3::X, 100.).is_none());
    }
}
