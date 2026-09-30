//! Bounded source snapshots for the production cooker. All reads share one SQLite snapshot.
use crate::catalog::{
    query_all_terrain_profiles, query_all_terrain_surfaces, query_all_terrain_texture_layers,
    query_all_terrain_texture_sets, query_world_spaces, read_vegetation_catalog,
};
use crate::project::query::{query_source_catalog, source_cell_from_row, source_object_from_row};
use crate::storage::ensure_schema_version;
use crate::{
    COOK_KEY_BATCH, MAX_COOK_CATALOG_BYTES, MAX_COOK_CATALOG_ROWS,
    MAX_COOK_MANUAL_OBJECTS_PER_CELL, ProjectDocument, RoadReadSnapshot, SourceCellRecord,
    SourceObjectRecord, WorldDbError, environment_dependency_cells, environment_store, road_store,
};
use environment::{CoverageSnapshot, EnvironmentDefinition};
use environment_compile::TerrainSource;
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use world::{CellCoord, PROJECT_SCHEMA_VERSION, WorldSpaceId};

pub struct ProjectCookSnapshot {
    connection: Connection,
    // Only global definitions/catalogs; all spatial fields in this document are empty.
    catalog: ProjectDocument,
}
/// A cell's cook-input fingerprint, and whether it has terrain (else only paint to validate).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellFingerprint {
    pub input: [u8; 32],
    pub source: bool,
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
        let (default_world_space, start_view) = connection.query_row(
            "SELECT default_world_space_id, start_view FROM project_settings WHERE singleton=1",
            [],
            |r| Ok((WorldSpaceId(r.get(0)?), r.get::<_, Option<Vec<u8>>>(1)?)),
        )?;
        let catalog = ProjectDocument {
            default_world_space,
            start_view: crate::storage::decode_start_view(start_view)?,
            gameplay_areas: crate::gameplay_areas::read(&connection)?.areas,
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
                || space
                    .sea_level
                    .is_some_and(|level| !(space.minimum_y..=space.maximum_y).contains(&level))
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
    /// Fingerprints of every cell's cook inputs, without reading heights or masks: revisions
    /// of its own and neighbouring terrain and coverage rows (the halo `read_cell` reads),
    /// its manual objects, and the exact road snapshot `read_cell` would take for cells near
    /// a road. `header` stands for everything global. Source writes bump revisions; a direct
    /// database edit that does not is invisible here, so a full cook remains available.
    pub fn cell_fingerprints(
        &self,
        definition: &EnvironmentDefinition,
        header: &[u8; 32],
    ) -> Result<BTreeMap<CellCoord, CellFingerprint>, WorldDbError> {
        let space = definition.space;
        let cell = |r: &rusqlite::Row<'_>| -> rusqlite::Result<CellCoord> {
            Ok(CellCoord {
                x: r.get(0)?,
                z: r.get(1)?,
            })
        };
        let rows = |sql: &str| -> Result<HashMap<CellCoord, [i64; 2]>, WorldDbError> {
            let mut query = self.connection.prepare(sql)?;
            let rows = query.query_map([space.0], |r| {
                Ok((cell(r)?, [r.get::<_, i64>(2)?, r.get::<_, i64>(3)?]))
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        };
        // Pairs: unused and revision; resolution and revision; revision twice.
        let sources = rows(
            "SELECT cell_x,cell_z,0,source_revision FROM source_cells WHERE world_space_id=?1",
        )?;
        let heights: HashMap<CellCoord, f32> = {
            let mut query = self
                .connection
                .prepare("SELECT cell_x,cell_z,height FROM source_cells WHERE world_space_id=?1")?;
            let rows = query.query_map([space.0], |r| Ok((cell(r)?, r.get::<_, f32>(2)?)))?;
            rows.collect::<Result<_, _>>()?
        };
        let heightfields = rows(
            "SELECT cell_x,cell_z,resolution,source_revision FROM terrain_cell_heightfields WHERE world_space_id=?1",
        )?;
        let coverage = rows(
            "SELECT cell_x,cell_z,revision,revision FROM environment_cells WHERE world_space_id=?1",
        )?;
        let mut objects: HashMap<CellCoord, blake3::Hasher> = HashMap::new();
        {
            let mut query = self.connection.prepare(
                "SELECT owner_cell_x,owner_cell_z,object_id,source_revision FROM object_placements \
                 WHERE world_space_id=?1 ORDER BY owner_cell_x,owner_cell_z,object_id",
            )?;
            let mut rows = query.query([space.0])?;
            while let Some(r) = rows.next()? {
                let hasher = objects.entry(cell(r)?).or_default();
                hasher.update(r.get_ref(2)?.as_blob().map_err(rusqlite::Error::from)?);
                hasher.update(&r.get::<_, i64>(3)?.to_le_bytes());
            }
        }
        let road_cells: HashSet<CellCoord> = {
            let mut query = self.connection.prepare(
                "SELECT cell_x,cell_z FROM road_span_cells WHERE world_space_id=?1 \
                 UNION SELECT cell_x,cell_z FROM road_junction_cells WHERE world_space_id=?1",
            )?;
            let rows = query.query_map([space.0], cell)?;
            rows.collect::<Result<_, _>>()?
        };
        let mut result = BTreeMap::new();
        for &target in sources.keys().chain(coverage.keys()) {
            if result.contains_key(&target) {
                continue;
            }
            let halo = environment_dependency_cells(&[target])?;
            let mut hash = blake3::Hasher::new();
            hash.update(b"cook-cell-input-v1");
            hash.update(header);
            hash.update(&target.x.to_le_bytes());
            hash.update(&target.z.to_le_bytes());
            for neighbour in &halo {
                for value in [
                    sources.get(neighbour).map(|v| v[1]),
                    heights.get(neighbour).map(|h| i64::from(h.to_bits())),
                    heightfields.get(neighbour).map(|v| v[0]),
                    heightfields.get(neighbour).map(|v| v[1]),
                    coverage.get(neighbour).map(|v| v[0]),
                ] {
                    match value {
                        Some(v) => {
                            hash.update(&[1]);
                            hash.update(&v.to_le_bytes());
                        }
                        None => {
                            hash.update(&[0]);
                        }
                    }
                }
            }
            hash.update(
                objects
                    .get(&target)
                    .map_or([0; 32], |h| *h.finalize().as_bytes())
                    .as_slice(),
            );
            if halo.iter().any(|c| road_cells.contains(c)) {
                let bounds = environment::roads::RoadCellBounds {
                    minimum: CellCoord {
                        x: target.x - 1,
                        z: target.z - 1,
                    },
                    maximum: CellCoord {
                        x: target.x + 1,
                        z: target.z + 1,
                    },
                };
                let roads = road_store::read_cook_roads(&self.connection, space, bounds)?;
                hash.update(
                    &bincode::serde::encode_to_vec(&roads.roads, bincode::config::standard())
                        .map_err(|e| invalid(e.to_string()))?,
                );
            }
            result.insert(
                target,
                CellFingerprint {
                    input: *hash.finalize().as_bytes(),
                    source: sources.contains_key(&target),
                },
            );
        }
        Ok(result)
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
        ("gameplay_areas", "length(payload)"),
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

fn invalid(message: impl Into<String>) -> WorldDbError {
    WorldDbError::Cook(message.into())
}
