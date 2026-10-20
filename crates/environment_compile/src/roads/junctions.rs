use super::*;

pub(super) struct PlannedJunction {
    pub source: RoadJunction,
    /// The road (index into the plan's roads) whose style and wear the junction's patch takes:
    /// the busiest of its roads in the junction's own style.
    pub hub: usize,
    pub profile: CartTrackProfile,
    pub wear: RoadWear,
    pub ground_preset: environment::Preset,
    pub roads: Vec<RoadId>,
    pub bounds: RoadCellBounds,
}

impl PlannedJunction {
    pub fn blocks_scatter(&self, cell: CellCoord, uv: [f64; 2], size: f32, clearance: f32) -> bool {
        let p = self.source.position.relative_to(cell, f64::from(size));
        (uv[0] * f64::from(size) - p[0]).hypot(uv[1] * f64::from(size) - p[1])
            < f64::from(self.source.radius + clearance)
    }

    pub fn blend(&self, cell: CellCoord, uv: [f64; 2], size: f32) -> f64 {
        if !self.bounds.contains(cell) {
            return 0.0;
        }

        let p = self.source.position.relative_to(cell, f64::from(size));
        let distance = (uv[0] * f64::from(size) - p[0]).hypot(uv[1] * f64::from(size) - p[1]);
        let world = [
            (f64::from(cell.x) + uv[0]) * f64::from(size),
            (f64::from(cell.z) + uv[1]) * f64::from(size),
        ];
        world::junction_blend(distance, self.source.radius, world, self.source.seed)
    }

    /// How much of the natural ground the patch wears at `uv` where it holds fully.
    pub fn worn(&self, cell: CellCoord, uv: [f64; 2], size: f32) -> f64 {
        let world = [
            (f64::from(cell.x) + uv[0]) * f64::from(size),
            (f64::from(cell.z) + uv[1]) * f64::from(size),
        ];
        let patches = smooth(
            (world::road_patch(
                world,
                self.profile.breakup_patch_size,
                self.source.seed ^ 0x815,
            ) - 0.40)
                / 0.30,
        );
        (1.0 - f64::from(self.wear.breakup(self.profile.breakup)) * patches)
            * f64::from(self.wear.tracks)
            * strength(&self.ground_preset)
    }

    pub fn depth(&self, cell: CellCoord, uv: [f64; 2], size: f32) -> f64 {
        let r = &self.profile.relief;
        let world = [
            (f64::from(cell.x) + uv[0]) * f64::from(size),
            (f64::from(cell.z) + uv[1]) * f64::from(size),
        ];
        let broad = world::road_patch(world, r.road_variation_length, self.source.seed ^ 0x906a)
            * 2.0
            - 1.0;
        let rut = world::road_patch(world, r.track_variation_length, self.source.seed ^ 0x418c)
            * 2.0
            - 1.0;

        // Ruts merge into a shallow worn basin, never accumulate into intersecting trenches.
        f64::from(r.road_depth) * (1.0 + f64::from(r.road_variation) * broad)
            + 0.5 * f64::from(r.track_depth) * (1.0 + f64::from(r.track_variation) * rut)
    }
}

impl RoadPlan {
    pub(super) fn joined_depth(
        &self,
        road: RoadId,
        cell: CellCoord,
        uv: [f64; 2],
        mut depth: f64,
    ) -> f64 {
        for j in &self.junctions {
            if j.roads.contains(&road) {
                let blend = j.blend(cell, uv, self.size);
                depth = depth * (1.0 - blend) + j.depth(cell, uv, self.size) * blend;
            }
        }
        depth
    }
}

pub(super) fn build(
    snapshot: &RoadSnapshot,
    roads: &[PlannedRoad],
    requested: &[CellCoord],
    size: f32,
) -> Result<Vec<PlannedJunction>, CompileError> {
    let mut result = vec![];
    let mut sources = snapshot.junctions.iter().collect::<Vec<_>>();
    sources.sort_by_key(|j| j.id);

    for j in sources {
        let bounds = j.bounds(size)?;

        if !requested.iter().any(|c| bounds.contains(*c)) {
            continue;
        }

        let mut members = std::collections::BTreeSet::new();
        let mut arms = 0;

        for r in roads {
            for c in &r.curves {
                for k in [c.source.start.id, c.source.end.id] {
                    if j.knots.contains(&k) {
                        members.insert(r.source.id);
                        arms += 1;
                    }
                }
            }
        }

        if arms < 2 || members.len() < 2 {
            continue;
        }

        let busiest = |styled: bool| {
            (0..roads.len())
                .filter(|&i| {
                    let r = &roads[i].source;
                    members.contains(&r.id) && (!styled || r.profile == j.profile)
                })
                .max_by(|&a, &b| roads[a].source.traffic.total_cmp(&roads[b].source.traffic))
        };
        let hub = busiest(true).or_else(|| busiest(false)).unwrap();
        let road = &roads[hub];
        result.push(PlannedJunction {
            source: j.clone(),
            hub,
            profile: road.profile.clone(),
            wear: road.wear,
            ground_preset: road.ground_preset.clone(),
            roads: members.into_iter().collect(),
            bounds,
        });
    }

    // Independent basins must not overlap: otherwise evaluation order would define terrain.
    for (i, a) in result.iter().enumerate() {
        for b in &result[i + 1..] {
            let p = b
                .source
                .position
                .relative_to(a.source.position.cell, f64::from(size));
            let d = (p[0] - a.source.position.local[0]).hypot(p[1] - a.source.position.local[1]);

            if d < f64::from(a.source.radius + b.source.radius) {
                return Err(road_error(
                    "junction areas overlap; reduce their radii or move them apart",
                ));
            }
        }
    }

    Ok(result)
}

