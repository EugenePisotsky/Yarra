//! Bounded terrain-node IO. A cook store only mutates an unpublished staging generation.
use crate::runtime::{read_page_connection, read_runtime_manifest};
use crate::storage::{blob_array, ensure_schema_version};
use crate::{EncodedPage, RuntimeManifest, RuntimeReader, WorldDbError, WorldSpaceRecord};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::io::{Cursor, Read};
use std::path::Path;
use world::{
    CellCoord, MAX_TERRAIN_NODE_BYTES, PageCodec, PageDomain, PageKey, RUNTIME_SCHEMA_VERSION,
    TerrainNode, TerrainNodeKey, WorldSpaceId, decode_terrain_node, encode_terrain_node,
};

pub const MAX_TERRAIN_NODE_QUERY: usize = 128;
pub const MAX_TERRAIN_COARSE_ROOTS: usize = 256;

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainNodeDescriptor {
    pub key: TerrainNodeKey,
    pub child_mask: u8,
    pub height_bounds: [f32; 2],
    pub geometric_error: f32,
    pub resolution: Option<u16>,
    pub decoded_bytes: u64,
    pub gpu_bytes_estimate: u64,
    pub checksum: [u8; 32],
}
#[derive(Debug, Clone)]
pub struct EncodedTerrainNode {
    pub descriptor: TerrainNodeDescriptor,
    pub codec: PageCodec,
    pub payload: Vec<u8>,
}
impl EncodedTerrainNode {
    pub fn decode(self) -> Result<TerrainNode, WorldDbError> {
        if self.descriptor.decoded_bytes > MAX_TERRAIN_NODE_BYTES as u64
            || self.payload.len() > MAX_TERRAIN_NODE_BYTES
        {
            return Err(invalid("terrain node size limit"));
        }
        let bytes = match self.codec {
            PageCodec::Raw => self.payload,
            PageCodec::Zstd => {
                let mut bytes = Vec::new();
                zstd::stream::read::Decoder::new(Cursor::new(self.payload))?
                    .take(MAX_TERRAIN_NODE_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)?;
                bytes
            }
        };
        if bytes.len() as u64 != self.descriptor.decoded_bytes
            || *blake3::hash(&bytes).as_bytes() != self.descriptor.checksum
        {
            return Err(invalid("terrain node size/checksum mismatch"));
        }
        let node = decode_terrain_node(&bytes).map_err(|e| invalid(e.to_string()))?;
        if node.key != self.descriptor.key
            || node.child_mask != self.descriptor.child_mask
            || node.height_bounds != self.descriptor.height_bounds
            || node.geometric_error != self.descriptor.geometric_error
            || node.heightfield.as_ref().map(|h| h.resolution) != self.descriptor.resolution
            || node.gpu_bytes_estimate() != self.descriptor.gpu_bytes_estimate
        {
            return Err(invalid("terrain descriptor disagrees with payload"));
        }
        Ok(node)
    }
}

#[derive(Debug, Clone)]
pub struct TerrainHierarchySummary {
    pub space: WorldSpaceId,
    pub nodes: u64,
    pub leaves: u64,
    pub roots: u64,
    pub root_decoded_bytes: u64,
    pub root_gpu_bytes: u64,
}

