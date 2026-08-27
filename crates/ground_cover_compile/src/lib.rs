//! Pure, bounded compilation from mutable ground-cover authoring records to one runtime cell page.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use thiserror::Error;
use world::{
    CellCoord, GroundCoverCluster, GroundCoverLayerId, GroundCoverPage, GroundCoverPresetId,
    GroundCoverRegionId, GroundCoverSpeciesId, GroundCoverVisualId, WorldSpaceId,
};
use world_db::{
    SourceGroundCoverCellMaskRecord, SourceGroundCoverLayerRecord, SourceGroundCoverPresetRecord,
    SourceGroundCoverRegionRecord, SourceGroundCoverVisualDefinition,
    SourceGroundCoverVisualRecord,
};

/// Immutable catalogs and cell properties needed to compile exactly one cell.
///
/// The caller owns querying and revision control. This boundary deliberately knows nothing about
/// SQLite, Bevy, editor state, or whole-project publication.
#[derive(Debug, Clone, Copy)]
pub struct GroundCoverCellContext<'a> {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub cell_size: f32,
    pub ground_height: f32,
    pub visuals: &'a [SourceGroundCoverVisualRecord],
    pub presets: &'a [SourceGroundCoverPresetRecord],
    pub layers: &'a [SourceGroundCoverLayerRecord],
    pub regions: &'a [SourceGroundCoverRegionRecord],
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GroundCoverCompileDiagnostics {
    pub input_mask_count: usize,
    pub active_mask_count: usize,
    pub merged_preset_count: usize,
    pub cluster_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledGroundCoverCell {
    pub page: GroundCoverPage,
    pub depended_species: Vec<GroundCoverSpeciesId>,
    pub maximum_y: f32,
    pub diagnostics: GroundCoverCompileDiagnostics,
}

#[derive(Debug, Error, Clone, PartialEq)]
pub enum GroundCoverCompileError {
    #[error("cell size must be finite and positive")]
    InvalidCellSize,
    #[error("ground height must be finite")]
    InvalidGroundHeight,
    #[error("duplicate ground-cover visual ID {0:?}")]
    DuplicateVisual(GroundCoverVisualId),
    #[error("ground-cover visual {0:?} is invalid")]
    InvalidVisual(GroundCoverVisualId),
    #[error("ground-cover visual {visual:?} uses unsupported built-in atlas version {version}")]
    UnsupportedAtlasVersion {
        visual: GroundCoverVisualId,
        version: u32,
    },
    #[error("duplicate ground-cover preset ID {0:?}")]
    DuplicatePreset(GroundCoverPresetId),
    #[error("ground-cover preset {0:?} is invalid")]
    InvalidPreset(GroundCoverPresetId),
    #[error("ground-cover preset {preset:?} references missing visual {visual:?}")]
    MissingVisual {
        preset: GroundCoverPresetId,
        visual: GroundCoverVisualId,
    },
    #[error("duplicate ground-cover layer ID {0:?}")]
    DuplicateLayer(GroundCoverLayerId),
    #[error("ground-cover layer {0:?} is invalid")]
    InvalidLayer(GroundCoverLayerId),
    #[error("duplicate ground-cover region ID {0:?}")]
    DuplicateRegion(GroundCoverRegionId),
    #[error("ground-cover region {0:?} is invalid")]
    InvalidRegion(GroundCoverRegionId),
    #[error("ground-cover region {region:?} references missing layer {layer:?}")]
    MissingLayer {
        region: GroundCoverRegionId,
        layer: GroundCoverLayerId,
    },
    #[error("ground-cover region {region:?} references missing preset {preset:?}")]
    MissingPreset {
        region: GroundCoverRegionId,
        preset: GroundCoverPresetId,
    },
    #[error("ground-cover mask references missing region {0:?}")]
    MissingRegion(GroundCoverRegionId),
    #[error("ground-cover mask references region {0:?} from another world space")]
    RegionOutsideSpace(GroundCoverRegionId),
    #[error("ground-cover mask for region {region:?} does not belong to requested cell {cell:?}")]
    MaskOutsideCell {
        region: GroundCoverRegionId,
        cell: CellCoord,
    },
    #[error("ground-cover mask for region {0:?} has invalid resolution or payload size")]
    InvalidMask(GroundCoverRegionId),
    #[error("overlapping regions using preset {0:?} must use the same mask resolution")]
    IncompatiblePresetResolution(GroundCoverPresetId),
}

/// Compiles one cell from its bounded source dependencies.
///
/// Regions that use the same preset are merged sample-by-sample with `max`, preserving density in
/// overlaps rather than adding it. Disabled layers, regions, and presets contribute no clusters.
pub fn compile_ground_cover_cell<'a>(
    context: GroundCoverCellContext<'a>,
    masks: impl IntoIterator<Item = &'a SourceGroundCoverCellMaskRecord>,
) -> Result<CompiledGroundCoverCell, GroundCoverCompileError> {
    validate_scalar_context(context)?;

    let visuals = index_visuals(context.visuals)?;
    let presets = index_presets(context.presets, &visuals)?;
    let layers = index_layers(context.layers)?;
    let regions = index_regions(context.regions, &layers, &presets)?;

    let mut diagnostics = GroundCoverCompileDiagnostics::default();
    let mut coverage_by_preset = BTreeMap::<GroundCoverPresetId, (u8, Vec<f32>)>::new();
    let mut resolution_by_preset = HashMap::<GroundCoverPresetId, u8>::new();
    for mask in masks {
        diagnostics.input_mask_count += 1;
        if mask.space != context.space || mask.cell != context.cell {
            return Err(GroundCoverCompileError::MaskOutsideCell {
                region: mask.region,
                cell: mask.cell,
            });
        }
        let Some(region) = regions.get(&mask.region).copied() else {
            return Err(GroundCoverCompileError::MissingRegion(mask.region));
        };
        if region.space != context.space {
            return Err(GroundCoverCompileError::RegionOutsideSpace(mask.region));
        }
        let resolution = usize::from(mask.resolution);
        if resolution == 0 || mask.coverage.len() != resolution * resolution {
            return Err(GroundCoverCompileError::InvalidMask(mask.region));
        }
        if resolution_by_preset
            .insert(region.preset, mask.resolution)
            .is_some_and(|existing| existing != mask.resolution)
        {
            return Err(GroundCoverCompileError::IncompatiblePresetResolution(
                region.preset,
            ));
        }

        let layer = layers[&region.layer];
        let preset = presets[&region.preset];
        if !layer.enabled || !region.enabled || !preset.enabled {
            continue;
        }
        diagnostics.active_mask_count += 1;
        let (existing_resolution, effective_coverage) = coverage_by_preset
            .entry(region.preset)
            .or_insert_with(|| (mask.resolution, vec![0.0; resolution * resolution]));
        debug_assert_eq!(*existing_resolution, mask.resolution);
        for (destination, coverage) in effective_coverage
            .iter_mut()
            .zip(mask.coverage.iter().copied())
        {
            *destination = destination.max(f32::from(coverage) / 255.0 * region.density_multiplier);
        }
    }

    diagnostics.merged_preset_count = coverage_by_preset.len();
    let mut clusters = Vec::new();
    let mut depended_species = BTreeSet::new();
    let mut maximum_y = context.ground_height;
    for (preset_id, (resolution, effective_coverage)) in coverage_by_preset {
        let preset = presets[&preset_id];
        let visual = visuals[&preset.visual];
        let SourceGroundCoverVisualDefinition::CardCluster(card) = &visual.definition;
        let species = GroundCoverSpeciesId(visual.id.0);
        let resolution = usize::from(resolution);
        let cluster_size = context.cell_size / resolution as f32;
        for (index, coverage) in effective_coverage.into_iter().enumerate() {
            if coverage <= 0.0 {
                continue;
            }
            let cluster_x = index % resolution;
            let cluster_z = index / resolution;
            let coverage_half_extent = cluster_size * 0.5;
            let horizontal_card_reach = card.maximum_card_height + card.maximum_wind_displacement;
            clusters.push(GroundCoverCluster {
                species,
                local_center: [
                    (cluster_x as f32 + 0.5) * cluster_size,
                    context.ground_height + card.maximum_card_height * 0.5,
                    (cluster_z as f32 + 0.5) * cluster_size,
                ],
                half_extents: [
                    coverage_half_extent + horizontal_card_reach,
                    card.maximum_card_height * 0.5,
                    coverage_half_extent + horizontal_card_reach,
                ],
                coverage_half_extents: [coverage_half_extent, coverage_half_extent],
                density_per_square_meter: preset.density_per_square_meter * coverage,
                seed: ground_cover_cluster_seed(preset.seed, context.cell, index as u32),
            });
            depended_species.insert(species);
            maximum_y = maximum_y.max(context.ground_height + card.maximum_card_height);
        }
    }
    diagnostics.cluster_count = clusters.len();

    Ok(CompiledGroundCoverCell {
        page: GroundCoverPage { clusters },
        depended_species: depended_species.into_iter().collect(),
        maximum_y,
        diagnostics,
    })
}

