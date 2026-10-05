//! Whole-build and incremental writes share the same runtime encoding and SQL.
use crate::catalog::{write_terrain_catalog, write_vegetation_catalog};
use crate::runtime::read_runtime_manifest;
use crate::storage::ensure_new_database_path;
use crate::{
    MAX_COOK_CATALOG_ROWS, MAX_COOK_CELL_BYTES, MAX_COOK_MANUAL_OBJECTS_PER_CELL, RuntimeBuild,
    RuntimeManifest, WorldDbError, atmosphere, schema,
};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub fn write_runtime_database(path: &Path, build: &RuntimeBuild) -> Result<(), WorldDbError> {
    ensure_new_database_path(path)?;
    let mut connection = Connection::open(path)?;
    connection.execute_batch(schema::RUNTIME_SCHEMA)?;
    let transaction = connection.transaction()?;
    write_runtime_build(&transaction, build)?;
    super::far_objects::rebuild(&transaction)?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA optimize;")?;
    Ok(())
}

fn write_runtime_build(transaction: &Connection, build: &RuntimeBuild) -> Result<(), WorldDbError> {
    write_runtime_header(transaction, build)?;
    // A whole build has no per-cell source fingerprints or hashes; incremental cooks
    // treat its cells as unknown.
    write_runtime_spatial(transaction, build, [0; 32], [0; 32])
}
fn write_runtime_header(
    transaction: &Connection,
    build: &RuntimeBuild,
) -> Result<(), WorldDbError> {
    let manifest = &build.manifest;
    for space in &manifest.world_spaces {
        transaction.execute(
            "INSERT INTO world_spaces(id, name, cell_size, minimum_y, maximum_y, atmosphere, atmosphere_revision, sea_level) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                space.id.0,
                space.name,
                space.cell_size,
                space.minimum_y,
                space.maximum_y,
                atmosphere::encode(&space.atmosphere)?,
                space.atmosphere_revision,
                space.sea_level
            ],
        )?;
    }
    transaction.execute(
        "INSERT INTO runtime_metadata( \
            singleton, schema_version, generation_id, content_hash, header_hash, \
            default_world_space_id, start_view, gameplay_areas \
         ) VALUES (1, ?1, ?2, ?3, ?3, ?4, ?5, ?6)",
        params![
            manifest.schema_version,
            manifest.generation_id,
            manifest.content_hash.as_slice(),
            manifest.default_world_space.0,
            crate::storage::encode_start_view(manifest.start_view.as_ref())?,
            crate::gameplay_areas::encode(&manifest.gameplay_areas)?
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
    input_fingerprint: [u8; 32],
    content_hash: [u8; 32],
) -> Result<(), WorldDbError> {
    for cell in &build.cells {
        transaction.execute(
            "INSERT INTO cells( \
                world_space_id, cell_x, cell_z, minimum_y, maximum_y, domain_mask, \
                source_revision, terrain_resolution, input_fingerprint, content_hash \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                cell.space.0,
                cell.cell.x,
                cell.cell.z,
                cell.minimum_y,
                cell.maximum_y,
                i64::try_from(cell.domain_mask).map_err(|_| WorldDbError::IntegerOverflow)?,
                cell.source_revision,
                cell.terrain_resolution,
                input_fingerprint.as_slice(),
                content_hash.as_slice()
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
    /// Continues a copy of the previous publication, whose unchanged cells are kept.
    /// The caller checks that its header hash matches this cook's.
    pub fn open_incremental(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open(path)?;
        crate::storage::ensure_schema_version(
            &connection,
            world::RUNTIME_SCHEMA_VERSION,
            "runtime",
        )?;
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA cache_size=-8192; PRAGMA temp_store=FILE; BEGIN IMMEDIATE;",
        )?;
        Ok(Self { connection })
    }
    pub fn header_hash(&self) -> Result<[u8; 32], WorldDbError> {
        Ok(self.connection.query_row(
            "SELECT header_hash FROM runtime_metadata WHERE singleton=1",
            [],
            |r| crate::storage::blob_array(r.get_ref(0)?.as_blob()?, "header hash"),
        )?)
    }
    /// Source fingerprints of the cells already in this output.
    pub fn cell_fingerprints(
        &self,
        space: world::WorldSpaceId,
    ) -> Result<std::collections::BTreeMap<world::CellCoord, [u8; 32]>, WorldDbError> {
        let mut query = self.connection.prepare(
            "SELECT cell_x, cell_z, input_fingerprint FROM cells WHERE world_space_id=?1",
        )?;
        let rows = query.query_map([space.0], |r| {
            Ok((
                world::CellCoord {
                    x: r.get(0)?,
                    z: r.get(1)?,
                },
                crate::storage::blob_array(r.get_ref(2)?.as_blob()?, "input fingerprint")?,
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }
    /// Replaces the start view of a continued publication; it is not part of any hash.
    pub fn set_start_view(
        &self,
        view: Option<&world::WorldViewBookmark>,
    ) -> Result<(), WorldDbError> {
        self.connection.execute(
            "UPDATE runtime_metadata SET start_view=?1 WHERE singleton=1",
            [crate::storage::encode_start_view(view)?],
        )?;
        Ok(())
    }
    /// Replaces the gameplay areas of a continued publication. They are not part of the
    /// content hash, but `finish` folds them into the generation.
    pub fn set_gameplay_areas(&self, areas: &[world::GameplayArea]) -> Result<(), WorldDbError> {
        self.connection.execute(
            "UPDATE runtime_metadata SET gameplay_areas=?1 WHERE singleton=1",
            [crate::gameplay_areas::encode(areas)?],
        )?;
        Ok(())
    }
    /// Drops the ground composites so they can be baked again from the updated hierarchy.
    pub fn clear_terrain_composites(&self) -> Result<(), WorldDbError> {
        self.connection.execute_batch(
            "DELETE FROM terrain_composites; DELETE FROM terrain_cores;
             DELETE FROM terrain_material_spaces;",
        )?;
        Ok(())
    }
    pub fn page_checksum(&self, key: world::PageKey) -> Result<Option<[u8; 32]>, WorldDbError> {
        let checksum: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT checksum FROM cell_pages WHERE world_space_id=?1 AND cell_x=?2 AND \
                 cell_z=?3 AND domain=?4 AND lod=?5",
                params![
                    key.space.0,
                    key.cell.x,
                    key.cell.z,
                    key.domain as i64,
                    key.lod
                ],
                |r| r.get(0),
            )
            .optional()?;
        Ok(checksum
            .map(|c| crate::storage::blob_array(&c, "page checksum"))
            .transpose()?)
    }
    /// Removes a cell and every page row that depends on it.
    pub fn remove_cell(
        &self,
        space: world::WorldSpaceId,
        cell: world::CellCoord,
    ) -> Result<(), WorldDbError> {
        for table in [
            "page_dependencies",
            "page_object_definitions",
            "page_terrain_surfaces",
            "cell_pages",
            "cells",
        ] {
            self.connection.execute(
                &format!("DELETE FROM {table} WHERE world_space_id=?1 AND cell_x=?2 AND cell_z=?3"),
                params![space.0, cell.x, cell.z],
            )?;
        }
        Ok(())
    }
    pub fn append_cell(
        &self,
        batch: &RuntimeBuild,
        input_fingerprint: [u8; 32],
    ) -> Result<(), WorldDbError> {
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
        write_runtime_spatial(
            &self.connection,
            batch,
            input_fingerprint,
            batch.manifest.content_hash,
        )
    }
    /// Regroups the impostor-drawn objects of every static-object page into far-object
    /// blocks. Derived from cell pages and asset variants, it changes no content hash.
    pub fn rebuild_far_objects(&self) -> Result<super::FarObjectStats, WorldDbError> {
        super::far_objects::rebuild(&self.connection)
    }
    /// The generation's content hash folds the header with every cell in key order, read
    /// back from the output, so full and incremental cooks agree.
    pub fn finish(self) -> Result<RuntimeManifest, WorldDbError> {
        let mut hash = blake3::Hasher::new();
        hash.update(b"bounded-source-cook-v2");
        hash.update(&self.header_hash()?);
        {
            let mut query = self.connection.prepare(
                "SELECT world_space_id, cell_x, cell_z, content_hash, minimum_y, maximum_y, \
                        domain_mask, source_revision \
                 FROM cells ORDER BY world_space_id, cell_x, cell_z",
            )?;
            let mut rows = query.query([])?;
            while let Some(r) = rows.next()? {
                hash.update(&r.get::<_, i64>(0)?.to_le_bytes());
                hash.update(&r.get::<_, i32>(1)?.to_le_bytes());
                hash.update(&r.get::<_, i32>(2)?.to_le_bytes());
                hash.update(r.get_ref(3)?.as_blob().map_err(rusqlite::Error::from)?);
                hash.update(&r.get::<_, f32>(4)?.to_bits().to_le_bytes());
                hash.update(&r.get::<_, f32>(5)?.to_bits().to_le_bytes());
                hash.update(&r.get::<_, i64>(6)?.to_le_bytes());
                hash.update(&r.get::<_, i64>(7)?.to_le_bytes());
            }
        }
        let content_hash = *hash.finalize().as_bytes();
        let generation_id = crate::gameplay_areas::generation_id(&self.connection, &content_hash)?;
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
