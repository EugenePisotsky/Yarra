//! In-place replacement of a world space's terrain for importers: heights and paint, written
//! cell by cell in one transaction. Revisions advance only where the data changed, so the next
//! incremental cook recompiles only those cells. Paint rows are never deleted, only cleared,
//! so a revision can never repeat with different contents.
use crate::environment_store::{read_cell, read_definition, store_cell};
use crate::storage::{decode_f32_blob, encode_f32_blob, ensure_schema_version};
use crate::{SourceEnvironmentCellRecord, WorldDbError};
use environment::{CoverageTile, EnvironmentDefinition};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use world::{CellCoord, PROJECT_SCHEMA_VERSION, WorldSpaceId};

fn invalid(message: impl Into<String>) -> WorldDbError {
    WorldDbError::Environment(message.into())
}

/// One cell's imported terrain.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTerrainCell {
    pub cell: CellCoord,
    /// The cell's height where it has no heightfield; otherwise its first sample.
    pub height: f32,
    /// `resolution²` samples, or `None` for an exactly flat cell.
    pub heights: Option<Vec<f32>>,
    pub resolution: u16,
    /// Coverage per environment layer; all-zero tiles are dropped.
    pub tiles: Vec<CoverageTile>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TerrainImportStats {
    pub cells: u64,
    pub added: u64,
    /// Existing cells whose heights or paint changed.
    pub changed: u64,
    /// Cells of the space the import no longer covers.
    pub removed: u64,
}

pub struct TerrainImportWriter {
    connection: Connection,
    space: WorldSpaceId,
    definition: EnvironmentDefinition,
    stats: TerrainImportStats,
}

