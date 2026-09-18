use super::*;
#[derive(Clone, Copy)]
pub(super) struct Vertex {
    pub point: [f64; 2],
    pub normal: [f64; 2],
    pub width: f64,
}
pub(super) struct Curve {
    pub source: RoadSpan,
    pub bounds: RoadCellBounds,
    pub vertices: Vec<Vertex>,
}
pub(super) fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
pub(super) fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn length(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}
fn mix(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}
fn normalized(a: [f64; 2]) -> Option<[f64; 2]> {
    let n = length(a);
    (n > 1e-9).then(|| a.map(|v| v / n))
}
fn derivative(p: [[f64; 2]; 4], t: f64) -> [f64; 2] {
    std::array::from_fn(|i| {
        3.0 * ((1.0 - t).powi(2) * (p[1][i] - p[0][i])
            + 2.0 * (1.0 - t) * t * (p[2][i] - p[1][i])
            + t * t * (p[3][i] - p[2][i]))
    })
}
fn second_derivative(p: [[f64; 2]; 4], t: f64) -> [f64; 2] {
    std::array::from_fn(|i| {
        6.0 * ((1.0 - t) * (p[2][i] - 2.0 * p[1][i] + p[0][i])
            + t * (p[3][i] - 2.0 * p[2][i] + p[1][i]))
    })
}
fn normal(p: [[f64; 2]; 4], t: f64) -> Result<[f64; 2], CompileError> {
    let tangent = normalized(derivative(p, t))
        .or_else(|| {
            if t == 0.0 {
                normalized(sub(p[2], p[0])).or_else(|| normalized(sub(p[3], p[0])))
            } else if t == 1.0 {
                normalized(sub(p[3], p[1])).or_else(|| normalized(sub(p[3], p[0])))
            } else {
                None
            }
        })
        .ok_or(road_error("degenerate curve or cusp"))?;
    Ok([-tangent[1], tangent[0]])
}
fn project(q: [f64; 2], a: [f64; 2], b: [f64; 2]) -> (f64, [f64; 2]) {
    let ab = sub(b, a);
    let denominator = dot(ab, ab);
    let t = if denominator > 1e-18 {
        (dot(sub(q, a), ab) / denominator).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (t, mix(a, b, t))
}
impl Curve {
    pub(super) fn build(
        source: &RoadSpan,
        profile: &CartTrackProfile,
        size: f32,
        config: &RoadCompileProfile,
        remaining: &mut usize,
    ) -> Result<Self, CompileError> {
        let p = source.control_points(source.start.position.cell, size);
        let vertex = |point, t| -> Result<Vertex, CompileError> {
            Ok(Vertex {
                point,
                normal: normal(p, t)?,
                width: f64::from(source.start.width)
                    + (f64::from(source.end.width) - f64::from(source.start.width)) * t,
            })
        };
        let radius = f64::from(
            source.start.width.max(source.end.width) * 0.5
                + profile.edge_variation
                + profile.edge_softness,
        );
        let mut stack = vec![(p, 0.0, 1.0, 0u8)];
        let mut vertices = vec![vertex(p[0], 0.0)?];
        while let Some((c, a, b, depth)) = stack.pop() {
            for t in [a, (a + b) * 0.5, b] {
                let d = derivative(p, t);
                let speed = length(d);
                if speed > 1e-9
                    && cross(d, second_derivative(p, t)).abs() / speed.powi(3) * radius > 0.85
                {
                    return Err(road_error(
                        "bend too tight for this corridor; widen the curve or narrow the road",
                    ));
                }
                normal(p, t)?;
            }
            let chord = length(sub(c[3], c[0]));
            let polygon =
                length(sub(c[1], c[0])) + length(sub(c[2], c[1])) + length(sub(c[3], c[2]));
            let deviation = length(sub(c[1], project(c[1], c[0], c[3]).1))
                .max(length(sub(c[2], project(c[2], c[0], c[3]).1)));
            // Tangent change also bounds the error of laterally offset wheel tracks.
            let normal_error = radius * length(sub(normal(p, a)?, normal(p, b)?));
            // Width is linear in source parameter t. A geometrically straight cubic can
            // still have nonlinear speed; flattening it to one chord would move the taper.
            let parameter_error = length(sub(c[1], mix(c[0], c[3], 1.0 / 3.0)))
                .max(length(sub(c[2], mix(c[0], c[3], 2.0 / 3.0))));
            let width_error = parameter_error / chord.max(1e-9)
                * f64::from((source.end.width - source.start.width).abs())
                * (b - a);
            if deviation <= config.curve_tolerance
                && polygon - chord <= config.curve_tolerance
                && normal_error <= config.curve_tolerance.sqrt()
                && width_error <= config.curve_tolerance
            {
                if chord < 1e-8 {
                    return Err(road_error("zero-length span or cusp"));
                }
                *remaining = remaining
                    .checked_sub(1)
                    .ok_or(CompileError::Budget("road tessellation segments"))?;
                vertices.push(vertex(c[3], b)?);
            } else {
                if depth >= config.max_subdivision_depth {
                    return Err(CompileError::Budget("road subdivision depth"));
                }
                let d = mix(c[0], c[1], 0.5);
                let e = mix(c[1], c[2], 0.5);
                let f = mix(c[2], c[3], 0.5);
                let g = mix(d, e, 0.5);
                let h = mix(e, f, 0.5);
                let i = mix(g, h, 0.5);
                let m = (a + b) * 0.5;
                stack.push(([i, h, f, c[3]], m, b, depth + 1));
                stack.push(([c[0], d, g, i], a, m, depth + 1));
            }
        }
        Ok(Self {
            source: source.clone(),
            bounds: influence_bounds(source, profile, size)?,
            vertices,
        })
    }
    pub(super) fn closest(&self, cell: CellCoord, uv: [f64; 2], size: f64) -> Closest {
        let p = [
            (i64::from(cell.x) - i64::from(self.source.start.position.cell.x)) as f64 * size
                + uv[0] * size,
            (i64::from(cell.z) - i64::from(self.source.start.position.cell.z)) as f64 * size
                + uv[1] * size,
        ];
        let mut best = Closest {
            distance_squared: f64::INFINITY,
            lateral: 0.0,
            longitudinal: 0.0,
            width: 0.0,
        };
        for pair in self.vertices.windows(2) {
            let [a, b] = [pair[0], pair[1]];
            let (t, q) = project(p, a.point, b.point);
            let delta = sub(p, q);
            let distance_squared = dot(delta, delta);
            if distance_squared < best.distance_squared {
                let n = normalized(mix(a.normal, b.normal, t)).unwrap_or(a.normal);
                let lateral = dot(delta, n);
                best = Closest {
                    distance_squared,
                    lateral,
                    longitudinal: (distance_squared - lateral * lateral).max(0.0).sqrt(),
                    width: a.width + (b.width - a.width) * t,
                };
            }
        }
        best
    }
}
pub(super) struct Closest {
    pub distance_squared: f64,
    pub lateral: f64,
    pub longitudinal: f64,
    pub width: f64,
}
fn intersects(a: [f64; 2], b: [f64; 2], c: [f64; 2], d: [f64; 2]) -> bool {
    if (0..2)
        .any(|i| a[i].min(b[i]) > c[i].max(d[i]) + 1e-8 || c[i].min(d[i]) > a[i].max(b[i]) + 1e-8)
    {
        return false;
    }
    let signs = [
        cross(sub(b, a), sub(c, a)),
        cross(sub(b, a), sub(d, a)),
        cross(sub(d, c), sub(a, c)),
        cross(sub(d, c), sub(b, c)),
    ];
    signs[0] * signs[1] <= 1e-12 && signs[2] * signs[3] <= 1e-12
}
/// Crossing/branch authoring is deliberately not guessed from overlapping curves.
pub(super) fn validate_joins_and_crossings(
    roads: &[PlannedRoad],
    junctions: &[RoadJunction],
    size: f32,
    maximum_work: usize,
) -> Result<(), CompileError> {
    let membership = junctions
        .iter()
        .flat_map(|j| j.knots.iter().map(move |k| (*k, j.id)))
        .collect::<BTreeMap<_, _>>();
    let mut arm_directions: BTreeMap<RoadJunctionId, Vec<[f64; 2]>> = BTreeMap::new();
    let mut lines = vec![];
    let mut joins = BTreeMap::new();
    for road in roads {
        for curve in &road.curves {
            for (id, start, v) in [
                (curve.source.start.id, true, curve.vertices[0]),
                (curve.source.end.id, false, *curve.vertices.last().unwrap()),
            ] {
                if let Some(j) = membership.get(&id) {
                    let tangent = if start {
                        [v.normal[1], -v.normal[0]]
                    } else {
                        [-v.normal[1], v.normal[0]]
                    };
                    let others = arm_directions.entry(*j).or_default();
                    if others.iter().any(|d| dot(*d, tangent) > 0.8660254037844386) {
                        return Err(road_error(
                            "junction arms must diverge by at least 30 degrees; adjust the tangent handles",
                        ));
                    }
                    others.push(tangent);
                }
                if let Some((other_start, normal)) = joins.insert(id, (start, v.normal))
                    && (other_start == start || dot(normal, v.normal) < 0.985)
                {
                    return Err(road_error(
                        "sharp or branching knot requires explicit join handling",
                    ));
                }
            }
            for (index, pair) in curve.vertices.windows(2).enumerate() {
                lines.push((curve, index, pair[0].point, pair[1].point));
            }
        }
    }
    if lines.len().saturating_mul(lines.len()) / 2 > maximum_work {
        return Err(CompileError::Budget("road intersection checks"));
    }
    for i in 0..lines.len() {
        let (a, ai, p0, p1) = lines[i];
        for &(b, bi, q0, q1) in &lines[i + 1..] {
            if a.source.id == b.source.id && ai.abs_diff(bi) <= 1 {
                continue;
            }
            if !a.bounds.intersects(b.bounds) {
                continue;
            }
            let endpoint = |c: &Curve, index: usize| {
                [
                    (index == 0).then_some(c.source.start.id),
                    (index + 2 == c.vertices.len()).then_some(c.source.end.id),
                ]
            };
            let shared = endpoint(a, ai).into_iter().flatten().any(|id| {
                endpoint(b, bi).into_iter().flatten().any(|other| {
                    id == other
                        || membership
                            .get(&id)
                            .is_some_and(|j| membership.get(&other) == Some(j))
                })
            });
            if shared {
                continue;
            }
            let offset = [
                (i64::from(b.source.start.position.cell.x)
                    - i64::from(a.source.start.position.cell.x)) as f64
                    * f64::from(size),
                (i64::from(b.source.start.position.cell.z)
                    - i64::from(a.source.start.position.cell.z)) as f64
                    * f64::from(size),
            ];
            let shift = |q: [f64; 2]| [q[0] + offset[0], q[1] + offset[1]];
            if intersects(p0, p1, shift(q0), shift(q1)) {
                return Err(CompileError::RoadJunctionRequired {
                    first: a.source.id,
                    second: b.source.id,
                });
            }
        }
    }
    Ok(())
}