fn validate_scalar_context(
    context: GroundCoverCellContext<'_>,
) -> Result<(), GroundCoverCompileError> {
    if !context.cell_size.is_finite() || context.cell_size <= 0.0 {
        return Err(GroundCoverCompileError::InvalidCellSize);
    }
    if !context.ground_height.is_finite() {
        return Err(GroundCoverCompileError::InvalidGroundHeight);
    }
    Ok(())
}

fn index_visuals(
    records: &[SourceGroundCoverVisualRecord],
) -> Result<HashMap<GroundCoverVisualId, &SourceGroundCoverVisualRecord>, GroundCoverCompileError> {
    let mut indexed = HashMap::new();
    for visual in records {
        if indexed.insert(visual.id, visual).is_some() {
            return Err(GroundCoverCompileError::DuplicateVisual(visual.id));
        }
        if visual.key.is_empty() || visual.display_name.is_empty() || visual.source_revision < 0 {
            return Err(GroundCoverCompileError::InvalidVisual(visual.id));
        }
        let SourceGroundCoverVisualDefinition::CardCluster(card) = &visual.definition;
        if card.built_in_atlas_version != 1 {
            return Err(GroundCoverCompileError::UnsupportedAtlasVersion {
                visual: visual.id,
                version: card.built_in_atlas_version,
            });
        }
        if card
            .procedural_recipe
            .is_some_and(|recipe| !recipe.is_valid())
        {
            return Err(GroundCoverCompileError::InvalidVisual(visual.id));
        }
        if !valid_card_visual(card) {
            return Err(GroundCoverCompileError::InvalidVisual(visual.id));
        }
    }
    Ok(indexed)
}

