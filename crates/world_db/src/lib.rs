mod schema;

use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use world::{
    AssetId, CellCoord, GroundCoverLayerId, GroundCoverSpecies, GroundCoverSpeciesId,
    MAX_DECODED_PAGE_BYTES, ObjectActivationPolicy, ObjectDefinitionId, PROJECT_SCHEMA_VERSION,
    PageCodec, PageDomain, PageKey, PagePayload, RUNTIME_SCHEMA_VERSION, StableObjectId,
    TerrainProfile, TerrainSurface, TerrainSurfaceId, TerrainTextureLayer, TerrainTextureSet,
    TerrainTextureSetId, WorldSpaceId, decode_page_payload,
};

#[derive(Debug, Clone)]
pub struct ProjectDocument {
    pub default_world_space: WorldSpaceId,
    pub world_spaces: Vec<WorldSpaceRecord>,
    pub cells: Vec<SourceCellRecord>,
    pub terrain_surfaces: Vec<TerrainSurface>,
    pub terrain_texture_sets: Vec<TerrainTextureSet>,
    pub terrain_texture_layers: Vec<TerrainTextureLayer>,
    pub terrain_profiles: Vec<TerrainProfile>,
    pub terrain_cell_surface_slots: Vec<SourceTerrainCellSurfaceSlotRecord>,
    pub terrain_cell_weight_pages: Vec<SourceTerrainCellWeightPageRecord>,
    pub ground_cover_species: Vec<GroundCoverSpecies>,
    pub ground_cover_layers: Vec<SourceGroundCoverLayerRecord>,
    pub ground_cover_masks: Vec<SourceGroundCoverCellMaskRecord>,
    pub assets: Vec<SourceAssetRecord>,
    pub asset_variants: Vec<SourceAssetVariantRecord>,
    pub definitions: Vec<SourceObjectDefinitionRecord>,
    pub objects: Vec<SourceObjectRecord>,
}

#[derive(Debug, Clone)]
pub struct WorldSpaceRecord {
    pub id: WorldSpaceId,
    pub name: String,
    pub cell_size: f32,
    pub minimum_y: f32,
    pub maximum_y: f32,
}

#[derive(Debug, Clone)]
pub struct SourceCellRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub height: f32,
    pub source_revision: i64,
}

#[derive(Debug, Clone)]
pub struct SourceTerrainCellSurfaceSlotRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub slot: u8,
    pub surface: TerrainSurfaceId,
}

#[derive(Debug, Clone)]
pub struct SourceTerrainCellWeightPageRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub page: u8,
    pub resolution: u16,
    pub rgba: Vec<u8>,
    pub source_revision: i64,
}

#[derive(Debug, Clone)]
pub struct SourceGroundCoverLayerRecord {
    pub id: GroundCoverLayerId,
    pub space: WorldSpaceId,
    pub key: String,
    pub species: GroundCoverSpeciesId,
    pub density_per_square_meter: f32,
    pub seed: u32,
}

#[derive(Debug, Clone)]
pub struct SourceGroundCoverCellMaskRecord {
    pub layer: GroundCoverLayerId,
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub resolution: u8,
    pub coverage: Vec<u8>,
    pub source_revision: i64,
}

#[derive(Debug, Clone)]
pub struct SourceAssetRecord {
    pub id: AssetId,
    pub key: String,
    pub kind: String,
    pub source_uri: String,
}

#[derive(Debug, Clone)]
pub struct SourceAssetVariantRecord {
    pub asset: AssetId,
    pub lod: u8,
    pub uri: String,
    pub bounds: [f32; 3],
    pub gpu_bytes_estimate: u64,
    pub shadow_policy: i64,
    pub minimum_screen_height: f32,
}

#[derive(Debug, Clone)]
pub struct SourceObjectDefinitionRecord {
    pub id: ObjectDefinitionId,
    pub key: String,
    pub display_name: String,
    pub visual_asset: Option<AssetId>,
    pub activation: ObjectActivationPolicy,
}

#[derive(Debug, Clone)]
pub struct SourceObjectRecord {
    pub id: StableObjectId,
    pub space: WorldSpaceId,
    pub owner_cell: CellCoord,
    pub definition: ObjectDefinitionId,
    pub local_translation: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
    pub source_revision: i64,
}

#[derive(Debug, Clone)]
pub struct RuntimeBuild {
    pub manifest: RuntimeManifest,
    pub cells: Vec<RuntimeCellRecord>,
    pub pages: Vec<EncodedPage>,
    pub terrain_surfaces: Vec<TerrainSurface>,
    pub terrain_texture_sets: Vec<TerrainTextureSet>,
    pub terrain_texture_layers: Vec<TerrainTextureLayer>,
    pub terrain_profiles: Vec<TerrainProfile>,
    pub assets: Vec<AssetVariantRecord>,
    pub definitions: Vec<RuntimeObjectDefinition>,
    pub ground_cover_species: Vec<GroundCoverSpecies>,
    pub dependencies: Vec<PageDependencyRecord>,
    pub definition_dependencies: Vec<PageObjectDefinitionRecord>,
    pub ground_cover_species_dependencies: Vec<PageGroundCoverSpeciesRecord>,
    pub terrain_surface_dependencies: Vec<PageTerrainSurfaceRecord>,
}

#[derive(Debug, Clone)]
pub struct RuntimeManifest {
    pub schema_version: i64,
    pub generation_id: String,
    pub content_hash: [u8; 32],
    pub default_world_space: WorldSpaceId,
    pub world_spaces: Vec<WorldSpaceRecord>,
}

impl RuntimeManifest {
    pub fn world_space(&self, id: WorldSpaceId) -> Option<&WorldSpaceRecord> {
        self.world_spaces.iter().find(|space| space.id == id)
    }

    pub fn default_world_space(&self) -> &WorldSpaceRecord {
        self.world_space(self.default_world_space)
            .expect("validated runtime manifest has no default world space")
    }
}

#[derive(Debug, Clone)]
pub struct RuntimeCellRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub minimum_y: f32,
    pub maximum_y: f32,
    pub domain_mask: u64,
    pub source_revision: i64,
}

#[derive(Debug, Clone)]
pub struct CellDescriptor {
    pub cell: CellCoord,
    pub minimum_y: f32,
    pub maximum_y: f32,
    pub domain_mask: u64,
}

impl CellDescriptor {
    pub fn has_domain(&self, domain: PageDomain) -> bool {
        self.domain_mask & domain_bit(domain) != 0
    }
}

#[derive(Debug, Clone)]
pub struct EncodedPage {
    pub key: PageKey,
    pub codec: PageCodec,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
    pub checksum: [u8; 32],
    pub payload: Vec<u8>,
}

