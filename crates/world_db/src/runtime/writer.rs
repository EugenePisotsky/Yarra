//! Runtime output: a header, then cells one at a time. A whole runtime written at once and the
//! cook's staged output share the SQL.
use crate::catalog::{write_terrain_catalog, write_vegetation_catalog};
use crate::runtime::read_runtime_manifest;
use crate::storage::ensure_new_database_path;

use crate::{
    MAX_COOK_CATALOG_ROWS, MAX_COOK_CELL_BYTES, MAX_COOK_MANUAL_OBJECTS_PER_CELL, RuntimeCell,
    RuntimeHeader, RuntimeManifest, WorldDbError, atmosphere, schema,
};

use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use world::{CellCoord, PageDomain, PageKey, WorldSpaceId};

/// Writes a whole runtime at once, for fixtures. Its cells have no source fingerprints, so an
/// incremental cook treats them as unknown.
pub fn write_runtime_database(
    path: &Path,
    header: &RuntimeHeader,
    cells: &[RuntimeCell],
) -> Result<(), WorldDbError> {
    ensure_new_database_path(path)?;
    let mut connection = Connection::open(path)?;
    schema::create_runtime_schema(&connection)?;
    let transaction = connection.transaction()?;
    write_header(&transaction, header)?;

    for cell in cells {
        write_cell(&transaction, cell, [0; 32])?;
    }

    super::far_objects::rebuild(&transaction)?;
    transaction.commit()?;
    connection.execute_batch("PRAGMA optimize;")?;
    Ok(())
}

