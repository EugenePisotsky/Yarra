use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs,
    path::Path,
    sync::OnceLock,
};

use anyhow::{Context, Result, bail};
use glam::Vec2;
use ground_cover_compile::{GroundCoverCellContext, compile_ground_cover_cell};
use world::{
    AssetId, CellCoord, DEFAULT_CELL_SIZE, GameplayObjectInstance, GameplayObjectsPage,
    GroundCoverLayerId, GroundCoverPresetId, GroundCoverRegionId, GroundCoverSpecies,
    GroundCoverVisualId, MAX_GROUND_COVER_ARTWORK_ATLAS_LAYERS, MAX_TERRAIN_SURFACES_PER_CELL,
    MAX_TERRAIN_WEIGHT_PAGES, MAX_TERRAIN_WEIGHT_RESOLUTION, ObjectActivationPolicy,
    ObjectDefinitionId, PageCodec, PageDomain, PageKey, PagePayload, RUNTIME_SCHEMA_VERSION,
    StableObjectId, StaticObjectInstance, StaticObjectsPage, TerrainProfile, TerrainRenderPage,
    TerrainSurface, TerrainSurfaceId, TerrainTextureLayer, TerrainTextureSet, TerrainTextureSetId,
    TerrainWeightPage, WorldSpaceId, encode_page_payload,
};
use world_db::{
    AssetVariantRecord, EncodedPage, PageDependencyRecord, PageGroundCoverSpeciesRecord,
    PageObjectDefinitionRecord, PageTerrainSurfaceRecord, ProjectDocument, RuntimeBuild,
    RuntimeCellRecord, RuntimeManifest, RuntimeObjectDefinition, SourceAssetRecord,
    SourceAssetVariantRecord, SourceCellRecord, SourceGroundCoverCardVisualRecord,
    SourceGroundCoverCellMaskRecord, SourceGroundCoverLayerRecord, SourceGroundCoverPresetRecord,
    SourceGroundCoverRegionRecord, SourceGroundCoverVisualDefinition,
    SourceGroundCoverVisualRecord, SourceObjectDefinitionRecord, SourceObjectRecord,
    SourceTerrainCellSurfaceSlotRecord, SourceTerrainCellWeightPageRecord, WorldSpaceRecord,
    domain_bit, read_project_database, write_project_database, write_runtime_database,
};

pub const DEMO_TREE_KEY: &str = "forest_tree_starter_kit/tree_07";
pub const DEMO_TREE_SOURCE_URI: &str =
    "local/forest_tree_starter_kit/source/tree_07/DA_Forest_Tree_11364_Tris.FBX";
pub const DEMO_TREE_LOD_URIS: [&str; 4] = [
    "local/forest_tree_starter_kit/runtime/tree_07/summer/tree_07_summer_lod0.gltf",
    "local/forest_tree_starter_kit/runtime/tree_07/summer/tree_07_summer_lod1.gltf",
    "local/forest_tree_starter_kit/runtime/tree_07/summer/tree_07_summer_lod2.gltf",
    "local/forest_tree_starter_kit/runtime/tree_07/summer/tree_07_summer_lod3.gltf",
];
const DEMO_TREE_MINIMUM_SCREEN_HEIGHTS: [f32; 4] = [320.0, 160.0, 80.0, 0.0];
const DEMO_TREE_GPU_BYTES: [u64; 4] = [593_464, 324_612, 175_064, 85_164];
const DEMO_TREE_BOUNDS: [f32; 3] = [7.9161, 15.9346, 5.5864];
const DEMO_MEADOW_CELL_RANGE: std::ops::Range<i32> = -30..30;
// Endpoint-inclusive samples: 65 gives the large 60-by-60-cell demo a
// half-metre control-map interval while keeping its Git-tracked project
// database below the practical size of the legacy 256-sample authoring maps.
const DEMO_TERRAIN_WEIGHT_RESOLUTION: u16 = 65;
const DEMO_TERRAIN_TEXTURE_ROOT: &str = "local/terrain/temperate_meadow/runtime";

pub fn create_demo_project(path: &Path) -> Result<()> {
    let document = demo_project_document();
    write_project_database(path, &document)
        .with_context(|| format!("failed to create demo project at {}", path.display()))
}

pub fn cook_project(project_path: &Path, runtime_path: &Path) -> Result<RuntimeManifest> {
    let project = read_project_database(project_path)
        .with_context(|| format!("failed to read project database {}", project_path.display()))?;
    let build = build_runtime(project)?;
    publish_runtime_database(runtime_path, &build)?;
    Ok(build.manifest)
}