pub struct TerrainCookStore {
    connection: Connection,
}
impl TerrainCookStore {
    pub fn open_staging(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        ensure_schema_version(&connection, RUNTIME_SCHEMA_VERSION, "runtime")?;
        connection
            .execute_batch("PRAGMA foreign_keys=ON; PRAGMA cache_size=-8192; BEGIN IMMEDIATE;")?;
        let count: i64 = connection.query_row(
            "SELECT (SELECT count(*) FROM terrain_nodes)+(SELECT count(*) FROM \
             terrain_hierarchy_spaces)",
            [],
            |r| r.get(0),
        )?;
        if count != 0 {
            return Err(invalid("staging hierarchy is already populated"));
        }
        Ok(Self { connection })
    }
    fn insert(&self, node: &TerrainNode) -> Result<(), WorldDbError> {
        let bytes = encode_terrain_node(node).map_err(|e| invalid(e.to_string()))?;
        let compressed = zstd::stream::encode_all(Cursor::new(&bytes), 3)?;
        let (codec, payload) = if compressed.len() < bytes.len() {
            (PageCodec::Zstd, compressed.as_slice())
        } else {
            (PageCodec::Raw, bytes.as_slice())
        };
        let parent = node.key.parent().map_err(|e| invalid(e.to_string()))?;
        self.connection.execute(
            "INSERT INTO terrain_nodes( \
                 world_space_id, level, node_x, node_z, parent_x, parent_z, child_mask, \
                 minimum_y, maximum_y, geometric_error, resolution, codec, decoded_bytes, \
                 gpu_bytes_estimate, checksum, payload \
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
            params![
                node.key.space.0,
                node.key.level,
                node.key.x,
                node.key.z,
                parent.map(|p| p.x),
                parent.map(|p| p.z),
                node.child_mask,
                node.height_bounds[0],
                node.height_bounds[1],
                node.geometric_error,
                node.heightfield.as_ref().map(|h| h.resolution),
                codec as i64,
                bytes.len() as i64,
                node.gpu_bytes_estimate() as i64,
                blake3::hash(&bytes).as_bytes().as_slice(),
                payload,
            ],
        )?;
        Ok(())
    }
    pub fn insert_leaf(
        &self,
        key: TerrainNodeKey,
        field: &world::TerrainHeightfield,
        resolution: u16,
    ) -> Result<(), WorldDbError> {
        let node = TerrainNode::leaf(key, field, resolution).map_err(|e| invalid(e.to_string()))?;
        let exists: bool = self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM cell_pages WHERE world_space_id=?1 AND domain=1 AND lod=0 \
             AND cell_x=?2 AND cell_z=?3)",
            params![key.space.0, key.x, key.z],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(invalid("terrain leaf has no source page"));
        }
        self.insert(&node)
    }
    /// Children are loaded and released per parent; no whole level of sample data is retained.
    pub fn build_parent(&self, key: TerrainNodeKey) -> Result<(), WorldDbError> {
        let keys = key
            .children()
            .map_err(|e| invalid(e.to_string()))?
            .ok_or(invalid("parent level"))?;
        let children = keys
            .map(|key| self.read(key))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let node = TerrainNode::parent(key, std::array::from_fn(|i| children[i].as_ref()))
            .map_err(|e| invalid(e.to_string()))?;
        self.insert(&node)
    }
    pub fn contains(&self, key: TerrainNodeKey) -> Result<bool, WorldDbError> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM terrain_nodes WHERE world_space_id=?1 AND level=?2 AND \
             node_x=?3 AND node_z=?4)",
            params![key.space.0, key.level, key.x, key.z],
            |r| r.get(0),
        )?)
    }
    pub fn read(&self, key: TerrainNodeKey) -> Result<Option<TerrainNode>, WorldDbError> {
        read_encoded(&self.connection, key)?
            .map(EncodedTerrainNode::decode)
            .transpose()
    }
    /// Keyset pagination returns all matching keys over successive calls; never a spatial truncation.
    pub fn keys(
        &self,
        space: WorldSpaceId,
        level: u8,
        after: Option<CellCoord>,
    ) -> Result<Vec<TerrainNodeKey>, WorldDbError> {
        let mut query = self.connection.prepare_cached(
            "SELECT node_x, node_z FROM terrain_nodes WHERE world_space_id=?1 AND level=?2 AND \
             (node_x, node_z)>(?3, ?4) ORDER BY node_x, node_z LIMIT ?5",
        )?;
        Ok(query
            .query_map(
                params![
                    space.0,
                    level,
                    after.map_or(i64::MIN, |c| i64::from(c.x)),
                    after.map_or(i64::MIN, |c| i64::from(c.z)),
                    MAX_TERRAIN_NODE_QUERY as i64
                ],
                |r| {
                    Ok(TerrainNodeKey {
                        space,
                        level,
                        x: r.get(0)?,
                        z: r.get(1)?,
                    })
                },
            )?
            .collect::<Result<_, _>>()?)
    }
    pub fn leaf_pages(
        &self,
        space: WorldSpaceId,
        after: Option<CellCoord>,
    ) -> Result<Vec<EncodedPage>, WorldDbError> {
        let mut query = self.connection.prepare_cached("SELECT cell_x, cell_z FROM cell_pages WHERE world_space_id=?1 AND domain=1 AND lod=0 AND \
             (cell_x, cell_z)>(?2, ?3) ORDER BY cell_x, cell_z LIMIT 1")?;
        let keys = query
            .query_map(
                params![
                    space.0,
                    after.map_or(i64::MIN, |c| i64::from(c.x)),
                    after.map_or(i64::MIN, |c| i64::from(c.z))
                ],
                |r| {
                    Ok(PageKey {
                        space,
                        cell: CellCoord {
                            x: r.get(0)?,
                            z: r.get(1)?,
                        },
                        domain: PageDomain::TerrainRender,
                        lod: 0,
                    })
                },
            )?
            .collect::<Result<Vec<_>, _>>()?;
        keys.into_iter()
            .map(|key| {
                read_page_connection(&self.connection, key)?.ok_or(invalid("missing staged leaf"))
            })
            .collect()
    }
    /// Seal a bounded coarse cover and fold all products into the generation's identity.
    pub fn finish(
        self,
        spaces: &[WorldSpaceRecord],
    ) -> Result<(RuntimeManifest, Vec<TerrainHierarchySummary>), WorldDbError> {
        let manifest = read_runtime_manifest(&self.connection)?;
        let supplied: std::collections::BTreeSet<_> = spaces.iter().map(|s| s.id).collect();
        let expected: std::collections::BTreeSet<_> =
            manifest.world_spaces.iter().map(|s| s.id).collect();
        if supplied != expected || supplied.len() != spaces.len() {
            return Err(invalid(
                "hierarchy world-space list disagrees with staged generation",
            ));
        }
        let mut summaries = Vec::new();
        for space in spaces {
            self.connection.execute(
                "INSERT INTO terrain_roots(world_space_id, level, node_x, node_z) SELECT \
             n.world_space_id, n.level, n.node_x, n.node_z FROM terrain_nodes n WHERE \
             n.world_space_id=?1 AND n.resolution IS NOT NULL AND NOT EXISTS (SELECT 1 FROM \
             terrain_nodes p WHERE p.world_space_id=n.world_space_id AND p.level=n.level+1 AND \
             p.node_x=n.parent_x AND p.node_z=n.parent_z AND p.resolution IS NOT NULL)",
                [space.id.0],
            )?;
            let (roots, decoded, gpu): (i64, i64, i64) = self.connection.query_row(
                "SELECT count(*), coalesce(sum(n.decoded_bytes), 0), \
                     coalesce(sum(n.gpu_bytes_estimate), 0) \
                 FROM terrain_roots r JOIN terrain_nodes n \
                     USING(world_space_id, level, node_x, node_z) \
                 WHERE r.world_space_id=?1",
                [space.id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if roots > MAX_TERRAIN_COARSE_ROOTS as i64
                || decoded > 32 * 1024 * 1024
                || gpu > 128 * 1024 * 1024
            {
                return Err(invalid(
                    "coarse terrain cover exceeds count or memory budget",
                ));
            }
            let (nodes, leaves): (i64, i64) = self.connection.query_row(
                "SELECT count(*), coalesce(sum(level=0), 0) FROM terrain_nodes \
                 WHERE world_space_id=?1",
                [space.id.0],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let expected: i64 = self.connection.query_row(
                "SELECT count(*) FROM cell_pages WHERE world_space_id=?1 AND domain=1 AND lod=0",
                [space.id.0],
                |r| r.get(0),
            )?;
            let covered: i64 = self.connection.query_row(
                "SELECT coalesce(sum(1 << (2*level)), 0) FROM terrain_roots WHERE world_space_id=?1",
                [space.id.0],
                |r| r.get(0),
            )?;
            if leaves != expected || leaves != covered {
                return Err(invalid("terrain hierarchy does not cover staged terrain"));
            }
            self.connection.execute(
                "INSERT INTO terrain_hierarchy_spaces( \
                     world_space_id, node_count, leaf_count, root_count \
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![space.id.0, nodes, leaves, roots],
            )?;
            summaries.push(TerrainHierarchySummary {
                space: space.id,
                nodes: nodes as u64,
                leaves: leaves as u64,
                roots: roots as u64,
                root_decoded_bytes: decoded as u64,
                root_gpu_bytes: gpu as u64,
            });
        }
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"terrain-hierarchy-v1");
        hasher.update(&read_runtime_manifest(&self.connection)?.content_hash);
        {
            let mut query = self.connection.prepare(
                "SELECT checksum FROM terrain_nodes ORDER BY world_space_id, level, node_x, node_z",
            )?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                hasher.update(row.get_ref(0)?.as_blob().map_err(rusqlite::Error::from)?);
            }
        }
        let hash = hasher.finalize();
        self.connection.execute(
            "UPDATE runtime_metadata SET generation_id=?1, content_hash=?2 WHERE singleton=1",
            params![&hash.to_hex()[..16], hash.as_bytes().as_slice()],
        )?;
        self.connection.execute_batch("COMMIT;")?;
        let manifest = read_runtime_manifest(&self.connection)?;
        Ok((manifest, summaries))
    }
}

