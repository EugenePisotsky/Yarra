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

/// A compressed filtering core in staging.
pub struct StagedCore {
    decoded_bytes: u64,
    payload: Vec<u8>,
}
impl StagedCore {
    pub fn compress(bytes: &[u8]) -> Result<Self, WorldDbError> {
        if bytes.is_empty() || bytes.len() > MAX_CORE_BYTES {
            return Err(invalid("composite core byte limit"));
        }
        Ok(Self {
            decoded_bytes: bytes.len() as u64,
            payload: zstd::stream::encode_all(Cursor::new(bytes), 1)?,
        })
    }
    pub fn decompress(&self) -> Result<Vec<u8>, WorldDbError> {
        decompress(&self.payload, self.decoded_bytes, MAX_CORE_BYTES)
    }
    pub(crate) fn from_parts(decoded_bytes: u64, payload: Vec<u8>) -> Self {
        Self {
            decoded_bytes,
            payload,
        }
    }
    pub(crate) fn parts(&self) -> (u64, &[u8]) {
        (self.decoded_bytes, &self.payload)
    }
}

/// An encoded, compressed composite ready to insert. Preparing it needs no database, so it
/// can run on any thread.
pub struct PreparedComposite {
    key: TerrainMaterialKey,
    fingerprint: [u8; 32],
    checksum: [u8; 32],
    decoded_bytes: u64,
    payload: Vec<u8>,
}
impl PreparedComposite {
    pub fn new(tile: &TerrainComposite) -> Result<Self, WorldDbError> {
        let bytes = encode_terrain_composite(tile).map_err(invalid)?;
        let payload = zstd::stream::encode_all(Cursor::new(&bytes), 3)?;
        if bytes.len() > MAX_TERRAIN_COMPOSITE_BYTES || payload.len() > MAX_TERRAIN_COMPOSITE_BYTES
        {
            return Err(invalid("composite byte limit"));
        }
        Ok(Self {
            key: tile.key,
            fingerprint: tile.fingerprint,
            checksum: *blake3::hash(&bytes).as_bytes(),
            decoded_bytes: bytes.len() as u64,
            payload,
        })
    }
}

