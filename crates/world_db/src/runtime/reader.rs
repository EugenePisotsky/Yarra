//! Immutable runtime queries. This path never reads authoring tables or constructs a project document.
use crate::catalog::{
    query_world_spaces, read_vegetation_catalog, terrain_profile_from_row,
    terrain_surface_from_row, terrain_texture_set_from_row,
};
use crate::storage::{blob_array, ensure_schema_version, immutable_uri};
use crate::{
    CellDescriptor, EncodedPage, MAX_CELL_DESCRIPTOR_QUERY, PageDependency, RuntimeManifest,
    RuntimeObjectDefinition, RuntimeTerrainSurface, TerrainRenderResources, WorldDbError,
};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::path::Path;
use world::{
    AssetId, CellCoord, ObjectActivationPolicy, ObjectDefinitionId, PageCodec, PageKey,
    RUNTIME_SCHEMA_VERSION, WorldSpaceId,
};

const CELL_DESCRIPTOR_SQL: &str = "SELECT cell_x, cell_z, minimum_y, maximum_y, domain_mask \
             FROM cells \
             WHERE world_space_id = ?1 \
               AND cell_x = ?2 \
               AND cell_z BETWEEN ?3 AND ?4 \
             ORDER BY cell_x, cell_z";

pub(crate) fn read_page_connection(
    connection: &Connection,
    key: PageKey,
) -> Result<Option<EncodedPage>, WorldDbError> {
    connection
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

pub struct RuntimeReader {
    pub(crate) connection: Connection,
    pub(crate) manifest: RuntimeManifest,
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
        if minimum.x > maximum.x || minimum.z > maximum.z {
            return Err(WorldDbError::InvalidSpatialQueryBounds { minimum, maximum });
        }
        let width = (i64::from(maximum.x) - i64::from(minimum.x) + 1) as u64;
        let depth = (i64::from(maximum.z) - i64::from(minimum.z) + 1) as u64;
        if width > MAX_CELL_DESCRIPTOR_QUERY as u64
            || depth > MAX_CELL_DESCRIPTOR_QUERY as u64
            || width * depth > MAX_CELL_DESCRIPTOR_QUERY as u64
        {
            return Err(WorldDbError::InvalidQueryLimit);
        }
        let mut statement = self.connection.prepare_cached(CELL_DESCRIPTOR_SQL)?;
        // A seek per X row bounds visited rows even for a world with millions of
        // cells outside the requested Z range. A broad BETWEEN on X cannot do that.
        let mut result = Vec::new();
        for x in minimum.x..=maximum.x {
            let rows = statement.query_map(params![space.0, x, minimum.z, maximum.z], |row| {
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
            })?;
            for row in rows {
                result.push(row?);
            }
        }
        Ok(result)
    }

    pub fn read_page(&self, key: PageKey) -> Result<Option<EncodedPage>, WorldDbError> {
        read_page_connection(&self.connection, key)
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

pub(crate) fn read_runtime_manifest(
    connection: &Connection,
) -> Result<RuntimeManifest, WorldDbError> {
    let world_spaces = query_world_spaces(connection)?;
    let vegetation_catalog = read_vegetation_catalog(connection)?;
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
                vegetation_catalog,
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

#[cfg(test)]
mod tests;
