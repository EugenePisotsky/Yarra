//! Targeted source asset resolution; never scans the placement table or loads glTF bytes.
use super::*;

pub const MAX_COLLECTION_ASSET_READS: usize = 256;
#[derive(Debug, Clone)]
pub struct CollectionAssetView {
    pub id: AssetId,
    pub name: String,
    pub variants: Vec<SourceAssetVariantRecord>,
}
fn invalid(message: &str) -> WorldDbError {
    WorldDbError::Environment(message.into())
}
pub(crate) fn read_asset(
    connection: &Connection,
    id: AssetId,
) -> Result<CollectionAssetView, WorldDbError> {
    let (name, kind): (String, String) = connection
        .query_row(
            "SELECT asset_key,kind FROM source_assets WHERE asset_id=?1",
            [id.0.as_slice()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| invalid("collection references an unknown asset"))?;
    if kind != "gltf-scene" {
        return Err(invalid("collections require glTF scene assets"));
    }
    let mut statement = connection.prepare("SELECT lod,uri,bounds_x,bounds_y,bounds_z,gpu_bytes_estimate,shadow_policy,minimum_screen_height FROM source_asset_variants WHERE asset_id=?1 ORDER BY lod LIMIT 17")?;
    let variants = statement
        .query_map([id.0.as_slice()], |r| {
            Ok(SourceAssetVariantRecord {
                asset: id,
                lod: r.get(0)?,
                uri: r.get(1)?,
                bounds: [r.get(2)?, r.get(3)?, r.get(4)?],
                gpu_bytes_estimate: u64::try_from(r.get::<_, i64>(5)?)
                    .map_err(|_| rusqlite::Error::IntegralValueOutOfRange(5, -1))?,
                shadow_policy: r.get(6)?,
                minimum_screen_height: r.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if variants.is_empty()
        || variants.len() > 16
        || variants[0].lod != 0
        || variants.last().unwrap().minimum_screen_height != 0.0
        || variants.iter().any(|v| {
            v.uri.is_empty()
                || v.uri.len() > 4096
                || !v.minimum_screen_height.is_finite()
                || v.minimum_screen_height < 0.0
                || v.bounds.iter().any(|b| !b.is_finite() || *b < 0.0)
        })
        || variants
            .windows(2)
            .any(|v| v[0].minimum_screen_height < v[1].minimum_screen_height)
    {
        return Err(invalid(
            "collection asset has invalid or missing LOD variants",
        ));
    }
    Ok(CollectionAssetView { id, name, variants })
}
impl ProjectReader {
    pub fn read_collection_assets(
        &self,
        ids: &[AssetId],
    ) -> Result<Vec<CollectionAssetView>, WorldDbError> {
        if ids.len() > MAX_COLLECTION_ASSET_READS {
            return Err(invalid("collection asset read budget"));
        }
        let tx = self.connection.unchecked_transaction()?;
        ids.iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .map(|id| read_asset(&tx, id))
            .collect()
    }
}
