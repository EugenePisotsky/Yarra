//! Roads near a terrain cell as the cook draws them into its wear field, and how a cart-track
//! road is drawn: its wheel tracks, grassy centre and shoulders, from the distance to its centre
//! line ([`RoadShape::zones`]). [`TerrainRoads::wear`] gives the field at a point
//! ([`world::TerrainWear`]); the compiler's grass follows the same zones.
use serde::{Deserialize, Serialize};
use world::WearSample;
use world::road_noise::{falloff, road_patch, smooth, value, value4};

/// Roads near a terrain cell: their centre lines as segments, how each road is drawn, and the
/// junctions where they meet.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct TerrainRoads {
    /// In the order the compiler lays them, each over the ones before.
    pub roads: Vec<TerrainRoad>,
    pub segments: Vec<TerrainRoadSegment>,
    /// Junctions near the page, laid over every road.
    pub junctions: Vec<TerrainRoadJunction>,
}

/// Where roads meet (`environment::roads::RoadJunction`): within `radius` of `centre`
/// (page-local metres) its roads give way to one worn patch of the ground of `road`, whose
/// breakup, strength and surfaces it takes. `members` has a bit per index into
/// `TerrainRoads::roads` for each road that meets there.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TerrainRoadJunction {
    pub centre: [f32; 2],
    pub radius: f32,
    pub seed: u32,
    pub road: u16,
    pub members: u32,
}

impl TerrainRoadJunction {
    /// How fully the junction's patch replaces its roads at `local` (page-local; `world` XZ).
    pub fn blend(&self, local: [f64; 2], world: [f64; 2]) -> f64 {
        let d = (local[0] - f64::from(self.centre[0])).hypot(local[1] - f64::from(self.centre[1]));
        junction_blend(d, self.radius, world, self.seed)
    }

    /// How much of its ground the patch lays at `world` (XZ), over everything, `road` being its
    /// road's drawing: worn bare in patches a metre or so across, trampled between them.
    pub fn ground(&self, road: &TerrainRoad, local: [f64; 2], world: [f64; 2]) -> f64 {
        let patches = smooth(
            (road_patch(world, road.shape.breakup_patch_size, self.seed ^ 0x815) - 0.40) / 0.30,
        );
        let bare = smooth((road_patch(world, 1.3, self.seed ^ 0x31c) - 0.3) / 0.4);
        (1.0 - f64::from(road.shape.breakup) * patches)
            * (0.45 + 0.55 * bare)
            * self.blend(local, world)
            * f64::from(road.strength)
    }

    pub fn joins(&self, road: usize) -> bool {
        road < 32 && self.members & (1 << road) != 0
    }
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
    /// How strongly the road wears the natural ground (its Ground preset's strength).
    pub strength: f32,
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
/// (centre to centre) and `track_width` wide, with edges `edge_softness` soft and ragged by up to
/// `edge_variation`, in patches `edge_patch_size` across and bites a fifth of that. Each track
/// wanders across the road by up to `track_wander` over about `track_wander_length` of it, and
/// its width by half that. `breakup` restores the ground in patches `breakup_patch_size` across.
/// `seed` varies the patterns from road to road.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RoadShape {
    pub seed: u32,
    pub track_spacing: f32,
    pub track_width: f32,
    pub edge_softness: f32,
    pub edge_variation: f32,
    pub edge_patch_size: f32,
    pub track_wander: f32,
    pub track_wander_length: f32,
    pub breakup: f32,
    pub breakup_patch_size: f32,
}

