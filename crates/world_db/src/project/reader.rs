use crate::catalog::{query_world_spaces, read_vegetation_catalog};
use crate::project::query::{
    query_sql_limit, source_cell_from_row, source_object_from_row, source_object_palette_from_row,
    source_object_view_from_row, validate_spatial_query,
};
use crate::storage::ensure_schema_version;
use crate::{
    ProjectManifest, SourceCellQuery, SourceObjectOutlinerCursor, SourceObjectOutlinerPage,
    SourceObjectPaletteCursor, SourceObjectPalettePage, SourceObjectPaletteQuery,
    SourceObjectQuery, SourceObjectRecord, SourceObjectViewQuery, SourceObjectViewRecord,
    WorldDbError,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use vegetation::VegetationCatalog;
use world::{CellCoord, PROJECT_SCHEMA_VERSION, StableObjectId, WorldSpaceId};

/// Read-only, query-shaped access to a mutable authoring database.
///
/// Unlike [`crate::read_project_database`], this reader never constructs a complete project document.
/// It is intended to live on an editor worker thread and return explicitly bounded spatial results.
pub struct ProjectReader {
    pub(crate) connection: Connection,
    manifest: ProjectManifest,
    has_spatial_object_overlap_index: bool,
}

impl ProjectReader {
    pub fn open_read_only(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        connection.execute_batch(
            "PRAGMA query_only = ON; PRAGMA foreign_keys = ON; PRAGMA busy_timeout = 250;",
        )?;
        ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;
        let world_spaces = query_world_spaces(&connection)?;
        let default_world_space = connection.query_row(
            "SELECT default_world_space_id FROM project_settings WHERE singleton = 1",
            [],
            |row| Ok(WorldSpaceId(row.get(0)?)),
        )?;
        if !world_spaces
            .iter()
            .any(|space| space.id == default_world_space)
        {
            return Err(WorldDbError::UnknownDefaultWorldSpace(default_world_space));
        }
        let has_spatial_object_overlap_index = connection.query_row(
            "SELECT EXISTS( \
                SELECT 1 FROM pragma_index_list('object_cell_overlaps') \
                WHERE name = 'object_cell_overlaps_cells' \
             )",
            [],
            |row| row.get(0),
        )?;
        Ok(Self {
            connection,
            manifest: ProjectManifest {
                schema_version: PROJECT_SCHEMA_VERSION,
                default_world_space,
                world_spaces,
            },
            has_spatial_object_overlap_index,
        })
    }

    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    /// Reads the generation-global vegetation source catalog without constructing the spatial
    /// project document. Editors keep this small global domain separate from bounded cell queries.
    pub fn read_vegetation_catalog(&self) -> Result<Option<VegetationCatalog>, WorldDbError> {
        read_vegetation_catalog(&self.connection)
    }

    pub fn read_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceCellQuery, WorldDbError> {
        validate_spatial_query(minimum, maximum, maximum_records)?;
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT world_space_id, cell_x, cell_z, height, source_revision \
             FROM source_cells \
             WHERE world_space_id = ?1 \
               AND cell_x BETWEEN ?2 AND ?3 \
               AND cell_z BETWEEN ?4 AND ?5 \
             ORDER BY cell_x, cell_z \
             LIMIT ?6",
        )?;
        let mut records = statement
            .query_map(
                params![
                    space.0, minimum.x, maximum.x, minimum.z, maximum.z, sql_limit
                ],
                source_cell_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceCellQuery { records, truncated })
    }

    pub fn read_objects_in_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceObjectQuery, WorldDbError> {
        validate_spatial_query(minimum, maximum, maximum_records)?;
        let sql_limit = query_sql_limit(maximum_records)?;
        let sql = if self.has_spatial_object_overlap_index {
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision \
             FROM object_placements o \
             WHERE o.world_space_id = ?1 \
               AND o.owner_cell_x BETWEEN ?2 AND ?3 \
               AND o.owner_cell_z BETWEEN ?4 AND ?5 \
             UNION \
             SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision \
             FROM object_cell_overlaps overlap \
             JOIN object_placements o ON o.object_id = overlap.object_id \
             WHERE overlap.world_space_id = ?1 \
               AND overlap.cell_x BETWEEN ?2 AND ?3 \
               AND overlap.cell_z BETWEEN ?4 AND ?5 \
             ORDER BY object_id \
             LIMIT ?6"
        } else {
            // Early version-7 databases do not have the optional overlap spatial index.
            // Owner-cell queries remain indexed; scanning every object in a space does not.
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision \
             FROM object_placements o \
             WHERE o.world_space_id = ?1 \
               AND o.owner_cell_x BETWEEN ?2 AND ?3 \
               AND o.owner_cell_z BETWEEN ?4 AND ?5 \
             ORDER BY o.object_id \
             LIMIT ?6"
        };
        let mut statement = self.connection.prepare_cached(sql)?;
        let mut records = statement
            .query_map(
                params![
                    space.0, minimum.x, maximum.x, minimum.z, maximum.z, sql_limit
                ],
                source_object_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceObjectQuery { records, truncated })
    }

    pub fn read_object(
        &self,
        object: StableObjectId,
    ) -> Result<Option<SourceObjectRecord>, WorldDbError> {
        self.connection
            .query_row(
                "SELECT object_id, world_space_id, owner_cell_x, owner_cell_z, definition_id, \
                        local_x, local_y, local_z, yaw, scale, source_revision \
                 FROM object_placements WHERE object_id = ?1",
                params![object.0.as_slice()],
                source_object_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn read_object_views_in_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceObjectViewQuery, WorldDbError> {
        validate_spatial_query(minimum, maximum, maximum_records)?;
        let sql_limit = query_sql_limit(maximum_records)?;
        let sql = if self.has_spatial_object_overlap_index {
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_placements o \
             JOIN object_definitions d ON d.definition_id = o.definition_id \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE o.world_space_id = ?1 \
               AND o.owner_cell_x BETWEEN ?2 AND ?3 \
               AND o.owner_cell_z BETWEEN ?4 AND ?5 \
             UNION \
             SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_cell_overlaps overlap \
             JOIN object_placements o ON o.object_id = overlap.object_id \
             JOIN object_definitions d ON d.definition_id = o.definition_id \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE overlap.world_space_id = ?1 \
               AND overlap.cell_x BETWEEN ?2 AND ?3 \
               AND overlap.cell_z BETWEEN ?4 AND ?5 \
             ORDER BY object_id \
             LIMIT ?6"
        } else {
            // Early version-7 databases do not have the optional overlap spatial index.
            // Owner-cell queries remain indexed; scanning every object in a space does not.
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_placements o \
             JOIN object_definitions d ON d.definition_id = o.definition_id \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE o.world_space_id = ?1 \
               AND o.owner_cell_x BETWEEN ?2 AND ?3 \
               AND o.owner_cell_z BETWEEN ?4 AND ?5 \
             ORDER BY o.object_id \
             LIMIT ?6"
        };
        let mut statement = self.connection.prepare_cached(sql)?;
        let mut records = statement
            .query_map(
                params![
                    space.0, minimum.x, maximum.x, minimum.z, maximum.z, sql_limit
                ],
                source_object_view_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceObjectViewQuery { records, truncated })
    }

    pub fn read_object_view(
        &self,
        object: StableObjectId,
    ) -> Result<Option<SourceObjectViewRecord>, WorldDbError> {
        self.connection
            .query_row(
                "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                        o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                        o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                        d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
                 FROM object_placements o \
                 JOIN object_definitions d ON d.definition_id = o.definition_id \
                 LEFT JOIN source_asset_variants v \
                        ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
                 WHERE o.object_id = ?1",
                params![object.0.as_slice()],
                source_object_view_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    /// Reads one stable owner-cell-ordered outliner page without materializing the project tree.
    pub fn read_object_outliner_page(
        &self,
        space: WorldSpaceId,
        search: &str,
        cursor: Option<&SourceObjectOutlinerCursor>,
        maximum_records: usize,
    ) -> Result<SourceObjectOutlinerPage, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let cursor_x = cursor.map(|cursor| cursor.owner_cell.x);
        let cursor_z = cursor.map(|cursor| cursor.owner_cell.z);
        let cursor_object = cursor.map(|cursor| cursor.object.0.to_vec());
        let mut statement = self.connection.prepare_cached(
            "SELECT o.object_id, o.world_space_id, o.owner_cell_x, o.owner_cell_z, \
                    o.definition_id, o.local_x, o.local_y, o.local_z, o.yaw, o.scale, \
                    o.source_revision, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_placements o \
             JOIN object_definitions d ON d.definition_id = o.definition_id \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE o.world_space_id = ?1 \
               AND (?2 = '' OR instr(lower(d.definition_key), lower(?2)) > 0 \
                            OR instr(lower(d.display_name), lower(?2)) > 0) \
               AND (?3 IS NULL OR o.owner_cell_x > ?3 \
                    OR (o.owner_cell_x = ?3 AND o.owner_cell_z > ?4) \
                    OR (o.owner_cell_x = ?3 AND o.owner_cell_z = ?4 AND o.object_id > ?5)) \
             ORDER BY o.owner_cell_x, o.owner_cell_z, o.object_id \
             LIMIT ?6",
        )?;
        let mut records = statement
            .query_map(
                params![
                    space.0,
                    search.trim(),
                    cursor_x,
                    cursor_z,
                    cursor_object,
                    sql_limit
                ],
                source_object_view_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let has_next = records.len() > maximum_records;
        records.truncate(maximum_records);
        let next_cursor = has_next.then(|| {
            let last = records
                .last()
                .expect("a truncated query page has at least one retained record");
            SourceObjectOutlinerCursor {
                owner_cell: last.object.owner_cell,
                object: last.object.id,
            }
        });
        Ok(SourceObjectOutlinerPage {
            records,
            next_cursor,
        })
    }

    pub fn read_object_palette(
        &self,
        maximum_records: usize,
    ) -> Result<SourceObjectPaletteQuery, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let mut statement = self.connection.prepare_cached(
            "SELECT d.definition_id, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_definitions d \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             ORDER BY d.definition_key \
             LIMIT ?1",
        )?;
        let mut records = statement
            .query_map(params![sql_limit], source_object_palette_from_row)?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = records.len() > maximum_records;
        records.truncate(maximum_records);
        Ok(SourceObjectPaletteQuery { records, truncated })
    }

    /// Reads one searchable definition page using a stable key/ID cursor.
    pub fn read_object_palette_page(
        &self,
        search: &str,
        cursor: Option<&SourceObjectPaletteCursor>,
        maximum_records: usize,
    ) -> Result<SourceObjectPalettePage, WorldDbError> {
        if maximum_records == 0 {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let sql_limit = query_sql_limit(maximum_records)?;
        let cursor_key = cursor.map(|cursor| cursor.definition_key.as_str());
        let cursor_definition = cursor.map(|cursor| cursor.definition.0.to_vec());
        let mut statement = self.connection.prepare_cached(
            "SELECT d.definition_id, d.definition_key, d.display_name, d.visual_asset_id, \
                    d.activation_policy, v.uri, v.bounds_x, v.bounds_y, v.bounds_z \
             FROM object_definitions d \
             LEFT JOIN source_asset_variants v \
                    ON v.asset_id = d.visual_asset_id AND v.lod = 0 \
             WHERE (?1 = '' OR instr(lower(d.definition_key), lower(?1)) > 0 \
                             OR instr(lower(d.display_name), lower(?1)) > 0) \
               AND (?2 IS NULL OR d.definition_key > ?2 \
                    OR (d.definition_key = ?2 AND d.definition_id > ?3)) \
             ORDER BY d.definition_key, d.definition_id \
             LIMIT ?4",
        )?;
        let mut records = statement
            .query_map(
                params![search.trim(), cursor_key, cursor_definition, sql_limit],
                source_object_palette_from_row,
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let has_next = records.len() > maximum_records;
        records.truncate(maximum_records);
        let next_cursor = has_next.then(|| {
            let last = records
                .last()
                .expect("a truncated query page has at least one retained record");
            SourceObjectPaletteCursor {
                definition_key: last.definition.key.clone(),
                definition: last.definition.id,
            }
        });
        Ok(SourceObjectPalettePage {
            records,
            next_cursor,
        })
    }
}
