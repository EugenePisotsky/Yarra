//! Far-object blocks: every impostor-drawn static object regrouped by block of cells, so a
//! view can stream them far beyond the cells' object range.
use crate::{EncodedPage, WorldDbError};
use rusqlite::{Connection, params};
use std::collections::{BTreeMap, HashMap};
use world::{
    AssetId, CellCoord, FAR_OBJECT_BLOCK_CELLS, FarObjectForm, FarObjectInstance, FarObjectsPage,
    PageCodec, PageDomain, PageKey, PagePayload, WorldSpaceId,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FarObjectStats {
    pub blocks: usize,
    pub instances: usize,
}

#[derive(Default)]
struct Block {
    forms: Vec<FarObjectForm>,
    form_of: HashMap<AssetId, u16>,
    instances: Vec<FarObjectInstance>,
}

/// Replaces every far-object block with one derived from the static-object pages and asset
/// variants in `connection`. Assets qualify when their last LOD is an impostor after a mesh.
pub(crate) fn rebuild(connection: &Connection) -> Result<FarObjectStats, WorldDbError> {
    let forms = impostor_forms(connection)?;
    let cell_sizes: HashMap<i64, f32> = connection
        .prepare("SELECT id, cell_size FROM world_spaces")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    connection.execute("DELETE FROM far_object_pages", [])?;
    let mut blocks: BTreeMap<(i64, CellCoord), Block> = BTreeMap::new();
    let mut query = connection.prepare(
        "SELECT world_space_id, cell_x, cell_z, codec, decoded_bytes, gpu_bytes_estimate, \
                checksum, payload \
         FROM cell_pages WHERE domain = ?1 AND lod = 0 \
         ORDER BY world_space_id, cell_x, cell_z",
    )?;
    let mut rows = query.query([PageDomain::StaticObjects as i64])?;
    while let Some(row) = rows.next()? {
        let space: i64 = row.get(0)?;
        let cell = CellCoord {
            x: row.get(1)?,
            z: row.get(2)?,
        };
        let page = EncodedPage {
            key: PageKey {
                space: WorldSpaceId(space),
                cell,
                domain: PageDomain::StaticObjects,
                lod: 0,
            },
            codec: PageCodec::try_from(row.get::<_, i64>(3)?)
                .map_err(|_| invalid("static-object page has an unknown codec"))?,
            decoded_bytes: row.get::<_, i64>(4)? as u64,
            gpu_bytes_estimate: row.get::<_, i64>(5)? as u64,
            checksum: crate::storage::blob_array(
                row.get_ref(6)?.as_blob().map_err(rusqlite::Error::from)?,
                "checksum",
            )?,
            payload: row.get(7)?,
        };
        let PagePayload::StaticObjects(objects) = page.decode()?.payload else {
            return Err(invalid("static-object page holds another payload"));
        };
        let size = *cell_sizes
            .get(&space)
            .ok_or_else(|| invalid("static-object page has no world space"))?;
        let block = world::far_object_block(cell);
        let offset = [
            ((cell.x - block.x * FAR_OBJECT_BLOCK_CELLS) as f32) * size,
            ((cell.z - block.z * FAR_OBJECT_BLOCK_CELLS) as f32) * size,
        ];
        for instance in objects.instances {
            let Some(form) = forms.get(&instance.asset) else {
                continue;
            };
            let block = blocks.entry((space, block)).or_default();
            let index = match block.form_of.get(&instance.asset) {
                Some(&index) => index,
                None => {
                    let index = u16::try_from(block.forms.len())
                        .ok()
                        .filter(|&i| usize::from(i) < world::MAX_FAR_OBJECT_FORMS)
                        .ok_or_else(|| invalid("far-object block has too many forms"))?;
                    block.forms.push(form.clone());
                    block.form_of.insert(instance.asset, index);
                    index
                }
            };
            block.instances.push(FarObjectInstance {
                form: index,
                translation: [
                    offset[0] + instance.translation[0],
                    instance.translation[1],
                    offset[1] + instance.translation[2],
                ],
                yaw: instance.yaw,
                scale: instance.scale,
            });
        }
    }
    let mut stats = FarObjectStats::default();
    let mut insert = connection.prepare(
        "INSERT INTO far_object_pages(world_space_id, block_x, block_z, instance_count, payload) \
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for (
        (space, block),
        Block {
            forms, instances, ..
        },
    ) in blocks
    {
        let page = FarObjectsPage { forms, instances };
        page.validate().map_err(invalid)?;
        let payload = page
            .encode()
            .map_err(|error| invalid(format!("could not encode far objects: {error}")))?;
        insert.execute(params![
            space,
            block.x,
            block.z,
            page.instances.len() as i64,
            payload
        ])?;
        stats.blocks += 1;
        stats.instances += page.instances.len();
    }
    Ok(stats)
}

/// Impostor forms by asset: the last variant is an impostor and the one before a mesh with a
/// nonzero switch height.
fn impostor_forms(
    connection: &Connection,
) -> Result<HashMap<AssetId, FarObjectForm>, WorldDbError> {
    let mut variants: BTreeMap<AssetId, Vec<(String, f32, f32)>> = BTreeMap::new();
    let mut query = connection.prepare(
        "SELECT asset_id, uri, bounds_y, minimum_screen_height FROM asset_variants \
         ORDER BY asset_id, lod",
    )?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let asset = AssetId(crate::storage::blob_array(
            row.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?,
            "asset_id",
        )?);
        variants
            .entry(asset)
            .or_default()
            .push((row.get(1)?, row.get(2)?, row.get(3)?));
    }
    Ok(variants
        .into_iter()
        .filter_map(|(asset, variants)| {
            let [.., (mesh, _, threshold), (impostor, _, _)] = variants.as_slice() else {
                return None;
            };
            if !world::is_impostor_uri(impostor)
                || world::is_impostor_uri(mesh)
                || *threshold <= 0.0
            {
                return None;
            }
            let height = variants.iter().map(|v| v.1).fold(0.0_f32, f32::max);
            (height > 0.0).then(|| {
                (
                    asset,
                    FarObjectForm {
                        uri: impostor.clone(),
                        switch: height / threshold,
                    },
                )
            })
        })
        .collect())
}