/// Angles between neighbouring arms, degrees, whose corner traffic cuts: sharper corners are
/// turned back on rather than cut; wider ones are all but straight through.
const LEG_ANGLES: [f64; 2] = [60.0, 165.0];

/// One road leaving a junction: its curve, which end of it the junction holds, and the
/// direction it leaves in (unit, world XZ).
struct Arm {
    road: usize,
    curve: usize,
    at_start: bool,
    direction: [f64; 2],
    width: f64,
}

/// Corner legs at the plan's junctions: wherever neighbouring arms meet at a corner traffic cuts
/// (`LEG_ANGLES`), a narrower road of the quieter arm's style, with a share of its traffic,
/// curves from one arm to the other, tangent to both, joining them far enough out
/// (`environment::roads::JUNCTION_LEG_REACH` at most) to leave a grass island against the
/// junction. The wider the corner, the more traffic cuts it. A leg only one-way traffic could
/// take against its road is left out, as is one too tight to draw.
pub(super) fn legs(
    junctions: &[PlannedJunction],
    roads: &[PlannedRoad],
    styles: &[CartTrackProfile],
    size: f32,
    config: &RoadCompileProfile,
    limits: &RoadDetailLimits,
    remaining: &mut usize,
) -> Vec<PlannedRoad> {
    let mut legs = vec![];

    for j in junctions {
        let centre = world_point(&j.source.position, size);
        let mut arms = vec![];

        for (r, road) in roads.iter().enumerate() {
            for (c, curve) in road.curves.iter().enumerate() {
                for (id, at_start) in [(curve.source.start.id, true), (curve.source.end.id, false)]
                {
                    if !j.source.knots.contains(&id) {
                        continue;
                    }

                    let v = if at_start {
                        curve.vertices[0]
                    } else {
                        *curve.vertices.last().unwrap()
                    };
                    let direction = if at_start {
                        [v.normal[1], -v.normal[0]]
                    } else {
                        [-v.normal[1], v.normal[0]]
                    };
                    arms.push(Arm {
                        road: r,
                        curve: c,
                        at_start,
                        direction,
                        width: v.width,
                    });
                }
            }
        }

        let angle = |a: &Arm| a.direction[1].atan2(a.direction[0]);
        arms.sort_by(|a, b| angle(a).total_cmp(&angle(b)));
        let n = arms.len();

        for k in 0..n {
            let (a, b) = (&arms[k], &arms[(k + 1) % n]);

            if n < 2 || (n == 2 && k == 1 && a.road == b.road) {
                continue;
            }

            let corner = (angle(b) - angle(a)).to_degrees().rem_euclid(360.0);

            if !(LEG_ANGLES[0]..=LEG_ANGLES[1]).contains(&corner) || !turnable(roads, a, b) {
                continue;
            }

            let cut = smooth((corner - 60.0) / 35.0) * (1.0 - smooth((corner - 150.0) / 15.0));

            if cut < 0.15 {
                continue;
            }

            // Far enough out that the leg's middle passes a road's width and three metres from
            // the junction (a cubic with handles 35% of the way in passes 0.74 s cos(θ/2) off).
            let half = (corner / 2.0).to_radians();
            let clear = 3.0 + a.width.max(b.width);
            let reach = (clear / (0.74 * half.cos()))
                .clamp(6.0, f64::from(environment::roads::JUNCTION_LEG_REACH));
            let (Some(pa), Some(pb)) = (
                along(&roads[a.road].curves[a.curve], a.at_start, reach, size),
                along(&roads[b.road].curves[b.curve], b.at_start, reach, size),
            ) else {
                continue;
            };
            let traffic = |arm: &Arm| (roads[arm.road].source.traffic, arm.width);
            let quieter = if traffic(a) <= traffic(b) { a } else { b };
            let source = &roads[quieter.road];
            let profile = &source.profile;
            let width = (0.7 * a.width.min(b.width)).max(f64::from(profile.minimum_width()));
            let handle = |p: [f64; 2]| {
                let d = [centre[0] - p[0], centre[1] - p[1]];
                let l = d[0].hypot(d[1]).max(1e-6);
                d.map(|v| v / l * reach * 0.35)
            };
            let key = |part: &str| {
                let mut hash = blake3::Hasher::new();
                hash.update(b"yarra.road.corner-leg.v1");
                hash.update(&j.source.id.0);
                hash.update(&[k as u8, n as u8]);
                hash.update(part.as_bytes());
                let mut id = [0; 16];
                id.copy_from_slice(&hash.finalize().as_bytes()[..16]);
                id
            };
            let knot = |id, p: [f64; 2], incoming: [f64; 2], outgoing: [f64; 2]| {
                Some(RoadKnot {
                    id: RoadKnotId(id),
                    revision: 1,
                    position: RoadPoint::from_relative(CellCoord::ZERO, p, f64::from(size)).ok()?,
                    incoming,
                    outgoing,
                    width: width as f32,
                })
            };
            let road = Road {
                id: RoadId(key("road")),
                revision: 1,
                name: format!("{} corner", source.source.name),
                profile: profile.id,
                seed: j.source.seed ^ u32::from_le_bytes(key("seed")[..4].try_into().unwrap()),
                enabled: true,
                order: source.source.order,
                direction: TravelDirection::Bidirectional,
                travel_modes: source.source.travel_modes.clone(),
                // Lightly enough that legs mostly trample, bare in their busiest stretches.
                traffic: source.source.traffic * (0.25 + 0.4 * cut) as f32,
            };
            let (Some(start), Some(end)) = (
                knot(key("start"), pa, [0.0; 2], handle(pa)),
                knot(key("end"), pb, handle(pb), [0.0; 2]),
            ) else {
                continue;
            };
            let span = RoadSpan {
                id: RoadSpanId(key("span")),
                revision: 1,
                road: road.id,
                start,
                end,
            };
            let Ok(curve) = Curve::build(&span, profile, size, config, remaining) else {
                continue;
            };

            // The style's depths as authored: the arm's profile holds them at its traffic.
            let Some(style) = styles.iter().find(|p| p.id == profile.id) else {
                continue;
            };
            let wear = RoadWear::at(road.traffic);
            legs.push(PlannedRoad {
                baked: limits.broaden(shape(profile, &wear, road.seed)),
                profile: CartTrackProfile {
                    relief: wear.relief(&style.relief),
                    ..profile.clone()
                },
                wear,
                source: road,
                ground_preset: source.ground_preset.clone(),
                curves: vec![curve],
            });
        }
    }

    legs
}

