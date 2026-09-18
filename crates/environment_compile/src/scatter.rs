//! World-coordinate priority thinning: one jittered candidate per spacing square, retained
//! only when it wins against every closer neighbour. Geometry is independent of paint/density
//! so neighbouring jobs, repainting, and density changes cannot move existing roots.
use crate::{CompileError, CompilePlan, coverage::Coverage, roads::RoadPlan};
use world::{CellCoord, StableObjectId, StaticObjectInstance, TerrainHeightfield};

const MAX_CANDIDATES: usize = 16_384;
const MAX_OBJECTS: usize = 2_048;
const MAX_WORK: usize = 4 * 1024 * 1024;

fn random(key: &[u8; 32], x: i64, z: i64) -> [u8; 32] {
    let mut h = blake3::Hasher::new_keyed(key);
    h.update(&x.to_le_bytes());
    h.update(&z.to_le_bytes());
    *h.finalize().as_bytes()
}
fn unit(h: &[u8; 32], offset: usize) -> f64 {
    (f64::from(u32::from_le_bytes(
        h[offset..offset + 4].try_into().unwrap(),
    )) + 0.5)
        / 4_294_967_296.0
}
fn position(h: &[u8; 32], x: i64, z: i64, step: f64) -> [f64; 2] {
    [
        (x as f64 + unit(h, 0)) * step,
        (z as f64 + unit(h, 4)) * step,
    ]
}