/// Encoded far-object pages of the blocks in `[minimum, maximum]`, which is bounded by
/// [`crate::MAX_CELL_DESCRIPTOR_QUERY`] blocks. Blocks without objects have no row.
pub(crate) fn read(
    connection: &Connection,
    space: WorldSpaceId,
    minimum: CellCoord,
    maximum: CellCoord,
) -> Result<Vec<(CellCoord, Vec<u8>)>, WorldDbError> {
    if minimum.x > maximum.x || minimum.z > maximum.z {
        return Err(WorldDbError::InvalidSpatialQueryBounds { minimum, maximum });
    }
    let width = (i64::from(maximum.x) - i64::from(minimum.x) + 1) as u64;
    let depth = (i64::from(maximum.z) - i64::from(minimum.z) + 1) as u64;
    if width.saturating_mul(depth) > crate::MAX_CELL_DESCRIPTOR_QUERY as u64 {
        return Err(WorldDbError::InvalidQueryLimit);
    }
    let mut statement = connection.prepare_cached(
        "SELECT block_z, payload FROM far_object_pages \
         WHERE world_space_id = ?1 AND block_x = ?2 AND block_z BETWEEN ?3 AND ?4 \
         ORDER BY block_z",
    )?;
    let mut result = Vec::new();
    for x in minimum.x..=maximum.x {
        let rows = statement.query_map(params![space.0, x, minimum.z, maximum.z], |row| {
            Ok((CellCoord { x, z: row.get(0)? }, row.get(1)?))
        })?;
        for row in rows {
            result.push(row?);
        }
    }
    Ok(result)
}