pub fn build_runtime(mut project: ProjectDocument) -> Result<RuntimeBuild> {
    if project.world_spaces.is_empty() {
        bail!("a project must contain at least one world space");
    }
    if !project
        .world_spaces
        .iter()
        .any(|space| space.id == project.default_world_space)
    {
        bail!(
            "default world space {:?} is not present in the project",
            project.default_world_space
        );
    }
    project.world_spaces.sort_by_key(|space| space.id);
    project.cells.sort_by_key(|cell| (cell.space, cell.cell));
    project.terrain_surfaces.sort_by_key(|surface| surface.id);
    project
        .terrain_texture_sets
        .sort_by_key(|texture_set| texture_set.id);
    project
        .terrain_texture_layers
        .sort_by_key(|layer| (layer.texture_set, layer.layer));
    project
        .terrain_profiles
        .sort_by_key(|profile| profile.space);
    project
        .terrain_cell_surface_slots
        .sort_by_key(|slot| (slot.space, slot.cell, slot.slot));
    project
        .terrain_cell_weight_pages
        .sort_by_key(|weights| (weights.space, weights.cell, weights.page));
    project.ground_cover_visuals.sort_by_key(|visual| visual.id);
    project.ground_cover_presets.sort_by_key(|preset| preset.id);
    project.ground_cover_layers.sort_by_key(|layer| layer.id);
    project.ground_cover_regions.sort_by_key(|region| region.id);
    project
        .ground_cover_masks
        .sort_by_key(|mask| (mask.region, mask.space, mask.cell));
    project.assets.sort_by_key(|asset| asset.id.0);
    project
        .asset_variants
        .sort_by_key(|variant| (variant.asset.0, variant.lod));
    project.definitions.sort_by_key(|definition| definition.id);
    project.objects.sort_by_key(|object| object.id.0);

    let spaces_by_id: HashMap<_, _> = project
        .world_spaces
        .iter()
        .map(|space| (space.id, space))
        .collect();
    let source_cells: HashSet<_> = project
        .cells
        .iter()
        .map(|cell| (cell.space, cell.cell))
        .collect();
    let terrain_surfaces_by_id: HashMap<_, _> = project
        .terrain_surfaces
        .iter()
        .map(|surface| (surface.id, surface))
        .collect();
    if terrain_surfaces_by_id.len() != project.terrain_surfaces.len() {
        bail!("terrain surface IDs must be unique");
    }
    if project
        .terrain_surfaces
        .iter()
        .map(|surface| surface.key.as_str())
        .collect::<HashSet<_>>()
        .len()
        != project.terrain_surfaces.len()
    {
        bail!("terrain surface keys must be unique");
    }
    for surface in &project.terrain_surfaces {
        validate_terrain_surface(surface)?;
    }
    let terrain_texture_sets_by_id: HashMap<_, _> = project
        .terrain_texture_sets
        .iter()
        .map(|texture_set| (texture_set.id, texture_set))
        .collect();
    if terrain_texture_sets_by_id.len() != project.terrain_texture_sets.len() {
        bail!("terrain texture-set IDs must be unique");
    }
    if project
        .terrain_texture_sets
        .iter()
        .map(|texture_set| texture_set.key.as_str())
        .collect::<HashSet<_>>()
        .len()
        != project.terrain_texture_sets.len()
    {
        bail!("terrain texture-set keys must be unique");
    }
    for texture_set in &project.terrain_texture_sets {
        validate_terrain_texture_set(texture_set)?;
    }
    let mut terrain_layers_by_surface = HashMap::new();
    let mut terrain_layers_by_index = HashSet::new();
    let mut terrain_layer_indices_by_set: BTreeMap<TerrainTextureSetId, Vec<u16>> = BTreeMap::new();
    for layer in &project.terrain_texture_layers {
        if !terrain_texture_sets_by_id.contains_key(&layer.texture_set) {
            bail!("terrain texture layer references a missing texture set");
        }
        if !terrain_surfaces_by_id.contains_key(&layer.surface) {
            bail!("terrain texture layer references a missing surface");
        }
        if terrain_layers_by_surface
            .insert((layer.texture_set, layer.surface), layer.layer)
            .is_some()
            || !terrain_layers_by_index.insert((layer.texture_set, layer.layer))
        {
            bail!("terrain texture-set layers must be unique");
        }
        terrain_layer_indices_by_set
            .entry(layer.texture_set)
            .or_default()
            .push(layer.layer);
    }
    for (texture_set, layers) in &terrain_layer_indices_by_set {
        if layers
            .iter()
            .enumerate()
            .any(|(expected, layer)| usize::from(*layer) != expected)
        {
            bail!(
                "terrain texture set {:?} layers must be contiguous from zero",
                texture_set
            );
        }
    }
    let terrain_profiles_by_space: HashMap<_, _> = project
        .terrain_profiles
        .iter()
        .map(|profile| (profile.space, profile))
        .collect();
    if terrain_profiles_by_space.len() != project.terrain_profiles.len() {
        bail!("world spaces may have only one terrain profile");
    }
    for space in &project.world_spaces {
        let Some(profile) = terrain_profiles_by_space.get(&space.id) else {
            bail!("world space {:?} has no terrain profile", space.id);
        };
        validate_terrain_profile(profile)?;
        if !terrain_texture_sets_by_id.contains_key(&profile.texture_set) {
            bail!(
                "world space {:?} references a missing terrain texture set",
                space.id
            );
        }
    }
    let mut terrain_slots_by_cell: BTreeMap<
        (WorldSpaceId, CellCoord),
        Vec<&SourceTerrainCellSurfaceSlotRecord>,
    > = BTreeMap::new();
    for slot in &project.terrain_cell_surface_slots {
        if !source_cells.contains(&(slot.space, slot.cell)) {
            bail!(
                "terrain surface slot references missing cell {:?}",
                slot.cell
            );
        }
        let Some(profile) = terrain_profiles_by_space.get(&slot.space) else {
            bail!("terrain surface slot references an unprofiled world space");
        };
        if !terrain_layers_by_surface.contains_key(&(profile.texture_set, slot.surface)) {
            bail!("terrain surface slot is not present in the world's texture set");
        }
        terrain_slots_by_cell
            .entry((slot.space, slot.cell))
            .or_default()
            .push(slot);
    }
    let mut terrain_weights_by_cell: BTreeMap<
        (WorldSpaceId, CellCoord),
        Vec<&SourceTerrainCellWeightPageRecord>,
    > = BTreeMap::new();
    for weights in &project.terrain_cell_weight_pages {
        if !source_cells.contains(&(weights.space, weights.cell)) {
            bail!(
                "terrain weight page references missing cell {:?}",
                weights.cell
            );
        }
        let profile = terrain_profiles_by_space[&weights.space];
        let expected_bytes = usize::from(weights.resolution).pow(2) * 4;
        if weights.resolution != profile.weight_resolution || weights.rgba.len() != expected_bytes {
            bail!("terrain weight page has the wrong resolution or byte count");
        }
        terrain_weights_by_cell
            .entry((weights.space, weights.cell))
            .or_default()
            .push(weights);
    }
    for source_cell in &project.cells {
        let Some(slots) = terrain_slots_by_cell.get(&(source_cell.space, source_cell.cell)) else {
            bail!("terrain cell {:?} has no surface slots", source_cell.cell);
        };
        if slots.is_empty() || slots.len() > MAX_TERRAIN_SURFACES_PER_CELL {
            bail!(
                "terrain cell {:?} has an invalid surface count",
                source_cell.cell
            );
        }
        if slots
            .iter()
            .enumerate()
            .any(|(expected, slot)| usize::from(slot.slot) != expected)
        {
            bail!(
                "terrain cell {:?} surface slots must be contiguous",
                source_cell.cell
            );
        }
        let expected_weight_pages = slots.len().div_ceil(4);
        let actual_weight_pages = terrain_weights_by_cell
            .get(&(source_cell.space, source_cell.cell))
            .map_or(0, Vec::len);
        if terrain_weights_by_cell
            .get(&(source_cell.space, source_cell.cell))
            .is_some_and(|pages| {
                pages
                    .iter()
                    .enumerate()
                    .any(|(expected, page)| usize::from(page.page) != expected)
            })
        {
            bail!(
                "terrain cell {:?} weight pages must be contiguous",
                source_cell.cell
            );
        }
        if slots.len() == 1 {
            if actual_weight_pages != 0 {
                bail!("constant terrain cells must not store weight pages");
            }
        } else if actual_weight_pages != expected_weight_pages
            || actual_weight_pages > MAX_TERRAIN_WEIGHT_PAGES
        {
            bail!(
                "terrain cell {:?} has the wrong number of weight pages",
                source_cell.cell
            );
        }
    }
    validate_terrain_weight_borders(&terrain_weights_by_cell)?;
    let ground_cover_visuals_by_id: HashMap<_, _> = project
        .ground_cover_visuals
        .iter()
        .map(|visual| (visual.id, visual))
        .collect();
    if ground_cover_visuals_by_id.len() != project.ground_cover_visuals.len() {
        bail!("ground-cover visual IDs must be unique");
    }
    if project
        .ground_cover_visuals
        .iter()
        .map(|visual| visual.key.as_str())
        .collect::<HashSet<_>>()
        .len()
        != project.ground_cover_visuals.len()
    {
        bail!("ground-cover visual keys must be unique");
    }
    let mut ground_cover_species_by_visual = HashMap::new();
    for visual in &project.ground_cover_visuals {
        validate_ground_cover_visual(visual)?;
        ground_cover_species_by_visual.insert(visual.id, visual.runtime_species());
    }
    let artwork_layers = ground_cover_species_by_visual
        .values()
        .map(|species| u32::from(species.artwork.variant_count))
        .sum::<u32>();
    if artwork_layers > MAX_GROUND_COVER_ARTWORK_ATLAS_LAYERS {
        bail!(
            "ground-cover visuals require {artwork_layers} artwork layers; the bounded runtime atlas supports {}",
            MAX_GROUND_COVER_ARTWORK_ATLAS_LAYERS
        );
    }
    let ground_cover_presets_by_id: HashMap<_, _> = project
        .ground_cover_presets
        .iter()
        .map(|preset| (preset.id, preset))
        .collect();
    if ground_cover_presets_by_id.len() != project.ground_cover_presets.len() {
        bail!("ground-cover preset IDs must be unique");
    }
    if project
        .ground_cover_presets
        .iter()
        .map(|preset| preset.key.as_str())
        .collect::<HashSet<_>>()
        .len()
        != project.ground_cover_presets.len()
    {
        bail!("ground-cover preset keys must be unique");
    }
    for preset in &project.ground_cover_presets {
        if preset.key.is_empty()
            || preset.display_name.is_empty()
            || preset.source_revision < 0
            || !ground_cover_visuals_by_id.contains_key(&preset.visual)
            || !preset.density_per_square_meter.is_finite()
            || preset.density_per_square_meter <= 0.0
        {
            bail!("ground-cover preset {:?} is invalid", preset.id);
        }
    }
    let ground_cover_layers_by_id: HashMap<_, _> = project
        .ground_cover_layers
        .iter()
        .map(|layer| (layer.id, layer))
        .collect();
    if ground_cover_layers_by_id.len() != project.ground_cover_layers.len() {
        bail!("ground-cover layer IDs must be unique");
    }
    for layer in &project.ground_cover_layers {
        if layer.key.is_empty()
            || layer.display_name.is_empty()
            || layer.source_revision < 0
            || !spaces_by_id.contains_key(&layer.space)
        {
            bail!(
                "ground-cover layer {:?} is invalid or references a missing world space",
                layer.id
            );
        }
    }
    let ground_cover_regions_by_id: HashMap<_, _> = project
        .ground_cover_regions
        .iter()
        .map(|region| (region.id, region))
        .collect();
    if ground_cover_regions_by_id.len() != project.ground_cover_regions.len() {
        bail!("ground-cover region IDs must be unique");
    }
    for region in &project.ground_cover_regions {
        let Some(layer) = ground_cover_layers_by_id.get(&region.layer) else {
            bail!(
                "ground-cover region {:?} references a missing layer",
                region.id
            );
        };
        if region.display_name.is_empty()
            || region.source_revision < 0
            || layer.space != region.space
            || !ground_cover_presets_by_id.contains_key(&region.preset)
            || !region.density_multiplier.is_finite()
            || region.density_multiplier <= 0.0
        {
            bail!("ground-cover region {:?} is invalid", region.id);
        }
    }
    let mut ground_cover_masks_by_cell: BTreeMap<
        (WorldSpaceId, CellCoord),
        Vec<&SourceGroundCoverCellMaskRecord>,
    > = BTreeMap::new();
    let mut preset_resolution_by_cell = HashMap::new();
    for mask in &project.ground_cover_masks {
        let Some(region) = ground_cover_regions_by_id.get(&mask.region) else {
            bail!(
                "ground-cover mask references missing region {:?}",
                mask.region
            );
        };
        if region.space != mask.space {
            bail!("ground-cover mask and region belong to different world spaces");
        }
        if !source_cells.contains(&(mask.space, mask.cell)) {
            bail!("ground-cover mask references missing cell {:?}", mask.cell);
        }
        let resolution = usize::from(mask.resolution);
        if resolution == 0 || mask.coverage.len() != resolution * resolution {
            bail!(
                "ground-cover mask {:?}/{:?} has invalid resolution or payload size",
                mask.region,
                mask.cell
            );
        }
        let resolution_key = (mask.space, mask.cell, region.preset);
        if preset_resolution_by_cell
            .insert(resolution_key, mask.resolution)
            .is_some_and(|existing| existing != mask.resolution)
        {
            bail!("overlapping regions using one preset must use the same mask resolution");
        }
        ground_cover_masks_by_cell
            .entry((mask.space, mask.cell))
            .or_default()
            .push(mask);
    }

    let assets_by_id: HashMap<_, _> = project
        .assets
        .iter()
        .map(|asset| (asset.id, asset))
        .collect();
    let mut variants_by_asset: HashMap<AssetId, Vec<(u8, f32)>> = HashMap::new();
    let mut maximum_height_by_asset: HashMap<AssetId, f32> = HashMap::new();
    for variant in &project.asset_variants {
        if !assets_by_id.contains_key(&variant.asset) {
            bail!(
                "LOD{} references missing asset {:?}",
                variant.lod,
                variant.asset
            );
        }
        if !variant.minimum_screen_height.is_finite() {
            bail!(
                "asset {:?} LOD{} has a non-finite screen-height threshold",
                variant.asset,
                variant.lod
            );
        }
        variants_by_asset
            .entry(variant.asset)
            .or_default()
            .push((variant.lod, variant.minimum_screen_height));
        maximum_height_by_asset
            .entry(variant.asset)
            .and_modify(|height| *height = height.max(variant.bounds[1]))
            .or_insert(variant.bounds[1]);
    }
    for (asset, variants) in &variants_by_asset {
        if variants.windows(2).any(|pair| pair[1].1 > pair[0].1) {
            bail!("asset {:?} LOD thresholds must descend by LOD", asset);
        }
        if variants.last().is_some_and(|variant| variant.1 != 0.0) {
            bail!("asset {:?} coarsest LOD threshold must be zero", asset);
        }
    }

    let definitions_by_id: HashMap<_, _> = project
        .definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect();
    for definition in &project.definitions {
        let Some(asset) = definition.visual_asset else {
            continue;
        };
        if !assets_by_id.contains_key(&asset) {
            bail!(
                "definition {:?} references missing visual asset {:?}",
                definition.id,
                asset
            );
        }
        if !variants_by_asset.contains_key(&asset) {
            bail!("visual asset {:?} has no runtime LOD variants", asset);
        }
    }
    for object in &project.objects {
        if !definitions_by_id.contains_key(&object.definition) {
            bail!(
                "object {:?} references missing definition {:?}",
                object.id,
                object.definition
            );
        }
    }

    let mut objects_by_cell: BTreeMap<(WorldSpaceId, CellCoord), Vec<&SourceObjectRecord>> =
        BTreeMap::new();
    for object in &project.objects {
        objects_by_cell
            .entry((object.space, object.owner_cell))
            .or_default()
            .push(object);
    }

    let mut cells = Vec::with_capacity(project.cells.len());
    let mut pages = Vec::with_capacity(project.cells.len() * 3);
    let mut dependencies = Vec::new();
    let mut definition_dependencies = Vec::new();
    let mut ground_cover_species_dependencies = Vec::new();
    let mut terrain_surface_dependencies = Vec::new();
    let mut content_hasher = blake3::Hasher::new();

    content_hasher.update(&project.default_world_space.0.to_le_bytes());
    for space in &project.world_spaces {
        content_hasher.update(&space.id.0.to_le_bytes());
        content_hasher.update(space.name.as_bytes());
        content_hasher.update(&space.cell_size.to_bits().to_le_bytes());
        content_hasher.update(&space.minimum_y.to_bits().to_le_bytes());
        content_hasher.update(&space.maximum_y.to_bits().to_le_bytes());
    }
    hash_terrain_catalog(&mut content_hasher, &project);
    for visual in &project.ground_cover_visuals {
        let species = &ground_cover_species_by_visual[&visual.id];
        content_hasher.update(&species.id.0);
        content_hasher.update(species.key.as_bytes());
        content_hasher.update(visual.display_name.as_bytes());
        content_hasher.update(&visual.source_revision.to_le_bytes());
        let SourceGroundCoverVisualDefinition::CardCluster(card) = &visual.definition;
        content_hasher.update(&card.built_in_atlas_version.to_le_bytes());
        content_hasher.update(&[u8::from(card.procedural_recipe.is_some())]);
        for color in species.bottom_color.into_iter().chain(species.top_color) {
            content_hasher.update(&color.to_bits().to_le_bytes());
        }
        content_hasher.update(&species.minimum_card_height.to_bits().to_le_bytes());
        content_hasher.update(&species.maximum_card_height.to_bits().to_le_bytes());
        content_hasher.update(&species.minimum_card_width.to_bits().to_le_bytes());
        content_hasher.update(&species.maximum_card_width.to_bits().to_le_bytes());
        content_hasher.update(&species.flattened_card_probability.to_bits().to_le_bytes());
        content_hasher.update(&species.maximum_wind_displacement.to_bits().to_le_bytes());
        content_hasher.update(&species.artwork.resolution.to_le_bytes());
        content_hasher.update(&[
            species.artwork.variant_count,
            species.artwork.mip_level_count,
        ]);
        content_hasher.update(&species.artwork.coverage_mips);
    }
    for preset in &project.ground_cover_presets {
        content_hasher.update(&preset.id.0);
        content_hasher.update(preset.key.as_bytes());
        content_hasher.update(preset.display_name.as_bytes());
        content_hasher.update(&[u8::from(preset.enabled)]);
        content_hasher.update(&preset.visual.0);
        content_hasher.update(&preset.density_per_square_meter.to_bits().to_le_bytes());
        content_hasher.update(&preset.seed.to_le_bytes());
        content_hasher.update(&preset.source_revision.to_le_bytes());
    }
    for layer in &project.ground_cover_layers {
        content_hasher.update(&layer.id.0);
        content_hasher.update(&layer.space.0.to_le_bytes());
        content_hasher.update(layer.key.as_bytes());
        content_hasher.update(layer.display_name.as_bytes());
        content_hasher.update(&[u8::from(layer.enabled)]);
        content_hasher.update(&layer.sort_order.to_le_bytes());
        content_hasher.update(&layer.source_revision.to_le_bytes());
    }
    for region in &project.ground_cover_regions {
        content_hasher.update(&region.id.0);
        content_hasher.update(&region.layer.0);
        content_hasher.update(&region.space.0.to_le_bytes());
        content_hasher.update(&region.preset.0);
        content_hasher.update(region.display_name.as_bytes());
        content_hasher.update(&[u8::from(region.enabled)]);
        content_hasher.update(&region.density_multiplier.to_bits().to_le_bytes());
        content_hasher.update(&region.source_revision.to_le_bytes());
    }
    for mask in &project.ground_cover_masks {
        content_hasher.update(&mask.region.0);
        content_hasher.update(&mask.space.0.to_le_bytes());
        content_hasher.update(&mask.cell.x.to_le_bytes());
        content_hasher.update(&mask.cell.z.to_le_bytes());
        content_hasher.update(&[mask.resolution]);
        content_hasher.update(&mask.coverage);
        content_hasher.update(&mask.source_revision.to_le_bytes());
    }
    for asset in &project.assets {
        content_hasher.update(&asset.id.0);
        content_hasher.update(asset.key.as_bytes());
        content_hasher.update(asset.kind.as_bytes());
        content_hasher.update(asset.source_uri.as_bytes());
    }
    for variant in &project.asset_variants {
        content_hasher.update(&variant.asset.0);
        content_hasher.update(&[variant.lod]);
        content_hasher.update(variant.uri.as_bytes());
        for bound in variant.bounds {
            content_hasher.update(&bound.to_bits().to_le_bytes());
        }
        content_hasher.update(&variant.gpu_bytes_estimate.to_le_bytes());
        content_hasher.update(&variant.shadow_policy.to_le_bytes());
        content_hasher.update(&variant.minimum_screen_height.to_bits().to_le_bytes());
    }
    for definition in &project.definitions {
        content_hasher.update(&definition.id.0);
        content_hasher.update(definition.key.as_bytes());
        content_hasher.update(definition.display_name.as_bytes());
        content_hasher.update(&[definition.activation as u8]);
        if let Some(asset) = definition.visual_asset {
            content_hasher.update(&asset.0);
        }
    }

    for source_cell in &project.cells {
        let terrain_slots =
            terrain_slots_by_cell[&(source_cell.space, source_cell.cell)].as_slice();
        let terrain_weights = terrain_weights_by_cell
            .get(&(source_cell.space, source_cell.cell))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let terrain_key = PageKey {
            space: source_cell.space,
            cell: source_cell.cell,
            domain: PageDomain::TerrainRender,
            lod: 0,
        };
        let terrain_page = encoded_page(
            terrain_key,
            PagePayload::TerrainRender(TerrainRenderPage {
                height: source_cell.height,
                surfaces: terrain_slots.iter().map(|slot| slot.surface).collect(),
                weight_pages: terrain_weights
                    .iter()
                    .map(|weights| TerrainWeightPage {
                        resolution: weights.resolution,
                        rgba: weights.rgba.clone(),
                    })
                    .collect(),
            }),
            terrain_weights
                .iter()
                .map(|weights| weights.rgba.len() as u64)
                .sum(),
        )?;
        hash_page(&mut content_hasher, &terrain_page);
        pages.push(terrain_page);
        terrain_surface_dependencies.extend(terrain_slots.iter().map(|slot| {
            PageTerrainSurfaceRecord {
                page: terrain_key,
                surface: slot.surface,
            }
        }));

        let source_objects = objects_by_cell
            .get(&(source_cell.space, source_cell.cell))
            .map(Vec::as_slice)
            .unwrap_or_default();
        let mut render_objects = Vec::new();
        let mut gameplay_objects = Vec::new();
        for object in source_objects {
            let definition = definitions_by_id[&object.definition];
            if let Some(asset) = definition.visual_asset {
                render_objects.push((*object, asset));
            }
            if definition.activation == ObjectActivationPolicy::Proximity {
                gameplay_objects.push(*object);
            }
        }

        let mut domain_mask = domain_bit(PageDomain::TerrainRender);
        let mut maximum_ground_cover_y = source_cell.height;
        if let Some(masks) = ground_cover_masks_by_cell.get(&(source_cell.space, source_cell.cell))
        {
            let region_ids = masks
                .iter()
                .map(|mask| mask.region)
                .collect::<BTreeSet<_>>();
            let bounded_regions = region_ids
                .iter()
                .map(|region| (*ground_cover_regions_by_id[region]).clone())
                .collect::<Vec<_>>();
            let layer_ids = bounded_regions
                .iter()
                .map(|region| region.layer)
                .collect::<BTreeSet<_>>();
            let preset_ids = bounded_regions
                .iter()
                .map(|region| region.preset)
                .collect::<BTreeSet<_>>();
            let bounded_layers = layer_ids
                .iter()
                .map(|layer| (*ground_cover_layers_by_id[layer]).clone())
                .collect::<Vec<_>>();
            let bounded_presets = preset_ids
                .iter()
                .map(|preset| (*ground_cover_presets_by_id[preset]).clone())
                .collect::<Vec<_>>();
            let visual_ids = bounded_presets
                .iter()
                .map(|preset| preset.visual)
                .collect::<BTreeSet<_>>();
            let bounded_visuals = visual_ids
                .iter()
                .map(|visual| (*ground_cover_visuals_by_id[visual]).clone())
                .collect::<Vec<_>>();
            let compiled = compile_ground_cover_cell(
                GroundCoverCellContext {
                    space: source_cell.space,
                    cell: source_cell.cell,
                    cell_size: spaces_by_id[&source_cell.space].cell_size,
                    ground_height: source_cell.height,
                    visuals: &bounded_visuals,
                    presets: &bounded_presets,
                    layers: &bounded_layers,
                    regions: &bounded_regions,
                },
                masks.iter().copied(),
            )
            .with_context(|| {
                format!(
                    "failed to compile ground cover for space {:?}, cell {:?}",
                    source_cell.space, source_cell.cell
                )
            })?;
            maximum_ground_cover_y = compiled.maximum_y;
            if !compiled.page.clusters.is_empty() {
                let key = PageKey {
                    space: source_cell.space,
                    cell: source_cell.cell,
                    domain: PageDomain::GroundCover,
                    lod: 0,
                };
                let gpu_bytes_estimate = compiled.page.clusters.len() as u64 * 64;
                let page = encoded_page(
                    key,
                    PagePayload::GroundCover(compiled.page),
                    gpu_bytes_estimate,
                )?;
                hash_page(&mut content_hasher, &page);
                pages.push(page);
                domain_mask |= domain_bit(PageDomain::GroundCover);
                for species in compiled.depended_species {
                    ground_cover_species_dependencies
                        .push(PageGroundCoverSpeciesRecord { page: key, species });
                }
            }
        }
        if !render_objects.is_empty() {
            domain_mask |= domain_bit(PageDomain::StaticObjects);
            let object_key = PageKey {
                space: source_cell.space,
                cell: source_cell.cell,
                domain: PageDomain::StaticObjects,
                lod: 0,
            };
            let instances = render_objects
                .iter()
                .map(|(object, asset)| StaticObjectInstance {
                    id: object.id,
                    asset: *asset,
                    translation: object.local_translation,
                    yaw: object.yaw,
                    scale: object.scale,
                })
                .collect();
            let object_page = encoded_page(
                object_key,
                PagePayload::StaticObjects(StaticObjectsPage { instances }),
                0,
            )?;
            hash_page(&mut content_hasher, &object_page);
            pages.push(object_page);
            let mut depended_assets = HashSet::new();
            for (_, asset) in &render_objects {
                if !depended_assets.insert(*asset) {
                    continue;
                }
                for (asset_lod, _) in &variants_by_asset[asset] {
                    dependencies.push(PageDependencyRecord {
                        page: object_key,
                        asset: *asset,
                        asset_lod: *asset_lod,
                    });
                }
            }
        }
        if !gameplay_objects.is_empty() {
            domain_mask |= domain_bit(PageDomain::GameplayObjects);
            let gameplay_key = PageKey {
                space: source_cell.space,
                cell: source_cell.cell,
                domain: PageDomain::GameplayObjects,
                lod: 0,
            };
            let instances = gameplay_objects
                .iter()
                .map(|object| GameplayObjectInstance {
                    id: object.id,
                    definition: object.definition,
                    translation: object.local_translation,
                    yaw: object.yaw,
                    scale: object.scale,
                })
                .collect();
            let gameplay_page = encoded_page(
                gameplay_key,
                PagePayload::GameplayObjects(GameplayObjectsPage { instances }),
                0,
            )?;
            hash_page(&mut content_hasher, &gameplay_page);
            pages.push(gameplay_page);
            let mut depended_definitions = HashSet::new();
            for object in gameplay_objects {
                if depended_definitions.insert(object.definition) {
                    definition_dependencies.push(PageObjectDefinitionRecord {
                        page: gameplay_key,
                        definition: object.definition,
                    });
                }
            }
        }

        let mut minimum_y = source_cell.height;
        let mut maximum_y = source_cell.height;
        for (object, asset) in &render_objects {
            minimum_y = minimum_y.min(object.local_translation[1]);
            maximum_y = maximum_y
                .max(object.local_translation[1] + maximum_height_by_asset[asset] * object.scale);
        }
        maximum_y = maximum_y.max(maximum_ground_cover_y);
        cells.push(RuntimeCellRecord {
            space: source_cell.space,
            cell: source_cell.cell,
            minimum_y,
            maximum_y,
            domain_mask,
            source_revision: source_cell.source_revision,
        });
    }

    let assets = project
        .asset_variants
        .into_iter()
        .map(|variant| {
            let source_asset = assets_by_id[&variant.asset];
            AssetVariantRecord {
                asset: variant.asset,
                lod: variant.lod,
                kind: source_asset.kind.clone(),
                uri: variant.uri,
                bounds: variant.bounds,
                gpu_bytes_estimate: variant.gpu_bytes_estimate,
                shadow_policy: variant.shadow_policy,
                minimum_screen_height: variant.minimum_screen_height,
            }
        })
        .collect();
    let definitions = project
        .definitions
        .iter()
        .map(|definition| RuntimeObjectDefinition {
            id: definition.id,
            key: definition.key.clone(),
            display_name: definition.display_name.clone(),
            visual_asset: definition.visual_asset,
            activation: definition.activation,
        })
        .collect();
    let ground_cover_species = project
        .ground_cover_visuals
        .iter()
        .map(SourceGroundCoverVisualRecord::runtime_species)
        .collect();
    let content_hash = *content_hasher.finalize().as_bytes();
    let generation_id = blake3::Hash::from_bytes(content_hash).to_hex()[..16].to_owned();

    Ok(RuntimeBuild {
        manifest: RuntimeManifest {
            schema_version: RUNTIME_SCHEMA_VERSION,
            generation_id,
            content_hash,
            default_world_space: project.default_world_space,
            world_spaces: project.world_spaces,
        },
        cells,
        pages,
        terrain_surfaces: project.terrain_surfaces,
        terrain_texture_sets: project.terrain_texture_sets,
        terrain_texture_layers: project.terrain_texture_layers,
        terrain_profiles: project.terrain_profiles,
        assets,
        definitions,
        ground_cover_species,
        dependencies,
        definition_dependencies,
        ground_cover_species_dependencies,
        terrain_surface_dependencies,
    })
}