impl EncodedPage {
    pub fn decode(self) -> Result<DecodedPage, WorldDbError> {
        if self.decoded_bytes > MAX_DECODED_PAGE_BYTES {
            return Err(WorldDbError::PageTooLarge {
                actual: self.decoded_bytes,
                maximum: MAX_DECODED_PAGE_BYTES,
            });
        }

        let bytes = match self.codec {
            PageCodec::Raw => self.payload,
            PageCodec::Zstd => {
                let decoder = zstd::stream::read::Decoder::new(Cursor::new(self.payload))?;
                let mut bytes = Vec::with_capacity(self.decoded_bytes as usize);
                decoder
                    .take(MAX_DECODED_PAGE_BYTES + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_DECODED_PAGE_BYTES {
                    return Err(WorldDbError::PageTooLarge {
                        actual: bytes.len() as u64,
                        maximum: MAX_DECODED_PAGE_BYTES,
                    });
                }
                bytes
            }
        };

        if bytes.len() as u64 != self.decoded_bytes {
            return Err(WorldDbError::DecodedSizeMismatch {
                expected: self.decoded_bytes,
                actual: bytes.len() as u64,
            });
        }
        let actual_checksum = *blake3::hash(&bytes).as_bytes();
        if actual_checksum != self.checksum {
            return Err(WorldDbError::ChecksumMismatch(self.key));
        }
        let payload = decode_page_payload(&bytes)?;
        if payload.domain() != self.key.domain {
            return Err(WorldDbError::DomainMismatch {
                expected: self.key.domain,
                actual: payload.domain(),
            });
        }

        Ok(DecodedPage {
            key: self.key,
            payload,
            decoded_bytes: self.decoded_bytes,
            gpu_bytes_estimate: self.gpu_bytes_estimate,
        })
    }
}

#[derive(Debug, Clone)]
pub struct DecodedPage {
    pub key: PageKey,
    pub payload: PagePayload,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
}

#[derive(Debug, Clone)]
pub struct AssetVariantRecord {
    pub asset: AssetId,
    pub lod: u8,
    pub kind: String,
    pub uri: String,
    pub bounds: [f32; 3],
    pub gpu_bytes_estimate: u64,
    pub shadow_policy: i64,
    pub minimum_screen_height: f32,
}

#[derive(Debug, Clone)]
pub struct PageDependencyRecord {
    pub page: PageKey,
    pub asset: AssetId,
    pub asset_lod: u8,
}

#[derive(Debug, Clone)]
pub struct RuntimeObjectDefinition {
    pub id: ObjectDefinitionId,
    pub key: String,
    pub display_name: String,
    pub visual_asset: Option<AssetId>,
    pub activation: ObjectActivationPolicy,
}

#[derive(Debug, Clone)]
pub struct PageObjectDefinitionRecord {
    pub page: PageKey,
    pub definition: ObjectDefinitionId,
}

#[derive(Debug, Clone)]
pub struct PageGroundCoverSpeciesRecord {
    pub page: PageKey,
    pub species: GroundCoverSpeciesId,
}

#[derive(Debug, Clone)]
pub struct PageTerrainSurfaceRecord {
    pub page: PageKey,
    pub surface: TerrainSurfaceId,
}

#[derive(Debug, Clone)]
pub struct RuntimeTerrainSurface {
    pub surface: TerrainSurface,
    pub layer: u16,
}

#[derive(Debug, Clone)]
pub struct TerrainRenderResources {
    pub profile: TerrainProfile,
    pub texture_set: TerrainTextureSet,
    pub surfaces: Vec<RuntimeTerrainSurface>,
}

#[derive(Debug, Clone)]
pub struct PageDependency {
    pub asset: AssetId,
    pub asset_lod: u8,
    pub kind: String,
    pub uri: String,
    pub bounds: [f32; 3],
    pub gpu_bytes_estimate: u64,
    pub shadow_policy: i64,
    pub minimum_screen_height: f32,
}

pub fn domain_bit(domain: PageDomain) -> u64 {
    1 << (domain as u64 - 1)
}

pub fn write_project_database(path: &Path, document: &ProjectDocument) -> Result<(), WorldDbError> {
    ensure_new_database_path(path)?;
    let mut connection = Connection::open(path)?;
    connection.execute_batch(schema::PROJECT_SCHEMA)?;
    let transaction = connection.transaction()?;
    write_project_document(&transaction, document)?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    Ok(())
}

fn write_project_document(
    transaction: &Transaction<'_>,
    document: &ProjectDocument,
) -> Result<(), WorldDbError> {
    transaction.execute(
        "INSERT INTO project_metadata(key, value) VALUES ('schema_name', 'yarra-project')",
        [],
    )?;
    for space in &document.world_spaces {
        transaction.execute(
            "INSERT INTO world_spaces(id, name, cell_size, minimum_y, maximum_y) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                space.id.0,
                space.name,
                space.cell_size,
                space.minimum_y,
                space.maximum_y
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO project_settings(singleton, default_world_space_id) VALUES (1, ?1)",
        [document.default_world_space.0],
    )?;
    for cell in &document.cells {
        transaction.execute(
            "INSERT INTO source_cells(world_space_id, cell_x, cell_z, height, source_revision) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                cell.space.0,
                cell.cell.x,
                cell.cell.z,
                cell.height,
                cell.source_revision
            ],
        )?;
    }
    write_terrain_catalog(
        transaction,
        &document.terrain_surfaces,
        &document.terrain_texture_sets,
        &document.terrain_texture_layers,
        &document.terrain_profiles,
    )?;
    for slot in &document.terrain_cell_surface_slots {
        transaction.execute(
            "INSERT INTO terrain_cell_surface_slots( \
                world_space_id, cell_x, cell_z, slot, surface_id \
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                slot.space.0,
                slot.cell.x,
                slot.cell.z,
                i64::from(slot.slot),
                slot.surface.0.as_slice(),
            ],
        )?;
    }
    for weights in &document.terrain_cell_weight_pages {
        transaction.execute(
            "INSERT INTO terrain_cell_weight_pages( \
                world_space_id, cell_x, cell_z, page, resolution, rgba, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                weights.space.0,
                weights.cell.x,
                weights.cell.z,
                i64::from(weights.page),
                i64::from(weights.resolution),
                weights.rgba,
                weights.source_revision,
            ],
        )?;
    }
    write_ground_cover_species(transaction, &document.ground_cover_species)?;
    for layer in &document.ground_cover_layers {
        transaction.execute(
            "INSERT INTO ground_cover_layers( \
                layer_id, world_space_id, layer_key, species_id, \
                density_per_square_meter, seed \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                layer.id.0.as_slice(),
                layer.space.0,
                layer.key,
                layer.species.0.as_slice(),
                layer.density_per_square_meter,
                i64::from(layer.seed),
            ],
        )?;
    }
    for mask in &document.ground_cover_masks {
        transaction.execute(
            "INSERT INTO ground_cover_cell_masks( \
                layer_id, world_space_id, cell_x, cell_z, resolution, coverage, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                mask.layer.0.as_slice(),
                mask.space.0,
                mask.cell.x,
                mask.cell.z,
                i64::from(mask.resolution),
                mask.coverage,
                mask.source_revision,
            ],
        )?;
    }
    for asset in &document.assets {
        transaction.execute(
            "INSERT INTO source_assets(asset_id, asset_key, kind, source_uri) \
             VALUES (?1, ?2, ?3, ?4)",
            params![
                asset.id.0.as_slice(),
                asset.key,
                asset.kind,
                asset.source_uri
            ],
        )?;
    }
    for variant in &document.asset_variants {
        transaction.execute(
            "INSERT INTO source_asset_variants( \
                asset_id, lod, uri, bounds_x, bounds_y, bounds_z, gpu_bytes_estimate, \
                shadow_policy, minimum_screen_height \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                variant.asset.0.as_slice(),
                i64::from(variant.lod),
                variant.uri,
                variant.bounds[0],
                variant.bounds[1],
                variant.bounds[2],
                i64::try_from(variant.gpu_bytes_estimate)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                variant.shadow_policy,
                variant.minimum_screen_height,
            ],
        )?;
    }
    for definition in &document.definitions {
        transaction.execute(
            "INSERT INTO object_definitions( \
                definition_id, definition_key, display_name, visual_asset_id, activation_policy \
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                definition.id.0.as_slice(),
                definition.key,
                definition.display_name,
                definition.visual_asset.map(|asset| asset.0),
                definition.activation as i64,
            ],
        )?;
    }
    for object in &document.objects {
        transaction.execute(
            "INSERT INTO object_placements( \
                object_id, world_space_id, owner_cell_x, owner_cell_z, definition_id, \
                local_x, local_y, local_z, yaw, scale, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                object.id.0.as_slice(),
                object.space.0,
                object.owner_cell.x,
                object.owner_cell.z,
                object.definition.0.as_slice(),
                object.local_translation[0],
                object.local_translation[1],
                object.local_translation[2],
                object.yaw,
                object.scale,
                object.source_revision
            ],
        )?;
    }
    Ok(())
}

