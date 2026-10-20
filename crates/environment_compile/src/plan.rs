use std::collections::BTreeMap;

use environment::{
    EnvironmentDefinition, GRASS_CHANNEL, Layer, LayerId, PresetLibrary, ResolvedComposition,
};

use vegetation::{GrassTypeId, VegetationCatalog};

use crate::{CompileError, CompileProfile};

pub(crate) struct PlannedLayer {
    pub source: Layer,
    pub composition: ResolvedComposition,
    pub ground: Vec<(usize, f64)>,
    /// The strongest of the layer's grass exclusions.
    pub grass_exclusion: f64,
}

/// A grass type that grows on some of the world's surfaces: its share of density on each, in
/// the definition's surface order, whether it lies on worn ground too (`DetailType::on_paths`)
/// and the wear it grows at, if only on worn ground (`DetailType::wear`).
pub(crate) struct GrassBinding {
    pub grass: GrassTypeId,
    pub amounts: Vec<f64>,
    pub on_paths: bool,
    pub wear: Option<[f32; 3]>,
}

/// Build once from the complete small layer/catalog definition and share with every cell job.
/// Fields are private so mutation cannot invalidate the plan's validation or fingerprint.
pub struct CompilePlan {
    pub(crate) presets: PresetLibrary,
    pub(crate) definition: EnvironmentDefinition,
    pub(crate) layers: Vec<PlannedLayer>,
    pub(crate) grass: Vec<GrassBinding>,
    /// What each surface (by index) wears bare to, shares normalised; empty: itself.
    pub(crate) worn: Vec<Vec<(usize, f64)>>,
    /// What light wear tramples each surface (by index) to, if anything.
    pub(crate) trampled: Vec<Option<usize>>,
    /// Whether roads wear each surface (by index) across their whole width.
    pub(crate) whole_road: Vec<bool>,
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
        Ok(source.validate(&self.definition, cells, self.coverage_limits())?)
    }

    pub fn new(
        source: &EnvironmentDefinition,
        grass: &VegetationCatalog,
        presets: &PresetLibrary,
        profile: CompileProfile,
    ) -> Result<Self, CompileError> {
        if !(2..=257).contains(&profile.terrain_resolution)
            || !(1..=256).contains(&profile.vegetation_resolution)
            || !(1..=world::MAX_TERRAIN_SURFACES_PER_CELL).contains(&profile.max_surfaces_per_cell)
            || profile.max_layers == 0
            || profile.max_grass_types == 0
            || profile.max_fields_per_cell == 0
            || profile.max_cells_per_batch == 0
            || profile.max_mask_samples == 0
            || profile.max_output_bytes == 0
            || profile.max_sample_work == 0
        {
            return Err(CompileError::InvalidProfile);
        }

        if source.layers.len() > profile.max_layers || source.surfaces.len() > 64 {
            return Err(CompileError::Budget("definition catalog"));
        }

        grass.validate()?;
        source.validate(presets)?;
        let mut definition = source.clone();
        definition.surfaces.sort();
        definition.layers.sort_by_key(|l| (l.order, l.id));

        for layer in &mut definition.layers {
            layer
                .overrides
                .sort_by(|a, b| (&a.path, a.value.key()).cmp(&(&b.path, b.value.key())));
        }

        let compositions: BTreeMap<LayerId, ResolvedComposition> = definition
            .layers
            .iter()
            .map(|l| {
                presets
                    .resolve(l.preset, &l.overrides)
                    .map(|resolved| (l.id, resolved))
            })
            .collect::<Result<_, _>>()?;

        // Grass and detail types in catalog order; a type grows here when some surface of this
        // world carries it.
        let bindings: Vec<_> = grass
            .type_ids()
            .map(|id| {
                let amounts = definition
                    .surfaces
                    .iter()
                    .map(|s| {
                        grass
                            .on_surface(s.0)
                            .iter()
                            .find(|a| a.grass == id)
                            .map_or(0.0, |a| f64::from(a.amount))
                    })
                    .collect::<Vec<_>>();
                let detail = grass.detail(id);
                GrassBinding {
                    grass: id,
                    amounts,
                    on_paths: detail.is_some_and(|d| d.on_paths),
                    wear: detail.and_then(|d| d.wear),
                }
            })
            .filter(|b| b.amounts.iter().any(|&a| a > 0.0))
            .collect();
        let index = |s: &world::TerrainSurfaceId| definition.surfaces.binary_search(s).unwrap();
        let mut worn = vec![vec![]; definition.surfaces.len()];
        let mut trampled = vec![None; definition.surfaces.len()];
        let mut whole_road = vec![false; definition.surfaces.len()];

        for w in &definition.worn {
            trampled[index(&w.surface)] = w.trampled.as_ref().map(index);
            whole_road[index(&w.surface)] = w.whole_road;
            let total: f64 = w.ground.iter().map(|g| f64::from(g.weight)).sum();
            worn[index(&w.surface)] = w
                .ground
                .iter()
                .map(|g| (index(&g.surface), f64::from(g.weight) / total))
                .collect();
        }

        if bindings.len() > profile.max_grass_types {
            return Err(CompileError::Budget("grass types"));
        }

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
                let grass_exclusion = composition
                    .exclusions
                    .iter()
                    .filter(|e| e.channel == GRASS_CHANNEL)
                    .map(|e| f64::from(e.strength))
                    .fold(0.0, f64::max);
                PlannedLayer {
                    source: source.clone(),
                    composition,
                    ground,
                    grass_exclusion,
                }
            })
            .collect();

        // Coverage depends on what grows where, not on how the grass looks.
        let growth: Vec<_> = bindings
            .iter()
            .map(|b| (b.grass, &b.amounts, b.on_paths, b.wear))
            .collect();
        let bytes = world::codec::encode(&(&definition, &compositions, &growth, &profile))?;
        let mut hash = blake3::Hasher::new();

        // v5: grass follows the ground that shows, not faint blend weights.
        // v7: up to twelve surfaces a page, those shared along its edges kept first.
        // v9: surfaces roads wear across their whole width.
        hash.update(b"yarra.environment.compiler.v9");
        hash.update(&bytes);
        Ok(Self {
            presets: presets.clone(),
            definition,
            layers,
            grass: bindings,
            worn,
            trampled,
            whole_road,
            catalog: grass.clone(),
            profile,
            fingerprint: *hash.finalize().as_bytes(),
        })
    }

    pub fn catalog(&self) -> &VegetationCatalog {
        &self.catalog
    }

    /// The grass types this world grows, in catalog order.
    pub fn grass_types(&self) -> impl Iterator<Item = GrassTypeId> + '_ {
        self.grass.iter().map(|b| b.grass)
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        self.fingerprint
    }
}