fn validate_terrain_surface(surface: &TerrainSurface) -> Result<()> {
    if surface.key.is_empty()
        || surface.display_name.is_empty()
        || !surface.tile_size.is_finite()
        || surface.tile_size <= 0.0
        || !surface.normal_y_sign.is_finite()
        || surface.normal_y_sign.abs() != 1.0
        || !surface.normal_strength.is_finite()
        || surface.normal_strength < 0.0
        || !surface.roughness_min.is_finite()
        || !surface.roughness_max.is_finite()
        || !(0.0..=1.0).contains(&surface.roughness_min)
        || !(surface.roughness_min..=1.0).contains(&surface.roughness_max)
    {
        bail!("terrain surface {:?} is invalid", surface.id);
    }
    Ok(())
}

fn validate_terrain_profile(profile: &TerrainProfile) -> Result<()> {
    if profile.weight_resolution < 2
        || profile.weight_resolution > MAX_TERRAIN_WEIGHT_RESOLUTION
        || profile
            .macro_scales
            .into_iter()
            .any(|scale| !scale.is_finite() || scale <= 0.0)
        || profile
            .macro_scales
            .windows(2)
            .any(|pair| pair[1] <= pair[0])
        || !profile.macro_contrast.is_finite()
        || profile.macro_contrast < 0.0
        || !profile.macro_albedo_strength.is_finite()
        || !(0.0..=0.5).contains(&profile.macro_albedo_strength)
    {
        bail!("terrain profile for {:?} is invalid", profile.space);
    }
    Ok(())
}