fn index_presets<'a>(
    records: &'a [SourceGroundCoverPresetRecord],
    visuals: &HashMap<GroundCoverVisualId, &SourceGroundCoverVisualRecord>,
) -> Result<HashMap<GroundCoverPresetId, &'a SourceGroundCoverPresetRecord>, GroundCoverCompileError>
{
    let mut indexed = HashMap::new();
    for preset in records {
        if indexed.insert(preset.id, preset).is_some() {
            return Err(GroundCoverCompileError::DuplicatePreset(preset.id));
        }
        if preset.key.is_empty()
            || preset.display_name.is_empty()
            || preset.source_revision < 0
            || !preset.density_per_square_meter.is_finite()
            || preset.density_per_square_meter <= 0.0
        {
            return Err(GroundCoverCompileError::InvalidPreset(preset.id));
        }
        if !visuals.contains_key(&preset.visual) {
            return Err(GroundCoverCompileError::MissingVisual {
                preset: preset.id,
                visual: preset.visual,
            });
        }
    }
    Ok(indexed)
}

fn index_layers(
    records: &[SourceGroundCoverLayerRecord],
) -> Result<HashMap<GroundCoverLayerId, &SourceGroundCoverLayerRecord>, GroundCoverCompileError> {
    let mut indexed = HashMap::new();
    for layer in records {
        if indexed.insert(layer.id, layer).is_some() {
            return Err(GroundCoverCompileError::DuplicateLayer(layer.id));
        }
        if layer.key.is_empty() || layer.display_name.is_empty() || layer.source_revision < 0 {
            return Err(GroundCoverCompileError::InvalidLayer(layer.id));
        }
    }
    Ok(indexed)
}

