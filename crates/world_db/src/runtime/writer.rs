//! Whole-build and incremental writes share the same runtime encoding and SQL.
use crate::catalog::{write_terrain_catalog, write_vegetation_catalog};
use crate::runtime::read_runtime_manifest;
use crate::storage::ensure_new_database_path;
use crate::{
    MAX_COOK_CATALOG_ROWS, MAX_COOK_CELL_BYTES, MAX_COOK_MANUAL_OBJECTS_PER_CELL, RuntimeBuild,
    RuntimeManifest, WorldDbError, atmosphere, schema,
};
use rusqlite::{Connection, params};
use std::path::Path;

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

fn write_runtime_build(transaction: &Connection, build: &RuntimeBuild) -> Result<(), WorldDbError> {
    write_runtime_header(transaction, build)?;
    write_runtime_spatial(transaction, build)
}
fn write_runtime_header(
    transaction: &Connection,
    build: &RuntimeBuild,
) -> Result<(), WorldDbError> {
    let manifest = &build.manifest;
    for space in &manifest.world_spaces {
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
    write_vegetation_catalog(transaction, manifest.vegetation_catalog.as_ref())?;
    write_terrain_catalog(
        transaction,
        &build.terrain_surfaces,
        &build.terrain_texture_sets,
        &build.terrain_texture_layers,
        &build.terrain_profiles,
    )?;
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
    Ok(())
}
fn write_runtime_spatial(
    transaction: &Connection,
    build: &RuntimeBuild,
) -> Result<(), WorldDbError> {
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

/// Owns only the unpublished output; dropping without finish rolls back its writes.
pub struct RuntimeCookWriter {
    connection: Connection,
}
impl RuntimeCookWriter {
    pub fn create(path: &Path, header: &RuntimeBuild) -> Result<Self, WorldDbError> {
        if !header.cells.is_empty()
            || !header.pages.is_empty()
            || !header.dependencies.is_empty()
            || !header.definition_dependencies.is_empty()
            || !header.terrain_surface_dependencies.is_empty()
        {
            return Err(invalid("runtime cook header contains spatial data"));
        }
        ensure_new_database_path(path)?;
        let connection = Connection::open(path)?;
        connection.execute_batch(schema::RUNTIME_SCHEMA)?;
        connection
            .execute_batch("PRAGMA cache_size=-8192; PRAGMA temp_store=FILE; BEGIN IMMEDIATE;")?;
        write_runtime_header(&connection, header)?;
        Ok(Self { connection })
    }
    pub fn append_cell(&self, batch: &RuntimeBuild) -> Result<(), WorldDbError> {
        let decoded_bytes = batch
            .pages
            .iter()
            .try_fold(0_u64, |bytes, page| bytes.checked_add(page.decoded_bytes));
        if batch.cells.len() != 1
            || batch.pages.len() > 9
            || decoded_bytes.is_none_or(|bytes| bytes > MAX_COOK_CELL_BYTES)
            || batch.dependencies.len() > MAX_COOK_CATALOG_ROWS
            || batch.definition_dependencies.len() > MAX_COOK_MANUAL_OBJECTS_PER_CELL
            || batch.terrain_surface_dependencies.len() > world::MAX_TERRAIN_SURFACES_PER_CELL
            || batch
                .pages
                .iter()
                .map(|p| p.payload.len() as u64)
                .sum::<u64>()
                > MAX_COOK_CELL_BYTES
        {
            return Err(invalid("runtime cell output exceeds cook budget"));
        }
        let cell = &batch.cells[0];
        if batch
            .pages
            .iter()
            .any(|p| p.key.space != cell.space || p.key.cell != cell.cell)
        {
            return Err(invalid("runtime batch crosses cell boundaries"));
        }
        if batch
            .dependencies
            .iter()
            .map(|d| d.page)
            .chain(batch.definition_dependencies.iter().map(|d| d.page))
            .chain(batch.terrain_surface_dependencies.iter().map(|d| d.page))
            .any(|key| key.space != cell.space || key.cell != cell.cell)
        {
            return Err(invalid("runtime dependencies cross cell boundaries"));
        }
        write_runtime_spatial(&self.connection, batch)
    }
    pub fn finish(self, content_hash: [u8; 32]) -> Result<RuntimeManifest, WorldDbError> {
        let generation_id = blake3::Hash::from_bytes(content_hash).to_hex()[..16].to_owned();
        self.connection.execute(
            "UPDATE runtime_metadata SET generation_id=?1,content_hash=?2 WHERE singleton=1",
            params![generation_id, content_hash.as_slice()],
        )?;
        self.connection.execute_batch("COMMIT; PRAGMA optimize;")?;
        read_runtime_manifest(&self.connection)
    }
}
fn invalid(message: impl Into<String>) -> WorldDbError {
    WorldDbError::Cook(message.into())
}
