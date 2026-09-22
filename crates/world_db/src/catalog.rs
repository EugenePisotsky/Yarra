//! Catalog encoding and queries shared by source import/export and runtime storage.
use crate::storage::blob_array;
use crate::{WorldDbError, WorldSpaceRecord, atmosphere};
use rusqlite::{Connection, OptionalExtension, params};
use vegetation::VegetationCatalog;
use world::{
    MAX_DECODED_PAGE_BYTES, TerrainProfile, TerrainSurface, TerrainSurfaceId, TerrainTextureLayer,
    TerrainTextureSet, TerrainTextureSetId, WorldSpaceId,
};

pub(crate) const VEGETATION_CATALOG_FORMAT_VERSION: i64 = 7;
pub(crate) fn write_vegetation_catalog(
    transaction: &Connection,
    catalog: Option<&VegetationCatalog>,
) -> Result<(), WorldDbError> {
    let Some(catalog) = catalog else {
        return Ok(());
    };
    let payload = encode_vegetation_catalog(catalog)?;
    transaction.execute(
        "INSERT INTO vegetation_catalog(singleton, format_version, payload) \
         VALUES (1, ?1, ?2)",
        params![VEGETATION_CATALOG_FORMAT_VERSION, payload],
    )?;
    Ok(())
}

pub(crate) fn encode_vegetation_catalog(
    catalog: &VegetationCatalog,
) -> Result<Vec<u8>, WorldDbError> {
    catalog.validate()?;
    Ok(bincode::serde::encode_to_vec(
        catalog,
        bincode::config::standard()
            .with_little_endian()
            .with_fixed_int_encoding(),
    )?)
}

pub(crate) fn read_vegetation_catalog(
    connection: &Connection,
) -> Result<Option<VegetationCatalog>, WorldDbError> {
    connection
        .query_row(
            "SELECT format_version, payload FROM vegetation_catalog WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?)),
        )
        .optional()?
        .map(|(format_version, payload)| {
            if format_version != VEGETATION_CATALOG_FORMAT_VERSION {
                return Err(WorldDbError::VegetationCatalogFormatVersion {
                    expected: VEGETATION_CATALOG_FORMAT_VERSION,
                    actual: format_version,
                });
            }
            let (catalog, consumed): (VegetationCatalog, usize) =
                bincode::serde::decode_from_slice(
                    &payload,
                    bincode::config::standard()
                        .with_little_endian()
                        .with_fixed_int_encoding()
                        .with_limit::<{ MAX_DECODED_PAGE_BYTES as usize }>(),
                )?;
            if consumed != payload.len() {
                return Err(WorldDbError::VegetationCatalogTrailingBytes {
                    consumed,
                    total: payload.len(),
                });
            }
            catalog.validate()?;
            Ok(catalog)
        })
        .transpose()
}

pub(crate) fn write_terrain_catalog(
    transaction: &Connection,
    surfaces: &[TerrainSurface],
    texture_sets: &[TerrainTextureSet],
    texture_layers: &[TerrainTextureLayer],
    profiles: &[TerrainProfile],
) -> Result<(), WorldDbError> {
    for surface in surfaces {
        transaction.execute(
            "INSERT INTO terrain_surfaces( \
                surface_id, surface_key, display_name, tile_size, anti_tiling, normal_y_sign, \
                normal_strength, roughness_min, roughness_max \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                surface.id.0.as_slice(),
                surface.key,
                surface.display_name,
                surface.tile_size,
                surface.anti_tiling,
                surface.normal_y_sign,
                surface.normal_strength,
                surface.roughness_min,
                surface.roughness_max,
            ],
        )?;
    }
    for texture_set in texture_sets {
        transaction.execute(
            "INSERT INTO terrain_texture_sets( \
                texture_set_id, texture_set_key, base_color_universal_uri, \
                normal_material_universal_uri, macro_variation_universal_uri, \
                base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                universal_gpu_bytes, astc_gpu_bytes \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                texture_set.id.0.as_slice(),
                texture_set.key,
                texture_set.base_color_universal_uri,
                texture_set.normal_material_universal_uri,
                texture_set.macro_variation_universal_uri,
                texture_set.base_color_astc_uri,
                texture_set.normal_material_astc_uri,
                texture_set.macro_variation_astc_uri,
                i64::try_from(texture_set.universal_gpu_bytes)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
                i64::try_from(texture_set.astc_gpu_bytes)
                    .map_err(|_| WorldDbError::IntegerOverflow)?,
            ],
        )?;
    }
    for layer in texture_layers {
        transaction.execute(
            "INSERT INTO terrain_texture_set_layers(texture_set_id, layer, surface_id) \
             VALUES (?1, ?2, ?3)",
            params![
                layer.texture_set.0.as_slice(),
                i64::from(layer.layer),
                layer.surface.0.as_slice(),
            ],
        )?;
    }
    for profile in profiles {
        transaction.execute(
            "INSERT INTO world_space_terrain_profiles( \
                world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                profile.space.0,
                profile.texture_set.0.as_slice(),
                i64::from(profile.weight_resolution),
                profile.macro_scales[0],
                profile.macro_scales[1],
                profile.macro_scales[2],
                profile.macro_contrast,
                profile.macro_albedo_strength,
            ],
        )?;
    }
    Ok(())
}

