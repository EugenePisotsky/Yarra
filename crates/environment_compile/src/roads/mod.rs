//! Road influences evaluated after the painted environment stack. No grass population is added.
mod geometry;
mod junctions;
mod relief;
use super::*;
use environment::roads::*;
use environment::{GRASS_CHANNEL, PresetKind};
use geometry::Curve;
pub use relief::{TerrainSource, TerrainSourceCell};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoadCompileProfile {
    pub curve_tolerance: f64,
    pub max_subdivision_depth: u8,
    pub max_segments: usize,
    pub max_sample_work: usize,
    pub max_intersection_work: usize,
}
impl Default for RoadCompileProfile {
    fn default() -> Self {
        Self {
            curve_tolerance: 0.005,
            max_subdivision_depth: 16,
            max_segments: 4096,
            max_sample_work: 64 * 1024 * 1024,
            max_intersection_work: 8 * 1024 * 1024,
        }
    }
}
impl RoadCompileProfile {
    /// Shared by the compiler and authoring controls. Round upwards so an f32 value
    /// at the advertised minimum also satisfies the grid's f64 sampling constraint.
    pub fn detail_limits(
        &self,
        cell_size: f32,
        terrain_resolution: u16,
        vegetation_resolution: u16,
    ) -> Result<RoadDetailLimits, CompileError> {
        if !cell_size.is_finite()
            || cell_size <= 0.0
            || terrain_resolution < 2
            || vegetation_resolution == 0
            || !self.curve_tolerance.is_finite()
            || self.curve_tolerance <= 0.0
        {
            return Err(CompileError::InvalidProfile);
        }
        let step = f64::from(cell_size)
            / (f64::from(terrain_resolution) - 1.0).min(f64::from(vegetation_resolution));
        let ceil = |v: f64| {
            let f = v as f32;
            if f64::from(f) < v { f.next_up() } else { f }
        };
        Ok(RoadDetailLimits {
            track_core: ceil(2.0 * step),
            edge_softness: ceil((step * 0.5).max(self.curve_tolerance / 0.1)),
            patch_size: ceil(4.0 * step),
        })
    }
}