fn index_regions<'a>(
    records: &'a [SourceGroundCoverRegionRecord],
    layers: &HashMap<GroundCoverLayerId, &SourceGroundCoverLayerRecord>,
    presets: &HashMap<GroundCoverPresetId, &SourceGroundCoverPresetRecord>,
) -> Result<HashMap<GroundCoverRegionId, &'a SourceGroundCoverRegionRecord>, GroundCoverCompileError>
{
    let mut indexed = HashMap::new();
    for region in records {
        if indexed.insert(region.id, region).is_some() {
            return Err(GroundCoverCompileError::DuplicateRegion(region.id));
        }
        if region.display_name.is_empty()
            || region.source_revision < 0
            || !region.density_multiplier.is_finite()
            || region.density_multiplier <= 0.0
        {
            return Err(GroundCoverCompileError::InvalidRegion(region.id));
        }
        let Some(layer) = layers.get(&region.layer) else {
            return Err(GroundCoverCompileError::MissingLayer {
                region: region.id,
                layer: region.layer,
            });
        };
        if layer.space != region.space {
            return Err(GroundCoverCompileError::InvalidRegion(region.id));
        }
        if !presets.contains_key(&region.preset) {
            return Err(GroundCoverCompileError::MissingPreset {
                region: region.id,
                preset: region.preset,
            });
        }
    }
    Ok(indexed)
}

fn valid_card_visual(card: &world_db::SourceGroundCoverCardVisualRecord) -> bool {
    let valid_color = |color: [f32; 3]| {
        color
            .into_iter()
            .all(|component| component.is_finite() && (0.0..=1.0).contains(&component))
    };
    valid_color(card.bottom_color)
        && valid_color(card.top_color)
        && card.minimum_card_height.is_finite()
        && card.minimum_card_height > 0.0
        && card.maximum_card_height.is_finite()
        && card.maximum_card_height >= card.minimum_card_height
        && card.minimum_card_width.is_finite()
        && card.minimum_card_width > 0.0
        && card.maximum_card_width.is_finite()
        && card.maximum_card_width >= card.minimum_card_width
        && card.flattened_card_probability.is_finite()
        && (0.0..=1.0).contains(&card.flattened_card_probability)
        && card.maximum_wind_displacement.is_finite()
        && card.maximum_wind_displacement >= 0.0
}