pub struct TerrainMaterialCookStore {
    reader: RuntimeReader,
}
impl TerrainMaterialCookStore {
    pub fn open_staging(path: &Path) -> Result<Self, WorldDbError> {
        let store = Self::open_incremental(path)?;
        let count: i64 = store.reader.connection.query_row(
            "SELECT (SELECT count(*) FROM terrain_composites)+(SELECT count(*) FROM terrain_cores)",
            [],
            |r| r.get(0),
        )?;
        if count != 0 {
            return Err(invalid("staged terrain composites already populated"));
        }
        Ok(store)
    }
    /// Continues the composites of a copied publication; see `clear_space` and `finish`.
    pub fn open_incremental(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        ensure_schema_version(&connection, RUNTIME_SCHEMA_VERSION, "runtime")?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA cache_size=-8192; PRAGMA temp_store=FILE; PRAGMA temp.cache_size=-8192; BEGIN IMMEDIATE;
            CREATE TEMP TABLE composite_cores(world_space_id INTEGER, level INTEGER, node_x INTEGER, node_z INTEGER, decoded_bytes INTEGER, payload BLOB, PRIMARY KEY(world_space_id,level,node_x,node_z)) WITHOUT ROWID;")?;
        let manifest = read_runtime_manifest(&connection)?;
        Ok(Self {
            reader: RuntimeReader {
                connection,
                manifest,
            },
        })
    }
    /// Drops a space's composites and cores, to bake it again from scratch.
    pub fn clear_space(&self, space: WorldSpaceId) -> Result<(), WorldDbError> {
        for table in [
            "terrain_composites",
            "terrain_cores",
            "temp.composite_cores",
        ] {
            self.reader.connection.execute(
                &format!("DELETE FROM {table} WHERE world_space_id=?1"),
                [space.0],
            )?;
        }
        Ok(())
    }
    /// The bake inputs the space's published composites were made with, if it has any.
    pub fn library_fingerprint(
        &self,
        space: WorldSpaceId,
    ) -> Result<Option<[u8; 32]>, WorldDbError> {
        let fingerprint: Option<Vec<u8>> = self
            .reader
            .connection
            .query_row(
                "SELECT library_fingerprint FROM terrain_material_spaces WHERE world_space_id=?1",
                [space.0],
                |r| r.get(0),
            )
            .optional()?;
        Ok(fingerprint
            .map(|f| blob_array(&f, "library fingerprint"))
            .transpose()?)
    }
    /// The fingerprint of the core behind `key`, as published or updated in this cook.
    pub fn core_fingerprint(
        &self,
        key: TerrainMaterialKey,
    ) -> Result<Option<[u8; 32]>, WorldDbError> {
        let k = key.0;
        let fingerprint: Option<Vec<u8>> = self
            .reader
            .connection
            .query_row(
                "SELECT fingerprint FROM terrain_cores WHERE world_space_id=?1 AND level=?2 AND \
                 node_x=?3 AND node_z=?4",
                params![k.space.0, k.level, k.x, k.z],
                |r| r.get(0),
            )
            .optional()?;
        Ok(fingerprint
            .map(|f| blob_array(&f, "core fingerprint"))
            .transpose()?)
    }
    pub fn put_core_fingerprint(
        &self,
        key: TerrainMaterialKey,
        fingerprint: &[u8; 32],
    ) -> Result<(), WorldDbError> {
        let k = key.0;
        self.reader.connection.execute(
            "INSERT OR REPLACE INTO terrain_cores VALUES (?1,?2,?3,?4,?5)",
            params![k.space.0, k.level, k.x, k.z, fingerprint.as_slice()],
        )?;
        Ok(())
    }
    pub fn has_composite(&self, key: TerrainMaterialKey) -> Result<bool, WorldDbError> {
        let k = key.0;
        Ok(self.reader.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM terrain_composites WHERE world_space_id=?1 AND level=?2 \
             AND node_x=?3 AND node_z=?4)",
            params![k.space.0, k.level, k.x, k.z],
            |r| r.get(0),
        )?)
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
    /// The profile's finest published level; spaces without a profile publish every level.
    pub fn composite_minimum_level(&self, space: WorldSpaceId) -> Result<u8, WorldDbError> {
        composite_minimum_level(&self.reader.connection, space)
    }
    /// Existing leaves under `key`, in key order.
    pub fn leaves_within(
        &self,
        key: TerrainMaterialKey,
    ) -> Result<Vec<TerrainMaterialKey>, WorldDbError> {
        let [minimum, maximum] = key.0.cell_bounds().map_err(|e| invalid(e.to_string()))?;
        let mut query = self.reader.connection.prepare_cached("SELECT node_x,node_z FROM terrain_nodes WHERE world_space_id=?1 AND level=0 AND node_x BETWEEN ?2 AND ?3 AND node_z BETWEEN ?4 AND ?5 ORDER BY node_x,node_z")?;
        Ok(query
            .query_map(
                params![key.0.space.0, minimum.x, maximum.x, minimum.z, maximum.z],
                |r| {
                    Ok(TerrainMaterialKey(TerrainNodeKey::leaf(
                        key.0.space,
                        CellCoord {
                            x: r.get(0)?,
                            z: r.get(1)?,
                        },
                    )))
                },
            )?
            .collect::<Result<_, _>>()?)
    }
    /// The finest level holding a root, if the space has any.
    pub fn lowest_root_level(&self, space: WorldSpaceId) -> Result<Option<u8>, WorldDbError> {
        Ok(self.reader.connection.query_row(
            "SELECT min(level) FROM terrain_roots WHERE world_space_id=?1",
            [space.0],
            |r| r.get(0),
        )?)
    }
    /// Roots stay published at any level: they are the resident fallback cover.
    pub fn is_root(&self, key: TerrainMaterialKey) -> Result<bool, WorldDbError> {
        let k = key.0;
        Ok(self.reader.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM terrain_roots WHERE world_space_id=?1 AND level=?2 AND node_x=?3 AND node_z=?4)",
            params![k.space.0, k.level, k.x, k.z],
            |r| r.get(0),
        )?)
    }
    /// Stores a core compressed elsewhere, for example on a worker thread.
    pub fn put_core_payload(
        &self,
        key: TerrainMaterialKey,
        core: &StagedCore,
    ) -> Result<(), WorldDbError> {
        key.0.cell_bounds().map_err(|e| invalid(e.to_string()))?;
        let k = key.0;
        self.reader.connection.execute(
            "INSERT OR REPLACE INTO composite_cores VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                k.space.0,
                k.level,
                k.x,
                k.z,
                core.decoded_bytes as i64,
                core.payload
            ],
        )?;
        Ok(())
    }
    pub fn core(&self, key: TerrainMaterialKey) -> Result<Option<Vec<u8>>, WorldDbError> {
        self.core_payload(key)?
            .map(|core| core.decompress())
            .transpose()
    }
    /// The still-compressed core, so callers can decompress it on another thread.
    pub fn core_payload(
        &self,
        key: TerrainMaterialKey,
    ) -> Result<Option<StagedCore>, WorldDbError> {
        let k = key.0;
        Ok(self.reader.connection.query_row("SELECT decoded_bytes,payload FROM composite_cores WHERE world_space_id=?1 AND level=?2 AND node_x=?3 AND node_z=?4",params![k.space.0,k.level,k.x,k.z], |r| {
            let bytes = r.get_ref(1)?.as_blob()?;
            if bytes.len() > MAX_CORE_BYTES { return Err(rusqlite::Error::InvalidQuery); }
            Ok(StagedCore { decoded_bytes: r.get::<_,i64>(0)? as u64, payload: bytes.to_vec() })
        }).optional()?)
    }
    pub fn insert(&self, tile: &TerrainComposite) -> Result<(), WorldDbError> {
        self.insert_prepared(&PreparedComposite::new(tile)?)
    }
    /// Stores a composite encoded elsewhere, for example on a worker thread, replacing the
    /// node's previous one.
    pub fn insert_prepared(&self, tile: &PreparedComposite) -> Result<(), WorldDbError> {
        let k = tile.key.0;
        self.reader.connection.execute("INSERT OR REPLACE INTO terrain_composites(world_space_id,level,node_x,node_z,fingerprint,checksum,decoded_bytes,gpu_bytes,payload) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![k.space.0,k.level,k.x,k.z,tile.fingerprint.as_slice(),tile.checksum.as_slice(),tile.decoded_bytes as i64,TerrainComposite::gpu_bytes() as i64,tile.payload])?;
        Ok(())
    }
    pub fn finish(self, library_fingerprint: &[u8; 32]) -> Result<RuntimeManifest, WorldDbError> {
        // Every drawable root and every drawable node at or above the profile's minimum level
        // has exactly one composite; finer non-root nodes have none.
        let mismatch: bool = self.reader.connection.query_row("SELECT EXISTS(SELECT 1 FROM terrain_nodes n LEFT JOIN terrain_composites c USING(world_space_id,level,node_x,node_z) LEFT JOIN terrain_roots r USING(world_space_id,level,node_x,node_z) LEFT JOIN world_space_terrain_profiles p USING(world_space_id) WHERE (n.resolution IS NOT NULL AND (n.level >= coalesce(p.composite_minimum_level,0) OR r.world_space_id IS NOT NULL)) != (c.payload IS NOT NULL))",[],|r| r.get(0))?;
        if mismatch {
            return Err(invalid("composite coverage disagrees with terrain"));
        }
        let uncored: bool = self.reader.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM terrain_composites c LEFT JOIN terrain_cores t \
             USING(world_space_id,level,node_x,node_z) WHERE t.fingerprint IS NULL)",
            [],
            |r| r.get(0),
        )?;
        if uncored {
            return Err(invalid("composite without a recorded core"));
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
        let content_hash = *hash.finalize().as_bytes();
        let generation =
            crate::gameplay_areas::generation_id(&self.reader.connection, &content_hash)?;
        self.reader.connection.execute(
            "UPDATE runtime_metadata SET generation_id=?1,content_hash=?2 WHERE singleton=1",
            params![generation, content_hash.as_slice()],
        )?;
        self.reader
            .connection
            .execute("DELETE FROM terrain_material_spaces", [])?;
        self.reader.connection.execute(
            "INSERT INTO terrain_material_spaces SELECT id, (SELECT count(*) FROM \
             terrain_composites c WHERE c.world_space_id=w.id), ?1 FROM world_spaces w",
            [library_fingerprint.as_slice()],
        )?;
        self.reader
            .connection
            .execute_batch("DROP TABLE composite_cores; COMMIT;")?;
        read_runtime_manifest(&self.reader.connection)
    }
}