/// The finest road detail a cell's output grids resolve: narrower tracks, softer edges and
/// smaller patches would alias between samples or vanish. The compiler broadens a style to these
/// for weights, grass and heights; near terrain detail draws the authored style per pixel.
#[derive(Debug, Clone, Copy)]
pub struct RoadDetailLimits {
    /// Track width remaining after variation and wander consume both edges.
    pub track_core: f32,
    pub edge_softness: f32,
    pub patch_size: f32,
}
impl RoadDetailLimits {
    /// `shape` as these grids can draw it.
    pub fn broaden(&self, mut shape: world::RoadShape) -> world::RoadShape {
        shape.edge_softness = shape.edge_softness.max(self.edge_softness);
        shape.track_width = shape
            .track_width
            .max(self.track_core + 2.0 * shape.edge_variation + shape.track_wander);
        shape.edge_patch_size = shape.edge_patch_size.max(self.patch_size);
        shape.breakup_patch_size = shape.breakup_patch_size.max(self.patch_size);
        shape.track_wander_length = shape.track_wander_length.max(self.patch_size);
        shape
    }
}
/// Independent influence values; a road can expose ground while retaining its grassy center.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct RoadInfluence {
    pub ground: f64,
    pub suppression: f64,
}
struct PlannedRoad {
    source: Road,
    profile: CartTrackProfile,
    /// The style's shape as the cell's grids draw it.
    baked: world::RoadShape,
    ground: Vec<(usize, f64)>,
    ground_preset: environment::Preset,
    curves: Vec<Curve>,
}
pub(crate) struct RoadPlan {
    roads: Vec<PlannedRoad>,
    junctions: Vec<junctions::PlannedJunction>,
    cells: BTreeMap<CellCoord, Vec<(usize, Vec<usize>)>>,
    size: f32,
    profile: RoadCompileProfile,
}
fn road_error(message: &'static str) -> CompileError {
    CompileError::Road(message.into())
}
impl RoadPlan {
    pub(crate) fn new(
        plan: &CompilePlan,
        requested: &[CellCoord],
        snapshot: &RoadSnapshot,
        profile: RoadCompileProfile,
    ) -> Result<Self, CompileError> {
        snapshot.validate()?;
        if snapshot.space != plan.definition.space
            || snapshot.cell_size != plan.definition.cell_size
        {
            return Err(road_error("road snapshot belongs to another world/grid"));
        }
        if !profile.curve_tolerance.is_finite()
            || !(0.0001..=0.05).contains(&profile.curve_tolerance)
            || !(1..=20).contains(&profile.max_subdivision_depth)
            || !(1..=16384).contains(&profile.max_segments)
            || profile.max_sample_work == 0
            || profile.max_intersection_work == 0
        {
            return Err(CompileError::InvalidProfile);
        }
        if requested.len() > plan.profile.max_cells_per_batch {
            return Err(CompileError::Budget("requested cells"));
        }
        for &cell in requested {
            if !snapshot.loaded_bounds.contains(cell) {
                return Err(CompileError::RoadWindow(cell));
            }
        }
        let mut remaining = profile.max_segments;
        let mut roads = vec![];
        let mut sources = snapshot.roads.iter().collect::<Vec<_>>();
        sources.sort_by_key(|r| (r.order, r.id));
        for source in sources {
            let cart = snapshot
                .profiles
                .iter()
                .find(|p| p.id == source.profile)
                .unwrap();
            let Some(preset) = plan.presets.get(cart.ground) else {
                return Err(road_error("missing road ground preset"));
            };
            let PresetKind::Ground(ground) = &preset.kind else {
                return Err(road_error("road surface must reference a Ground preset"));
            };
            let total = ground
                .surfaces
                .iter()
                .map(|s| f64::from(s.weight))
                .sum::<f64>();
            let surface_weights = ground
                .surfaces
                .iter()
                .map(|s| {
                    plan.definition
                        .surfaces
                        .binary_search(&s.surface)
                        .map(|index| (index, f64::from(s.weight) / total))
                        .map_err(|_| {
                            CompileError::Source(environment::ValidationError::MissingSurface(
                                s.surface,
                            ))
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            if !source.enabled {
                continue;
            }
            let mut spans = vec![];
            for span in snapshot.spans.iter().filter(|s| s.road == source.id) {
                let bounds = influence_bounds(span, cart, snapshot.cell_size)?;
                let junction = snapshot.junctions.iter().any(|j| {
                    (j.knots.contains(&span.start.id) || j.knots.contains(&span.end.id))
                        && j.bounds(snapshot.cell_size)
                            .is_ok_and(|b| requested.iter().any(|c| b.contains(*c)))
                });
                if junction || requested.iter().any(|c| bounds.contains(*c)) {
                    spans.push(span);
                }
            }
            if spans.is_empty() {
                continue;
            }
            spans.sort_by_key(|s| s.id);
            let baked = profile
                .detail_limits(
                    snapshot.cell_size,
                    plan.profile.terrain_resolution,
                    plan.profile.vegetation_resolution,
                )?
                .broaden(shape(cart, source.seed));
            let mut curves = vec![];
            for span in spans {
                curves.push(Curve::build(
                    span,
                    cart,
                    snapshot.cell_size,
                    &profile,
                    &mut remaining,
                )?);
            }
            if !curves.is_empty() {
                roads.push(PlannedRoad {
                    source: source.clone(),
                    profile: cart.clone(),
                    baked,
                    ground: surface_weights,
                    ground_preset: preset.clone(),
                    curves,
                });
            }
        }
        let junctions = junctions::build(snapshot, &roads, requested, snapshot.cell_size)?;
        geometry::validate_joins_and_crossings(
            &roads,
            &snapshot.junctions,
            snapshot.cell_size,
            profile.max_intersection_work,
        )?;
        let mut cells = BTreeMap::new();
        let mut work = 0usize;
        let samples = usize::from(plan.profile.terrain_resolution).pow(2)
            + usize::from(plan.profile.vegetation_resolution).pow(2);
        for &cell in requested {
            let mut entries = vec![];
            for (r, road) in roads.iter().enumerate() {
                let curves = road
                    .curves
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.bounds.contains(cell))
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                if curves.is_empty() {
                    continue;
                }
                let segments = curves
                    .iter()
                    .map(|i| road.curves[*i].vertices.len() - 1)
                    .sum::<usize>();
                work = work.saturating_add(samples.saturating_mul(segments.saturating_add(16)));
                entries.push((r, curves));
            }
            cells.insert(cell, entries);
        }
        work = work.saturating_add(
            requested
                .len()
                .saturating_mul(samples)
                .saturating_mul(junctions.len().saturating_mul(16)),
        );
        if work > profile.max_sample_work {
            return Err(CompileError::Budget("road sample work"));
        }
        Ok(Self {
            roads,
            junctions,
            cells,
            size: snapshot.cell_size,
            profile,
        })
    }
    pub(crate) fn scatter_work(&self) -> usize {
        self.roads
            .iter()
            .flat_map(|r| &r.curves)
            .map(|c| c.vertices.len())
            .sum::<usize>()
            + self.junctions.len()
    }
    pub(crate) fn blocks_scatter(&self, cell: CellCoord, uv: [f64; 2], clearance: f32) -> bool {
        self.roads.iter().any(|road| {
            road.curves.iter().any(|c| {
                let closest = c.closest(cell, uv, f64::from(self.size));
                closest.distance_squared.sqrt() < closest.width * 0.5 + f64::from(clearance)
            })
        }) || self
            .junctions
            .iter()
            .any(|j| j.blocks_scatter(cell, uv, self.size, clearance))
    }

    fn influence(
        &self,
        road: &PlannedRoad,
        curves: &[usize],
        cell: CellCoord,
        uv: [f64; 2],
    ) -> RoadInfluence {
        let closest = curves
            .iter()
            .map(|i| road.curves[*i].closest(cell, uv, f64::from(self.size)))
            .min_by(|a, b| a.distance_squared.total_cmp(&b.distance_squared))
            .unwrap();
        let p = &road.profile;
        let size = f64::from(self.size);
        let world = [
            (f64::from(cell.x) + uv[0]) * size,
            (f64::from(cell.z) + uv[1]) * size,
        ];
        let z = road.baked.zones(
            &world::RoadOffset {
                distance: closest.distance_squared.sqrt(),
                lateral: closest.lateral,
                longitudinal: closest.longitudinal,
                width: closest.width,
                point: closest.point,
            },
            world,
        );
        RoadInfluence {
            ground: z
                .tracks
                .max(z.center * f64::from(p.center_ground))
                .max(z.shoulder * f64::from(p.shoulder_ground))
                * z.wear,
            suppression: (z.tracks * f64::from(1.0 - p.track_retention))
                .max(z.center * f64::from(1.0 - p.center_retention))
                .max(z.shoulder * f64::from(1.0 - p.shoulder_retention))
                * z.wear,
        }
    }
    pub(crate) fn ground(&self, cell: CellCoord, uv: [f64; 2], weights: &mut [f64]) {
        for (index, curves) in &self.cells[&cell] {
            let road = &self.roads[*index];
            let PresetKind::Ground(ground) = &road.ground_preset.kind else {
                unreachable!()
            };
            // Fade each ordinary road continuously; changing blend support must not
            // switch abruptly between sequential material layers and a max union.
            let a = self.influence(road, curves, cell, uv).ground
                * (1.0 - self.junction_blend(road.source.id, cell, uv))
                * f64::from(ground.strength);
            for value in weights.iter_mut() {
                *value *= 1.0 - a;
            }
            for &(index, value) in &road.ground {
                weights[index] += a * value;
            }
        }
        for j in &self.junctions {
            let PresetKind::Ground(ground) = &j.ground_preset.kind else {
                unreachable!()
            };
            let blend = j.blend(cell, uv, self.size);
            if blend == 0.0 {
                continue;
            }
            let a = j.wear(cell, uv, self.size) * blend * f64::from(ground.strength);
            for value in weights.iter_mut() {
                *value *= 1.0 - a;
            }
            for &(index, value) in &j.ground {
                weights[index] += a * value;
            }
        }
    }
    /// How much road and junction wear clears grass at `uv`: the strongest in the grass channel.
    pub(crate) fn grass_suppression(&self, cell: CellCoord, uv: [f64; 2]) -> f64 {
        let mut suppression = 0.0_f64;
        for (index, curves) in &self.cells[&cell] {
            let road = &self.roads[*index];
            if road.profile.vegetation_channel != GRASS_CHANNEL {
                continue;
            }
            let mut value = self.influence(road, curves, cell, uv).suppression;
            for j in &self.junctions {
                if j.roads.contains(&road.source.id) {
                    let blend = j.blend(cell, uv, self.size);
                    value = value * (1.0 - blend)
                        + j.wear(cell, uv, self.size)
                            * f64::from(1.0 - j.profile.track_retention)
                            * blend;
                }
            }
            suppression = suppression.max(value);
        }
        for j in &self.junctions {
            if j.profile.vegetation_channel == GRASS_CHANNEL {
                let value = j.blend(cell, uv, self.size)
                    * j.wear(cell, uv, self.size)
                    * f64::from(1.0 - j.profile.track_retention);
                suppression = suppression.max(value);
            }
        }
        suppression
    }
    pub(crate) fn fingerprint(
        &self,
        cell: CellCoord,
        base: [u8; 32],
    ) -> Result<[u8; 32], CompileError> {
        if self.cells[&cell].is_empty() && !self.junctions.iter().any(|j| j.bounds.contains(cell)) {
            return Ok(base);
        }
        let dependencies = self.cells[&cell]
            .iter()
            .map(|(index, curves)| {
                let r = &self.roads[*index];
                (
                    &r.source,
                    &r.profile,
                    &r.ground_preset,
                    curves
                        .iter()
                        .map(|i| &r.curves[*i].source)
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let bytes = bincode::serde::encode_to_vec(
            (
                &self.profile,
                dependencies,
                self.junctions
                    .iter()
                    .filter(|j| j.bounds.contains(cell))
                    .map(|j| (&j.source, &j.profile, &j.ground_preset))
                    .collect::<Vec<_>>(),
            ),
            bincode::config::standard(),
        )?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"yarra.environment.road.v2");
        hash.update(&base);
        hash.update(&bytes);
        Ok(*hash.finalize().as_bytes())
    }
}
impl RoadPlan {
    /// The roads reaching `cell` as its terrain page carries them: each road's drawing and every
    /// tessellated centre-line segment whose corridor touches the page, in page-local metres.
    /// Near terrain detail resolves wheel tracks from these that the page's weights cannot.
    /// Ruts that heights `height_step` apart carry are not drawn again.
    pub(crate) fn clear_carved_ruts(
        &self,
        cell: CellCoord,
        height_step: f32,
        roads: &mut world::TerrainRoads,
    ) {
        for ((index, _), road) in self.cells[&cell].iter().zip(&mut roads.roads) {
            if self.roads[*index].profile.carves_ruts(height_step) {
                road.track_depth = 0.0;
            }
        }
    }
    pub(crate) fn terrain_roads(&self, cell: CellCoord) -> world::TerrainRoads {
        let mut result = world::TerrainRoads::default();
        let Some(entries) = self.cells.get(&cell) else {
            return result;
        };
        let size = f64::from(self.size);
        for (index, curves) in entries {
            let road = &self.roads[*index];
            let p = &road.profile;
            let PresetKind::Ground(ground) = &road.ground_preset.kind else {
                unreachable!()
            };
            let local = result.roads.len() as u16;
            result.roads.push(world::TerrainRoad {
                shape: shape(p, road.source.seed),
                center_ground: p.center_ground,
                shoulder_ground: p.shoulder_ground,
                track_depth: p.relief.track_depth,
                rut_roundness: p.relief.rut_roundness,
                strength: ground.strength,
                surfaces: {
                    let total: f32 = ground.surfaces.iter().map(|s| s.weight).sum();
                    ground
                        .surfaces
                        .iter()
                        .map(|s| (s.surface, s.weight / total))
                        .collect()
                },
            });
            for &curve in curves {
                let curve = &road.curves[curve];
                let origin = curve.source.start.position.cell;
                let offset = [
                    (f64::from(origin.x) - f64::from(cell.x)) * size,
                    (f64::from(origin.z) - f64::from(cell.z)) * size,
                ];
                let reach = f64::from(p.edge_variation + p.edge_softness);
                for pair in curve.vertices.windows(2) {
                    let (a, b) = (&pair[0], &pair[1]);
                    let pa = [a.point[0] + offset[0], a.point[1] + offset[1]];
                    let pb = [b.point[0] + offset[0], b.point[1] + offset[1]];
                    let pad = a.width.max(b.width) * 0.5 + reach;
                    let outside =
                        (0..2).any(|i| pa[i].max(pb[i]) < -pad || pa[i].min(pb[i]) > size + pad);
                    if outside {
                        continue;
                    }
                    result.segments.push(world::TerrainRoadSegment {
                        a: pa.map(|v| v as f32),
                        b: pb.map(|v| v as f32),
                        normal_a: a.normal.map(|v| v as f32),
                        normal_b: b.normal.map(|v| v as f32),
                        width: [a.width as f32, b.width as f32],
                        road: local,
                    });
                }
            }
        }
        result
    }
}

fn shape(p: &CartTrackProfile, seed: u32) -> world::RoadShape {
    world::RoadShape {
        seed,
        track_spacing: p.track_spacing,
        track_width: p.track_width,
        edge_softness: p.edge_softness,
        edge_variation: p.edge_variation,
        edge_patch_size: p.edge_patch_size,
        track_wander: p.track_wander,
        track_wander_length: p.track_wander_length,
        breakup: p.breakup,
        breakup_patch_size: p.breakup_patch_size,
    }
}
fn smooth(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
impl CompilePlan {
    pub fn compile_cells_with_roads(
        &self,
        requested: &[CellCoord],
        source: &environment::CoverageSnapshot,
        roads: &RoadSnapshot,
        profile: RoadCompileProfile,
    ) -> Result<Vec<CompiledCell>, CompileError> {
        let queried = if self.has_collections() {
            let mut halo = std::collections::BTreeSet::new();
            for &cell in requested {
                halo.extend(crate::coverage::halo(cell)?.into_iter().map(|(_, _, c)| c));
            }
            halo.into_iter().collect::<Vec<_>>()
        } else {
            requested.to_vec()
        };
        let road_plan = RoadPlan::new(self, &queried, roads, profile)?;
        self.compile_with_influences(requested, source, Some(&road_plan))
    }
}
