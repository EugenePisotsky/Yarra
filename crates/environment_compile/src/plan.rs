use std::collections::{BTreeMap, BTreeSet};

use environment::{
    ChannelId, EnvironmentDefinition, Layer, LayerId, OutputId, PresetLibrary, ResolvedComposition,
};
use serde::{Deserialize, Serialize};
use vegetation::{VegetationCatalog, VegetationPopulationId};

use crate::{CompileError, CompileProfile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct BindingKey {
    pub layer: LayerId,
    pub output: OutputId,
    pub population: VegetationPopulationId,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopulationBinding {
    pub key: BindingKey,
    pub runtime_population: VegetationPopulationId,
    pub channel: ChannelId,
}

pub(crate) struct PlannedLayer {
    pub source: Layer,
    pub composition: ResolvedComposition,
    pub ground: Vec<(usize, f64)>,
    pub outputs: BTreeMap<OutputId, Vec<usize>>,
}

/// Build once from the complete small layer/catalog definition and share with every cell job.
/// Fields are private so mutation cannot invalidate the plan's validation or fingerprint.
pub struct CompilePlan {
    pub(crate) presets: PresetLibrary,
    pub(crate) definition: EnvironmentDefinition,
    pub(crate) layers: Vec<PlannedLayer>,
    pub(crate) bindings: Vec<PopulationBinding>,
    pub(crate) catalog: VegetationCatalog,
    pub(crate) profile: CompileProfile,
    pub(crate) fingerprint: [u8; 32],
}

impl CompilePlan {
    /// Checks a bounded source gesture and its shared borders without allocating render output.
    pub fn validate_coverage(
        &self,
        cells: &[world::CellCoord],
        source: &environment::CoverageSnapshot,
    ) -> Result<(), CompileError> {
        if cells.len() > self.profile.max_cells_per_batch {
            return Err(CompileError::Budget("requested cells"));
        }
        let coverage = crate::coverage::Coverage::new(self, source)?;
        let mut unique = BTreeSet::new();
        for &cell in cells {
            if !unique.insert(cell) {
                return Err(CompileError::DuplicateCell(cell));
            }
            coverage.check_halo(self, cell)?;
        }
        Ok(())
    }

    pub fn new(
        source: &EnvironmentDefinition,
        plants: &VegetationCatalog,
        presets: &PresetLibrary,
        profile: CompileProfile,
    ) -> Result<Self, CompileError> {
        if !(2..=257).contains(&profile.terrain_resolution)
            || !(1..=256).contains(&profile.vegetation_resolution)
            || !(1..=8).contains(&profile.max_surfaces_per_cell)
            || profile.max_layers == 0
            || profile.max_population_bindings == 0
            || profile.max_fields_per_cell == 0
            || profile.max_cells_per_batch == 0
            || profile.max_mask_samples == 0
            || profile.max_output_bytes == 0
            || profile.max_sample_work == 0
        {
            return Err(CompileError::InvalidProfile);
        }
        if source.layers.len() > profile.max_layers
            || source.surfaces.len() > 64
            || plants.species.len() > 1_024
            || plants.populations.len() > 4_096
            || plants.assemblages.len() > 4_096
        {
            return Err(CompileError::Budget("definition catalog"));
        }
        source.validate(plants, presets)?;
        let mut definition = source.clone();
        definition.surfaces.sort();
        definition.layers.sort_by_key(|l| (l.order, l.id));
        for layer in &mut definition.layers {
            layer
                .overrides
                .sort_by(|a, b| (&a.path, a.value.key()).cmp(&(&b.path, b.value.key())));
        }
        let compositions: BTreeMap<_, _> = definition
            .layers
            .iter()
            .map(|l| {
                presets
                    .resolve(l.preset, &l.overrides)
                    .map(|resolved| (l.id, resolved))
            })
            .collect::<Result<_, _>>()?;
        let assemblages: BTreeMap<_, _> = plants.assemblages.iter().map(|a| (a.id, a)).collect();
        let populations: BTreeMap<_, _> = plants.populations.iter().map(|p| (p.id, p)).collect();
        let mut requested = BTreeMap::new();
        // Disabled layers also participate: enabling another layer never renumbers the plan.
        for layer in &definition.layers {
            for output in &compositions[&layer.id].vegetation {
                for population in &assemblages[&output.assemblage].populations {
                    let key = BindingKey {
                        layer: layer.id,
                        output: output.id,
                        population: *population,
                    };
                    requested.insert(
                        key,
                        (
                            layer.seed,
                            output.seed,
                            output.channel,
                            populations[population],
                        ),
                    );
                    if requested.len() > profile.max_population_bindings {
                        return Err(CompileError::Budget("population bindings"));
                    }
                }
            }
        }
        let mut groups = BTreeMap::new();
        let mut ids = BTreeSet::new();
        let mut bindings = Vec::new();
        let mut catalog = VegetationCatalog {
            species: Vec::new(),
            populations: Vec::new(),
            assemblages: Vec::new(),
        };
        for (key, (layer_seed, use_seed, channel, source_population)) in requested {
            let mut hash = blake3::Hasher::new();
            hash.update(b"yarra.environment.population.v3");
            hash.update(&definition.space.0.to_le_bytes());
            hash.update(&key.layer.0);
            hash.update(&key.output.0);
            hash.update(&key.population.0);
            let identity = hash.finalize();
            let id = VegetationPopulationId(identity.as_bytes()[..16].try_into().unwrap());
            if !ids.insert(id) {
                return Err(CompileError::BindingCollision(id));
            }
            let mut population = source_population.clone();
            population.id = id;
            population.key = format!("environment_{}", identity.to_hex());
            // Seed overrides change the layout, but not the binding's persistent identity.
            hash.update(&layer_seed.to_le_bytes());
            hash.update(&use_seed.to_le_bytes());
            hash.update(&source_population.seed.to_le_bytes());
            population.seed =
                u32::from_le_bytes(hash.finalize().as_bytes()[..4].try_into().unwrap());
            if let Some(group) = population.competition_group {
                let group_key = (key.layer, key.output, group);
                let resolved = if let Some(&existing) = groups.get(&group_key) {
                    existing
                } else {
                    let next = u16::try_from(groups.len() + 1)
                        .map_err(|_| CompileError::Budget("competition groups"))?;
                    groups.insert(group_key, next);
                    next
                };
                population.competition_group = Some(resolved);
            }
            catalog.populations.push(population);
            bindings.push(PopulationBinding {
                key,
                runtime_population: id,
                channel,
            });
        }
        let used_species: BTreeSet<_> = catalog
            .populations
            .iter()
            .flat_map(|p| p.species.iter().map(|s| s.species))
            .collect();
        catalog.species = plants
            .species
            .iter()
            .filter(|s| used_species.contains(&s.id))
            .cloned()
            .collect();
        catalog.species.sort_by_key(|s| s.id);
        catalog.validate()?;
        let layers = definition
            .layers
            .iter()
            .map(|source| {
                let composition = compositions[&source.id].clone();
                let ground = composition
                    .ground
                    .as_ref()
                    .map(|ground| {
                        let total: f64 = ground.surfaces.iter().map(|s| f64::from(s.weight)).sum();
                        ground
                            .surfaces
                            .iter()
                            .map(|s| {
                                (
                                    definition.surfaces.binary_search(&s.surface).unwrap(),
                                    f64::from(s.weight) / total,
                                )
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let mut outputs: BTreeMap<_, Vec<_>> = BTreeMap::new();
                for (index, binding) in bindings
                    .iter()
                    .enumerate()
                    .filter(|(_, b)| b.key.layer == source.id)
                {
                    outputs.entry(binding.key.output).or_default().push(index);
                }
                PlannedLayer {
                    source: source.clone(),
                    composition,
                    ground,
                    outputs,
                }
            })
            .collect();
        let bytes = bincode::serde::encode_to_vec(
            (&definition, &compositions, &catalog, &profile),
            bincode::config::standard(),
        )?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"yarra.environment.compiler.v3");
        hash.update(&bytes);
        Ok(Self {
            presets: presets.clone(),
            definition,
            layers,
            bindings,
            catalog,
            profile,
            fingerprint: *hash.finalize().as_bytes(),
        })
    }

    pub fn catalog(&self) -> &VegetationCatalog {
        &self.catalog
    }
    pub fn bindings(&self) -> &[PopulationBinding] {
        &self.bindings
    }
    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}

/// Merge world-local plans into the generation catalog. Competition groups are isolated between
/// worlds as well as between layers. Use this for both cooking and editor catalog previews.
pub fn merge_runtime_catalogs(plans: &[&CompilePlan]) -> Result<VegetationCatalog, CompileError> {
    let mut plans = plans.to_vec();
    plans.sort_by_key(|plan| plan.definition.space);
    let mut spaces = BTreeSet::new();
    let mut species = BTreeMap::new();
    let mut populations = BTreeMap::new();
    let mut groups = BTreeMap::new();
    for plan in plans {
        if !spaces.insert(plan.definition.space) {
            return Err(CompileError::DuplicateWorld(plan.definition.space));
        }
        for plant in &plan.catalog.species {
            if let Some(old) = species.insert(plant.id, plant.clone())
                && old != *plant
            {
                return Err(CompileError::ConflictingSpecies(plant.id));
            }
        }
        for population in &plan.catalog.populations {
            let mut population = population.clone();
            if let Some(group) = population.competition_group {
                let next = u16::try_from(groups.len() + 1)
                    .map_err(|_| CompileError::Budget("competition groups"))?;
                population.competition_group =
                    Some(*groups.entry((plan.definition.space, group)).or_insert(next));
            }
            let id = population.id;
            if populations.insert(id, population).is_some() {
                return Err(CompileError::BindingCollision(id));
            }
        }
    }
    let result = VegetationCatalog {
        species: species.into_values().collect(),
        populations: populations.into_values().collect(),
        assemblages: Vec::new(),
    };
    result.validate()?;
    Ok(result)
}
