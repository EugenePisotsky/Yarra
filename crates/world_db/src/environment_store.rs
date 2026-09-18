//! Environment source storage. Render weights and vegetation fields are never authoring records.
//!
//! Reads use one SQLite snapshot. Writes compare definition and cell revisions in an IMMEDIATE
//! transaction, validate the shared mask borders, and commit the entire gesture or nothing.

use std::collections::{BTreeMap, BTreeSet};

use environment::{
    CoverageCell, CoverageSnapshot, CoverageTile, EnvironmentDefinition, PresetLibrary,
};
use environment_compile::{CompilePlan, CompileProfile};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use vegetation::VegetationCatalog;
use world::{CellCoord, WorldSpaceId};

use crate::{ProjectDocument, ProjectReader, ProjectWriter, WorldDbError, read_vegetation_catalog};

pub const MAX_ENVIRONMENT_DEFINITIONS: usize = 32;
pub const MAX_ENVIRONMENT_READ_CELLS: usize = 576;
pub const MAX_ENVIRONMENT_MASK_BYTES: usize = 4 * 1024 * 1024;
const MAX_DEFINITION_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceEnvironmentCellRecord {
    pub space: WorldSpaceId,
    pub cell: CellCoord,
    pub source_revision: i64,
    /// Revision of the small definition catalog used to interpret these masks.
    pub definition_revision: u64,
    pub tiles: Vec<CoverageTile>,
}

impl SourceEnvironmentCellRecord {
    pub fn coverage(&self) -> CoverageCell {
        CoverageCell {
            cell: self.cell,
            revision: self.source_revision as u64,
            tiles: self.tiles.clone(),
        }
    }

    pub fn sample_bytes(&self) -> usize {
        self.tiles.iter().map(|tile| tile.samples.len()).sum()
    }
}

/// Keyset page of cells affected by one layer, for bounded invalidation after a definition edit.
#[derive(Debug, Clone)]
pub struct EnvironmentLayerCells {
    pub definition_revision: u64,
    pub cells: Vec<CellCoord>,
    pub next_cursor: Option<CellCoord>,
}

#[derive(Debug, Clone)]
pub struct SourceEnvironmentCellQuery {
    pub records: Vec<SourceEnvironmentCellRecord>,
    pub truncated: bool,
}

