//! Bounded composite IO and private staging storage; no renderer dependencies.
use crate::runtime::read_runtime_manifest;
use crate::storage::{blob_array, ensure_schema_version};
use crate::{RuntimeManifest, RuntimeReader, WorldDbError};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::io::{Cursor, Read};
use std::path::Path;
use world::{
    CellCoord, MAX_TERRAIN_COMPOSITE_BYTES, RUNTIME_SCHEMA_VERSION, TerrainComposite,
    TerrainMaterialKey, TerrainNodeKey, WorldSpaceId, decode_terrain_composite,
    encode_terrain_composite,
};
const MAX_CORE_BYTES: usize = 262144;
const MAX_QUERY: usize = 128;
fn invalid(message: impl Into<String>) -> WorldDbError {
    WorldDbError::TerrainHierarchy(message.into())
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainCompositeDescriptor {
    pub key: TerrainMaterialKey,
    pub fingerprint: [u8; 32],
    pub checksum: [u8; 32],
    pub encoded_bytes: u64,
    pub decoded_bytes: u64,
    pub gpu_bytes: u64,
    /// Final descendant height bounds for material demand, independent of mesh LOD.
    pub height_bounds: [f32; 2],
}
#[derive(Debug)]
pub struct EncodedTerrainComposite {
    pub descriptor: TerrainCompositeDescriptor,
    pub payload: Vec<u8>,
}
impl EncodedTerrainComposite {
    pub fn decode(self) -> Result<TerrainComposite, WorldDbError> {
        if self.payload.len() as u64 != self.descriptor.encoded_bytes {
            return Err(invalid("composite encoded length"));
        }
        let bytes = decompress(
            &self.payload,
            self.descriptor.decoded_bytes,
            MAX_TERRAIN_COMPOSITE_BYTES,
        )?;
        if *blake3::hash(&bytes).as_bytes() != self.descriptor.checksum {
            return Err(invalid("composite checksum mismatch"));
        }
        let tile = decode_terrain_composite(&bytes).map_err(invalid)?;
        if tile.key != self.descriptor.key
            || tile.fingerprint != self.descriptor.fingerprint
            || self.descriptor.gpu_bytes != TerrainComposite::gpu_bytes() as u64
        {
            return Err(invalid("composite descriptor/payload mismatch"));
        }
        Ok(tile)
    }
}

pub struct TerrainMaterialCookStore {
    reader: RuntimeReader,
}
impl TerrainMaterialCookStore {
    pub fn open_staging(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        ensure_schema_version(&connection, RUNTIME_SCHEMA_VERSION, "runtime")?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA cache_size=-8192; PRAGMA temp_store=FILE; PRAGMA temp.cache_size=-8192; BEGIN IMMEDIATE;
            CREATE TEMP TABLE composite_cores(world_space_id INTEGER, level INTEGER, node_x INTEGER, node_z INTEGER, decoded_bytes INTEGER, payload BLOB, PRIMARY KEY(world_space_id,level,node_x,node_z)) WITHOUT ROWID;")?;
        let count: i64 =
            connection.query_row("SELECT count(*) FROM terrain_composites", [], |r| r.get(0))?;
        if count != 0 {
            return Err(invalid("staged terrain composites already populated"));
        }
        let manifest = read_runtime_manifest(&connection)?;
        Ok(Self {
            reader: RuntimeReader {
                connection,
                manifest,
            },
        })
    }
    pub fn reader(&self) -> &RuntimeReader {
        &self.reader
    }
    /// Address batches contain metadata only. Partial nodes still supply filtering halos.
    pub fn keys(
        &self,
        space: WorldSpaceId,
        level: u8,
        after: Option<CellCoord>,
    ) -> Result<Vec<(TerrainMaterialKey, bool)>, WorldDbError> {
        let mut query = self.reader.connection.prepare_cached("SELECT node_x,node_z,resolution IS NOT NULL FROM terrain_nodes WHERE world_space_id=?1 AND level=?2 AND (node_x,node_z)>(?3,?4) ORDER BY node_x,node_z LIMIT 128")?;
        Ok(query
            .query_map(
                params![
                    space.0,
                    level,
                    after.map_or(i64::MIN, |c| i64::from(c.x)),
                    after.map_or(i64::MIN, |c| i64::from(c.z))
                ],
                |r| {
                    Ok((
                        TerrainMaterialKey(TerrainNodeKey {
                            space,
                            level,
                            x: r.get(0)?,
                            z: r.get(1)?,
                        }),
                        r.get(2)?,
                    ))
                },
            )?
            .collect::<Result<_, _>>()?)
    }
    pub fn put_core(&self, key: TerrainMaterialKey, bytes: &[u8]) -> Result<(), WorldDbError> {
        key.0.cell_bounds().map_err(|e| invalid(e.to_string()))?;
        if bytes.is_empty() || bytes.len() > MAX_CORE_BYTES {
            return Err(invalid("composite core byte limit"));
        }
        let payload = zstd::stream::encode_all(Cursor::new(bytes), 1)?;
        let k = key.0;
        self.reader.connection.execute(
            "INSERT INTO composite_cores VALUES (?1,?2,?3,?4,?5,?6)",
            params![k.space.0, k.level, k.x, k.z, bytes.len() as i64, payload],
        )?;
        Ok(())
    }
    pub fn core(&self, key: TerrainMaterialKey) -> Result<Option<Vec<u8>>, WorldDbError> {
        let k = key.0;
        let data = self.reader.connection.query_row("SELECT decoded_bytes,payload FROM composite_cores WHERE world_space_id=?1 AND level=?2 AND node_x=?3 AND node_z=?4",params![k.space.0,k.level,k.x,k.z], |r| {
            let bytes = r.get_ref(1)?.as_blob()?;
            if bytes.len() > MAX_CORE_BYTES { return Err(rusqlite::Error::InvalidQuery); }
            Ok((r.get::<_,i64>(0)? as u64,bytes.to_vec()))
        }).optional()?;
        data.map(|(size, payload)| decompress(&payload, size, MAX_CORE_BYTES))
            .transpose()
    }
    pub fn insert(&self, tile: &TerrainComposite) -> Result<(), WorldDbError> {
        let bytes = encode_terrain_composite(tile).map_err(invalid)?;
        let payload = zstd::stream::encode_all(Cursor::new(&bytes), 3)?;
        if bytes.len() > MAX_TERRAIN_COMPOSITE_BYTES || payload.len() > MAX_TERRAIN_COMPOSITE_BYTES
        {
            return Err(invalid("composite byte limit"));
        }
        let k = tile.key.0;
        self.reader.connection.execute("INSERT INTO terrain_composites(world_space_id,level,node_x,node_z,fingerprint,checksum,decoded_bytes,gpu_bytes,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![k.space.0,k.level,k.x,k.z,tile.fingerprint.as_slice(),blake3::hash(&bytes).as_bytes().as_slice(),bytes.len() as i64,TerrainComposite::gpu_bytes() as i64,payload])?;
        Ok(())
    }
    pub fn finish(self) -> Result<RuntimeManifest, WorldDbError> {
        let mismatch: bool = self.reader.connection.query_row("SELECT EXISTS(SELECT 1 FROM terrain_nodes n LEFT JOIN terrain_composites c USING(world_space_id,level,node_x,node_z) WHERE (n.resolution IS NOT NULL) != (c.payload IS NOT NULL))",[],|r| r.get(0))?;
        if mismatch {
            return Err(invalid("composite coverage disagrees with terrain"));
        }
        let oversized: bool = self.reader.connection.query_row("SELECT EXISTS(SELECT 1 FROM terrain_roots r JOIN terrain_composites c USING(world_space_id,level,node_x,node_z) GROUP BY r.world_space_id HAVING sum(c.gpu_bytes)>33554432)",[],|r| r.get(0))?;
        if oversized {
            return Err(invalid("coarse composite cover exceeds 32 MiB"));
        }
        let mut hash = blake3::Hasher::new();
        hash.update(b"terrain-composites-v1");
        hash.update(&read_runtime_manifest(&self.reader.connection)?.content_hash);
        {
            let mut q = self.reader.connection.prepare("SELECT checksum FROM terrain_composites ORDER BY world_space_id,level,node_x,node_z")?;
            let mut rows = q.query([])?;
            while let Some(r) = rows.next()? {
                hash.update(r.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?);
            }
        }
        self.reader.connection.execute(
            "UPDATE runtime_metadata SET generation_id=?1,content_hash=?2 WHERE singleton=1",
            params![
                &hash.finalize().to_hex()[..16],
                hash.finalize().as_bytes().as_slice()
            ],
        )?;
        self.reader.connection.execute_batch("INSERT INTO terrain_material_spaces SELECT world_space_id,count(*) FROM terrain_composites GROUP BY world_space_id;
            INSERT OR IGNORE INTO terrain_material_spaces SELECT id,0 FROM world_spaces;
            DROP TABLE composite_cores; COMMIT;")?;
        read_runtime_manifest(&self.reader.connection)
    }
}

impl RuntimeReader {
    pub fn has_terrain_composites(&self, space: WorldSpaceId) -> Result<bool, WorldDbError> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM terrain_material_spaces WHERE world_space_id=?1)",
            [space.0],
            |r| r.get(0),
        )?)
    }
    pub fn read_terrain_composite_descriptors(
        &self,
        keys: &[TerrainMaterialKey],
    ) -> Result<Vec<Option<TerrainCompositeDescriptor>>, WorldDbError> {
        if keys.len() > MAX_QUERY {
            return Err(invalid("composite descriptor query limit"));
        }
        keys.iter()
            .map(|&key| descriptor(&self.connection, key))
            .collect()
    }
    pub fn read_terrain_composite(
        &self,
        key: TerrainMaterialKey,
    ) -> Result<Option<EncodedTerrainComposite>, WorldDbError> {
        let Some(descriptor) = descriptor(&self.connection, key)? else {
            return Ok(None);
        };
        let k = key.0;
        let payload = self.connection.query_row("SELECT payload FROM terrain_composites WHERE world_space_id=?1 AND level=?2 AND node_x=?3 AND node_z=?4",params![k.space.0,k.level,k.x,k.z],|r| {
            let bytes = r.get_ref(0)?.as_blob()?;
            if bytes.len() > MAX_TERRAIN_COMPOSITE_BYTES { return Err(rusqlite::Error::InvalidQuery); }
            Ok(bytes.to_vec())
        })?;
        Ok(Some(EncodedTerrainComposite {
            descriptor,
            payload,
        }))
    }
}
fn descriptor(
    connection: &Connection,
    key: TerrainMaterialKey,
) -> Result<Option<TerrainCompositeDescriptor>, WorldDbError> {
    let k = key.0;
    k.cell_bounds().map_err(|e| invalid(e.to_string()))?;
    let d = connection.query_row("SELECT c.fingerprint,c.checksum,length(c.payload),c.decoded_bytes,c.gpu_bytes,n.minimum_y,n.maximum_y FROM terrain_composites c JOIN terrain_nodes n USING(world_space_id,level,node_x,node_z) WHERE world_space_id=?1 AND level=?2 AND node_x=?3 AND node_z=?4",params![k.space.0,k.level,k.x,k.z],|r| Ok(TerrainCompositeDescriptor {key,fingerprint:blob_array(r.get_ref(0)?.as_blob()?,"composite fingerprint")?,checksum:blob_array(r.get_ref(1)?.as_blob()?,"composite checksum")?,encoded_bytes:r.get::<_,i64>(2)? as u64,decoded_bytes:r.get::<_,i64>(3)? as u64,gpu_bytes:r.get::<_,i64>(4)? as u64,height_bounds:[r.get(5)?,r.get(6)?]})).optional()?;
    if let Some(d) = &d {
        if !d.height_bounds.iter().all(|v| v.is_finite())
            || d.height_bounds[0] > d.height_bounds[1]
            || !(1..=MAX_TERRAIN_COMPOSITE_BYTES as u64).contains(&d.decoded_bytes)
            || !(1..=MAX_TERRAIN_COMPOSITE_BYTES as u64).contains(&d.encoded_bytes)
            || d.gpu_bytes != TerrainComposite::gpu_bytes() as u64
        {
            return Err(invalid("composite descriptor byte limits"));
        }
    }
    Ok(d)
}
fn decompress(payload: &[u8], expected: u64, limit: usize) -> Result<Vec<u8>, WorldDbError> {
    if expected == 0 || expected > limit as u64 || payload.len() > limit {
        return Err(invalid("composite declared byte limit"));
    }
    let mut bytes = Vec::with_capacity(expected as usize);
    zstd::stream::read::Decoder::new(Cursor::new(payload))?
        .take(expected + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 != expected {
        return Err(invalid("composite decoded length"));
    }
    Ok(bytes)
}