impl TerrainImportWriter {
    /// Starts replacing the terrain of the project's default world space, whose height range
    /// becomes `bounds`.
    pub fn open(
        path: &Path,
        bounds: [f32; 2],
        sea_level: Option<f32>,
    ) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        ensure_schema_version(&connection, PROJECT_SCHEMA_VERSION, "project")?;
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; BEGIN IMMEDIATE;
             CREATE TEMP TABLE imported_cells(cell_x INTEGER, cell_z INTEGER,
                 PRIMARY KEY(cell_x, cell_z)) WITHOUT ROWID;",
        )?;
        let space = WorldSpaceId(connection.query_row(
            "SELECT default_world_space_id FROM project_settings WHERE singleton=1",
            [],
            |r| r.get(0),
        )?);
        let definition = read_definition(&connection, space)?
            .ok_or_else(|| invalid("imported terrain needs an environment definition"))?;
        // World settings are part of every cell's cook inputs: rewrite them only on change.
        connection.execute(
            "UPDATE world_spaces SET minimum_y=?2, maximum_y=?3, sea_level=?4 WHERE id=?1 \
             AND NOT (minimum_y IS ?2 AND maximum_y IS ?3 AND sea_level IS ?4)",
            params![space.0, bounds[0], bounds[1], sea_level],
        )?;
        Ok(Self {
            connection,
            space,
            definition,
            stats: TerrainImportStats::default(),
        })
    }

    pub fn definition(&self) -> &EnvironmentDefinition {
        &self.definition
    }

    pub fn put(&mut self, imported: &ImportedTerrainCell) -> Result<(), WorldDbError> {
        let space = self.space;
        let cell = imported.cell;
        if let Some(heights) = &imported.heights
            && (heights.len() != usize::from(imported.resolution).pow(2)
                || !(2..=257).contains(&imported.resolution))
        {
            return Err(invalid("imported heightfield size"));
        }
        let mask = usize::from(self.definition.mask_resolution).pow(2);
        if imported.tiles.iter().any(|t| {
            t.samples.len() != mask || !self.definition.layers.iter().any(|l| l.id == t.layer)
        }) {
            return Err(invalid(
                "imported paint does not match the environment definition",
            ));
        }
        let c = &self.connection;
        c.execute(
            "INSERT INTO imported_cells VALUES (?1, ?2)",
            params![cell.x, cell.z],
        )
        .map_err(|_| invalid("a cell was imported twice"))?;
        let source: Option<(f32, i64)> = c
            .query_row(
                "SELECT height, source_revision FROM source_cells \
                 WHERE world_space_id=?1 AND cell_x=?2 AND cell_z=?3",
                params![space.0, cell.x, cell.z],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let field: Option<(u16, Vec<f32>, i64)> = c
            .query_row(
                "SELECT resolution, heights, source_revision FROM terrain_cell_heightfields \
                 WHERE world_space_id=?1 AND cell_x=?2 AND cell_z=?3",
                params![space.0, cell.x, cell.z],
                |r| {
                    Ok((
                        r.get(0)?,
                        decode_f32_blob(r.get_ref(1)?.as_blob()?, "heights")?,
                        r.get(2)?,
                    ))
                },
            )
            .optional()?;
        let bits = |values: &[f32]| values.iter().map(|v| v.to_bits()).collect::<Vec<_>>();
        let same_terrain = source
            .is_some_and(|(height, _)| height.to_bits() == imported.height.to_bits())
            && match (&field, &imported.heights) {
                (None, None) => true,
                (Some((resolution, old, _)), Some(new)) => {
                    *resolution == imported.resolution && bits(old) == bits(new)
                }
                _ => false,
            };
        let mut changed = false;
        if !same_terrain {
            // One revision covers the cell and its heightfield, above both previous ones.
            let revision = source
                .map(|(_, r)| r)
                .max(field.as_ref().map(|f| f.2))
                .map_or(1, |r| r + 1);
            c.execute(
                "INSERT INTO source_cells(world_space_id, cell_x, cell_z, height, source_revision) \
                 VALUES (?1, ?2, ?3, ?4, ?5) ON CONFLICT(world_space_id, cell_x, cell_z) \
                 DO UPDATE SET height=excluded.height, source_revision=excluded.source_revision",
                params![space.0, cell.x, cell.z, imported.height, revision],
            )?;
            c.execute(
                "DELETE FROM terrain_cell_heightfields \
                 WHERE world_space_id=?1 AND cell_x=?2 AND cell_z=?3",
                params![space.0, cell.x, cell.z],
            )?;
            if let Some(heights) = &imported.heights {
                c.execute(
                    "INSERT INTO terrain_cell_heightfields(world_space_id, cell_x, cell_z, \
                     resolution, heights, source_revision) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        space.0,
                        cell.x,
                        cell.z,
                        imported.resolution,
                        encode_f32_blob(heights),
                        revision
                    ],
                )?;
            }
            changed = true;
        }
        let mut tiles: Vec<_> = imported
            .tiles
            .iter()
            .filter(|t| t.samples.iter().any(|&s| s != 0))
            .cloned()
            .collect();
        tiles.sort_by_key(|t| t.layer.0);
        let mut budget = usize::MAX;
        let paint = read_cell(c, &self.definition, cell, &mut budget)?;
        let same_paint = match &paint {
            Some(old) => old.tiles == tiles,
            None => tiles.is_empty(),
        };
        if !same_paint {
            store_cell(
                c,
                &SourceEnvironmentCellRecord {
                    space,
                    cell,
                    source_revision: paint.map_or(1, |p| p.source_revision + 1),
                    definition_revision: self.definition.revision,
                    tiles,
                },
            )?;
            changed = true;
        }
        self.stats.cells += 1;
        if source.is_none() {
            self.stats.added += 1;
        } else if changed {
            self.stats.changed += 1;
        }
        Ok(())
    }

    /// Where play starts without an explicit start view.
    pub fn set_start_view(&self, view: &world::WorldViewBookmark) -> Result<(), WorldDbError> {
        self.connection.execute(
            "UPDATE project_settings SET start_view=?1 WHERE singleton=1",
            [crate::storage::encode_start_view(Some(view))?],
        )?;
        Ok(())
    }

    /// Removes the space's cells this import did not write, and commits.
    pub fn finish(mut self) -> Result<TerrainImportStats, WorldDbError> {
        let c = &self.connection;
        let space = self.space.0;
        let stale = "NOT EXISTS (SELECT 1 FROM imported_cells i \
                     WHERE i.cell_x = t.cell_x AND i.cell_z = t.cell_z)";
        // Coverage and heightfields cascade; a cell that still owns objects or roads fails.
        c.execute(
            &format!("DELETE FROM environment_cells AS t WHERE world_space_id=?1 AND {stale}"),
            [space],
        )?;
        self.stats.removed = c.execute(
            &format!("DELETE FROM source_cells AS t WHERE world_space_id=?1 AND {stale}"),
            [space],
        )? as u64;
        c.execute_batch("DROP TABLE imported_cells; COMMIT;")?;
        Ok(self.stats)
    }
}
