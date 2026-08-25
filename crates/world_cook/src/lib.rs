use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail};
use world::{
    AssetId, CellCoord, DEFAULT_CELL_SIZE, GameplayObjectInstance, GameplayObjectsPage,
    GroundCoverCluster, GroundCoverLayerId, GroundCoverPage, GroundCoverSpecies,
    GroundCoverSpeciesId, ObjectActivationPolicy, ObjectDefinitionId, PageCodec, PageDomain,
    PageKey, PagePayload, RUNTIME_SCHEMA_VERSION, StableObjectId, StaticObjectInstance,
    StaticObjectsPage, TerrainRenderPage, WorldSpaceId, encode_page_payload,
};
use world_db::{
    AssetVariantRecord, EncodedPage, PageDependencyRecord, PageGroundCoverSpeciesRecord,
    PageObjectDefinitionRecord, ProjectDocument, RuntimeBuild, RuntimeCellRecord, RuntimeManifest,
    RuntimeObjectDefinition, SourceAssetRecord, SourceAssetVariantRecord, SourceCellRecord,
    SourceGroundCoverCellMaskRecord, SourceGroundCoverLayerRecord, SourceObjectDefinitionRecord,
    SourceObjectRecord, WorldSpaceRecord, domain_bit, read_project_database,
    write_project_database, write_runtime_database,
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
    project
        .ground_cover_species
        .sort_by_key(|species| species.id);
    project.ground_cover_layers.sort_by_key(|layer| layer.id);
    project
        .ground_cover_masks
        .sort_by_key(|mask| (mask.layer, mask.space, mask.cell));
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
    let ground_cover_species_by_id: HashMap<_, _> = project
        .ground_cover_species
        .iter()
        .map(|species| (species.id, species))
        .collect();
    if ground_cover_species_by_id.len() != project.ground_cover_species.len() {
        bail!("ground-cover species IDs must be unique");
    }
    for species in &project.ground_cover_species {
        validate_ground_cover_species(species)?;
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
        if !spaces_by_id.contains_key(&layer.space) {
            bail!(
                "ground-cover layer {:?} references a missing world space",
                layer.id
            );
        }
        if !ground_cover_species_by_id.contains_key(&layer.species) {
            bail!(
                "ground-cover layer {:?} references a missing species",
                layer.id
            );
        }
        if !layer.density_per_square_meter.is_finite() || layer.density_per_square_meter <= 0.0 {
            bail!("ground-cover layer {:?} has an invalid density", layer.id);
        }
    }
    let mut ground_cover_masks_by_cell: BTreeMap<
        (WorldSpaceId, CellCoord),
        Vec<&SourceGroundCoverCellMaskRecord>,
    > = BTreeMap::new();
    for mask in &project.ground_cover_masks {
        let Some(layer) = ground_cover_layers_by_id.get(&mask.layer) else {
            bail!(
                "ground-cover mask references missing layer {:?}",
                mask.layer
            );
        };
        if layer.space != mask.space {
            bail!("ground-cover mask and layer belong to different world spaces");
        }
        if !source_cells.contains(&(mask.space, mask.cell)) {
            bail!("ground-cover mask references missing cell {:?}", mask.cell);
        }
        let resolution = usize::from(mask.resolution);
        if resolution == 0 || mask.coverage.len() != resolution * resolution {
            bail!(
                "ground-cover mask {:?}/{:?} has invalid resolution or payload size",
                mask.layer,
                mask.cell
            );
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
    let mut content_hasher = blake3::Hasher::new();

    content_hasher.update(&project.default_world_space.0.to_le_bytes());
    for space in &project.world_spaces {
        content_hasher.update(&space.id.0.to_le_bytes());
        content_hasher.update(space.name.as_bytes());
        content_hasher.update(&space.cell_size.to_bits().to_le_bytes());
        content_hasher.update(&space.minimum_y.to_bits().to_le_bytes());
        content_hasher.update(&space.maximum_y.to_bits().to_le_bytes());
    }
    for species in &project.ground_cover_species {
        content_hasher.update(&species.id.0);
        content_hasher.update(species.key.as_bytes());
        for color in species.bottom_color.into_iter().chain(species.top_color) {
            content_hasher.update(&color.to_bits().to_le_bytes());
        }
        content_hasher.update(&species.minimum_card_height.to_bits().to_le_bytes());
        content_hasher.update(&species.maximum_card_height.to_bits().to_le_bytes());
        content_hasher.update(&species.minimum_card_width.to_bits().to_le_bytes());
        content_hasher.update(&species.maximum_card_width.to_bits().to_le_bytes());
        content_hasher.update(&species.flattened_card_probability.to_bits().to_le_bytes());
        content_hasher.update(&species.maximum_wind_displacement.to_bits().to_le_bytes());
    }
    for layer in &project.ground_cover_layers {
        content_hasher.update(&layer.id.0);
        content_hasher.update(&layer.space.0.to_le_bytes());
        content_hasher.update(layer.key.as_bytes());
        content_hasher.update(&layer.species.0);
        content_hasher.update(&layer.density_per_square_meter.to_bits().to_le_bytes());
        content_hasher.update(&layer.seed.to_le_bytes());
    }
    for mask in &project.ground_cover_masks {
        content_hasher.update(&mask.layer.0);
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
                base_color: source_cell.base_color,
            }),
            256,
        )?;
        hash_page(&mut content_hasher, &terrain_page);
        pages.push(terrain_page);

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
            let cell_size = spaces_by_id[&source_cell.space].cell_size;
            let mut clusters = Vec::new();
            let mut depended_species = HashSet::new();
            for mask in masks {
                let layer = ground_cover_layers_by_id[&mask.layer];
                let species = ground_cover_species_by_id[&layer.species];
                let resolution = usize::from(mask.resolution);
                let cluster_size = cell_size / resolution as f32;
                for (index, coverage) in mask.coverage.iter().copied().enumerate() {
                    if coverage == 0 {
                        continue;
                    }
                    let cluster_x = index % resolution;
                    let cluster_z = index / resolution;
                    let coverage_half_extent = cluster_size * 0.5;
                    let horizontal_card_reach =
                        species.maximum_card_height + species.maximum_wind_displacement;
                    clusters.push(GroundCoverCluster {
                        species: species.id,
                        local_center: [
                            (cluster_x as f32 + 0.5) * cluster_size,
                            source_cell.height + species.maximum_card_height * 0.5,
                            (cluster_z as f32 + 0.5) * cluster_size,
                        ],
                        half_extents: [
                            coverage_half_extent + horizontal_card_reach,
                            species.maximum_card_height * 0.5,
                            coverage_half_extent + horizontal_card_reach,
                        ],
                        coverage_half_extents: [coverage_half_extent, coverage_half_extent],
                        density_per_square_meter: layer.density_per_square_meter
                            * f32::from(coverage)
                            / 255.0,
                        seed: ground_cover_cluster_seed(layer.seed, source_cell.cell, index as u32),
                    });
                    depended_species.insert(species.id);
                    maximum_ground_cover_y = maximum_ground_cover_y
                        .max(source_cell.height + species.maximum_card_height);
                }
            }
            if !clusters.is_empty() {
                let key = PageKey {
                    space: source_cell.space,
                    cell: source_cell.cell,
                    domain: PageDomain::GroundCover,
                    lod: 0,
                };
                let gpu_bytes_estimate = clusters.len() as u64 * 64;
                let page = encoded_page(
                    key,
                    PagePayload::GroundCover(GroundCoverPage { clusters }),
                    gpu_bytes_estimate,
                )?;
                hash_page(&mut content_hasher, &page);
                pages.push(page);
                domain_mask |= domain_bit(PageDomain::GroundCover);
                for species in depended_species {
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
    let ground_cover_species = project.ground_cover_species.clone();
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
        assets,
        definitions,
        ground_cover_species,
        dependencies,
        definition_dependencies,
        ground_cover_species_dependencies,
    })
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
    {
        bail!("ground-cover species {:?} is invalid", species.id);
    }
    Ok(())
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
    let tree_asset = AssetId(*blake3::hash(DEMO_TREE_KEY.as_bytes()).as_bytes());
    let meadow_species = GroundCoverSpeciesId(stable_id("demo-meadow-grass"));
    let meadow_layer = GroundCoverLayerId(stable_id("demo-meadow-layer"));
    let tree_definition = definition_id("demo-tree");
    let proximity_marker_definition = definition_id("demo-proximity-marker");
    let mut cells = Vec::with_capacity(64 * 64 + 9 * 9);
    let mut objects = Vec::new();
    let mut ground_cover_masks = Vec::new();
    for x in -32_i32..32 {
        for z in -32_i32..32 {
            let checker = (x + z).rem_euclid(2) as f32;
            cells.push(SourceCellRecord {
                space: overworld.id,
                cell: CellCoord { x, z },
                height: 0.0,
                base_color: [
                    0.205 + checker * 0.012,
                    0.265 + checker * 0.012,
                    0.225 + checker * 0.010,
                ],
                source_revision: 1,
            });
            if (-4..=4).contains(&x) && (-4..=4).contains(&z) {
                ground_cover_masks.push(SourceGroundCoverCellMaskRecord {
                    layer: meadow_layer,
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
            let checker = (x + z).rem_euclid(2) as f32;
            cells.push(SourceCellRecord {
                space: interior.id,
                cell: CellCoord { x, z },
                height: 0.0,
                base_color: [
                    0.115 + checker * 0.010,
                    0.145 + checker * 0.010,
                    0.205 + checker * 0.015,
                ],
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

    ProjectDocument {
        default_world_space: overworld.id,
        world_spaces: vec![overworld, interior],
        cells,
        ground_cover_species: vec![GroundCoverSpecies {
            id: meadow_species,
            key: "meadow-long-grass".into(),
            bottom_color: [0.025, 0.055, 0.020],
            top_color: [0.105, 0.205, 0.075],
            minimum_card_height: 0.55,
            maximum_card_height: 0.78,
            minimum_card_width: 0.7,
            maximum_card_width: 1.4,
            flattened_card_probability: 0.2,
            maximum_wind_displacement: 0.22,
        }],
        ground_cover_layers: vec![SourceGroundCoverLayerRecord {
            id: meadow_layer,
            space: overworld_id,
            key: "demo-meadow".into(),
            species: meadow_species,
            density_per_square_meter: 5.0,
            seed: 0x6f53_91d2,
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
        let build = build_runtime(demo_project_document()).unwrap();
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
        assert_eq!(ground_cover_pages.len(), 9 * 9);
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
}