impl RuntimeReader {
    pub fn read_terrain_roots(
        &self,
        space: WorldSpaceId,
    ) -> Result<Vec<TerrainNodeDescriptor>, WorldDbError> {
        let expected: Option<i64> = self
            .connection
            .query_row(
                "SELECT root_count FROM terrain_hierarchy_spaces WHERE world_space_id=?1",
                [space.0],
                |r| r.get(0),
            )
            .optional()?;
        let expected = expected.ok_or(invalid("terrain hierarchy has not been cooked"))?;
        if expected < 0 || expected > MAX_TERRAIN_COARSE_ROOTS as i64 {
            return Err(invalid("terrain root count limit"));
        }
        let mut query=self.connection.prepare_cached("SELECT level, node_x, node_z FROM terrain_roots WHERE world_space_id=?1 ORDER BY level, \
             node_x, node_z LIMIT ?2")?;
        let keys = query
            .query_map(params![space.0, MAX_TERRAIN_COARSE_ROOTS as i64 + 1], |r| {
                Ok(TerrainNodeKey {
                    space,
                    level: r.get(0)?,
                    x: r.get(1)?,
                    z: r.get(2)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        if keys.len() as i64 != expected {
            return Err(invalid("terrain root index mismatch"));
        }
        let roots: Vec<_> = keys
            .into_iter()
            .map(|key| {
                read_descriptor(&self.connection, key)?.ok_or(invalid("missing terrain root"))
            })
            .collect::<Result<_, _>>()?;
        if roots.iter().any(|r| r.resolution.is_none())
            || roots.iter().map(|r| r.decoded_bytes).sum::<u64>() > 32 * 1024 * 1024
            || roots.iter().map(|r| r.gpu_bytes_estimate).sum::<u64>() > 128 * 1024 * 1024
        {
            return Err(invalid("terrain root memory/coverage limit"));
        }
        Ok(roots)
    }
    pub fn read_terrain_node_descriptors(
        &self,
        keys: &[TerrainNodeKey],
    ) -> Result<Vec<Option<TerrainNodeDescriptor>>, WorldDbError> {
        if keys.len() > MAX_TERRAIN_NODE_QUERY {
            return Err(invalid("terrain metadata query limit"));
        }
        keys.iter()
            .map(|&key| read_descriptor(&self.connection, key))
            .collect()
    }
    pub fn read_terrain_node(
        &self,
        key: TerrainNodeKey,
    ) -> Result<Option<EncodedTerrainNode>, WorldDbError> {
        read_encoded(&self.connection, key)
    }
}

fn read_descriptor(
    c: &Connection,
    key: TerrainNodeKey,
) -> Result<Option<TerrainNodeDescriptor>, WorldDbError> {
    key.cell_bounds().map_err(|e| invalid(e.to_string()))?;
    let descriptor: Option<TerrainNodeDescriptor> = c
        .query_row(
            "SELECT child_mask, minimum_y, maximum_y, geometric_error, resolution, decoded_bytes, \
             gpu_bytes_estimate, checksum FROM terrain_nodes \
         WHERE world_space_id=?1 AND level=?2 AND node_x=?3 AND node_z=?4",
            params![key.space.0, key.level, key.x, key.z],
            |r| {
                Ok(TerrainNodeDescriptor {
                    key,
                    child_mask: r.get(0)?,
                    height_bounds: [r.get(1)?, r.get(2)?],
                    geometric_error: r.get(3)?,
                    resolution: r.get(4)?,
                    decoded_bytes: r.get::<_, i64>(5)? as u64,
                    gpu_bytes_estimate: r.get::<_, i64>(6)? as u64,
                    checksum: blob_array(r.get_ref(7)?.as_blob()?, "terrain checksum")?,
                })
            },
        )
        .optional()?;
    if let Some(d) = &descriptor {
        let expected_gpu = if let Some(n) = d.resolution {
            if !(2..=world::MAX_TERRAIN_HEIGHTFIELD_RESOLUTION).contains(&n)
                || !(n - 1).is_power_of_two()
            {
                return Err(invalid("terrain descriptor resolution"));
            }
            let n = u64::from(n);
            n * n * 48 + (n - 1) * (n - 1) * 6 * 4
        } else {
            0
        };
        if d.child_mask > 15
            || (key.level == 0) != (d.child_mask == 0)
            || (key.level == 0 && (d.resolution.is_none() || d.geometric_error != 0.0))
            || (key.level > 0 && d.resolution.is_some() && d.child_mask != 15)
            || (d.resolution.is_none() && d.geometric_error != 0.0)
            || d.gpu_bytes_estimate != expected_gpu
            || d.decoded_bytes == 0
            || !d.height_bounds.iter().all(|v| v.is_finite())
            || d.height_bounds[0] > d.height_bounds[1]
            || !d.geometric_error.is_finite()
            || d.geometric_error < 0.0
            || d.decoded_bytes > MAX_TERRAIN_NODE_BYTES as u64
        {
            return Err(invalid("terrain descriptor bounds/size"));
        }
    }
    Ok(descriptor)
}
fn read_encoded(
    c: &Connection,
    key: TerrainNodeKey,
) -> Result<Option<EncodedTerrainNode>, WorldDbError> {
    let Some(descriptor) = read_descriptor(c, key)? else {
        return Ok(None);
    };
    let (codec, payload) = c.query_row(
        "SELECT codec, payload FROM terrain_nodes WHERE world_space_id=?1 AND level=?2 AND \
             node_x=?3 AND node_z=?4",
        params![key.space.0, key.level, key.x, key.z],
        |r| {
            let bytes = r.get_ref(1)?.as_blob()?;
            if bytes.len() > MAX_TERRAIN_NODE_BYTES {
                return Err(rusqlite::Error::InvalidQuery);
            }
            let codec = PageCodec::try_from(r.get::<_, i64>(0)?)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            Ok((codec, bytes.to_vec()))
        },
    )?;
    Ok(Some(EncodedTerrainNode {
        descriptor,
        codec,
        payload,
    }))
}
fn invalid(message: impl Into<String>) -> WorldDbError {
    WorldDbError::TerrainHierarchy(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RuntimeBuild, RuntimeCellRecord, domain_bit, write_runtime_database};
    use std::{fs, path::PathBuf};
    use world::PagePayload;
    use world::{TerrainHeightfield, TerrainHeightfieldPage, encode_page_payload};

    struct Fixture {
        path: PathBuf,
        build: RuntimeBuild,
        field: TerrainHeightfield,
    }
    impl Fixture {
        fn new(count: i32) -> Self {
            static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "yarra-terrain-nodes-{}-{}.sqlite",
                std::process::id(),
                SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let field = TerrainHeightfield::from_heights(2, &[0.0; 4], -1.0, 1.0, 8.0).unwrap();
            let payload =
                encode_page_payload(&PagePayload::TerrainHeightfield(TerrainHeightfieldPage {
                    heightfield: field.clone(),
                    surfaces: vec![],
                    weight_pages: vec![],
                }))
                .unwrap();
            let space = WorldSpaceId(1);
            let build = RuntimeBuild {
                manifest: RuntimeManifest {
                    schema_version: RUNTIME_SCHEMA_VERSION,
                    generation_id: "before".into(),
                    content_hash: [7; 32],
                    default_world_space: space,
                    world_spaces: vec![WorldSpaceRecord {
                        atmosphere: Default::default(),
                        atmosphere_revision: 1,
                        id: space,
                        name: "test".into(),
                        cell_size: 8.0,
                        minimum_y: -1.0,
                        maximum_y: 1.0,
                    }],
                    vegetation_catalog: None,
                },
                cells: (0..count)
                    .map(|x| RuntimeCellRecord {
                        space,
                        cell: CellCoord { x, z: 0 },
                        minimum_y: 0.0,
                        maximum_y: 0.0,
                        source_revision: 1,
                        domain_mask: domain_bit(PageDomain::TerrainRender),
                    })
                    .collect(),
                pages: (0..count)
                    .map(|x| EncodedPage {
                        key: PageKey {
                            space,
                            cell: CellCoord { x, z: 0 },
                            domain: PageDomain::TerrainRender,
                            lod: 0,
                        },
                        codec: PageCodec::Raw,
                        decoded_bytes: payload.len() as u64,
                        gpu_bytes_estimate: 0,
                        checksum: *blake3::hash(&payload).as_bytes(),
                        payload: payload.clone(),
                    })
                    .collect(),
                terrain_surfaces: vec![],
                terrain_texture_sets: vec![],
                terrain_texture_layers: vec![],
                terrain_profiles: vec![],
                assets: vec![],
                definitions: vec![],
                dependencies: vec![],
                definition_dependencies: vec![],
                terrain_surface_dependencies: vec![],
            };
            write_runtime_database(&path, &build).unwrap();
            Self { path, build, field }
        }
        fn key(x: i32) -> TerrainNodeKey {
            TerrainNodeKey::leaf(WorldSpaceId(1), CellCoord { x, z: 0 })
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    #[test]
    fn node_reads_are_bounded_and_reject_corruption() {
        let fixture = Fixture::new(2);
        let store = TerrainCookStore::open_staging(&fixture.path).unwrap();
        for x in 0..2 {
            store
                .insert_leaf(Fixture::key(x), &fixture.field, 33)
                .unwrap();
        }
        let parent = Fixture::key(0).parent().unwrap().unwrap();
        store.build_parent(parent).unwrap();
        assert!(store.read(parent).unwrap().unwrap().heightfield.is_none());
        let (manifest, summary) = store.finish(&fixture.build.manifest.world_spaces).unwrap();
        assert_ne!(manifest.content_hash, fixture.build.manifest.content_hash);
        assert_eq!(
            (summary[0].nodes, summary[0].leaves, summary[0].roots),
            (3, 2, 2)
        );
        let reader = RuntimeReader::open_immutable(&fixture.path).unwrap();
        assert_eq!(reader.read_terrain_roots(WorldSpaceId(1)).unwrap().len(), 2);
        assert!(
            reader
                .read_terrain_node_descriptors(&[Fixture::key(0); 129])
                .is_err()
        );
        assert!(reader.read_terrain_node(Fixture::key(3)).unwrap().is_none());
        let encoded = reader.read_terrain_node(Fixture::key(0)).unwrap().unwrap();
        assert_eq!(
            encoded
                .clone()
                .decode()
                .unwrap()
                .heightfield
                .unwrap()
                .resolution,
            33
        );
        let mut truncated = encoded.clone();
        truncated.payload.pop();
        assert!(truncated.decode().is_err());
        let mut oversized = encoded.clone();
        oversized.descriptor.decoded_bytes = MAX_TERRAIN_NODE_BYTES as u64 + 1;
        assert!(oversized.decode().is_err());
        let mut mismatch = encoded;
        mismatch.descriptor.height_bounds[1] = 10.0;
        assert!(mismatch.decode().is_err());
        assert!(TerrainCookStore::open_staging(&fixture.path).is_err());
    }

    #[test]
    fn interrupted_or_incomplete_cook_does_not_seal_or_change_generation() {
        let fixture = Fixture::new(2);
        {
            let store = TerrainCookStore::open_staging(&fixture.path).unwrap();
            store
                .insert_leaf(Fixture::key(0), &fixture.field, 2)
                .unwrap();
        }
        let store = TerrainCookStore::open_staging(&fixture.path).unwrap();
        assert!(
            store
                .insert_leaf(Fixture::key(7), &fixture.field, 2)
                .is_err()
        );
        store
            .insert_leaf(Fixture::key(0), &fixture.field, 2)
            .unwrap();
        assert!(store.finish(&fixture.build.manifest.world_spaces).is_err());
        let reader = RuntimeReader::open_immutable(&fixture.path).unwrap();
        assert_eq!(reader.manifest().generation_id, "before");
        assert!(reader.read_terrain_roots(WorldSpaceId(1)).is_err());
        assert!(reader.read_terrain_node(Fixture::key(0)).unwrap().is_none());
    }

    #[test]
    fn paginated_keys_do_not_silently_drop_terrain_and_coarse_cover_is_limited() {
        let fixture = Fixture::new(257);
        let store = TerrainCookStore::open_staging(&fixture.path).unwrap();
        for x in 0..257 {
            store
                .insert_leaf(Fixture::key(x), &fixture.field, 2)
                .unwrap();
        }
        let mut cursor = None;
        let mut count = 0;
        loop {
            let keys = store.keys(WorldSpaceId(1), 0, cursor).unwrap();
            if keys.is_empty() {
                break;
            }
            assert!(keys.len() <= MAX_TERRAIN_NODE_QUERY);
            count += keys.len();
            cursor = keys.last().map(|k| CellCoord { x: k.x, z: k.z });
        }
        assert_eq!(count, 257);
        assert!(
            store
                .finish(&fixture.build.manifest.world_spaces)
                .unwrap_err()
                .to_string()
                .contains("coarse terrain cover")
        );
        assert!(
            RuntimeReader::open_immutable(&fixture.path)
                .unwrap()
                .read_terrain_roots(WorldSpaceId(1))
                .is_err()
        );
    }
}