fn invalid(message: impl Into<String>) -> WorldDbError {
    WorldDbError::Cook(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{atmosphere, schema};
    use world::{StableObjectId, StaticObjectInstance, StaticObjectsPage};

    const TREE: AssetId = AssetId([1; 32]);
    const ROCK: AssetId = AssetId([2; 32]);

    fn runtime() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        schema::create_runtime_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO world_spaces VALUES (1,'test',8,-1,1,?1,1,NULL)",
                [atmosphere::encode(&Default::default()).unwrap()],
            )
            .unwrap();
        // The tree's last LOD is an impostor after a mesh switching at 200 px; the rock
        // has meshes only.
        for (asset, lod, uri, height, threshold) in [
            (TREE, 0, "tree_lod0.gltf", 20.0, 480.0),
            (TREE, 1, "tree_lod1.gltf", 18.0, 200.0),
            (TREE, 2, "tree.impostor.json", 20.0, 0.0),
            (ROCK, 0, "rock.gltf", 2.0, 0.0),
        ] {
            connection
                .execute(
                    "INSERT INTO asset_variants VALUES (?1,?2,'gltf-scene',?3,1,?4,1,0,1,?5)",
                    params![asset.0.as_slice(), lod, uri, height, threshold],
                )
                .unwrap();
        }
        connection
    }

    fn add_cell(connection: &Connection, cell: CellCoord, instances: &[(AssetId, [f32; 3])]) {
        connection
            .execute(
                "INSERT INTO cells VALUES (1,?1,?2,-1,1,0,1,2,zeroblob(32),zeroblob(32))",
                params![cell.x, cell.z],
            )
            .unwrap();
        let payload = world::encode_page_payload(&PagePayload::StaticObjects(StaticObjectsPage {
            instances: instances
                .iter()
                .enumerate()
                .map(|(i, &(asset, translation))| StaticObjectInstance {
                    generated: false,
                    id: StableObjectId([i as u8; 16]),
                    asset,
                    translation,
                    yaw: 0.5,
                    scale: 2.0,
                })
                .collect(),
        }))
        .unwrap();
        connection
            .execute(
                "INSERT INTO cell_pages VALUES (1,?1,?2,?3,0,0,?4,?4,0,?5,?6)",
                params![
                    cell.x,
                    cell.z,
                    PageDomain::StaticObjects as i64,
                    payload.len() as i64,
                    blake3::hash(&payload).as_bytes().as_slice(),
                    payload
                ],
            )
            .unwrap();
    }

    #[test]
    fn impostor_objects_regroup_by_block_relative_to_its_corner() {
        let connection = runtime();
        add_cell(
            &connection,
            CellCoord { x: -1, z: 0 },
            &[(TREE, [1.0, 5.0, 2.0]), (ROCK, [3.0, 0.0, 3.0])],
        );
        add_cell(
            &connection,
            CellCoord { x: 16, z: 3 },
            &[(TREE, [4.0, 6.0, 7.0])],
        );
        let stats = rebuild(&connection).unwrap();
        assert_eq!(
            stats,
            FarObjectStats {
                blocks: 2,
                instances: 2
            }
        );
        // A second rebuild replaces, not duplicates.
        assert_eq!(rebuild(&connection).unwrap(), stats);

        let pages = read(
            &connection,
            WorldSpaceId(1),
            CellCoord { x: -1, z: -1 },
            CellCoord { x: 1, z: 1 },
        )
        .unwrap();
        assert_eq!(
            pages.iter().map(|(block, _)| *block).collect::<Vec<_>>(),
            [CellCoord { x: -1, z: 0 }, CellCoord { x: 1, z: 0 }]
        );
        let west = FarObjectsPage::decode(&pages[0].1).unwrap();
        assert_eq!(
            west.forms,
            [FarObjectForm {
                uri: "tree.impostor.json".into(),
                switch: 20.0 / 200.0,
            }]
        );
        // Cell -1 is the last of block -1's 16 cells of 8 m.
        assert_eq!(west.instances[0].translation, [15.0 * 8.0 + 1.0, 5.0, 2.0]);
        assert_eq!((west.instances[0].yaw, west.instances[0].scale), (0.5, 2.0));
        let east = FarObjectsPage::decode(&pages[1].1).unwrap();
        assert_eq!(east.instances[0].translation, [4.0, 6.0, 3.0 * 8.0 + 7.0]);
    }

    #[test]
    fn block_windows_are_bounded() {
        let connection = runtime();
        let query = |max: i32| {
            read(
                &connection,
                WorldSpaceId(1),
                CellCoord { x: 0, z: 0 },
                CellCoord { x: max, z: max },
            )
        };
        assert!(query(63).is_ok());
        assert!(matches!(query(64), Err(WorldDbError::InvalidQueryLimit)));
    }
}
