//! Read-only access to one published content bundle. Opening reads only the manifest; assets
//! are read, checksummed and decoded when asked for. Reads are synchronous.
use crate::{
    BUNDLE_APPLICATION_ID, BUNDLE_SCHEMA_VERSION, ContentError, Result, asset::*,
    bundle::BundleManifest,
};
use game_types::require;
use rusqlite::{Connection, OpenFlags, params};
use std::path::Path;

pub struct ContentRepository {
    pub(crate) connection: Connection,
    manifest: BundleManifest,
}
impl ContentRepository {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        require(
            std::fs::metadata(&path)?.is_file(),
            "bundle is not a regular file",
        )?;
        let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        // One read transaction for the repository's lifetime: every read sees the same bundle.
        connection.execute_batch("PRAGMA query_only=ON; PRAGMA trusted_schema=OFF; BEGIN;")?;
        let application: i64 =
            connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
        if application != BUNDLE_APPLICATION_ID {
            return Err(ContentError::WrongDatabase);
        }
        let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != BUNDLE_SCHEMA_VERSION {
            return Err(ContentError::BundleVersion(version));
        }
        let bytes: Vec<u8> = connection.query_row(
            "SELECT payload FROM bundle_manifest WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        let manifest: BundleManifest = serde_json::from_slice(&bytes)?;
        manifest.validate()?;
        Ok(Self {
            connection,
            manifest,
        })
    }
    pub fn manifest(&self) -> &BundleManifest {
        &self.manifest
    }
    /// Headers of one kind in identity order, without reading payloads. For tools.
    pub fn headers(&self, kind: AssetKind) -> Result<Vec<AssetHeader>> {
        let mut stmt = self
            .connection
            .prepare("SELECT id,position,byte_len,hash FROM assets WHERE kind=?1 ORDER BY id")?;
        let mut rows = stmt.query([kind as i64])?;
        let mut headers = Vec::new();
        while let Some(row) = rows.next()? {
            headers.push(AssetHeader {
                id: AssetId::from_key(kind, row.get(0)?)?,
                position: row.get(1)?,
                payload_bytes: row.get::<_, u32>(2)? as usize,
                hash: row
                    .get_ref(3)?
                    .as_blob()
                    .map_err(rusqlite::Error::from)?
                    .try_into()
                    .map_err(|_| game_types::Invalid("invalid asset hash".into()))?,
            });
        }
        Ok(headers)
    }
    /// One asset, verified against its checksum and identity.
    pub fn read(&self, id: &AssetId) -> Result<Asset> {
        let key = id.key();
        self.read_range(id.kind(), &key, &format!("{key}\0"))?
            .pop()
            .ok_or_else(|| ContentError::MissingAsset(id.clone()))
    }
    /// Every asset of a kind, in authored order.
    pub fn read_kind(&self, kind: AssetKind) -> Result<Vec<Asset>> {
        // Every key sorts below this bound.
        self.read_range(kind, "", "\u{10ffff}")
    }
    /// Assets of one kind with `from <= key < to`, in authored order.
    pub(crate) fn read_range(&self, kind: AssetKind, from: &str, to: &str) -> Result<Vec<Asset>> {
        let mut stmt = self.connection.prepare(
            "SELECT id,hash,payload FROM assets WHERE kind=?1 AND id>=?2 AND id<?3 ORDER BY position",
        )?;
        let mut rows = stmt.query(params![kind as i64, from, to])?;
        let mut assets = Vec::new();
        while let Some(row) = rows.next()? {
            let id = AssetId::from_key(kind, row.get(0)?)?;
            let hash = row.get_ref(1)?.as_blob().map_err(rusqlite::Error::from)?;
            let payload = row.get_ref(2)?.as_blob().map_err(rusqlite::Error::from)?;
            if blake3::hash(payload).as_bytes() != hash {
                return Err(corrupt(&id, "payload checksum mismatch"));
            }
            let asset: Asset =
                serde_json::from_slice(payload).map_err(|e| corrupt(&id, e.to_string()))?;
            if asset.id() != id {
                return Err(corrupt(&id, "payload identity mismatch"));
            }
            asset
                .validate_local()
                .map_err(|e| corrupt(&id, e.to_string()))?;
            assets.push(asset);
        }
        Ok(assets)
    }
}