fn write_terrain_catalog(
    transaction: &Transaction<'_>,
    surfaces: &[TerrainSurface],
    texture_sets: &[TerrainTextureSet],
    texture_layers: &[TerrainTextureLayer],
    profiles: &[TerrainProfile],
) -> Result<(), WorldDbError> {
    for surface in surfaces {
        transaction.execute(
            "INSERT INTO terrain_surfaces( \
                surface_id, surface_key, display_name, tile_size, anti_tiling, normal_y_sign, \
                normal_strength, roughness_min, roughness_max \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                surface.id.0.as_slice(),
                surface.key,
                surface.display_name,
                surface.tile_size,
                surface.anti_tiling,
                surface.normal_y_sign,
                surface.normal_strength,
                surface.roughness_min,
                surface.roughness_max,
            ],
        )?;
    }
    for texture_set in texture_sets {
        transaction.execute(
            "INSERT INTO terrain_texture_sets( \
                texture_set_id, texture_set_key, base_color_universal_uri, \
                normal_material_universal_uri, macro_variation_universal_uri, \
                base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                universal_gpu_bytes, astc_gpu_bytes \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                texture_set.id.0.as_slice(),
                texture_set.key,
                texture_set.base_color_universal_uri,
                texture_set.normal_material_universal_uri,
                texture_set.macro_variation_universal_uri,
                texture_set.base_color_astc_uri,
                texture_set.normal_material_astc_uri,
                texture_set.macro_variation_astc_uri,
                i64::try_from(texture_set.universal_gpu_bytes)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                i64::try_from(texture_set.astc_gpu_bytes)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
            ],
        )?;
    }
    for layer in texture_layers {
        transaction.execute(
            "INSERT INTO terrain_texture_set_layers(texture_set_id, layer, surface_id) \
             VALUES (?1, ?2, ?3)",
            params![
                layer.texture_set.0.as_slice(),
                i64::from(layer.layer),
                layer.surface.0.as_slice(),
            ],
        )?;
    }
    for profile in profiles {
        transaction.execute(
            "INSERT INTO world_space_terrain_profiles( \
                world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                profile.space.0,
                profile.texture_set.0.as_slice(),
                i64::from(profile.weight_resolution),
                profile.macro_scales[0],
                profile.macro_scales[1],
                profile.macro_scales[2],
                profile.macro_contrast,
                profile.macro_albedo_strength,
            ],
        )?;
    }
    Ok(())
}

fn write_ground_cover_species(
    transaction: &Transaction<'_>,
    species: &[GroundCoverSpecies],
) -> Result<(), WorldDbError> {
    for species in species {
        transaction.execute(
            "INSERT INTO ground_cover_species( \
                species_id, species_key, bottom_color_r, bottom_color_g, bottom_color_b, \
                top_color_r, top_color_g, top_color_b, minimum_card_height, \
                maximum_card_height, minimum_card_width, maximum_card_width, \
                flattened_card_probability, maximum_wind_displacement \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                species.id.0.as_slice(),
                species.key,
                species.bottom_color[0],
                species.bottom_color[1],
                species.bottom_color[2],
                species.top_color[0],
                species.top_color[1],
                species.top_color[2],
                species.minimum_card_height,
                species.maximum_card_height,
                species.minimum_card_width,
                species.maximum_card_width,
                species.flattened_card_probability,
                species.maximum_wind_displacement,
            ],
        )?;
    }
    Ok(())
}

