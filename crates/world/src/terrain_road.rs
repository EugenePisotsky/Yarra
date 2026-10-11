//! Roads as a terrain page carries them, and how a cart-track road is drawn: its wheel tracks,
//! grassy centre and shoulders, from the distance to its centre line. The environment compiler
//! lays roads into page weights and grass with [`RoadShape::zones`]; near terrain detail draws
//! them per pixel from [`TerrainRoads`], and `shaders/terrain_road.wesl` mirrors this module.
use crate::TerrainSurfaceId;
use serde::{Deserialize, Serialize};

/// Roads crossing a terrain page: their centre lines near it as segments, and how each road is
/// drawn. The page's weights and heights already carry the roads at their resolution; near
/// terrain detail draws wheel tracks, verges and ruts from these at full detail.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TerrainRoads {
    pub roads: Vec<TerrainRoad>,
    pub segments: Vec<TerrainRoadSegment>,
}

/// One road's drawing: its cart-track style (`environment::roads::CartTrackProfile`), metres,
/// and the ground its tracks are worn into.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TerrainRoad {
    pub shape: RoadShape,
    /// How much of the road's ground the centre strip and the shoulders show.
    pub center_ground: f32,
    pub shoulder_ground: f32,
    pub track_depth: f32,
    pub rut_roundness: f32,
    /// How strongly the road's ground replaces the natural ground (its Ground preset's strength).
    pub strength: f32,
    /// The road's ground: surfaces and their shares, summing to one.
    pub surfaces: Vec<(TerrainSurfaceId, f32)>,
}

/// A straight piece of a road's centre line, in page-local metres (ends may lie outside the page).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TerrainRoadSegment {
    pub a: [f32; 2],
    pub b: [f32; 2],
    /// Unit normals of the centre line at the ends.
    pub normal_a: [f32; 2],
    pub normal_b: [f32; 2],
    /// Corridor width at the ends.
    pub width: [f32; 2],
    /// Index into `TerrainRoads::roads`.
    pub road: u16,
}

/// The shape of a cart-track road's drawing, metres: two wheel tracks `track_spacing` apart
/// (centre to centre) and `track_width` wide, with edges `edge_softness` soft that wander by up
/// to `edge_variation` in patches `edge_patch_size` across, all broken up by `breakup` in patches
/// `breakup_patch_size` across. `seed` varies the patterns from road to road.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RoadShape {
    pub seed: u32,
    pub track_spacing: f32,
    pub track_width: f32,
    pub edge_softness: f32,
    pub edge_variation: f32,
    pub edge_patch_size: f32,
    pub breakup: f32,
    pub breakup_patch_size: f32,
}

/// A point's place beside a road: its distance to the nearest centre-line point, its offset along
/// the line's normal there (signed), how far past the line's end it lies, and the corridor width.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RoadOffset {
    pub distance: f64,
    pub lateral: f64,
    pub longitudinal: f64,
    pub width: f64,
}

/// How fully each part of a road covers a point, 0..1, within its irregular corridor: the wheel
/// tracks, the centre strip between them and the shoulders outside them; and the wear that
/// breaks them all up in patches.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoadZones {
    pub tracks: f64,
    pub center: f64,
    pub shoulder: f64,
    pub wear: f64,
}

impl RoadShape {
    /// The road's parts at a point `offset` from its centre line, `world` (XZ, metres).
    pub fn zones(&self, offset: &RoadOffset, world: [f64; 2]) -> RoadZones {
        let irregular = (road_patch(world, self.edge_patch_size, self.seed ^ 0x153) * 2.0 - 1.0)
            * f64::from(self.edge_variation);
        let feather = f64::from(self.edge_softness);
        let lateral = offset.lateral.abs();
        let track_outer = f64::from(self.track_spacing + self.track_width) * 0.5;
        let center_radius = f64::from(self.track_spacing - self.track_width) * 0.5;
        let corridor = falloff(offset.distance, offset.width * 0.5 + irregular, feather);
        let track_distance =
            (lateral - f64::from(self.track_spacing) * 0.5).hypot(offset.longitudinal);
        let tracks = falloff(
            track_distance,
            f64::from(self.track_width) * 0.5 + irregular,
            feather,
        ) * corridor;
        let center = falloff(lateral.hypot(offset.longitudinal), center_radius, feather) * corridor;
        let shoulder = smooth((lateral - center_radius) / feather)
            * smooth(
                (offset.width * 0.5 - lateral) / (offset.width * 0.5 - track_outer).max(feather),
            )
            * corridor;
        let patches =
            smooth((road_patch(world, self.breakup_patch_size, self.seed ^ 0x815) - 0.40) / 0.30);
        RoadZones {
            tracks,
            center,
            shoulder,
            wear: 1.0 - f64::from(self.breakup) * patches,
        }
    }
}