pub(crate) fn query_all_terrain_surfaces(
    connection: &Connection,
) -> Result<Vec<TerrainSurface>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT surface_id, surface_key, display_name, tile_size, anti_tiling, normal_y_sign, \
                normal_strength, roughness_min, roughness_max \
         FROM terrain_surfaces ORDER BY surface_id",
    )?;
    Ok(statement
        .query_map([], terrain_surface_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn terrain_surface_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<TerrainSurface> {
    Ok(TerrainSurface {
        id: TerrainSurfaceId(blob_array(row.get_ref(0)?.as_blob()?, "surface_id")?),
        key: row.get(1)?,
        display_name: row.get(2)?,
        tile_size: row.get(3)?,
        anti_tiling: row.get(4)?,
        normal_y_sign: row.get(5)?,
        normal_strength: row.get(6)?,
        roughness_min: row.get(7)?,
        roughness_max: row.get(8)?,
    })
}

pub(crate) fn query_all_terrain_texture_sets(
    connection: &Connection,
) -> Result<Vec<TerrainTextureSet>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT texture_set_id, texture_set_key, base_color_universal_uri, \
                normal_material_universal_uri, macro_variation_universal_uri, \
                base_color_astc_uri, normal_material_astc_uri, macro_variation_astc_uri, \
                universal_gpu_bytes, astc_gpu_bytes \
         FROM terrain_texture_sets ORDER BY texture_set_id",
    )?;
    Ok(statement
        .query_map([], terrain_texture_set_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn terrain_texture_set_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<TerrainTextureSet> {
    Ok(TerrainTextureSet {
        id: TerrainTextureSetId(blob_array(row.get_ref(0)?.as_blob()?, "texture_set_id")?),
        key: row.get(1)?,
        base_color_universal_uri: row.get(2)?,
        normal_material_universal_uri: row.get(3)?,
        macro_variation_universal_uri: row.get(4)?,
        base_color_astc_uri: row.get(5)?,
        normal_material_astc_uri: row.get(6)?,
        macro_variation_astc_uri: row.get(7)?,
        universal_gpu_bytes: row.get::<_, i64>(8)? as u64,
        astc_gpu_bytes: row.get::<_, i64>(9)? as u64,
    })
}

pub(crate) fn query_all_terrain_texture_layers(
    connection: &Connection,
) -> Result<Vec<TerrainTextureLayer>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT texture_set_id, surface_id, layer \
         FROM terrain_texture_set_layers ORDER BY texture_set_id, layer",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(TerrainTextureLayer {
                texture_set: TerrainTextureSetId(blob_array(
                    row.get_ref(0)?.as_blob()?,
                    "texture_set_id",
                )?),
                surface: TerrainSurfaceId(blob_array(row.get_ref(1)?.as_blob()?, "surface_id")?),
                layer: row.get::<_, i64>(2)? as u16,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn query_all_terrain_profiles(
    connection: &Connection,
) -> Result<Vec<TerrainProfile>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT world_space_id, texture_set_id, weight_resolution, macro_small_scale, \
                macro_medium_scale, macro_large_scale, macro_contrast, macro_albedo_strength \
         FROM world_space_terrain_profiles ORDER BY world_space_id",
    )?;
    Ok(statement
        .query_map([], terrain_profile_from_row)?
        .collect::<Result<Vec<_>, _>>()?)
}

pub(crate) fn terrain_profile_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<TerrainProfile> {
    Ok(TerrainProfile {
        space: WorldSpaceId(row.get(0)?),
        texture_set: TerrainTextureSetId(blob_array(row.get_ref(1)?.as_blob()?, "texture_set_id")?),
        weight_resolution: row.get::<_, i64>(2)? as u16,
        macro_scales: [row.get(3)?, row.get(4)?, row.get(5)?],
        macro_contrast: row.get(6)?,
        macro_albedo_strength: row.get(7)?,
    })
}

pub(crate) fn query_world_spaces(
    connection: &Connection,
) -> Result<Vec<WorldSpaceRecord>, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT id, name, cell_size, minimum_y, maximum_y, atmosphere, atmosphere_revision FROM world_spaces ORDER BY id LIMIT 33",
    )?;
    let mut rows = statement.query([])?;
    let mut spaces = Vec::new();
    while let Some(row) = rows.next()? {
        let bytes: Vec<u8> = row.get(5)?;
        let profile = atmosphere::decode(&bytes)?;
        spaces.push(WorldSpaceRecord {
            id: WorldSpaceId(row.get(0)?),
            name: row.get(1)?,
            cell_size: row.get(2)?,
            minimum_y: row.get(3)?,
            maximum_y: row.get(4)?,
            atmosphere: profile,
            atmosphere_revision: row.get(6)?,
        });
    }
    if spaces.len() > 32 {
        return Err(WorldDbError::Cook(
            "at most 32 world atmospheres are supported".into(),
        ));
    }
    Ok(spaces)
}
