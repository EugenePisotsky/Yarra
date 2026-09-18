use std::collections::{BTreeMap, BTreeSet};

use environment::{CoverageSnapshot, VegetationBlend};
use vegetation::{VegetationFieldPageData, VegetationPopulationField};
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
        self.compile_with_influences(requested, source, None)
    }

    pub(crate) fn compile_with_influences(
        &self,
        requested: &[CellCoord],
        source: &CoverageSnapshot,
        roads: Option<&crate::roads::RoadPlan>,
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
        let coverage = Coverage::new(self, source)?;
        // Check the entire halo before any allocation/rasterization, including each target itself.
        for cell in &cells {
            coverage.check_halo(self, *cell)?;
        }
        cells
            .into_iter()
            .map(|cell| {
                Ok(CompiledCell {
                    terrain: None,
                    space: self.definition.space,
                    cell,
                    ground: self.ground(cell, &coverage, roads)?,
                    vegetation: self.vegetation(cell, &coverage, roads)?,
                    input_fingerprint: {
                        let base = coverage.fingerprint(self, cell)?;
                        if let Some(roads) = roads {
                            roads.fingerprint(cell, base)?
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
        let plant_work = vegetation.saturating_mul(self.bindings.len());
        let bytes = ground_work
            .saturating_mul(4)
            .saturating_add(plant_work.saturating_mul(2))
            .saturating_mul(cells);
        if bytes > self.profile.max_output_bytes {
            return Err(CompileError::Budget("raster bytes"));
        }
        // A layer may contain many exclusion/output rules even with few population bindings.
        // Count those rules explicitly so an exclusion-heavy stack cannot evade this estimate.
        let rules = self.layers.iter().fold(0usize, |total, layer| {
            total
                .saturating_add(layer.composition.vegetation.len())
                .saturating_add(layer.composition.exclusions.len())
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

    fn ground(
        &self,
        cell: CellCoord,
        coverage: &Coverage<'_>,
        roads: Option<&crate::roads::RoadPlan>,
    ) -> Result<CompiledGround, CompileError> {
        let resolution = usize::from(self.profile.terrain_resolution);
        let base = self
            .definition
            .surfaces
            .binary_search(&self.definition.base_surface)
            .unwrap();
        let surface_count = self.definition.surfaces.len();
        let mut samples = Vec::with_capacity(resolution * resolution * surface_count);
        let mut used = vec![false; surface_count];
        for z in 0..resolution {
            for x in 0..resolution {
                let uv = [
                    x as f64 / (resolution - 1) as f64,
                    z as f64 / (resolution - 1) as f64,
                ];
                let mut weights = vec![0.0; surface_count];
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
                if let Some(roads) = roads {
                    roads.ground(cell, uv, &mut weights);
                }
                let quantized = quantize(&weights);
                for (index, value) in quantized.into_iter().enumerate() {
                    used[index] |= value != 0;
                    samples.push(value);
                }
            }
        }
        let palette: Vec<_> = used
            .iter()
            .enumerate()
            .filter_map(|(i, used)| used.then_some(i))
            .collect();
        if palette.len() > self.profile.max_surfaces_per_cell {
            return Err(CompileError::SurfaceLimit {
                cell,
                required: palette.len(),
                maximum: self.profile.max_surfaces_per_cell,
            });
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
        Ok(CompiledGround {
            surfaces: palette
                .into_iter()
                .map(|i| self.definition.surfaces[i])
                .collect(),
            weight_pages,
        })
    }

    fn vegetation(
        &self,
        cell: CellCoord,
        coverage: &Coverage<'_>,
        roads: Option<&crate::roads::RoadPlan>,
    ) -> Result<VegetationFieldPageData, CompileError> {
        let resolution = usize::from(self.profile.vegetation_resolution);
        let mut samples = vec![vec![0; resolution * resolution]; self.bindings.len()];
        for z in 0..resolution {
            for x in 0..resolution {
                let uv = [
                    (x as f64 + 0.5) / resolution as f64,
                    (z as f64 + 0.5) / resolution as f64,
                ];
                let mut weights = vec![0.0f64; self.bindings.len()];
                for layer in &self.layers {
                    if !layer.source.enabled {
                        continue;
                    }
                    let a = coverage.sample(cell, layer.source.id, uv)
                        * f64::from(layer.source.opacity);
                    if a == 0.0 {
                        continue;
                    }
                    let mut attenuation = BTreeMap::<_, f64>::new();
                    for output in &layer.composition.exclusions {
                        let strength = attenuation.entry(output.channel).or_default();
                        *strength = strength.max(a * f64::from(output.strength));
                    }
                    for output in &layer.composition.vegetation {
                        if output.blend == VegetationBlend::Replace {
                            let strength = attenuation.entry(output.channel).or_default();
                            *strength = strength.max(a * f64::from(output.strength));
                        }
                    }
                    // All outputs act on the same lower stack. Own contributions are added last.
                    for (weight, binding) in weights.iter_mut().zip(&self.bindings) {
                        *weight *= 1.0 - attenuation.get(&binding.channel).copied().unwrap_or(0.0);
                    }
                    for output in &layer.composition.vegetation {
                        let value = a * f64::from(output.strength) * f64::from(output.density);
                        for &index in &layer.outputs[&output.id] {
                            weights[index] += value;
                        }
                    }
                }
                if let Some(roads) = roads {
                    roads.vegetation(cell, uv, &self.bindings, &mut weights);
                }
                for (index, weight) in weights.into_iter().enumerate() {
                    samples[index][z * resolution + x] =
                        (weight.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            }
        }
        let fields: Vec<_> = samples
            .into_iter()
            .zip(&self.bindings)
            .filter(|(samples, _)| samples.iter().any(|&v| v != 0))
            .map(|(coverage, binding)| VegetationPopulationField {
                population: binding.runtime_population,
                resolution: self.profile.vegetation_resolution,
                coverage,
                flow_direction: [0.0; 2],
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