/// A point's place beside a road: its distance to the nearest centre-line point, its offset along
/// the line's normal there (signed), how far past the line's end it lies, the corridor width, and
/// the nearest centre-line point (world XZ).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RoadOffset {
    pub distance: f64,
    pub lateral: f64,
    pub longitudinal: f64,
    pub width: f64,
    pub point: [f64; 2],
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
    /// The wheel tracks where the centre line passes `point` (world XZ): each one's offset from
    /// the line (the left one negative) and half-width. A footpath (`track_spacing` 0) has one.
    pub fn tracks(&self, point: [f64; 2]) -> [(f64, f64); 2] {
        let half_spacing = f64::from(self.track_spacing) * 0.5;
        let half_width = f64::from(self.track_width) * 0.5;
        let mut tracks = [(-half_spacing, half_width), (half_spacing, half_width)];

        if self.track_wander > 0.0 {
            let wander = f64::from(self.track_wander);
            let mut n = value4(
                point.map(|v| v / f64::from(self.track_wander_length)),
                self.seed ^ 0x5a1,
            )
            .map(|v| v * 2.0 - 1.0);

            // A footpath's one track (no spacing) wanders as one.
            if self.track_spacing == 0.0 {
                n = [n[0], n[0], n[2], n[2]];
            }

            tracks[0].0 += wander * n[0];
            tracks[1].0 += wander * n[1];
            tracks[0].1 += wander * 0.5 * n[2];
            tracks[1].1 += wander * 0.5 * n[3];
        }

        tracks
    }

    /// How far the road's edges stand out at `world` (negative: in): broad patches and fine bites.
    pub fn ragged(&self, world: [f64; 2]) -> f64 {
        let size = f64::from(self.edge_patch_size);
        let broad = road_patch(world, self.edge_patch_size, self.seed ^ 0x153) * 2.0 - 1.0;
        let bites = value(world.map(|v| v * 5.0 / size), self.seed ^ 0x2e1) * 2.0 - 1.0;
        (broad * 0.7 + bites * 0.3) * f64::from(self.edge_variation)
    }

    /// The road's parts at a point `offset` from its centre line, `world` (XZ, metres).
    pub fn zones(&self, offset: &RoadOffset, world: [f64; 2]) -> RoadZones {
        let irregular = self.ragged(world);
        let feather = f64::from(self.edge_softness);
        let (lateral, longitudinal) = (offset.lateral, offset.longitudinal);
        let [(left, left_half), (right, right_half)] = self.tracks(offset.point);
        let corridor = falloff(offset.distance, offset.width * 0.5 + irregular, feather);
        let track = |centre: f64, half: f64| {
            falloff(
                (lateral - centre).hypot(longitudinal),
                half + irregular,
                feather,
            )
        };
        let tracks = track(left, left_half).max(track(right, right_half)) * corridor;

        // The strip between the tracks' inner edges, and outside each track its shoulder.
        let inner = [left + left_half, right - right_half];
        let middle = (inner[0] + inner[1]) * 0.5;
        let center = falloff(
            (lateral - middle).hypot(longitudinal),
            (inner[1] - inner[0]) * 0.5,
            feather,
        ) * corridor;
        let (out, near, far) = if lateral < middle {
            (-lateral, -inner[0], left_half - left)
        } else {
            (lateral, inner[1], right + right_half)
        };
        let half_width = offset.width * 0.5;
        let shoulder = smooth((out - near) / feather)
            * smooth((half_width - out) / (half_width - far).max(feather))
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
    /// How deep the road's ruts lie at a point `offset` from its centre line, metres. Horses and
    /// carts, not cars: each wheel track is a shallow worn trough with a narrow wheel groove in
    /// it that wobbles across the track, now and then doubled where another cart's gauge ran
    /// beside it. Ruts deepen and shallow along the road (over twice the wander length by 30%,
    /// every metre or two by 20%), fade out over stretches of several metres and sink into
    /// potholes a couple of metres long where water stands; the deeper of the two tracks.
    pub fn rut(&self, offset: &RoadOffset) -> f64 {
        if self.track_depth <= 0.0 {
            return 0.0;
        }

        let shape = &self.shape;
        let centre = offset.point;
        let along = |size: f64, seed: u32| value4(centre.map(|v| v / size), shape.seed ^ seed);
        let slow = along(2.0 * f64::from(shape.track_wander_length.max(0.5)), 0x77d);
        let fast = along(1.4, 0x3c7);
        let wobble = along(3.1, 0x6b5);
        let breaks = along(7.3, 0x1f9);
        let holes = along(2.3, 0x4d3);
        let power = 2.0 + 6.0 * (1.0 - f64::from(self.rut_roundness));
        let profile = |distance: f64, half: f64, power: f64| {
            smooth(1.0 - (distance / half.max(0.01)).min(1.0).powf(power))
        };
        shape
            .tracks(centre)
            .iter()
            .enumerate()
            .map(|(i, &(at, half))| {
                let kept = 0.3 + 0.7 * smooth((breaks[i] - 0.2) / 0.35);
                let hole = 1.0 + 0.9 * smooth((holes[i] - 0.74) / 0.16);
                let depth = f64::from(self.track_depth)
                    * (0.7 + 0.6 * slow[i])
                    * (0.8 + 0.4 * fast[2 + i])
                    * kept
                    * hole;
                let distance = |from: f64| (offset.lateral - from).hypot(offset.longitudinal);
                let trough = profile(distance(at), half, power);

                // The wheel: the groove an iron tyre presses into soft ground, a few times its
                // own width and never finer than the wear field resolves, off the track's middle.
                let groove_half = (half * 0.45).clamp(0.1, 0.2);
                let groove_at = at + (wobble[i] * 2.0 - 1.0) * half * 0.4;
                let groove = profile(distance(groove_at), groove_half, 2.0);
                let outward = if i == 0 { -1.0 } else { 1.0 };
                let second = profile(
                    distance(groove_at + outward * groove_half * 1.7),
                    groove_half,
                    2.0,
                ) * 0.7
                    * smooth((wobble[2 + i] - 0.6) / 0.2);
                depth * (0.35 * trough + 0.65 * groove.max(second))
            })
            .fold(0.0, f64::max)
    }

    /// How fully the road's whole width covers a point `offset` from its centre line, `world`
    /// (XZ, metres): tracks, centre strip and shoulders alike.
    pub fn corridor(&self, offset: &RoadOffset, world: [f64; 2]) -> f64 {
        let z = self.shape.zones(offset, world);
        z.tracks.max(z.center).max(z.shoulder) * f64::from(self.strength)
    }

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
    /// The road whose centre line passes nearest `local` (page-local metres). Its offset's
    /// `point` is page-local too.
    pub fn nearest(&self, local: [f64; 2]) -> Option<RoadHit> {
        self.nearest_of(local, |_| true)
    }

    /// The nearest centre-line point of road `road` (an index into `roads`).
    pub fn nearest_on(&self, road: usize, local: [f64; 2]) -> Option<RoadHit> {
        self.nearest_of(local, |r| r == road)
    }

    fn nearest_of(&self, local: [f64; 2], keep: impl Fn(usize) -> bool) -> Option<RoadHit> {
        let mut best: Option<(f64, RoadHit)> = None;

        for s in self.segments.iter().filter(|s| keep(usize::from(s.road))) {
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
                        point: q,
                    },
                    point: q,
                    normal: n,
                },
            ));
        }

        best.map(|(_, hit)| hit)
    }

    /// How the page's roads lay their ground at `local` on a page whose origin is `origin`
    /// (world XZ, metres), as the compiler lays it into page weights: each road in order, then
    /// each junction, replacing that share of all ground laid before with its own. Amounts per
    /// layer: roads by index, then junctions after them.
    pub fn layers(&self, origin: [f64; 2], local: [f64; 2]) -> Vec<f64> {
        self.layers_by(origin, local, TerrainRoad::ground)
    }

    /// Each road's share at `local` by `share`, then each junction's ground.
    fn layers_by(
        &self,
        origin: [f64; 2],
        local: [f64; 2],
        share: impl Fn(&TerrainRoad, &RoadOffset, [f64; 2]) -> f64,
    ) -> Vec<f64> {
        let world = [origin[0] + local[0], origin[1] + local[1]];
        let mut layers: Vec<_> = (0..self.roads.len())
            .map(|road| {
                let Some(mut hit) = self.nearest_on(road, local) else {
                    return 0.0;
                };
                hit.offset.point = [origin[0] + hit.point[0], origin[1] + hit.point[1]];
                let joined = self
                    .junctions
                    .iter()
                    .filter(|j| j.joins(road))
                    .map(|j| j.blend(local, world))
                    .fold(0.0, f64::max);
                share(&self.roads[road], &hit.offset, world) * (1.0 - joined)
            })
            .collect();
        layers.extend(
            self.junctions
                .iter()
                .map(|j| j.ground(&self.roads[usize::from(j.road)], local, world)),
        );
        layers
    }

    /// The wear field at `local` on a page whose origin is `origin`, as the cook draws it: the
    /// share of the ground roads and junctions wear down, the deepest rut (flattened where
    /// junctions take over), and the direction of the road that wears it most.
    pub fn wear(&self, origin: [f64; 2], local: [f64; 2]) -> WearSample {
        let world = [origin[0] + local[0], origin[1] + local[1]];
        let mut kept = 1.0;
        let mut rut = 0.0_f64;
        let mut strongest = (0.0, [0.0; 2]);

        for (road, drawn) in self.roads.iter().enumerate() {
            let Some(mut hit) = self.nearest_on(road, local) else {
                continue;
            };
            hit.offset.point = [origin[0] + hit.point[0], origin[1] + hit.point[1]];
            let joined = self
                .junctions
                .iter()
                .filter(|j| j.joins(road))
                .map(|j| j.blend(local, world))
                .fold(0.0, f64::max);
            let amount = drawn.ground(&hit.offset, world) * (1.0 - joined);
            kept *= 1.0 - amount;
            rut = rut.max(drawn.rut(&hit.offset) * (1.0 - joined));

            if amount > strongest.0 {
                strongest = (amount, [hit.normal[1], -hit.normal[0]]);
            }
        }

        for j in &self.junctions {
            kept *= 1.0 - j.ground(&self.roads[usize::from(j.road)], local, world);
        }

        WearSample {
            wear: 1.0 - kept,
            rut,
            direction: strongest.1,
        }
    }

    /// The share of the ground at `local` that roads and junctions lay, of all their kinds.
    pub fn ground(&self, origin: [f64; 2], local: [f64; 2]) -> f64 {
        combined(self.layers(origin, local))
    }

    /// How fully roads' whole widths and junctions cover the ground at `local`.
    pub fn corridor(&self, origin: [f64; 2], local: [f64; 2]) -> f64 {
        combined(self.layers_by(origin, local, TerrainRoad::corridor))
    }
}

/// Overlapping shares combined as independent layers.
fn combined(layers: Vec<f64>) -> f64 {
    1.0 - layers.into_iter().map(|a| 1.0 - a).product::<f64>()
}

/// How fully a junction's worn patch replaces its roads `distance` metres from its centre, at
/// `world` (XZ): a common centre, then a broad transition into each road's own tracks, the
/// patch's edge ragged by up to a third of its radius in broad patches.
pub fn junction_blend(distance: f64, radius: f32, world: [f64; 2], seed: u32) -> f64 {
    let ragged = 1.0 + 0.3 * (road_patch(world, radius * 0.8, seed ^ 0x5e1) * 2.0 - 1.0);
    smooth((1.0 - distance / (f64::from(radius) * ragged)) / 0.65)
}
