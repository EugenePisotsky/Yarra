//! Contact demand is independent of the view. Certificates describe the surface
//! actually drawn, including the full sweep of a morph, rather than its target.
use super::{PatchMetadata, StitchEdges};
use bevy::math::DVec3;
use std::collections::BTreeMap;
use world::TerrainNodeKey;

pub const MAX_CONTACT_REGIONS: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ContactPriority {
    Vegetation,
    Actor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContactRegion {
    pub bounds: [DVec3; 2],
    pub exact: bool,
    pub tolerance: f32,
    /// Allocation priority only; never relaxes the surface readiness certificate.
    pub priority: ContactPriority,
}
impl ContactRegion {
    pub fn validate(&self) -> bool {
        self.bounds.iter().all(|p| p.is_finite())
            && self.bounds[0].cmple(self.bounds[1]).all()
            && self.tolerance.is_finite()
            && self.tolerance >= 0.
    }
    pub fn intersects(&self, bounds: [DVec3; 2]) -> bool {
        self.bounds[0].cmple(bounds[1]).all() && bounds[0].cmple(self.bounds[1]).all()
    }
    pub fn needs_refinement(&self, bounds: [DVec3; 2], level: u8, error: f32) -> bool {
        self.intersects(bounds)
            && ((self.exact && (level > 0 || error > 0.)) || error > self.tolerance)
    }
    pub fn accepts(&self, certificate: &ContactCertificate) -> bool {
        !overlaps_xz(self.bounds, certificate.bounds)
            || (certificate.error <= self.tolerance && (!self.exact || certificate.exact))
    }
}

#[derive(Clone, Debug)]
pub struct ContactCertificate {
    /// Canonical swept bounds, including possible horizontal vertex movement.
    pub bounds: [DVec3; 2],
    pub error: f32,
    pub exact: bool,
}
pub fn overlaps_xz(a: [DVec3; 2], b: [DVec3; 2]) -> bool {
    a[0].x <= b[1].x && b[0].x <= a[1].x && a[0].z <= b[1].z && b[0].z <= a[1].z
}

/// A stitched fan lies within a parent triangle. Its retained vertex heights may
/// deviate from that plane by E, while the authoritative surface differs by E too.
/// Missing parent metadata is unknown, never permission to ignore a seam.
pub fn patch_error(
    key: TerrainNodeKey,
    edges: StitchEdges,
    metadata: &BTreeMap<TerrainNodeKey, PatchMetadata>,
) -> f32 {
    if edges.0 == 0 {
        return metadata[&key].geometric_error;
    }
    key.parent()
        .ok()
        .flatten()
        .and_then(|p| metadata.get(&p))
        .map_or(f32::INFINITY, |m| 2. * m.geometric_error)
}

pub fn static_certificate(
    key: TerrainNodeKey,
    edges: StitchEdges,
    metadata: &BTreeMap<TerrainNodeKey, PatchMetadata>,
    cell_size: f64,
) -> ContactCertificate {
    let error = patch_error(key, edges, metadata);
    ContactCertificate {
        bounds: metadata[&key].bounds(cell_size),
        error,
        exact: key.level == 0 && error == 0.,
    }
}

pub fn cover_accepts(
    region: &ContactRegion,
    cover: &BTreeMap<TerrainNodeKey, StitchEdges>,
    metadata: &BTreeMap<TerrainNodeKey, PatchMetadata>,
    cell_size: f64,
) -> bool {
    covered(region, cover.keys().map(|k| metadata[k].bounds(cell_size)))
        && cover.iter().all(|(&key, &edges)| {
            region.accepts(&static_certificate(key, edges, metadata, cell_size))
        })
}

/// Conservative for every morph weight, including XZ collapse. Each animated
/// triangle lies within its swept AABB. Every authoritative height under that
/// footprint lies inside one of the old cover's descendant extrema. Unlike an
/// endpoint error, this interval bound does not assume vertical-only morphing.
pub fn morph_certificate(
    bounds: [DVec3; 2],
    old: impl Iterator<Item = TerrainNodeKey>,
    metadata: &BTreeMap<TerrainNodeKey, PatchMetadata>,
    cell_size: f64,
) -> ContactCertificate {
    let mut heights = [f64::INFINITY, f64::NEG_INFINITY];
    for key in old {
        let m = &metadata[&key];
        if overlaps_xz(bounds, m.bounds(cell_size)) {
            heights[0] = heights[0].min(f64::from(m.height_bounds[0]));
            heights[1] = heights[1].max(f64::from(m.height_bounds[1]));
        }
    }
    let bound = (bounds[1].y - heights[0])
        .abs()
        .max((heights[1] - bounds[0].y).abs());
    // Round outwards when converting to f32; never understate a positive bound.
    let error = if bound == 0. {
        0.
    } else {
        (bound as f32).next_up()
    };
    ContactCertificate {
        bounds,
        error,
        exact: error == 0.,
    }
}

/// Coverage and accuracy are separate: a sparse authored world must not certify
/// an empty hole just because no inaccurate patch intersects it. Covers are
/// non-overlapping; sum clipped areas to require the complete requested footprint.
pub fn covered(region: &ContactRegion, cover: impl Iterator<Item = [DVec3; 2]>) -> bool {
    let mut area = 0.;
    let expected =
        (region.bounds[1].x - region.bounds[0].x) * (region.bounds[1].z - region.bounds[0].z);
    if expected == 0. {
        // A line request must cover its full length, not just touch one patch.
        // The same union handles a point (a zero-length interval).
        let axis = if region.bounds[0].x == region.bounds[1].x {
            2
        } else {
            0
        };
        let mut intervals: Vec<_> = cover
            .filter(|b| overlaps_xz(region.bounds, *b))
            .map(|b| {
                (
                    b[0][axis].max(region.bounds[0][axis]),
                    b[1][axis].min(region.bounds[1][axis]),
                )
            })
            .collect();
        intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut end = region.bounds[0][axis];
        for (low, high) in intervals {
            if low > end {
                return false;
            }
            end = end.max(high);
            if end >= region.bounds[1][axis] {
                return true;
            }
        }
        return false;
    }
    for bounds in cover {
        if overlaps_xz(region.bounds, bounds) {
            let width = region.bounds[1].x.min(bounds[1].x) - region.bounds[0].x.max(bounds[0].x);
            let depth = region.bounds[1].z.min(bounds[1].z) - region.bounds[0].z.max(bounds[0].z);
            area += width.max(0.) * depth.max(0.);
        }
    }
    area >= expected * (1. - 1e-12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lod::{LodSettings, LodView, plan_cover_with_contacts};
    use bevy::math::DMat4;
    use std::collections::BTreeSet;
    use world::WorldSpaceId;
    fn key(level: u8, x: i32, z: i32) -> TerrainNodeKey {
        TerrainNodeKey {
            space: WorldSpaceId(1),
            level,
            x,
            z,
        }
    }
    fn region(x: f64, z: f64) -> ContactRegion {
        ContactRegion {
            bounds: [DVec3::new(x, -10., z), DVec3::new(x + 0.5, 10., z + 0.5)],
            exact: true,
            tolerance: 0.,
            priority: ContactPriority::Actor,
        }
    }
    #[test]
    fn sparse_cover_does_not_certify_missing_ground() {
        let a = [DVec3::ZERO, DVec3::new(1., 1., 1.)];
        let b = a.map(|p| p + DVec3::X * 2.);
        assert!(covered(&region(0.25, 0.25), [a, b].into_iter()));
        assert!(!covered(&region(1.25, 0.25), [a, b].into_iter()));
        assert!(!covered(&region(0.75, 0.25), [a, b].into_iter()));
        assert!(!covered(&region(0., 0.), [].into_iter()));
        let line = ContactRegion {
            bounds: [DVec3::new(0.5, 0., 0.5), DVec3::new(2.5, 0., 0.5)],
            ..region(0., 0.)
        };
        assert!(!covered(&line, [a, b].into_iter()));
    }
    #[test]
    fn contacts_refine_offscreen_actors_and_report_insufficient_budget() {
        let root = key(2, -1, -1);
        let mut metadata = BTreeMap::new();
        let mut todo = vec![root];
        while let Some(k) = todo.pop() {
            metadata.insert(
                k,
                PatchMetadata {
                    key: k,
                    resolution: 5,
                    height_bounds: [0., 0.],
                    geometric_error: 0.,
                },
            );
            if let Some(children) = k.children().unwrap() {
                todo.extend(children);
            }
        }
        let view = LodView {
            clip_from_world: DMat4::orthographic_rh(-1., 1., -1., 1., 0.1, 10000.)
                * DMat4::look_at_rh(
                    DVec3::new(1000., 1000., 1000.),
                    DVec3::new(1000., 0., 1000.),
                    DVec3::Z,
                ),
            viewport: [512, 512],
            contact_position: DVec3::new(1000., 1000., 1000.),
        };
        let settings = LodSettings {
            exact_radius: 0.,
            contact_radius: 0.,
            ..Default::default()
        };
        let r = region(-12., -12.);
        let p = plan_cover_with_contacts(
            &[root],
            &metadata,
            &BTreeSet::new(),
            &view,
            8.,
            &settings,
            std::slice::from_ref(&r),
        )
        .unwrap();
        assert!(cover_accepts(&r, &p.patches, &metadata, 8.));
        assert!(!p.stats.contact_limited);
        let p = plan_cover_with_contacts(
            &[root],
            &metadata,
            &BTreeSet::new(),
            &view,
            8.,
            &LodSettings {
                max_patches: 1,
                ..settings
            },
            std::slice::from_ref(&r),
        )
        .unwrap();
        assert!(p.stats.contact_limited && p.stats.budget_limited);
        assert!(!cover_accepts(&r, &p.patches, &metadata, 8.));
    }
    #[test]
    fn swept_interval_is_conservative_and_unknown_seams_stay_closed() {
        let k = key(1, 0, 0);
        let metadata = BTreeMap::from([(
            k,
            PatchMetadata {
                key: k,
                resolution: 5,
                height_bounds: [-7., 13.],
                geometric_error: 0.,
            },
        )]);
        let c = morph_certificate(
            [DVec3::new(1., -3., 1.), DVec3::new(7., 11., 7.)],
            [k].into_iter(),
            &metadata,
            8.,
        );
        assert!(c.error >= 18. && !c.exact);
        assert!(!region(2., 2.).accepts(&c));
        let c = static_certificate(k, StitchEdges(1), &metadata, 8.);
        assert!(c.error.is_infinite());
        let c = morph_certificate([DVec3::ZERO, DVec3::ONE], [].into_iter(), &metadata, 8.);
        assert!(c.error.is_infinite());
    }
    #[test]
    fn elevated_contact_does_not_force_detail_in_a_valley() {
        let r = ContactRegion {
            bounds: [DVec3::new(0., 900., 0.), DVec3::new(8., 1100., 8.)],
            exact: true,
            tolerance: 0.,
            priority: ContactPriority::Vegetation,
        };
        assert!(!r.needs_refinement([DVec3::ZERO, DVec3::splat(8.)], 5, 20.));
    }
}
