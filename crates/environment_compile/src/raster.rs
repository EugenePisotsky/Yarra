use std::collections::BTreeSet;

use environment::CoverageSnapshot;
use vegetation::{GrassField, VegetationFieldPageData};
use world::{CellCoord, TerrainWeightPage};

use crate::{CompileError, CompilePlan, CompiledCell, CompiledGround, coverage::Coverage};

impl CompilePlan {
    /// Uses an explicitly loaded one-cell halo. Returns cells in canonical coordinate order.
    /// A batch is all-or-error: callers never receive partially accepted derived products.
    pub fn compile_cells(
        &self,
        requested: &[CellCoord],
        source: &CoverageSnapshot,
    ) -> Result<Vec<CompiledCell>, CompileError> {
        self.compile_with_influences(requested, source, None, None)
    }

    /// `height_step`: the spacing of the cell's terrain heights, whose carved ruts the wear
    /// field does not sink again (`RoadPlan::clear_carved_ruts`).
    pub(crate) fn compile_with_influences(
        &self,
        requested: &[CellCoord],
        source: &CoverageSnapshot,
        roads: Option<&crate::roads::RoadPlan>,
        height_step: Option<f32>,
    ) -> Result<Vec<CompiledCell>, CompileError> {
        if requested.len() > self.profile.max_cells_per_batch {
            return Err(CompileError::Budget("requested cells"));
        }

        let mut cells = BTreeSet::new();

        for cell in requested {
            if !cells.insert(*cell) {
                return Err(CompileError::DuplicateCell(*cell));
            }
        }

        self.check_raster_budget(cells.len())?;
        let coverage = self.coverage(source)?;

        // Check the entire halo before any allocation/rasterization, including each target itself.
        for cell in &cells {
            coverage.check_halo(*cell)?;
        }

        cells
            .into_iter()
            .map(|cell| {
                let mut page_roads = roads.map(|r| r.terrain_roads(cell)).unwrap_or_default();

                if let (Some(roads), Some(step)) = (roads, height_step) {
                    roads.clear_carved_ruts(cell, step, &mut page_roads);
                }

                let (ground, wear) = self.ground(cell, &coverage, &page_roads)?;
                Ok(CompiledCell {
                    terrain: None,
                    objects: self.scatter(cell, &coverage, roads, None)?,
                    space: self.definition.space,
                    cell,
                    ground,
                    vegetation: self.vegetation(
                        cell,
                        &coverage,
                        roads,
                        &page_roads,
                        wear.as_ref(),
                    )?,
                    roads: page_roads,
                    wear,
                    input_fingerprint: {
                        let base = crate::coverage::fingerprint(&coverage, self, cell)?;
                        if let Some(roads) = roads {
                            let base = roads.fingerprint(cell, base)?;
                            if self.has_collections() {
                                let mut hash = blake3::Hasher::new();
                                hash.update(&base);

                                for (_, _, neighbour) in crate::coverage::halo(cell)? {
                                    hash.update(&roads.fingerprint(neighbour, [0; 32])?);
                                }

                                *hash.finalize().as_bytes()
                            } else {
                                base
                            }
                        } else {
                            base
                        }
                    },
                })
            })
            .collect()
    }

    fn check_raster_budget(&self, cells: usize) -> Result<(), CompileError> {
        let terrain = usize::from(self.profile.terrain_resolution).pow(2);
        let vegetation = usize::from(self.profile.vegetation_resolution).pow(2);
        let ground_work = terrain.saturating_mul(self.definition.surfaces.len());
        let plant_work =
            vegetation.saturating_mul(self.grass.len() + self.definition.surfaces.len());
        let bytes = ground_work
            .saturating_mul(4)
            .saturating_add(plant_work.saturating_mul(2))
            .saturating_add(self.scatter_bytes_per_cell())
            .saturating_mul(cells);

        if bytes > self.profile.max_output_bytes {
            return Err(CompileError::Budget("raster bytes"));
        }

        // A layer may contain many exclusion rules. Count them explicitly so an exclusion-heavy
        // stack cannot evade this estimate.
        let rules = self.layers.iter().fold(0usize, |total, layer| {
            total.saturating_add(layer.composition.exclusions.len())
        });
        let layer_count = self.layers.len().saturating_add(1);
        let work = ground_work
            .saturating_mul(layer_count)
            .saturating_add(plant_work.saturating_mul(layer_count))
            .saturating_add(vegetation.saturating_mul(rules.saturating_add(layer_count)))
            .saturating_add(
                usize::from(self.definition.mask_resolution)
                    .saturating_mul(self.layers.len())
                    .saturating_mul(8),
            )
            .saturating_mul(cells);

        if work > self.profile.max_sample_work {
            return Err(CompileError::Budget("sample work"));
        }

        Ok(())
    }