/// Whether traffic can take the corner from arm `a` into arm `b` or back: a one-way road lets
/// traffic only one way through its end at the junction.
fn turnable(roads: &[PlannedRoad], a: &Arm, b: &Arm) -> bool {
    // Whether traffic may come in along `arm` (true) or leave along it (false).
    let may = |arm: &Arm, coming_in: bool| match roads[arm.road].source.direction {
        TravelDirection::Bidirectional => true,
        // Forward runs from each span's start to its end: into the junction at an end.
        TravelDirection::Forward => coming_in != arm.at_start,
        TravelDirection::Reverse => coming_in == arm.at_start,
    };
    (may(a, true) && may(b, false)) || (may(b, true) && may(a, false))
}

/// The world XZ point `distance` metres along `curve` from the end the junction holds, at most
/// four fifths of its length; `None` for a degenerate curve.
fn along(curve: &Curve, at_start: bool, distance: f64, size: f32) -> Option<[f64; 2]> {
    let origin = curve.source.start.position.cell.origin(size);
    let mut points: Vec<_> = curve
        .vertices
        .iter()
        .map(|v| [origin[0] + v.point[0], origin[1] + v.point[1]])
        .collect();

    if !at_start {
        points.reverse();
    }

    let length: f64 = points
        .windows(2)
        .map(|p| (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]))
        .sum();

    if length <= 1e-6 {
        return None;
    }

    let mut left = distance.min(length * 0.8);

    for p in points.windows(2) {
        let step = (p[1][0] - p[0][0]).hypot(p[1][1] - p[0][1]);

        if left <= step {
            let t = left / step.max(1e-9);
            return Some([
                p[0][0] + (p[1][0] - p[0][0]) * t,
                p[0][1] + (p[1][1] - p[0][1]) * t,
            ]);
        }

        left -= step;
    }

    points.last().copied()
}

fn world_point(p: &RoadPoint, size: f32) -> [f64; 2] {
    let o = p.cell.origin(size);
    [o[0] + p.local[0], o[1] + p.local[1]]
}
