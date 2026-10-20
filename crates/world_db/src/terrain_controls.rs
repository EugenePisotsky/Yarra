//! Terrain control tiles (`world::terrain_control`): cooked into their own table, one per node
//! and level, zstd-compressed little-endian samples.
use crate::runtime::RuntimeReader;
use crate::storage::ensure_schema_version;
use crate::{TerrainRenderResources, WorldDbError};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::io::Read;
use std::path::Path;
use world::{
    CONTROL_SAMPLES, CellCoord, PageDomain, RUNTIME_SCHEMA_VERSION, TerrainNodeKey, WorldSpaceId,
};

const TILE_BYTES: usize = CONTROL_SAMPLES * CONTROL_SAMPLES * 4;

pub fn encode_terrain_control(samples: &[u32]) -> Result<Vec<u8>, WorldDbError> {
    if samples.len() != CONTROL_SAMPLES * CONTROL_SAMPLES {
        return Err(invalid("terrain control tile size"));
    }
    let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
    Ok(zstd::stream::encode_all(bytes.as_slice(), 3)?)
}

pub fn decode_terrain_control(payload: &[u8]) -> Result<Vec<u32>, WorldDbError> {
    let mut bytes = Vec::with_capacity(TILE_BYTES);
    zstd::stream::read::Decoder::new(payload)?
        .take(TILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() != TILE_BYTES {
        return Err(invalid("terrain control tile size"));
    }
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| u32::from_le_bytes(*b))
        .collect())
}

/// Writes a staging generation's control tiles, replacing any it had.
pub struct TerrainControlStore {
    connection: Connection,
}

impl TerrainControlStore {
    pub fn open_staging(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        ensure_schema_version(&connection, RUNTIME_SCHEMA_VERSION, "runtime")?;
        connection.execute_batch("BEGIN IMMEDIATE; DELETE FROM terrain_controls;")?;
        Ok(Self { connection })
    }

    pub fn write(&self, key: TerrainNodeKey, payload: &[u8]) -> Result<(), WorldDbError> {
        self.connection.execute(
            "INSERT INTO terrain_controls(world_space_id, level, node_x, node_z, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![key.space.0, key.level, key.x, key.z, payload],
        )?;
        Ok(())
    }

    pub fn finish(self) -> Result<(), WorldDbError> {
        self.connection.execute_batch("COMMIT;")?;
        Ok(())
    }
}

impl RuntimeReader {
    /// Compressed control tiles of `keys`, in order; `None` where a node has none.
    pub fn read_terrain_controls(
        &self,
        keys: &[TerrainNodeKey],
    ) -> Result<Vec<Option<Vec<u8>>>, WorldDbError> {
        let mut statement = self.connection.prepare_cached(
            "SELECT payload FROM terrain_controls \
             WHERE world_space_id=?1 AND level=?2 AND node_x=?3 AND node_z=?4",
        )?;
        keys.iter()
            .map(|k| {
                statement
                    .query_row(params![k.space.0, k.level, k.x, k.z], |r| r.get(0))
                    .optional()
                    .map_err(WorldDbError::from)
            })
            .collect()
    }

    /// Every cell of a space with terrain.
    pub fn terrain_cells(&self, space: WorldSpaceId) -> Result<Vec<CellCoord>, WorldDbError> {
        let mut statement = self.connection.prepare(
            "SELECT cell_x, cell_z FROM cell_pages \
             WHERE world_space_id=?1 AND domain=?2 AND lod=0 ORDER BY cell_z, cell_x",
        )?;
        statement
            .query_map(params![space.0, PageDomain::Terrain as i64], |r| {
                Ok(CellCoord {
                    x: r.get(0)?,
                    z: r.get(1)?,
                })
            })?
            .collect::<Result<_, _>>()
            .map_err(WorldDbError::from)
    }

    /// A space's terrain profile, texture set and every surface of that set with its layer.
    pub fn read_space_terrain_resources(
        &self,
        space: WorldSpaceId,
    ) -> Result<TerrainRenderResources, WorldDbError> {
        let profile = self.connection.query_row(
            "SELECT world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                    macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength, \
                    composite_minimum_level \
             FROM world_space_terrain_profiles WHERE world_space_id = ?1",
            [space.0],
            crate::catalog::terrain_profile_from_row,
        )?;
        let texture_set = self.connection.query_row(
            "SELECT texture_set_id, texture_set_key, base_color_universal_uri, \
                    normal_material_universal_uri, macro_variation_universal_uri, \
                    base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                    universal_gpu_bytes, astc_gpu_bytes \
             FROM terrain_texture_sets WHERE texture_set_id = ?1",
            [profile.texture_set.0.as_slice()],
            crate::catalog::terrain_texture_set_from_row,
        )?;
        let mut statement = self.connection.prepare(
            "SELECT s.surface_id, s.surface_key, s.display_name, s.tile_size, s.anti_tiling, \
                    s.normal_y_sign, s.normal_strength, s.roughness_min, s.roughness_max, l.layer \
             FROM terrain_texture_set_layers l \
             JOIN terrain_surfaces s ON s.surface_id = l.surface_id \
             WHERE l.texture_set_id = ?1 ORDER BY l.layer",
        )?;
        let surfaces = statement
            .query_map([profile.texture_set.0.as_slice()], |row| {
                Ok(crate::RuntimeTerrainSurface {
                    surface: crate::catalog::terrain_surface_from_row(row)?,
                    layer: row.get::<_, i64>(9)? as u16,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(TerrainRenderResources {
            profile,
            texture_set,
            surfaces,
        })
    }
}

fn invalid(message: &str) -> WorldDbError {
    WorldDbError::TerrainHierarchy(message.into())
}

#[cfg(test)]
mod tests {
    #[test]
    fn tiles_round_trip() {
        let samples: Vec<u32> = (0..65 * 65)
            .map(|i: u32| i.wrapping_mul(2_654_435_761))
            .collect();
        let payload = super::encode_terrain_control(&samples).unwrap();
        assert_eq!(super::decode_terrain_control(&payload).unwrap(), samples);
        assert!(super::encode_terrain_control(&samples[1..]).is_err());
    }
}
