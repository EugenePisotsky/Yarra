use super::*;
fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn lerp(a: [f64; 2], b: [f64; 2], t: f64) -> [f64; 2] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
}
impl RoadSpan {
    pub fn control_points(&self, origin: CellCoord, size: f32) -> [[f64; 2]; 4] {
        let a = self.start.position.relative_to(origin, f64::from(size));
        let b = self.end.position.relative_to(origin, f64::from(size));
        [a, add(a, self.start.outgoing), add(b, self.end.incoming), b]
    }
}
/// Conservative cubic-hull bounds, expanded for full corridor, edge noise and feathering.
pub fn influence_bounds(
    span: &RoadSpan,
    profile: &CartTrackProfile,
    size: f32,
) -> Result<RoadCellBounds, ValidationError> {
    profile.validate()?;
    if !size.is_finite()
        || size <= 0.0
        || !span.start.width.is_finite()
        || !span.end.width.is_finite()
    {
        return Err(invalid("cell size or width"));
    }
    let points = span.control_points(span.start.position.cell, size);
    let radius = f64::from(
        span.start.width.max(span.end.width) * 0.5 + profile.edge_variation + profile.edge_softness,
    );
    let min =
        std::array::from_fn(|i| points.iter().map(|p| p[i]).fold(f64::INFINITY, f64::min) - radius);
    let max = std::array::from_fn(|i| {
        points
            .iter()
            .map(|p| p[i])
            .fold(f64::NEG_INFINITY, f64::max)
            + radius
    });
    Ok(RoadCellBounds {
        minimum: RoadPoint::from_relative(span.start.position.cell, min, f64::from(size))?.cell,
        maximum: RoadPoint::from_relative(span.start.position.cell, max, f64::from(size))?.cell,
    })
}
/// Exact de Casteljau split. Caller allocates stable IDs and writes both spans plus any
/// neighboring copies of their shared endpoint knots atomically. Noise never uses these IDs.
pub fn split_span(
    span: &RoadSpan,
    size: f32,
    t: f64,
    knot_id: RoadKnotId,
    right_id: RoadSpanId,
) -> Result<(RoadSpan, RoadSpan), ValidationError> {
    if !t.is_finite()
        || !(0.0001..=0.9999).contains(&t)
        || knot_id == span.start.id
        || knot_id == span.end.id
        || right_id == span.id
    {
        return Err(invalid("split identity or parameter"));
    }
    let [p0, p1, p2, p3] = span.control_points(span.start.position.cell, size);
    let a = lerp(p0, p1, t);
    let b = lerp(p1, p2, t);
    let c = lerp(p2, p3, t);
    let d = lerp(a, b, t);
    let e = lerp(b, c, t);
    let f = lerp(d, e, t);
    let knot = RoadKnot {
        id: knot_id,
        revision: 1,
        position: RoadPoint::from_relative(span.start.position.cell, f, f64::from(size))?,
        incoming: sub(d, f),
        outgoing: sub(e, f),
        width: (f64::from(span.start.width)
            + (f64::from(span.end.width) - f64::from(span.start.width)) * t) as f32,
    };
    let increment = |n: u64| n.checked_add(1).ok_or(invalid("revision overflow"));
    let mut left = span.clone();
    let mut right = span.clone();
    left.revision = increment(left.revision)?;
    left.start.revision = increment(left.start.revision)?;
    left.start.outgoing = sub(a, p0);
    left.end = knot.clone();
    right.id = right_id;
    right.revision = 1;
    right.start = knot;
    right.end.revision = increment(right.end.revision)?;
    right.end.incoming = sub(c, p3);
    Ok((left, right))
}