pub fn read_project_database(path: &Path) -> Result<ProjectDocument, WorldDbError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;

    let world_spaces = query_world_spaces(&connection)?;
    let default_world_space = connection.query_row(
        "SELECT default_world_space_id FROM project_settings WHERE singleton = 1",
        [],
        |row| Ok(WorldSpaceId(row.get(0)?)),
    )?;
    let mut statement = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, height, source_revision \
         FROM source_cells ORDER BY world_space_id, cell_x, cell_z",
    )?;
    let cells = statement
        .query_map([], |row| {
            Ok(SourceCellRecord {
                space: WorldSpaceId(row.get(0)?),
                cell: CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                height: row.get(3)?,
                source_revision: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let terrain_surfaces = query_all_terrain_surfaces(&connection)?;
    let terrain_texture_sets = query_all_terrain_texture_sets(&connection)?;
    let terrain_texture_layers = query_all_terrain_texture_layers(&connection)?;
    let terrain_profiles = query_all_terrain_profiles(&connection)?;
    let mut statement = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, slot, surface_id \
         FROM terrain_cell_surface_slots \
         ORDER BY world_space_id, cell_x, cell_z, slot",
    )?;
    let terrain_cell_surface_slots = statement
        .query_map([], |row| {
            Ok(SourceTerrainCellSurfaceSlotRecord {
                space: WorldSpaceId(row.get(0)?),
                cell: CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                slot: row.get::<_, i64>(3)? as u8,
                surface: TerrainSurfaceId(blob_array(row.get_ref(4)?.as_blob()?, "surface_id")?),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, page, resolution, rgba, source_revision \
         FROM terrain_cell_weight_pages \
         ORDER BY world_space_id, cell_x, cell_z, page",
    )?;
    let terrain_cell_weight_pages = statement
        .query_map([], |row| {
            Ok(SourceTerrainCellWeightPageRecord {
                space: WorldSpaceId(row.get(0)?),
                cell: CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                page: row.get::<_, i64>(3)? as u8,
                resolution: row.get::<_, i64>(4)? as u16,
                rgba: row.get(5)?,
                source_revision: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let ground_cover_species = query_all_ground_cover_species(&connection)?;
    let mut statement = connection.prepare(
        "SELECT layer_id, world_space_id, layer_key, species_id, \
                density_per_square_meter, seed \
         FROM ground_cover_layers ORDER BY layer_id",
    )?;
    let ground_cover_layers = statement
        .query_map([], |row| {
            Ok(SourceGroundCoverLayerRecord {
                id: GroundCoverLayerId(blob_array(row.get_ref(0)?.as_blob()?, "layer_id")?),
                space: WorldSpaceId(row.get(1)?),
                key: row.get(2)?,
                species: GroundCoverSpeciesId(blob_array(
                    row.get_ref(3)?.as_blob()?,
                    "species_id",
                )?),
                density_per_square_meter: row.get(4)?,
                seed: row.get::<_, i64>(5)? as u32,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut statement = connection.prepare(
        "SELECT layer_id, world_space_id, cell_x, cell_z, resolution, coverage, source_revision \
         FROM ground_cover_cell_masks \
         ORDER BY layer_id, world_space_id, cell_x, cell_z",
    )?;
    let ground_cover_masks = statement
        .query_map([], |row| {
            Ok(SourceGroundCoverCellMaskRecord {
                layer: GroundCoverLayerId(blob_array(row.get_ref(0)?.as_blob()?, "layer_id")?),
                space: WorldSpaceId(row.get(1)?),
                cell: CellCoord {
                    x: row.get(2)?,
                    z: row.get(3)?,
                },
                resolution: row.get::<_, i64>(4)? as u8,
                coverage: row.get(5)?,
                source_revision: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT asset_id, asset_key, kind, source_uri FROM source_assets ORDER BY asset_id",
    )?;
    let assets = statement
        .query_map([], |row| {
            Ok(SourceAssetRecord {
                id: AssetId(blob_array(row.get_ref(0)?.as_blob()?, "asset_id")?),
                key: row.get(1)?,
                kind: row.get(2)?,
                source_uri: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT asset_id, lod, uri, bounds_x, bounds_y, bounds_z, gpu_bytes_estimate, \
                shadow_policy, minimum_screen_height \
         FROM source_asset_variants ORDER BY asset_id, lod",
    )?;
    let asset_variants = statement
        .query_map([], |row| {
            let gpu_bytes_estimate: i64 = row.get(6)?;
            Ok(SourceAssetVariantRecord {
                asset: AssetId(blob_array(row.get_ref(0)?.as_blob()?, "asset_id")?),
                lod: row.get::<_, i64>(1)? as u8,
                uri: row.get(2)?,
                bounds: [row.get(3)?, row.get(4)?, row.get(5)?],
                gpu_bytes_estimate: gpu_bytes_estimate as u64,
                shadow_policy: row.get(7)?,
                minimum_screen_height: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT definition_id, definition_key, display_name, visual_asset_id, activation_policy \
         FROM object_definitions ORDER BY definition_id",
    )?;
    let definitions = statement
        .query_map([], |row| {
            Ok(SourceObjectDefinitionRecord {
                id: ObjectDefinitionId(blob_array(row.get_ref(0)?.as_blob()?, "definition_id")?),
                key: row.get(1)?,
                display_name: row.get(2)?,
                visual_asset: row
                    .get::<_, Option<Vec<u8>>>(3)?
                    .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
                    .transpose()?,
                activation: ObjectActivationPolicy::try_from(row.get::<_, i64>(4)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    },
                )?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, definition_id, \
                local_x, local_y, local_z, yaw, scale, source_revision \
         FROM object_placements ORDER BY object_id",
    )?;
    let objects = statement
        .query_map([], |row| {
            Ok(SourceObjectRecord {
                id: StableObjectId(blob_array(row.get_ref(0)?.as_blob()?, "object_id")?),
                space: WorldSpaceId(row.get(1)?),
                owner_cell: CellCoord {
                    x: row.get(2)?,
                    z: row.get(3)?,
                },
                definition: ObjectDefinitionId(blob_array(
                    row.get_ref(4)?.as_blob()?,
                    "definition_id",
                )?),
                local_translation: [row.get(5)?, row.get(6)?, row.get(7)?],
                yaw: row.get(8)?,
                scale: row.get(9)?,
                source_revision: row.get(10)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok(ProjectDocument {
        default_world_space,
        world_spaces,
        cells,
        terrain_surfaces,
        terrain_texture_sets,
        terrain_texture_layers,
        terrain_profiles,
        terrain_cell_surface_slots,
        terrain_cell_weight_pages,
        ground_cover_species,
        ground_cover_layers,
        ground_cover_masks,
        assets,
        asset_variants,
        definitions,
        objects,
    })
}

pub fn write_runtime_database(path: &Path, build: &RuntimeBuild) -> Result<(), WorldDbError> {
    ensure_new_database_path(path)?;
    let mut connection = Connection::open(path)?;
    connection.execute_batch(schema::RUNTIME_SCHEMA)?;
    let transaction = connection.transaction()?;
    write_runtime_build(&transaction, build)?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA optimize;")?;
    Ok(())
}

fn write_runtime_build(
    transaction: &Transaction<'_>,
    build: &RuntimeBuild,
) -> Result<(), WorldDbError> {
    let manifest = &build.manifest;
    for space in &manifest.world_spaces {
        transaction.execute(
            "INSERT INTO world_spaces(id, name, cell_size, minimum_y, maximum_y) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                space.id.0,
                space.name,
                space.cell_size,
                space.minimum_y,
                space.maximum_y
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO runtime_metadata( \
            singleton, schema_version, generation_id, content_hash, default_world_space_id \
         ) VALUES (1, ?1, ?2, ?3, ?4)",
        params![
            manifest.schema_version,
            manifest.generation_id,
            manifest.content_hash.as_slice(),
            manifest.default_world_space.0
        ],
    )?;
    write_terrain_catalog(
        transaction,
        &build.terrain_surfaces,
        &build.terrain_texture_sets,
        &build.terrain_texture_layers,
        &build.terrain_profiles,
    )?;
    for cell in &build.cells {
        transaction.execute(
            "INSERT INTO cells( \
                world_space_id, cell_x, cell_z, minimum_y, maximum_y, domain_mask, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                cell.space.0,
                cell.cell.x,
                cell.cell.z,
                cell.minimum_y,
                cell.maximum_y,
                i64::try_from(cell.domain_mask).map_err(|_| WorldDbError::IntegerOverflow)?,
                cell.source_revision
            ],
        )?;
    }
    for asset in &build.assets {
        transaction.execute(
            "INSERT INTO asset_variants( \
                asset_id, lod, kind, uri, bounds_x, bounds_y, bounds_z, \
                gpu_bytes_estimate, shadow_policy, minimum_screen_height \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                asset.asset.0.as_slice(),
                i64::from(asset.lod),
                asset.kind,
                asset.uri,
                asset.bounds[0],
                asset.bounds[1],
                asset.bounds[2],
                i64::try_from(asset.gpu_bytes_estimate)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                asset.shadow_policy,
                asset.minimum_screen_height,
            ],
        )?;
    }
    for definition in &build.definitions {
        transaction.execute(
            "INSERT INTO object_definitions( \
                definition_id, definition_key, display_name, visual_asset_id, activation_policy \
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                definition.id.0.as_slice(),
                definition.key,
                definition.display_name,
                definition.visual_asset.map(|asset| asset.0),
                definition.activation as i64,
            ],
        )?;
    }
    write_ground_cover_species(transaction, &build.ground_cover_species)?;
    for page in &build.pages {
        transaction.execute(
            "INSERT INTO cell_pages( \
                world_space_id, cell_x, cell_z, domain, lod, codec, encoded_bytes, \
                decoded_bytes, gpu_bytes_estimate, checksum, payload \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                page.key.space.0,
                page.key.cell.x,
                page.key.cell.z,
                page.key.domain as i64,
                i64::from(page.key.lod),
                page.codec as i64,
                i64::try_from(page.payload.len()).map_err(|_| WorldDbError::IntegerOverflow)?,
                i64::try_from(page.decoded_bytes).map_err(|_| WorldDbError::IntegerOverflow)?,
                i64::try_from(page.gpu_bytes_estimate)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                page.checksum.as_slice(),
                page.payload
            ],
        )?;
    }
    for dependency in &build.dependencies {
        transaction.execute(
            "INSERT INTO page_dependencies( \
                world_space_id, cell_x, cell_z, domain, lod, asset_id, asset_lod \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                dependency.page.space.0,
                dependency.page.cell.x,
                dependency.page.cell.z,
                dependency.page.domain as i64,
                i64::from(dependency.page.lod),
                dependency.asset.0.as_slice(),
                i64::from(dependency.asset_lod)
            ],
        )?;
    }
    for dependency in &build.definition_dependencies {
        transaction.execute(
            "INSERT INTO page_object_definitions( \
                world_space_id, cell_x, cell_z, domain, lod, definition_id \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                dependency.page.space.0,
                dependency.page.cell.x,
                dependency.page.cell.z,
                dependency.page.domain as i64,
                i64::from(dependency.page.lod),
                dependency.definition.0.as_slice(),
            ],
        )?;
    }
    for dependency in &build.ground_cover_species_dependencies {
        transaction.execute(
            "INSERT INTO page_ground_cover_species( \
                world_space_id, cell_x, cell_z, domain, lod, species_id \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                dependency.page.space.0,
                dependency.page.cell.x,
                dependency.page.cell.z,
                dependency.page.domain as i64,
                i64::from(dependency.page.lod),
                dependency.species.0.as_slice(),
            ],
        )?;
    }
    for dependency in &build.terrain_surface_dependencies {
        transaction.execute(
            "INSERT INTO page_terrain_surfaces( \
                world_space_id, cell_x, cell_z, domain, lod, surface_id \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                dependency.page.space.0,
                dependency.page.cell.x,
                dependency.page.cell.z,
                dependency.page.domain as i64,
                i64::from(dependency.page.lod),
                dependency.surface.0.as_slice(),
            ],
        )?;
    }
    Ok(())
}

pub struct RuntimeReader {
    connection: Connection,
    manifest: RuntimeManifest,
}

impl RuntimeReader {
    pub fn open_immutable(path: &Path) -> Result<Self, WorldDbError> {
        let uri = immutable_uri(path);
        let connection = Connection::open_with_flags(
            uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        connection.execute_batch("PRAGMA query_only = ON; PRAGMA foreign_keys = ON;")?;
        ensure_schema_version(&connection, RUNTIME_SCHEMA_VERSION, "runtime")?;
        let manifest = read_runtime_manifest(&connection)?;
        if manifest.schema_version != RUNTIME_SCHEMA_VERSION {
            return Err(WorldDbError::SchemaVersion {
                database: "runtime",
                expected: RUNTIME_SCHEMA_VERSION,
                actual: manifest.schema_version,
            });
        }
        Ok(Self {
            connection,
            manifest,
        })
    }

    pub fn manifest(&self) -> &RuntimeManifest {
        &self.manifest
    }

    pub fn read_cell_descriptors(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
    ) -> Result<Vec<CellDescriptor>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT cell_x, cell_z, minimum_y, maximum_y, domain_mask \
             FROM cells \
             WHERE world_space_id = ?1 \
               AND cell_x BETWEEN ?2 AND ?3 \
               AND cell_z BETWEEN ?4 AND ?5 \
             ORDER BY cell_x, cell_z",
        )?;
        statement
            .query_map(
                params![space.0, minimum.x, maximum.x, minimum.z, maximum.z],
                |row| {
                    let domain_mask: i64 = row.get(4)?;
                    Ok(CellDescriptor {
                        cell: CellCoord {
                            x: row.get(0)?,
                            z: row.get(1)?,
                        },
                        minimum_y: row.get(2)?,
                        maximum_y: row.get(3)?,
                        domain_mask: domain_mask as u64,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_page(&self, key: PageKey) -> Result<Option<EncodedPage>, WorldDbError> {
        self.connection
            .query_row(
                "SELECT codec, decoded_bytes, gpu_bytes_estimate, checksum, payload \
                 FROM cell_pages \
                 WHERE world_space_id = ?1 AND cell_x = ?2 AND cell_z = ?3 \
                   AND domain = ?4 AND lod = ?5",
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod)
                ],
                |row| {
                    let decoded_bytes: i64 = row.get(1)?;
                    let gpu_bytes_estimate: i64 = row.get(2)?;
                    Ok(EncodedPage {
                        key,
                        codec: PageCodec::try_from(row.get::<_, i64>(0)?).map_err(|error| {
                            rusqlite::Error::FromSqlConversionFailure(
                                0,
                                rusqlite::types::Type::Integer,
                                Box::new(error),
                            )
                        })?,
                        decoded_bytes: decoded_bytes as u64,
                        gpu_bytes_estimate: gpu_bytes_estimate as u64,
                        checksum: blob_array(row.get_ref(3)?.as_blob()?, "checksum")?,
                        payload: row.get(4)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn read_dependencies(&self, key: PageKey) -> Result<Vec<PageDependency>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT d.asset_id, d.asset_lod, a.kind, a.uri, \
                    a.bounds_x, a.bounds_y, a.bounds_z, a.gpu_bytes_estimate, a.shadow_policy, \
                    a.minimum_screen_height \
             FROM page_dependencies d \
             JOIN asset_variants a ON a.asset_id = d.asset_id AND a.lod = d.asset_lod \
             WHERE d.world_space_id = ?1 AND d.cell_x = ?2 AND d.cell_z = ?3 \
               AND d.domain = ?4 AND d.lod = ?5 \
             ORDER BY d.asset_id, d.asset_lod",
        )?;
        statement
            .query_map(
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod)
                ],
                |row| {
                    let gpu_bytes_estimate: i64 = row.get(7)?;
                    Ok(PageDependency {
                        asset: AssetId(blob_array(row.get_ref(0)?.as_blob()?, "asset_id")?),
                        asset_lod: row.get::<_, i64>(1)? as u8,
                        kind: row.get(2)?,
                        uri: row.get(3)?,
                        bounds: [row.get(4)?, row.get(5)?, row.get(6)?],
                        gpu_bytes_estimate: gpu_bytes_estimate as u64,
                        shadow_policy: row.get(8)?,
                        minimum_screen_height: row.get(9)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_object_definitions(
        &self,
        key: PageKey,
    ) -> Result<Vec<RuntimeObjectDefinition>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT d.definition_id, d.definition_key, d.display_name, \
                    d.visual_asset_id, d.activation_policy \
             FROM page_object_definitions p \
             JOIN object_definitions d ON d.definition_id = p.definition_id \
             WHERE p.world_space_id = ?1 AND p.cell_x = ?2 AND p.cell_z = ?3 \
               AND p.domain = ?4 AND p.lod = ?5 \
             ORDER BY d.definition_id",
        )?;
        statement
            .query_map(
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod),
                ],
                |row| {
                    Ok(RuntimeObjectDefinition {
                        id: ObjectDefinitionId(blob_array(
                            row.get_ref(0)?.as_blob()?,
                            "definition_id",
                        )?),
                        key: row.get(1)?,
                        display_name: row.get(2)?,
                        visual_asset: row
                            .get::<_, Option<Vec<u8>>>(3)?
                            .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
                            .transpose()?,
                        activation: ObjectActivationPolicy::try_from(row.get::<_, i64>(4)?)
                            .map_err(|error| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    4,
                                    rusqlite::types::Type::Integer,
                                    Box::new(error),
                                )
                            })?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_ground_cover_species(
        &self,
        key: PageKey,
    ) -> Result<Vec<GroundCoverSpecies>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT s.species_id, s.species_key, \
                    s.bottom_color_r, s.bottom_color_g, s.bottom_color_b, \
                    s.top_color_r, s.top_color_g, s.top_color_b, \
                    s.minimum_card_height, s.maximum_card_height, \
                    s.minimum_card_width, s.maximum_card_width, \
                    s.flattened_card_probability, s.maximum_wind_displacement \
             FROM page_ground_cover_species p \
             JOIN ground_cover_species s ON s.species_id = p.species_id \
             WHERE p.world_space_id = ?1 AND p.cell_x = ?2 AND p.cell_z = ?3 \
               AND p.domain = ?4 AND p.lod = ?5 \
             ORDER BY s.species_id",
        )?;
        statement
            .query_map(
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod),
                ],
                ground_cover_species_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()
            .map_err(Into::into)
    }

    pub fn read_terrain_resources(
        &self,
        key: PageKey,
    ) -> Result<TerrainRenderResources, WorldDbError> {
        let profile = self.connection.query_row(
            "SELECT world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                    macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength \
             FROM world_space_terrain_profiles WHERE world_space_id = ?1",
            [key.space.0],
            terrain_profile_from_row,
        )?;
        let texture_set = self.connection.query_row(
            "SELECT texture_set_id, texture_set_key, base_color_universal_uri, \
                    normal_material_universal_uri, macro_variation_universal_uri, \
                    base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                    universal_gpu_bytes, astc_gpu_bytes \
             FROM terrain_texture_sets WHERE texture_set_id = ?1",
            [profile.texture_set.0.as_slice()],
            terrain_texture_set_from_row,
        )?;
        let mut statement = self.connection.prepare_cached(
            "SELECT s.surface_id, s.surface_key, s.display_name, s.tile_size, s.anti_tiling, \
                    s.normal_y_sign, s.normal_strength, s.roughness_min, s.roughness_max, l.layer \
             FROM page_terrain_surfaces p \
             JOIN terrain_surfaces s ON s.surface_id = p.surface_id \
             JOIN terrain_texture_set_layers l \
               ON l.surface_id = s.surface_id AND l.texture_set_id = ?6 \
             WHERE p.world_space_id = ?1 AND p.cell_x = ?2 AND p.cell_z = ?3 \
               AND p.domain = ?4 AND p.lod = ?5 \
             ORDER BY l.layer",
        )?;
        let surfaces = statement
            .query_map(
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    i64::from(key.lod),
                    profile.texture_set.0.as_slice(),
                ],
                |row| {
                    Ok(RuntimeTerrainSurface {
                        surface: terrain_surface_from_row(row)?,
                        layer: row.get::<_, i64>(9)? as u16,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TerrainRenderResources {
            profile,
            texture_set,
            surfaces,
        })
    }
}

fn query_all_ground_cover_species(
    connection: &Connection,
) -> Result<Vec<GroundCoverSpecies>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT species_id, species_key, \
                bottom_color_r, bottom_color_g, bottom_color_b, \
                top_color_r, top_color_g, top_color_b, \
                minimum_card_height, maximum_card_height, \
                minimum_card_width, maximum_card_width, \
                flattened_card_probability, maximum_wind_displacement \
         FROM ground_cover_species ORDER BY species_id",
    )?;
    Ok(statement
        .query_map([], ground_cover_species_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn ground_cover_species_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GroundCoverSpecies> {
    Ok(GroundCoverSpecies {
        id: GroundCoverSpeciesId(blob_array(row.get_ref(0)?.as_blob()?, "species_id")?),
        key: row.get(1)?,
        bottom_color: [row.get(2)?, row.get(3)?, row.get(4)?],
        top_color: [row.get(5)?, row.get(6)?, row.get(7)?],
        minimum_card_height: row.get(8)?,
        maximum_card_height: row.get(9)?,
        minimum_card_width: row.get(10)?,
        maximum_card_width: row.get(11)?,
        flattened_card_probability: row.get(12)?,
        maximum_wind_displacement: row.get(13)?,
    })
}

fn query_all_terrain_surfaces(
    connection: &Connection,
) -> Result<Vec<TerrainSurface>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT surface_id, surface_key, display_name, tile_size, anti_tiling, normal_y_sign, \
                normal_strength, roughness_min, roughness_max \
         FROM terrain_surfaces ORDER BY surface_id",
    )?;
    Ok(statement
        .query_map([], terrain_surface_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn terrain_surface_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerrainSurface> {
    Ok(TerrainSurface {
        id: TerrainSurfaceId(blob_array(row.get_ref(0)?.as_blob()?, "surface_id")?),
        key: row.get(1)?,
        display_name: row.get(2)?,
        tile_size: row.get(3)?,
        anti_tiling: row.get(4)?,
        normal_y_sign: row.get(5)?,
        normal_strength: row.get(6)?,
        roughness_min: row.get(7)?,
        roughness_max: row.get(8)?,
    })
}

fn query_all_terrain_texture_sets(
    connection: &Connection,
) -> Result<Vec<TerrainTextureSet>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT texture_set_id, texture_set_key, base_color_universal_uri, \
                normal_material_universal_uri, macro_variation_universal_uri, \
                base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                universal_gpu_bytes, astc_gpu_bytes \
         FROM terrain_texture_sets ORDER BY texture_set_id",
    )?;
    Ok(statement
        .query_map([], terrain_texture_set_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn terrain_texture_set_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerrainTextureSet> {
    Ok(TerrainTextureSet {
        id: TerrainTextureSetId(blob_array(row.get_ref(0)?.as_blob()?, "texture_set_id")?),
        key: row.get(1)?,
        base_color_universal_uri: row.get(2)?,
        normal_material_universal_uri: row.get(3)?,
        macro_variation_universal_uri: row.get(4)?,
        base_color_astc_uri: row.get(5)?,
        normal_material_astc_uri: row.get(6)?,
        macro_variation_astc_uri: row.get(7)?,
        universal_gpu_bytes: row.get::<_, i64>(8)? as u64,
        astc_gpu_bytes: row.get::<_, i64>(9)? as u64,
    })
}

fn query_all_terrain_texture_layers(
    connection: &Connection,
) -> Result<Vec<TerrainTextureLayer>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT texture_set_id, surface_id, layer \
         FROM terrain_texture_set_layers ORDER BY texture_set_id, layer",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(TerrainTextureLayer {
                texture_set: TerrainTextureSetId(blob_array(
                    row.get_ref(0)?.as_blob()?,
                    "texture_set_id",
                )?),
                surface: TerrainSurfaceId(blob_array(row.get_ref(1)?.as_blob()?, "surface_id")?),
                layer: row.get::<_, i64>(2)? as u16,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn query_all_terrain_profiles(
    connection: &Connection,
) -> Result<Vec<TerrainProfile>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength \
         FROM world_space_terrain_profiles ORDER BY world_space_id",
    )?;
    Ok(statement
        .query_map([], terrain_profile_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

fn terrain_profile_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TerrainProfile> {
    Ok(TerrainProfile {
        space: WorldSpaceId(row.get(0)?),
        texture_set: TerrainTextureSetId(blob_array(row.get_ref(1)?.as_blob()?, "texture_set_id")?),
        weight_resolution: row.get::<_, i64>(2)? as u16,
        macro_scales: [row.get(3)?, row.get(4)?, row.get(5)?],
        macro_contrast: row.get(6)?,
        macro_albedo_strength: row.get(7)?,
    })
}

fn query_world_spaces(connection: &Connection) -> Result<Vec<WorldSpaceRecord>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT id, name, cell_size, minimum_y, maximum_y FROM world_spaces ORDER BY id",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(WorldSpaceRecord {
                id: WorldSpaceId(row.get(0)?),
                name: row.get(1)?,
                cell_size: row.get(2)?,
                minimum_y: row.get(3)?,
                maximum_y: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

fn read_runtime_manifest(connection: &Connection) -> Result<RuntimeManifest, WorldDbError> {
    let world_spaces = query_world_spaces(connection)?;
    let manifest = connection.query_row(
        "SELECT schema_version, generation_id, content_hash, default_world_space_id \
         FROM runtime_metadata WHERE singleton = 1",
        [],
        |row| {
            Ok(RuntimeManifest {
                schema_version: row.get(0)?,
                generation_id: row.get(1)?,
                content_hash: blob_array(row.get_ref(2)?.as_blob()?, "content_hash")?,
                default_world_space: WorldSpaceId(row.get(3)?),
                world_spaces,
            })
        },
    )?;
    if manifest.world_space(manifest.default_world_space).is_none() {
        return Err(WorldDbError::UnknownDefaultWorldSpace(
            manifest.default_world_space,
        ));
    }
    Ok(manifest)
}

fn ensure_new_database_path(path: &Path) -> Result<(), WorldDbError> {
    if path.exists() {
        return Err(WorldDbError::AlreadyExists(path.to_path_buf()));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    Ok(())
}

fn ensure_schema_version(
    connection: &Connection,
    expected: i64,
    database: &'static str,
) -> Result<(), WorldDbError> {
    let actual: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if actual != expected {
        return Err(WorldDbError::SchemaVersion {
            database,
            expected,
            actual,
        });
    }
    Ok(())
}

fn immutable_uri(path: &Path) -> String {
    let raw = path.to_string_lossy();
    let escaped = raw
        .replace('%', "%25")
        .replace(' ', "%20")
        .replace('?', "%3F")
        .replace('#', "%23");
    format!("file:{escaped}?immutable=1")
}

fn blob_array<const N: usize>(bytes: &[u8], field: &'static str) -> rusqlite::Result<[u8; N]> {
    bytes.try_into().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Blob,
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{field} must contain exactly {N} bytes"),
            )
            .into(),
        )
    })
}

#[derive(Debug, thiserror::Error)]
pub enum WorldDbError {
    #[error("SQLite operation failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("page payload is invalid: {0}")]
    Payload(#[from] world::PagePayloadDecodeError),
    #[error("database already exists: {0}")]
    AlreadyExists(PathBuf),
    #[error("{database} schema version is {actual}, expected {expected}")]
    SchemaVersion {
        database: &'static str,
        expected: i64,
        actual: i64,
    },
    #[error("page {0:?} failed its BLAKE3 checksum")]
    ChecksumMismatch(PageKey),
    #[error("decoded page size is {actual} bytes, maximum is {maximum}")]
    PageTooLarge { actual: u64, maximum: u64 },
    #[error("decoded page size is {actual} bytes, expected {expected}")]
    DecodedSizeMismatch { expected: u64, actual: u64 },
    #[error("page domain is {actual:?}, expected {expected:?}")]
    DomainMismatch {
        expected: PageDomain,
        actual: PageDomain,
    },
    #[error("integer does not fit the SQLite representation")]
    IntegerOverflow,
    #[error("default world space {0:?} is not present in the world-space catalog")]
    UnknownDefaultWorldSpace(WorldSpaceId),
}

#[cfg(test)]
mod tests {
    use super::*;
    use world::{
        GameplayObjectInstance, GameplayObjectsPage, GroundCoverCluster, GroundCoverLayerId,
        GroundCoverPage, GroundCoverSpecies, GroundCoverSpeciesId, TerrainRenderPage,
        encode_page_payload,
    };

    #[test]
    fn project_and_runtime_databases_are_distinct_and_readable() {
        let directory = unique_test_directory();
        fs::create_dir_all(&directory).unwrap();
        let project_path = directory.join("project.sqlite");
        let runtime_path = directory.join("runtime.sqlite");
        let space = WorldSpaceRecord {
            id: WorldSpaceId(1),
            name: "test".into(),
            cell_size: 32.0,
            minimum_y: 0.0,
            maximum_y: 0.0,
        };
        let second_space = WorldSpaceRecord {
            id: WorldSpaceId(2),
            name: "second-test-space".into(),
            cell_size: 16.0,
            minimum_y: -4.0,
            maximum_y: 8.0,
        };
        let definition_id = ObjectDefinitionId([3; 16]);
        let object_id = StableObjectId([5; 16]);
        let asset_id = AssetId([9; 32]);
        let ground_cover_species_id = GroundCoverSpeciesId([11; 16]);
        let ground_cover_layer_id = GroundCoverLayerId([12; 16]);
        let terrain_surface_id = TerrainSurfaceId([13; 16]);
        let terrain_texture_set_id = TerrainTextureSetId([14; 16]);
        let terrain_surface = TerrainSurface {
            id: terrain_surface_id,
            key: "test-grass".into(),
            display_name: "Test grass".into(),
            tile_size: 2.0,
            anti_tiling: true,
            normal_y_sign: 1.0,
            normal_strength: 0.5,
            roughness_min: 0.8,
            roughness_max: 1.0,
        };
        let terrain_texture_set = TerrainTextureSet {
            id: terrain_texture_set_id,
            key: "test-terrain".into(),
            base_color_universal_uri: "base-universal.ktx2".into(),
            normal_material_universal_uri: "normal-universal.ktx2".into(),
            macro_variation_universal_uri: "macro-universal.ktx2".into(),
            base_color_astc_uri: "base-astc.ktx2".into(),
            normal_material_astc_uri: "normal-astc.ktx2".into(),
            macro_variation_astc_uri: "macro-astc.ktx2".into(),
            universal_gpu_bytes: 1200,
            astc_gpu_bytes: 600,
        };
        let ground_cover_species = GroundCoverSpecies {
            id: ground_cover_species_id,
            key: "test/meadow-grass".into(),
            bottom_color: [0.04, 0.11, 0.03],
            top_color: [0.18, 0.32, 0.10],
            minimum_card_height: 0.55,
            maximum_card_height: 0.78,
            minimum_card_width: 0.7,
            maximum_card_width: 1.4,
            flattened_card_probability: 0.2,
            maximum_wind_displacement: 0.2,
        };
        write_project_database(
            &project_path,
            &ProjectDocument {
                default_world_space: space.id,
                world_spaces: vec![space.clone(), second_space.clone()],
                cells: vec![
                    SourceCellRecord {
                        space: space.id,
                        cell: CellCoord::ZERO,
                        height: 0.0,
                        source_revision: 1,
                    },
                    SourceCellRecord {
                        space: second_space.id,
                        cell: CellCoord::ZERO,
                        height: 2.0,
                        source_revision: 1,
                    },
                ],
                terrain_surfaces: vec![terrain_surface.clone()],
                terrain_texture_sets: vec![terrain_texture_set.clone()],
                terrain_texture_layers: vec![TerrainTextureLayer {
                    texture_set: terrain_texture_set_id,
                    surface: terrain_surface_id,
                    layer: 0,
                }],
                terrain_profiles: vec![
                    TerrainProfile {
                        space: space.id,
                        texture_set: terrain_texture_set_id,
                        weight_resolution: 2,
                        macro_scales: [8.0, 32.0, 128.0],
                        macro_contrast: 1.25,
                        macro_albedo_strength: 0.12,
                    },
                    TerrainProfile {
                        space: second_space.id,
                        texture_set: terrain_texture_set_id,
                        weight_resolution: 2,
                        macro_scales: [8.0, 32.0, 128.0],
                        macro_contrast: 1.25,
                        macro_albedo_strength: 0.12,
                    },
                ],
                terrain_cell_surface_slots: vec![
                    SourceTerrainCellSurfaceSlotRecord {
                        space: space.id,
                        cell: CellCoord::ZERO,
                        slot: 0,
                        surface: terrain_surface_id,
                    },
                    SourceTerrainCellSurfaceSlotRecord {
                        space: second_space.id,
                        cell: CellCoord::ZERO,
                        slot: 0,
                        surface: terrain_surface_id,
                    },
                ],
                terrain_cell_weight_pages: Vec::new(),
                ground_cover_species: vec![ground_cover_species.clone()],
                ground_cover_layers: vec![SourceGroundCoverLayerRecord {
                    id: ground_cover_layer_id,
                    space: space.id,
                    key: "test/meadow".into(),
                    species: ground_cover_species_id,
                    density_per_square_meter: 7.0,
                    seed: 91,
                }],
                ground_cover_masks: vec![SourceGroundCoverCellMaskRecord {
                    layer: ground_cover_layer_id,
                    space: space.id,
                    cell: CellCoord::ZERO,
                    resolution: 2,
                    coverage: vec![255, 128, 0, 255],
                    source_revision: 2,
                }],
                assets: vec![SourceAssetRecord {
                    id: asset_id,
                    key: "test/tree".into(),
                    kind: "gltf-scene".into(),
                    source_uri: "local/source/tree.fbx".into(),
                }],
                asset_variants: vec![SourceAssetVariantRecord {
                    asset: asset_id,
                    lod: 0,
                    uri: "local/runtime/tree_lod0.gltf".into(),
                    bounds: [2.0, 8.0, 2.0],
                    gpu_bytes_estimate: 4096,
                    shadow_policy: 1,
                    minimum_screen_height: 0.0,
                }],
                definitions: vec![SourceObjectDefinitionRecord {
                    id: definition_id,
                    key: "test-door".into(),
                    display_name: "Test door".into(),
                    visual_asset: None,
                    activation: ObjectActivationPolicy::Proximity,
                }],
                objects: vec![SourceObjectRecord {
                    id: object_id,
                    space: space.id,
                    owner_cell: CellCoord::ZERO,
                    definition: definition_id,
                    local_translation: [1.0, 0.0, 2.0],
                    yaw: 0.0,
                    scale: 1.0,
                    source_revision: 1,
                }],
            },
        )
        .unwrap();
        let project = read_project_database(&project_path).unwrap();
        assert_eq!(project.default_world_space, space.id);
        assert_eq!(project.world_spaces.len(), 2);
        assert_eq!(project.cells.len(), 2);
        assert_eq!(project.terrain_surfaces, vec![terrain_surface.clone()]);
        assert_eq!(
            project.terrain_texture_sets,
            vec![terrain_texture_set.clone()]
        );
        assert_eq!(project.terrain_cell_surface_slots.len(), 2);
        assert_eq!(project.assets[0].key, "test/tree");
        assert_eq!(project.asset_variants[0].minimum_screen_height, 0.0);
        assert_eq!(project.definitions.len(), 1);
        assert_eq!(project.objects[0].definition, definition_id);
        assert_eq!(
            project.ground_cover_species,
            vec![ground_cover_species.clone()]
        );
        assert_eq!(project.ground_cover_layers.len(), 1);
        assert_eq!(
            project.ground_cover_masks[0].coverage,
            vec![255, 128, 0, 255]
        );

        let payload = PagePayload::TerrainRender(TerrainRenderPage {
            height: 0.0,
            surfaces: vec![terrain_surface_id],
            weight_pages: Vec::new(),
        });
        let decoded = encode_page_payload(&payload).unwrap();
        let page = EncodedPage {
            key: PageKey {
                space: space.id,
                cell: CellCoord::ZERO,
                domain: PageDomain::TerrainRender,
                lod: 0,
            },
            codec: PageCodec::Raw,
            decoded_bytes: decoded.len() as u64,
            gpu_bytes_estimate: 128,
            checksum: *blake3::hash(&decoded).as_bytes(),
            payload: decoded,
        };
        let gameplay_payload = PagePayload::GameplayObjects(GameplayObjectsPage {
            instances: vec![GameplayObjectInstance {
                id: object_id,
                definition: definition_id,
                translation: [1.0, 0.0, 2.0],
                yaw: 0.0,
                scale: 1.0,
            }],
        });
        let gameplay_decoded = encode_page_payload(&gameplay_payload).unwrap();
        let gameplay_page = EncodedPage {
            key: PageKey {
                space: space.id,
                cell: CellCoord::ZERO,
                domain: PageDomain::GameplayObjects,
                lod: 0,
            },
            codec: PageCodec::Raw,
            decoded_bytes: gameplay_decoded.len() as u64,
            gpu_bytes_estimate: 0,
            checksum: *blake3::hash(&gameplay_decoded).as_bytes(),
            payload: gameplay_decoded,
        };
        let ground_cover_payload = PagePayload::GroundCover(GroundCoverPage {
            clusters: vec![GroundCoverCluster {
                species: ground_cover_species_id,
                local_center: [8.0, 0.35, 8.0],
                half_extents: [8.2, 0.35, 8.2],
                coverage_half_extents: [8.0, 8.0],
                density_per_square_meter: 7.0,
                seed: 91,
            }],
        });
        let ground_cover_decoded = encode_page_payload(&ground_cover_payload).unwrap();
        let ground_cover_page = EncodedPage {
            key: PageKey {
                space: space.id,
                cell: CellCoord::ZERO,
                domain: PageDomain::GroundCover,
                lod: 0,
            },
            codec: PageCodec::Raw,
            decoded_bytes: ground_cover_decoded.len() as u64,
            gpu_bytes_estimate: 64,
            checksum: *blake3::hash(&ground_cover_decoded).as_bytes(),
            payload: ground_cover_decoded,
        };
        write_runtime_database(
            &runtime_path,
            &RuntimeBuild {
                manifest: RuntimeManifest {
                    schema_version: RUNTIME_SCHEMA_VERSION,
                    generation_id: "test-generation".into(),
                    content_hash: [7; 32],
                    default_world_space: space.id,
                    world_spaces: vec![space.clone(), second_space.clone()],
                },
                cells: vec![RuntimeCellRecord {
                    space: space.id,
                    cell: CellCoord::ZERO,
                    minimum_y: 0.0,
                    maximum_y: 0.0,
                    domain_mask: domain_bit(PageDomain::TerrainRender)
                        | domain_bit(PageDomain::GameplayObjects)
                        | domain_bit(PageDomain::GroundCover),
                    source_revision: 1,
                }],
                pages: vec![
                    page.clone(),
                    gameplay_page.clone(),
                    ground_cover_page.clone(),
                ],
                terrain_surfaces: vec![terrain_surface.clone()],
                terrain_texture_sets: vec![terrain_texture_set.clone()],
                terrain_texture_layers: vec![TerrainTextureLayer {
                    texture_set: terrain_texture_set_id,
                    surface: terrain_surface_id,
                    layer: 0,
                }],
                terrain_profiles: vec![TerrainProfile {
                    space: space.id,
                    texture_set: terrain_texture_set_id,
                    weight_resolution: 2,
                    macro_scales: [8.0, 32.0, 128.0],
                    macro_contrast: 1.25,
                    macro_albedo_strength: 0.12,
                }],
                assets: Vec::new(),
                definitions: vec![RuntimeObjectDefinition {
                    id: definition_id,
                    key: "test-door".into(),
                    display_name: "Test door".into(),
                    visual_asset: None,
                    activation: ObjectActivationPolicy::Proximity,
                }],
                ground_cover_species: vec![ground_cover_species.clone()],
                dependencies: Vec::new(),
                definition_dependencies: vec![PageObjectDefinitionRecord {
                    page: gameplay_page.key,
                    definition: definition_id,
                }],
                ground_cover_species_dependencies: vec![PageGroundCoverSpeciesRecord {
                    page: ground_cover_page.key,
                    species: ground_cover_species_id,
                }],
                terrain_surface_dependencies: vec![PageTerrainSurfaceRecord {
                    page: page.key,
                    surface: terrain_surface_id,
                }],
            },
        )
        .unwrap();

        let reader = RuntimeReader::open_immutable(&runtime_path).unwrap();
        assert_eq!(reader.manifest().generation_id, "test-generation");
        assert_eq!(reader.manifest().world_spaces.len(), 2);
        assert_eq!(reader.manifest().default_world_space, space.id);
        assert_eq!(
            reader
                .read_page(page.key)
                .unwrap()
                .unwrap()
                .decode()
                .unwrap()
                .payload,
            payload
        );
        let terrain = reader.read_terrain_resources(page.key).unwrap();
        assert_eq!(terrain.profile.space, space.id);
        assert_eq!(terrain.texture_set, terrain_texture_set);
        assert_eq!(terrain.surfaces[0].surface, terrain_surface);
        assert_eq!(terrain.surfaces[0].layer, 0);
        let definitions = reader.read_object_definitions(gameplay_page.key).unwrap();
        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0].id, definition_id);
        assert_eq!(definitions[0].key, "test-door");
        assert_eq!(
            reader
                .read_page(ground_cover_page.key)
                .unwrap()
                .unwrap()
                .decode()
                .unwrap()
                .payload,
            ground_cover_payload
        );
        assert_eq!(
            reader
                .read_ground_cover_species(ground_cover_page.key)
                .unwrap(),
            vec![ground_cover_species]
        );
        fs::remove_dir_all(directory).unwrap();
    }

    fn unique_test_directory() -> PathBuf {
        std::env::temp_dir().join(format!(
            "yarra-world-db-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ))
    }
}