fn ground_cover_cluster_seed(seed: u32, cell: CellCoord, sample_index: u32) -> u32 {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&seed.to_le_bytes());
    hasher.update(&cell.x.to_le_bytes());
    hasher.update(&cell.z.to_le_bytes());
    hasher.update(&sample_index.to_le_bytes());
    let bytes = hasher.finalize();
    u32::from_le_bytes(bytes.as_bytes()[..4].try_into().expect("four seed bytes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::{
        GroundCoverLayerId, GroundCoverPresetId, GroundCoverRegionId, GroundCoverVisualId,
    };
    use world_db::SourceGroundCoverCardVisualRecord;

    fn id(value: u8) -> [u8; 16] {
        [value; 16]
    }

    fn visual() -> SourceGroundCoverVisualRecord {
        SourceGroundCoverVisualRecord {
            id: GroundCoverVisualId(id(1)),
            key: "meadow".into(),
            display_name: "Meadow".into(),
            source_revision: 1,
            definition: SourceGroundCoverVisualDefinition::CardCluster(
                SourceGroundCoverCardVisualRecord {
                    built_in_atlas_version: 1,
                    procedural_recipe: None,
                    bottom_color: [0.1, 0.2, 0.1],
                    top_color: [0.2, 0.4, 0.2],
                    minimum_card_height: 0.5,
                    maximum_card_height: 1.0,
                    minimum_card_width: 0.1,
                    maximum_card_width: 0.2,
                    flattened_card_probability: 0.1,
                    maximum_wind_displacement: 0.2,
                },
            ),
        }
    }

    fn preset(enabled: bool) -> SourceGroundCoverPresetRecord {
        SourceGroundCoverPresetRecord {
            id: GroundCoverPresetId(id(2)),
            key: "dense-meadow".into(),
            display_name: "Dense meadow".into(),
            enabled,
            visual: GroundCoverVisualId(id(1)),
            density_per_square_meter: 10.0,
            seed: 41,
            source_revision: 1,
        }
    }

    fn layer(enabled: bool) -> SourceGroundCoverLayerRecord {
        SourceGroundCoverLayerRecord {
            id: GroundCoverLayerId(id(3)),
            space: WorldSpaceId(1),
            key: "natural".into(),
            display_name: "Natural".into(),
            enabled,
            sort_order: 0,
            source_revision: 1,
        }
    }

    fn region(value: u8, multiplier: f32, enabled: bool) -> SourceGroundCoverRegionRecord {
        SourceGroundCoverRegionRecord {
            id: GroundCoverRegionId(id(value)),
            layer: GroundCoverLayerId(id(3)),
            space: WorldSpaceId(1),
            preset: GroundCoverPresetId(id(2)),
            display_name: format!("Region {value}"),
            enabled,
            density_multiplier: multiplier,
            source_revision: 1,
        }
    }

    fn mask(region: u8, resolution: u8, coverage: Vec<u8>) -> SourceGroundCoverCellMaskRecord {
        SourceGroundCoverCellMaskRecord {
            region: GroundCoverRegionId(id(region)),
            space: WorldSpaceId(1),
            cell: CellCoord { x: 2, z: -3 },
            resolution,
            coverage,
            source_revision: 1,
        }
    }

    fn compile(
        preset: SourceGroundCoverPresetRecord,
        layer: SourceGroundCoverLayerRecord,
        regions: Vec<SourceGroundCoverRegionRecord>,
        masks: &[SourceGroundCoverCellMaskRecord],
    ) -> Result<CompiledGroundCoverCell, GroundCoverCompileError> {
        let visuals = [visual()];
        let presets = [preset];
        let layers = [layer];
        compile_ground_cover_cell(
            GroundCoverCellContext {
                space: WorldSpaceId(1),
                cell: CellCoord { x: 2, z: -3 },
                cell_size: 32.0,
                ground_height: 4.0,
                visuals: &visuals,
                presets: &presets,
                layers: &layers,
                regions: &regions,
            },
            masks,
        )
    }

    #[test]
    fn compiles_one_exact_runtime_page_with_stable_seeds_and_bounds() {
        let masks = [mask(4, 2, vec![255, 0, 128, 64])];
        let compiled = compile(
            preset(true),
            layer(true),
            vec![region(4, 1.0, true)],
            &masks,
        )
        .expect("valid bounded cell should compile");
        assert_eq!(compiled.page.clusters.len(), 3);
        assert_eq!(compiled.depended_species, vec![GroundCoverSpeciesId(id(1))]);
        assert_eq!(compiled.maximum_y, 5.0);
        assert_eq!(compiled.page.clusters[0].local_center, [8.0, 4.5, 8.0]);
        assert_eq!(compiled.page.clusters[0].coverage_half_extents, [8.0, 8.0]);
        assert_eq!(compiled.page.clusters[0].density_per_square_meter, 10.0);
        assert_eq!(
            compiled.page.clusters[0].seed,
            ground_cover_cluster_seed(41, CellCoord { x: 2, z: -3 }, 0)
        );
    }

    #[test]
    fn overlapping_regions_using_one_preset_merge_with_maximum_coverage() {
        let masks = [mask(4, 1, vec![128]), mask(5, 1, vec![128])];
        let compiled = compile(
            preset(true),
            layer(true),
            vec![region(4, 1.0, true), region(5, 0.5, true)],
            &masks,
        )
        .unwrap();
        assert_eq!(compiled.page.clusters.len(), 1);
        assert_eq!(
            compiled.page.clusters[0].density_per_square_meter,
            10.0 * (128.0 / 255.0)
        );
        assert_eq!(compiled.diagnostics.merged_preset_count, 1);
    }

    #[test]
    fn disabled_authoring_levels_emit_an_empty_page() {
        let masks = [mask(4, 1, vec![255])];
        for (preset_enabled, layer_enabled, region_enabled) in [
            (false, true, true),
            (true, false, true),
            (true, true, false),
        ] {
            let compiled = compile(
                preset(preset_enabled),
                layer(layer_enabled),
                vec![region(4, 1.0, region_enabled)],
                &masks,
            )
            .unwrap();
            assert!(compiled.page.clusters.is_empty());
            assert_eq!(compiled.maximum_y, 4.0);
        }
    }

    #[test]
    fn incompatible_overlapping_resolutions_are_rejected() {
        let masks = [mask(4, 1, vec![255]), mask(5, 2, vec![255; 4])];
        let error = compile(
            preset(true),
            layer(true),
            vec![region(4, 1.0, true), region(5, 1.0, true)],
            &masks,
        )
        .unwrap_err();
        assert_eq!(
            error,
            GroundCoverCompileError::IncompatiblePresetResolution(GroundCoverPresetId(id(2)))
        );
    }

    #[test]
    fn a_mask_with_an_unresolved_region_is_rejected_instead_of_disappearing() {
        let masks = [mask(9, 1, vec![255])];
        let error = compile(preset(true), layer(true), Vec::new(), &masks).unwrap_err();
        assert_eq!(
            error,
            GroundCoverCompileError::MissingRegion(GroundCoverRegionId(id(9)))
        );
    }
}
