//! Source snapshots and staged writes for the production cooker. Spatial sample data
//! is never collected for the whole world. All reads share one SQLite snapshot.
use super::*;
use environment::{CoverageSnapshot, EnvironmentDefinition};
use environment_compile::TerrainSource;

pub const COOK_KEY_BATCH: usize = 128;
pub const MAX_COOK_MANUAL_OBJECTS_PER_CELL: usize = 8192;
pub const MAX_COOK_CATALOG_ROWS: usize = 32768;
pub const MAX_COOK_CATALOG_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_COOK_CELL_BYTES: u64 = 32 * 1024 * 1024;

pub struct ProjectCookSnapshot {
    connection: Connection,
    // Only global definitions/catalogs; all spatial fields in this document are empty.
    catalog: ProjectDocument,
}
pub struct CookSourceCell {
    pub source: Option<SourceCellRecord>,
    pub coverage: CoverageSnapshot,
    pub terrain: TerrainSource,
    pub roads: RoadReadSnapshot,
    pub objects: Vec<SourceObjectRecord>,
}
impl ProjectCookSnapshot {
    pub fn open(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=1000; PRAGMA cache_size=-8192; PRAGMA temp_store=FILE; BEGIN DEFERRED;")?;
        ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;
        check_catalog_budget(&connection)?;
        if connection
            .prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_some()
        {
            return Err(invalid("source contains broken foreign-key references"));
        }
        let (assets, asset_variants, definitions) = query_source_catalog(&connection)?;
        let catalog = ProjectDocument {
            default_world_space: connection.query_row(
                "SELECT default_world_space_id FROM project_settings WHERE singleton=1",
                [],
                |r| Ok(WorldSpaceId(r.get(0)?)),
            )?,
            world_spaces: query_world_spaces(&connection)?,
            vegetation_catalog: read_vegetation_catalog(&connection)?,
            terrain_surfaces: query_all_terrain_surfaces(&connection)?,
            terrain_texture_sets: query_all_terrain_texture_sets(&connection)?,
            terrain_texture_layers: query_all_terrain_texture_layers(&connection)?,
            terrain_profiles: query_all_terrain_profiles(&connection)?,
            presets: environment_store::read_library(&connection)?,
            environments: environment_store::read_definitions(&connection)?,
            assets,
            asset_variants,
            definitions,
            cells: vec![],
            environment_cells: vec![],
            roads: Default::default(),
            terrain_cell_heightfields: vec![],
            objects: vec![],
        };
        for space in &catalog.world_spaces {
            if !space.cell_size.is_finite()
                || space.cell_size <= 0.0
                || !space.minimum_y.is_finite()
                || !space.maximum_y.is_finite()
                || space.minimum_y > space.maximum_y
            {
                return Err(invalid("invalid world bounds"));
            }
        }
        road_store::validate_cook_source(&connection, &catalog.presets)?;
        Ok(Self {
            connection,
            catalog,
        })
    }
    pub fn catalog(&self) -> &ProjectDocument {
        &self.catalog
    }
    /// Ordered union includes painted cells outside the terrain domain, so those masks
    /// still receive seam/reference validation. Both branches seek their primary keys.
    pub fn next_cells(
        &self,
        space: WorldSpaceId,
        after: Option<CellCoord>,
    ) -> Result<Vec<CellCoord>, WorldDbError> {
        let mut q=self.connection.prepare_cached(
            "SELECT cell_x,cell_z FROM source_cells WHERE world_space_id=?1 AND (cell_x,cell_z)>(?2,?3) \
             UNION SELECT cell_x,cell_z FROM environment_cells WHERE world_space_id=?1 AND (cell_x,cell_z)>(?2,?3) \
             ORDER BY cell_x,cell_z LIMIT ?4")?;
        Ok(q.query_map(
            params![
                space.0,
                after.map_or(i64::MIN, |c| i64::from(c.x)),
                after.map_or(i64::MIN, |c| i64::from(c.z)),
                COOK_KEY_BATCH as i64
            ],
            |r| {
                Ok(CellCoord {
                    x: r.get(0)?,
                    z: r.get(1)?,
                })
            },
        )?
        .collect::<Result<_, _>>()?)
    }
    pub fn read_cell(
        &self,
        definition: &EnvironmentDefinition,
        cell: CellCoord,
    ) -> Result<CookSourceCell, WorldDbError> {
        let space = definition.space;
        let halo = environment_dependency_cells(&[cell])?;
        let source = self
            .connection
            .query_row(
                "SELECT world_space_id,cell_x,cell_z,height,source_revision FROM source_cells \
             WHERE world_space_id=?1 AND cell_x=?2 AND cell_z=?3",
                params![space.0, cell.x, cell.z],
                source_cell_from_row,
            )
            .optional()?;
        let coverage = environment_store::read_coverage(&self.connection, definition, &halo)?;
        let terrain = road_store::read_terrain_source(&self.connection, space, &halo)?;
        let bounds = environment::roads::RoadCellBounds {
            minimum: CellCoord {
                x: cell.x - 1,
                z: cell.z - 1,
            },
            maximum: CellCoord {
                x: cell.x + 1,
                z: cell.z + 1,
            },
        };
        let roads = road_store::read_cook_roads(&self.connection, space, bounds)?;
        if roads.roads.truncated {
            return Err(invalid("road source query was truncated"));
        }
        let mut q=self.connection.prepare_cached(
            "SELECT object_id,world_space_id,owner_cell_x,owner_cell_z,definition_id, \
                 local_x,local_y,local_z,yaw,scale,source_revision FROM object_placements \
             WHERE world_space_id=?1 AND owner_cell_x=?2 AND owner_cell_z=?3 ORDER BY object_id LIMIT ?4")?;
        let objects = q
            .query_map(
                params![
                    space.0,
                    cell.x,
                    cell.z,
                    MAX_COOK_MANUAL_OBJECTS_PER_CELL as i64 + 1
                ],
                source_object_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if objects.len() > MAX_COOK_MANUAL_OBJECTS_PER_CELL {
            return Err(invalid("manual objects exceed per-cell cook budget"));
        }
        if objects.iter().any(|o| {
            o.source_revision < 0
                || !o.scale.is_finite()
                || o.scale <= 0.0
                || !o.yaw.is_finite()
                || !o.local_translation.iter().all(|v| v.is_finite())
        }) {
            return Err(invalid("invalid manual object transform/revision"));
        }
        Ok(CookSourceCell {
            source,
            coverage,
            terrain,
            roads,
            objects,
        })
    }
}

// Count rows and variable data before decoding anything. The remaining record data
// has a fixed size, so both metadata cardinality and variable allocations are bounded.
fn check_catalog_budget(c: &Connection) -> Result<(), WorldDbError> {
    let mut total_rows = 0_i64;
    let mut total_bytes = 0_i64;
    for (table, bytes) in [
        (
            "world_spaces",
            "length(CAST(name AS BLOB))+length(atmosphere)",
        ),
        ("vegetation_catalog", "length(payload)"),
        (
            "terrain_surfaces",
            "length(CAST(surface_key AS BLOB))+length(CAST(display_name AS BLOB))",
        ),
        (
            "terrain_texture_sets",
            "length(CAST(texture_set_key AS BLOB))+length(CAST(base_color_universal_uri AS BLOB))+length(CAST(normal_material_universal_uri AS BLOB))+length(CAST(macro_variation_universal_uri AS BLOB))+coalesce(length(CAST(base_color_astc_uri AS BLOB)),0)+coalesce(length(CAST(normal_material_astc_uri AS BLOB)),0)+coalesce(length(CAST(macro_variation_astc_uri AS BLOB)),0)",
        ),
        ("terrain_texture_set_layers", "0"),
        ("world_space_terrain_profiles", "0"),
        ("environment_presets", "length(payload)"),
        ("environment_definitions", "length(payload)"),
        (
            "source_assets",
            "length(CAST(asset_key AS BLOB))+length(CAST(kind AS BLOB))+length(CAST(source_uri AS BLOB))",
        ),
        ("source_asset_variants", "length(CAST(uri AS BLOB))"),
        (
            "object_definitions",
            "length(CAST(definition_key AS BLOB))+length(CAST(display_name AS BLOB))",
        ),
    ] {
        let (rows, bytes): (i64, i64) = c.query_row(
            &format!("SELECT count(*),coalesce(sum({bytes}),0) FROM {table}"),
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        total_rows += rows;
        total_bytes += bytes;
        if total_rows > MAX_COOK_CATALOG_ROWS as i64 || total_bytes > MAX_COOK_CATALOG_BYTES as i64
        {
            return Err(invalid("global cook catalog exceeds row/byte budget"));
        }
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