impl CompilePlan {
    pub(crate) fn has_collections(&self) -> bool {
        self.layers
            .iter()
            .any(|l| l.source.enabled && !l.composition.collections.is_empty())
    }
    pub(crate) fn scatter_bytes_per_cell(&self) -> usize {
        let candidates = self
            .layers
            .iter()
            .filter(|l| l.source.enabled)
            .flat_map(|l| &l.composition.collections)
            .fold(0usize, |sum, c| {
                let side = (f64::from(self.definition.cell_size) / f64::from(c.spacing)).ceil()
                    as usize
                    + 1;
                sum.saturating_add(side.saturating_mul(side))
            });
        candidates
            .min(MAX_OBJECTS)
            .saturating_mul(std::mem::size_of::<StaticObjectInstance>())
            .saturating_mul(2)
    }
    pub fn collection_assets(&self) -> Vec<world::AssetId> {
        self.layers
            .iter()
            .flat_map(|l| &l.composition.collections)
            .flat_map(|c| c.assets.iter().map(|a| a.asset))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub(crate) fn scatter(
        &self,
        cell: CellCoord,
        coverage: &Coverage<'_>,
        roads: Option<&RoadPlan>,
        terrain: Option<&TerrainHeightfield>,
    ) -> Result<Vec<StaticObjectInstance>, CompileError> {
        let size = f64::from(self.definition.cell_size);
        let origin = [f64::from(cell.x) * size, f64::from(cell.z) * size];
        let mut result = vec![];
        let mut candidates = 0usize;
        let mut work = 0usize;
        for (li, layer) in self
            .layers
            .iter()
            .enumerate()
            .filter(|(_, l)| l.source.enabled)
        {
            for output in &layer.composition.collections {
                let step = f64::from(output.spacing);
                let min = [
                    (origin[0] / step).floor() as i64,
                    (origin[1] / step).floor() as i64,
                ];
                let max = [
                    ((origin[0] + size) / step).ceil() as i64,
                    ((origin[1] + size) / step).ceil() as i64,
                ];
                candidates = candidates.saturating_add(
                    ((max[0] - min[0]) as usize).saturating_mul((max[1] - min[1]) as usize),
                );
                if candidates > MAX_CANDIDATES {
                    return Err(CompileError::Budget(
                        "scatter candidates per cell; increase spacing",
                    ));
                }
                let exclusion_work = self.layers[li + 1..]
                    .iter()
                    .map(|l| l.composition.exclusions.len())
                    .sum::<usize>();
                work = work.saturating_add(
                    ((max[0] - min[0]) as usize)
                        .saturating_mul((max[1] - min[1]) as usize)
                        .saturating_mul(
                            25 + exclusion_work + roads.map_or(0, RoadPlan::scatter_work),
                        ),
                );
                if work > MAX_WORK.min(self.profile.max_sample_work) {
                    return Err(CompileError::Budget("scatter evaluation work"));
                }
                let mut h = blake3::Hasher::new();
                h.update(b"yarra.scatter.v1");
                h.update(&self.definition.space.0.to_le_bytes());
                h.update(&layer.source.id.0);
                h.update(&output.id.0);
                h.update(&layer.source.seed.to_le_bytes());
                h.update(&output.seed.to_le_bytes());
                let key = *h.finalize().as_bytes();
                for z in min[1]..max[1] {
                    for x in min[0]..max[0] {
                        let hash = random(&key, x, z);
                        let p = position(&hash, x, z, step);
                        let local = [p[0] - origin[0], p[1] - origin[1]];
                        if local.iter().any(|v| *v < 0.0 || *v >= size) {
                            continue;
                        }
                        let uv = [local[0] / size, local[1] / size];
                        let mut amount = coverage.sample(cell, layer.source.id, uv)
                            * f64::from(layer.source.opacity)
                            * f64::from(output.density);
                        // Exclusions suppress only lower layers, matching foliage stack semantics.
                        for upper in self.layers[li + 1..].iter().filter(|l| l.source.enabled) {
                            let strength = upper
                                .composition
                                .exclusions
                                .iter()
                                .filter(|e| e.channel == output.channel)
                                .map(|e| f64::from(e.strength))
                                .fold(0.0, f64::max);
                            amount *= 1.0
                                - coverage.sample(cell, upper.source.id, uv)
                                    * f64::from(upper.source.opacity)
                                    * strength;
                        }
                        if unit(&hash, 8) >= amount {
                            continue;
                        }
                        let wins = (-2..=2).all(|dz| {
                            (-2..=2).all(|dx| {
                                if dx == 0 && dz == 0 {
                                    return true;
                                }
                                let other = random(&key, x + dx, z + dz);
                                // Priority is independent of position; coordinates break ties.
                                if (&other[24..], x + dx, z + dz) >= (&hash[24..], x, z) {
                                    return true;
                                }
                                let q = position(&other, x + dx, z + dz, step);
                                (p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) >= step * step
                            })
                        });
                        if !wins
                            || roads
                                .is_some_and(|r| r.blocks_scatter(cell, uv, output.road_clearance))
                        {
                            continue;
                        }
                        let (height, normal_y) = terrain.map_or((0.0, 1.0), |t| {
                            let s = t.sample(
                                [local[0] as f32, local[1] as f32],
                                self.definition.cell_size,
                            );
                            (s.height, s.normal[1])
                        });
                        if normal_y + 1e-6 < output.max_slope_degrees.to_radians().cos() {
                            continue;
                        }
                        let total: f64 = output.assets.iter().map(|a| f64::from(a.weight)).sum();
                        let mut pick = unit(&hash, 12) * total;
                        let mut asset = output.assets.last().unwrap();
                        for a in &output.assets {
                            if pick < f64::from(a.weight) {
                                asset = a;
                                break;
                            }
                            pick -= f64::from(a.weight);
                        }
                        if result.len() >= MAX_OBJECTS {
                            return Err(CompileError::Budget("generated objects per cell"));
                        }
                        result.push(StaticObjectInstance {
                            generated: true,
                            id: StableObjectId(hash[..16].try_into().unwrap()),
                            asset: asset.asset,
                            translation: [local[0] as f32, height, local[1] as f32],
                            yaw: (unit(&hash, 16) * std::f64::consts::TAU) as f32,
                            scale: asset.scale_min
                                + (asset.scale_max - asset.scale_min) * unit(&hash, 20) as f32,
                        });
                    }
                }
            }
        }
        result.sort_by_key(|o| o.id);
        if result.windows(2).any(|w| w[0].id == w[1].id) {
            return Err(CompileError::Budget("generated object identity collision"));
        }
        Ok(result)
    }
}