impl TerrainRoad {
    /// How much of the road's ground replaces the natural ground at a point `offset` from its
    /// centre line, `world` (XZ, metres), as the compiler lays it into page weights.
    pub fn ground(&self, offset: &RoadOffset, world: [f64; 2]) -> f64 {
        let z = self.shape.zones(offset, world);
        z.tracks
            .max(z.center * f64::from(self.center_ground))
            .max(z.shoulder * f64::from(self.shoulder_ground))
            * z.wear
            * f64::from(self.strength)
    }
}

/// The nearest road to a point: its index in `TerrainRoads::roads`, the point's offset from it,
/// and the nearest centre-line point (page-local) with the line's unit normal there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RoadHit {
    pub road: usize,
    pub offset: RoadOffset,
    pub point: [f64; 2],
    pub normal: [f64; 2],
}

impl TerrainRoads {
    /// The road whose centre line passes nearest `local` (page-local metres).
    pub fn nearest(&self, local: [f64; 2]) -> Option<RoadHit> {
        let mut best: Option<(f64, RoadHit)> = None;
        for s in &self.segments {
            let a = s.a.map(f64::from);
            let ab = [f64::from(s.b[0]) - a[0], f64::from(s.b[1]) - a[1]];
            let denominator = ab[0] * ab[0] + ab[1] * ab[1];
            let t = if denominator > 1e-12 {
                (((local[0] - a[0]) * ab[0] + (local[1] - a[1]) * ab[1]) / denominator)
                    .clamp(0.0, 1.0)
            } else {
                0.0
            };
            let q = [a[0] + ab[0] * t, a[1] + ab[1] * t];
            let delta = [local[0] - q[0], local[1] - q[1]];
            let distance_squared = delta[0] * delta[0] + delta[1] * delta[1];
            if best.is_some_and(|(d, _)| d <= distance_squared) {
                continue;
            }
            let m = [0, 1].map(|i| {
                f64::from(s.normal_a[i]) + (f64::from(s.normal_b[i]) - f64::from(s.normal_a[i])) * t
            });
            let length = m[0].hypot(m[1]);
            let n = if length > 1e-9 {
                m.map(|v| v / length)
            } else {
                s.normal_a.map(f64::from)
            };
            let lateral = delta[0] * n[0] + delta[1] * n[1];
            let width = f64::from(s.width[0]) + f64::from(s.width[1] - s.width[0]) * t;
            best = Some((
                distance_squared,
                RoadHit {
                    road: usize::from(s.road),
                    offset: RoadOffset {
                        distance: distance_squared.sqrt(),
                        lateral,
                        longitudinal: (distance_squared - lateral * lateral).max(0.0).sqrt(),
                        width,
                    },
                    point: q,
                    normal: n,
                },
            ));
        }
        best.map(|(_, hit)| hit)
    }

    /// The nearest road at `local` on a page whose origin is `origin` (world XZ, metres), and how
    /// much of its ground replaces the natural ground there.
    pub fn ground(&self, origin: [f64; 2], local: [f64; 2]) -> Option<(usize, f64)> {
        let hit = self.nearest(local)?;
        let world = [origin[0] + local[0], origin[1] + local[1]];
        Some((hit.road, self.roads[hit.road].ground(&hit.offset, world)))
    }
}

/// Domain-warped value noise, 0..1, in patches about `size` metres across, at world XZ `p`.
/// Its hash is 32-bit so shaders reproduce it.
pub fn road_patch(p: [f64; 2], size: f32, seed: u32) -> f64 {
    let p = p.map(|v| v / f64::from(size));
    let warp = [
        value(p.map(|v| v * 0.43), seed ^ 0x731) - 0.5,
        value(p.map(|v| v * 0.43), seed ^ 0x891) - 0.5,
    ];
    let p = [p[0] + warp[0] * 0.6, p[1] + warp[1] * 0.6];
    value(p, seed) * 0.75 + value(p.map(|v| v * 2.0), seed ^ 0x463) * 0.25
}
fn hash32(mut v: u32) -> u32 {
    v ^= v >> 16;
    v = v.wrapping_mul(0x7feb_352d);
    v ^= v >> 15;
    v = v.wrapping_mul(0x846c_a68b);
    v ^ (v >> 16)
}
fn hash(x: i64, z: i64, seed: u32) -> f64 {
    let v = (x as i32 as u32).wrapping_mul(0x8da6_b343)
        ^ (z as i32 as u32).wrapping_mul(0xd816_3841)
        ^ seed;
    f64::from(hash32(v)) / 4_294_967_296.0
}
fn quintic(t: f64) -> f64 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
fn value(p: [f64; 2], seed: u32) -> f64 {
    let x = p[0].floor() as i64;
    let z = p[1].floor() as i64;
    let u = quintic(p[0] - p[0].floor());
    let v = quintic(p[1] - p[1].floor());
    let a = hash(x, z, seed) * (1.0 - u) + hash(x + 1, z, seed) * u;
    let b = hash(x, z + 1, seed) * (1.0 - u) + hash(x + 1, z + 1, seed) * u;
    a * (1.0 - v) + b * v
}
fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
fn falloff(distance: f64, radius: f64, softness: f64) -> f64 {
    smooth((radius + softness - distance) / (2.0 * softness))
}
