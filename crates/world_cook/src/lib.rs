mod environment_cook;
use environment_cook::{
    CookedEnvironment, TerrainSlot, TerrainWeights, compile_environment, demo_environment,
};

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
    sync::OnceLock,
};

use anyhow::{Context, Result, bail};
use glam::Vec2;
use world::{
    AssetId, CellCoord, DEFAULT_CELL_SIZE, GameplayObjectInstance, GameplayObjectsPage,
    MAX_TERRAIN_HEIGHTFIELD_RESOLUTION, MAX_TERRAIN_SURFACES_PER_CELL, MAX_TERRAIN_WEIGHT_PAGES,
    MAX_TERRAIN_WEIGHT_RESOLUTION, ObjectActivationPolicy, ObjectDefinitionId, PageCodec,
    PageDomain, PageKey, PagePayload, RUNTIME_SCHEMA_VERSION, StableObjectId, StaticObjectInstance,
    StaticObjectsPage, TerrainHeightfieldPage, TerrainProfile, TerrainSurface, TerrainSurfaceId,
    TerrainTextureLayer, TerrainTextureSet, TerrainTextureSetId, TerrainWeightPage, WorldSpaceId,
    encode_page_payload,
};
use world_db::{
    AssetVariantRecord, EncodedPage, PageDependencyRecord, PageObjectDefinitionRecord,
    PageTerrainSurfaceRecord, ProjectDocument, RuntimeBuild, RuntimeCellRecord, RuntimeManifest,
    RuntimeObjectDefinition, SourceAssetRecord, SourceAssetVariantRecord, SourceCellRecord,
    SourceObjectDefinitionRecord, SourceObjectRecord, SourceTerrainCellHeightfieldRecord,
    WorldSpaceRecord, domain_bit, read_project_database, write_project_database,
    write_runtime_database,
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
const DEMO_WORLD_CELL_RANGE: std::ops::Range<i32> = -8..8;
// Half-metre source masks and endpoint-inclusive compiled ground weights.
const DEMO_TERRAIN_WEIGHT_RESOLUTION: u16 = 65;
const DEMO_TERRAIN_HEIGHTFIELD_RESOLUTION: u16 = 33;
const DEMO_TERRAIN_TEXTURE_ROOT: &str = "local/terrain/temperate_meadow/runtime";
const DEMO_TERRAIN_MINIMUM_HEIGHT: f32 = -4.0;
const DEMO_TERRAIN_MAXIMUM_HEIGHT: f32 = 4.0;

mod road_demo;
pub use road_demo::create_road_demo_project;

/// Initialize a fresh editable world at the grid used by the road/layer authoring tools.
/// Existing projects are never replaced by initialization or cooking.
pub fn create_world_project(path: &Path) -> Result<()> {
    let mut document = road_demo::document();
    for space in &mut document.world_spaces {
        if let Some(name) = space.name.strip_prefix("demo-") {
            space.name = name.to_owned();
        }
    }
    write_project_database(path, &document)
        .with_context(|| format!("failed to create authoring world at {}", path.display()))
}

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
        .terrain_cell_heightfields
        .sort_by_key(|heightfield| (heightfield.space, heightfield.cell));
    project.assets.sort_by_key(|asset| asset.id.0);
    project
        .asset_variants
        .sort_by_key(|variant| (variant.asset.0, variant.lod));
    project.definitions.sort_by_key(|definition| definition.id);
    project.objects.sort_by_key(|object| object.id.0);

    let environment = compile_environment(&project)?;

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
    let mut terrain_slots_by_cell: BTreeMap<(WorldSpaceId, CellCoord), Vec<&TerrainSlot>> =
        BTreeMap::new();
    for slot in &environment.slots {
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
    let mut terrain_weights_by_cell: BTreeMap<(WorldSpaceId, CellCoord), Vec<&TerrainWeights>> =
        BTreeMap::new();
    for weights in &environment.weights {
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
    let mut terrain_heightfields_by_cell = BTreeMap::new();
    for heightfield in &project.terrain_cell_heightfields {
        if !source_cells.contains(&(heightfield.space, heightfield.cell)) {
            bail!(
                "terrain heightfield references missing cell {:?}",
                heightfield.cell
            );
        }
        let resolution = usize::from(heightfield.resolution);
        if !(2..=usize::from(MAX_TERRAIN_HEIGHTFIELD_RESOLUTION)).contains(&resolution)
            || heightfield.heights.len() != resolution * resolution
            || !heightfield.heights.iter().all(|height| height.is_finite())
            || heightfield.source_revision < 0
        {
            bail!(
                "terrain heightfield {:?} has invalid dimensions or samples",
                heightfield.cell
            );
        }
        let space = spaces_by_id[&heightfield.space];
        if heightfield
            .heights
            .iter()
            .any(|height| *height < space.minimum_y || *height > space.maximum_y)
        {
            bail!(
                "terrain heightfield {:?} exceeds world-space height bounds",
                heightfield.cell
            );
        }
        if terrain_heightfields_by_cell
            .insert((heightfield.space, heightfield.cell), heightfield)
            .is_some()
        {
            bail!("terrain cells may have only one heightfield");
        }
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
    validate_terrain_heightfield_borders(&terrain_heightfields_by_cell)?;
    let mut vegetation_pages_by_cell = HashMap::new();
    if !environment.vegetation.is_empty() {
        let catalog = environment
            .catalog
            .as_ref()
            .context("vegetation field pages require a vegetation catalog")?;
        catalog.validate()?;
        for page in &environment.vegetation {
            if !source_cells.contains(&(page.space, page.cell)) {
                bail!("vegetation page references missing cell {:?}", page.cell);
            }
            if page.source_revision < 0 {
                bail!("vegetation page {:?} has a negative revision", page.cell);
            }
            page.data.validate(catalog)?;
            if vegetation_pages_by_cell
                .insert((page.space, page.cell), page)
                .is_some()
            {
                bail!("vegetation cells may have only one field page");
            }
        }
    } else if let Some(catalog) = &environment.catalog {
        catalog.validate()?;
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
    hash_terrain_catalog(&mut content_hasher, &project, &environment);
    content_hasher.update(&environment.fingerprint);
    if let Some(catalog) = &environment.catalog {
        let encoded = bincode::serde::encode_to_vec(
            catalog,
            bincode::config::standard()
                .with_little_endian()
                .with_fixed_int_encoding(),
        )?;
        content_hasher.update(&encoded);
    }
    for heightfield in &project.terrain_cell_heightfields {
        content_hasher.update(&heightfield.space.0.to_le_bytes());
        content_hasher.update(&heightfield.cell.x.to_le_bytes());
        content_hasher.update(&heightfield.cell.z.to_le_bytes());
        content_hasher.update(&heightfield.resolution.to_le_bytes());
        for height in &heightfield.heights {
            content_hasher.update(&height.to_bits().to_le_bytes());
        }
        content_hasher.update(&heightfield.source_revision.to_le_bytes());
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
        let surfaces = terrain_slots.iter().map(|slot| slot.surface).collect();
        let weight_pages = terrain_weights
            .iter()
            .map(|weights| TerrainWeightPage {
                resolution: weights.resolution,
                rgba: weights.rgba.clone(),
            })
            .collect();
        let heightfield = environment
            .terrain
            .get(&(source_cell.space, source_cell.cell))
            .context("environment compiler omitted terrain")?
            .clone();
        let terrain_height_bounds = heightfield.height_bounds();
        let heightfield_gpu_bytes = estimate_heightfield_gpu_bytes(heightfield.resolution);
        let terrain_payload = PagePayload::TerrainHeightfield(TerrainHeightfieldPage {
            heightfield,
            surfaces,
            weight_pages,
        });
        let terrain_page = encoded_page(
            terrain_key,
            terrain_payload,
            terrain_weights
                .iter()
                .map(|weights| weights.rgba.len() as u64)
                .sum::<u64>()
                + heightfield_gpu_bytes,
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
        if let Some(source_page) =
            vegetation_pages_by_cell.get(&(source_cell.space, source_cell.cell))
        {
            let key = PageKey {
                space: source_cell.space,
                cell: source_cell.cell,
                domain: PageDomain::Vegetation,
                lod: 0,
            };
            let gpu_bytes_estimate = source_page
                .data
                .fields
                .iter()
                .map(|field| field.coverage.len() as u64)
                .sum();
            let page = encoded_page(
                key,
                PagePayload::Vegetation(source_page.data.clone()),
                gpu_bytes_estimate,
            )?;
            hash_page(&mut content_hasher, &page);
            pages.push(page);
            domain_mask |= domain_bit(PageDomain::Vegetation);
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

        let mut minimum_y = terrain_height_bounds[0];
        let mut maximum_y = terrain_height_bounds[1];
        for (object, asset) in &render_objects {
            minimum_y = minimum_y.min(object.local_translation[1]);
            maximum_y = maximum_y
                .max(object.local_translation[1] + maximum_height_by_asset[asset] * object.scale);
        }
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
    let content_hash = *content_hasher.finalize().as_bytes();
    let generation_id = blake3::Hash::from_bytes(content_hash).to_hex()[..16].to_owned();

    Ok(RuntimeBuild {
        manifest: RuntimeManifest {
            schema_version: RUNTIME_SCHEMA_VERSION,
            generation_id,
            content_hash,
            default_world_space: project.default_world_space,
            world_spaces: project.world_spaces,
            vegetation_catalog: environment.catalog,
        },
        cells,
        pages,
        terrain_surfaces: project.terrain_surfaces,
        terrain_texture_sets: project.terrain_texture_sets,
        terrain_texture_layers: project.terrain_texture_layers,
        terrain_profiles: project.terrain_profiles,
        assets,
        definitions,
        dependencies,
        definition_dependencies,
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

#[derive(Clone, Copy)]
enum WeightEdge {
    Left,
    Right,
    Bottom,
    Top,
}

fn validate_terrain_heightfield_borders(
    pages_by_cell: &BTreeMap<(WorldSpaceId, CellCoord), &SourceTerrainCellHeightfieldRecord>,
) -> Result<()> {
    for (&(space, cell), page) in pages_by_cell {
        for &(dx, dz, current_edge, neighbour_edge) in &[
            (1, 0, WeightEdge::Right, WeightEdge::Left),
            (0, 1, WeightEdge::Top, WeightEdge::Bottom),
        ] {
            let neighbour_cell = CellCoord {
                x: cell.x + dx,
                z: cell.z + dz,
            };
            let Some(neighbour) = pages_by_cell.get(&(space, neighbour_cell)) else {
                continue;
            };
            if page.resolution != neighbour.resolution
                || height_edge(page, current_edge)
                    .iter()
                    .zip(height_edge(neighbour, neighbour_edge))
                    .any(|(left, right)| (*left - right).abs() > 1.0e-5)
            {
                bail!(
                    "terrain heightfield borders do not match between {:?} and {:?}",
                    cell,
                    neighbour_cell
                );
            }
        }
    }
    Ok(())
}

fn height_edge(page: &SourceTerrainCellHeightfieldRecord, edge: WeightEdge) -> Vec<f32> {
    let resolution = usize::from(page.resolution);
    (0..resolution)
        .map(|index| {
            let sample = match edge {
                WeightEdge::Left => index * resolution,
                WeightEdge::Right => index * resolution + resolution - 1,
                WeightEdge::Bottom => index,
                WeightEdge::Top => (resolution - 1) * resolution + index,
            };
            page.heights[sample]
        })
        .collect()
}

fn hash_terrain_catalog(
    hasher: &mut blake3::Hasher,
    project: &ProjectDocument,
    environment: &CookedEnvironment,
) {
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
    for slot in &environment.slots {
        hasher.update(&slot.space.0.to_le_bytes());
        hasher.update(&slot.cell.x.to_le_bytes());
        hasher.update(&slot.cell.z.to_le_bytes());
        hasher.update(&[slot.slot]);
        hasher.update(&slot.surface.0);
    }
    for weights in &environment.weights {
        hasher.update(&weights.space.0.to_le_bytes());
        hasher.update(&weights.cell.x.to_le_bytes());
        hasher.update(&weights.cell.z.to_le_bytes());
        hasher.update(&[weights.page]);
        hasher.update(&weights.resolution.to_le_bytes());
        hasher.update(&weights.rgba);
        hasher.update(&weights.source_revision.to_le_bytes());
    }
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

fn estimate_heightfield_gpu_bytes(resolution: u16) -> u64 {
    let resolution = u64::from(resolution);
    let vertex_bytes = resolution * resolution * 48;
    let index_bytes = (resolution - 1) * (resolution - 1) * 6 * size_of::<u32>() as u64;
    vertex_bytes + index_bytes
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
        minimum_y: DEMO_TERRAIN_MINIMUM_HEIGHT,
        maximum_y: DEMO_TERRAIN_MAXIMUM_HEIGHT,
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
    let tree_definition = definition_id("demo-tree");
    let proximity_marker_definition = definition_id("demo-proximity-marker");
    let mut cells = Vec::new();
    let mut objects = Vec::new();
    let mut terrain_cell_heightfields = Vec::new();
    for x in DEMO_WORLD_CELL_RANGE {
        for z in DEMO_WORLD_CELL_RANGE {
            cells.push(SourceCellRecord {
                space: overworld.id,
                cell: CellCoord { x, z },
                height: 0.0,
                source_revision: 1,
            });
            terrain_cell_heightfields.push(SourceTerrainCellHeightfieldRecord {
                space: overworld.id,
                cell: CellCoord { x, z },
                resolution: DEMO_TERRAIN_HEIGHTFIELD_RESOLUTION,
                heights: demo_terrain_heights(CellCoord { x, z }),
                source_revision: 1,
            });
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
                    local_translation: [
                        DEFAULT_CELL_SIZE * 0.5,
                        demo_terrain_height(Vec2::new(
                            (x as f32 + 0.5) * DEFAULT_CELL_SIZE,
                            (z as f32 + 0.5) * DEFAULT_CELL_SIZE,
                        )),
                        DEFAULT_CELL_SIZE * 0.5,
                    ],
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

    let (presets, environments, environment_cells) =
        demo_environment(overworld_id, interior_id, uncut_grass, dried_grass);
    ProjectDocument {
        default_world_space: overworld.id,
        world_spaces: vec![overworld, interior],
        vegetation_catalog: Some(vegetation::fixtures::reference_catalog()),
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
        presets,
        environments,
        environment_cells,
        roads: Default::default(),
        terrain_cell_heightfields,
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

fn demo_terrain_heights(cell: CellCoord) -> Vec<f32> {
    let resolution = usize::from(DEMO_TERRAIN_HEIGHTFIELD_RESOLUTION);
    let intervals = (resolution - 1) as f32;
    let mut heights = Vec::with_capacity(resolution * resolution);
    for z in 0..resolution {
        for x in 0..resolution {
            let world = Vec2::new(
                cell.x as f32 * DEFAULT_CELL_SIZE + x as f32 * DEFAULT_CELL_SIZE / intervals,
                cell.z as f32 * DEFAULT_CELL_SIZE + z as f32 * DEFAULT_CELL_SIZE / intervals,
            );
            heights.push(demo_terrain_height(world));
        }
    }
    heights
}

/// A deliberately gentle, continuous relief fixture rather than a production landscape tool.
///
/// The small level area around the origin keeps the current non-physical character controller
/// usable. Beyond it, two coherent scales expose page seams, surface conformance, silhouettes,
/// interaction direction, and shadow behaviour without producing extreme slopes.
fn demo_terrain_height(world: Vec2) -> f32 {
    const RELIEF_SEED: u32 = 0x7a31_c5d9;
    let broad = terrain_fbm(
        world / 92.0 + terrain_material_offset(RELIEF_SEED, 0),
        RELIEF_SEED,
    ) * 2.35;
    let medium = terrain_fbm(
        world / 34.0 + terrain_material_offset(RELIEF_SEED, 1),
        RELIEF_SEED ^ 0x6a09_e667,
    ) * 0.62;
    let origin_plateau = smoothstep(6.0, 24.0, world.length());
    ((broad + medium) * origin_plateau).clamp(
        DEMO_TERRAIN_MINIMUM_HEIGHT + 0.25,
        DEMO_TERRAIN_MAXIMUM_HEIGHT - 0.25,
    )
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
        assert_eq!(project.environments.len(), 2);
        assert_eq!(project.terrain_cell_heightfields.len(), 16 * 16);
        let build = build_runtime(project).unwrap();
        assert_eq!(build.manifest.world_spaces.len(), 2);
        assert_eq!(build.manifest.default_world_space, WorldSpaceId(1));
        assert_eq!(build.cells.len(), 16 * 16 + 9 * 9);
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
        let terrain_page = |cell| {
            build
                .pages
                .iter()
                .find(|page| {
                    page.key.space == WorldSpaceId(1)
                        && page.key.cell == cell
                        && page.key.domain == PageDomain::TerrainRender
                })
                .unwrap()
                .clone()
                .decode()
                .unwrap()
        };
        let left = terrain_page(CellCoord::ZERO);
        let right = terrain_page(CellCoord { x: 1, z: 0 });
        let PagePayload::TerrainHeightfield(left) = left.payload else {
            panic!("demo overworld terrain must cook to heightfields");
        };
        let PagePayload::TerrainHeightfield(right) = right.payload else {
            panic!("demo overworld terrain must cook to heightfields");
        };
        let resolution = usize::from(left.heightfield.resolution);
        for z in 0..resolution {
            let left_index = z * resolution + resolution - 1;
            let right_index = z * resolution;
            assert_eq!(
                left.heightfield.heights[left_index],
                right.heightfield.heights[right_index]
            );
            assert_eq!(
                left.heightfield.normals_oct[left_index],
                right.heightfield.normals_oct[right_index]
            );
        }
        assert_eq!(build.definitions.len(), 2);
        let vegetation_pages = build
            .pages
            .iter()
            .filter(|page| page.key.domain == PageDomain::Vegetation)
            .collect::<Vec<_>>();
        assert!(!vegetation_pages.is_empty());
        assert!(vegetation_pages.len() <= 16 * 16);
        assert_eq!(vegetation_pages[0].key.cell, CellCoord { x: -8, z: -8 });
        let decoded = vegetation_pages[0].clone().decode().unwrap();
        let PagePayload::Vegetation(page) = decoded.payload else {
            panic!("vegetation page decoded to the wrong domain");
        };
        let catalog = build.manifest.vegetation_catalog.as_ref().unwrap();
        page.validate(catalog).unwrap();
        assert!(!page.fields.is_empty());
        assert!(page.fields.len() <= 5);
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
