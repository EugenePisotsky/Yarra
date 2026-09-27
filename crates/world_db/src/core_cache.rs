//! Cook-only cache of composite filtering cores at every baked level, keyed by node and core
//! fingerprint. A core's fingerprint hashes everything its evaluation reads, so a
//! matching entry is exactly the core the bake would compute. It lives beside the project,
//! never ships, keeps one entry per node and can be deleted at any time.
use crate::{StagedCore, WorldDbError};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;
use world::TerrainMaterialKey;

const CACHE_VERSION: i64 = 1;

pub struct CoreCache {
    connection: Connection,
}

impl CoreCache {
    pub fn open(path: &Path) -> Result<Self, WorldDbError> {
        let connection = Connection::open(path)?;
        connection.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version != CACHE_VERSION {
            connection.execute_batch("DROP TABLE IF EXISTS cores;")?;
        }
        connection.execute_batch(&format!(
            "CREATE TABLE IF NOT EXISTS cores (
                world_space_id INTEGER NOT NULL,
                level INTEGER NOT NULL,
                node_x INTEGER NOT NULL,
                node_z INTEGER NOT NULL,
                fingerprint BLOB NOT NULL CHECK(length(fingerprint) = 32),
                decoded_bytes INTEGER NOT NULL,
                payload BLOB NOT NULL,
                PRIMARY KEY(world_space_id, level, node_x, node_z)
            ) STRICT, WITHOUT ROWID;
            PRAGMA user_version = {CACHE_VERSION};"
        ))?;
        Ok(Self { connection })
    }

    /// The cached core for `key`, if it was computed from inputs with this fingerprint.
    pub fn get(
        &self,
        key: TerrainMaterialKey,
        fingerprint: &[u8; 32],
    ) -> Result<Option<StagedCore>, WorldDbError> {
        let k = key.0;
        Ok(self
            .connection
            .prepare_cached(
                "SELECT decoded_bytes, payload FROM cores WHERE world_space_id = ?1 AND level = ?2 \
                 AND node_x = ?3 AND node_z = ?4 AND fingerprint = ?5",
            )?
            .query_row(
                params![k.space.0, k.level, k.x, k.z, fingerprint.as_slice()],
                |r| {
                    Ok(StagedCore::from_parts(
                        r.get::<_, i64>(0)? as u64,
                        r.get(1)?,
                    ))
                },
            )
            .optional()?)
    }

    pub fn contains(
        &self,
        key: TerrainMaterialKey,
        fingerprint: &[u8; 32],
    ) -> Result<bool, WorldDbError> {
        let k = key.0;
        Ok(self
            .connection
            .prepare_cached(
                "SELECT EXISTS(SELECT 1 FROM cores WHERE world_space_id = ?1 AND level = ?2 \
                 AND node_x = ?3 AND node_z = ?4 AND fingerprint = ?5)",
            )?
            .query_row(
                params![k.space.0, k.level, k.x, k.z, fingerprint.as_slice()],
                |r| r.get(0),
            )?)
    }

    /// Replaces whatever was cached for `key`.
    pub fn put(
        &self,
        key: TerrainMaterialKey,
        fingerprint: &[u8; 32],
        core: &StagedCore,
    ) -> Result<(), WorldDbError> {
        let k = key.0;
        let (decoded_bytes, payload) = core.parts();
        self.connection
            .prepare_cached("INSERT OR REPLACE INTO cores VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")?
            .execute(params![
                k.space.0,
                k.level,
                k.x,
                k.z,
                fingerprint.as_slice(),
                decoded_bytes as i64,
                payload
            ])?;
        Ok(())
    }
}