    /// The cell's natural ground, and where roads wear it, the wear field with what each of its
    /// surfaces wears down to. Worn ground joins the palette wherever wear brings it in, ahead of
    /// heavier natural ground when more surfaces meet than a page holds.
    fn ground(
        &self,
        cell: CellCoord,
        coverage: &Coverage<'_>,
        roads: &crate::roads::TerrainRoads,
    ) -> Result<(CompiledGround, Option<world::TerrainWear>), CompileError> {
        let resolution = usize::from(self.profile.terrain_resolution);
        let base = self.base_surface();
        let surface_count = self.definition.surfaces.len();
        let size = f64::from(self.definition.cell_size);
        let origin = cell.origin(self.definition.cell_size);
        let wearing = !roads.roads.is_empty();
        let mut samples = Vec::with_capacity(resolution * resolution * surface_count);
        let mut used = vec![false; surface_count];
        let mut worn = vec![false; surface_count];

        // How much each surface shows, worn where roads wear the ground, samples on the cell's
        // edge counting EDGE_PRIORITY times: where more surfaces meet than a page holds, those
        // a neighbour shares along the edge are kept, so the pages agree there.
        let mut score = vec![0.0; surface_count];

        for z in 0..resolution {
            for x in 0..resolution {
                let uv = [
                    x as f64 / (resolution - 1) as f64,
                    z as f64 / (resolution - 1) as f64,
                ];
                let local = uv.map(|v| v * size);
                let natural = self.road_packed(
                    self.surface_weights(cell, coverage, uv, base),
                    roads,
                    origin,
                    local,
                );
                let edge = x == 0 || z == 0 || x + 1 == resolution || z + 1 == resolution;
                let priority = if edge { EDGE_PRIORITY } else { 1.0 };

                if wearing {
                    let laid = roads.ground(origin, local);
                    let bare =
                        world::bare_share(laid, [origin[0] + local[0], origin[1] + local[1]]);
                    let full = self.worn_ground(&natural, laid, bare);

                    for (i, (full, natural)) in full.iter().zip(&natural).enumerate() {
                        worn[i] |= full - natural > 0.5 / 255.0;
                        score[i] += full * priority;
                    }
                } else {
                    for (s, w) in score.iter_mut().zip(&natural) {
                        *s += w * priority;
                    }
                }

                let quantized = quantize(&natural);

                for (index, value) in quantized.into_iter().enumerate() {
                    used[index] |= value != 0;
                    samples.push(value);
                }
            }
        }

        let mut palette: Vec<_> = (0..surface_count).filter(|&i| used[i] || worn[i]).collect();

        if palette.len() > self.profile.max_surfaces_per_cell {
            palette = keep_heaviest(
                &mut samples,
                surface_count,
                &palette,
                &score,
                self.profile.max_surfaces_per_cell,
            );
        }

        let mut weight_pages = Vec::new();

        if palette.len() > 1 {
            for slots in palette.chunks(4) {
                let mut rgba = vec![0; resolution * resolution * 4];

                for (pixel, weights) in samples.chunks(surface_count).enumerate() {
                    for (slot, &surface) in slots.iter().enumerate() {
                        rgba[pixel * 4 + slot] = weights[surface];
                    }
                }

                weight_pages.push(TerrainWeightPage {
                    resolution: self.profile.terrain_resolution,
                    rgba,
                });
            }
        }

        let wear = wearing.then(|| world::TerrainWear {
            resolution: world::WEAR_RESOLUTION,
            texels: wear_texels(roads, origin, size),
            worn: palette
                .iter()
                .map(|&i| self.worn_slots(i, &palette))
                .collect(),
            trampled: palette
                .iter()
                .map(|&i| {
                    self.trampled[i]
                        .and_then(|t| palette.iter().position(|&p| p == t))
                        .map_or(world::NO_WORN_SLOT, |slot| slot as u8)
                })
                .collect(),
        });
        Ok((
            CompiledGround {
                surfaces: palette
                    .into_iter()
                    .map(|i| self.definition.surfaces[i])
                    .collect(),
                weight_pages,
            },
            wear,
        ))
    }

