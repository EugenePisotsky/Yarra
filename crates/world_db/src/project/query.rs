//! Shared source row conversion, catalog queries and bounded-query validation.
use crate::storage::blob_array;
use crate::{
    SourceAssetRecord, SourceAssetVariantRecord, SourceCellRecord, SourceObjectDefinitionRecord,
    SourceObjectPaletteRecord, SourceObjectRecord, SourceObjectViewRecord, WorldDbError,
};
use rusqlite::Connection;
use world::{
    AssetId, CellCoord, ObjectActivationPolicy, ObjectDefinitionId, StableObjectId, WorldSpaceId,
};

type SourceCatalogRecords = (
    Vec<SourceAssetRecord>,
    Vec<SourceAssetVariantRecord>,
    Vec<SourceObjectDefinitionRecord>,
);
pub(crate) fn query_source_catalog(
    connection: &Connection,
) -> Result<SourceCatalogRecords, WorldDbError> {
    let mut statement = connection.prepare(
        "SELECT asset_id, asset_key, kind, source_uri FROM source_assets ORDER BY asset_id",
    )?;
    let assets = statement
        .query_map([], |row| {
            Ok(SourceAssetRecord {
                id: AssetId(blob_array(row.get_ref(0)?.as_blob()?, "asset_id")?),
                key: row.get(1)?,
                kind: row.get(2)?,
                source_uri: row.get(3)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT asset_id, lod, uri, bounds_x, bounds_y, bounds_z, gpu_bytes_estimate, \
                shadow_policy, minimum_screen_height \
         FROM source_asset_variants ORDER BY asset_id, lod",
    )?;
    let asset_variants = statement
        .query_map([], |row| {
            let gpu_bytes_estimate: i64 = row.get(6)?;
            Ok(SourceAssetVariantRecord {
                asset: AssetId(blob_array(row.get_ref(0)?.as_blob()?, "asset_id")?),
                lod: row.get::<_, i64>(1)? as u8,
                uri: row.get(2)?,
                bounds: [row.get(3)?, row.get(4)?, row.get(5)?],
                gpu_bytes_estimate: gpu_bytes_estimate as u64,
                shadow_policy: row.get(7)?,
                minimum_screen_height: row.get(8)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut statement = connection.prepare(
        "SELECT definition_id, definition_key, display_name, visual_asset_id, activation_policy \
         FROM object_definitions ORDER BY definition_id",
    )?;
    let definitions = statement
        .query_map([], |row| {
            Ok(SourceObjectDefinitionRecord {
                id: ObjectDefinitionId(blob_array(row.get_ref(0)?.as_blob()?, "definition_id")?),
                key: row.get(1)?,
                display_name: row.get(2)?,
                visual_asset: row
                    .get::<_, Option<Vec<u8>>>(3)?
                    .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
                    .transpose()?,
                activation: ObjectActivationPolicy::try_from(row.get::<_, i64>(4)?).map_err(
                    |error| {
                        rusqlite::Error::FromSqlConversionFailure(
                            4,
                            rusqlite::types::Type::Integer,
                            Box::new(error),
                        )
                    },
                )?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;

    Ok((assets, asset_variants, definitions))
}

pub(crate) fn source_cell_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceCellRecord> {
    Ok(SourceCellRecord {
        space: WorldSpaceId(row.get(0)?),
        cell: CellCoord {
            x: row.get(1)?,
            z: row.get(2)?,
        },
        height: row.get(3)?,
        source_revision: row.get(4)?,
    })
}

pub(crate) fn source_object_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceObjectRecord> {
    Ok(SourceObjectRecord {
        id: StableObjectId(blob_array(row.get_ref(0)?.as_blob()?, "object_id")?),
        space: WorldSpaceId(row.get(1)?),
        owner_cell: CellCoord {
            x: row.get(2)?,
            z: row.get(3)?,
        },
        definition: ObjectDefinitionId(blob_array(row.get_ref(4)?.as_blob()?, "definition_id")?),
        local_translation: [row.get(5)?, row.get(6)?, row.get(7)?],
        yaw: row.get(8)?,
        scale: row.get(9)?,
        source_revision: row.get(10)?,
    })
}

pub(crate) fn source_object_view_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceObjectViewRecord> {
    let object = source_object_from_row(row)?;
    let visual_asset = row
        .get::<_, Option<Vec<u8>>>(13)?
        .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
        .transpose()?;
    let activation = ObjectActivationPolicy::try_from(row.get::<_, i64>(14)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            14,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    let definition = SourceObjectDefinitionRecord {
        id: object.definition,
        key: row.get(11)?,
        display_name: row.get(12)?,
        visual_asset,
        activation,
    };
    let visual_uri = row.get(15)?;
    let visual_bounds = row
        .get::<_, Option<f32>>(16)?
        .map(|bounds_x| -> rusqlite::Result<[f32; 3]> {
            Ok([bounds_x, row.get(17)?, row.get(18)?])
        })
        .transpose()?;
    Ok(SourceObjectViewRecord {
        object,
        definition,
        visual_uri,
        visual_bounds,
    })
}

pub(crate) fn source_object_palette_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<SourceObjectPaletteRecord> {
    let id = ObjectDefinitionId(blob_array(row.get_ref(0)?.as_blob()?, "definition_id")?);
    let visual_asset = row
        .get::<_, Option<Vec<u8>>>(3)?
        .map(|bytes| blob_array(&bytes, "visual_asset_id").map(AssetId))
        .transpose()?;
    let activation = ObjectActivationPolicy::try_from(row.get::<_, i64>(4)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            4,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })?;
    let visual_bounds = row
        .get::<_, Option<f32>>(6)?
        .map(|bounds_x| -> rusqlite::Result<[f32; 3]> { Ok([bounds_x, row.get(7)?, row.get(8)?]) })
        .transpose()?;
    Ok(SourceObjectPaletteRecord {
        definition: SourceObjectDefinitionRecord {
            id,
            key: row.get(1)?,
            display_name: row.get(2)?,
            visual_asset,
            activation,
        },
        visual_uri: row.get(5)?,
        visual_bounds,
    })
}

pub(crate) fn validate_spatial_query(
    minimum: CellCoord,
    maximum: CellCoord,
    maximum_records: usize,
) -> Result<(), WorldDbError> {
    if minimum.x > maximum.x || minimum.z > maximum.z {
        return Err(WorldDbError::InvalidSpatialQueryBounds { minimum, maximum });
    }
    if maximum_records == 0 {
        return Err(WorldDbError::InvalidQueryLimit);
    }
    Ok(())
}

pub(crate) fn query_sql_limit(maximum_records: usize) -> Result<i64, WorldDbError> {
    i64::try_from(maximum_records.saturating_add(1)).map_err(|_| WorldDbError::IntegerOverflow)
}