fn validate_terrain_texture_set(texture_set: &TerrainTextureSet) -> Result<()> {
    if texture_set.key.is_empty()
        || [
            &texture_set.base_color_universal_uri,
            &texture_set.normal_material_universal_uri,
            &texture_set.macro_variation_universal_uri,
            &texture_set.base_color_astc_uri,
            &texture_set.normal_material_astc_uri,
            &texture_set.macro_variation_astc_uri,
        ]
        .into_iter()
        .any(|uri| uri.is_empty())
    {
        bail!("terrain texture set {:?} is invalid", texture_set.id);
    }
    Ok(())
}

fn validate_terrain_weight_borders(
    pages_by_cell: &BTreeMap<(WorldSpaceId, CellCoord), Vec<&SourceTerrainCellWeightPageRecord>>,
) -> Result<()> {
    for (&(space, cell), pages) in pages_by_cell {
        for &(dx, dz, current_edge, neighbour_edge) in &[
            (1, 0, WeightEdge::Right, WeightEdge::Left),
            (0, 1, WeightEdge::Top, WeightEdge::Bottom),
        ] {
            let neighbour_cell = CellCoord {
                x: cell.x + dx,
                z: cell.z + dz,
            };
            let Some(neighbour_pages) = pages_by_cell.get(&(space, neighbour_cell)) else {
                continue;
            };
            if pages.len() != neighbour_pages.len() {
                bail!("adjacent terrain cells have incompatible weight pages");
            }
            for (current, neighbour) in pages.iter().zip(neighbour_pages) {
                if current.page != neighbour.page
                    || current.resolution != neighbour.resolution
                    || weight_edge(current, current_edge) != weight_edge(neighbour, neighbour_edge)
                {
                    bail!(
                        "terrain weight-map borders do not match between {:?} and {:?}",
                        cell,
                        neighbour_cell
                    );
                }
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum WeightEdge {
    Left,
    Right,
    Bottom,
    Top,
}

fn weight_edge(page: &SourceTerrainCellWeightPageRecord, edge: WeightEdge) -> Vec<u8> {
    let resolution = usize::from(page.resolution);
    let mut result = Vec::with_capacity(resolution * 4);
    for index in 0..resolution {
        let sample = match edge {
            WeightEdge::Left => index * resolution,
            WeightEdge::Right => index * resolution + resolution - 1,
            WeightEdge::Bottom => index,
            WeightEdge::Top => (resolution - 1) * resolution + index,
        };
        result.extend_from_slice(&page.rgba[sample * 4..sample * 4 + 4]);
    }
    result
}

fn hash_terrain_catalog(hasher: &mut blake3::Hasher, project: &ProjectDocument) {
    for surface in &project.terrain_surfaces {
        hasher.update(&surface.id.0);
        hasher.update(surface.key.as_bytes());
        hasher.update(surface.display_name.as_bytes());
        hasher.update(&[u8::from(surface.anti_tiling)]);
        for value in [
            surface.tile_size,
            surface.normal_y_sign,
            surface.normal_strength,
            surface.roughness_min,
            surface.roughness_max,
        ] {
            hasher.update(&value.to_bits().to_le_bytes());
        }
    }
    for texture_set in &project.terrain_texture_sets {
        hasher.update(&texture_set.id.0);
        hasher.update(texture_set.key.as_bytes());
        for uri in [
            &texture_set.base_color_universal_uri,
            &texture_set.normal_material_universal_uri,
            &texture_set.macro_variation_universal_uri,
            &texture_set.base_color_astc_uri,
            &texture_set.normal_material_astc_uri,
            &texture_set.macro_variation_astc_uri,
        ] {
            hasher.update(uri.as_bytes());
        }
        hasher.update(&texture_set.universal_gpu_bytes.to_le_bytes());
        hasher.update(&texture_set.astc_gpu_bytes.to_le_bytes());
    }
    for layer in &project.terrain_texture_layers {
        hasher.update(&layer.texture_set.0);
        hasher.update(&layer.surface.0);
        hasher.update(&layer.layer.to_le_bytes());
    }
    for profile in &project.terrain_profiles {
        hasher.update(&profile.space.0.to_le_bytes());
        hasher.update(&profile.texture_set.0);
        hasher.update(&profile.weight_resolution.to_le_bytes());
        for value in profile.macro_scales {
            hasher.update(&value.to_bits().to_le_bytes());
        }
        hasher.update(&profile.macro_contrast.to_bits().to_le_bytes());
        hasher.update(&profile.macro_albedo_strength.to_bits().to_le_bytes());
    }
    for slot in &project.terrain_cell_surface_slots {
        hasher.update(&slot.space.0.to_le_bytes());
        hasher.update(&slot.cell.x.to_le_bytes());
        hasher.update(&slot.cell.z.to_le_bytes());
        hasher.update(&[slot.slot]);
        hasher.update(&slot.surface.0);
    }
    for weights in &project.terrain_cell_weight_pages {
        hasher.update(&weights.space.0.to_le_bytes());
        hasher.update(&weights.cell.x.to_le_bytes());
        hasher.update(&weights.cell.z.to_le_bytes());
        hasher.update(&[weights.page]);
        hasher.update(&weights.resolution.to_le_bytes());
        hasher.update(&weights.rgba);
        hasher.update(&weights.source_revision.to_le_bytes());
    }
}

fn validate_ground_cover_visual(visual: &SourceGroundCoverVisualRecord) -> Result<()> {
    if visual.key.is_empty() || visual.display_name.is_empty() || visual.source_revision < 0 {
        bail!("ground-cover visual {:?} is invalid", visual.id);
    }
    let SourceGroundCoverVisualDefinition::CardCluster(card) = &visual.definition;
    if card.built_in_atlas_version != 1 {
        bail!(
            "ground-cover visual {:?} uses unsupported built-in atlas version {}",
            visual.id,
            card.built_in_atlas_version
        );
    }
    if card
        .procedural_recipe
        .is_some_and(|recipe| !recipe.is_valid())
    {
        bail!(
            "ground-cover visual {:?} has an invalid procedural recipe",
            visual.id
        );
    }
    validate_ground_cover_species(&visual.runtime_species())
}

fn validate_ground_cover_species(species: &GroundCoverSpecies) -> Result<()> {
    let valid_color = |color: [f32; 3]| {
        color
            .into_iter()
            .all(|component| component.is_finite() && (0.0..=1.0).contains(&component))
    };
    if species.key.is_empty()
        || !valid_color(species.bottom_color)
        || !valid_color(species.top_color)
        || !species.minimum_card_height.is_finite()
        || species.minimum_card_height <= 0.0
        || !species.maximum_card_height.is_finite()
        || species.maximum_card_height < species.minimum_card_height
        || !species.minimum_card_width.is_finite()
        || species.minimum_card_width <= 0.0
        || !species.maximum_card_width.is_finite()
        || species.maximum_card_width < species.minimum_card_width
        || !species.flattened_card_probability.is_finite()
        || !(0.0..=1.0).contains(&species.flattened_card_probability)
        || !species.maximum_wind_displacement.is_finite()
        || species.maximum_wind_displacement < 0.0
        || !species.artwork.is_valid()
    {
        bail!("ground-cover species {:?} is invalid", species.id);
    }
    Ok(())
}

fn encoded_page(
    key: PageKey,
    payload: PagePayload,
    gpu_bytes_estimate: u64,
) -> Result<EncodedPage> {
    let decoded = encode_page_payload(&payload)?;
    let checksum = *blake3::hash(&decoded).as_bytes();
    let encoded = zstd::stream::encode_all(decoded.as_slice(), 1)?;
    Ok(EncodedPage {
        key,
        codec: PageCodec::Zstd,
        decoded_bytes: decoded.len() as u64,
        gpu_bytes_estimate,
        checksum,
        payload: encoded,
    })
}

fn hash_page(hasher: &mut blake3::Hasher, page: &EncodedPage) {
    hasher.update(&page.key.space.0.to_le_bytes());
    hasher.update(&page.key.cell.x.to_le_bytes());
    hasher.update(&page.key.cell.z.to_le_bytes());
    hasher.update(&(page.key.domain as i64).to_le_bytes());
    hasher.update(&[page.key.lod]);
    hasher.update(&page.checksum);
}

fn publish_runtime_database(runtime_path: &Path, build: &RuntimeBuild) -> Result<()> {
    let file_name = runtime_path
        .file_name()
        .and_then(|name| name.to_str())
        .context("runtime database path must have a UTF-8 file name")?;
    let temporary_path = runtime_path.with_file_name(format!(".{file_name}.building"));
    if temporary_path.exists() {
        fs::remove_file(&temporary_path).with_context(|| {
            format!(
                "failed to remove stale cooker output {}",
                temporary_path.display()
            )
        })?;
    }
    write_runtime_database(&temporary_path, build)?;
    world_db::RuntimeReader::open_immutable(&temporary_path)
        .context("cooked runtime database did not pass validation")?;
    fs::rename(&temporary_path, runtime_path).with_context(|| {
        format!(
            "failed to publish runtime database {}",
            runtime_path.display()
        )
    })?;
    Ok(())
}

fn demo_project_document() -> ProjectDocument {
    let overworld = WorldSpaceRecord {
        id: WorldSpaceId(1),
        name: "demo-overworld".into(),
        cell_size: DEFAULT_CELL_SIZE,
        minimum_y: 0.0,
        maximum_y: 0.0,
    };
    let interior = WorldSpaceRecord {
        id: WorldSpaceId(2),
        name: "demo-interior".into(),
        cell_size: DEFAULT_CELL_SIZE,
        minimum_y: 0.0,
        maximum_y: 0.0,
    };
    let overworld_id = overworld.id;
    let interior_id = interior.id;
    let terrain_texture_set = TerrainTextureSetId(stable_id("temperate-meadow-texture-set"));
    let uncut_grass = TerrainSurfaceId(stable_id("uncut-grass-oilpt20"));
    let dried_grass = TerrainSurfaceId(stable_id("grass-dried-pjwhw0"));
    let tree_asset = AssetId(*blake3::hash(DEMO_TREE_KEY.as_bytes()).as_bytes());
    let meadow_visual = GroundCoverVisualId(stable_id("demo-meadow-grass"));
    let meadow_layer = GroundCoverLayerId(stable_id("demo-meadow-layer"));
    let meadow_preset = GroundCoverPresetId(meadow_layer.0);
    let meadow_region = GroundCoverRegionId(meadow_layer.0);
    let meadow_preset_key = format!(
        "demo-meadow/preset/{}",
        meadow_layer
            .0
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let tree_definition = definition_id("demo-tree");
    let proximity_marker_definition = definition_id("demo-proximity-marker");
    let mut cells = Vec::with_capacity(64 * 64 + 9 * 9);
    let mut objects = Vec::new();
    let mut ground_cover_masks = Vec::new();
    let mut terrain_cell_surface_slots = Vec::with_capacity((64 * 64 * 2) + 9 * 9);
    let mut terrain_cell_weight_pages = Vec::with_capacity(64 * 64);
    for x in -32_i32..32 {
        for z in -32_i32..32 {
            cells.push(SourceCellRecord {
                space: overworld.id,
                cell: CellCoord { x, z },
                height: 0.0,
                source_revision: 1,
            });
            terrain_cell_surface_slots.extend([
                SourceTerrainCellSurfaceSlotRecord {
                    space: overworld.id,
                    cell: CellCoord { x, z },
                    slot: 0,
                    surface: uncut_grass,
                },
                SourceTerrainCellSurfaceSlotRecord {
                    space: overworld.id,
                    cell: CellCoord { x, z },
                    slot: 1,
                    surface: dried_grass,
                },
            ]);
            terrain_cell_weight_pages.push(SourceTerrainCellWeightPageRecord {
                space: overworld.id,
                cell: CellCoord { x, z },
                page: 0,
                resolution: DEMO_TERRAIN_WEIGHT_RESOLUTION,
                rgba: demo_terrain_weights(CellCoord { x, z }),
                source_revision: 1,
            });
            if DEMO_MEADOW_CELL_RANGE.contains(&x) && DEMO_MEADOW_CELL_RANGE.contains(&z) {
                ground_cover_masks.push(SourceGroundCoverCellMaskRecord {
                    region: meadow_region,
                    space: overworld.id,
                    cell: CellCoord { x, z },
                    resolution: 16,
                    coverage: vec![255; 16 * 16],
                    source_revision: 1,
                });
            }
            let lod_test_line = x == z && (-3..=-1).contains(&x);
            if lod_test_line || (x.rem_euclid(5) == 2 && z.rem_euclid(5) == 2) {
                let hash = blake3::hash(format!("demo-tree:{x}:{z}").as_bytes());
                let mut object_id = [0; 16];
                object_id.copy_from_slice(&hash.as_bytes()[..16]);
                objects.push(SourceObjectRecord {
                    id: StableObjectId(object_id),
                    space: overworld.id,
                    owner_cell: CellCoord { x, z },
                    definition: tree_definition,
                    local_translation: [DEFAULT_CELL_SIZE * 0.5, 0.0, DEFAULT_CELL_SIZE * 0.5],
                    yaw: ((x * 17 + z * 31).rem_euclid(360) as f32).to_radians(),
                    scale: 0.55,
                    source_revision: 1,
                });
            }
        }
    }
    for x in -4_i32..=4 {
        for z in -4_i32..=4 {
            cells.push(SourceCellRecord {
                space: interior.id,
                cell: CellCoord { x, z },
                height: 0.0,
                source_revision: 1,
            });
            terrain_cell_surface_slots.push(SourceTerrainCellSurfaceSlotRecord {
                space: interior.id,
                cell: CellCoord { x, z },
                slot: 0,
                surface: dried_grass,
            });
        }
    }
    objects.push(SourceObjectRecord {
        id: StableObjectId(proximity_marker_definition.0),
        space: interior.id,
        owner_cell: CellCoord::ZERO,
        definition: proximity_marker_definition,
        local_translation: [DEFAULT_CELL_SIZE * 0.5, 0.0, DEFAULT_CELL_SIZE * 0.5],
        yaw: 0.0,
        scale: 1.0,
        source_revision: 1,
    });

    ProjectDocument {
        default_world_space: overworld.id,
        world_spaces: vec![overworld, interior],
        cells,
        terrain_surfaces: vec![
            TerrainSurface {
                id: uncut_grass,
                key: "uncut-grass-oilpt20".into(),
                display_name: "Uncut grass".into(),
                // The legacy prototype meadow applied a 1.6x area-wide
                // appearance multiplier to this surface's 2 m source scale.
                tile_size: 3.2,
                anti_tiling: true,
                normal_y_sign: 1.0,
                normal_strength: 0.48,
                roughness_min: 0.82,
                roughness_max: 0.98,
            },
            TerrainSurface {
                id: dried_grass,
                key: "grass-dried-pjwhw0".into(),
                display_name: "Dried grass".into(),
                tile_size: 1.6,
                anti_tiling: true,
                normal_y_sign: 1.0,
                normal_strength: 0.42,
                roughness_min: 0.86,
                roughness_max: 1.0,
            },
        ],
        terrain_texture_sets: vec![TerrainTextureSet {
            id: terrain_texture_set,
            key: "temperate-meadow".into(),
            base_color_universal_uri: format!(
                "{DEMO_TERRAIN_TEXTURE_ROOT}/universal/base_color_array.ktx2"
            ),
            normal_material_universal_uri: format!(
                "{DEMO_TERRAIN_TEXTURE_ROOT}/universal/normal_material_array.ktx2"
            ),
            macro_variation_universal_uri: format!(
                "{DEMO_TERRAIN_TEXTURE_ROOT}/universal/macro_variation.ktx2"
            ),
            base_color_astc_uri: format!("{DEMO_TERRAIN_TEXTURE_ROOT}/astc/base_color_array.ktx2"),
            normal_material_astc_uri: format!(
                "{DEMO_TERRAIN_TEXTURE_ROOT}/astc/normal_material_array.ktx2"
            ),
            macro_variation_astc_uri: format!(
                "{DEMO_TERRAIN_TEXTURE_ROOT}/astc/macro_variation.ktx2"
            ),
            universal_gpu_bytes: 6_554_120,
            astc_gpu_bytes: 3_846_512,
        }],
        terrain_texture_layers: vec![
            TerrainTextureLayer {
                texture_set: terrain_texture_set,
                surface: uncut_grass,
                layer: 0,
            },
            TerrainTextureLayer {
                texture_set: terrain_texture_set,
                surface: dried_grass,
                layer: 1,
            },
        ],
        terrain_profiles: vec![
            TerrainProfile {
                space: overworld_id,
                texture_set: terrain_texture_set,
                weight_resolution: DEMO_TERRAIN_WEIGHT_RESOLUTION,
                macro_scales: [7.7, 31.5, 235.0],
                macro_contrast: 2.5,
                macro_albedo_strength: 0.395,
            },
            TerrainProfile {
                space: interior_id,
                texture_set: terrain_texture_set,
                weight_resolution: DEMO_TERRAIN_WEIGHT_RESOLUTION,
                macro_scales: [7.7, 31.5, 235.0],
                macro_contrast: 2.5,
                macro_albedo_strength: 0.395,
            },
        ],
        terrain_cell_surface_slots,
        terrain_cell_weight_pages,
        ground_cover_visuals: vec![SourceGroundCoverVisualRecord {
            id: meadow_visual,
            key: "meadow-long-grass".into(),
            display_name: "meadow-long-grass".into(),
            source_revision: 1,
            definition: SourceGroundCoverVisualDefinition::CardCluster(
                SourceGroundCoverCardVisualRecord::original_meadow_v1(),
            ),
        }],
        ground_cover_presets: vec![SourceGroundCoverPresetRecord {
            id: meadow_preset,
            key: meadow_preset_key,
            display_name: "demo-meadow".into(),
            enabled: true,
            visual: meadow_visual,
            density_per_square_meter: 5.0,
            seed: 0x6f53_91d2,
            source_revision: 1,
        }],
        ground_cover_layers: vec![SourceGroundCoverLayerRecord {
            id: meadow_layer,
            space: overworld_id,
            key: "demo-meadow".into(),
            display_name: "demo-meadow".into(),
            enabled: true,
            sort_order: 0,
            source_revision: 1,
        }],
        ground_cover_regions: vec![SourceGroundCoverRegionRecord {
            id: meadow_region,
            layer: meadow_layer,
            space: overworld_id,
            preset: meadow_preset,
            display_name: "Existing coverage".into(),
            enabled: true,
            density_multiplier: 1.0,
            source_revision: 1,
        }],
        ground_cover_masks,
        assets: vec![SourceAssetRecord {
            id: tree_asset,
            key: DEMO_TREE_KEY.into(),
            kind: "gltf-scene".into(),
            source_uri: DEMO_TREE_SOURCE_URI.into(),
        }],
        asset_variants: DEMO_TREE_LOD_URIS
            .iter()
            .enumerate()
            .map(|(lod, uri)| SourceAssetVariantRecord {
                asset: tree_asset,
                lod: lod as u8,
                uri: (*uri).into(),
                bounds: DEMO_TREE_BOUNDS,
                gpu_bytes_estimate: DEMO_TREE_GPU_BYTES[lod],
                shadow_policy: 1,
                minimum_screen_height: DEMO_TREE_MINIMUM_SCREEN_HEIGHTS[lod],
            })
            .collect(),
        definitions: vec![
            SourceObjectDefinitionRecord {
                id: tree_definition,
                key: "demo-tree".into(),
                display_name: "Demo tree".into(),
                visual_asset: Some(tree_asset),
                activation: ObjectActivationPolicy::RenderOnly,
            },
            SourceObjectDefinitionRecord {
                id: proximity_marker_definition,
                key: "demo-proximity-marker".into(),
                display_name: "Proximity gameplay marker".into(),
                visual_asset: None,
                activation: ObjectActivationPolicy::Proximity,
            },
        ],
        objects,
    }
}

fn demo_terrain_weights(cell: CellCoord) -> Vec<u8> {
    let resolution = usize::from(DEMO_TERRAIN_WEIGHT_RESOLUTION);
    let intervals = (resolution - 1) as f32;
    let mut rgba = Vec::with_capacity(resolution * resolution * 4);
    for z in 0..resolution {
        for x in 0..resolution {
            let world_x =
                cell.x as f32 * DEFAULT_CELL_SIZE + x as f32 * DEFAULT_CELL_SIZE / intervals;
            let world_z =
                cell.z as f32 * DEFAULT_CELL_SIZE + z as f32 * DEFAULT_CELL_SIZE / intervals;
            // This is the useful two-compatible-surface path from the legacy
            // meadow compiler: warped coherent noise at broad, medium, small,
            // and mottling scales, followed by a calibrated continuous mix.
            // It produces irregular internal structure rather than one huge
            // analytic gradient between two regions.
            let signal = demo_terrain_mix_signal(Vec2::new(world_x, world_z));
            let dried =
                (demo_terrain_mix_offset() + signal * demo_terrain_mix_amplitude()).clamp(0.0, 1.0);
            let dried_byte = (dried * 255.0).round() as u8;
            rgba.extend_from_slice(&[255 - dried_byte, dried_byte, 0, 0]);
        }
    }
    rgba
}

const DEMO_TERRAIN_SEED: u32 = 1_863_996_882;
const DEMO_TERRAIN_MIX_SCALE: f32 = 8.7;
const DEMO_TERRAIN_DETAIL_SIZE: f32 = 0.5;
const DEMO_TERRAIN_FINE_VARIATION: f32 = 0.82;
const DEMO_TERRAIN_VARIANT_MOTTLING: f32 = 0.71;
const DEMO_TERRAIN_BLEND_SOFTNESS: f32 = 0.14;
const DEMO_DRY_COVERAGE: f32 = 0.32 / (0.62 + 0.32);

fn demo_terrain_mix_signal(world: Vec2) -> f32 {
    let region_scale = DEMO_TERRAIN_MIX_SCALE;
    let broad_offset = terrain_material_offset(DEMO_TERRAIN_SEED, 1);
    let warp = Vec2::new(
        terrain_fbm(
            world / (region_scale * 3.2) + broad_offset,
            DEMO_TERRAIN_SEED ^ 0x3c6e_f372,
        ),
        terrain_fbm(
            world / (region_scale * 3.2) + broad_offset + Vec2::new(17.3, -29.1),
            DEMO_TERRAIN_SEED ^ 0xbb67_ae85,
        ),
    ) * (region_scale * 0.58);
    let warped_world = world + warp;
    let broad = terrain_patch_signal(
        warped_world / region_scale + broad_offset,
        DEMO_TERRAIN_SEED ^ 0x679d_443f,
    );
    let medium = terrain_patch_signal(
        warped_world / (region_scale * 0.55 * DEMO_TERRAIN_DETAIL_SIZE)
            + terrain_material_offset(DEMO_TERRAIN_SEED, 2),
        DEMO_TERRAIN_SEED ^ 0x243f_6a88,
    );
    let small = terrain_patch_signal(
        (world + warp * 0.35) / (region_scale * 0.28 * DEMO_TERRAIN_DETAIL_SIZE)
            + terrain_material_offset(DEMO_TERRAIN_SEED, 3),
        DEMO_TERRAIN_SEED ^ 0xb7e1_5163,
    );
    let hole_field = terrain_patch_signal(
        (world - warp * 0.2) / (region_scale * 0.36 * DEMO_TERRAIN_DETAIL_SIZE)
            + terrain_material_offset(DEMO_TERRAIN_SEED, 4),
        DEMO_TERRAIN_SEED ^ 0x1319_8a2e,
    );
    let detail = smootherstep(DEMO_TERRAIN_FINE_VARIATION);
    let medium_strength = 0.65 * detail;
    let small_strength = 0.35 * detail;
    let variation = (broad + medium * medium_strength + small * small_strength)
        / (1.0 + medium_strength + small_strength);
    let green_mottling =
        (smoothstep(0.05, 0.75, hole_field) - 0.35) * DEMO_TERRAIN_VARIANT_MOTTLING * 0.42;
    // Flat terrain has the same neutral feature response as the legacy
    // sampler: moisture 0.65, slope 0.0.
    let flat_terrain_bias = (0.65 - 0.5) * -0.55 * 0.2 + (0.0 - 0.35) * 0.2 * 0.14;
    variation - green_mottling + flat_terrain_bias
}

fn demo_terrain_mix_amplitude() -> f32 {
    const HARD_AMPLITUDE: f32 = 3.0;
    const SOFT_AMPLITUDE: f32 = 0.10;
    HARD_AMPLITUDE
        * (SOFT_AMPLITUDE / HARD_AMPLITUDE).powf(DEMO_TERRAIN_BLEND_SOFTNESS.clamp(0.0, 1.0))
}

fn demo_terrain_mix_offset() -> f32 {
    static OFFSET: OnceLock<f32> = OnceLock::new();
    *OFFSET.get_or_init(|| {
        // The original compiler calibrates coverage over the complete area.
        // A deterministic stratified grid is sufficient for this flat demo
        // and avoids making each cell choose its own incompatible threshold.
        const SIDE: usize = 192;
        let extent = DEFAULT_CELL_SIZE * 60.0;
        let minimum = -extent * 0.5;
        let signals = (0..SIDE)
            .flat_map(|z| {
                (0..SIDE).map(move |x| {
                    let world = Vec2::new(
                        minimum + (x as f32 + 0.5) / SIDE as f32 * extent,
                        minimum + (z as f32 + 0.5) / SIDE as f32 * extent,
                    );
                    demo_terrain_mix_signal(world)
                })
            })
            .collect::<Vec<_>>();
        let amplitude = demo_terrain_mix_amplitude();
        let mut lower = -(amplitude * 2.0 + 1.0);
        let mut upper = amplitude * 2.0 + 1.0;
        for _ in 0..24 {
            let offset = (lower + upper) * 0.5;
            let average = signals
                .iter()
                .map(|signal| (offset + signal * amplitude).clamp(0.0, 1.0))
                .sum::<f32>()
                / signals.len() as f32;
            if average < DEMO_DRY_COVERAGE {
                lower = offset;
            } else {
                upper = offset;
            }
        }
        (lower + upper) * 0.5
    })
}

fn terrain_patch_signal(mut point: Vec2, seed: u32) -> f32 {
    let mut signal = terrain_value_noise(point, seed) * 0.7;
    point = Vec2::new(
        point.x * 1.71 - point.y * 1.04 + 7.3,
        point.x * 1.04 + point.y * 1.71 - 11.9,
    );
    signal += terrain_value_noise(point, seed ^ 0x510e_527f) * 0.22;
    point = Vec2::new(
        point.x * 1.53 + point.y * 1.29 - 19.7,
        -point.x * 1.29 + point.y * 1.53 + 3.1,
    );
    signal + terrain_value_noise(point, seed ^ 0x9b05_688c) * 0.08
}

fn terrain_material_offset(seed: u32, index: usize) -> Vec2 {
    Vec2::new(
        terrain_hash01(index as i32, 17, seed ^ 0xa409_3822) * 97.0,
        terrain_hash01(index as i32, 53, seed ^ 0x299f_31d0) * 97.0,
    )
}

fn terrain_value_noise(point: Vec2, seed: u32) -> f32 {
    let cell = point.floor();
    let local = point - cell;
    let fade = Vec2::new(smootherstep(local.x), smootherstep(local.y));
    let x = cell.x as i32;
    let z = cell.y as i32;
    let lower = mix(
        terrain_hash_signed(x, z, seed),
        terrain_hash_signed(x + 1, z, seed),
        fade.x,
    );
    let upper = mix(
        terrain_hash_signed(x, z + 1, seed),
        terrain_hash_signed(x + 1, z + 1, seed),
        fade.x,
    );
    mix(lower, upper, fade.y)
}

fn terrain_fbm(mut point: Vec2, seed: u32) -> f32 {
    let mut sum = 0.0;
    let mut amplitude = 0.55;
    let mut normalization = 0.0;
    for octave in 0..5_u32 {
        sum += terrain_value_noise(point, seed ^ octave.wrapping_mul(0x9e37_79b9)) * amplitude;
        normalization += amplitude;
        point = Vec2::new(
            point.x * 1.76 - point.y * 1.13 + 13.1,
            point.x * 1.13 + point.y * 1.76 - 7.9,
        );
        amplitude *= 0.5;
    }
    sum / normalization
}

fn terrain_hash_signed(x: i32, z: i32, seed: u32) -> f32 {
    terrain_hash01(x, z, seed) * 2.0 - 1.0
}

fn terrain_hash01(x: i32, z: i32, seed: u32) -> f32 {
    let mut value =
        seed ^ (x as u32).wrapping_mul(0x9e37_79b1) ^ (z as u32).wrapping_mul(0x85eb_ca77);
    value ^= value >> 16;
    value = value.wrapping_mul(0x7feb_352d);
    value ^= value >> 15;
    value = value.wrapping_mul(0x846c_a68b);
    value ^= value >> 16;
    value as f32 / u32::MAX as f32
}

fn smootherstep(value: f32) -> f32 {
    let value = value.clamp(0.0, 1.0);
    value * value * value * (value * (value * 6.0 - 15.0) + 10.0)
}

fn mix(a: f32, b: f32, amount: f32) -> f32 {
    a + (b - a) * amount
}

fn smoothstep(minimum: f32, maximum: f32, value: f32) -> f32 {
    let normalized = ((value - minimum) / (maximum - minimum)).clamp(0.0, 1.0);
    normalized * normalized * (3.0 - 2.0 * normalized)
}

fn definition_id(key: &str) -> ObjectDefinitionId {
    ObjectDefinitionId(stable_id(key))
}

fn stable_id(key: &str) -> [u8; 16] {
    let hash = blake3::hash(key.as_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&hash.as_bytes()[..16]);
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEMO_TREE_PACK_MANIFEST: &str =
        include_str!("../../../assets/packs/forest_tree_starter_kit/tree_07.toml");

    #[test]
    fn demo_cook_is_large_logically_but_page_addressable() {
        let project = demo_project_document();
        assert_eq!(project.ground_cover_visuals.len(), 1);
        assert_eq!(project.ground_cover_presets.len(), 1);
        assert_eq!(project.ground_cover_layers.len(), 1);
        assert_eq!(project.ground_cover_regions.len(), 1);
        let build = build_runtime(project).unwrap();
        assert_eq!(build.manifest.world_spaces.len(), 2);
        assert_eq!(build.manifest.default_world_space, WorldSpaceId(1));
        assert_eq!(build.cells.len(), 64 * 64 + 9 * 9);
        assert!(build.pages.len() > build.cells.len());
        assert_eq!(build.assets.len(), 4);
        assert_eq!(
            build
                .assets
                .iter()
                .map(|asset| asset.lod)
                .collect::<Vec<_>>(),
            vec![0, 1, 2, 3]
        );
        let static_pages = build
            .pages
            .iter()
            .filter(|page| page.key.domain == PageDomain::StaticObjects)
            .count();
        assert_eq!(build.dependencies.len(), static_pages * 4);
        assert!(
            build.cells.iter().any(|cell| cell.maximum_y > 8.0),
            "static model bounds did not expand cell visibility bounds"
        );
        assert_eq!(build.definitions.len(), 2);
        let ground_cover_pages = build
            .pages
            .iter()
            .filter(|page| page.key.domain == PageDomain::GroundCover)
            .collect::<Vec<_>>();
        assert_eq!(ground_cover_pages.len(), 60 * 60);
        assert_eq!(ground_cover_pages[0].key.cell, CellCoord { x: -30, z: -30 });
        assert_eq!(
            ground_cover_pages[0]
                .checksum
                .iter()
                .map(|byte| format!("{byte:02X}"))
                .collect::<String>(),
            "8057EB7845C9E4FBDA75D0D9A7E376D25C5BEB03ED5DF8B647DB15F15882294E",
            "the source-model conversion must preserve the optimized runtime page payload"
        );
        assert_eq!(build.ground_cover_species.len(), 1);
        assert_eq!(
            build.ground_cover_species_dependencies.len(),
            ground_cover_pages.len()
        );
        let decoded = ground_cover_pages[0].clone().decode().unwrap();
        let PagePayload::GroundCover(page) = decoded.payload else {
            panic!("ground-cover page decoded to the wrong domain");
        };
        assert_eq!(page.clusters.len(), 16 * 16);
        assert!(page.clusters.iter().all(|cluster| {
            cluster.density_per_square_meter > 0.0 && cluster.half_extents[0] > 0.0
        }));
        assert_eq!(
            build
                .pages
                .iter()
                .filter(|page| page.key.domain == PageDomain::GameplayObjects)
                .count(),
            1
        );
        assert!(
            build
                .pages
                .iter()
                .all(|page| page.decoded_bytes > 0 && !page.payload.is_empty())
        );
    }

    #[test]
    fn demo_tree_lod_uris_match_the_tracked_pack_contract() {
        for uri in DEMO_TREE_LOD_URIS {
            let expected = format!("runtime_uri = \"{uri}\"");
            assert!(
                DEMO_TREE_PACK_MANIFEST
                    .lines()
                    .any(|line| line.trim() == expected),
                "the demo URI {uri} and tree pack manifest must change together"
            );
        }
    }

    #[test]
    fn demo_terrain_blend_contains_green_dry_and_transition_regions() {
        let dried_weights = (-4..=4)
            .flat_map(|cell_x| {
                (-4..=4).flat_map(move |cell_z| {
                    demo_terrain_weights(CellCoord {
                        x: cell_x,
                        z: cell_z,
                    })
                    .into_iter()
                    .skip(1)
                    .step_by(4)
                })
            })
            .collect::<Vec<_>>();

        assert!(dried_weights.iter().any(|weight| *weight <= 8));
        assert!(dried_weights.iter().any(|weight| *weight >= 247));
        assert!(
            dried_weights
                .iter()
                .any(|weight| (32..=223).contains(weight))
        );
    }

    #[test]
    fn terrain_profile_validation_matches_sqlite_bounds() {
        let mut profile = TerrainProfile {
            space: WorldSpaceId(1),
            texture_set: TerrainTextureSetId([1; 16]),
            weight_resolution: MAX_TERRAIN_WEIGHT_RESOLUTION,
            macro_scales: [7.7, 31.5, 235.0],
            macro_contrast: 0.0,
            macro_albedo_strength: 0.5,
        };
        assert!(validate_terrain_profile(&profile).is_ok());

        profile.weight_resolution = MAX_TERRAIN_WEIGHT_RESOLUTION + 1;
        assert!(validate_terrain_profile(&profile).is_err());

        profile.weight_resolution = MAX_TERRAIN_WEIGHT_RESOLUTION;
        profile.macro_albedo_strength = 0.500_1;
        assert!(validate_terrain_profile(&profile).is_err());
    }
}