    /// The natural ground's `weights` with the surfaces roads wear across their whole width
    /// already worn under their corridors at `local`: the pages hold them so, and the wear field
    /// (`world::TerrainWear::lay`) wears only where traffic does. They go to what they are
    /// trampled to, else to their worn ground.
    fn road_packed(
        &self,
        mut weights: Vec<f64>,
        roads: &crate::roads::TerrainRoads,
        origin: [f64; 2],
        local: [f64; 2],
    ) -> Vec<f64> {
        if roads.roads.is_empty() || !self.whole_road.iter().any(|&w| w) {
            return weights;
        }

        let corridor = roads.corridor(origin, local);

        if corridor <= 0.0 {
            return weights;
        }

        for i in 0..weights.len() {
            if !self.whole_road[i] || weights[i] <= 0.0 {
                continue;
            }

            let given = weights[i] * corridor;
            weights[i] -= given;

            match self.trampled[i] {
                Some(t) => weights[t] += given,
                None => {
                    for &(j, share) in &self.worn[i] {
                        weights[j] += given * share;
                    }
                }
            }
        }

        weights
    }

    /// The natural ground's `weights` (per surface) worn by `wear`, `bare` of it bare and the
    /// rest trampled, as `world::TerrainWear::lay` wears a page's.
    fn worn_ground(&self, weights: &[f64], wear: f64, bare: f64) -> Vec<f64> {
        let mut full = weights.to_vec();

        if wear <= 0.0 {
            return full;
        }

        for (i, &w) in weights.iter().enumerate() {
            if w <= 0.0 {
                continue;
            }

            let given = w * wear;
            let to_trampled = self.trampled[i].map_or(0.0, |_| given * (1.0 - bare));
            let to_bare = if self.worn[i].is_empty() {
                0.0
            } else {
                given - to_trampled
            };
            full[i] -= to_trampled + to_bare;

            if let Some(t) = self.trampled[i] {
                full[t] += to_trampled;
            }

            for &(j, share) in &self.worn[i] {
                full[j] += to_bare * share;
            }
        }

        full
    }

    /// What surface `surface` wears down to as page slots of `palette`, shares of 255; none
    /// where it wears to itself (or its worn ground did not fit the page).
    fn worn_slots(&self, surface: usize, palette: &[usize]) -> [(u8, u8); 2] {
        let mut slots = [(world::NO_WORN_SLOT, 0); 2];
        let kept: Vec<_> = self.worn[surface]
            .iter()
            .filter_map(|&(i, share)| Some((palette.iter().position(|&p| p == i)?, share)))
            .collect();
        let total: f64 = kept.iter().map(|k| k.1).sum();

        if total <= 0.0 {
            return slots;
        }

        let mut given = 0u8;

        for (k, &(slot, share)) in kept.iter().enumerate() {
            let value = if k + 1 == kept.len() {
                255 - given
            } else {
                (share / total * 255.0).round() as u8
            };
            given += value;
            slots[k] = (slot as u8, value);
        }

        slots
    }

    fn base_surface(&self) -> usize {
        self.definition
            .surfaces
            .binary_search(&self.definition.base_surface)
            .unwrap()
    }

    /// Each surface's share of the natural ground at `uv`, before quantization: the base
    /// surface under every enabled layer's ground in order.
    fn surface_weights(
        &self,
        cell: CellCoord,
        coverage: &Coverage<'_>,
        uv: [f64; 2],
        base: usize,
    ) -> Vec<f64> {
        let mut weights = vec![0.0; self.definition.surfaces.len()];
        weights[base] = 1.0;

        for layer in &self.layers {
            if !layer.source.enabled {
                continue;
            }

            let Some(ground) = &layer.composition.ground else {
                continue;
            };
            let a = coverage.sample(cell, layer.source.id, uv)
                * f64::from(layer.source.opacity)
                * f64::from(ground.strength);

            for weight in &mut weights {
                *weight *= 1.0 - a;
            }

            for &(index, weight) in &layer.ground {
                weights[index] += a * weight;
            }
        }

        weights
    }