/// Definitions, plants and requested coverage from a single database read transaction.
#[derive(Debug, Clone)]
pub struct EnvironmentReadSnapshot {
    pub definition: EnvironmentDefinition,
    pub presets: PresetLibrary,
    pub vegetation_catalog: VegetationCatalog,
    pub coverage: CoverageSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum DenseSourceRecordKey {
    EnvironmentCoverage {
        space: WorldSpaceId,
        cell: CellCoord,
    },
}

#[derive(Debug, Clone)]
pub enum DenseSourceWrite {
    EnvironmentCoverage {
        /// None creates a cell; clearing an existing cell retains its revision tombstone.
        expected_source_revision: Option<i64>,
        record: SourceEnvironmentCellRecord,
    },
}

impl DenseSourceWrite {
    pub fn key(&self) -> DenseSourceRecordKey {
        match self {
            Self::EnvironmentCoverage { record, .. } => DenseSourceRecordKey::EnvironmentCoverage {
                space: record.space,
                cell: record.cell,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseSourceRecord {
    EnvironmentCoverage(SourceEnvironmentCellRecord),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DenseSourceWriteTransactionResult {
    Committed(Vec<DenseSourceRecord>),
    Conflict {
        key: DenseSourceRecordKey,
        actual: Option<DenseSourceRecord>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EnvironmentDefinitionWriteResult {
    Committed(EnvironmentDefinition),
    Conflict {
        actual: Option<EnvironmentDefinition>,
    },
}

fn invalid(message: impl Into<String>) -> WorldDbError {
    WorldDbError::Environment(message.into())
}

fn checked_definition(
    definition: &EnvironmentDefinition,
    plants: &VegetationCatalog,
    presets: &PresetLibrary,
) -> Result<(), WorldDbError> {
    CompilePlan::new(definition, plants, presets, CompileProfile::default())?;
    Ok(())
}

pub(crate) fn read_definition(
    connection: &Connection,
    space: WorldSpaceId,
) -> Result<Option<EnvironmentDefinition>, WorldDbError> {
    let raw = connection
        .query_row(
            "SELECT revision, payload FROM environment_definitions WHERE world_space_id = ?1",
            [space.0],
            |row| {
                let payload = row.get_ref(1)?.as_blob().map_err(rusqlite::Error::from)?;
                if payload.len() > MAX_DEFINITION_BYTES {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok((row.get::<_, i64>(0)?, payload.to_vec()))
            },
        )
        .optional()?;
    raw.map(|(revision, bytes)| {
        let (definition, consumed): (EnvironmentDefinition, usize) =
            bincode::serde::decode_from_slice(
                &bytes,
                bincode::config::standard()
                    .with_little_endian()
                    .with_fixed_int_encoding()
                    .with_limit::<MAX_DEFINITION_BYTES>(),
            )?;
        if consumed != bytes.len()
            || definition.space != space
            || revision <= 0
            || definition.revision != revision as u64
        {
            return Err(invalid(
                "definition payload does not match its database header",
            ));
        }
        Ok(definition)
    })
    .transpose()
}

pub(crate) fn read_definitions(
    connection: &Connection,
) -> Result<Vec<EnvironmentDefinition>, WorldDbError> {
    let mut query = connection.prepare(
        "SELECT world_space_id FROM environment_definitions ORDER BY world_space_id LIMIT ?1",
    )?;
    let spaces = query
        .query_map([MAX_ENVIRONMENT_DEFINITIONS as i64 + 1], |row| {
            Ok(WorldSpaceId(row.get(0)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if spaces.len() > MAX_ENVIRONMENT_DEFINITIONS {
        return Err(invalid("too many environment definitions"));
    }
    spaces
        .into_iter()
        .map(|space| {
            read_definition(connection, space)?.ok_or_else(|| invalid("definition disappeared"))
        })
        .collect()
}

fn validate_definition_references(
    connection: &Connection,
    definition: &EnvironmentDefinition,
) -> Result<(), WorldDbError> {
    let cell_size: f32 = connection.query_row(
        "SELECT cell_size FROM world_spaces WHERE id = ?1",
        [definition.space.0],
        |row| row.get(0),
    )?;
    if definition.cell_size != cell_size {
        return Err(invalid("environment and world grids differ"));
    }
    for surface in &definition.surfaces {
        let exists: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM terrain_surfaces WHERE surface_id = ?1)",
            [surface.0.as_slice()],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(invalid("environment references a missing terrain surface"));
        }
        let unavailable: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM world_space_terrain_profiles p WHERE p.world_space_id = ?1 AND NOT EXISTS(SELECT 1 FROM terrain_texture_set_layers l WHERE l.texture_set_id = p.texture_set_id AND l.surface_id = ?2))", params![definition.space.0, surface.0.as_slice()], |row| row.get(0))?;
        if unavailable {
            return Err(invalid(
                "environment surface is unavailable in the world's terrain texture set",
            ));
        }
    }
    Ok(())
}

fn store_definition(
    connection: &Connection,
    definition: &EnvironmentDefinition,
) -> Result<(), WorldDbError> {
    let revision = i64::try_from(definition.revision).map_err(|_| WorldDbError::IntegerOverflow)?;
    let bytes = bincode::serde::encode_to_vec(
        definition,
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding(),
    )?;
    if bytes.len() > MAX_DEFINITION_BYTES || revision <= 0 {
        return Err(invalid("definition size or revision is invalid"));
    }
    connection.execute("INSERT INTO environment_definitions(world_space_id, revision, payload) VALUES (?1, ?2, ?3) ON CONFLICT(world_space_id) DO UPDATE SET revision = excluded.revision, payload = excluded.payload", params![definition.space.0, revision, bytes])?;
    connection.execute(
        "DELETE FROM environment_layer_presets WHERE world_space_id=?1",
        [definition.space.0],
    )?;
    for layer in &definition.layers {
        connection.execute("INSERT INTO environment_layer_presets(world_space_id,layer_id,preset_id) VALUES (?1,?2,?3)",params![definition.space.0,layer.id.0.as_slice(),layer.preset.0.as_slice()])?;
    }

    Ok(())
}

fn read_cell(
    connection: &Connection,
    definition: &EnvironmentDefinition,
    cell: CellCoord,
    remaining_bytes: &mut usize,
) -> Result<Option<SourceEnvironmentCellRecord>, WorldDbError> {
    let revision = connection.query_row("SELECT revision FROM environment_cells WHERE world_space_id = ?1 AND cell_x = ?2 AND cell_z = ?3", params![definition.space.0, cell.x, cell.z], |row| row.get::<_, i64>(0)).optional()?;
    let Some(source_revision) = revision else {
        return Ok(None);
    };
    let mut query = connection.prepare_cached("SELECT layer_id, samples FROM environment_coverage WHERE world_space_id = ?1 AND cell_x = ?2 AND cell_z = ?3 ORDER BY layer_id")?;
    let mut rows = query.query(params![definition.space.0, cell.x, cell.z])?;
    let mut tiles = Vec::new();
    while let Some(row) = rows.next()? {
        let samples = row.get_ref(1)?.as_blob().map_err(rusqlite::Error::from)?;
        if tiles.len() >= definition.layers.len()
            || samples.len() != usize::from(definition.mask_resolution).pow(2)
            || samples.len() > *remaining_bytes
        {
            return Err(invalid(
                "coverage read exceeded its byte budget or contains malformed tiles",
            ));
        }
        let layer = environment::LayerId(crate::blob_array(
            row.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?,
            "layer_id",
        )?);
        if !definition.layers.iter().any(|l| l.id == layer) {
            return Err(invalid("coverage references an unknown layer"));
        }
        *remaining_bytes -= samples.len();
        tiles.push(CoverageTile {
            layer,
            samples: samples.to_vec(),
        });
    }
    Ok(Some(SourceEnvironmentCellRecord {
        space: definition.space,
        cell,
        source_revision,
        definition_revision: definition.revision,
        tiles,
    }))
}

/// The caller has explicitly requested each coordinate. Missing rows are therefore known empty;
/// this never infers empty cells from a truncated spatial result.
fn read_coverage(
    connection: &Connection,
    definition: &EnvironmentDefinition,
    cells: &[CellCoord],
) -> Result<CoverageSnapshot, WorldDbError> {
    if cells.len() > MAX_ENVIRONMENT_READ_CELLS
        || cells.iter().collect::<BTreeSet<_>>().len() != cells.len()
    {
        return Err(invalid(
            "coverage read requires unique coordinates within the cell budget",
        ));
    }
    let mut remaining = MAX_ENVIRONMENT_MASK_BYTES;
    let cells = cells
        .iter()
        .map(|&cell| {
            Ok(
                read_cell(connection, definition, cell, &mut remaining)?.map_or(
                    CoverageCell {
                        cell,
                        revision: 0,
                        tiles: Vec::new(),
                    },
                    |record| record.coverage(),
                ),
            )
        })
        .collect::<Result<Vec<_>, WorldDbError>>()?;
    Ok(CoverageSnapshot {
        space: definition.space,
        cells,
    })
}

/// Coordinates needed to compile or validate these cells, including shared edge/corner owners.
pub fn environment_dependency_cells(cells: &[CellCoord]) -> Result<Vec<CellCoord>, WorldDbError> {
    if cells.len() > crate::MAX_DENSE_DOMAIN_WRITES_PER_TRANSACTION {
        return Err(invalid("too many environment cells in one job"));
    }
    let mut result = BTreeSet::new();
    for cell in cells {
        for dx in -1..=1 {
            for dz in -1..=1 {
                result.insert(CellCoord {
                    x: cell
                        .x
                        .checked_add(dx)
                        .ok_or_else(|| invalid("cell halo overflows"))?,
                    z: cell
                        .z
                        .checked_add(dz)
                        .ok_or_else(|| invalid("cell halo overflows"))?,
                });
            }
        }
    }
    Ok(result.into_iter().collect())
}

impl ProjectReader {
    pub fn read_environment_catalog(
        &self,
    ) -> Result<
        (
            Option<VegetationCatalog>,
            PresetLibrary,
            Vec<EnvironmentDefinition>,
        ),
        WorldDbError,
    > {
        let transaction = self.connection.unchecked_transaction()?;
        let plants = read_vegetation_catalog(&transaction)?;
        let presets = read_library(&transaction)?;
        let definitions = read_definitions(&transaction)?;
        let empty = VegetationCatalog {
            species: Vec::new(),
            populations: Vec::new(),
            assemblages: Vec::new(),
        };
        validate_library_references(&transaction, &presets, plants.as_ref().unwrap_or(&empty))?;
        for definition in &definitions {
            checked_definition(definition, plants.as_ref().unwrap_or(&empty), &presets)?;
        }
        Ok((plants, presets, definitions))
    }

    pub fn read_environment_layer_cells(
        &self,
        space: WorldSpaceId,
        layer: environment::LayerId,
        after: Option<CellCoord>,
        maximum_records: usize,
    ) -> Result<EnvironmentLayerCells, WorldDbError> {
        if maximum_records == 0 || maximum_records > MAX_ENVIRONMENT_READ_CELLS {
            return Err(invalid("layer dependency query exceeds the cell budget"));
        }
        let transaction = self.connection.unchecked_transaction()?;
        let definition = read_definition(&transaction, space)?
            .ok_or_else(|| invalid("world has no environment definition"))?;
        if !definition.layers.iter().any(|l| l.id == layer) {
            return Err(invalid(
                "layer dependency query references an unknown layer",
            ));
        }
        let mut query = transaction.prepare("SELECT cell_x, cell_z FROM environment_coverage WHERE world_space_id = ?1 AND layer_id = ?2 AND (cell_x, cell_z) >= (?3, ?4) ORDER BY cell_x, cell_z LIMIT ?5")?;
        let minimum = after.unwrap_or(CellCoord {
            x: i32::MIN,
            z: i32::MIN,
        });
        // Include the cursor row in the index range, then skip it; it may have been erased since
        // the previous page. The definition revision lets the caller reject obsolete work.
        let mut cells = query
            .query_map(
                params![
                    space.0,
                    layer.0.as_slice(),
                    minimum.x,
                    minimum.z,
                    maximum_records as i64 + 2
                ],
                |row| {
                    Ok(CellCoord {
                        x: row.get(0)?,
                        z: row.get(1)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        if let Some(after) = after {
            cells.retain(|&cell| cell != after);
        }
        let truncated = cells.len() > maximum_records;
        cells.truncate(maximum_records);
        let next_cursor = if truncated {
            cells.last().copied()
        } else {
            None
        };
        Ok(EnvironmentLayerCells {
            definition_revision: definition.revision,
            cells,
            next_cursor,
        })
    }

    pub fn read_environment_definitions(&self) -> Result<Vec<EnvironmentDefinition>, WorldDbError> {
        let transaction = self.connection.unchecked_transaction()?;
        read_definitions(&transaction)
    }

    pub fn read_environment_snapshot(
        &self,
        space: WorldSpaceId,
        cells: &[CellCoord],
    ) -> Result<EnvironmentReadSnapshot, WorldDbError> {
        let transaction = self.connection.unchecked_transaction()?;
        read_snapshot(&transaction, space, cells)
    }

    pub fn read_environment_cells_in_cells(
        &self,
        space: WorldSpaceId,
        minimum: CellCoord,
        maximum: CellCoord,
        maximum_records: usize,
    ) -> Result<SourceEnvironmentCellQuery, WorldDbError> {
        crate::validate_spatial_query(minimum, maximum, maximum_records)?;
        if maximum_records > MAX_ENVIRONMENT_READ_CELLS {
            return Err(invalid("environment query exceeds the cell budget"));
        }
        let width = i64::from(maximum.x) - i64::from(minimum.x) + 1;
        let height = i64::from(maximum.z) - i64::from(minimum.z) + 1;
        if width
            .checked_mul(height)
            .is_none_or(|area| area > MAX_ENVIRONMENT_READ_CELLS as i64)
        {
            return Err(invalid("environment query exceeds the spatial area budget"));
        }
        let transaction = self.connection.unchecked_transaction()?;
        let Some(definition) = read_definition(&transaction, space)? else {
            return Ok(SourceEnvironmentCellQuery {
                records: Vec::new(),
                truncated: false,
            });
        };
        let mut query = transaction.prepare("SELECT cell_x, cell_z FROM environment_cells WHERE world_space_id = ?1 AND cell_x BETWEEN ?2 AND ?3 AND cell_z BETWEEN ?4 AND ?5 ORDER BY cell_x, cell_z LIMIT ?6")?;
        let mut cells = query
            .query_map(
                params![
                    space.0,
                    minimum.x,
                    maximum.x,
                    minimum.z,
                    maximum.z,
                    maximum_records as i64 + 1
                ],
                |row| {
                    Ok(CellCoord {
                        x: row.get(0)?,
                        z: row.get(1)?,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        let truncated = cells.len() > maximum_records;
        cells.truncate(maximum_records);
        let mut remaining = MAX_ENVIRONMENT_MASK_BYTES;
        let records = cells
            .into_iter()
            .map(|cell| {
                read_cell(&transaction, &definition, cell, &mut remaining)?
                    .ok_or_else(|| invalid("cell disappeared"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(SourceEnvironmentCellQuery { records, truncated })
    }
}

mod presence;
pub use presence::{EnvironmentLayerPresence, MAX_ENVIRONMENT_PRESENCE_ROWS};
mod presets;
pub use presets::EnvironmentPresetLayers;
pub(crate) use presets::read_library;
use presets::{revisioned_library, store_library, validate_library_references};
mod transaction;
pub use transaction::{
    EnvironmentDefinitionWrite, EnvironmentSourceCommit, EnvironmentSourceWriteResult,
};

fn next_revision(current: u64) -> Result<u64, WorldDbError> {
    current
        .checked_add(1)
        .filter(|&r| r <= i64::MAX as u64)
        .ok_or(WorldDbError::IntegerOverflow)
}

fn validate_cell(
    definition: &EnvironmentDefinition,
    record: &SourceEnvironmentCellRecord,
) -> Result<(), WorldDbError> {
    let mut layers = BTreeSet::new();
    if record.space != definition.space
        || record.definition_revision != definition.revision
        || record.source_revision < 0
        || record.tiles.iter().any(|t| {
            !layers.insert(t.layer)
                || !definition.layers.iter().any(|l| l.id == t.layer)
                || t.samples.len() != usize::from(definition.mask_resolution).pow(2)
        })
    {
        return Err(WorldDbError::InvalidDenseSourceRecord);
    }
    Ok(())
}

fn store_cell(
    connection: &Connection,
    record: &SourceEnvironmentCellRecord,
) -> Result<(), WorldDbError> {
    connection.execute("INSERT INTO environment_cells(world_space_id, cell_x, cell_z, revision) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(world_space_id, cell_x, cell_z) DO UPDATE SET revision = excluded.revision", params![record.space.0, record.cell.x, record.cell.z, record.source_revision])?;
    connection.execute("DELETE FROM environment_coverage WHERE world_space_id = ?1 AND cell_x = ?2 AND cell_z = ?3", params![record.space.0, record.cell.x, record.cell.z])?;
    for tile in &record.tiles {
        if tile.samples.iter().all(|&s| s == 0) {
            continue;
        }
        connection.execute("INSERT INTO environment_coverage(world_space_id, cell_x, cell_z, layer_id, samples) VALUES (?1, ?2, ?3, ?4, ?5)", params![record.space.0, record.cell.x, record.cell.z, tile.layer.0.as_slice(), tile.samples])?;
    }
    Ok(())
}

pub(crate) fn validate_catalog_dependencies(
    connection: &Connection,
    plants: &VegetationCatalog,
) -> Result<(), WorldDbError> {
    let presets = read_library(connection)?;
    presets
        .validate(plants)
        .map_err(|e| invalid(e.to_string()))?;
    for definition in read_definitions(connection)? {
        checked_definition(&definition, plants, &presets)?;
    }
    Ok(())
}

pub(crate) fn write_environment_document(
    transaction: &Transaction<'_>,
    document: &ProjectDocument,
) -> Result<(), WorldDbError> {
    let plants = document
        .vegetation_catalog
        .clone()
        .unwrap_or_else(|| VegetationCatalog {
            species: Vec::new(),
            populations: Vec::new(),
            assemblages: Vec::new(),
        });
    let presets = &document.presets;
    validate_library_references(transaction, presets, &plants)?;
    store_library(transaction, presets)?;
    let definitions: BTreeMap<_, _> = document.environments.iter().map(|d| (d.space, d)).collect();
    if definitions.len() != document.environments.len()
        || definitions.len() > MAX_ENVIRONMENT_DEFINITIONS
    {
        return Err(invalid("duplicate or excessive environment definitions"));
    }
    for definition in definitions.values() {
        checked_definition(definition, &plants, presets)?;
        validate_definition_references(transaction, definition)?;
        store_definition(transaction, definition)?;
    }
    let mut keys = BTreeSet::new();
    for record in &document.environment_cells {
        if !keys.insert((record.space, record.cell)) {
            return Err(invalid("duplicate environment cell"));
        }
        let definition = definitions
            .get(&record.space)
            .ok_or_else(|| invalid("coverage has no definition"))?;
        validate_cell(definition, record)?;
        store_cell(transaction, record)?;
    }
    for (&space, definition) in &definitions {
        let plan = CompilePlan::new(definition, &plants, presets, CompileProfile::default())?;
        let cells = document
            .environment_cells
            .iter()
            .filter(|r| r.space == space)
            .map(|r| r.cell)
            .collect::<Vec<_>>();
        // Small chunks also bound the worst-case layer-mask input, not only the output cell count.
        for batch in cells.chunks(1) {
            let coverage = read_coverage(
                transaction,
                definition,
                &environment_dependency_cells(batch)?,
            )?;
            plan.validate_coverage(batch, &coverage)?;
        }
    }
    Ok(())
}

pub(crate) fn read_environment_document(
    connection: &Connection,
) -> Result<
    (
        PresetLibrary,
        Vec<EnvironmentDefinition>,
        Vec<SourceEnvironmentCellRecord>,
    ),
    WorldDbError,
> {
    let definitions = read_definitions(connection)?;
    let mut records = Vec::new();
    for definition in &definitions {
        let mut query = connection.prepare("SELECT cell_x, cell_z FROM environment_cells WHERE world_space_id = ?1 ORDER BY cell_x, cell_z")?;
        let mut rows = query.query([definition.space.0])?;
        while let Some(row) = rows.next()? {
            let cell = CellCoord {
                x: row.get(0)?,
                z: row.get(1)?,
            };
            let mut remaining = MAX_ENVIRONMENT_MASK_BYTES;
            records.push(
                read_cell(connection, definition, cell, &mut remaining)?
                    .ok_or_else(|| invalid("cell disappeared"))?,
            );
        }
    }
    Ok((read_library(connection)?, definitions, records))
}

impl ProjectReader {
    /// Small asset metadata for previewing any surface in a world's pack, including a surface
    /// absent from the currently cooked cell. Spatial masks are read separately.
    pub fn read_environment_terrain_resources(
        &self,
        space: WorldSpaceId,
    ) -> Result<crate::TerrainRenderResources, WorldDbError> {
        let tx = self.connection.unchecked_transaction()?;
        let profile = tx.query_row("SELECT world_space_id, texture_set_id, weight_resolution, macro_small_scale, macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength FROM world_space_terrain_profiles WHERE world_space_id = ?1", [space.0], crate::terrain_profile_from_row)?;
        let texture_set = tx.query_row("SELECT texture_set_id, texture_set_key, base_color_universal_uri, normal_material_universal_uri, macro_variation_universal_uri, base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, universal_gpu_bytes, astc_gpu_bytes FROM terrain_texture_sets WHERE texture_set_id = ?1", [profile.texture_set.0.as_slice()], crate::terrain_texture_set_from_row)?;
        let mut query = tx.prepare("SELECT s.surface_id, s.surface_key, s.display_name, s.tile_size, s.anti_tiling, s.normal_y_sign, s.normal_strength, s.roughness_min, s.roughness_max, l.layer FROM terrain_texture_set_layers l JOIN terrain_surfaces s ON s.surface_id = l.surface_id WHERE l.texture_set_id = ?1 ORDER BY l.layer LIMIT 65")?;
        let surfaces = query
            .query_map([profile.texture_set.0.as_slice()], |row| {
                Ok(crate::RuntimeTerrainSurface {
                    surface: crate::terrain_surface_from_row(row)?,
                    layer: row.get(9)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if surfaces.len() > 64 {
            return Err(invalid("terrain preview pack exceeds the surface budget"));
        }
        Ok(crate::TerrainRenderResources {
            profile,
            texture_set,
            surfaces,
        })
    }
}

#[cfg(test)]
mod tests;

/// Shared snapshot body for atomic environment + road reads.
pub(crate) fn read_snapshot(
    connection: &Connection,
    space: WorldSpaceId,
    cells: &[CellCoord],
) -> Result<EnvironmentReadSnapshot, WorldDbError> {
    let definition = read_definition(connection, space)?
        .ok_or_else(|| invalid("world has no environment definition"))?;
    let vegetation_catalog =
        read_vegetation_catalog(connection)?.unwrap_or_else(|| VegetationCatalog {
            species: Vec::new(),
            populations: Vec::new(),
            assemblages: Vec::new(),
        });
    let presets = read_library(connection)?;
    checked_definition(&definition, &vegetation_catalog, &presets)?;
    let coverage = read_coverage(connection, &definition, cells)?;
    Ok(EnvironmentReadSnapshot {
        definition,
        presets,
        vegetation_catalog,
        coverage,
    })
}