fn write_header(transaction: &Connection, header: &RuntimeHeader) -> Result<(), WorldDbError> {
    let manifest = &header.manifest;

    for space in &manifest.world_spaces {
        transaction.execute(
            "INSERT INTO world_spaces(id, name, cell_size, minimum_y, maximum_y, atmosphere, \
                atmosphere_revision, sea_level) \
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
            singleton, generation_id, content_hash, header_hash, default_world_space_id, \
            start_view, gameplay_areas \
         ) VALUES (1, ?1, ?2, ?2, ?3, ?4, ?5)",
        params![
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
        &header.terrain_surfaces,
        &header.terrain_texture_sets,
        &header.terrain_texture_layers,
        &header.terrain_profiles,
    )?;

    for asset in &header.assets {
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

    for definition in &header.definitions {
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

/// A cell's record, pages, what they name and its leaf control tile.
fn write_cell(
    transaction: &Connection,
    cell: &RuntimeCell,
    input_fingerprint: [u8; 32],
) -> Result<(), WorldDbError> {
    let record = &cell.record;
    let (space, x, z) = (record.space.0, record.cell.x, record.cell.z);
    transaction.execute(
        "INSERT INTO cells( \
            world_space_id, cell_x, cell_z, minimum_y, maximum_y, domain_mask, \
            source_revision, terrain_resolution, input_fingerprint, content_hash \
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            space,
            x,
            z,
            record.minimum_y,
            record.maximum_y,
            i64::try_from(record.domain_mask).map_err(|_| WorldDbError::IntegerOverflow)?,
            record.source_revision,
            record.terrain_resolution,
            input_fingerprint.as_slice(),
            cell.content_hash.as_slice()
        ],
    )?;

    for page in &cell.pages {
        transaction.execute(
            "INSERT INTO cell_pages( \
                world_space_id, cell_x, cell_z, domain, codec, encoded_bytes, \
                decoded_bytes, gpu_bytes_estimate, checksum, payload \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                space,
                x,
                z,
                page.key.domain as i64,
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

    for (asset, lod) in &cell.assets {
        transaction.execute(
            "INSERT INTO page_dependencies(world_space_id, cell_x, cell_z, asset_id, asset_lod) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![space, x, z, asset.0.as_slice(), i64::from(*lod)],
        )?;
    }

    for definition in &cell.definitions {
        transaction.execute(
            "INSERT INTO page_object_definitions(world_space_id, cell_x, cell_z, definition_id) \
             VALUES (?1, ?2, ?3, ?4)",
            params![space, x, z, definition.0.as_slice()],
        )?;
    }

    for surface in &cell.surfaces {
        transaction.execute(
            "INSERT INTO page_terrain_surfaces(world_space_id, cell_x, cell_z, surface_id) \
             VALUES (?1, ?2, ?3, ?4)",
            params![space, x, z, surface.0.as_slice()],
        )?;
    }

    transaction.execute(
        "INSERT INTO terrain_controls(world_space_id, level, node_x, node_z, payload) \
         VALUES (?1, 0, ?2, ?3, ?4)",
        params![space, x, z, cell.control],
    )?;
    Ok(())
}

/// Owns only the unpublished output; dropping without finish rolls back its writes.
pub struct RuntimeCookWriter {
    connection: Connection,
}

impl RuntimeCookWriter {
    pub fn create(path: &Path, header: &RuntimeHeader) -> Result<Self, WorldDbError> {
        ensure_new_database_path(path)?;
        let connection = Connection::open(path)?;
        schema::create_runtime_schema(&connection)?;
        connection
            .execute_batch("PRAGMA cache_size=-8192; PRAGMA temp_store=FILE; BEGIN IMMEDIATE;")?;
        write_header(&connection, header)?;
        Ok(Self { connection })
    }

    /// Continues a copy of the previous publication, whose unchanged cells are kept.
    /// The caller checks that its header hash matches this cook's.
    pub fn open_incremental(path: &Path) -> Result<Self, WorldDbError> {
        let connection = crate::storage::open_staging_runtime(path)?;
        connection.execute_batch("BEGIN IMMEDIATE;")?;
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
        space: WorldSpaceId,
    ) -> Result<std::collections::BTreeMap<CellCoord, [u8; 32]>, WorldDbError> {
        let mut query = self.connection.prepare(
            "SELECT cell_x, cell_z, input_fingerprint FROM cells WHERE world_space_id=?1",
        )?;
        let rows = query.query_map([space.0], |r| {
            Ok((
                CellCoord {
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

    pub fn page_checksum(&self, key: PageKey) -> Result<Option<[u8; 32]>, WorldDbError> {
        let checksum: Option<Vec<u8>> = self
            .connection
            .query_row(
                "SELECT checksum FROM cell_pages WHERE world_space_id=?1 AND cell_x=?2 AND \
                 cell_z=?3 AND domain=?4",
                params![key.space.0, key.cell.x, key.cell.z, key.domain as i64],
                |r| r.get(0),
            )
            .optional()?;
        Ok(checksum
            .map(|c| crate::storage::blob_array(&c, "page checksum"))
            .transpose()?)
    }

    /// Removes a cell, every row that depends on it and its leaf control tile.
    pub fn remove_cell(&self, space: WorldSpaceId, cell: CellCoord) -> Result<(), WorldDbError> {
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

        self.connection.execute(
            "DELETE FROM terrain_controls \
             WHERE world_space_id=?1 AND level=0 AND node_x=?2 AND node_z=?3",
            params![space.0, cell.x, cell.z],
        )?;
        Ok(())
    }

    pub fn append_cell(
        &self,
        cell: &RuntimeCell,
        input_fingerprint: [u8; 32],
    ) -> Result<(), WorldDbError> {
        let decoded = cell
            .pages
            .iter()
            .try_fold(0_u64, |bytes, page| bytes.checked_add(page.decoded_bytes));
        let encoded: u64 = cell.pages.iter().map(|p| p.payload.len() as u64).sum();

        if cell.pages.len() > PageDomain::ALL.len()
            || decoded.is_none_or(|bytes| bytes > MAX_COOK_CELL_BYTES)
            || encoded > MAX_COOK_CELL_BYTES
            || cell.assets.len() > MAX_COOK_CATALOG_ROWS
            || cell.definitions.len() > MAX_COOK_MANUAL_OBJECTS_PER_CELL
            || cell.surfaces.len() > world::MAX_TERRAIN_SURFACES_PER_CELL
        {
            return Err(invalid("runtime cell output exceeds cook budget"));
        }

        let record = &cell.record;

        if cell
            .pages
            .iter()
            .any(|p| p.key.space != record.space || p.key.cell != record.cell)
        {
            return Err(invalid("runtime page belongs to another cell"));
        }

        write_cell(&self.connection, cell, input_fingerprint)
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