impl RuntimeReader {
    /// `None` when the publication has no baked ground; otherwise the finest level whose
    /// non-root composites were published (see `TerrainProfile::composite_minimum_level`).
    pub fn terrain_composite_minimum_level(
        &self,
        space: WorldSpaceId,
    ) -> Result<Option<u8>, WorldDbError> {
        let baked: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM terrain_material_spaces WHERE world_space_id=?1)",
            [space.0],
            |r| r.get(0),
        )?;
        baked
            .then(|| composite_minimum_level(&self.connection, space))
            .transpose()
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
fn composite_minimum_level(
    connection: &Connection,
    space: WorldSpaceId,
) -> Result<u8, WorldDbError> {
    let level: Option<i64> = connection
        .query_row(
            "SELECT composite_minimum_level FROM world_space_terrain_profiles WHERE world_space_id=?1",
            [space.0],
            |r| r.get(0),
        )
        .optional()?;
    u8::try_from(level.unwrap_or(0))
        .ok()
        .filter(|&level| level <= world::MAX_TERRAIN_NODE_LEVEL)
        .ok_or_else(|| invalid("composite minimum level"))
}
fn descriptor(
    connection: &Connection,
    key: TerrainMaterialKey,
) -> Result<Option<TerrainCompositeDescriptor>, WorldDbError> {
    let k = key.0;
    k.cell_bounds().map_err(|e| invalid(e.to_string()))?;
    let d = connection.query_row("SELECT c.fingerprint,c.checksum,length(c.payload),c.decoded_bytes,c.gpu_bytes,n.minimum_y,n.maximum_y FROM terrain_composites c JOIN terrain_nodes n USING(world_space_id,level,node_x,node_z) WHERE world_space_id=?1 AND level=?2 AND node_x=?3 AND node_z=?4",params![k.space.0,k.level,k.x,k.z],|r| Ok(TerrainCompositeDescriptor {key,fingerprint:blob_array(r.get_ref(0)?.as_blob()?,"composite fingerprint")?,checksum:blob_array(r.get_ref(1)?.as_blob()?,"composite checksum")?,encoded_bytes:r.get::<_,i64>(2)? as u64,decoded_bytes:r.get::<_,i64>(3)? as u64,gpu_bytes:r.get::<_,i64>(4)? as u64,height_bounds:[r.get(5)?,r.get(6)?]})).optional()?;
    if let Some(d) = &d
        && (!d.height_bounds.iter().all(|v| v.is_finite())
            || d.height_bounds[0] > d.height_bounds[1]
            || !(1..=MAX_TERRAIN_COMPOSITE_BYTES as u64).contains(&d.decoded_bytes)
            || !(1..=MAX_TERRAIN_COMPOSITE_BYTES as u64).contains(&d.encoded_bytes)
            || d.gpu_bytes != TerrainComposite::gpu_bytes() as u64)
    {
        return Err(invalid("composite descriptor byte limits"));
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