    /// Each grass type's coverage: what the surfaces that show (worn where roads wear them)
    /// carry, less what exclusions and road wear clear (of plants; stones and twigs stay). Types
    /// growing by wear (`DetailType::wear`) take what the natural ground carries by how worn it
    /// is, over each sample's area of the wear field `wear`.
    fn vegetation(
        &self,
        cell: CellCoord,
        coverage: &Coverage<'_>,
        roads: Option<&crate::roads::RoadPlan>,
        page_roads: &crate::roads::TerrainRoads,
        wear: Option<&world::TerrainWear>,
    ) -> Result<VegetationFieldPageData, CompileError> {
        let resolution = usize::from(self.profile.vegetation_resolution);
        let base = self.base_surface();
        let size = f64::from(self.definition.cell_size);
        let origin = cell.origin(self.definition.cell_size);
        let mut samples = vec![vec![0; resolution * resolution]; self.grass.len()];

        for z in 0..resolution {
            for x in 0..resolution {
                let uv = [
                    (x as f64 + 0.5) / resolution as f64,
                    (z as f64 + 0.5) / resolution as f64,
                ];
                let local = uv.map(|v| v * size);
                let natural = self.road_packed(
                    self.surface_weights(cell, coverage, uv, base),
                    page_roads,
                    origin,
                    local,
                );
                let unworn = wear.map(|_| shown(natural.clone()));
                let surfaces = if page_roads.roads.is_empty() {
                    shown(natural)
                } else {
                    let wear = page_roads.ground(origin, local);
                    let bare =
                        world::bare_share(wear, [origin[0] + local[0], origin[1] + local[1]]);
                    shown(self.worn_ground(&natural, wear, bare))
                };
                let mut kept = 1.0;

                for layer in &self.layers {
                    if layer.source.enabled && layer.grass_exclusion > 0.0 {
                        kept *= 1.0
                            - coverage.sample(cell, layer.source.id, uv)
                                * f64::from(layer.source.opacity)
                                * layer.grass_exclusion;
                    }
                }

                if let Some(roads) = roads {
                    kept *= 1.0 - roads.grass_suppression(cell, uv);
                }

                for (field, grass) in samples.iter_mut().zip(&self.grass) {
                    let (ground, share) = match (grass.wear, wear, &unworn) {
                        (Some(band), Some(wear), Some(unworn)) => {
                            (unworn, band_share(wear, band, [x, z], resolution))
                        }
                        (Some(_), ..) => continue,
                        _ => (&surfaces, if grass.on_paths { 1.0 } else { kept }),
                    };
                    let amount: f64 = ground.iter().zip(&grass.amounts).map(|(w, a)| w * a).sum();
                    field[z * resolution + x] =
                        ((amount * share).clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }

        let fields: Vec<_> = samples
            .into_iter()
            .zip(&self.grass)
            .filter(|(samples, _)| samples.iter().any(|&v| v != 0))
            .map(|(coverage, grass)| GrassField {
                grass: grass.grass,
                resolution: self.profile.vegetation_resolution,
                coverage,
            })
            .collect();

        if fields.len() > self.profile.max_fields_per_cell {
            return Err(CompileError::Budget("fields per cell"));
        }

        let page = VegetationFieldPageData { fields };
        page.validate(&self.catalog)?;
        Ok(page)
    }
}

/// The mean share a type with the wear band `band` grows over sample `at` of a `resolution`²
/// grid on the page whose wear field is `wear`.
fn band_share(wear: &world::TerrainWear, band: [f32; 3], at: [usize; 2], resolution: usize) -> f64 {
    let n = usize::from(wear.resolution);
    let [x, z] = at.map(|v| {
        let start = v * n / resolution;
        start..((v + 1) * n / resolution).max(start + 1)
    });
    let count = x.len() * z.len();
    let total: f64 = z
        .flat_map(|j| x.clone().map(move |i| j * n + i))
        .map(|i| vegetation::wear_share(band, f64::from(wear.texels[i][0]) / 255.0))
        .sum();
    total / count as f64
}

/// The wear field's texels over a cell whose origin is `origin` and side `size`
/// (`world::TerrainWear`), at texel centres.
fn wear_texels(roads: &crate::roads::TerrainRoads, origin: [f64; 2], size: f64) -> Vec<[u8; 4]> {
    let n = usize::from(world::WEAR_RESOLUTION);
    (0..n * n)
        .map(|i| {
            let local = [(i % n) as f64 + 0.5, (i / n) as f64 + 0.5].map(|v| v / n as f64 * size);
            let s = roads.wear(origin, local);
            world::TerrainWear::encode(s.wear, s.rut, s.direction)
        })
        .collect()
}

/// How much more a weight sample on a cell's edge counts than one inside it when a cell keeps
/// its surfaces: the edge's samples then weigh about as much as the inside's.
const EDGE_PRIORITY: f64 = 16.0;

/// Where more surfaces meet in a cell than its weight pages hold, keeps the `maximum` with the
/// highest `score` (how much each shows, worn ground included, its edge counting most) and shares
/// each sample among those. Surfaces a neighbour shares along the edge are kept, so pages agree
/// there; the dropped ones are slivers inside the cell (shading shows at most three at a point).
/// Returns the kept palette, in order.
fn keep_heaviest(
    samples: &mut [u8],
    surface_count: usize,
    palette: &[usize],
    score: &[f64],
    maximum: usize,
) -> Vec<usize> {
    let mut kept = palette.to_vec();

    // Highest score first; equal scores keep the lower surface index.
    kept.sort_by(|&a, &b| score[b].total_cmp(&score[a]).then(a.cmp(&b)));
    kept.truncate(maximum);
    let heaviest = kept[0];
    kept.sort_unstable();

    for weights in samples.chunks_mut(surface_count) {
        let mut shares = vec![0.0; surface_count];

        for &i in &kept {
            shares[i] = f64::from(weights[i]);
        }

        if shares.iter().all(|&w| w == 0.0) {
            shares[heaviest] = 1.0;
        }

        weights.copy_from_slice(&quantize(&shares));
    }

    kept
}

/// Largest-remainder quantization preserves sum=255, with stable surface-ID tie breaking.
fn quantize(weights: &[f64]) -> Vec<u8> {
    let sum: f64 = weights.iter().sum();
    let exact: Vec<_> = weights.iter().map(|v| v / sum * 255.0).collect();
    let mut result: Vec<_> = exact.iter().map(|v| v.floor() as u8).collect();
    let remaining = 255 - result.iter().map(|&v| usize::from(v)).sum::<usize>();
    let mut order: Vec<_> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| {
        (exact[b] - f64::from(result[b]))
            .total_cmp(&(exact[a] - f64::from(result[a])))
            .then(a.cmp(&b))
    });

    for &index in order.iter().take(remaining) {
        result[index] += 1;
    }

    result
}

/// Each surface's share of the ground that shows. The ground draws a surface only where its
/// weight comes close to the largest (within the height blend's margin), so a faint blend, such
/// as coastal grass fading out over dune sand, shows as sand and grows no grass there; even
/// blends keep their shares.
fn shown(mut weights: Vec<f64>) -> Vec<f64> {
    let step = |w: f64| {
        let t = ((w - 0.2) / 0.4).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let total: f64 = weights.iter().map(|&w| step(w)).sum();

    if total <= 0.0 {
        return weights;
    }

    for w in &mut weights {
        *w = step(*w) / total;
    }

    weights
}

#[cfg(test)]
mod tests {
    use super::shown;

    #[test]
    fn faint_blends_grow_nothing_and_even_blends_keep_their_shares() {
        let faint = shown(vec![0.85, 0.15]);
        assert_eq!(faint, vec![1.0, 0.0]);
        let even = shown(vec![0.5, 0.5]);
        assert!((even[0] - 0.5).abs() < 1e-12 && (even[1] - 0.5).abs() < 1e-12);
        let leaning = shown(vec![0.65, 0.35]);
        assert!(leaning[1] > 0.0 && leaning[1] < 0.35);
        assert!((leaning.iter().sum::<f64>() - 1.0).abs() < 1e-12);
    }
}
