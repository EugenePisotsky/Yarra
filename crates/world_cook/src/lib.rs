use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
};

use anyhow::{Context, Result, bail};
use world::{
    AssetId, CellCoord, DEFAULT_CELL_SIZE, GameplayObjectInstance, GameplayObjectsPage,
    ObjectActivationPolicy, ObjectDefinitionId, PageCodec, PageDomain, PageKey, PagePayload,
    RUNTIME_SCHEMA_VERSION, StableObjectId, StaticObjectInstance, StaticObjectsPage,
    TerrainRenderPage, WorldSpaceId, encode_page_payload,
};
use world_db::{
    AssetVariantRecord, EncodedPage, PageDependencyRecord, PageObjectDefinitionRecord,
    ProjectDocument, RuntimeBuild, RuntimeCellRecord, RuntimeManifest, RuntimeObjectDefinition,
    SourceAssetRecord, SourceCellRecord, SourceObjectDefinitionRecord, SourceObjectRecord,
    WorldSpaceRecord, domain_bit, read_project_database, write_project_database,
    write_runtime_database,
};

pub const DEMO_TREE_URI: &str = "local/forest_tree_07/tree_07_summer.gltf";

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
    project.assets.sort_by_key(|asset| asset.id.0);
    project.definitions.sort_by_key(|definition| definition.id);
    project.objects.sort_by_key(|object| object.id.0);

    let definitions_by_id: HashMap<_, _> = project
        .definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect();
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
    let mut content_hasher = blake3::Hasher::new();

    content_hasher.update(&project.default_world_space.0.to_le_bytes());
    for space in &project.world_spaces {
        content_hasher.update(&space.id.0.to_le_bytes());
        content_hasher.update(space.name.as_bytes());
        content_hasher.update(&space.cell_size.to_bits().to_le_bytes());
        content_hasher.update(&space.minimum_y.to_bits().to_le_bytes());
        content_hasher.update(&space.maximum_y.to_bits().to_le_bytes());
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
                dependencies.push(PageDependencyRecord {
                    page: object_key,
                    asset: *asset,
                    asset_lod: 0,
                });
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

        cells.push(RuntimeCellRecord {
            space: source_cell.space,
            cell: source_cell.cell,
            minimum_y: source_cell.height,
            maximum_y: source_cell.height,
            domain_mask,
            source_revision: source_cell.source_revision,
        });
    }

    let assets = project
        .assets
        .into_iter()
        .map(|asset| {
            content_hasher.update(&asset.id.0);
            content_hasher.update(asset.source_uri.as_bytes());
            AssetVariantRecord {
                asset: asset.id,
                lod: 0,
                kind: asset.kind,
                uri: asset.source_uri,
                bounds: [8.0, 18.0, 8.0],
                gpu_bytes_estimate: 2 * 1024 * 1024,
                shadow_policy: 1,
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
        },
        cells,
        pages,
        assets,
        definitions,
        dependencies,
        definition_dependencies,
    })
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
    let tree_asset = AssetId(*blake3::hash(DEMO_TREE_URI.as_bytes()).as_bytes());
    let tree_definition = definition_id("demo-tree");
    let proximity_marker_definition = definition_id("demo-proximity-marker");
    let mut cells = Vec::with_capacity(64 * 64 + 9 * 9);
    let mut objects = Vec::new();
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
            if x.rem_euclid(5) == 2 && z.rem_euclid(5) == 2 {
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
        assets: vec![SourceAssetRecord {
            id: tree_asset,
            kind: "gltf-scene".into(),
            source_uri: DEMO_TREE_URI.into(),
        }],
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
    let hash = blake3::hash(key.as_bytes());
    let mut id = [0; 16];
    id.copy_from_slice(&hash.as_bytes()[..16]);
    ObjectDefinitionId(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_cook_is_large_logically_but_page_addressable() {
        let build = build_runtime(demo_project_document()).unwrap();
        assert_eq!(build.manifest.world_spaces.len(), 2);
        assert_eq!(build.manifest.default_world_space, WorldSpaceId(1));
        assert_eq!(build.cells.len(), 64 * 64 + 9 * 9);
        assert!(build.pages.len() > build.cells.len());
        assert_eq!(build.assets.len(), 1);
        assert_eq!(build.definitions.len(), 2);
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
}
