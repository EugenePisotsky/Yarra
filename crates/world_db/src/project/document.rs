//! Whole-project import/export. Interactive callers use ProjectReader/ProjectWriter.
use crate::catalog::{
    query_all_terrain_profiles, query_all_terrain_surfaces, query_all_terrain_texture_layers,
    query_all_terrain_texture_sets, query_world_spaces, read_vegetation_catalog,
    write_terrain_catalog, write_vegetation_catalog,
};
use crate::project::query::query_source_catalog;
use crate::storage::{
    blob_array, decode_f32_blob, encode_f32_blob, ensure_new_database_path, ensure_schema_version,
};
use crate::{
    ProjectDocument, SourceCellRecord, SourceObjectRecord, SourceTerrainCellHeightfieldRecord,
    WorldDbError, atmosphere, environment_store, road_store, schema,
};
use rusqlite::{Connection, OpenFlags, Transaction, params};
use std::path::Path;
use world::{CellCoord, ObjectDefinitionId, PROJECT_SCHEMA_VERSION, StableObjectId, WorldSpaceId};

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
            "INSERT INTO world_spaces(id, name, cell_size, minimum_y, maximum_y, atmosphere, atmosphere_revision) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                space.id.0,
                space.name,
                space.cell_size,
                space.minimum_y,
                space.maximum_y,
                atmosphere::encode(&space.atmosphere)?,
                space.atmosphere_revision
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO project_settings(singleton, default_world_space_id) VALUES (1, ?1)",
        [document.default_world_space.0],
    )?;
    write_vegetation_catalog(transaction, document.vegetation_catalog.as_ref())?;
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
    for heightfield in &document.terrain_cell_heightfields {
        transaction.execute(
            "INSERT INTO terrain_cell_heightfields( \
                world_space_id, cell_x, cell_z, resolution, heights, source_revision \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                heightfield.space.0,
                heightfield.cell.x,
                heightfield.cell.z,
                i64::from(heightfield.resolution),
                encode_f32_blob(&heightfield.heights),
                heightfield.source_revision,
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
    // Collection presets validate against assets and their runtime LOD variants.
    environment_store::write_environment_document(transaction, document)?;
    road_store::write_document(transaction, &document.roads)?;
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

pub fn read_project_database(path: &Path) -> Result<ProjectDocument, WorldDbError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.execute_batch("BEGIN DEFERRED;")?;
    ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;

    let world_spaces = query_world_spaces(&connection)?;
    let default_world_space = connection.query_row(
        "SELECT default_world_space_id FROM project_settings WHERE singleton = 1",
        [],
        |row| Ok(WorldSpaceId(row.get(0)?)),
    )?;
    let vegetation_catalog = read_vegetation_catalog(&connection)?;
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
        "SELECT world_space_id, cell_x, cell_z, resolution, heights, source_revision \
         FROM terrain_cell_heightfields \
         ORDER BY world_space_id, cell_x, cell_z",
    )?;
    let terrain_cell_heightfields = statement
        .query_map([], |row| {
            Ok(SourceTerrainCellHeightfieldRecord {
                space: WorldSpaceId(row.get(0)?),
                cell: CellCoord {
                    x: row.get(1)?,
                    z: row.get(2)?,
                },
                resolution: row.get::<_, i64>(3)? as u16,
                heights: decode_f32_blob(row.get_ref(4)?.as_blob()?, "terrain heights")?,
                source_revision: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let (presets, environments, environment_cells) =
        environment_store::read_environment_document(&connection)?;
    let roads = road_store::read_document(&connection)?;

    let (assets, asset_variants, definitions) = query_source_catalog(&connection)?;

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
        vegetation_catalog,
        cells,
        terrain_surfaces,
        terrain_texture_sets,
        terrain_texture_layers,
        terrain_profiles,
        presets,
        environments,
        environment_cells,
        roads,
        terrain_cell_heightfields,
        assets,
        asset_variants,
        definitions,
        objects,
    })
}
