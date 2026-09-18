use super::*;

pub(super) struct PlannedJunction {
    pub source: RoadJunction,
    pub profile: CartTrackProfile,
    pub ground: Vec<(usize, f64)>,
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
        // A common center, followed by a broad transition into individual wheel tracks.
        smooth((1.0 - distance / f64::from(self.source.radius)) / 0.65)
    }
    pub fn wear(&self, cell: CellCoord, uv: [f64; 2], size: f32) -> f64 {
        let world = [
            (f64::from(cell.x) + uv[0]) * f64::from(size),
            (f64::from(cell.z) + uv[1]) * f64::from(size),
        ];
        let patches = smooth(
            (noise::patch(
                world,
                self.profile.breakup_patch_size,
                self.source.seed ^ 0x815,
            ) - 0.40)
                / 0.30,
        );
        1.0 - f64::from(self.profile.breakup) * patches
    }
    pub fn depth(&self, cell: CellCoord, uv: [f64; 2], size: f32) -> f64 {
        let r = &self.profile.relief;
        let world = [
            (f64::from(cell.x) + uv[0]) * f64::from(size),
            (f64::from(cell.z) + uv[1]) * f64::from(size),
        ];
        let broad =
            noise::patch(world, r.road_variation_length, self.source.seed ^ 0x906a) * 2.0 - 1.0;
        let rut =
            noise::patch(world, r.track_variation_length, self.source.seed ^ 0x418c) * 2.0 - 1.0;
        // Ruts merge into a shallow worn basin, never accumulate into intersecting trenches.
        f64::from(r.road_depth) * (1.0 + f64::from(r.road_variation) * broad)
            + 0.5 * f64::from(r.track_depth) * (1.0 + f64::from(r.track_variation) * rut)
    }
}
impl RoadPlan {
    pub(super) fn junction_blend(&self, road: RoadId, cell: CellCoord, uv: [f64; 2]) -> f64 {
        self.junctions
            .iter()
            .filter(|j| j.roads.contains(&road))
            .map(|j| j.blend(cell, uv, self.size))
            .fold(0.0, f64::max)
    }
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
        let road = roads
            .iter()
            .find(|r| members.contains(&r.source.id))
            .unwrap();
        result.push(PlannedJunction {
            source: j.clone(),
            profile: road.profile.clone(),
            ground: road.ground.clone(),
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
